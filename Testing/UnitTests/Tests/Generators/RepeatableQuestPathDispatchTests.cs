using NUnit.Framework;
using SPTarkov.Common.Models.Logging;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Generators.RepeatableQuests;
using SPTarkov.Server.Core.Helpers.Items;
using SPTarkov.Server.Core.Helpers.Profile;
using SPTarkov.Server.Core.Helpers.Quest;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Common.Tables;
using SPTarkov.Server.Core.Models.Enums;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Mod;
using SPTarkov.Server.Core.Models.Spt.Repeatable;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Native.RepeatableQuests;
using SPTarkov.Server.Core.Services.Items;
using SPTarkov.Server.Core.Services.Locales;
using SPTarkov.Server.Core.Services.Server;
using SPTarkov.Server.Core.Utils;
using SPTarkov.Server.Core.Utils.Cloners;

namespace UnitTests.Tests.Generators;

/// <summary>
/// Pins the reasons repeatable-quest generation falls back to the retained 4.1.2 C# implementation
/// that are not Harmony patches (those are <see cref="RepeatableQuestHookLivenessTests"/>) - the
/// config flag and a mod-substituted collaborator - plus what the native slice cache does at the
/// <c>Send</c> level: the kill switch, the mod/trust truth table, and the stale-slice retry.
///
/// One generator is the vehicle for all of it. The four generators carry per-class copies of the
/// same seam, and <see cref="RepeatableQuestParityTests"/> already asserts the flag drives each of
/// them onto the path it names, for both paths - so running the substitution and cache cases four
/// times over would gate nothing the parity fixture does not.
///
/// Mutates the shared <see cref="QuestConfig"/> singleton and the shared builder's last-sent stamp,
/// so it restores the flags and resets the stamp per case, and never runs in parallel.
/// </summary>
[TestFixture]
[NonParallelizable]
public class RepeatableQuestPathDispatchTests
{
    private const ulong NativeSeed = 424242;

    private static readonly MongoId _sessionId = new("6193a720f8ee7e52e4290000");

    private ExplorationQuestGenerator _explorationQuestGenerator = default!;
    private RepeatableQuestNativeRequestBuilder _builder = default!;
    private QuestConfig _questConfig = default!;
    private DatabaseMutationStamp _databaseMutationStamp = default!;

    private RepeatableQuestConfig _repeatableConfig = default!;
    private MongoId _traderId;
    private int _pmcLevel;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        var di = DI.GetInstance();

        _explorationQuestGenerator = di.GetService<ExplorationQuestGenerator>();
        _builder = di.GetService<RepeatableQuestNativeRequestBuilder>();
        _questConfig = di.GetService<QuestConfig>();
        _databaseMutationStamp = di.GetService<DatabaseMutationStamp>();

        _repeatableConfig = _questConfig.RepeatableQuests.First(config => config.Side == PlayerGroup.Pmc);
        _traderId = _repeatableConfig.TraderWhitelist.First(whitelist => whitelist.QuestTypes.Contains("Exploration")).TraderId;

