using System.Text.Json;
using System.Text.Json.Nodes;
using NUnit.Framework;
using SPTarkov.Server.Core.Generators.Bot;
using SPTarkov.Server.Core.Helpers.Bot;
using SPTarkov.Server.Core.Helpers.InRaid;
using SPTarkov.Server.Core.Helpers.Items;
using SPTarkov.Server.Core.Helpers.Profile;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Profile;
using SPTarkov.Server.Core.Models.Enums;
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
using BotType = SPTarkov.Server.Core.Models.Eft.Common.Tables.BotType;

namespace UnitTests.Tests.Generators;

/// <summary>
/// Pins the wire contract between the bot payload records and <c>spt_generate_bot_inventory</c>.
/// The request is built by <see cref="BotPayloadProjection"/> off the live test database, so a
/// renamed member, a dropped projection or a dictionary key that serialises as a number fails here
/// rather than as a silently emptier bot at runtime.
/// </summary>
[TestFixture]
public class SptNativeBotWireTests
{
    private const ulong TestSeed = 42;

    private GenerateBotInventoryRequest _request = default!;

    private BotType _template = default!;

    [OneTimeSetUp]
    public void Initialize()
    {
        var di = DI.GetInstance();

        // Publishes the static JsonSerializerOptions the wrapper serialises the payload with
        di.GetService<JsonUtil>();

        var sessionId = new MongoId();
        di.GetService<SaveServer>().CreateProfile(new Info { ProfileId = sessionId });

        _template = di.GetService<BotTable>().Types["assault"]!;

        _request = BotPayloadProjection.BuildRequest(
            new MongoId(),
            sessionId,
            _template,
            new BotGenerationDetails
            {
                Role = "assault",
                RoleLowercase = "assault",
                Side = "Savage",
                BotLevel = 1,
                BotDifficulty = "normal",
                GameVersion = "standard",
                Location = "bigmap",
                // Kept, so the container grids ride back out and their slot keys are exercised too
                ClearBotContainerCacheAfterGeneration = false,
            },
            TestSeed,
            di.GetService<ProfileHelper>(),
            di.GetService<ProfileActivityService>(),
            di.GetService<WeatherHelper>(),
            di.GetService<BotLootCacheService>(),
            di.GetService<BotConfig>(),
            di.GetService<PmcConfig>()
        );
        // The override send (epoch 0), which is the arm whose wire this fixture pins
        _request.ViewsOverride = BotPayloadProjection.BuildViewsOverride(
            di.GetService<PresetHelper>(),
            di.GetService<HandbookHelper>(),
            di.GetService<ItemHelper>(),
            di.GetService<GlobalTable>(),
            di.GetService<ItemFilterService>(),
            di.GetService<BotConfig>(),
            di.GetService<PmcConfig>(),
            di.GetService<RepairConfig>(),
            [_request.LootPools]
        );
    }

    [Test]
    public void BotInventoryRequestRoundTripsThroughTheNativeLibrary()
    {
        var result = SptNative.GenerateBotInventory(_request);

        Assert.That(result.Inventory.Items, Is.Not.Null.And.Not.Empty);
        Assert.That(result.RandomisationClamps, Is.Not.Null);
        // Rust mints every id itself; C# must be able to parse all of them back
        foreach (var item in result.Inventory.Items!)
        {
            Assert.That(new MongoId(item.Id.ToString()).ToString(), Is.EqualTo(item.Id.ToString()));
        }

        Assert.That(result.Inventory.Equipment, Is.EqualTo(result.Inventory.Items![0].Id));
        // The cache was asked to be kept, so the container state comes back keyed by slot name
        Assert.That(result.ContainerGrids, Is.Not.Empty);
    }

