using System.Text.Json.Nodes;
using NUnit.Framework;
using SPTarkov.Server.Core.Generators;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Helpers.Items;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Enums;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Utils;

namespace UnitTests.Tests.Generators;

/// <summary>
/// Golden parity gate on the scav case reward port: the same seed must make the legacy 4.1.2 C# path
/// and the spt-native path generate equivalent rewards for every recipe the shipped database ships.
///
/// State this fixture mutates, all of it restored in <see cref="Generate"/>'s <c>finally</c>:
/// <see cref="ScavCaseConfig.ForceLegacyScavCaseGeneration"/> on the shared config singleton - the
/// path selector - that config's <c>MoneyRewards.MoneyRewardChancePercent</c> and
/// <c>AmmoRewards.AmmoRewardChancePercent</c> for the two forced cases, and
/// <see cref="RandomUtil.RandomSource"/>, the seam the legacy path draws through.
/// The generator itself is built fresh per run, so its two instance caches rebuild every time and the
/// <c>NativeTestSeed</c> never outlives the call.
///
/// The one sanctioned parity gap: <c>MongoId</c> minting sits outside the seeded stream on both sides
/// - the reward's root id, the ids <c>ReplaceIDs</c> gives a preset's items, and the ones
/// <c>AddCartridgesToAmmoBox</c> mints. <see cref="LootIdNormalizer"/> maps every <c>_id</c> to a
/// positional placeholder and rewrites the <c>parentId</c> pointers at them, so a path that
/// re-parented or reordered a tree still fails.
/// </summary>
[TestFixture]
[NonParallelizable]
public class ScavCaseParityTests
{
    private static readonly ulong[] _seeds = [42, 1337];

    private ScavCaseConfig _scavCaseConfig = default!;
    private RandomUtil _randomUtil = default!;
    private JsonUtil _jsonUtil = default!;
    private ItemHelper _itemHelper = default!;
    private List<MongoId> _recipeIds = default!;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        var di = DI.GetInstance();

