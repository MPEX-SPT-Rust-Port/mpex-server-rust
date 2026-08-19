using System.Reflection;
using HarmonyLib;
using SPTarkov.Common.Models.Logging;
using SPTarkov.DI.Annotations;
using SPTarkov.Server.Core.Extensions;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Helpers.Items;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Common.Tables;
using SPTarkov.Server.Core.Models.Enums;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Native.Ragfair;

namespace SPTarkov.Server.Core.Services.Ragfair;

/// <summary>
///     Cache the items linked to each item in the items db inside a dictionary
///
///     The table build runs in <c>rust/spt-native</c> by default; the request builder projects the
///     live items table into the native payload. The full 4.1.2 C# implementation is retained below
///     as the legacy path - it is the frozen mod contract (constructor and protected members are
///     apicompat-gated against the 4.1.2 baseline) and runs instead of the native path when a Harmony
///     patch on any frozen member is detected, when a mod substituted the service, when the frozen
///     constructor built the instance or when RagfairConfig.ForceLegacyRagfairLinkedItemBuild is set,
///     so mod hooks fire with genuine baseline semantics.
/// </summary>
[Injectable(InjectionType.Singleton)]
public class RagfairLinkedItemService(TemplateTable templateTable, ItemHelper itemHelper, ISptLogger<RagfairLinkedItemService> logger)
{
    protected readonly Dictionary<MongoId, HashSet<MongoId>> linkedItemsCache = new();

    private readonly RagfairLinkedItemNativeRequestBuilder? _requestBuilder;

    /// <summary>
    ///     Only set beside <see cref="_requestBuilder"/> by the additive constructor, and only read
    ///     once that builder has been found non-null.
    /// </summary>
    private readonly RagfairConfig? _ragfairConfig;

    /// <summary>
    ///     The constructor the container uses: the frozen 4.1.2 one plus the native request builder.
    ///     Additive and apicompat-verified.
    /// </summary>
    public RagfairLinkedItemService(
        TemplateTable templateTable,
        ItemHelper itemHelper,
        ISptLogger<RagfairLinkedItemService> logger,
        RagfairLinkedItemNativeRequestBuilder requestBuilder,
        RagfairConfig ragfairConfig
    )
        : this(templateTable, itemHelper, logger)
    {
        _requestBuilder = requestBuilder;
        _ragfairConfig = ragfairConfig;
    }

    /// <summary>
    ///     Which implementation the most recent table build ran - the spt-native path or the
    ///     retained 4.1.2 C# path. Test seam; also handy in a debugger.
    /// </summary>
    internal LootGenerationPath LastPathTaken { get; private set; }

    /// <summary>
    ///     The built table, for parity tests to compare both paths' output.
    /// </summary>
    internal IReadOnlyDictionary<MongoId, HashSet<MongoId>> CacheForTests
    {
        get { return linkedItemsCache; }
    }

    /// <summary>
    ///     The 4.1.2 members a mod can Harmony-patch. Public, protected and protected-internal
    ///     methods declared on this class - exactly the surface the apicompat gate freezes, statics
    ///     included. <see cref="BuildLinkedItemTable"/> is excluded: it is the dispatcher now, and a
    ///     patch on it wraps whichever path runs. Everything else is never called natively, so a
    ///     patch on one would silently do nothing.
    /// </summary>
    private static readonly List<MethodBase> _hookableMembers =
    [
        .. typeof(RagfairLinkedItemService)
            .GetMethods(
                BindingFlags.Instance | BindingFlags.Static | BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.DeclaredOnly
            )
            // Property accessors and operators are IsSpecialName; constructors are not returned at all
            .Where(method => !method.IsSpecialName && (method.IsPublic || method.IsFamily || method.IsFamilyOrAssembly))
            .Where(method => method.Name != nameof(BuildLinkedItemTable)),
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
        if (_requestBuilder is null || _ragfairConfig?.ForceLegacyRagfairLinkedItemBuild == true)
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
        return GetType() != typeof(RagfairLinkedItemService);
    }

    public HashSet<MongoId> GetLinkedItems(MongoId linkedSearchId)
    {
        if (!linkedItemsCache.TryGetValue(linkedSearchId, out var set))
        {
            // Regenerate cache
            BuildLinkedItemTable();

            return linkedItemsCache[linkedSearchId];
        }

        return set;
    }

    /// <summary>
    ///     Use ragfair linked item service to get a list of items that can fit on or in designated itemTpl
    /// </summary>
    /// <param name="itemTpl"> Item to get sub-items for </param>
    /// <returns> TemplateItem list </returns>
    public List<TemplateItem> GetLinkedDbItems(MongoId itemTpl)
    {
        var linkedItemsToWeaponTpls = GetLinkedItems(itemTpl);
        return linkedItemsToWeaponTpls.Aggregate(
            new List<TemplateItem>(),
            (result, linkedTpl) =>
            {
                var itemDetails = itemHelper.GetItem(linkedTpl);
                if (itemDetails.Key)
                {
                    result.Add(itemDetails.Value);
                }
                else
                {
                    logger.Warning($"Item {itemTpl} has invalid linked item {linkedTpl}");
                }

                return result;
            }
        );
    }

