using System.Reflection;
using System.Text.Json.Nodes;
using NUnit.Framework;
using SPTarkov.Server.Core.Generators.Bot;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Common;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Services.Bot;
using SPTarkov.Server.Core.Utils;

namespace UnitTests.Tests.Generators;

/// <summary>
/// Parity gate on the player scav port: the same seed must make the retained 4.1.2 C# path and the
/// spt-native path build the same scav profile, field for field, once the two sanctioned gaps are
/// masked - the fresh MongoIds neither path can reproduce (positional placeholders, as everywhere
/// else) and the wall-clock SavageLockTime.
///
/// The additional-loot draws are the one deliberate cross-arm stream divergence - legacy rolls them
/// C#-side after the bot is built, native rolls them inside the export - so the field-for-field case
/// suppresses them and two per-arm cases cover them separately at a certainty.
///
/// Mutates the shared PlayerScavConfig singleton, the RandomUtil seam and the ProbabilityRandomSource
/// static, so it restores all of them and never runs in parallel with other fixtures.
/// </summary>
[TestFixture]
[NonParallelizable]
public class PlayerScavParityTests
{
    private const ulong Seed = 1337;

    /// <summary>
    /// The karma level the fixture profile selects - Fence standing 0. Its shipped
    /// <c>lootItemsToAddChancePercent</c> is empty, which the additional-loot cases fill.
    /// </summary>
    private const string KarmaLevelKey = "0";

    /// <summary>
    /// A one-slot item the shipped config already hands out at higher karma levels, so it is known
    /// to fit the containers the additional-loot pass writes into.
    /// </summary>
    private static readonly MongoId _knownLootTpl = new("5c94bbff86f7747ee735c08f");

    private PlayerScavGenerator _playerScavGenerator = default!;
    private BotInventoryGenerator _botInventoryGenerator = default!;
    private PlayerScavConfig _playerScavConfig = default!;
    private BotNameService _botNameService = default!;
    private RandomUtil _randomUtil = default!;
    private JsonUtil _jsonUtil = default!;

    private MongoId _sessionId;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        var di = DI.GetInstance();

        _playerScavGenerator = di.GetService<PlayerScavGenerator>();
        _botInventoryGenerator = NestedBotInventoryGenerator(_playerScavGenerator);
        _playerScavConfig = di.GetService<PlayerScavConfig>();
        _botNameService = di.GetService<BotNameService>();
        _randomUtil = di.GetService<RandomUtil>();
        _jsonUtil = di.GetService<JsonUtil>();

