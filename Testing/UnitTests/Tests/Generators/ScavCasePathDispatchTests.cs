using System.Reflection;
using HarmonyLib;
using NUnit.Framework;
using SPTarkov.Common.Models.Logging;
using SPTarkov.Server.Core.Generators;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Helpers.Items;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Common.Tables;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Native.ScavCase;
using SPTarkov.Server.Core.Services.Items;
using SPTarkov.Server.Core.Services.Locales;
using SPTarkov.Server.Core.Services.Ragfair;
using SPTarkov.Server.Core.Services.Server;
using SPTarkov.Server.Core.Utils;
using SPTarkov.Server.Core.Utils.Cloners;

namespace UnitTests.Tests.Generators;

/// <summary>
/// Pins the dual-path dispatch for scav case rewards: native by default, the retained 4.1.2 C#
/// implementation when <c>ScavCaseConfig.ForceLegacyScavCaseGeneration</c> is set, when the frozen
/// 4.1.2 constructor built the instance (no native seam to dispatch to), when a mod substituted the
/// generator itself, or when a Harmony patch on a frozen member is live
/// (<see cref="ScavCaseHookLivenessTests"/> covers the whole hookable set).
///
/// Mutates the shared <see cref="ScavCaseConfig"/> singleton and patches process-wide, so both are
/// restored per case and the fixture never runs in parallel.
/// </summary>
[TestFixture]
[NonParallelizable]
public class ScavCasePathDispatchTests
{
    private const ulong NativeSeed = 424242;

    private ScavCaseRewardGenerator _scavCaseRewardGenerator = default!;
    private ScavCaseConfig _scavCaseConfig = default!;
    private MongoId _recipeId;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        var di = DI.GetInstance();

        _scavCaseRewardGenerator = di.GetService<ScavCaseRewardGenerator>();
        _scavCaseConfig = di.GetService<ScavCaseConfig>();
        _recipeId = di.GetService<HideoutTable>().Production.ScavRecipes!.First().Id;

