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
using SPTarkov.Server.Core.Native.Bot;
using SPTarkov.Server.Core.Servers;
using SPTarkov.Server.Core.Services.Bot;
using SPTarkov.Server.Core.Services.Profile;
using SPTarkov.Server.Core.Utils;
using SPTarkov.Server.Core.Utils.Cloners;
using ProfileInfo = SPTarkov.Server.Core.Models.Eft.Profile.Info;

namespace UnitTests.Tests.Generators;

/// <summary>
/// The native bot request is rebuilt, serialised and re-parsed for every single bot, and ~92% of a
/// bot's wall clock is that round trip - so a byte on this wire is paid for per bot, not per raid.
/// These tests pin the two places the payload carried the same presets more than once.
/// </summary>
[TestFixture]
[NonParallelizable]
public class BotPayloadSizeTests
{
    private BotWeaponGenerator _botWeaponGenerator = default!;
    private BotLootGenerator _botLootGenerator = default!;
    private BotEquipmentModGenerator _botEquipmentModGenerator = default!;
    private BotGeneratorHelper _botGeneratorHelper = default!;
    private ProfileHelper _profileHelper = default!;
    private ItemHelper _itemHelper = default!;
    private WeatherHelper _weatherHelper = default!;
    private ProfileActivityService _profileActivityService = default!;
    private BotEquipmentFilterService _botEquipmentFilterService = default!;
    private BotEquipmentModPoolService _botEquipmentModPoolService = default!;
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
        _botEquipmentModPoolService = di.GetService<BotEquipmentModPoolService>();
        _botConfig = di.GetService<BotConfig>();
        _pmcConfig = di.GetService<PmcConfig>();
        _botTable = di.GetService<BotTable>();
        _cloner = di.GetService<ICloner>();