    /// <summary>
    ///     Create Dictionary of every item and the items associated with it
    /// </summary>
    protected void BuildLinkedItemTable()
    {
        if (UseLegacyPath())
        {
            LastPathTaken = LootGenerationPath.Legacy;

            BuildLinkedItemTableLegacy();

            return;
        }

        LastPathTaken = LootGenerationPath.Native;

        var result = _requestBuilder!.Send();

        // The same final copy loop legacy runs. Quirk 1, ported verbatim: `.Add`, never TryAdd or
        // assignment - a rebuild over a warm cache throws on both paths, and the miss path's
        // indexer quirk stays with it (RagfairLinkedItemService.cs:106-109, :19-24). Quirk 6: the
        // service is lock-free; the native arm adds no lock.
        foreach (var entry in result.LinkedItems)
        {
            linkedItemsCache.Add(entry.Key, entry.Value);
        }
    }

    private void BuildLinkedItemTableLegacy()
    {
        var linkedItems = new Dictionary<MongoId, HashSet<MongoId>>();

        foreach (var item in templateTable.Items.Values)
        {
            // Ensure hashset exists for item
            linkedItems.TryAdd(item.Id, []);
            var itemLinkedSet = linkedItems[item.Id];

            // Slots
            foreach (var linkedItemId in GetSlotFilters(item))
            {
                itemLinkedSet.Add(linkedItemId);

                linkedItems.TryAdd(linkedItemId, []);
                linkedItems[linkedItemId].Add(item.Id);
            }

            // Chambers
            foreach (var linkedItemId in GetChamberFilters(item))
            {
                itemLinkedSet.Add(linkedItemId);

                linkedItems.TryAdd(linkedItemId, []);
                linkedItems[linkedItemId].Add(item.Id);
            }

            // Cartridges
            foreach (var linkedItemId in GetCartridgeFilters(item))
            {
                itemLinkedSet.Add(linkedItemId);

                linkedItems.TryAdd(linkedItemId, []);
                linkedItems[linkedItemId].Add(item.Id);
            }

            // Edge case, ensure ammo for revolvers is included
            if (item.Parent == BaseClasses.REVOLVER)
            // Find magazine for revolver
            {
                AddRevolverCylinderAmmoToLinkedItems(item, itemLinkedSet);
            }
        }

        // We have our linked item pool generated, add to class property
        foreach (var item in linkedItems)
        {
            linkedItemsCache.Add(item.Key, item.Value);
        }
    }

    /// <summary>
    ///     Add ammo to revolvers linked item dictionary
    /// </summary>
    /// <param name="cylinder"> Revolvers cylinder </param>
    /// <param name="itemLinkedSet"> Set to add to </param>
    protected void AddRevolverCylinderAmmoToLinkedItems(TemplateItem cylinder, HashSet<MongoId> itemLinkedSet)
    {
        var cylinderMod = cylinder.Properties.Slots?.FirstOrDefault(x => x.Name == "mod_magazine");
        if (cylinderMod == null)
        {
            return;
        }

        // Get the first cylinder filter tpl
        var cylinderTpl = cylinderMod.Properties?.Filters?.First().Filter?.FirstOrDefault() ?? MongoId.Empty();

        if (!cylinderTpl.IsValidMongoId())
        {
            // No cylinder, nothing to do
            return;
        }

        // Get db data for cylinder tpl, add found slots info (camora_xxx) to linked items on revolver weapon
        var cylinderTemplate = itemHelper.GetItem(cylinderTpl).Value;
        itemLinkedSet.UnionWith(GetSlotFilters(cylinderTemplate));
    }

    /// <summary>
    /// Get a set of unique tpls from an items Slot 'filter' array
    /// </summary>
    /// <param name="item">Db item to get tpls from</param>
    /// <returns>Set of tpls</returns>
    protected HashSet<MongoId> GetSlotFilters(TemplateItem item)
    {
        var result = new HashSet<MongoId>();

        var slots = item.Properties?.Slots;
        if (slots is null || !slots.Any())
        {
            // No slots, skip
            return result;
        }

        // Check each slot and merge contents together into result set
        foreach (var slot in slots)
        {
            if (slot.Properties?.Filters is null)
            {
                continue;
            }

            foreach (var slotFilters in slot.Properties.Filters)
            {
                result.UnionWith(slotFilters.Filter);
            }
        }

        return result;
    }

    protected HashSet<MongoId> GetChamberFilters(TemplateItem item)
    {
        var result = new HashSet<MongoId>();

        var chambers = item.Properties?.Chambers;
        if (chambers is null || !chambers.Any())
        {
            return result;
        }

        foreach (var chamber in chambers)
        {
            if (chamber.Properties?.Filters is null)
            {
                continue;
            }

            foreach (var slotFilters in chamber.Properties.Filters)
            {
                result.UnionWith(slotFilters.Filter);
            }
        }

        return result;
    }

    protected HashSet<MongoId> GetCartridgeFilters(TemplateItem item)
    {
        var result = new HashSet<MongoId>();

        var cartridges = item.Properties?.Cartridges;
        if (cartridges is null || !cartridges.Any())
        {
            return result;
        }

        foreach (var cartridge in cartridges)
        {
            if (cartridge.Properties?.Filters is null)
            {
                continue;
            }

            foreach (var slotFilters in cartridge.Properties.Filters)
            {
                result.UnionWith(slotFilters.Filter);
            }
        }

        return result;
    }
}
