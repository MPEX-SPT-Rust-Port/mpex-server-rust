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
using SPTarkov.Server.Core.Services.Profile;
using SPTarkov.Server.Core.Utils;
using SPTarkov.Server.Core.Utils.Cloners;
using ProfileInfo = SPTarkov.Server.Core.Models.Eft.Profile.Info;

namespace UnitTests.Tests.Generators;

/// <summary>
/// Diagnostic: splits the native bot path into its phases so the 45-65x figure can be attributed to
/// a component instead of inferred. Dumps the request JSON to /tmp so the Rust side can be timed on
/// the same bytes.
/// </summary>
[TestFixture]
[Explicit("diagnostic, run on demand in Release")]
[NonParallelizable]
public class BotNativePhaseDiagTests
{
    private const int WarmupRuns = 2;
    private const int TimedRuns = 20;

    private static readonly string[] _roles = ["usec", "assault"];

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

    [Test]
    public void NativePhaseBreakdown()
    {
        foreach (var role in _roles)
        {
            var build = new List<double>(TimedRuns);
            var serialize = new List<double>(TimedRuns);
            var native = new List<double>(TimedRuns);
            var itemsView = new List<double>(TimedRuns);
            var bytes = 0;

            for (var run = 0; run < WarmupRuns; run++)
            {
                var (t, d) = BuildCase(role);
                var r = BuildRequest(t, d);
                _ = SptNative.Generate<BotInventoryResult>(
                    LootExport.BotInventory,
                    JsonSerializer.SerializeToUtf8Bytes(r, JsonUtil.JsonSerializerOptionsNoIndent!)
                );
            }

            GC.Collect();
            GC.WaitForPendingFinalizers();
            GC.Collect();

            for (var run = 0; run < TimedRuns; run++)
            {
                var (template, details) = BuildCase(role);

                var sw = Stopwatch.StartNew();
                var request = BuildRequest(template, details);
                sw.Stop();
                build.Add(sw.Elapsed.TotalMilliseconds);

                sw = Stopwatch.StartNew();
                var utf8 = JsonSerializer.SerializeToUtf8Bytes(request, JsonUtil.JsonSerializerOptionsNoIndent!);
                sw.Stop();
                serialize.Add(sw.Elapsed.TotalMilliseconds);
                bytes = utf8.Length;

                if (run == 0)
                {
                    File.WriteAllBytes($"/tmp/bot-request-{role}.json", utf8);
                }

                sw = Stopwatch.StartNew();
                _ = SptNative.Generate<BotInventoryResult>(LootExport.BotInventory, utf8);
                sw.Stop();
                native.Add(sw.Elapsed.TotalMilliseconds);
            }

            // Separate loop: measuring it inside the one above builds the items view twice per
            // iteration, which doubles the allocation rate and charges the GC to the wrong phase
            for (var run = 0; run < TimedRuns; run++)
            {
                var sw = Stopwatch.StartNew();
                _ = PayloadProjectionShim(_itemHelper);
                sw.Stop();
                itemsView.Add(sw.Elapsed.TotalMilliseconds);
            }

            TestContext.Out.WriteLine($"=== {role}  request={bytes / 1024.0 / 1024.0:F2} MiB ===");
            Report("  BuildItemsView only", itemsView);
            Report("  BuildRequest (total)", build);
            Report("  Serialize to UTF-8", serialize);
            Report("  Generate (ffi+rust+deser)", native);
            TestContext.Out.WriteLine($"  sum of medians: {Median(build) + Median(serialize) + Median(native):F2} ms");
        }
    }

    private object PayloadProjectionShim(ItemHelper itemHelper)
    {
        return SPTarkov.Server.Core.Native.Loot.PayloadProjection.BuildItemsView(itemHelper.TemplateTable.Items);
    }

    private GenerateBotInventoryRequest BuildRequest(BotType template, BotGenerationDetails details)
    {
        return BotPayloadProjection.BuildRequest(
            new MongoId(),
            _sessionId,
            template,
            details,
            null,
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

    private (BotType Template, BotGenerationDetails Details) BuildCase(string role)
    {
        var details = role switch
        {
            "assault" => new BotGenerationDetails
            {
                Role = "assault",
                RoleLowercase = "assault",
                Side = "Savage",
                BotDifficulty = "normal",
                GameVersion = "standard",
                BotLevel = 1,
            },
            "usec" => new BotGenerationDetails
            {
                Role = "pmcUSEC",
                RoleLowercase = "pmcusec",
                Side = "Usec",
                BotDifficulty = "normal",
                GameVersion = "standard",
                BotLevel = 1,
                IsPmc = true,
            },
            _ => throw new ArgumentOutOfRangeException(nameof(role), role, "no case defined"),
        };

        var template = _cloner.Clone(_botTable.Types[role])!;
        _botEquipmentFilterService.FilterBotEquipment(_sessionId, template, details);

        return (template, details);
    }

    private static void Report(string label, List<double> timings)
    {
        TestContext.Out.WriteLine(
            $"{label, -30} median={Median(timings):F2} ms  mean={timings.Average():F2} ms  "
                + $"min={timings.Min():F2} ms  max={timings.Max():F2} ms"
        );
    }

    private static double Median(List<double> timings)
    {
        var sorted = timings.Order().ToList();
        return sorted.Count % 2 == 1 ? sorted[sorted.Count / 2] : (sorted[sorted.Count / 2 - 1] + sorted[sorted.Count / 2]) / 2;
    }
}