        _sessionId = PlayerScavProfileFixture.Create();
    }

    [Test]
    public void TheArmsAgreeFieldForFieldWithAdditionalLootSuppressed()
    {
        var karmaSettings = _playerScavConfig.KarmaLevel[KarmaLevelKey];
        var originalChances = karmaSettings.LootItemsToAddChancePercent;

        karmaSettings.LootItemsToAddChancePercent = [];
        try
        {
            var legacy = Normalize(GenerateArm(forceLegacy: true, Seed));
            var native = Normalize(GenerateArm(forceLegacy: false, Seed));

            LootJsonAssert.AssertEqual(legacy, native, $"karma={KarmaLevelKey}", Seed);
        }
        finally
        {
            karmaSettings.LootItemsToAddChancePercent = originalChances;
        }
    }

    [Test]
    public void TheNativeArmAddsCertainAdditionalLoot()
    {
        AssertCertainAdditionalLootIsAdded(forceLegacy: false);
    }

    /// <summary>
    /// The same at a certainty on the other arm: without it, the suppression the field-for-field
    /// case relies on could be hiding a legacy pass that stopped adding anything at all.
    /// </summary>
    [Test]
    public void TheLegacyArmAddsCertainAdditionalLoot()
    {
        AssertCertainAdditionalLootIsAdded(forceLegacy: true);
    }

    /// <summary>
    /// The unseeded smoke: nothing pins the output, so this earns its keep by failing on a throw or
    /// a silent fallback on the path every real request takes.
    /// </summary>
    [Test]
    public void TheNativePathGeneratesUnseeded()
    {
        PlayerScavProfileFixture.Reseed(_sessionId);

        _playerScavGenerator.Generate(_sessionId);

        Assert.That(_playerScavGenerator.LastPathTaken, Is.EqualTo(LootGenerationPath.Native));
    }

    private void AssertCertainAdditionalLootIsAdded(bool forceLegacy)
    {
        var karmaSettings = _playerScavConfig.KarmaLevel[KarmaLevelKey];
        var originalChances = karmaSettings.LootItemsToAddChancePercent;

        karmaSettings.LootItemsToAddChancePercent = new Dictionary<MongoId, double> { { _knownLootTpl, 100.0 } };
        try
        {
            var scav = GenerateArm(forceLegacy, Seed);

            Assert.That(
                scav.Inventory!.Items!.Any(item => item.Template == _knownLootTpl),
                Is.True,
                $"the {(forceLegacy ? "legacy" : "native")} arm did not add the certain additional loot item"
            );
        }
        finally
        {
            karmaSettings.LootItemsToAddChancePercent = originalChances;
        }
    }

    /// <summary>
    /// The <see cref="BotInventoryGenerator"/> the legacy arm actually runs its inventory through.
    /// <c>[Injectable]</c> registers transient, so every <c>GetService</c> hands back a fresh
    /// instance: seeding a container-resolved one leaves the instance
    /// <see cref="PlayerScavGenerator"/> -> <see cref="BotGenerator"/> holds unseeded, its native
    /// inventory call falls back to entropy, and the legacy arm is irreproducible. It also makes the
    /// <c>LastPathTaken</c> guard below vacuous, <c>Native</c> being the default enum value.
    /// </summary>
    private static BotInventoryGenerator NestedBotInventoryGenerator(PlayerScavGenerator generator)
    {
        return (BotInventoryGenerator)CapturedField(CapturedField(generator, typeof(BotGenerator)), typeof(BotInventoryGenerator));
    }

    /// <summary>
    /// One primary-constructor capture field, found by its type. <c>Single</c> rather than
    /// <c>First</c> on purpose: if a second field of that type ever appears, the walk has to fail
    /// loudly instead of silently seeding whichever one it happened to pick.
    /// </summary>
    private static object CapturedField(object instance, Type fieldType)
    {
        return instance
                .GetType()
                .GetFields(BindingFlags.Instance | BindingFlags.NonPublic)
                .Single(field => field.FieldType == fieldType)
                .GetValue(instance)
            ?? throw new InvalidOperationException($"{instance.GetType().Name} holds no {fieldType.Name}");
    }

    private PmcData GenerateArm(bool forceLegacy, ulong seed)
    {
        // Generation writes the scav back into the profile, so both arms have to start from an
        // identical one
        PlayerScavProfileFixture.Reseed(_sessionId);

        // "assault" is in botRolesThatMustHaveUniqueName, so the nickname draw is retried until it
        // finds a name the process-wide cache has not handed out yet - and the first arm puts its
        // name in that cache. Left alone, the second arm rejects the name the first one took and
        // redraws, which shifts every C#-side draw after it by two.
        _botNameService.ClearNameCache();

        // Restore-what-you-captured (BotParityTests precedent): these are process-wide statics
        // shared across NUnit fixtures - do not assume the previous value was Crypto.
        var previousRandomSource = _randomUtil.RandomSource;
        var previousProbabilitySource = ProbabilityRandomSource.Current;
        var seeded = new SeededRandomSource(seed);
        _randomUtil.RandomSource = seeded;
        ProbabilityRandomSource.Current = seeded;
        _playerScavGenerator.NativeTestSeed = seed;
        _botInventoryGenerator.NativeTestSeed = seed; // the legacy arm's inventory rides the bot export
        _playerScavConfig.ForceLegacyPlayerScavGeneration = forceLegacy;
        try
        {
            var result = _playerScavGenerator.Generate(_sessionId);
            Assert.That(
                _playerScavGenerator.LastPathTaken,
                Is.EqualTo(forceLegacy ? LootGenerationPath.Legacy : LootGenerationPath.Native)
            );
            // Fail fast on silent fallback before comparing anything (BotParityTests precedent):
            // the legacy arm's inventory must still ride the native bot export, or the parity
            // diff reads like a karma-seam leak.
            Assert.That(_botInventoryGenerator.LastPathTaken, Is.EqualTo(LootGenerationPath.Native));
            return result;
        }
        finally
        {
            _playerScavConfig.ForceLegacyPlayerScavGeneration = false;
            _playerScavGenerator.NativeTestSeed = null;
            _botInventoryGenerator.NativeTestSeed = null;
            _randomUtil.RandomSource = previousRandomSource;
            ProbabilityRandomSource.Current = previousProbabilitySource;
        }
    }

    /// <summary>
    /// The two sanctioned masks: SavageLockTime is derived from the wall clock at the moment the
    /// arm ran, and every fresh MongoId becomes a positional placeholder.
    /// </summary>
    private string Normalize(PmcData scav)
    {
        return LootIdNormalizer.Normalize(RemoveWallClock(_jsonUtil.Serialize(scav)!));
    }

    private static string RemoveWallClock(string json)
    {
        var root = JsonNode.Parse(json) ?? throw new InvalidOperationException("player scav output parsed to null");

        Remove(root);

        return root.ToJsonString();
    }

    private static void Remove(JsonNode node)
    {
        switch (node)
        {
            case JsonObject obj:
                // Materialize the keys first: mutating obj while enumerating throws.
                foreach (var key in obj.Select(pair => pair.Key).ToList())
                {
                    if (string.Equals(key, "SavageLockTime", StringComparison.OrdinalIgnoreCase))
                    {
                        obj.Remove(key);
                        continue;
                    }

                    if (obj[key] is { } child)
                    {
                        Remove(child);
                    }
                }
                break;
            case JsonArray array:
                foreach (var child in array)
                {
                    if (child is not null)
                    {
                        Remove(child);
                    }
                }
                break;
        }
    }
}
