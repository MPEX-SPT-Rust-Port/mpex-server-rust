using System.Reflection;
using HarmonyLib;
using SPTarkov.Common.Models.Logging;
using SPTarkov.DI.Annotations;
using SPTarkov.Server.Core.Constants;
using SPTarkov.Server.Core.Helpers;
using SPTarkov.Server.Core.Helpers.Bot;
using SPTarkov.Server.Core.Helpers.InRaid;
using SPTarkov.Server.Core.Helpers.Items;
using SPTarkov.Server.Core.Helpers.Profile;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Common.Tables;
using SPTarkov.Server.Core.Models.Enums;
using SPTarkov.Server.Core.Models.Spt.Bots;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Mod;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Native;
using SPTarkov.Server.Core.Native.Bot;
using SPTarkov.Server.Core.Native.Db;
using SPTarkov.Server.Core.Services.Bot;
using SPTarkov.Server.Core.Services.Items;
using SPTarkov.Server.Core.Services.Profile;
using SPTarkov.Server.Core.Services.Server;
using SPTarkov.Server.Core.Utils.Cloners;

namespace SPTarkov.Server.Core.Generators.Bot;

/// <summary>
///     The batched wave path: one native call generates every inventory in the wave, with the
///     shared views on the wire once. Declines (returns null) whenever a mod could observe the
///     difference from the per-bot path - the same contract the three existing native dispatchers
///     honour - or when the wave could write nighttime equipment clamps, whose cross-bot feedback
///     loop only the per-bot path replays. Bots that fail are skipped with one Critical log each,
///     matching BotController.TryGenerateSingleBot.
///
///     One carve-out from "whenever a mod could observe the difference": pool and price hydration.
///     BotLootCacheService.GetLootFromCache (12 calls) runs once per level band here, not once per
///     bot, and HandbookHelper.GetTemplatePrice once per wave on the views-override arm (never on
///     the resident arm, whose prices were derived at publish); neither is in the decline set.
///     Both are patched constantly by economy mods, so declining on them would de-batch most
///     modded servers - and their per-bot results are identical anyway for bots sharing a band.
/// </summary>
[Injectable]
public class BotWaveBatcher(
    ISptLogger<BotWaveBatcher> logger,
    BotGenerator botGenerator,
    BotLevelGenerator botLevelGenerator,
    BotInventoryGenerator botInventoryGenerator,
    BotEquipmentFilterService botEquipmentFilterService,
    BotGeneratorHelper botGeneratorHelper,
    ProfileHelper profileHelper,
    ProfileActivityService profileActivityService,
    WeatherHelper weatherHelper,
    ItemHelper itemHelper,
    ItemFilterService itemFilterService,
    PresetHelper presetHelper,
    BotLootCacheService botLootCacheService,
    HandbookHelper handbookHelper,
    WeightedRandomHelper weightedRandomHelper,
    GlobalTable globalTable,
    MatchBotDetailsCacheService matchBotDetailsCacheService,
    BotConfig botConfig,
    PmcConfig pmcConfig,
    RepairConfig repairConfig,
    ICloner cloner,
    IReadOnlyList<SptMod> loadedMods,
    DbPublisher dbPublisher
)
{
    /// <summary>
    ///     The frozen 4.1.2 members of the four classes the batch path routes around. A live Harmony
    ///     patch on any of them means a mod expects per-bot semantics, so the batch declines - the
    ///     level is drawn natively rather than per bot by BotLevelGenerator, and the equipment filter
    ///     runs once per level band rather than once per bot. Same construction as
    ///     BotInventoryGenerator's set; GenerateBotWave itself is excluded because a patch on the
    ///     dispatcher wraps whichever path runs. Internal additions (the prelude/finish split,
    ///     PrepareBot, the two batch seams) are IsAssembly and fall out of the visibility filter on
    ///     their own.
    ///
    ///     SeasonalEventService is member-scoped where the four above are whole-type: only its two
    ///     christmas members are re-timed by the batch (ApplyBatchTemplateMutations runs them once
    ///     per level band), and the type carries a lot of unrelated event surface that seasonal mods
    ///     patch - sweeping it whole would de-batch those servers for no fidelity gain.
    /// </summary>
    private static readonly List<MethodBase> _hookableWaveMembers =
    [
        .. new[] { typeof(BotGenerator), typeof(BotLevelGenerator), typeof(BotEquipmentFilterService), typeof(Controllers.BotController) }
            .SelectMany(type =>
                type.GetMethods(
                    BindingFlags.Instance | BindingFlags.Static | BindingFlags.Public | BindingFlags.NonPublic | BindingFlags.DeclaredOnly
                )
            )
            .Where(method => !method.IsSpecialName && (method.IsPublic || method.IsFamily || method.IsFamilyOrAssembly))
            // Protected on another class, so nameof() cannot reach it
            .Where(method => method.Name != "GenerateBotWave"),
        // OfType drops a lookup that stopped resolving rather than putting a null in the set; the
        // decline test patches both, so a rename that slips past nameof() fails there
        .. new[]
        {
            typeof(SeasonalEventService).GetMethod(nameof(SeasonalEventService.ChristmasEventEnabled)),
            typeof(SeasonalEventService).GetMethod(nameof(SeasonalEventService.RemoveChristmasItemsFromBotInventory)),
        }.OfType<MethodBase>(),
    ];

    private sealed record PreparedWaveBot(BotBase Bot, BotType Template, BotGenerationDetails Details);

    /// <summary>
    ///     One level band's filtered template and the loot pools hydrated from it. The C#-side
    ///     BotType is kept alongside the wire members because BuildBatchRequest projects the band's
    ///     filtered appearance, health, skills and experience blocks onto the wire.
    /// </summary>
    private sealed record TemplateVariant(int LevelMin, int LevelMax, BotType Template, BotLootCache LootPools);

    /// <summary>
    ///     Whether the most recent native send carried the C#-built views override rather than
    ///     naming a resident-DB epoch. Test seam.
    /// </summary>
    internal bool LastSendIncludedViewsOverride { get; private set; }

    /// <summary>
    ///     Whether an override-less send off the resident DB is ever allowed: the kill switch is
    ///     off, and either no mods are loaded or the user vouched their mods don't write tables
    ///     directly.
    /// </summary>
    private bool ResidentDbEligible()
    {
        return ResidentDbDispatch.Eligible(
            dbPublisher,
            loadedMods.Count,
            botConfig.DisableNativeRequestCache,
            botConfig.TrustNativeRequestCacheWithMods
        );
    }

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

                    botGenerator.GenerateBotPrelude(sessionId, bot, template, detailsClone, nativeLevelAndFilter: true);
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
            var variants = new List<TemplateVariant>();
            var waveDetails = cloner.Clone(botGenerationDetails)!;
            waveDetails.RoleLowercase = waveDetails.Role.ToLowerInvariant();
            // Batch preludes no longer mutate templates, so any survivor's clone is the pristine
            // wave template
            var waveTemplate = survivors[0].Template;

            LevelGenerationView? levelGeneration = null;
            var range = new MinMax<int>(1, 1);
            if (waveDetails.IsPmc)
            {
                // The exp table the drawn level is summed out of rides the views (override bundle
                // or the resident DB); only the range is wave-varying
                var expTable = globalTable.Configuration.Exp.Level.ExperienceTable;
                range = botLevelGenerator.GetRelativePmcBotLevelRange(waveDetails, waveTemplate.BotExperience.Level, expTable.Length);
                levelGeneration = new LevelGenerationView { LevelMin = range.Min, LevelMax = range.Max };
            }

            var segments = EnumerateLevelSegments(
                waveDetails,
                range,
                // BotEquipmentFilterService.cs:28 - a PMC wave filters against the literal "pmc" entry
                waveDetails.IsPmc && botConfig.Equipment.TryGetValue("pmc", out var pmcFilters)
                    ? pmcFilters
                    : null,
                pmcConfig.LootItemLimitsRub
            );
            foreach (var segment in segments)
            {
                var variantDetails = cloner.Clone(waveDetails)!;
                variantDetails.BotLevel = segment.Min;
                var variantTemplate = cloner.Clone(waveTemplate)!;
                botGenerator.ApplyBatchTemplateMutations(sessionId, variantTemplate, variantDetails);
                var lootPools = BotPayloadProjection.BuildLootPools(botLootCacheService, variantTemplate, variantDetails, pmcConfig);
                variants.Add(new TemplateVariant(segment.Min, segment.Max, variantTemplate, lootPools));
            }

            var request = BuildBatchRequest(sessionId, waveDetails, survivors, levelGeneration, variants);
            if (!ResidentDbEligible())
            {
                LastSendIncludedViewsOverride = true;
                request.ViewsOverride = BotPayloadProjection.BuildViewsOverride(
                    presetHelper,
                    handbookHelper,
                    itemHelper,
                    globalTable,
                    itemFilterService,
                    botConfig,
                    pmcConfig,
                    repairConfig,
                    variants.Select(variant => variant.LootPools)
                );
                batchResult = SptNative.GenerateBotInventoryBatch(request);
            }
            else
            {
                batchResult = ResidentDbDispatch.Send(
                    dbPublisher,
                    epoch =>
                    {
                        request.Epoch = epoch;
                        return SptNative.GenerateBotInventoryBatch(request);
                    }
                );
                LastSendIncludedViewsOverride = false;
            }
        }
        catch (Exception e)
        {
            // The wave prep runs in here too: the per-bot path contains the same level-range,
            // filter and pool-hydration throws in its per-bot catch, and BotController's wave
            // caller has no catch of its own. A wholesale native failure is a native bug; on the
            // per-bot path every bot's call would have thrown the same way and the wave would come
            // back empty there too
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

                // Before everything else: CacheBot reads Info.Level
                // (MatchBotDetailsCacheService.cs:54)
                var native = envelope.Result;
                entry.Details.BotLevel = native.Level!.Value;
                entry.Bot.Info.Experience = native.Exp;
                entry.Bot.Info.Level = native.Level;

                // The prelude draws that moved native at ABI 38 (spec 2026-08-29), written back
                // instead of drawn. Enum.Parse (case-sensitive) throws on an unknown skill key
                // inside this per-bot try, so the bot is skipped and the wave survives - same
                // outcome as the legacy prelude's throw. CommonSkill.Progress has a clamping
                // setter (MaxSkillProgress), so a >5100 native value clamps here exactly as it
                // did legacy; the Rust golden pins the unclamped wire value.
                entry.Bot.Customization.Head = new MongoId(native.Customization!.Head);
                entry.Bot.Customization.Body = new MongoId(native.Customization.Body);
                entry.Bot.Customization.Feet = new MongoId(native.Customization.Feet);
                entry.Bot.Customization.Hands = new MongoId(native.Customization.Hands);
                entry.Bot.Customization.Voice = new MongoId(native.Customization.Voice);
                entry.Bot.Health = ToBotBaseHealth(native.Health!);
                entry.Bot.Skills = new Skills
                {
                    Common = native
                        .Skills!.Common.Select(skill => new CommonSkill
                        {
                            Id = Enum.Parse<SkillTypes>(skill.Id),
                            Progress = skill.Progress,
                            PointsEarnedDuringSession = 0,
                            LastAccess = 0,
                        })
                        .ToList(),
                    Mastering = native
                        .Skills.Mastering.Select(skill => new MasterySkill { Id = skill.Id, Progress = skill.Progress })
                        .ToList(),
                    Points = 0,
                };
                entry.Bot.Info.Settings.Experience = native.SettingsExperience!.Value;
                if (native.GameVersion is not null)
                {
                    entry.Bot.Info.GameVersion = native.GameVersion;
                    entry.Bot.Info.MemberCategory = (MemberCategory)native.MemberCategory!.Value;
                    if (native.SelectedMemberCategory is not null)
                    {
                        entry.Bot.Info.SelectedMemberCategory = (MemberCategory)native.SelectedMemberCategory.Value;
                    }
                }

                entry.Bot.Inventory = native.Inventory;
                if (!entry.Details.ClearBotContainerCacheAfterGeneration)
                {
                    botInventoryGenerator.RestoreContainerGrids(entry.Bot.Id.Value, native.ContainerGrids);
                }

                botInventoryGenerator.ReplayRandomisationClamps(entry.Details, native.RandomisationClamps);
                botGenerator.GenerateBotFinish(entry.Bot, entry.Details, nativeDogtag: true);

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

    private static BotBaseHealth ToBotBaseHealth(BotHealthResultView view)
    {
        return new BotBaseHealth
        {
            Hydration = new CurrentMinMax { Current = view.Hydration.Current, Maximum = view.Hydration.Maximum },
            Energy = new CurrentMinMax { Current = view.Energy.Current, Maximum = view.Energy.Maximum },
            Temperature = new CurrentMinMax { Current = view.Temperature.Current, Maximum = view.Temperature.Maximum },
            BodyParts = view.BodyParts.ToDictionary(
                part => part.Key,
                part => new BodyPartHealth
                {
                    Health = new CurrentMinMax { Current = part.Value.Current, Maximum = part.Value.Maximum },
                }
            ),
            UpdateTime = 0,
            Immortal = false,
        };
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

        // A mod substituted one of the three; only per-bot generation routes through them
        if (
            botGenerator.GetType() != typeof(BotGenerator)
            || botLevelGenerator.GetType() != typeof(BotLevelGenerator)
            || botEquipmentFilterService.GetType() != typeof(BotEquipmentFilterService)
        )
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

    /// <summary>
    ///     Split the wave's level range into segments on which every pre-call band lookup is
    ///     constant. Edges come from the five band sources consulted per bot: the filter's
    ///     blacklist/whitelist/weighting lists and its randomisation list
    ///     (BotEquipmentFilterService.cs:29-37 - for a PMC wave all four read Equipment["pmc"]),
    ///     plus the loot price bands (BotPayloadProjection GetSingleItemLootPriceLimits). All are
    ///     FirstOrDefault over inclusive ranges, so outcomes are piecewise-constant between
    ///     adjacent band edges; spurious edges only cost a duplicate variant, never correctness.
    /// </summary>
    internal static List<MinMax<int>> EnumerateLevelSegments(
        BotGenerationDetails waveDetails,
        MinMax<int> range,
        EquipmentFilters? pmcEquipmentFilters,
        List<MinMaxLootItemValue> lootItemLimitsRub
    )
    {
        if (!waveDetails.IsPmc)
        {
            // Non-PMC bots are always level 1 (BotLevelGenerator.cs:23-26)
            return [new MinMax<int>(1, 1)];
        }

        var edges = new SortedSet<int>();
        void AddBands(IEnumerable<MinMax<int>>? bands)
        {
            foreach (var band in bands ?? [])
            {
                edges.Add(band.Min);
                edges.Add(band.Max + 1);
            }
        }

        AddBands(pmcEquipmentFilters?.Blacklist?.Select(filter => filter.LevelRange));
        AddBands(pmcEquipmentFilters?.Whitelist?.Select(filter => filter.LevelRange));
        AddBands(pmcEquipmentFilters?.WeightingAdjustmentsByBotLevel?.Select(adjustment => adjustment.LevelRange));
        AddBands(pmcEquipmentFilters?.Randomisation?.Select(details => details.LevelRange));

        foreach (var band in lootItemLimitsRub)
        {
            // Double-valued bands (PmcConfig.cs:139); integer-level outcomes change at ceil(Min)
            // and floor(Max) + 1
            edges.Add((int)Math.Ceiling(band.Min));
            edges.Add((int)Math.Floor(band.Max) + 1);
        }

        var segments = new List<MinMax<int>>();
        var start = range.Min;
        foreach (var cut in edges.Where(edge => edge > range.Min && edge <= range.Max))
        {
            segments.Add(new MinMax<int>(start, cut - 1));
            start = cut;
        }

        segments.Add(new MinMax<int>(start, range.Max));

        return segments;
    }

    private GenerateBotInventoryBatchRequest BuildBatchRequest(
        MongoId sessionId,
        BotGenerationDetails waveDetails,
        List<PreparedWaveBot> bots,
        LevelGenerationView? levelGeneration,
        List<TemplateVariant> variants
    )
    {
        return new GenerateBotInventoryBatchRequest
        {
            // The dispatch site stamps the resident epoch on an eligible send; 0 rides with the
            // views override
            Epoch = 0,
            Shared = BotPayloadProjection.BuildSharedVarying(
                sessionId,
                profileHelper,
                profileActivityService,
                weatherHelper,
                botConfig,
                levelGeneration,
                [
                    .. variants.Select(variant => new BotTemplateVariantView
                    {
                        LevelMin = variant.LevelMin,
                        LevelMax = variant.LevelMax,
                        Template = BotPayloadProjection.BuildTemplateView(variant.Template),
                        LootPools = variant.LootPools,
                        Appearance = variant.Template.BotAppearance,
                        Health = variant.Template.BotHealth,
                        Skills = variant.Template.BotSkills,
                        ExperienceReward = variant.Template.BotExperience.Reward,
                    }),
                ]
            ),
            Bots =
            [
                .. bots.Select(entry =>
                    BotPayloadProjection.BuildBotSlice(
                        entry.Bot.Id.Value,
                        entry.Details,
                        botInventoryGenerator.NativeTestSeed,
                        isNikita: string.Equals(entry.Bot.Info.Nickname, "nikita", StringComparison.OrdinalIgnoreCase)
                    )
                ),
            ],
        };
    }
}
