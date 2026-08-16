using SPTarkov.Common.Models.Logging;
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
using SPTarkov.Server.Core.Native.Loot;
using SPTarkov.Server.Core.Services.Items;
using SPTarkov.Server.Core.Services.Locales;
using SPTarkov.Server.Core.Services.Server;
using LocationTable = SPTarkov.Server.Core.Models.Spt.Tables.LocationTable;

namespace SPTarkov.Server.Core.Native.RepeatableQuests;

/// <summary>
/// Assembles the <see cref="GenerateRepeatableQuestRequest"/> for one repeatable-quest generation
/// out of the live database, services and config - everything the five generators and
/// <c>RepeatableQuestHelper</c> would have read for themselves - and owns the stamp-gated native
/// slice cache for the whole family.
///
/// A singleton, unlike ragfair's projection: five generators share one native slice cache, so the
/// last-sent stamp has to be shared too. Concurrent sends can race that field, and both outcomes
/// self-heal - one redundant slice on the wire, or one stale-slice retry.
/// </summary>
[Injectable(InjectionType.Singleton)]
public class RepeatableQuestNativeRequestBuilder(
    ISptLogger<RepeatableQuestNativeRequestBuilder> logger,
    TemplateTable templateTable,
    LocationTable locationTable,
    HandbookHelper handbookHelper,
    PresetHelper presetHelper,
    ItemFilterService itemFilterService,
    SeasonalEventService seasonalEventService,
    ServerLocalisationService localisationService,
    QuestConfig questConfig,
    DatabaseMutationStamp databaseMutationStamp,
    IReadOnlyList<SptMod> loadedMods
)
{
    /// <summary>
    ///     The native side caches the parsed invariant slice under the stamp value it was sent
    ///     with; this is the stamp of the last slice it accepted, so an unchanged stamp can skip
    ///     the slice entirely. Null until a slice is sent under an eligible cache. Internal set:
    ///     the desync test seam.
    /// </summary>
    internal long? LastSentSliceStamp { get; set; }

    /// <summary>
    ///     Whether the most recent native send carried the invariant slice. Test seam.
    /// </summary>
    internal bool LastSendIncludedSlice { get; private set; }

    /// <summary>
    ///     Whether a slice-less send is ever allowed: the kill switch is off, and either no mods are
    ///     loaded or the user vouched their mods don't write tables directly.
    /// </summary>
    internal bool CacheEligible()
    {
        if (questConfig.DisableNativeRequestCache)
        {
            return false;
        }

        return loadedMods.Count == 0 || questConfig.TrustNativeRequestCacheWithMods;
    }

    /// <summary>
    ///     One native generation pass, with the invariant slice included only when the native cache
    ///     cannot already be holding it. The one entry point the dispatch calls.
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
        var stamp = databaseMutationStamp.Current;
        var eligible = CacheEligible();
        var sendSlice = !eligible || LastSentSliceStamp != stamp;
        var varying = BuildVarying(questType, sessionId, pmcLevel, traderId, questTypePool, repeatableConfig, seed);

        RepeatableQuestResult result;
        try
        {
            result = SptNative.GenerateRepeatableQuest(BuildRequest(sendSlice, stamp, varying));
            LastSendIncludedSlice = sendSlice;
        }
        catch (NativeStaleSliceException)
        {
            // The native cache does not hold the slice this stamp names - resend it whole
            result = SptNative.GenerateRepeatableQuest(BuildRequest(true, stamp, varying));
            LastSendIncludedSlice = true;
        }

        LastSentSliceStamp = eligible ? stamp : null;

        PayloadProjection.ReplayDiagnostics(result.Diagnostics, logger, localisationService);

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
        };
    }

    internal QuestInvariantSlice BuildInvariantSlice()
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

        return new QuestInvariantSlice
        {
            Items = PayloadProjection.BuildItemsView(templateItems),
            HandbookPrices = handbookPrices,
            FleaPrices = templateTable.Prices,
            DefaultWeaponPresets = presetHelper.GetDefaultWeaponPresets().Values.Select(ToPresetView).ToList(),
            DefaultPresetOrItemPrices = defaultPresetOrItemPrices,
            ItemBlacklist = itemFilterService.GetItemBlacklistCache(),
            RewardItemBlacklist = itemFilterService.GetItemRewardBlacklist(),
            BossItems = itemFilterService.GetBossItems(),
            SeasonalItemTplBlacklist = seasonalEventService.GetInactiveSeasonalEventItems(),
            RepeatableQuestTemplates = templateTable.RepeatableQuests.Templates!,
            CompletionItemsWhitelist = completionFilters?.ItemsWhitelist ?? [],
            CompletionItemsBlacklist = completionFilters?.ItemsBlacklist ?? [],
            BossSpawnsByLocation = BuildBossSpawnsByLocation(),
            ExtractsByLocation = BuildExtractsByLocation(),
            RepeatableQuestTemplateIds = questConfig.RepeatableQuestTemplates,
            LocationIdMap = questConfig.LocationIdMap,
        };
    }

    private GenerateRepeatableQuestRequest BuildRequest(bool sendSlice, long stamp, RepeatableQuestVaryingFields varying)
    {
        return new GenerateRepeatableQuestRequest
        {
            InvariantStamp = stamp,
            Invariant = sendSlice ? BuildInvariantSlice() : null,
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

    private static PresetView ToPresetView(Preset preset)
    {
        return new PresetView
        {
            Items = preset.Items,
            Id = preset.Id,
            Name = preset.Name,
            Encyclopedia = preset.Encyclopedia,
        };
    }
}
