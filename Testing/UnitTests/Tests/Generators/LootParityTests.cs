using NUnit.Framework;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Utils;

namespace UnitTests.Tests.Generators;

/// <summary>
/// Golden parity gate on the Rust port: the same seed must make the legacy 4.1.2 C# path and the
/// spt-native path generate equivalent loot (deep-equal after LootIdNormalizer). Mutates the
/// shared config singleton, the RandomUtil seam and the ProbabilityRandomSource static, so it
/// restores all of them and never runs in parallel with other fixtures.
/// </summary>
[TestFixture]
[NonParallelizable]
public class LootParityTests
{
    // Every loot-bearing map in the database - pinned by EveryLootBearingLocationIsCovered.
    // Snake_case ids: GenerateLocationLoot lowercases the id for config lookups keyed like
    // "factory4_day", so PascalCase property names would break those lookups.
    private static readonly string[] _locationIds =
    [
        "bigmap",
        "factory4_day",
        "factory4_night",
        "interchange",
        "laboratory",
        "labyrinth",
        "lighthouse",
        "rezervbase",
        "sandbox",
        "sandbox_high",
        "shoreline",
        "tarkovstreets",
        "woods",
    ];
    private static readonly ulong[] _seeds = [42, 1337];

    private LocationLootGenerator _locationLootGenerator = default!;
    private LocationConfig _locationConfig = default!;
    private RandomUtil _randomUtil = default!;
    private JsonUtil _jsonUtil = default!;
    private LocationTable _locationTable = default!;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        var di = DI.GetInstance();

        _locationLootGenerator = di.GetService<LocationLootGenerator>();
        _locationConfig = di.GetService<LocationConfig>();
        _randomUtil = di.GetService<RandomUtil>();
        _jsonUtil = di.GetService<JsonUtil>();
        _locationTable = di.GetService<LocationTable>();
    }

    /// <summary>
    /// Pins _locationIds to the database so a new loot-bearing map cannot silently skip the gate.
    /// Compares by object identity - GetLocation and GetDictionary hand out the same instances - so
    /// id-spelling problems surface as a set mismatch too.
    /// </summary>
    [Test]
    public void EveryLootBearingLocationIsCovered()
    {
        var covered = _locationIds.Select(id => _locationTable.GetLocation(id)).ToHashSet();
        var lootBearing = _locationTable
            .GetDictionary()
            .Values.Where(location => location?.LooseLoot is not null && location.StaticContainers is not null)
            .ToHashSet();

        Assert.That(covered, Is.EquivalentTo(lootBearing));
    }

    [Test]
    public void TheSameSeedGeneratesEquivalentLootOnBothPaths(
        [ValueSource(nameof(_locationIds))] string locationId,
        [ValueSource(nameof(_seeds))] ulong seed
    )
    {
        var native = Generate(locationId, seed, forceLegacy: false, LootGenerationPath.Native);
        var legacy = Generate(locationId, seed, forceLegacy: true, LootGenerationPath.Legacy);

        LootJsonAssert.AssertEqual(legacy, native, $"map={locationId}", seed);
    }

    private string Generate(string locationId, ulong seed, bool forceLegacy, LootGenerationPath expected)
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
                _locationLootGenerator.NativeTestSeed = seed;
            }

            var spawnpoints = _locationLootGenerator.GenerateLocationLoot(locationId);

            // Fail fast on silent fallback before comparing anything.
            Assert.That(_locationLootGenerator.LastPathTaken, Is.EqualTo(expected), $"generation did not take the {expected} path");

            // Two empty lists compare equal, which would make every parity case pass vacuously.
            Assert.That(spawnpoints, Is.Not.Empty, $"{expected} path generated no loot for {locationId}");

            return LootIdNormalizer.Normalize(_jsonUtil.Serialize(spawnpoints)!);
        }
        finally
        {
            _locationConfig.ForceLegacyLootGeneration = originalForce;
            _randomUtil.RandomSource = originalSource;
            ProbabilityRandomSource.Current = originalProbabilitySource;
            _locationLootGenerator.NativeTestSeed = null;
        }
    }
}