        _sessionId = new MongoId();
        di.GetService<SaveServer>().CreateProfile(new ProfileInfo { ProfileId = _sessionId });
    }

    /// <summary>
    /// <c>itemPresets</c> and <c>presetsById</c> were projected from one dictionary and serialised
    /// twice - the same ~0.47 MiB on the wire in both, about 9% of the request.
    /// </summary>
    [Test]
    public void PresetsAreSentOnce()
    {
        using var request = JsonDocument.Parse(SerializeRequest());

        Assert.That(
            request.RootElement.GetProperty("viewsOverride").TryGetProperty("presetsById", out _),
            Is.False,
            "presetsById duplicates itemPresets verbatim; the native side should read one map"
        );
    }

    /// <summary>
    /// Every default preset is an entry of <c>itemPresets</c> - <c>PresetHelper</c> resolves them
    /// out of <c>globalTable.ItemPresets</c> - so the tpl only needs to carry the preset's id.
    /// </summary>
    [Test]
    public void DefaultPresetsAreSentAsIdsNotWholePresets()
    {
        using var request = JsonDocument.Parse(SerializeRequest());

        var defaults = request.RootElement.GetProperty("viewsOverride").GetProperty("defaultPresetsByTpl");
        Assert.That(defaults.EnumerateObject().Any(), Is.True, "fixture needs a database with default presets");

        foreach (var entry in defaults.EnumerateObject())
        {
            Assert.That(
                entry.Value.ValueKind,
                Is.EqualTo(JsonValueKind.String),
                $"default preset for {entry.Name} should be the preset id, resolved against itemPresets"
            );
        }
    }

    /// <summary>
    /// The `items` block is 81% of the request, and most templates carry the default for the
    /// extra-size and blocking members. Every native read of those is an <c>unwrap_or</c> of that
    /// same default, so omitting them cannot change generation - but a member that grows a real
    /// meaning later would be silently dropped, which is what this pins.
    /// </summary>
    [Test]
    public void OmitsDefaultsTheNativeSideUnwraps()
    {
        using var request = JsonDocument.Parse(SerializeRequest());
        var items = request.RootElement.GetProperty("viewsOverride").GetProperty("items");

        string[] omitted =
        [
            "extraSizeUp",
            "extraSizeDown",
            "extraSizeLeft",
            "extraSizeRight",
            "extraSizeForceAdd",
            "sizeReduceRight",
            "hasHinge",
            "faceShieldComponent",
            "blocksEarpiece",
            "blocksEyewear",
            "blocksFaceCover",
            "blocksHeadwear",
            "blocksFolding",
            "blocksCollapsible",
            "blockLeftStance",
            "blocksArmorVest",
        ];

        foreach (var item in items.EnumerateObject())
        {
            foreach (var member in omitted)
            {
                if (item.Value.TryGetProperty(member, out var value))
                {
                    Assert.That(
                        value.ValueKind is JsonValueKind.False || (value.ValueKind is JsonValueKind.Number && value.GetInt32() == 0),
                        Is.False,
                        $"{item.Name}.{member} is on the wire at its default; the native side unwraps to it anyway"
                    );
                }
            }

            // slots, chambers and cartridges are all SlotView, and every native read of its
            // `required` is an unwrap_or(false) too
            foreach (var slotArray in new[] { "slots", "chambers", "cartridges" })
            {
                if (!item.Value.TryGetProperty(slotArray, out var slots))
                {
                    continue;
                }

                foreach (var slot in slots.EnumerateArray())
                {
                    if (slot.TryGetProperty("required", out var required))
                    {
                        Assert.That(
                            required.ValueKind is JsonValueKind.False,
                            Is.False,
                            $"{item.Name}.{slotArray}[].required is on the wire at its default; the native side unwraps to it anyway"
                        );
                    }
                }
            }
        }

        // questItem keeps null and false distinct, and canSellOnRagfair defaults to true in the
        // database while the native read unwraps to false - both must keep paying their bytes
        Assert.That(
            items
                .EnumerateObject()
                .Any(item => item.Value.TryGetProperty("questItem", out var quest) && quest.ValueKind is JsonValueKind.False),
            Is.True,
            "questItem false must stay on the wire - the sealed container pool filters on null"
        );
    }

    /// <summary>
    /// Both fixes together, as the thing they are actually for: bytes crossing the FFI per bot.
    /// Sending the presets twice and inlining the defaults cost 766,817 bytes of a 5,583,351-byte
    /// request, and the always-default members another ~595,000; the budget is what that leaves,
    /// plus headroom for database churn. It is a regression guard, so re-baseline it deliberately -
    /// never widen it to make a diff pass.
    /// </summary>
    [Test]
    public void RequestStaysUnderTheWireBudget()
    {
        const int budgetBytes = 4_300_000;

        Assert.That(
            SerializeRequest().Length,
            Is.LessThan(budgetBytes),
            "the bot request is rebuilt and re-parsed per bot; a regression here is paid on every spawn"
        );
    }

    /// <summary>
    /// The batch request carries the shared block once for the whole wave, so its per-bot cost is
    /// <c>shared/N + slice</c>. Nothing else guards that block from re-inflating - which is the very
    /// thing both fixes exist to protect - so this pins the ratio rather than an absolute size: at a
    /// wave of 10 a bot must cost under a ninth of a single-bot request, against a real ratio of
    /// about a tenth. The template and its loot views moved onto the shared block as per-level-band
    /// variants, leaving a slice with only an id, a seed and the generation details.
    /// </summary>
    [Test]
    public void BatchAmortisesTheSharedBlock()
    {
        const int waveSize = 10;

        var singleBytes = SerializeRequest().Length;
        var batchBytes = JsonSerializer.SerializeToUtf8Bytes(BuildBatchRequest(waveSize), JsonUtil.JsonSerializerOptionsNoIndent!).Length;

        Assert.That(
            batchBytes / waveSize,
            Is.LessThan(singleBytes / 9),
            "the shared block stopped amortising - something per-bot moved into it, or a per-bot member grew"
        );
    }

    private GenerateBotInventoryBatchRequest BuildBatchRequest(int waveSize)
    {
        var details = new BotGenerationDetails
        {
            Role = "pmcUSEC",
            RoleLowercase = "pmcusec",
            Side = "Usec",
            BotDifficulty = "normal",
            GameVersion = "standard",
            BotLevel = 1,
            IsPmc = true,
        };

        // One filtered template for the whole wave, on the shared block as a level-band variant -
        // production splits the wave's level range into bands and ships one of these per band
        var template = _cloner.Clone(_botTable.Types["usec"])!;
        _botEquipmentFilterService.FilterBotEquipment(_sessionId, template, details);
        var lootPools = BotPayloadProjection.BuildLootPools(_botLootGenerator.BotLootCacheService, template, details, _pmcConfig);
        var expTable = _botWeaponGenerator.GlobalTable.Configuration.Exp.Level.ExperienceTable;

        return new GenerateBotInventoryBatchRequest
        {
            // The override send: the wire whose per-bot byte cost this fixture pins - the
            // resident send is a fraction of it by construction
            Epoch = 0,
            ViewsOverride = BuildViewsOverride([lootPools]),
            Shared = BotPayloadProjection.BuildSharedVarying(
                _sessionId,
                _profileHelper,
                _profileActivityService,
                _weatherHelper,
                _botEquipmentModPoolService,
                _itemHelper,
                _botConfig,
                // A PMC wave draws its levels natively, so it carries the draw's inputs once
                new LevelGenerationView { LevelMin = 1, LevelMax = expTable.Length },
                [
                    new BotTemplateVariantView
                    {
                        LevelMin = 1,
                        LevelMax = expTable.Length,
                        Template = BotPayloadProjection.BuildTemplateView(template),
                        LootPools = lootPools,
                    },
                ]
            ),
            Bots = [.. Enumerable.Range(0, waveSize).Select(_ => BotPayloadProjection.BuildBotSlice(new MongoId(), details, null))],
        };
    }

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

    private byte[] SerializeRequest()
    {
        var details = new BotGenerationDetails
        {
            Role = "pmcUSEC",
            RoleLowercase = "pmcusec",
            Side = "Usec",
            BotDifficulty = "normal",
            GameVersion = "standard",
            BotLevel = 1,
            IsPmc = true,
        };

        var template = _cloner.Clone(_botTable.Types["usec"])!;
        _botEquipmentFilterService.FilterBotEquipment(_sessionId, template, details);

        var request = BotPayloadProjection.BuildRequest(
            new MongoId(),
            _sessionId,
            template,
            details,
            null,
            _profileHelper,
            _profileActivityService,
            _weatherHelper,
            _botEquipmentModPoolService,
            _botLootGenerator.BotLootCacheService,
            _itemHelper,
            _botConfig,
            _pmcConfig
        );
        request.ViewsOverride = BuildViewsOverride([request.LootPools]);

        return JsonSerializer.SerializeToUtf8Bytes(request, JsonUtil.JsonSerializerOptionsNoIndent!);
    }
}
