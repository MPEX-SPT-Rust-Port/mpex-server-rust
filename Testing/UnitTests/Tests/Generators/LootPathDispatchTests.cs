using NUnit.Framework;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Models.Spt.Config;

namespace UnitTests.Tests.Generators;

/// <summary>
/// Pins the dual-path dispatch: native by default, the retained 4.1.2 C# implementation when
/// LocationConfig.ForceLegacyLootGeneration is set. Mutates the shared config singleton, so it
/// must restore it and never run in parallel with other fixtures.
/// </summary>
[TestFixture]
[NonParallelizable]
public class LootPathDispatchTests
{
    // Smallest map in the database - keeps the legacy full-generation run cheap
    private const string LocationId = "factory4_day";

    private LocationLootGenerator _locationLootGenerator = default!;
    private LocationConfig _locationConfig = default!;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        var di = DI.GetInstance();
        _locationLootGenerator = di.GetService<LocationLootGenerator>();
        _locationConfig = di.GetService<LocationConfig>();
    }

    [Test]
    public void NativePathIsTakenByDefault()
    {
        var spawnpoints = _locationLootGenerator.GenerateLocationLoot(LocationId);

        Assert.That(_locationLootGenerator.LastPathTaken, Is.EqualTo(LootGenerationPath.Native));
        Assert.That(spawnpoints, Is.Not.Empty);
    }

    [Test]
    public void ForceLegacyFlagRoutesToTheLegacyPath()
    {
        _locationConfig.ForceLegacyLootGeneration = true;
        try
        {
            var spawnpoints = _locationLootGenerator.GenerateLocationLoot(LocationId);

            Assert.That(_locationLootGenerator.LastPathTaken, Is.EqualTo(LootGenerationPath.Legacy));
            Assert.That(spawnpoints, Is.Not.Empty);
        }
        finally
        {
            _locationConfig.ForceLegacyLootGeneration = false;
        }
    }
}
