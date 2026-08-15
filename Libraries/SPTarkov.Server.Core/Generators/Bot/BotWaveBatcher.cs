using System.Reflection;
using HarmonyLib;
using SPTarkov.Common.Models.Logging;
using SPTarkov.DI.Annotations;
using SPTarkov.Server.Core.Constants;
using SPTarkov.Server.Core.Helpers.Bot;
using SPTarkov.Server.Core.Helpers.InRaid;
using SPTarkov.Server.Core.Helpers.Items;
using SPTarkov.Server.Core.Helpers.Profile;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Common.Tables;
using SPTarkov.Server.Core.Models.Enums;
using SPTarkov.Server.Core.Models.Spt.Bots;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Native;
using SPTarkov.Server.Core.Native.Bot;
using SPTarkov.Server.Core.Native.Loot;
using SPTarkov.Server.Core.Services.Bot;
using SPTarkov.Server.Core.Services.Items;
using SPTarkov.Server.Core.Services.Locales;
using SPTarkov.Server.Core.Services.Profile;
using SPTarkov.Server.Core.Utils.Cloners;

namespace SPTarkov.Server.Core.Generators.Bot;

/// <summary>
///     The batched wave path: one native call generates every inventory in the wave, with the
///     shared views on the wire once. Declines (returns null) whenever a mod could observe the
///     difference from the per-bot path - the same contract the three existing native dispatchers
///     honour - or when the wave could write nighttime equipment clamps, whose cross-bot feedback
///     loop only the per-bot path replays. Bots that fail are skipped with one Critical log each,
///     matching BotController.TryGenerateSingleBot.
/// </summary>
[Injectable]
public class BotWaveBatcher(
    ISptLogger<BotWaveBatcher> logger,
    BotGenerator botGenerator,
    BotInventoryGenerator botInventoryGenerator,
    BotEquipmentFilterService botEquipmentFilterService,
    BotEquipmentModPoolService botEquipmentModPoolService,
    BotGeneratorHelper botGeneratorHelper,
    ProfileHelper profileHelper,
    ProfileActivityService profileActivityService,
    WeatherHelper weatherHelper,
    ItemHelper itemHelper,
    ItemFilterService itemFilterService,
    PresetHelper presetHelper,
    BotLootCacheService botLootCacheService,
    HandbookHelper handbookHelper,
    GlobalTable globalTable,
    MatchBotDetailsCacheService matchBotDetailsCacheService,
    ServerLocalisationService serverLocalisationService,
    BotConfig botConfig,
    PmcConfig pmcConfig,
    RepairConfig repairConfig,
    ICloner cloner
)
{
    /// <summary>
    ///     The frozen 4.1.2 members of the two classes the batch path routes around. A live Harmony
    ///     patch on any of them means a mod expects per-bot semantics, so the batch declines. Same
    ///     construction as BotInventoryGenerator's set; GenerateBotWave itself is excluded because a
    ///     patch on the dispatcher wraps whichever path runs. Internal additions (the prelude/finish
    ///     split, PrepareBot) are IsAssembly and fall out of the visibility filter on their own.
    /// </summary>
    private static readonly List<MethodBase> _hookableWaveMembers =
    [
        .. new[] { typeof(BotGenerator), typeof(Controllers.BotController) }
            .SelectMany(type =>
                type.GetMethods(
                    BindingFlags.Instance | BindingFlags.Static | BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.DeclaredOnly
                )
            )
            .Where(method => !method.IsSpecialName && (method.IsPublic || method.IsFamily || method.IsFamilyOrAssembly))
            // Protected on another class, so nameof() cannot reach it
            .Where(method => method.Name != "GenerateBotWave"),
    ];

    private sealed record PreparedWaveBot(BotBase Bot, BotType Template, BotGenerationDetails Details);

    /// <summary>
    ///     Generate the whole wave through the batch path, or return null when the wave must run
    ///     per bot. Null is a routing decision, not a failure - the caller falls through to the
    ///     unchanged per-bot path.
    /// </summary>
    internal List<BotBase?>? TryGenerateWave(MongoId sessionId, BotGenerationDetails botGenerationDetails)
    {
        if (!CanBatch(sessionId, botGenerationDetails))
        {
            return null;
        }

        var count = botGenerationDetails.BotCountToGenerate;
        var prepared = new PreparedWaveBot?[count];
        Parallel.For(
            0,
            count,
            index =>
            {
                try
                {
                    // Clone for thread safety, exactly as TryGenerateSingleBot does
                    var detailsClone = cloner.Clone(botGenerationDetails)!;
                    var (bot, template) = botGenerator.PrepareBot(detailsClone);
                    if (template is null)
                    {
                        // PrepareBot already logged the missing template
                        return;
                    }

                    botGenerator.GenerateBotPrelude(sessionId, bot, template, detailsClone);
                    prepared[index] = new PreparedWaveBot(bot, template, detailsClone);
                }
                catch (Exception e)
                {
                    logger.Critical($"Failed to generate bot #{index + 1} ({botGenerationDetails.Role}): {e.Message}", e);
                }
            }
        );

        var survivors = prepared.OfType<PreparedWaveBot>().ToList();
        if (survivors.Count == 0)
        {
            return [];
        }

        BotInventoryBatchResult batchResult;
        try
        {
            batchResult = SptNative.GenerateBotInventoryBatch(BuildBatchRequest(sessionId, botGenerationDetails, survivors));
        }
        catch (Exception e)
        {
            // A wholesale failure is a native bug; on the per-bot path every bot's call would
            // have thrown the same way and the wave would come back empty there too
            logger.Critical($"Failed to generate bot wave ({botGenerationDetails.Role}): {e.Message}", e);

            return [];
        }

        var bots = new List<BotBase?>(survivors.Count);
        for (var index = 0; index < survivors.Count; index++)
        {
            var envelope = batchResult.Bots[index];
            var entry = survivors[index];
            try
            {
                if (envelope.Result is null)
                {
                    logger.Critical($"Failed to generate bot #{index + 1} ({entry.Details.Role}): {envelope.Error ?? "no result"}");

                    continue;
                }

                entry.Bot.Inventory = envelope.Result.Inventory;
                PayloadProjection.ReplayDiagnostics(envelope.Result.Diagnostics, logger, serverLocalisationService);
                if (!entry.Details.ClearBotContainerCacheAfterGeneration)
                {
                    botInventoryGenerator.RestoreContainerGrids(entry.Bot.Id.Value, envelope.Result.ContainerGrids);
                }

                botInventoryGenerator.ReplayRandomisationClamps(entry.Details, envelope.Result.RandomisationClamps);
                botGenerator.GenerateBotFinish(entry.Bot, entry.Details);

                // Client expects Side for PMCs to be `Savage`, must be altered here before it's cached
                if (entry.Bot.Info?.Side is Sides.Bear or Sides.Usec)
                {
                    entry.Bot.Info.Side = Sides.Savage;
                }

                matchBotDetailsCacheService.CacheBot(entry.Bot);
                bots.Add(entry.Bot);
            }
            catch (Exception e)
            {
                logger.Critical($"Failed to generate bot #{index + 1} ({entry.Details.Role}): {e.Message}", e);
            }
        }

        return bots;
    }

    private bool CanBatch(MongoId sessionId, BotGenerationDetails details)
    {
        if (botConfig.ForcePerBotGeneration)
        {
            return false;
        }

        // The batch bypasses GenerateInventory, so every reason that dispatcher would take the
        // legacy path applies here unchanged
        if (botInventoryGenerator.UseLegacyPath())
        {
            return false;
        }

        if (
            _hookableWaveMembers.Any(member =>
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
            return false;
        }

        // A mod substituted its own BotGenerator; only per-bot generation routes through it
        if (botGenerator.GetType() != typeof(BotGenerator))
        {
            return false;
        }

        return !WaveCanWriteNighttimeClamps(sessionId, details);
    }

    /// <summary>
    ///     The nighttime clamp (GenerateAndAddEquipmentToBot) is a cross-bot feedback loop through
    ///     the live BotConfig that only the per-bot path replays between bots. Decidable per wave:
    ///     the raid must be nighttime AND the wave role's equipment config must carry nighttime
    ///     modifiers in some randomisation band - conservative on bands, since bot levels vary
    ///     within the wave.
    /// </summary>
    private bool WaveCanWriteNighttimeClamps(MongoId sessionId, BotGenerationDetails details)
    {
        var raidConfig = profileActivityService.GetProfileActivityRaidData(sessionId)?.RaidConfiguration;
        if (raidConfig is null || !weatherHelper.IsNightTime(raidConfig.TimeVariant, raidConfig.Location!))
        {
            return false;
        }

        if (
            !botConfig.Equipment.TryGetValue(botGeneratorHelper.GetBotEquipmentRole(details.Role.ToLowerInvariant()), out var equipConfig)
            || equipConfig?.Randomisation is null
        )
        {
            return false;
        }

        return equipConfig.Randomisation.Any(band => band.NighttimeChanges?.EquipmentModsModifiers is { Count: > 0 });
    }

    private GenerateBotInventoryBatchRequest BuildBatchRequest(
        MongoId sessionId,
        BotGenerationDetails waveDetails,
        List<PreparedWaveBot> bots
    )
    {
        return new GenerateBotInventoryBatchRequest
        {
            Shared = BotPayloadProjection.BuildSharedViews(
                sessionId,
                waveDetails.Role.ToLowerInvariant(),
                profileHelper,
                profileActivityService,
                weatherHelper,
                botGeneratorHelper,
                botEquipmentFilterService,
                botEquipmentModPoolService,
                presetHelper,
                itemFilterService,
                itemHelper,
                globalTable,
                botConfig,
                pmcConfig,
                repairConfig
            ),
            Bots =
            [
                .. bots.Select(entry =>
                    BotPayloadProjection.BuildBotSlice(
                        entry.Bot.Id.Value,
                        entry.Template,
                        entry.Details,
                        botInventoryGenerator.NativeTestSeed,
                        botLootCacheService,
                        handbookHelper,
                        pmcConfig
                    )
                ),
            ],
        };
    }
}
