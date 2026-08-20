using System.Text.Json.Nodes;
using NUnit.Framework;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Enums;
using SPTarkov.Server.Core.Models.Spt.Services;
using SPTarkov.Server.Core.Native;
using SPTarkov.Server.Core.Native.Loot;
using SPTarkov.Server.Core.Utils;
using SPTarkov.Server.Core.Utils.Json;

namespace UnitTests.Tests.Generators;

/// <summary>
/// Pins the wire contract between the reward loot payload records and the four native reward
/// exports. Every member of the new envelope family is optional or defaulted on the Rust side, so a
/// misspelled <c>JsonPropertyName</c> fails silently rather than loudly - the assertions here are
/// chosen so a member that never arrives changes the result.
/// </summary>
[TestFixture]
public class LootGeneratorNativeTests
{
    private static readonly MongoId _stackableTpl = new("111111111111111111111111");

    private JsonUtil _jsonUtil = default!;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        // Publishes the static JsonSerializerOptions the wrappers serialise payloads with, exactly
        // as the server's DI container does at startup.
        _jsonUtil = new JsonUtil([new SptJsonConverterRegistrator()]);
    }

    /// <summary>
    /// Two of a tpl whose stack caps at one, so the native side has to split them into two groups -
    /// which it can only do if <c>stackMaxSize</c> reached it. A dropped member would raise
    /// "StackMaxSize is null or not positive" instead.
    /// </summary>
    [Test]
    public void ForcedLootRoundTripsThroughTheNativeLibrary()
    {
        var request = new CreateForcedLootRequest
        {
            Epoch = 0,
            ViewsOverride = new RewardViewsOverride
            {
                ItemsView = BuildItemsView(),
                DefaultPresets = [],
                DefaultPresetsByTpl = [],
                ConfigBlacklist = [],
                RewardItemBlacklist = [],
                RewardBaseTypeBlacklist = [],
                BossItems = [],
            },
            Varying = new CreateForcedLootVarying
            {
                GlobalBlacklist = [],
                InactiveSeasonalItems = [],
                TestSeed = 42,
                ForcedLoot = new Dictionary<MongoId, MinMax<int>> { [_stackableTpl] = new MinMax<int>(2, 2) },
            },
        };

        var result = SptNative.CreateForcedLoot(request);

        Assert.That(result.Items, Has.Count.EqualTo(2));
        foreach (var group in result.Items)
        {
            Assert.That(group, Is.Not.Empty);
            var item = group[0];
            Assert.That(item.Template, Is.EqualTo(_stackableTpl));
            // Rust mints the ids itself; C# must be able to parse every one of them back.
            Assert.That(new MongoId(item.Id.ToString()).ToString(), Is.EqualTo(item.Id.ToString()));
            Assert.That(item.Upd!.StackObjectsCount, Is.EqualTo(1));
        }
    }

    /// <summary>
    /// <c>ItemView.Name</c> is only read by the sealed-crate filter, which throws on a null one. The
    /// two failure messages are different, so the one that comes back proves whether the member
    /// crossed the boundary.
    /// </summary>
    [Test]
    public void ItemViewNameReachesTheNativeSide()
    {
        var request = new CreateRandomLootRequest
        {
            Epoch = 0,
            ViewsOverride = new RewardViewsOverride
            {
                ItemsView = BuildItemsView(),
                DefaultPresets = [],
                DefaultPresetsByTpl = [],
                ConfigBlacklist = [],
                RewardItemBlacklist = [],
                RewardBaseTypeBlacklist = [],
                BossItems = [],
            },
            Varying = new CreateRandomLootVarying
            {
                GlobalBlacklist = [],
                InactiveSeasonalItems = [],
                TestSeed = 42,
                LootRequest = new LootRequest { WeaponCrateCount = new MinMax<int>(1, 1), ItemLimits = [] },
            },
        };

        var error = Assert.Throws<InvalidOperationException>(() => SptNative.CreateRandomLoot(request));

        Assert.That(error!.Message, Does.Contain("No sealed weapon containers found"));
        Assert.That(error.Message, Does.Not.Contain("has no name"));
    }

    /// <summary>
    /// The four members <c>ItemView</c> grew for reward generation are all-optional on the Rust
    /// side, so their wire names get one explicit check rather than a silent drop.
    /// </summary>
    [Test]
    public void ItemViewRewardMembersUseTheirBareWireNames()
    {
        var view = new ItemView
        {
            Name = "event_container_airdrop",
            Type = "Item",
            ArmorClass = 4,
            QuestItem = false,
        };

        var serialised = JsonNode.Parse(_jsonUtil.Serialize(view)!)!;

        Assert.Multiple(() =>
        {
            Assert.That(serialised["name"]!.GetValue<string>(), Is.EqualTo("event_container_airdrop"));
            Assert.That(serialised["type"]!.GetValue<string>(), Is.EqualTo("Item"));
            Assert.That(serialised["armorClass"]!.GetValue<int>(), Is.EqualTo(4));
            Assert.That(serialised["questItem"]!.GetValue<bool>(), Is.False);
        });
    }

    /// <summary>
    /// The base class node the tpl walk needs, plus one stackable item that is not a sealed crate.
    /// </summary>
    private static Dictionary<MongoId, ItemView> BuildItemsView()
    {
        return new Dictionary<MongoId, ItemView>
        {
            [BaseClasses.ITEM] = new ItemView { Name = "item" },
            [_stackableTpl] = new ItemView
            {
                Parent = BaseClasses.ITEM,
                Name = "not_a_sealed_crate",
                Type = "Item",
                QuestItem = false,
                Width = 1,
                Height = 1,
                StackMaxSize = 1,
            },
        };
    }
}