    /// <summary>
    /// <c>BotTypeInventory.Equipment</c> is keyed by the <c>EquipmentSlots</c> enum. If
    /// System.Text.Json wrote those keys as their numeric values the native side would find no pool
    /// for any slot and quietly hand back a bot in its underwear, so the key text is asserted
    /// directly.
    /// </summary>
    [Test]
    public void EquipmentSlotDictionaryKeysSerialiseAsMemberNames()
    {
        var json = JsonNode.Parse(JsonSerializer.Serialize(_request, JsonUtil.JsonSerializerOptionsNoIndent))!;

        var equipmentPools = json["template"]!["inventory"]!["equipment"]!.AsObject();
        Assert.That(equipmentPools.Count, Is.GreaterThan(0));
        Assert.That(equipmentPools.ContainsKey("Headwear"), Is.True, "equipment pools are not keyed by EquipmentSlots member name");
    }

    /// <summary>
    /// The projection additions the bot path needs and the loot ports never read. A dropped block
    /// is silent on the wire - a missing <c>Ammo</c> template quietly falls back to
    /// <c>DefAmmo</c> - so each is asserted against a template known to carry it.
    /// </summary>
    [Test]
    public void ItemViewCarriesTheBotProjections()
    {
        // AK-74N: a weapon with chambers, a magazine slot and a reload mode
        var weapon = _request.ViewsOverride!.Items[new MongoId("5644bd2b4bdc2d3b4c8b4572")];
        Assert.That(weapon.WeapClass, Is.Not.Null);
        Assert.That(weapon.ReloadMode, Is.EqualTo("ExternalMagazine"));
        Assert.That(weapon.Chambers, Is.Not.Null.And.Not.Empty);
        Assert.That(weapon.WeapFireType, Is.Not.Null.And.Not.Empty);
        Assert.That(weapon.MaxDurability, Is.Not.Null);

        // 6B3TM armour rig: a container, so it has grids
        var rig = _request.ViewsOverride!.Items[new MongoId("545cdae64bdc2d39198b4568")];
        Assert.That(rig.Grids, Is.Not.Null.And.Not.Empty);
        Assert.That(rig.Grids![0].CellsH, Is.Not.Null);
    }

    /// <summary>
    /// The blocks whose absence would change generation without failing the parse.
    /// </summary>
    [Test]
    public void RequestCarriesTheResolvedConfigAndPoolSlices()
    {
        // The profile the fixture created carries no level, so the raw member is null and the
        // native side applies the equipment path's `?? 1` and the weapon-mod path's `?? 0` itself
        Assert.That(_request.Shared.GeneratingPlayerLevel, Is.Null);
        Assert.That(_request.Bot.TestSeed, Is.EqualTo(TestSeed));
        Assert.That(_request.ViewsOverride!.Equipment, Does.ContainKey("assault"));
        // The live EquipmentMods bands ride the shared block on both arms, never the views. Only
        // the two roles with a `randomisation` list project a band; `assault` has none
        Assert.That(_request.Shared.LiveEquipmentMods["pmc"], Is.Not.Empty);
        Assert.That(_request.Shared.LiveEquipmentMods["pmc"][0].EquipmentMods, Is.Not.Empty);
        Assert.That(_request.ViewsOverride!.Bosses, Is.Not.Empty);
        Assert.That(_request.ViewsOverride!.BotRolesWithDogTags, Is.Not.Empty);
        Assert.That(_request.ViewsOverride.BodyToFixedHands, Is.Not.Empty);
        Assert.That(_request.ViewsOverride.ItemPresets, Is.Not.Empty);
        Assert.That(_request.ViewsOverride.DefaultPresetsByTpl, Is.Not.Empty);
        // The defaults ride as ids, so every one has to resolve against the only preset map sent
        Assert.That(_request.ViewsOverride.DefaultPresetsByTpl.Values, Is.SubsetOf(_request.ViewsOverride.ItemPresets.Keys));
        Assert.That(_request.ViewsOverride.ConfigBlacklist, Is.Not.Empty);
        Assert.That(_request.LootPools.BackpackLoot, Is.Not.Empty);
        // Every pool tpl has to be priceable, or the running rouble total silently reads 0
        Assert.That(_request.ViewsOverride.HandbookPrices.Keys, Is.SupersetOf(_request.LootPools.BackpackLoot.Keys));
    }

