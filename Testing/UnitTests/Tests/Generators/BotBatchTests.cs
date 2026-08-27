using System.Diagnostics;
using System.Text.Json;
using NUnit.Framework;
using SPTarkov.Server.Core.Generators.Bot;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Generators.Weapons;
using SPTarkov.Server.Core.Helpers;
using SPTarkov.Server.Core.Helpers.InRaid;
using SPTarkov.Server.Core.Helpers.Items;
using SPTarkov.Server.Core.Helpers.Profile;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Common.Tables;
using SPTarkov.Server.Core.Models.Spt.Bots;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Native;
using SPTarkov.Server.Core.Native.Bot;
using SPTarkov.Server.Core.Servers;
using SPTarkov.Server.Core.Services.Bot;
using SPTarkov.Server.Core.Services.Items;
using SPTarkov.Server.Core.Services.Profile;
using SPTarkov.Server.Core.Utils;
using SPTarkov.Server.Core.Utils.Cloners;
using ProfileInfo = SPTarkov.Server.Core.Models.Eft.Profile.Info;

namespace UnitTests.Tests.Generators;

/// <summary>
/// Solution A: one native call per wave instead of one per bot. Since the level fold the per-bot
/// slice is only an id, a seed and the generation details - everything else, the templates and loot
/// views included, rides the shared block - so batching a wave of N divides essentially the whole
/// request by N. See BENCHMARK.md's *Batched wave* for the measurement.
///
/// <see cref="BatchGeneratesTheSameBotsAsThePerBotPath"/> is the correctness gate - the whole
/// exercise is worthless if the batched path draws differently - and
/// <see cref="WaveCostPerBot"/> is the measurement it makes trustworthy.
/// </summary>
[TestFixture]
[NonParallelizable]
public class BotBatchTests
{
    private const string Role = "assault";

    private BotWeaponGenerator _botWeaponGenerator = default!;
    private BotLootGenerator _botLootGenerator = default!;
    private BotEquipmentModGenerator _botEquipmentModGenerator = default!;
    private ProfileHelper _profileHelper = default!;
    private ItemHelper _itemHelper = default!;
    private WeatherHelper _weatherHelper = default!;
    private ProfileActivityService _profileActivityService = default!;
    private BotEquipmentFilterService _botEquipmentFilterService = default!;
    private BotConfig _botConfig = default!;
    private PmcConfig _pmcConfig = default!;
    private BotTable _botTable = default!;
    private ICloner _cloner = default!;

    private MongoId _sessionId;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        var di = DI.GetInstance();

        _botWeaponGenerator = di.GetService<BotWeaponGenerator>();
        _botLootGenerator = di.GetService<BotLootGenerator>();
        _botEquipmentModGenerator = di.GetService<BotEquipmentModGenerator>();
        _profileHelper = di.GetService<ProfileHelper>();
        _itemHelper = di.GetService<ItemHelper>();
        _weatherHelper = di.GetService<WeatherHelper>();
        _profileActivityService = di.GetService<ProfileActivityService>();
        _botEquipmentFilterService = di.GetService<BotEquipmentFilterService>();
        _botConfig = di.GetService<BotConfig>();
        _pmcConfig = di.GetService<PmcConfig>();
        _botTable = di.GetService<BotTable>();
        _cloner = di.GetService<ICloner>();