        _scavCaseConfig = di.GetService<ScavCaseConfig>();
        _randomUtil = di.GetService<RandomUtil>();
        _jsonUtil = di.GetService<JsonUtil>();
        _itemHelper = di.GetService<ItemHelper>();
        _recipeIds = di.GetService<HideoutTable>().Production.ScavRecipes!.Select(recipe => recipe.Id).ToList();
    }

    /// <summary>
    /// Every shipped recipe: their end-product counts differ per rarity, so between them they cover an
    /// empty rarity, a fixed count and a ranged count.
    /// </summary>
    [Test]
    public void EveryRecipeMatchesOnBothPaths([ValueSource(nameof(_seeds))] ulong seed)
    {
        Assert.That(_recipeIds, Is.Not.Empty, "the shipped database has no scav case recipes");

        foreach (var recipeId in _recipeIds)
        {
            var legacy = Generate(recipeId, seed, forceLegacy: true);
            var native = Generate(recipeId, seed, forceLegacy: false);

            LootJsonAssert.AssertEqual(legacy, native, $"recipe={recipeId}", seed);
        }
    }

    /// <summary>
    /// Money is a 5% roll per reward (<c>scavcase.json</c>'s <c>moneyRewardChancePercent</c>) and never
    /// came up across the matrix above, so nothing there proves the money pool's fixed
    /// rouble/euro/dollar/GP order or the per-currency stack counts. Forced here, the way
    /// <c>RepeatableQuestParityTests</c> forces its specific-location arm - the whole config crosses to
    /// the native side, so the one write reaches both paths.
    /// </summary>
    [Test]
    public void AForcedMoneyRewardMatchesOnBothPaths([ValueSource(nameof(_seeds))] ulong seed)
    {
        var legacy = Generate(_recipeIds[0], seed, forceLegacy: true, moneyRewardChancePercent: 100);
        var native = Generate(_recipeIds[0], seed, forceLegacy: false, moneyRewardChancePercent: 100);

        LootJsonAssert.AssertEqual(legacy, native, $"forced-money recipe={_recipeIds[0]}", seed);

        // A forced case that produced no money would compare two ordinary runs
        Assert.That(
            Money.GetMoneyTpls().Any(tpl => native.Contains(tpl.ToString(), StringComparison.Ordinal)),
            $"seed={seed} produced no money reward at a 100% money chance, so the money arm never ran"
        );
    }

    /// <summary>
    /// Ammo is its own 5% roll (<c>scavcase.json</c>'s <c>ammoRewardChancePercent</c>) and never came
    /// up across the matrix either, so nothing there proves the ammo pool - a filter of its own, not
    /// the reward pool plus a baseclass test - or its narrowing to the rarity's price band. Forced
    /// the same way the money case above is.
    /// </summary>
    [Test]
    public void AForcedAmmoRewardMatchesOnBothPaths([ValueSource(nameof(_seeds))] ulong seed)
    {
        var legacy = Generate(_recipeIds[0], seed, forceLegacy: true, ammoRewardChancePercent: 100);
        var native = Generate(_recipeIds[0], seed, forceLegacy: false, ammoRewardChancePercent: 100);

        LootJsonAssert.AssertEqual(legacy, native, $"forced-ammo recipe={_recipeIds[0]}", seed);

        // A forced case that produced no ammo would compare two ordinary runs. Group roots only: an
        // ammo box's cartridges are ammo too and they arrive as children of an ordinary reward.
        Assert.That(
            RewardRootTpls(native).Any(tpl => _itemHelper.IsOfBaseclass(tpl, BaseClasses.AMMO)),
            $"seed={seed} produced no ammo reward at a 100% ammo chance, so the ammo arm never ran"
        );
    }

    /// <summary>
    /// Without this the parity cases could pass by both paths producing something seed-independent.
    /// </summary>
    [Test]
    public void ADifferentSeedProducesDifferentRewards()
    {
        foreach (var forceLegacy in new[] { true, false })
        {
            var atSeed = Generate(_recipeIds[0], _seeds[0], forceLegacy);
            var atOtherSeed = Generate(_recipeIds[0], _seeds[1], forceLegacy);

            Assert.That(atOtherSeed, Is.Not.EqualTo(atSeed), $"forceLegacy={forceLegacy} ignored the seed - the draws are not reaching it");
        }
    }

    /// <summary>
    /// A seed has to pin the whole run, not just its first draws: two runs of one seed on one path
    /// must be byte-equal, or the parity cases above are comparing noise.
    /// </summary>
    [Test]
    public void TheSameSeedTwiceProducesIdenticalRewards([ValueSource(nameof(_seeds))] ulong seed)
    {
        foreach (var forceLegacy in new[] { true, false })
        {
            var first = Generate(_recipeIds[0], seed, forceLegacy);
            var second = Generate(_recipeIds[0], seed, forceLegacy);

            Assert.That(second, Is.EqualTo(first), $"forceLegacy={forceLegacy} is not deterministic under a fixed seed");
        }
    }

    /// <summary>
    /// One generation on one path, off a generator built for this run alone - the legacy path caches
    /// its two item pools on the instance, and a run has to rebuild them exactly as the first one did.
    /// </summary>
    private string Generate(
        MongoId recipeId,
        ulong seed,
        bool forceLegacy,
        int? moneyRewardChancePercent = null,
        int? ammoRewardChancePercent = null
    )
    {
        var generator = BuildGenerator();
        var expected = forceLegacy ? LootGenerationPath.Legacy : LootGenerationPath.Native;
        var originalForce = _scavCaseConfig.ForceLegacyScavCaseGeneration;
        var originalMoneyChance = _scavCaseConfig.MoneyRewards.MoneyRewardChancePercent;
        var originalAmmoChance = _scavCaseConfig.AmmoRewards.AmmoRewardChancePercent;
        var originalSource = _randomUtil.RandomSource;

        try
        {
            _scavCaseConfig.ForceLegacyScavCaseGeneration = forceLegacy;
            _scavCaseConfig.MoneyRewards.MoneyRewardChancePercent = moneyRewardChancePercent ?? originalMoneyChance;
            _scavCaseConfig.AmmoRewards.AmmoRewardChancePercent = ammoRewardChancePercent ?? originalAmmoChance;

            if (forceLegacy)
            {
                _randomUtil.RandomSource = new SeededRandomSource(seed);
            }
            else
            {
                generator.NativeTestSeed = seed;
            }

            var rewards = generator.Generate(recipeId).ToList();

            // Fail fast on silent fallback before comparing anything
            Assert.That(generator.LastPathTaken, Is.EqualTo(expected), $"generation did not take the {expected} path");

            // Two empty lists compare equal, which would make every parity case pass vacuously
            Assert.That(rewards, Is.Not.Empty, $"{expected} path generated no rewards for recipe={recipeId}");

            return LootIdNormalizer.Normalize(_jsonUtil.Serialize(rewards)!);
        }
        finally
        {
            _scavCaseConfig.ForceLegacyScavCaseGeneration = originalForce;
            _scavCaseConfig.MoneyRewards.MoneyRewardChancePercent = originalMoneyChance;
            _scavCaseConfig.AmmoRewards.AmmoRewardChancePercent = originalAmmoChance;
            _randomUtil.RandomSource = originalSource;
        }
    }

    /// <summary>
    /// The <c>_tpl</c> of each reward group's first item - the reward the pick itself produced,
    /// before any branch gave it children.
    /// </summary>
    private static IEnumerable<MongoId> RewardRootTpls(string rewards)
    {
        return JsonNode.Parse(rewards)!.AsArray().Select(group => new MongoId(group![0]!["_tpl"]!.GetValue<string>()));
    }

    /// <summary>
    /// A generator built off the container's own services on the widest constructor - the one the
    /// container itself picks, so the native seam is wired.
    /// </summary>
    private static ScavCaseRewardGenerator BuildGenerator()
    {
        var constructor = typeof(ScavCaseRewardGenerator).GetConstructors().MaxBy(candidate => candidate.GetParameters().Length)!;
        var arguments = constructor.GetParameters().Select(parameter => DI.GetInstance().GetService(parameter.ParameterType)).ToArray();

        return (ScavCaseRewardGenerator)constructor.Invoke(arguments);
    }
}