    /// <summary>
    /// The Rust variant wire (<c>TemplateVariantWire</c>) hardcodes these member names - appearance
    /// lowercase via the model's <c>JsonPropertyName</c>s, health/skills PascalCase because
    /// <c>JsonUtil</c> applies no naming policy. A failure here means the serde renames in
    /// <c>rust/spt-native/src/bot/models.rs</c> are wrong, not this test.
    ///
    /// The appearance members are asserted by JSON *kind*, not merely non-null:
    /// <c>Hands</c>/<c>Head</c>/<c>Voice</c> carry <c>ArrayToObjectFactoryConverter</c>, whose
    /// <c>Write</c> emits <c>[]</c> for a null dictionary, and <c>AppearanceWire</c> types those
    /// members with no serde default - so an array there fails the *whole-request* deserialize and
    /// kills a wave rather than one bot. Shipped data never hits it (no bot type file has a null
    /// appearance member), so this guards a regression in our own serialisation.
    /// </summary>
    [Test]
    public void TemplateVariantBlocksSerialiseWithTheNamesTheNativeSideExpects()
    {
        var json = JsonNode.Parse(JsonSerializer.Serialize(BuildTemplateVariantView(), JsonUtil.JsonSerializerOptionsNoIndent))!;

        foreach (var member in new[] { "body", "feet", "hands", "head", "voice" })
        {
            Assert.That(json["appearance"]![member], Is.InstanceOf<JsonObject>(), $"appearance.{member} is not a weighted object");
            Assert.That(json["appearance"]![member]!.AsObject(), Is.Not.Empty, $"appearance.{member} came out empty");
        }

        Assert.That(json["health"]!["Hydration"]!["min"], Is.Not.Null);
        Assert.That(json["health"]!["Energy"]!["min"], Is.Not.Null);
        Assert.That(json["health"]!["Temperature"]!["min"], Is.Not.Null);
        Assert.That(json["health"]!["BodyParts"]!.AsArray(), Is.Not.Empty);
        Assert.That(json["health"]!["BodyParts"]![0]!["LeftArm"]!["min"], Is.Not.Null);

        Assert.That(json["skills"]!["Common"], Is.InstanceOf<JsonObject>());
        // `assault` ships no Mastering, so it cannot be asserted present - but a rename of either
        // member would surface as a key the native BotDbSkillsWire does not name.
        Assert.That(json["skills"]!.AsObject().Select(member => member.Key), Is.SubsetOf(new[] { "Common", "Mastering" }));

        Assert.That(json["experienceReward"]!["normal"]!["min"], Is.Not.Null);
    }

    /// <summary>
    /// <c>bot_generator.rs</c> hardcodes these numeric values (<c>MEMBER_CATEGORY_*</c>) - the enum
    /// is not shared across the boundary.
    /// </summary>
    [Test]
    public void MemberCategoryNumericValuesMatchTheNativeConstants()
    {
        Assert.That((int)MemberCategory.Developer, Is.EqualTo(1));
        Assert.That((int)MemberCategory.UniqueId, Is.EqualTo(2));
        Assert.That((int)MemberCategory.Unheard, Is.EqualTo(1024));
    }

    /// <summary>
    /// The fixture request is a single-bot one, which carries no variants, so the variant view the
    /// name pin needs is assembled off the same template exactly as
    /// <c>BotWaveBatcher.BuildBatchRequest</c> does.
    /// </summary>
    private BotTemplateVariantView BuildTemplateVariantView()
    {
        return new BotTemplateVariantView
        {
            LevelMin = 1,
            LevelMax = 1,
            Template = BotPayloadProjection.BuildTemplateView(_template),
            LootPools = _request.LootPools,
            Appearance = _template.BotAppearance,
            Health = _template.BotHealth,
            Skills = _template.BotSkills,
            ExperienceReward = _template.BotExperience.Reward,
        };
    }
}
