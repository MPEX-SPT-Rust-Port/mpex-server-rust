using System.Text.Json;
using System.Text.Json.Nodes;
using NUnit.Framework;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Enums;
using SPTarkov.Server.Core.Models.Spt.Bots;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Native;
using SPTarkov.Server.Core.Native.PlayerScav;
using SPTarkov.Server.Core.Utils;

namespace UnitTests.Tests.Generators;

/// <summary>
/// Pins the wire contract between the player scav payload records and <c>spt_generate_player_scav</c>.
/// The request is built by the production <see cref="PlayerScavNativeRequestBuilder"/> off the live
/// test database, so a renamed member, a dropped karma map or a dictionary key that serialises as a
/// number fails here rather than as a silently karma-less scav at runtime.
///
/// The karma level is hand-built rather than taken from the shipped config: every map has to be
/// non-empty for its pin to mean anything, and the shipped level "0" carries an empty
/// <c>lootItemsToAddChancePercent</c>.
///
/// Pins the override arm's request shape only (<c>ViewsOverride</c> attached at <c>Epoch = 0</c>, as
/// <c>PlayerScavGenerator.GenerateScavNative</c>'s ineligible arm sends it).
/// </summary>
[TestFixture]
[NonParallelizable]
public class SptNativePlayerScavWireTests
{
    private const ulong TestSeed = 42;

    private GeneratePlayerScavRequest _request = default!;

    [OneTimeSetUp]
    public void Initialize()
    {
        var di = DI.GetInstance();

        // Publishes the static JsonSerializerOptions the wrapper serialises the payload with
        di.GetService<JsonUtil>();

        var builder = di.GetService<PlayerScavNativeRequestBuilder>();
        var sessionId = PlayerScavProfileFixture.Create();

        var karma = new KarmaLevel
        {
            BotTypeForLoot = "assault",
            ItemLimits = new(),
            Modifiers = new Modifiers
            {
                Equipment = new() { { "Headwear", -10.0 } },
                Mod = new() { { "mod_scope", -10.0 } },
            },
            EquipmentBlacklist = new() { { EquipmentSlots.Scabbard, [new MongoId("57e26fc7245977162a14b800")] } },
            LootItemsToAddChancePercent = new() { { new MongoId("5c94bbff86f7747ee735c08f"), 100.0 } },
        };

        _request = builder.Build(
            new MongoId(),
            sessionId,
            di.GetService<BotTable>().Types["assault"],
            new BotGenerationDetails
            {
                Role = "assault",
                RoleLowercase = "assault",
                Side = "Savage",
                BotLevel = 1,
                IsPlayerScav = true,
                BotDifficulty = "normal",
                Location = "bigmap",
                // GameVersion deliberately left unset: the pin below exercises BuildBotSlice's
                // `?? string.Empty` defaulting rather than a hand-set value that masks it
                //
                // Mirrors what production's native literal sends
                // (BotGenerator.GeneratePlayerScavNative); the model's initializer defaults this to
                // true, so the explicit false is the tripwire that keeps
                // TheRequestSkipsTheContainerGridEcho honest - the builder must be the thing that
                // flips the wire flag to true
                ClearBotContainerCacheAfterGeneration = false,
            },
            karma,
            TestSeed
        );
        // The override send (epoch 0), which is the arm whose wire this fixture pins
        _request.ViewsOverride = builder.BuildViewsOverride(_request.LootPools);
    }

    [Test]
    public void PlayerScavRequestRoundTripsThroughTheNativeLibrary()
    {
        var result = SptNative.GeneratePlayerScav(_request);

        Assert.That(result.Inventory, Is.Not.Null);
        Assert.That(result.Inventory.Items, Is.Not.Null.And.Not.Empty);
        Assert.That(result.RandomisationClamps, Is.Not.Null);
        // Rust mints every id itself; C# must be able to parse all of them back
        foreach (var item in result.Inventory.Items!)
        {
            Assert.That(new MongoId(item.Id.ToString()).ToString(), Is.EqualTo(item.Id.ToString()));
        }

        Assert.That(result.ContainerGrids, Is.Empty, "nothing reads the pscav response grids; the wire flag empties them");
    }

    [Test]
    public void TheRequestSkipsTheContainerGridEcho()
    {
        Assert.That(
            _request.Bot.Details.ClearBotContainerCacheAfterGeneration,
            Is.True,
            "the pscav arm never restores container grids, so the wire flag must suppress the echo"
        );
    }

    /// <summary>
    /// <c>KarmaLevel.EquipmentBlacklist</c> is keyed by the <c>EquipmentSlots</c> enum, which
    /// System.Text.Json writes numerically - so <c>BuildKarmaView</c> re-keys it by slot name. If
    /// that re-key were dropped the native side would match no slot and quietly blacklist nothing,
    /// so the key text is asserted directly.
    /// </summary>
    [Test]
    public void EquipmentSlotDictionaryKeysSerialiseAsMemberNames()
    {
        var json = JsonNode.Parse(JsonSerializer.Serialize(_request, JsonUtil.JsonSerializerOptionsNoIndent))!;

        Assert.That(
            json["karma"]!["equipmentBlacklist"]!.AsObject().ContainsKey("Scabbard"),
            Is.True,
            "EquipmentSlots keys must serialise as member names, not numbers (BuildKarmaView's re-key)"
        );
    }

    /// <summary>
    /// The karma maps and the details defaulting: a dropped map is silent on the wire - the scav
    /// simply generates without that half of its karma - so each is asserted by name.
    /// </summary>
    [Test]
    public void TheKarmaBlockCarriesItsFourCamelCaseMaps()
    {
        var json = JsonNode.Parse(JsonSerializer.Serialize(_request, JsonUtil.JsonSerializerOptionsNoIndent))!;

        var karma = json["karma"]!.AsObject();
        Assert.Multiple(() =>
        {
            Assert.That(karma.ContainsKey("equipmentModifiers"), Is.True);
            Assert.That(karma.ContainsKey("modModifiers"), Is.True);
            Assert.That(karma.ContainsKey("equipmentBlacklist"), Is.True);
            Assert.That(karma.ContainsKey("lootItemsToAddChancePercent"), Is.True);
            Assert.That(
                json["bot"]!["details"]!["gameVersion"]!.GetValue<string>(),
                Is.EqualTo(string.Empty),
                "an unset GameVersion must cross as empty string, never null"
            );
        });
    }

    /// <summary>
    /// <c>BuildViewsOverride</c> passes the request's own pools on to <c>BuildHandbookPrices</c>; the
    /// pin is on the wire shape that produces - the prices cover the request's own pools. No pscav
    /// runtime consequence is claimed: the native side reads handbook prices only when
    /// <c>total_value_limit_rub</c> is above 0, which is a PMC-only path.
    /// </summary>
    [Test]
    public void TheViewsOverridePricesEveryLootPool()
    {
        Assert.That(_request.LootPools.BackpackLoot, Is.Not.Empty);
        Assert.That(_request.ViewsOverride!.HandbookPrices.Keys, Is.SupersetOf(_request.LootPools.BackpackLoot.Keys));
    }
}
