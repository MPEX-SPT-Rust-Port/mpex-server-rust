using System.Reflection;
using HarmonyLib;
using SPTarkov.DI.Annotations;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Helpers;
using SPTarkov.Server.Core.Helpers.Quest;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Common.Tables;
using SPTarkov.Server.Core.Models.Enums;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Repeatable;
using SPTarkov.Server.Core.Native.RepeatableQuests;
using SPTarkov.Server.Core.Utils;
using SPTarkov.Server.Core.Utils.Json;

namespace SPTarkov.Server.Core.Generators.RepeatableQuests;

[Injectable]
public class PickupQuestGenerator(
    RepeatableQuestHelper repeatableQuestHelper,
    RepeatableQuestRewardGenerator repeatableQuestRewardGenerator,
    RandomUtil randomUtil
) : IRepeatableQuestGenerator
{
    private readonly QuestConfig? _questConfig;
    private readonly RepeatableQuestNativeRequestBuilder? _requestBuilder;

    /// <summary>
    ///     The constructor the container uses: the frozen 4.1.2 one plus the config that carries the
    ///     native path flags and the native request builder. Additive and apicompat-verified.
    /// </summary>
    public PickupQuestGenerator(
        RepeatableQuestHelper repeatableQuestHelper,
        RepeatableQuestRewardGenerator repeatableQuestRewardGenerator,
        RandomUtil randomUtil,
        QuestConfig questConfig,
        RepeatableQuestNativeRequestBuilder requestBuilder
    )
        : this(repeatableQuestHelper, repeatableQuestRewardGenerator, randomUtil)
    {
        _questConfig = questConfig;
        _requestBuilder = requestBuilder;
    }

    /// <summary>
    ///     Which implementation the most recent generation call ran - the spt-native path or the
    ///     retained 4.1.2 C# path. Test seam; also handy in a debugger.
    /// </summary>
    internal LootGenerationPath LastPathTaken { get; private set; }

    /// <summary>
    ///     Test-only seed forwarded as <c>RepeatableQuestVaryingFields.Seed</c> on every native
    ///     request.
    /// </summary>
    internal ulong? NativeTestSeed { get; set; }

    /// <summary>
    ///     The 4.1.2 members a mod can Harmony-patch, across the four repeatable-quest generators and
    ///     the two collaborators the native path folds in. Public, protected and protected-internal
    ///     methods declared on each - exactly the surface the apicompat gate freezes, statics
    ///     included. The four <c>Generate</c> methods (all four share the name) are excluded: each is
    ///     a dispatcher now, and a patch on one wraps whichever path runs. Everything else is never
    ///     called natively, so a patch on one would silently do nothing.
    /// </summary>
    private static readonly List<MethodBase> _hookableMembers =
    [
        .. new[]
        {
            typeof(EliminationQuestGenerator),
            typeof(CompletionQuestGenerator),
            typeof(ExplorationQuestGenerator),
            typeof(PickupQuestGenerator),
            typeof(RepeatableQuestRewardGenerator),
            typeof(RepeatableQuestHelper),
        }
            .SelectMany(type =>
                type.GetMethods(
                    BindingFlags.Instance | BindingFlags.Static | BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.DeclaredOnly
                )
            )
            // Property accessors and operators are IsSpecialName; constructors are not returned at all
            .Where(method => !method.IsSpecialName && (method.IsPublic || method.IsFamily || method.IsFamilyOrAssembly))
            .Where(method => method.Name != nameof(Generate)),
    ];

    /// <summary>
    ///     The legacy path runs when the frozen 4.1.2 constructor built this instance (it has no
    ///     native seam to dispatch to), when forced by config, when any of the frozen 4.1.2 members
    ///     carries a live Harmony patch, or when a mod has substituted one of the collaborators the
    ///     native path folded in - running the retained C# implementation is the only way those hooks
    ///     and replacements can take effect with real baseline semantics.
    /// </summary>
    private bool UseLegacyPath()
    {
        if (_requestBuilder is null || _questConfig is null || _questConfig.ForceLegacyRepeatableQuestGeneration)
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
        return GetType() != typeof(PickupQuestGenerator)
            || repeatableQuestHelper.GetType() != typeof(RepeatableQuestHelper)
            || repeatableQuestRewardGenerator.GetType() != typeof(RepeatableQuestRewardGenerator);
    }

    /// <summary>
    ///     Refill the caller's pool from the pool the native side returned. The controller holds the
    ///     instance it passed in and keeps reading it after the call, so that instance - and the
    ///     three sub-pools hanging off it - have to survive; only the collections they carry change.
    /// </summary>
    private static void CopyPoolInto(QuestTypePool target, QuestTypePool source)
    {
        target.Types.Clear();
        target.Types.AddRange(source.Types);

        target.Pool.Exploration.Locations = source.Pool.Exploration.Locations;
        target.Pool.Elimination.Targets = source.Pool.Elimination.Targets;
        target.Pool.Pickup.Locations = source.Pool.Pickup.Locations;
    }

    public RepeatableQuest? Generate(
        MongoId sessionId,
        int pmcLevel,
        MongoId traderId,
        QuestTypePool questTypePool,
        RepeatableQuestConfig repeatableConfig
    )
    {
        if (UseLegacyPath())
        {
            LastPathTaken = LootGenerationPath.Legacy;

            return GenerateLegacy(sessionId, pmcLevel, traderId, questTypePool, repeatableConfig);
        }

        LastPathTaken = LootGenerationPath.Native;

        var (quest, pool) = _requestBuilder!.Send(
            RepeatableQuestType.Pickup,
            sessionId,
            pmcLevel,
            traderId,
            questTypePool,
            repeatableConfig,
            NativeTestSeed
        );

        CopyPoolInto(questTypePool, pool);

        return quest;
    }

    // TODO: This isn't really implemented, not in the current pool.
    private RepeatableQuest? GenerateLegacy(
        MongoId sessionId,
        int pmcLevel,
        MongoId traderId,
        QuestTypePool questTypePool,
        RepeatableQuestConfig repeatableConfig
    )
    {
        var pickupConfig = repeatableConfig.QuestConfig.Pickup;

        var quest = repeatableQuestHelper.GenerateRepeatableTemplate(
            RepeatableQuestType.Pickup,
            traderId,
            repeatableConfig.Side,
            sessionId
        );

        var itemTypeToFetchWithCount = randomUtil.GetArrayValue(pickupConfig.ItemTypeToFetchWithMaxCount);

        var itemCountToFetch = randomUtil.RandInt(
            itemTypeToFetchWithCount.MinimumPickupCount.Value,
            itemTypeToFetchWithCount.MaximumPickupCount + 1
        );
        // Choose location - doesn't seem to work for anything other than 'any'
        // var locationKey: string = this.randomUtil.drawRandomFromDict(questTypePool.pool.Pickup.locations)[0];
        // var locationTarget = questTypePool.pool.Pickup.locations[locationKey];

        var findCondition = quest.Conditions.AvailableForFinish.FirstOrDefault(x => x.ConditionType == "FindItem");
        findCondition.Target = new ListOrT<string>([itemTypeToFetchWithCount.ItemType], null);
        findCondition.Value = itemCountToFetch;

        var counterCreatorCondition = quest.Conditions.AvailableForFinish.FirstOrDefault(x => x.ConditionType == "CounterCreator");
        // var locationCondition = counterCreatorCondition._props.counter.conditions.find(x => x._parent === "Location");
        // (locationCondition._props as ILocationConditionProps).target = [...locationTarget];

        var equipmentCondition = counterCreatorCondition.Counter.Conditions.FirstOrDefault(x => x.ConditionType == "Equipment");
        equipmentCondition.EquipmentInclusive =
        [
            [itemTypeToFetchWithCount.ItemType],
        ];

        // Add rewards
        quest.Rewards = repeatableQuestRewardGenerator.GenerateReward(pmcLevel, 1, traderId, repeatableConfig, pickupConfig);

        return quest;
    }
}