        _scavCaseRewardGenerator.NativeTestSeed = NativeSeed;
    }

    [OneTimeTearDown]
    public void OneTimeTearDown()
    {
        _scavCaseRewardGenerator.NativeTestSeed = null;
    }

    [TearDown]
    public void TearDown()
    {
        _scavCaseConfig.ForceLegacyScavCaseGeneration = false;
    }

    /// <summary>
    /// The negative control: a stock container, no force flag and no patches take the native path.
    /// </summary>
    [Test]
    public void NativePathIsTakenByDefault()
    {
        var rewards = Generate(_scavCaseRewardGenerator);

        Assert.That(_scavCaseRewardGenerator.LastPathTaken, Is.EqualTo(LootGenerationPath.Native));
        Assert.That(rewards, Is.Not.Empty, "the native path produced no rewards");
    }

    [Test]
    public void ForceLegacyFlagRoutesToTheLegacyPath()
    {
        _scavCaseConfig.ForceLegacyScavCaseGeneration = true;

        var rewards = Generate(_scavCaseRewardGenerator);

        Assert.That(_scavCaseRewardGenerator.LastPathTaken, Is.EqualTo(LootGenerationPath.Legacy));
        Assert.That(rewards, Is.Not.Empty, "the legacy path produced no rewards");
    }

    /// <summary>
    /// A mod compiled against 4.1.2 can construct the generator itself, and the frozen constructor
    /// has no native seam wired - such an instance has to run the C# body it was built for.
    /// </summary>
    [Test]
    public void TheFrozenConstructorRoutesToTheLegacyPath()
    {
        var generator = Build(narrowest: true);

        Generate(generator);

        Assert.That(generator.LastPathTaken, Is.EqualTo(LootGenerationPath.Legacy));
    }

    /// <summary>
    /// The negative control for the two cases above and below: hand-building the generator off the
    /// container's own services is not by itself a reason to fall back.
    /// </summary>
    [Test]
    public void AHandBuiltGeneratorWithStockServicesTakesTheNativePath()
    {
        var generator = Build();

        Generate(generator);

        Assert.That(generator.LastPathTaken, Is.EqualTo(LootGenerationPath.Native));
    }

    /// <summary>
    /// A mod registering its own generator with a higher TypePriority hands the container a
    /// subclass, whose overrides only the C# path can run. Built on the widest constructor, so the
    /// native seam is wired and the substituted type is the only reason left to fall back.
    /// </summary>
    [Test]
    public void AReplacedGeneratorRoutesToTheLegacyPath()
    {
        var generator = (ScavCaseRewardGenerator)Construct(typeof(TestScavCaseRewardGeneratorSubclass));

        Generate(generator);

        Assert.That(generator.LastPathTaken, Is.EqualTo(LootGenerationPath.Legacy));
    }

    /// <summary>
    /// GetRandomMoney is one of the frozen 4.1.2 members, so a live patch on it has to route
    /// generation to the only body the patch can hook.
    /// </summary>
    [Test]
    public void HarmonyPatchOnAFrozenMemberForcesTheLegacyPath()
    {
        var harmony = new Harmony("unit-tests.scav-case-path-dispatch.GetRandomMoney");
        var target = AccessTools.Method(typeof(ScavCaseRewardGenerator), "GetRandomMoney");
        Assert.That(target, Is.Not.Null, "frozen member GetRandomMoney not found");

        try
        {
            harmony.Patch(target, postfix: new HarmonyMethod(typeof(ScavCasePathDispatchTests), nameof(Postfix)));

            var rewards = Generate(_scavCaseRewardGenerator);

            Assert.That(_scavCaseRewardGenerator.LastPathTaken, Is.EqualTo(LootGenerationPath.Legacy));
            Assert.That(rewards, Is.Not.Empty, "the legacy path produced no rewards");
        }
        finally
        {
            harmony.UnpatchSelf();
        }
    }

    private List<List<Item>> Generate(ScavCaseRewardGenerator generator)
    {
        return generator.Generate(_recipeId).ToList();
    }

    /// <summary>
    /// A generator built by hand off the container's own services, on either the frozen 4.1.2
    /// constructor or the additive one the container picks.
    /// </summary>
    private static ScavCaseRewardGenerator Build(bool narrowest = false)
    {
        return (ScavCaseRewardGenerator)Construct(typeof(ScavCaseRewardGenerator), narrowest);
    }

    private static object Construct(Type type, bool narrowest = false)
    {
        // The generator carries the frozen 4.1.2 constructor plus the additive overload the container
        // uses; take the widest unless the frozen one is what is under test
        var constructors = type.GetConstructors();
        var constructor = narrowest
            ? constructors.MinBy(candidate => candidate.GetParameters().Length)!
            : constructors.MaxBy(candidate => candidate.GetParameters().Length)!;

        var arguments = constructor.GetParameters().Select(parameter => DI.GetInstance().GetService(parameter.ParameterType)).ToArray();
        var instance = constructor.Invoke(arguments);

        if (instance is ScavCaseRewardGenerator generator)
        {
            generator.NativeTestSeed = NativeSeed;
        }

        return instance;
    }

    /// <summary>
    /// The dispatch check reads Harmony's patch info, so the patch only has to exist - whether it
    /// fires is <see cref="ScavCaseHookLivenessTests"/>' business.
    /// </summary>
    private static void Postfix() { }

    /// <summary>
    /// Stands in for a mod-registered generator: identical behaviour, different type. Chains the
    /// widest base constructor, so the native seam is wired and only the type check can fall back.
    /// </summary>
    private class TestScavCaseRewardGeneratorSubclass(
        ISptLogger<ScavCaseRewardGenerator> logger,
        HideoutTable hideoutTable,
        TemplateTable templateTable,
        RandomUtil randomUtil,
        ItemHelper itemHelper,
        PresetHelper presetHelper,
        RagfairPriceService ragfairPriceService,
        SeasonalEventService seasonalEventService,
        ItemFilterService itemFilterService,
        ServerLocalisationService localisationService,
        ScavCaseConfig scavCaseConfig,
        ICloner cloner,
        ScavCaseNativeRequestBuilder requestBuilder
    )
        : ScavCaseRewardGenerator(
            logger,
            hideoutTable,
            templateTable,
            randomUtil,
            itemHelper,
            presetHelper,
            ragfairPriceService,
            seasonalEventService,
            itemFilterService,
            localisationService,
            scavCaseConfig,
            cloner,
            requestBuilder
        ) { }
}
