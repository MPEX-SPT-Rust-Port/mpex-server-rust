using SPTarkov.Server.Core.Constants;
using SPTarkov.Server.Core.Helpers.Items;
using SPTarkov.Server.Core.Helpers.Profile;
using SPTarkov.Server.Core.Helpers.Traders;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Common.Tables;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Native.Loot;
using SPTarkov.Server.Core.Services.Items;
using SPTarkov.Server.Core.Services.Server;

namespace SPTarkov.Server.Core.Native.Ragfair;

/// <summary>
/// Assembles the <see cref="GenerateDynamicOffersRequest"/> for one offer-generation pass out of the
/// live database, services and config - everything <c>RagfairOfferGenerator</c>,
/// <c>RagfairAssortGenerator</c> and <c>RagfairPriceService</c> would have read for themselves.
///
/// The services are parameters rather than injected members so the caller keeps its own 4.1.2
/// constructor: this class is a projection, not a component.
/// </summary>
internal static class RagfairPayloadProjection
{
    internal static GenerateDynamicOffersRequest BuildRequest(
        IEnumerable<List<Item>>? expiredOffers,
        long timestamp,
        int offerCounterStart,
        ulong? testSeed,
        TemplateTable templateTable,
        HandbookHelper handbookHelper,
        TraderHelper traderHelper,
        PresetHelper presetHelper,
        ItemFilterService itemFilterService,
        SeasonalEventService seasonalEventService,
        BotTable botTable,
        ItemHelper itemHelper,
        BotConfig botConfig,
        RagfairConfig ragfairConfig
    )
    {
        var templateItems = templateTable.Items;
        var handbookPrices = new Dictionary<MongoId, double>(templateItems.Count);
        var highestTraderPrices = new Dictionary<MongoId, double>(templateItems.Count);
        var presetsByTpl = new Dictionary<MongoId, List<PresetView>>();

        // One pass over the items table: the pricing math reaches arbitrary tpls through barter
        // schemes and preset children, so both price maps cover the whole table rather than a pool
        foreach (var tpl in templateItems.Keys)
        {
            handbookPrices[tpl] = handbookHelper.GetTemplatePrice(tpl);
            highestTraderPrices[tpl] = traderHelper.GetHighestSellToTraderPrice(tpl);

            if (presetHelper.HasPreset(tpl))
            {
                presetsByTpl[tpl] = ToPresetViews(presetHelper.GetPresets(tpl) ?? []);
            }
        }

        return new GenerateDynamicOffersRequest
        {
            TestSeed = testSeed,
            Timestamp = timestamp,
            OfferCounterStart = offerCounterStart,
            ExpiredOffers = expiredOffers,
            Dynamic = ragfairConfig.Dynamic,
            // Keyed by preset id, in the globals' insertion order - the assort walk scans it in order
            ItemPresets = (presetHelper.GetAllPresets() ?? []).ToDictionary(preset => preset.Id, ToPresetView),
            DefaultPresets = ToPresetViews(presetHelper.GetDefaultPresets().Values),
            DefaultPresetsByTpl = presetHelper
                .GetDefaultPresetByTpl()
                .ToDictionary(preset => preset.Key, preset => ToPresetView(preset.Value)),
            PresetsByTpl = presetsByTpl,
            // The whole flea price table, in source order: GetFleaPricesAsArray draws an index into it
            FleaPrices = templateTable.Prices,
            HandbookPrices = handbookPrices,
            HighestTraderPrices = highestTraderPrices,
            ConfigBlacklist = itemFilterService.GetBlacklistedItems(),
            SeasonalEventActive = seasonalEventService.SeasonalEventEnabled(),
            SeasonalItemTplBlacklist = seasonalEventService.GetInactiveSeasonalEventItems(),
            PmcNamesUsec = GatherPmcNamesOfLength(botTable, Sides.Usec.ToLowerInvariant(), botConfig.BotNameLengthLimit),
            PmcNamesBear = GatherPmcNamesOfLength(botTable, Sides.Bear.ToLowerInvariant(), botConfig.BotNameLengthLimit),
            Items = PayloadProjection.BuildItemsView(itemHelper.TemplateTable.Items),
        };
    }

    /// <summary>
    /// <c>BotHelper.GatherPmcNamesOfLength</c> (<c>:183-205</c>), fallback included: a filter that
    /// matches nothing falls back to the whole name pool rather than leaving the faction nameless.
    /// </summary>
    private static List<string> GatherPmcNamesOfLength(BotTable botTable, string faction, int maxLength)
    {
        var names = botTable.Types[faction]!.FirstNames;
        var matchingNames = names.Where(name => name.Length <= maxLength).ToList();

        return matchingNames.Count != 0 ? matchingNames : names.ToList();
    }

    private static List<PresetView> ToPresetViews(IEnumerable<Preset> presets)
    {
        return presets.Select(ToPresetView).ToList();
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
