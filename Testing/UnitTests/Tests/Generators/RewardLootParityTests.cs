using System.Reflection;
using NUnit.Framework;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Common.Tables;
using SPTarkov.Server.Core.Models.Enums;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Services;
using SPTarkov.Server.Core.Services.InRaid;
using SPTarkov.Server.Core.Utils;

namespace UnitTests.Tests.Generators;

/// <summary>
/// Golden parity gate on the reward loot port: the same seed must make the legacy 4.1.2 C# path and
/// the spt-native path generate equivalent loot from every one of the four public entry points,
/// fed the live config the server itself feeds them. Mutates the shared config singleton, the
/// RandomUtil seam and the ProbabilityRandomSource static, so it restores all of them and never
/// runs in parallel with other fixtures.
/// </summary>
[TestFixture]
[NonParallelizable]
public class RewardLootParityTests
{
    private static readonly ulong[] _seeds = [42, 1337];

    private LootGenerator _lootGenerator = default!;
    private LocationConfig _locationConfig = default!;
    private RandomUtil _randomUtil = default!;
    private JsonUtil _jsonUtil = default!;
    private AirdropConfig _airdropConfig = default!;
    private InventoryConfig _inventoryConfig = default!;
    private TraderConfig _traderConfig = default!;
    private AirdropService _airdropService = default!;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        var di = DI.GetInstance();

        _lootGenerator = di.GetService<LootGenerator>();
        _locationConfig = di.GetService<LocationConfig>();
        _randomUtil = di.GetService<RandomUtil>();
        _jsonUtil = di.GetService<JsonUtil>();
        _airdropConfig = di.GetService<AirdropConfig>();
        _inventoryConfig = di.GetService<InventoryConfig>();
        _traderConfig = di.GetService<TraderConfig>();
        _airdropService = di.GetService<AirdropService>();
    }

    /// <summary>
    /// The airdrop request as AirdropService builds it - the blacklist folding it does on top of the
    /// raw config is part of the input, so the real method is invoked rather than reproduced.
    /// </summary>
    [Test]
    public void AirdropRandomLootMatchesOnBothPaths([ValueSource(nameof(_seeds))] ulong seed)
    {
        AssertParity(() => _lootGenerator.CreateRandomLoot(BuildAirdropRequest(SptAirdropTypeEnum.mixed)), seed, "airdrop=mixed");
    }

    [Test]
    public void CoopExtractGiftMatchesOnBothPaths([ValueSource(nameof(_seeds))] ulong seed)
    {
        AssertParity(() => _lootGenerator.CreateRandomLoot(_traderConfig.Fence.CoopExtractGift), seed, "coopExtractGift");
    }

    [Test]
    public void AirdropForcedLootMatchesOnBothPaths([ValueSource(nameof(_seeds))] ulong seed)
    {
        var forcedLootConfigs = _airdropConfig
            .Loot.Where(entry => entry.Value.ForcedLoot is { Count: > 0 })
            .ToDictionary(entry => entry.Key, entry => entry.Value.ForcedLoot!);

        Assert.That(forcedLootConfigs, Is.Not.Empty, "no airdrop config carries forced loot");

        foreach (var (airdropType, forcedLoot) in forcedLootConfigs)
        {
            AssertParity(() => _lootGenerator.CreateForcedLoot(forcedLoot), seed, $"forcedLoot={airdropType}");
        }
    }

    [Test]
    public void SealedWeaponCaseLootMatchesOnBothPaths([ValueSource(nameof(_seeds))] ulong seed)
    {
        AssertParity(() => _lootGenerator.GetSealedWeaponCaseLoot(_inventoryConfig.SealedAirdropContainer), seed, "sealedAirdropContainer");
    }

    /// <summary>
    /// Every reward container in the config, which covers both branches of PickRewardItem - the
    /// weighted RewardTplPool and the RewardTypePool item pool draw.
    /// </summary>
    [Test]
    public void RandomLootContainerLootMatchesOnBothPaths([ValueSource(nameof(_seeds))] ulong seed)
    {
        Assert.That(_inventoryConfig.RandomLootContainers, Is.Not.Empty, "no reward containers configured");

        foreach (var (containerTpl, rewardDetails) in _inventoryConfig.RandomLootContainers)
        {
            AssertParity(() => _lootGenerator.GetRandomLootContainerLoot(rewardDetails), seed, $"container={containerTpl}");
        }
    }

    private LootRequest BuildAirdropRequest(SptAirdropTypeEnum airdropType)
    {
        var method = typeof(AirdropService).GetMethod("GetAirdropLootConfigByType", BindingFlags.Instance | BindingFlags.NonPublic);
        Assert.That(method, Is.Not.Null, "AirdropService.GetAirdropLootConfigByType not found");

        return (LootRequest)method!.Invoke(_airdropService, [airdropType])!;
    }

    private void AssertParity(Func<IEnumerable<List<Item>>> generate, ulong seed, string label)
    {
        var native = Generate(generate, seed, forceLegacy: false, LootGenerationPath.Native, label);
        var legacy = Generate(generate, seed, forceLegacy: true, LootGenerationPath.Legacy, label);

        LootJsonAssert.AssertEqual(legacy, native, label, seed);
    }

    private string Generate(Func<IEnumerable<List<Item>>> generate, ulong seed, bool forceLegacy, LootGenerationPath expected, string label)
    {
        var originalForce = _locationConfig.ForceLegacyLootGeneration;
        var originalSource = _randomUtil.RandomSource;
        var originalProbabilitySource = ProbabilityRandomSource.Current;

        try
        {
            _locationConfig.ForceLegacyLootGeneration = forceLegacy;
            if (forceLegacy)
            {
                // One instance in both seams: one shared draw stream, mirroring the single
                // thread-local the Rust side installs for testSeed.
                var seeded = new SeededRandomSource(seed);
                _randomUtil.RandomSource = seeded;
                ProbabilityRandomSource.Current = seeded;
            }
            else
            {
                _lootGenerator.NativeTestSeed = seed;
            }

            var loot = generate().ToList();

            // Fail fast on silent fallback before comparing anything.
            Assert.That(_lootGenerator.LastPathTaken, Is.EqualTo(expected), $"generation did not take the {expected} path");

            // Two empty lists compare equal, which would make every parity case pass vacuously.
            Assert.That(loot, Is.Not.Empty, $"{expected} path generated no loot for {label}");

            return LootIdNormalizer.Normalize(_jsonUtil.Serialize(loot)!);
        }
        finally
        {
            _locationConfig.ForceLegacyLootGeneration = originalForce;
            _randomUtil.RandomSource = originalSource;
            ProbabilityRandomSource.Current = originalProbabilitySource;
            _lootGenerator.NativeTestSeed = null;
        }
    }
}
