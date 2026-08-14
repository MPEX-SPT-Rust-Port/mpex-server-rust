using System.Diagnostics;
using System.Text.Json;
using NUnit.Framework;
using SPTarkov.Server.Core.Generators.Bot;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Generators.Weapons;
using SPTarkov.Server.Core.Helpers;
using SPTarkov.Server.Core.Helpers.Bot;
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
/// Solution A: one native call per wave instead of one per bot. The shared database and config
/// views are 95.7% of a single-bot request's bytes, so batching a wave of N divides that share by N.
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
    private BotGeneratorHelper _botGeneratorHelper = default!;
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
        _botGeneratorHelper = di.GetService<BotGeneratorHelper>();
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
        // One case per bot, shared by both paths: FilterBotEquipment randomises the template it
        // returns, so calling BuildCase once per path would compare two different waves
        var cases = Enumerable.Range(0, botCount).Select(index => (Case: BuildCase(), Seed: (ulong?)(1000 + index))).ToList();

        var perBot = cases.Select(entry => SptNative.GenerateBotInventory(BuildSingleRequest(entry.Case, entry.Seed))).ToList();
        var batched = SptNative.GenerateBotInventoryBatch(BuildBatchRequest(cases));

        Assert.That(batched.Bots, Has.Count.EqualTo(botCount));
        for (var index = 0; index < botCount; index++)
        {
            Assert.That(
                Serialize(batched.Bots[index].Inventory),
                Is.EqualTo(Serialize(perBot[index].Inventory)),
                $"bot {index} diverged between the batched and per-bot paths"
            );
        }
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
            // Cases are built outside the timed regions: FilterBotEquipment is identical work on
            // both paths, and leaving it in would charge both for the same thing while adding its
            // randomisation to the variance
            var cases = Enumerable.Range(0, waveSize).Select(_ => (Case: BuildCase(), Seed: (ulong?)null)).ToList();

            // Warm both paths so neither pays the other's first-call heap growth
            _ = SptNative.GenerateBotInventory(BuildSingleRequest(cases[0].Case, null));
            _ = SptNative.GenerateBotInventoryBatch(BuildBatchRequest(cases[..1]));

            GC.Collect();
            GC.WaitForPendingFinalizers();
            GC.Collect();

            var perBotTimings = new List<double>();
            var parallelTimings = new List<double>();
            var batchTimings = new List<double>();

            // Measured outside the timed loops - each path serialises its own request once inside
            // the native wrapper, and counting the bytes there would charge it a second pass
            var perBotBytes = JsonSerializer
                .SerializeToUtf8Bytes(BuildSingleRequest(cases[0].Case, null), JsonUtil.JsonSerializerOptionsNoIndent!)
                .Length;
            var batchBytes =
                JsonSerializer.SerializeToUtf8Bytes(BuildBatchRequest(cases), JsonUtil.JsonSerializerOptionsNoIndent!).Length / waveSize;

            for (var run = 0; run < 5; run++)
            {
                var stopwatch = Stopwatch.StartNew();
                foreach (var entry in cases)
                {
                    _ = SptNative.GenerateBotInventory(BuildSingleRequest(entry.Case, null));
                }
                stopwatch.Stop();
                perBotTimings.Add(stopwatch.Elapsed.TotalMilliseconds / waveSize);

                // What GenerateBotWave actually does
                stopwatch = Stopwatch.StartNew();
                cases.AsParallel().ForAll(entry => SptNative.GenerateBotInventory(BuildSingleRequest(entry.Case, null)));
                stopwatch.Stop();
                parallelTimings.Add(stopwatch.Elapsed.TotalMilliseconds / waveSize);

                stopwatch = Stopwatch.StartNew();
                _ = SptNative.GenerateBotInventoryBatch(BuildBatchRequest(cases));
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
        return BotPayloadProjection.BuildRequest(
            new MongoId(),
            _sessionId,
            botCase.Template,
            botCase.Details,
            testSeed,
            _profileHelper,
            _profileActivityService,
            _weatherHelper,
            _botGeneratorHelper,
            _botEquipmentFilterService,
            _botLootGenerator.BotLootCacheService,
            _botEquipmentModGenerator.PresetHelper,
            _botEquipmentModGenerator.ItemFilterService,
            _botLootGenerator.HandbookHelper,
            _itemHelper,
            _botWeaponGenerator.GlobalTable,
            _botConfig,
            _pmcConfig,
            _botWeaponGenerator.RepairConfig
        );
    }

    private GenerateBotInventoryBatchRequest BuildBatchRequest(
        IEnumerable<((BotType Template, BotGenerationDetails Details) Case, ulong? Seed)> cases
    )
    {
        return new GenerateBotInventoryBatchRequest
        {
            Shared = BotPayloadProjection.BuildSharedViews(
                _sessionId,
                Role,
                _profileHelper,
                _profileActivityService,
                _weatherHelper,
                _botGeneratorHelper,
                _botEquipmentFilterService,
                _botEquipmentModGenerator.PresetHelper,
                _botEquipmentModGenerator.ItemFilterService,
                _itemHelper,
                _botWeaponGenerator.GlobalTable,
                _botConfig,
                _pmcConfig,
                _botWeaponGenerator.RepairConfig
            ),
            Bots =
            [
                .. cases.Select(entry =>
                    BotPayloadProjection.BuildBotSlice(
                        new MongoId(),
                        entry.Case.Template,
                        entry.Case.Details,
                        entry.Seed,
                        _botLootGenerator.BotLootCacheService,
                        _botLootGenerator.HandbookHelper,
                        _pmcConfig
                    )
                ),
            ],
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
