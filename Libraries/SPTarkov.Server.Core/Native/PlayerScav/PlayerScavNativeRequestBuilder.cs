using SPTarkov.DI.Annotations;
using SPTarkov.Server.Core.Helpers.InRaid;
using SPTarkov.Server.Core.Helpers.Items;
using SPTarkov.Server.Core.Helpers.Profile;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Common.Tables;
using SPTarkov.Server.Core.Models.Spt.Bots;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Native.Bot;
using SPTarkov.Server.Core.Services.Bot;
using SPTarkov.Server.Core.Services.Items;
using SPTarkov.Server.Core.Services.Profile;

namespace SPTarkov.Server.Core.Native.PlayerScav;

/// <summary>
/// Assembles the <see cref="GeneratePlayerScavRequest"/> for one player scav out of the live
/// database, services and config: the bot request <c>BotPayloadProjection</c> builds, plus the
/// karma slice <c>PlayerScavGenerator</c> would have applied for itself.
/// </summary>
[Injectable]
public class PlayerScavNativeRequestBuilder(
    ProfileHelper profileHelper,
    ProfileActivityService profileActivityService,
    WeatherHelper weatherHelper,
    BotLootCacheService botLootCacheService,
    PresetHelper presetHelper,
    HandbookHelper handbookHelper,
    ItemHelper itemHelper,
    GlobalTable globalTable,
    ItemFilterService itemFilterService,
    BotConfig botConfig,
    PmcConfig pmcConfig,
    RepairConfig repairConfig
)
{
    /// <summary>
    /// The request every send carries: the single-bot bot request at epoch 0 and with no views
    /// override, plus the karma slice. The dispatch site stamps the resident epoch, or asks for the
    /// override, as the bot path does.
    /// </summary>
    /// <param name="hydrationTemplate">The karma-adjusted template - its generation block feeds the loot pool hydration</param>
    internal GeneratePlayerScavRequest Build(
        MongoId botId,
        MongoId sessionId,
        BotType hydrationTemplate,
        BotGenerationDetails details,
        KarmaLevel karmaSettings,
        ulong? testSeed
    )
    {
        var botRequest = BotPayloadProjection.BuildRequest(
            botId,
            sessionId,
            hydrationTemplate,
            details,
            testSeed,
            profileHelper,
            profileActivityService,
            weatherHelper,
            botLootCacheService,
            botConfig,
            pmcConfig
        );

        return new GeneratePlayerScavRequest
        {
            Epoch = 0,
            Shared = botRequest.Shared,
            Bot = botRequest.Bot,
            Template = botRequest.Template,
            LootPools = botRequest.LootPools,
            Karma = BuildKarmaView(karmaSettings),
        };
    }

    /// <summary>
    /// The database half, for the ineligible arm. The request's own pools are the ninth argument -
    /// they feed <c>BuildHandbookPrices</c>, and an empty enumerable would price every override-arm
    /// loot draw at 0.
    /// </summary>
    internal BotViewsOverride BuildViewsOverride(BotLootCache lootPools)
    {
        return BotPayloadProjection.BuildViewsOverride(
            presetHelper,
            handbookHelper,
            itemHelper,
            globalTable,
            itemFilterService,
            botConfig,
            pmcConfig,
            repairConfig,
            [lootPools]
        );
    }

    /// <summary>
    /// The wire slice of one karma level. <c>ItemLimits</c> is left off: it is applied to the
    /// template's generation block C#-side, before <see cref="Build"/> hydrates the loot pools from
    /// it.
    /// </summary>
    private static KarmaSettingsView BuildKarmaView(KarmaLevel karmaSettings)
    {
        return new KarmaSettingsView
        {
            EquipmentModifiers = karmaSettings.Modifiers.Equipment,
            ModModifiers = karmaSettings.Modifiers.Mod,
            // System.Text.Json writes enum dictionary keys numerically, which the native side could
            // not match to a slot name
            EquipmentBlacklist = karmaSettings.EquipmentBlacklist.ToDictionary(pair => pair.Key.ToString(), pair => pair.Value),
            LootItemsToAddChancePercent = karmaSettings.LootItemsToAddChancePercent,
        };
    }
}
