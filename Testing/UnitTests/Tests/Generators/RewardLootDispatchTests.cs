using System.Reflection;
using HarmonyLib;
using NUnit.Framework;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Models.Eft.Common.Tables;
using SPTarkov.Server.Core.Models.Spt.Config;

namespace UnitTests.Tests.Generators;

/// <summary>
/// Pins the dual-path dispatch and the mod hook contract for reward loot: native by default, the
/// retained 4.1.2 C# implementation when LocationConfig.ForceLegacyLootGeneration is set or when a
/// Harmony patch on one of the restored protected members is live. The config flag only works if
/// the container picked the additive constructor that takes LocationConfig, so
/// ForceLegacyFlagRoutesToTheLegacyPath doubles as the check on that. Mutates the shared config
/// singleton and patches process-wide, so both are restored and the fixture never runs in parallel.
/// </summary>
[TestFixture]
[NonParallelizable]
public class RewardLootDispatchTests
{
    private static bool _patchFired;

    private LootGenerator _lootGenerator = default!;
    private LocationConfig _locationConfig = default!;
    private InventoryConfig _inventoryConfig = default!;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        var di = DI.GetInstance();

        _lootGenerator = di.GetService<LootGenerator>();
        _locationConfig = di.GetService<LocationConfig>();
        _inventoryConfig = di.GetService<InventoryConfig>();
    }

    [Test]
    public void NativePathIsTakenByDefault()
    {
        var loot = GenerateRewardContainerLoot();

        Assert.That(_lootGenerator.LastPathTaken, Is.EqualTo(LootGenerationPath.Native));
        Assert.That(loot, Is.Not.Empty);
    }

    [Test]
    public void ForceLegacyFlagRoutesToTheLegacyPath()
    {
        _locationConfig.ForceLegacyLootGeneration = true;
        try
        {
            var loot = GenerateRewardContainerLoot();

            Assert.That(_lootGenerator.LastPathTaken, Is.EqualTo(LootGenerationPath.Legacy));
            Assert.That(loot, Is.Not.Empty);
        }
        finally
        {
            _locationConfig.ForceLegacyLootGeneration = false;
        }
    }

    /// <summary>
    /// PickRewardItem runs once per reward, so a patch on it has to fire on the legacy path - and
    /// the patch being live is what routes the call there in the first place.
    /// </summary>
    [Test]
    public void HarmonyPatchOnAProtectedMemberFiresAndForcesTheLegacyPath()
    {
        var harmony = new Harmony("unit-tests.reward-loot-hook-liveness");
        var target = typeof(LootGenerator).GetMethod("PickRewardItem", BindingFlags.Instance | BindingFlags.NonPublic);
        Assert.That(target, Is.Not.Null, "restored protected member PickRewardItem not found");

        _patchFired = false;
        try
        {
            harmony.Patch(target, postfix: new HarmonyMethod(typeof(RewardLootDispatchTests), nameof(Postfix)));
            GenerateRewardContainerLoot();

            Assert.That(_lootGenerator.LastPathTaken, Is.EqualTo(LootGenerationPath.Legacy));
            Assert.That(_patchFired, Is.True, "postfix on PickRewardItem never ran");
        }
        finally
        {
            harmony.UnpatchSelf();
        }
    }

    /// <summary>
    /// The cheapest of the four entry points: one weighted draw per reward, no item pool build.
    /// </summary>
    private List<List<Item>> GenerateRewardContainerLoot()
    {
        var rewardDetails = _inventoryConfig.RandomLootContainers.Values.First(details => details.RewardTplPool is { Count: > 0 });

        return _lootGenerator.GetRandomLootContainerLoot(rewardDetails);
    }

    private static void Postfix()
    {
        _patchFired = true;
    }
}
