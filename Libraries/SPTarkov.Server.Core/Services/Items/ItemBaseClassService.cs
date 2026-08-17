using System.Reflection;
using HarmonyLib;
using SPTarkov.Common.Models.Logging;
using SPTarkov.DI.Annotations;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Common.Tables;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Native;
using SPTarkov.Server.Core.Native.BaseClass;
using SPTarkov.Server.Core.Services.Locales;

namespace SPTarkov.Server.Core.Services.Items;

/// <summary>
///     Cache the baseids for each item in the items db inside a dictionary
///
///     The cache build runs in <c>rust/spt-native</c> by default; the request builder projects the
///     live items table into the native payload. The full 4.1.2 C# implementation is retained below
///     as the legacy path - it is the frozen mod contract (constructor and protected members are
///     apicompat-gated against the 4.1.2 baseline) and runs instead of the native path when a Harmony
///     patch on any frozen member is detected, when a mod substituted the service, when the frozen
///     constructor built the instance or when ItemConfig.ForceLegacyItemBaseClassHydration is set, so
///     mod hooks fire with genuine baseline semantics.
/// </summary>
[Injectable(InjectionType.Singleton)]
public class ItemBaseClassService(
    ISptLogger<ItemBaseClassService> logger,
    TemplateTable templateTable,
    ServerLocalisationService serverLocalisationService
)
{
    /// <summary>
    /// Key = Item tpl, values = Ids of its parents
    /// </summary>
    private Dictionary<MongoId, HashSet<MongoId>> _itemBaseClassesCache = [];
    private readonly Lock _itemBaseClassesLock = new();
    private readonly HashSet<MongoId> _rootNodeIds = [];

    private readonly ItemBaseClassNativeRequestBuilder? _requestBuilder;

    /// <summary>
    ///     Only set beside <see cref="_requestBuilder"/> by the additive constructor, and only read
    ///     once that builder has been found non-null.
    /// </summary>
    private readonly ItemConfig? _itemConfig;

    /// <summary>
    ///     The constructor the container uses: the frozen 4.1.2 one plus the native request builder.
    ///     Additive and apicompat-verified.
    /// </summary>
    public ItemBaseClassService(
        ISptLogger<ItemBaseClassService> logger,
        TemplateTable templateTable,
        ServerLocalisationService serverLocalisationService,
        ItemBaseClassNativeRequestBuilder requestBuilder,
        ItemConfig itemConfig
    )
        : this(logger, templateTable, serverLocalisationService)
    {
        _requestBuilder = requestBuilder;
        _itemConfig = itemConfig;
    }

    /// <summary>
    ///     Which implementation the most recent hydrate call ran - the spt-native path or the
    ///     retained 4.1.2 C# path. Test seam; also handy in a debugger.
    /// </summary>
    internal LootGenerationPath LastPathTaken { get; private set; }

    /// <summary>
    ///     The built cache, for parity tests to compare both paths' output.
    /// </summary>
    internal IReadOnlyDictionary<MongoId, HashSet<MongoId>> CacheForTests
    {
        get { return _itemBaseClassesCache; }
    }

    /// <summary>
    ///     The tpls that failed the <c>_type == "Item"</c> test, for parity tests.
    /// </summary>
    internal IReadOnlyCollection<MongoId> RootNodeIdsForTests
    {
        get { return _rootNodeIds; }
    }

    /// <summary>
    ///     The 4.1.2 members a mod can Harmony-patch. Public, protected and protected-internal
    ///     methods declared on this class - exactly the surface the apicompat gate freezes, statics
    ///     included. <see cref="HydrateItemBaseClassCache"/> is excluded: it is the dispatcher now,
    ///     and a patch on it wraps whichever path runs. Everything else is never called natively, so
    ///     a patch on one would silently do nothing.
    /// </summary>
    private static readonly List<MethodBase> _hookableMembers =
    [
        .. typeof(ItemBaseClassService)
            .GetMethods(
                BindingFlags.Instance | BindingFlags.Static | BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.DeclaredOnly
            )
            // Property accessors and operators are IsSpecialName; constructors are not returned at all
            .Where(method => !method.IsSpecialName && (method.IsPublic || method.IsFamily || method.IsFamilyOrAssembly))
            .Where(method => method.Name != nameof(HydrateItemBaseClassCache)),
    ];

    /// <summary>
    ///     The legacy path runs when the frozen 4.1.2 constructor built this instance (it has no
    ///     native seam to dispatch to), when forced by config, when any of the frozen 4.1.2 members
    ///     carries a live Harmony patch, or when a mod has substituted the service itself - running
    ///     the retained C# implementation is the only way those hooks and replacements can take
    ///     effect with real baseline semantics.
    /// </summary>
    private bool UseLegacyPath()
    {
        if (_requestBuilder is null || _itemConfig?.ForceLegacyItemBaseClassHydration == true)
        {
            return true;
        }

        if (
            _hookableMembers.Any(member =>
                Harmony.GetPatchInfo(member) is { } patches
                && (
                    patches.Prefixes.Count > 0
                    || patches.Postfixes.Count > 0
                    || patches.Transpilers.Count > 0
                    || patches.Finalizers.Count > 0
                )
            )
        )
        {
            return true;
        }

        // A mod registered its own subclass with a higher TypePriority, so the container handed us
        // an implementation the native side does not have
        return GetType() != typeof(ItemBaseClassService);
    }

    /// <summary>
    ///     Create cache and store inside ItemBaseClassService <br />
    ///     Store a dict of an items tpl to the base classes it and its parents have
    /// </summary>
    public void HydrateItemBaseClassCache()
    {
        if (UseLegacyPath())
        {
            LastPathTaken = LootGenerationPath.Legacy;

            HydrateItemBaseClassCacheLegacy();

            return;
        }

        LastPathTaken = LootGenerationPath.Native;

        var result = SptNative.BuildItemBaseClassCache(_requestBuilder!.Build());

        lock (_itemBaseClassesLock)
        {
            _itemBaseClassesCache = result.ItemBaseClasses;
            // Quirk 1, ported verbatim: hydrate never clears _rootNodeIds - legacy's reset touches
            // only the cache dictionary, so root ids accumulate across re-hydrates. Union, never
            // replace.
            _rootNodeIds.UnionWith(result.RootNodeIds);
        }
    }

    private void HydrateItemBaseClassCacheLegacy()
    {
        // Clear existing cache
        _itemBaseClassesCache = [];

        foreach (var item in templateTable.Items)
        {
            AddItemToCache(item.Key);
        }
    }

    public void AddItemToCache(MongoId itemTpl)
    {
        var itemDb = templateTable.Items;

        if (!itemDb.TryGetValue(itemTpl, out var item))
        {
            logger.Error($"Could not add {itemTpl} to cache, it does not exist in the item database!");
            return;
        }

        lock (_itemBaseClassesLock)
        {
            if (string.Equals(item.Type, "Item", StringComparison.OrdinalIgnoreCase))
            {
                _itemBaseClassesCache.TryAdd(item.Id, []);
                AddBaseItems(item.Id, item);
            }
            else
            {
                _rootNodeIds.Add(item.Id);
            }
        }
    }

    /// <summary>
    ///     Helper method, recursively iterate through items parent items, finding and adding ids to dictionary
    /// </summary>
    /// <param name="itemIdToUpdate"> Item tpl to store base ids against in dictionary </param>
    /// <param name="item"> Item being checked </param>
    protected void AddBaseItems(MongoId itemIdToUpdate, TemplateItem item)
    {
        _itemBaseClassesCache[itemIdToUpdate].Add(item.Parent);
        templateTable.Items.TryGetValue(item.Parent, out var parent);

        if (parent is not null && !parent.Parent.IsEmpty)
        {
            AddBaseItems(itemIdToUpdate, parent);
        }
    }

    /// <summary>
    ///     Does item tpl inherit from the requested base class
    /// </summary>
    /// <param name="itemTpl"> ItemTpl item to check base classes of </param>
    /// <param name="baseClasses"> BaseClass base class to check for </param>
    /// <returns> true if item inherits from base class passed in </returns>
    public bool ItemHasBaseClass(MongoId itemTpl, IEnumerable<MongoId> baseClasses)
    {
        if (itemTpl.IsEmpty)
        {
            logger.Warning("Unable to check itemTpl base class as value passed is null");

            return false;
        }

        // The cache is only generated for item templates with `_type == "Item"`, so return false for any other type,
        // including item templates that simply don't exist.
        if (_rootNodeIds.Contains(itemTpl))
        {
            return false;
        }

        var existsInCache = _itemBaseClassesCache.TryGetValue(itemTpl, out var baseClassList);
        if (!existsInCache)
        {
            // Not found in cache, attempt to add first
            AddItemToCache(itemTpl);

            existsInCache = _itemBaseClassesCache.TryGetValue(itemTpl, out baseClassList);
        }

        if (existsInCache)
        {
            return baseClassList.Overlaps(baseClasses);
        }

        logger.Warning(serverLocalisationService.GetText("baseclass-item_not_found_failed", itemTpl.ToString()));

        return false;
    }

    /// <summary>
    ///     Does item tpl inherit from the requested base class
    /// </summary>
    /// <param name="itemTpl"> ItemTpl item to check base classes of </param>
    /// <param name="baseClasses"> BaseClass base class to check for </param>
    /// <returns> true if item inherits from base class passed in </returns>
    public bool ItemHasBaseClass(MongoId itemTpl, MongoId baseClasses)
    {
        if (itemTpl.IsEmpty)
        {
            logger.Warning("Unable to check itemTpl base class as value passed is null");

            return false;
        }

        // The cache is only generated for item templates with `_type == "Item"`, so return false for any other type,
        // including item templates that simply don't exist.
        if (_rootNodeIds.Contains(itemTpl))
        {
            return false;
        }

        var existsInCache = _itemBaseClassesCache.TryGetValue(itemTpl, out var baseClassList);
        if (!existsInCache)
        {
            // Not found in cache, attempt to add first
            AddItemToCache(itemTpl);

            existsInCache = _itemBaseClassesCache.TryGetValue(itemTpl, out baseClassList);
        }

        if (existsInCache)
        {
            return baseClassList.Contains(baseClasses);
        }

        logger.Warning(serverLocalisationService.GetText("baseclass-item_not_found_failed", itemTpl.ToString()));

        return false;
    }

    /// <summary>
    ///     Get base classes item inherits from
    /// </summary>
    /// <param name="itemTpl"> ItemTpl item to get base classes for </param>
    /// <returns> array of base classes </returns>
    public HashSet<MongoId> GetItemBaseClasses(MongoId itemTpl)
    {
        if (!_itemBaseClassesCache.TryGetValue(itemTpl, out var value))
        {
            return [];
        }

        return value;
    }
}
