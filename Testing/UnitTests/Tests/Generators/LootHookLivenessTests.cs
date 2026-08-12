using System.Reflection;
using HarmonyLib;
using NUnit.Framework;
using SPTarkov.Server.Core.Generators.Loot;

namespace UnitTests.Tests.Generators;

/// <summary>
/// Pins the mod hook contract: a Harmony patch on one of the restored 4.1.2 protected members must
/// actually fire during generation - patch detection routes the call to the legacy path. Harmony
/// patches are process-wide, so the patch is removed in a finally and the fixture never runs in
/// parallel with others.
/// </summary>
[TestFixture]
[NonParallelizable]
public class LootHookLivenessTests
{
    private const string LocationId = "factory4_day";

    private static bool _patchFired;

    private LocationLootGenerator _locationLootGenerator = default!;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        _locationLootGenerator = DI.GetInstance().GetService<LocationLootGenerator>();
    }

    [Test]
    public void HarmonyPatchOnAProtectedMemberFiresAndForcesTheLegacyPath()
    {
        var harmony = new Harmony("unit-tests.loot-hook-liveness");
        var target = typeof(LocationLootGenerator).GetMethod("CreateStaticLootItem", BindingFlags.Instance | BindingFlags.NonPublic);
        Assert.That(target, Is.Not.Null, "restored protected member CreateStaticLootItem not found");

        _patchFired = false;
        try
        {
            harmony.Patch(target, postfix: new HarmonyMethod(typeof(LootHookLivenessTests), nameof(Postfix)));
            _locationLootGenerator.GenerateLocationLoot(LocationId);

            Assert.That(_locationLootGenerator.LastPathTaken, Is.EqualTo(LootGenerationPath.Legacy));
            Assert.That(_patchFired, Is.True, "postfix on CreateStaticLootItem never ran");
        }
        finally
        {
            harmony.UnpatchSelf();
        }
    }

    private static void Postfix()
    {
        _patchFired = true;
    }
}