        _sessionId = new MongoId();
        di.GetService<SaveServer>().CreateProfile(new ProfileInfo { ProfileId = _sessionId });
    }

    /// <summary>
    /// Seeded, so both paths draw the same numbers in the same order. A batched bot that differs
    /// from its per-bot twin means the refactor moved state across a bot boundary.
    /// </summary>
    [Test]
    public void BatchGeneratesTheSameBotsAsThePerBotPath()
    {
        const int botCount = 4;
        // One case for the whole wave, shared by both paths. FilterBotEquipment randomises the
        // template it returns, so building one per path would compare two different waves - and the
        // batch now carries one filtered template per level band, which for a non-PMC wave (every
        // bot level 1) is the single [1, 1] band, so the per-bot arm has to run the same template
        var waveCase = BuildCase();
        var seeds = Enumerable.Range(0, botCount).Select(index => (ulong?)(1000 + index)).ToList();

        var perBot = seeds.Select(seed => SptNative.GenerateBotInventory(BuildSingleRequest(waveCase, seed))).ToList();
        var batched = SptNative.GenerateBotInventoryBatch(BuildBatchRequest(waveCase, seeds));

        Assert.That(batched.Bots, Has.Count.EqualTo(botCount));
        for (var index = 0; index < botCount; index++)
        {
            Assert.That(
                Serialize(batched.Bots[index].Result!.Inventory),
                Is.EqualTo(Serialize(perBot[index].Inventory)),
                $"bot {index} diverged between the batched and per-bot paths"
            );
        }
    }

    /// <summary>
    /// The wire contract for a bot that fails inside the batch: its own envelope carries the error
    /// and the rest of the wave still comes back. Same poison as the native side's
    /// <c>batch_isolates_a_failing_bot</c> - an equipment config with a nighttime band but no
    /// equipmentMods, generated at night, errors at the top of equipment generation.
    /// </summary>
    [Test]
    public void AFailingBotComesBackAsAnErrorEnvelopeOnItsOwn()
    {
        var request = BuildBatchRequest(BuildCase(), [null, null]);

        request.Shared.IsNightTime = true;
        request.ViewsOverride!.Equipment["poisoned"] = new EquipmentFilters
        {
            Randomisation =
            [
                new RandomisationDetails
                {
                    LevelRange = new MinMax<int> { Min = 1, Max = 99 },
                    NighttimeChanges = new NighttimeChanges
                    {
                        EquipmentModsModifiers = new Dictionary<string, float> { ["front_plate"] = 30 },
                    },
                },
            ],
        };
        // Only the second bot names the poisoned config; the first is untouched `assault`
        request.Bots[1].Details.RoleLowercase = "poisoned";

        var result = SptNative.GenerateBotInventoryBatch(request);

        Assert.That(result.Bots, Has.Count.EqualTo(2));
        Assert.That(result.Bots[0].Result, Is.Not.Null);
        Assert.That(result.Bots[0].Error, Is.Null);
        Assert.That(result.Bots[1].Result, Is.Null);
        Assert.That(result.Bots[1].Error, Is.Not.Null);
    }

    /// <summary>
    /// The number the batching question turns on: wall clock per bot, both paths, same wave.
    ///
    /// Three arms, because the sequential per-bot arm is not what production runs -
    /// <c>BotController.GenerateBotWave</c> generates a wave under <c>.AsParallel()</c>. Comparing a
    /// single-threaded batch against a single-threaded loop measures CPU time, which flatters the
    /// batch by roughly the core count. The parallel arm is the honest baseline.
    /// </summary>
    [Test]
    [Explicit("benchmark, run on demand in Release")]
    public void WaveCostPerBot()
    {
        // The wave sizes that actually occur: BotConfig.PresetBatch runs 1 (some bosses) to 50, with
        // a median of 10 - and 45 for `assault`, the role most waves are made of
        foreach (var waveSize in new[] { 45, 20, 10, 5, 1 })
        {
            // The wave's case is built outside the timed regions: FilterBotEquipment is identical
            // work on both paths, and leaving it in would charge both for the same thing while
            // adding its randomisation to the variance. Production splits that differently now -
            // the batch arm runs the filter once per level band inside TryGenerateWave, which is
            // work the per-bot arms here pay outside the clock - so this measures the native call,
            // not the whole dispatch
            var waveCase = BuildCase();
            var seeds = Enumerable.Repeat((ulong?)null, waveSize).ToList();

            // Warm both paths so neither pays the other's first-call heap growth
            _ = SptNative.GenerateBotInventory(BuildSingleRequest(waveCase, null));
            _ = SptNative.GenerateBotInventoryBatch(BuildBatchRequest(waveCase, seeds[..1]));

            GC.Collect();
            GC.WaitForPendingFinalizers();
            GC.Collect();

            var perBotTimings = new List<double>();
            var parallelTimings = new List<double>();
            var batchTimings = new List<double>();

            // Measured outside the timed loops - each path serialises its own request once inside
            // the native wrapper, and counting the bytes there would charge it a second pass
            var perBotBytes = JsonSerializer
                .SerializeToUtf8Bytes(BuildSingleRequest(waveCase, null), JsonUtil.JsonSerializerOptionsNoIndent!)
                .Length;
            var batchBytes =
                JsonSerializer.SerializeToUtf8Bytes(BuildBatchRequest(waveCase, seeds), JsonUtil.JsonSerializerOptionsNoIndent!).Length
                / waveSize;

            for (var run = 0; run < 5; run++)
            {
                var stopwatch = Stopwatch.StartNew();
                foreach (var seed in seeds)
                {
                    _ = SptNative.GenerateBotInventory(BuildSingleRequest(waveCase, seed));
                }
                stopwatch.Stop();
                perBotTimings.Add(stopwatch.Elapsed.TotalMilliseconds / waveSize);

                // What GenerateBotWave actually does
                stopwatch = Stopwatch.StartNew();
                seeds.AsParallel().ForAll(seed => SptNative.GenerateBotInventory(BuildSingleRequest(waveCase, seed)));
                stopwatch.Stop();
                parallelTimings.Add(stopwatch.Elapsed.TotalMilliseconds / waveSize);

                stopwatch = Stopwatch.StartNew();
                _ = SptNative.GenerateBotInventoryBatch(BuildBatchRequest(waveCase, seeds));
                stopwatch.Stop();
                batchTimings.Add(stopwatch.Elapsed.TotalMilliseconds / waveSize);
            }

            var perBot = Median(perBotTimings);
            var parallel = Median(parallelTimings);
            var batch = Median(batchTimings);
            TestContext.Out.WriteLine(
                $"wave={waveSize, 2}  serial: {perBot, 7:F2}  parallel: {parallel, 7:F2}  batched: {batch, 7:F2} ms/bot   "
                    + $"({perBotBytes / 1024.0 / 1024.0:F2} -> {batchBytes / 1024.0 / 1024.0:F2} MiB/bot)   "
                    + $"speedup vs serial: {perBot / batch:F2}x  vs parallel: {parallel / batch:F2}x"
            );
        }
    }

    private GenerateBotInventoryRequest BuildSingleRequest((BotType Template, BotGenerationDetails Details) botCase, ulong? testSeed)
    {
        var request = BotPayloadProjection.BuildRequest(
            new MongoId(),
            _sessionId,
            botCase.Template,
            botCase.Details,
            testSeed,
            _profileHelper,
            _profileActivityService,
            _weatherHelper,
            _botLootGenerator.BotLootCacheService,
            _botConfig,
            _pmcConfig
        );
        request.ViewsOverride = BuildViewsOverride([request.LootPools]);

        return request;
    }

    /// <summary>
    /// Both paths send the views override (epoch 0): what is under test is batch-vs-per-bot
    /// equivalence, not residency - the resident/override equivalence gate is BotResidentDbTests.
    /// </summary>
    private BotViewsOverride BuildViewsOverride(IEnumerable<BotLootCache> lootPools)
    {
        return BotPayloadProjection.BuildViewsOverride(
            _botEquipmentModGenerator.PresetHelper,
            _botLootGenerator.HandbookHelper,
            _itemHelper,
            _botWeaponGenerator.GlobalTable,
            _botEquipmentModGenerator.ItemFilterService,
            _botConfig,
            _pmcConfig,
            _botWeaponGenerator.RepairConfig,
            lootPools
        );
    }

    /// <summary>
    /// One wave off one case: the template and its two loot views ride the shared block as a single
    /// level-band variant, and each bot contributes only its identity, details and seed.
    /// </summary>
    private GenerateBotInventoryBatchRequest BuildBatchRequest(
        (BotType Template, BotGenerationDetails Details) waveCase,
        IEnumerable<ulong?> seeds
    )
    {
        var lootPools = BotPayloadProjection.BuildLootPools(
            _botLootGenerator.BotLootCacheService,
            waveCase.Template,
            waveCase.Details,
            _pmcConfig
        );

        return new GenerateBotInventoryBatchRequest
        {
            Epoch = 0,
            ViewsOverride = BuildViewsOverride([lootPools]),
            Shared = BotPayloadProjection.BuildSharedVarying(
                _sessionId,
                _profileHelper,
                _profileActivityService,
                _weatherHelper,
                _botConfig,
                // Non-PMC: the native side takes the constant level 1 without drawing, so the wave
                // ships no level inputs and its one band is [1, 1]
                null,
                [
                    new BotTemplateVariantView
                    {
                        LevelMin = 1,
                        LevelMax = 1,
                        Template = BotPayloadProjection.BuildTemplateView(waveCase.Template),
                        LootPools = lootPools,
                    },
                ]
            ),
            Bots = [.. seeds.Select(seed => BotPayloadProjection.BuildBotSlice(new MongoId(), waveCase.Details, seed))],
        };
    }

    private (BotType Template, BotGenerationDetails Details) BuildCase()
    {
        var details = new BotGenerationDetails
        {
            Role = Role,
            RoleLowercase = Role,
            Side = "Savage",
            BotDifficulty = "normal",
            GameVersion = "standard",
            BotLevel = 1,
        };

        var template = _cloner.Clone(_botTable.Types[Role])!;
        _botEquipmentFilterService.FilterBotEquipment(_sessionId, template, details);

        return (template, details);
    }

    /// <summary>
    /// Every generated item carries a fresh <c>MongoId</c>, whose counter half is process-global and
    /// so never repeats between two calls. <c>LootIdNormalizer</c> rewrites them to positional
    /// placeholders, which is what <c>BotParityTests</c> compares the two paths on too.
    /// </summary>
    private static string Serialize(object value)
    {
        return LootIdNormalizer.Normalize(JsonSerializer.Serialize(value, JsonUtil.JsonSerializerOptionsNoIndent!));
    }

    private static double Median(List<double> timings)
    {
        var sorted = timings.Order().ToList();
        return sorted.Count % 2 == 1 ? sorted[sorted.Count / 2] : (sorted[sorted.Count / 2 - 1] + sorted[sorted.Count / 2]) / 2;
    }
}
