using SPTarkov.DI.Annotations;
using SPTarkov.Server.Core.Helpers.Items;
using SPTarkov.Server.Core.Helpers.Profile;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Common;
using SPTarkov.Server.Core.Models.Eft.Common.Tables;
using SPTarkov.Server.Core.Models.Enums;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Mod;
using SPTarkov.Server.Core.Models.Spt.Repeatable;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Native.Db;
using SPTarkov.Server.Core.Native.Loot;
using SPTarkov.Server.Core.Services.Items;
using SPTarkov.Server.Core.Services.Server;
using LocationTable = SPTarkov.Server.Core.Models.Spt.Tables.LocationTable;

namespace SPTarkov.Server.Core.Native.RepeatableQuests;

/// <summary>
/// Assembles the <see cref="GenerateRepeatableQuestRequest"/> for one repeatable-quest generation
/// out of the live database, services and config - everything the five generators and
/// <c>RepeatableQuestHelper</c> would have read for themselves - and drives the resident-DB epoch
/// protocol for the whole family: an eligible send names the epoch <see cref="DbPublisher"/>
/// guarantees current, an ineligible one carries the C#-built views override instead.
///
/// A singleton, unlike ragfair's projection: five generators share one dispatch seam. Concurrent
/// sends can race a republish, and nothing here has to win the race: the native side validates the
/// epoch every request names against the resident DB it actually holds, so a stale read costs one
/// self-healing republish-and-retry. Neither side can generate from the wrong views.
/// </summary>
[Injectable(InjectionType.Singleton)]
public class RepeatableQuestNativeRequestBuilder(
    TemplateTable templateTable,
    LocationTable locationTable,
    HandbookHelper handbookHelper,
    PresetHelper presetHelper,
    ItemFilterService itemFilterService,
    SeasonalEventService seasonalEventService,
    QuestConfig questConfig,
    // Unused since the resident-DB flip (DbPublisher reads the stamp itself); frozen 4.1.2 apicompat surface, do not remove
    DatabaseMutationStamp databaseMutationStamp,
    IReadOnlyList<SptMod> loadedMods
)
{
    private readonly DbPublisher? _dbPublisher;

    /// <summary>
    ///     Whether the most recent native send carried the C#-built views override rather than
    ///     naming a resident-DB epoch. Test seam.
    /// </summary>
    internal bool LastSendIncludedViewsOverride { get; private set; }

    /// <summary>
    ///     The constructor the container uses: the frozen one plus the resident-DB publisher.
    ///     Additive and apicompat-verified.
    /// </summary>
    public RepeatableQuestNativeRequestBuilder(
        TemplateTable templateTable,
        LocationTable locationTable,
        HandbookHelper handbookHelper,
        PresetHelper presetHelper,
        ItemFilterService itemFilterService,
        SeasonalEventService seasonalEventService,
        QuestConfig questConfig,
        DatabaseMutationStamp databaseMutationStamp,
        IReadOnlyList<SptMod> loadedMods,
        DbPublisher dbPublisher
    )
        : this(
            templateTable,
            locationTable,
            handbookHelper,
            presetHelper,
            itemFilterService,
            seasonalEventService,
            questConfig,
            databaseMutationStamp,
            loadedMods
        )
    {
        _dbPublisher = dbPublisher;
    }

    /// <summary>
    ///     Whether an override-less send off the resident DB is ever allowed: the publisher exists,
    ///     the kill switch is off, and either no mods are loaded or the user vouched their mods
    ///     don't write tables directly. A builder built on the frozen constructor has no publisher
    ///     and always sends the override.
    /// </summary>
    internal bool ResidentDbEligible()
    {
        return ResidentDbDispatch.Eligible(
            _dbPublisher,
            loadedMods.Count,
            questConfig.DisableNativeRequestCache,
            questConfig.TrustNativeRequestCacheWithMods
        );
    }

    /// <summary>
    ///     One native generation pass, with the views override included only when the caller is
    ///     ineligible to generate off the resident DB. The one entry point the dispatch calls.
    /// </summary>
    /// <returns> The generated quest - null is a valid no-quest outcome - and the mutated pool </returns>
    internal (RepeatableQuest? Quest, QuestTypePool Pool) Send(
        RepeatableQuestType questType,
        string sessionId,
        int pmcLevel,
        MongoId traderId,
        QuestTypePool questTypePool,
        RepeatableQuestConfig repeatableConfig,
        ulong? seed = null
    )
    {
        var varying = BuildVarying(questType, sessionId, pmcLevel, traderId, questTypePool, repeatableConfig, seed);

        RepeatableQuestResult result;
        if (!ResidentDbEligible())
        {
            result = SptNative.GenerateRepeatableQuest(BuildRequest(BuildViewsOverride(), epoch: 0, varying));
            LastSendIncludedViewsOverride = true;
        }
        else
        {
            result = ResidentDbDispatch.Send(
                _dbPublisher!,
                epoch => SptNative.GenerateRepeatableQuest(BuildRequest(viewsOverride: null, epoch, varying))
            );
            LastSendIncludedViewsOverride = false;
        }

        return (result.Quest, result.Pool);
    }

    internal RepeatableQuestVaryingFields BuildVarying(
        RepeatableQuestType questType,
        string sessionId,
        int pmcLevel,
        MongoId traderId,
        QuestTypePool questTypePool,
        RepeatableQuestConfig repeatableConfig,
        ulong? seed = null
    )
    {
        return new RepeatableQuestVaryingFields
        {
            QuestType = questType,
            SessionId = sessionId,
            PmcLevel = pmcLevel,
            TraderId = traderId,
            QuestTypePool = questTypePool,
            RepeatableConfig = repeatableConfig,
            Seed = seed,
            ItemBlacklist = itemFilterService.GetItemBlacklistCache(),
            RewardItemBlacklist = itemFilterService.GetItemRewardBlacklist(),
            BossItems = itemFilterService.GetBossItems(),
            SeasonalItemTplBlacklist = seasonalEventService.GetInactiveSeasonalEventItems(),
            RepeatableQuestTemplateIds = questConfig.RepeatableQuestTemplates,
            LocationIdMap = questConfig.LocationIdMap,
        };
    }

    internal QuestViewsOverride BuildViewsOverride()
    {
        var templateItems = templateTable.Items;
        var handbookPrices = new Dictionary<MongoId, double>(templateItems.Count);
        var defaultPresetOrItemPrices = new Dictionary<MongoId, double>(templateItems.Count);

        // One pass over the items table: the reward math reaches arbitrary tpls through preset
        // children and the currency conversion, so both price maps cover the whole table rather
        // than a pool - the currency tpls FromRoubles reads out of the handbook map included
        foreach (var tpl in templateItems.Keys)
        {
            handbookPrices[tpl] = handbookHelper.GetTemplatePrice(tpl);
            defaultPresetOrItemPrices[tpl] = presetHelper.GetDefaultPresetOrItemPrice(tpl);
        }

        var completionFilters = templateTable.RepeatableQuests.Data?.Completion;

        return new QuestViewsOverride
        {
            Items = PayloadProjection.BuildItemsView(templateItems),
            HandbookPrices = handbookPrices,
            FleaPrices = templateTable.Prices,
            DefaultWeaponPresets = presetHelper.GetDefaultWeaponPresets().Values.Select(PayloadProjection.ToPresetView).ToList(),
            DefaultPresetOrItemPrices = defaultPresetOrItemPrices,
            RepeatableQuestTemplates = templateTable.RepeatableQuests.Templates!,
            CompletionItemsWhitelist = completionFilters?.ItemsWhitelist ?? [],
            CompletionItemsBlacklist = completionFilters?.ItemsBlacklist ?? [],
            BossSpawnsByLocation = BuildBossSpawnsByLocation(),
            ExtractsByLocation = BuildExtractsByLocation(),
        };
    }

    private static GenerateRepeatableQuestRequest BuildRequest(
        QuestViewsOverride? viewsOverride,
        ulong epoch,
        RepeatableQuestVaryingFields varying
    )
    {
        return new GenerateRepeatableQuestRequest
        {
            Epoch = epoch,
            ViewsOverride = viewsOverride,
            Varying = varying,
        };
    }

    /// <summary>
    ///     <c>EliminationQuestGenerator.cs:517-530</c>: every location's boss spawns, reduced to the
    ///     one member it reads and keyed by the raw <c>LocationBase.Id</c> its blacklist compares
    ///     against. A spawn with no name can never match a bot type, so it is dropped rather than
    ///     crossing as a null.
    /// </summary>
    private Dictionary<string, List<string>> BuildBossSpawnsByLocation()
    {
        var bossSpawns = new Dictionary<string, List<string>>(StringComparer.Ordinal);

        foreach (var location in locationTable.GetDictionary().Values)
        {
            if (location?.Base?.Id is not { } locationId)
            {
                continue;
            }

            bossSpawns[locationId] =
            [
                .. (location.Base.BossLocationSpawn ?? []).Where(spawn => spawn.BossName is not null).Select(spawn => spawn.BossName!),
            ];
        }

        return bossSpawns;
    }

    /// <summary>
    ///     <c>ExplorationQuestGenerator.cs:202-206</c>: the extracts of every location the quest pool
    ///     can name, under the same lowercased key that lookup uses. A map the table does not hold is
    ///     omitted - that is the C# null the "unable to find exits" branch reads - while a map with no
    ///     extracts carries an empty list.
    /// </summary>
    private Dictionary<string, List<ExitView>> BuildExtractsByLocation()
    {
        var extracts = new Dictionary<string, List<ExitView>>(StringComparer.Ordinal);

        // The pool is keyed by ELocationName, so its names are the whole key domain
        foreach (var locationName in Enum.GetNames<ELocationName>())
        {
            var locationKey = locationName.ToLowerInvariant();
            var location = locationTable.GetLocation(locationKey);
            if (location is null)
            {
                continue;
            }

            extracts[locationKey] = [.. (location.AllExtracts ?? []).Select(ToExitView)];
        }

        return extracts;
    }

    private static ExitView ToExitView(Exit exit)
    {
        return new ExitView
        {
            Name = exit.Name,
            Side = exit.Side,
            Chance = exit.Chance,
            PassageRequirement = exit.PassageRequirement.ToString(),
        };
    }
}