        // The midpoint of the second shipped exploration band, so the level tracks the data rather
        // than an edge this fixture invents
        var band = _repeatableConfig.QuestConfig.ExplorationConfig[1].LevelRange;
        _pmcLevel = (band.Min + band.Max) / 2;
    }

    [SetUp]
    public void SetUp()
    {
        // The builder is the shared DI singleton, so another fixture's send would otherwise leave the
        // native cache primed and the first send here slice-less
        _builder.LastSentSliceStamp = RepeatableQuestNativeRequestBuilder.NeverSent;
    }

    [TearDown]
    public void TearDown()
    {
        _questConfig.ForceLegacyRepeatableQuestGeneration = false;
        _questConfig.DisableNativeRequestCache = false;
        _questConfig.TrustNativeRequestCacheWithMods = false;
        _explorationQuestGenerator.NativeTestSeed = null;
    }

    /// <summary>
    /// The negative control: a stock container, no force flag and no patches take the native path.
    /// </summary>
    [Test]
    public void NativePathIsTakenByDefault()
    {
        var quest = Generate(_explorationQuestGenerator);

        Assert.That(_explorationQuestGenerator.LastPathTaken, Is.EqualTo(LootGenerationPath.Native));
        Assert.That(quest, Is.Not.Null, "the native path produced no quest");
    }

    [Test]
    public void ForceLegacyFlagRoutesToTheLegacyPath()
    {
        _questConfig.ForceLegacyRepeatableQuestGeneration = true;

        var quest = Generate(_explorationQuestGenerator);

        Assert.That(_explorationQuestGenerator.LastPathTaken, Is.EqualTo(LootGenerationPath.Legacy));
        Assert.That(quest, Is.Not.Null, "the legacy path produced no quest");
    }

    /// <summary>
    /// The negative control for the three substitution cases below: hand-building the generator off
    /// the container's own services is not by itself a reason to fall back.
    /// </summary>
    [Test]
    public void AHandBuiltGeneratorWithStockServicesTakesTheNativePath()
    {
        var generator = BuildGenerator();

        Generate(generator);

        Assert.That(generator.LastPathTaken, Is.EqualTo(LootGenerationPath.Native));
    }

    /// <summary>
    /// A mod registering its own generator with a higher TypePriority hands the container a subclass,
    /// whose overrides only the C# path can run. Built on the widest base constructor, so the native
    /// seam is wired and the substituted type is the only reason left to fall back.
    /// </summary>
    [Test]
    public void AReplacedGeneratorRoutesToTheLegacyPath()
    {
        var generator = (ExplorationQuestGenerator)Construct(typeof(TestExplorationQuestGeneratorSubclass));

        Generate(generator);

        Assert.That(generator.LastPathTaken, Is.EqualTo(LootGenerationPath.Legacy));
    }

    /// <inheritdoc cref="AReplacedGeneratorRoutesToTheLegacyPath"/>
    [Test]
    public void AReplacedRepeatableQuestHelperRoutesToTheLegacyPath()
    {
        AssertSubstitutionForcesLegacyPath(typeof(TestRepeatableQuestHelperSubclass));
    }

    /// <inheritdoc cref="AReplacedGeneratorRoutesToTheLegacyPath"/>
    [Test]
    public void AReplacedRewardGeneratorRoutesToTheLegacyPath()
    {
        AssertSubstitutionForcesLegacyPath(typeof(TestRepeatableQuestRewardGeneratorSubclass));
    }

    [Test]
    public void TheKillSwitchAlwaysSendsTheSlice()
    {
        _questConfig.DisableNativeRequestCache = true;

        Generate(_explorationQuestGenerator);
        Assert.That(_builder.LastSendIncludedSlice, Is.True);

        Generate(_explorationQuestGenerator);

        Assert.That(_builder.LastSendIncludedSlice, Is.True, "the kill switch let a send skip the slice");
    }

    /// <summary>
    /// The cache gate at the <c>Send</c> level, where <c>RepeatableQuestNativeRequestBuilderTests</c>
    /// covers <c>CacheEligible</c> alone: a loaded mod can write the tables the slice projects, so
    /// without the trust flag every send carries the slice.
    /// </summary>
    [Test]
    public void AModdedBuilderAlwaysSendsTheSlice()
    {
        var modded = BuildBuilderWithMods();
        var generator = BuildGenerator(modded);

        Generate(generator);
        Generate(generator);

        Assert.That(modded.LastSendIncludedSlice, Is.True, "a loaded mod without the trust flag disables the cache");
    }

    /// <inheritdoc cref="AModdedBuilderAlwaysSendsTheSlice"/>
    [Test]
    public void TheTrustFlagKeepsTheCacheLiveWithModsLoaded()
    {
        var modded = BuildBuilderWithMods();
        var generator = BuildGenerator(modded);
        _questConfig.TrustNativeRequestCacheWithMods = true;

        Generate(generator);
        Assert.That(modded.LastSendIncludedSlice, Is.True, "the first send has nothing cached to hit");

        Generate(generator);

        Assert.That(modded.LastSendIncludedSlice, Is.False, "the trust flag should keep the cache live despite the loaded mod");
    }

    /// <summary>
    /// The native cache holds one slice under one stamp. Bump the stamp without sending a slice for
    /// it and then claim it was already sent, and the next slice-less request names a stamp the
    /// native side does not hold - which has to self-heal through exactly one retry, not throw.
    /// </summary>
    [Test]
    public void ANativeSideDesyncSelfHealsThroughOneRetry()
    {
        // Park a slice under the current stamp, then move the stamp on and lie about having sent it
        Generate(_explorationQuestGenerator);
        _databaseMutationStamp.Bump();
        _builder.LastSentSliceStamp = _databaseMutationStamp.Current;

        var quest = Generate(_explorationQuestGenerator);

        Assert.That(_builder.LastSendIncludedSlice, Is.True, "the stale-slice miss should have retried with the slice");
        Assert.That(quest, Is.Not.Null, "the retry produced no quest");

        // The retry parked the slice under the stamp it named, so the next send is a clean hit - one
        // retry healed it rather than leaving every later send stale
        Generate(_explorationQuestGenerator);

        Assert.That(_builder.LastSendIncludedSlice, Is.False, "the retry did not leave the native cache holding the current stamp");
    }

    /// <summary>
    /// The pool-exhaustion outcome across the real FFI boundary: <c>quest: null</c> is a valid
    /// response, and it has to come back deserialized as a null quest alongside a pool that is still
    /// usable - not a throw, and not an empty pool that would strand the controller's next draw.
    /// </summary>
    [Test]
    public void AnExhaustedPoolRoundTripsANullQuestAndAUsablePool()
    {
        var pool = BuildPool();
        pool.Pool.Exploration.Locations!.Clear();

        var quest = _explorationQuestGenerator.Generate(_sessionId, _pmcLevel, _traderId, pool, _repeatableConfig);

        Assert.That(_explorationQuestGenerator.LastPathTaken, Is.EqualTo(LootGenerationPath.Native));
        Assert.That(quest, Is.Null, "the native path found a location in an emptied pool");
        Assert.Multiple(() =>
        {
            // The exhausted type drops out of the draw list and the rest of it survives - the pool
            // mutation the null response carries back is the whole point of returning one
            Assert.That(pool.Types, Does.Not.Contain("Exploration"), "the exhausted type stayed in the pool's draw list");
            Assert.That(
                pool.Types,
                Is.EqualTo(_repeatableConfig.Types.Where(type => type != "Exploration")),
                "the null-quest response lost the pool's other quest types"
            );
            Assert.That(pool.Pool.Exploration.Locations, Is.Not.Null.And.Empty, "the emptied half came back as anything but an empty map");
            Assert.That(pool.Pool.Pickup.Locations, Is.Not.Empty, "the null-quest response emptied the untouched half of the pool");
        });
    }

    private RepeatableQuest? Generate(ExplorationQuestGenerator generator)
    {
        return generator.Generate(_sessionId, _pmcLevel, _traderId, BuildPool(), _repeatableConfig);
    }

    private void AssertSubstitutionForcesLegacyPath(Type substituteType)
    {
        var generator = BuildGenerator(Construct(substituteType));

        Generate(generator);

        Assert.That(
            generator.LastPathTaken,
            Is.EqualTo(LootGenerationPath.Legacy),
            $"a substituted {substituteType.Name} did not force legacy"
        );
    }

    /// <summary>
    /// <c>RepeatableQuestController.GenerateQuestPool</c> (<c>:840-885</c>) for the two halves an
    /// exploration draw touches: it consumes the exploration locations, and the pickup half rides
    /// along untouched so a round-trip can be told from a wipe. The elimination targets stay empty -
    /// no exploration draw reads them.
    /// </summary>
    private QuestTypePool BuildPool()
    {
        var locations = _repeatableConfig.Locations.Where(location => location.Key != ELocationName.any).ToList();

        return new QuestTypePool
        {
            Types = [.. _repeatableConfig.Types],
            Pool = new QuestPool
            {
                Exploration = new ExplorationPool
                {
                    Locations = locations.ToDictionary(location => location.Key, location => location.Value),
                },
                Elimination = new EliminationPool { Targets = [] },
                Pickup = new ExplorationPool { Locations = locations.ToDictionary(location => location.Key, location => location.Value) },
            },
        };
    }

    /// <summary>
    /// An ExplorationQuestGenerator built by hand off the container's own services, with the given
    /// instances substituted for the parameters they fit - the shape DI would hand it if a mod had
    /// registered them.
    /// </summary>
    private static ExplorationQuestGenerator BuildGenerator(params object[] substitutes)
    {
        return (ExplorationQuestGenerator)Construct(typeof(ExplorationQuestGenerator), substitutes);
    }

    /// <summary>
    /// A second request builder that believes one mod is loaded. The gate only reads
    /// <c>Count</c>, so a placeholder element stands in for a real mod.
    /// </summary>
    private static RepeatableQuestNativeRequestBuilder BuildBuilderWithMods()
    {
        return (RepeatableQuestNativeRequestBuilder)Construct(
            typeof(RepeatableQuestNativeRequestBuilder),
            new SptMod[] { null! } as IReadOnlyList<SptMod>
        );
    }

    private static object Construct(Type type, params object[] substitutes)
    {
        var instance = ConstructCore(type, substitutes);
        if (instance is ExplorationQuestGenerator generator)
        {
            generator.NativeTestSeed = NativeSeed;
        }

        return instance;
    }

    private static object ConstructCore(Type type, params object[] substitutes)
    {
        // The generators carry the frozen 4.1.2 constructor plus the additive overload the container
        // uses; take the widest, which is what DI would pick
        var constructor = type.GetConstructors().MaxBy(candidate => candidate.GetParameters().Length)!;
        var arguments = constructor
            .GetParameters()
            .Select(parameter =>
                substitutes.FirstOrDefault(substitute => parameter.ParameterType.IsInstanceOfType(substitute))
                ?? DI.GetInstance().GetService(parameter.ParameterType)
            )
            .ToArray();

        return constructor.Invoke(arguments);
    }

    /// <summary>
    /// Stands in for a mod-registered generator: identical behaviour, different type. Chains the
    /// widest base constructor, so the native seam is wired and only the type check can fall back.
    /// </summary>
    private class TestExplorationQuestGeneratorSubclass(
        ISptLogger<ExplorationQuestGenerator> logger,
        LocationTable locationTable,
        RepeatableQuestHelper repeatableQuestHelper,
        RepeatableQuestRewardGenerator repeatableQuestRewardGenerator,
        ServerLocalisationService localisationService,
        RandomUtil randomUtil,
        MathUtil mathUtil,
        QuestConfig questConfig,
        RepeatableQuestNativeRequestBuilder requestBuilder
    )
        : ExplorationQuestGenerator(
            logger,
            locationTable,
            repeatableQuestHelper,
            repeatableQuestRewardGenerator,
            localisationService,
            randomUtil,
            mathUtil,
            questConfig,
            requestBuilder
        ) { }

    /// <inheritdoc cref="TestExplorationQuestGeneratorSubclass"/>
    private class TestRepeatableQuestHelperSubclass(
        ISptLogger<RepeatableQuestHelper> logger,
        TemplateTable templateTable,
        ServerLocalisationService serverLocalisationService,
        ICloner cloner,
        QuestConfig questConfig
    ) : RepeatableQuestHelper(logger, templateTable, serverLocalisationService, cloner, questConfig) { }

    /// <inheritdoc cref="TestExplorationQuestGeneratorSubclass"/>
    private class TestRepeatableQuestRewardGeneratorSubclass(
        ISptLogger<RepeatableQuestRewardGenerator> logger,
        TemplateTable templateTable,
        RandomUtil randomUtil,
        MathUtil mathUtil,
        ItemHelper itemHelper,
        PresetHelper presetHelper,
        HandbookHelper handbookHelper,
        ServerLocalisationService localisationService,
        ItemFilterService itemFilterService,
        SeasonalEventService seasonalEventService,
        ICloner cloner,
        QuestConfig questConfig
    )
        : RepeatableQuestRewardGenerator(
            logger,
            templateTable,
            randomUtil,
            mathUtil,
            itemHelper,
            presetHelper,
            handbookHelper,
            localisationService,
            itemFilterService,
            seasonalEventService,
            cloner,
            questConfig
        ) { }
}
