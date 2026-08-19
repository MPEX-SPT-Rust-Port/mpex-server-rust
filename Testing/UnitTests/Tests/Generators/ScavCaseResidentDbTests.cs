using System.Reflection;
using NUnit.Framework;
using SPTarkov.Server.Core.Generators;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Mod;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Services.Server;

namespace UnitTests.Tests.Generators;

/// <summary>
/// Pins the resident-DB epoch protocol on the scav case native path: an eligible generator names an
/// epoch and never sends the views override, and the kill switch and untrusted mods fall back to the
/// override. The resident-vs-override reward parity and the stale-epoch republish gate live on the
/// Rust side (<c>rust/spt-native/tests/flip5_scavcase_resident.rs</c>). Epochs are process-global
/// (other fixtures publish too), so every assertion is relative. Mutates the shared config
/// singleton, so it restores it and never runs in parallel with other fixtures.
/// </summary>
[TestFixture]
[NonParallelizable]
public class ScavCaseResidentDbTests
{
    private const ulong Seed = 424242;

    private ScavCaseRewardGenerator _generator = default!;
    private ScavCaseConfig _scavCaseConfig = default!;
    private DatabaseMutationStamp _stamp = default!;
    private MongoId _recipeId;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        var di = DI.GetInstance();
        _generator = di.GetService<ScavCaseRewardGenerator>();
        _scavCaseConfig = di.GetService<ScavCaseConfig>();
        _stamp = di.GetService<DatabaseMutationStamp>();
        _recipeId = di.GetService<HideoutTable>().Production.ScavRecipes!.First().Id;
    }

    [OneTimeTearDown]
    public void OneTimeTearDown()
    {
        _scavCaseConfig.DisableNativeRequestCache = false;
        _scavCaseConfig.TrustNativeRequestCacheWithMods = false;
        // leave the shared container fresher than we found it for whatever fixture runs next
        _stamp.Bump();
    }

    /// <summary>
    /// One seeded generation on the native path. Fails fast on a silent legacy fallback or an empty
    /// result before asserting anything about the send.
    /// </summary>
    private void Generate(ScavCaseRewardGenerator generator)
    {
        generator.NativeTestSeed = Seed;
        try
        {
            var rewards = generator.Generate(_recipeId).ToList();

            Assert.That(generator.LastPathTaken, Is.EqualTo(LootGenerationPath.Native), "generation did not take the native path");
            Assert.That(rewards, Is.Not.Empty, "the native path generated no rewards");
        }
        finally
        {
            generator.NativeTestSeed = null;
        }
    }

    [Test]
    public void EligibleGenerationBuildsOffTheResidentDb()
    {
        Generate(_generator);

        Assert.That(_generator.LastSendIncludedViewsOverride, Is.False, "an eligible generator must not send the override");
    }

    [Test]
    public void KillSwitchForcesTheViewsOverride()
    {
        _scavCaseConfig.DisableNativeRequestCache = true;
        try
        {
            Generate(_generator);

            Assert.That(_generator.LastSendIncludedViewsOverride, Is.True, "the kill switch must force the override");
        }
        finally
        {
            _scavCaseConfig.DisableNativeRequestCache = false;
        }
    }

    [Test]
    public void ModsLoadedWithoutTheTrustFlagForceTheViewsOverride()
    {
        // The gate only reads Count, so a placeholder element stands in for a real mod
        var modded = BuildWithOverloadConstructor(DI.GetInstance(), new SptMod[] { null! });

        Generate(modded);

        Assert.That(modded.LastSendIncludedViewsOverride, Is.True, "a loaded mod without the trust flag disables residency");
    }

    [Test]
    public void TheTrustFlagKeepsTheResidentPathLiveWithModsLoaded()
    {
        var modded = BuildWithOverloadConstructor(DI.GetInstance(), new SptMod[] { null! });

        _scavCaseConfig.TrustNativeRequestCacheWithMods = true;
        try
        {
            Generate(modded);

            Assert.That(
                modded.LastSendIncludedViewsOverride,
                Is.False,
                "the trust flag should keep the resident path live despite the mod"
            );
        }
        finally
        {
            _scavCaseConfig.TrustNativeRequestCacheWithMods = false;
        }
    }

    private static ScavCaseRewardGenerator BuildWithOverloadConstructor(DI di, IReadOnlyList<SptMod> mods)
    {
        var constructor = typeof(ScavCaseRewardGenerator).GetConstructors().MaxBy(ctor => ctor.GetParameters().Length)!;

        var arguments = constructor
            .GetParameters()
            .Select(parameter =>
            {
                if (parameter.ParameterType == typeof(IReadOnlyList<SptMod>))
                {
                    return mods;
                }

                return di.GetService(parameter.ParameterType);
            })
            .ToArray();

        return (ScavCaseRewardGenerator)constructor.Invoke(arguments);
    }
}
