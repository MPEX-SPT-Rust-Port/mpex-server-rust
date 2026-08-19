using System.Text.Json;
using System.Text.Json.Nodes;
using System.Text.RegularExpressions;
using NUnit.Framework;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Generators.RepeatableQuests;
using SPTarkov.Server.Core.Helpers.Quest;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Enums;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Repeatable;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Native;
using SPTarkov.Server.Core.Utils;
using SPTarkov.Server.Core.Utils.Cloners;
using SPTarkov.Server.Core.Utils.Collections;

namespace UnitTests.Tests.Generators;

/// <summary>
/// Golden parity gate on the repeatable-quest port: the same seed must make the legacy 4.1.2 C# path
/// and the spt-native path build a byte-equal quest <i>and</i> leave a byte-equal
/// <see cref="QuestTypePool"/> behind, for every generator, at two level bands, for two traders.
/// The pool round-trip is half the contract - the controller keeps reading the instance it passed in
/// - so it is asserted alongside the quest rather than trusted.
///
/// State this fixture mutates, all of it restored in <see cref="Generate"/>'s <c>finally</c>:
/// <list type="number">
/// <item>
/// <see cref="QuestConfig.ForceLegacyRepeatableQuestGeneration"/> on the shared config singleton -
/// the path selector - and, for the one forced case, that band's
/// <c>EliminationConfig.SpecificLocationChance</c>. Both live in the varying half of the request, so
/// (unlike ragfair) no <c>DatabaseMutationStamp</c> bump is needed: this fixture never writes
/// anything the resident DB projects.
/// </item>
/// <item>
/// <see cref="RandomUtil.RandomSource"/> and <c>ProbabilityRandomSource.Current</c> - one shared
/// <see cref="SeededRandomSource"/> instance in both seams on the legacy path, mirroring the single
/// stream the Rust side installs for <c>seed</c>. Both restored to whatever was installed before.
/// </item>
/// <item>
/// The generator's <c>NativeTestSeed</c> - the DI singletons are shared, so it is always cleared.
/// </item>
/// </list>
/// The <see cref="QuestTypePool"/> is built fresh per call and never shared, so it needs no restore.
/// One thing is mutated and <i>not</i> restored: an eligible native send keeps the process-global
/// resident DB current, so the native side may hold a new epoch afterwards (and the shared builder's
/// <c>LastSendIncludedViewsOverride</c> seam records the send shape). It is test-session state like
/// ragfair's pre-generated flea - the native side validates the epoch every request names and a
/// stale one self-heals with a republish and one retry.
///
/// The one sanctioned parity gap: <c>MongoId</c> minting sits outside the seeded stream on both
/// sides, so ~12-25 ids per quest can never match. <see cref="LootIdNormalizer"/> maps every one that
/// some <c>_id</c> in the same document anchors - the quest id behind <c>qid</c>, a reward's
/// <c>target</c> pointing at its own <c>items[0]</c>, and any <c>id</c> member aliasing one of those.
/// What is left is the ids nothing references, which <see cref="Canonicalise"/> masks positionally,
/// and only when the shipped quest templates never carried the value - so a path that mints where the
/// other copies, whether it copies a template id or an anchored one, still fails. The single blind
/// spot that leaves is a straight swap of two <i>unreferenced</i> minted ids between sibling
/// <c>id</c> members: positional numbering renames both, so it compares equal. That is semantically
/// inert by definition - nothing reads either value - while aliasing, ordering and count changes are
/// all still caught.
/// </summary>
[TestFixture]
[NonParallelizable]
public class RepeatableQuestParityTests
{
    private static readonly ulong[] _seeds = [42, 1337];

    // Pickup is generated directly rather than through the pool: no shipped config lists it in
    // `types`, so the controller can never draw it
    private static readonly string[] _questTypes = ["Elimination", "Completion", "Exploration"];

    // Resolved against the shipped level bands in LevelForBand, not hard-coded: the fixture has to
    // track the data, and only the count of bands is a fixture assumption
    private static readonly int[] _bandIndexes = [0, 1];

    private static readonly int[] _traderIndexes = [0, 1];

    private static readonly MongoId _sessionId = new("6193a720f8ee7e52e4290000");

    private static readonly Regex _mongoIdShape = new("^[0-9a-f]{24}$", RegexOptions.Compiled);

    private EliminationQuestGenerator _eliminationQuestGenerator = default!;
    private CompletionQuestGenerator _completionQuestGenerator = default!;
    private ExplorationQuestGenerator _explorationQuestGenerator = default!;
    private PickupQuestGenerator _pickupQuestGenerator = default!;
    private RepeatableQuestHelper _repeatableQuestHelper = default!;
    private QuestConfig _questConfig = default!;
    private RandomUtil _randomUtil = default!;
    private ICloner _cloner = default!;

    /// <summary>The pmc daily config - the one the three pooled quest types are generated from.</summary>
    private RepeatableQuestConfig _pmcDaily = default!;

    /// <summary>The only shipped config carrying a <c>Pickup</c> block; both paths throw without it.</summary>
    private RepeatableQuestConfig _pickupConfig = default!;

    /// <summary>Every id the shipped quest templates carry, so a template id is never mistaken for a minted one.</summary>
    private HashSet<string> _templateIds = default!;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        var di = DI.GetInstance();

        _eliminationQuestGenerator = di.GetService<EliminationQuestGenerator>();
        _completionQuestGenerator = di.GetService<CompletionQuestGenerator>();
        _explorationQuestGenerator = di.GetService<ExplorationQuestGenerator>();
        _pickupQuestGenerator = di.GetService<PickupQuestGenerator>();
        _repeatableQuestHelper = di.GetService<RepeatableQuestHelper>();
        _questConfig = di.GetService<QuestConfig>();
        _randomUtil = di.GetService<RandomUtil>();
        _cloner = di.GetService<ICloner>();

        _pmcDaily = _questConfig.RepeatableQuests.First(config => config.Side == PlayerGroup.Pmc);
        _pickupConfig = _questConfig.RepeatableQuests.First(config => config.QuestConfig.Pickup is not null);

        var templates = JsonSerializer.Serialize(di.GetService<TemplateTable>().RepeatableQuests.Templates, SptNative.QuestJsonOptions);
        _templateIds = [.. Regex.Matches(templates, "[0-9a-f]{24}").Select(match => match.Value)];
    }

    [Test]
    public void TheSameSeedGeneratesEquivalentQuestsAndPoolsOnBothPaths(
        [ValueSource(nameof(_questTypes))] string questType,
        [ValueSource(nameof(_bandIndexes))] int bandIndex,
        [ValueSource(nameof(_traderIndexes))] int traderIndex,
        [ValueSource(nameof(_seeds))] ulong seed
    )
    {
        var pmcLevel = LevelForBand(_pmcDaily, questType, bandIndex);
        var traderId = TraderForType(questType, traderIndex);
        var label = $"type={questType} level={pmcLevel} trader={traderId}";

        var legacy = Generate(questType, pmcLevel, traderId, seed, forceLegacy: true);
        var native = Generate(questType, pmcLevel, traderId, seed, forceLegacy: false);

        LootJsonAssert.AssertEqual(legacy.Quest, native.Quest, label, seed);
        LootJsonAssert.AssertEqual(legacy.Pool, native.Pool, $"{label} pool", seed);
    }

    /// <summary>
    /// Pickup is dead in every shipped pool, so it gets its own case with a hand-built pool. It only
    /// runs against the config that ships a <c>Pickup</c> block - the native side reports
    /// "Pickup config was null" for the others, and C# would throw on the same input.
    /// </summary>
    [Test]
    public void TheSameSeedGeneratesEquivalentPickupQuestsOnBothPaths([ValueSource(nameof(_seeds))] ulong seed)
    {
        var pmcLevel = LevelForBand(_pickupConfig, "Pickup", 1);
        var traderId = _pickupConfig.TraderWhitelist.First(whitelist => whitelist.QuestTypes.Contains("Pickup")).TraderId;

        var legacy = Generate("Pickup", pmcLevel, traderId, seed, forceLegacy: true);
        var native = Generate("Pickup", pmcLevel, traderId, seed, forceLegacy: false);

        LootJsonAssert.AssertEqual(legacy.Quest, native.Quest, $"type=Pickup trader={traderId}", seed);
        LootJsonAssert.AssertEqual(legacy.Pool, native.Pool, $"type=Pickup trader={traderId} pool", seed);
    }

    /// <summary>
    /// The elimination half of the pool is only consumed down the specific-location arm, which
    /// shipped config reaches 0% of the time in the lowest band and 15% in the others - so no
    /// unforced case above proves that half of the pool round-trip. Forced here, the way
    /// <c>RagfairParityTests</c> forces its barter and pack arms.
    /// </summary>
    [Test]
    public void AForcedSpecificEliminationLocationMatchesOnBothPaths([ValueSource(nameof(_seeds))] ulong seed)
    {
        var pmcLevel = LevelForBand(_pmcDaily, "Elimination", 1);
        var traderId = TraderForType("Elimination", 0);

        var legacy = Generate("Elimination", pmcLevel, traderId, seed, forceLegacy: true, forceSpecificLocation: true);
        var native = Generate("Elimination", pmcLevel, traderId, seed, forceLegacy: false, forceSpecificLocation: true);

        LootJsonAssert.AssertEqual(legacy.Quest, native.Quest, $"forced-specific-location trader={traderId}", seed);
        LootJsonAssert.AssertEqual(legacy.Pool, native.Pool, $"forced-specific-location trader={traderId} pool", seed);

        // Trimming the chosen location out of the target's location list is the only thing that arm
        // does to the pool, so an untouched pool means the arm never ran
        Assert.That(
            native.Pool,
            Is.Not.EqualTo(Canonicalise(Serialize(BuildPool(_pmcDaily, pmcLevel)))),
            $"seed={seed} left the elimination pool untouched, so the specific-location arm never ran"
        );
    }

    /// <summary>
    /// An exploration pool with no locations left is the exhaustion the controller drives the
    /// generators into; both paths have to report it the same way - a null quest, not a throw and not
    /// a half-built quest.
    /// </summary>
    [Test]
    public void AnExhaustedPoolReturnsNullOnBothPaths([ValueSource(nameof(_seeds))] ulong seed)
    {
        var pmcLevel = LevelForBand(_pmcDaily, "Exploration", 1);
        var traderId = TraderForType("Exploration", 0);

        var legacy = Generate("Exploration", pmcLevel, traderId, seed, forceLegacy: true, exhaustPool: true);
        var native = Generate("Exploration", pmcLevel, traderId, seed, forceLegacy: false, exhaustPool: true);

        Assert.That(legacy.Quest, Is.EqualTo("null"), "the legacy path found a location in an emptied pool");
        Assert.That(native.Quest, Is.EqualTo("null"), "the native path found a location in an emptied pool");
        LootJsonAssert.AssertEqual(legacy.Pool, native.Pool, "exhausted pool", seed);
    }

    /// <summary>
    /// Without this the parity cases could pass by both paths producing something seed-independent.
    /// </summary>
    [Test]
    public void ADifferentSeedProducesADifferentQuest([ValueSource(nameof(_questTypes))] string questType)
    {
        var pmcLevel = LevelForBand(_pmcDaily, questType, 1);
        var traderId = TraderForType(questType, 0);

        var atSeed = Generate(questType, pmcLevel, traderId, _seeds[0], forceLegacy: false);
        var atOtherSeed = Generate(questType, pmcLevel, traderId, _seeds[1], forceLegacy: false);

        Assert.That(atOtherSeed.Quest, Is.Not.EqualTo(atSeed.Quest), $"{questType} ignored the seed - the draws are not reaching it");
    }

    /// <summary>
    /// One generation on one path, returning the canonicalised quest and the pool it left behind.
    /// </summary>
    private (string Quest, string Pool) Generate(
        string questType,
        int pmcLevel,
        MongoId traderId,
        ulong seed,
        bool forceLegacy,
        bool exhaustPool = false,
        bool forceSpecificLocation = false
    )
    {
        var repeatableConfig = questType == "Pickup" ? _pickupConfig : _pmcDaily;
        var generator = GeneratorFor(questType);
        var pool = BuildPool(repeatableConfig, pmcLevel);
        var expected = forceLegacy ? LootGenerationPath.Legacy : LootGenerationPath.Native;

        if (exhaustPool)
        {
            pool.Pool.Exploration.Locations!.Clear();
        }

        // Resolved only for the one case that writes to it - reading it here for a Pickup or
        // Exploration run would be a latent NRE the moment the shipped bands stop covering the level
        var forcedEliminationConfig = forceSpecificLocation
            ? _repeatableQuestHelper.GetEliminationConfigByPmcLevel(pmcLevel, repeatableConfig)
            : null;
        var originalForce = _questConfig.ForceLegacyRepeatableQuestGeneration;
        var originalSpecificLocationChance = forcedEliminationConfig?.SpecificLocationChance;
        var originalSource = _randomUtil.RandomSource;
        var originalProbabilitySource = ProbabilityRandomSource.Current;

        try
        {
            _questConfig.ForceLegacyRepeatableQuestGeneration = forceLegacy;

            // The whole repeatableConfig crosses in the varying half, so one write reaches both paths
            if (forcedEliminationConfig is not null)
            {
                forcedEliminationConfig.SpecificLocationChance = 100;
            }

            if (forceLegacy)
            {
                // One instance in both seams: one shared draw stream, mirroring the single
                // thread-local the Rust side installs for seed
                var seeded = new SeededRandomSource(seed);
                _randomUtil.RandomSource = seeded;
                ProbabilityRandomSource.Current = seeded;
            }
            else
            {
                SetNativeSeed(generator, seed);
            }

            var quest = generator.Generate(_sessionId, pmcLevel, traderId, pool, repeatableConfig);

            // Fail fast on silent fallback, and on a case that would compare two nulls, before
            // comparing anything
            Assert.That(PathTaken(generator), Is.EqualTo(expected), $"generation did not take the {expected} path");
            Assert.That(quest, exhaustPool ? Is.Null : Is.Not.Null, $"{expected} path produced an unexpected quest for {questType}");

            return (Canonicalise(Serialize(quest)), Canonicalise(Serialize(pool)));
        }
        finally
        {
            _questConfig.ForceLegacyRepeatableQuestGeneration = originalForce;
            if (forcedEliminationConfig is not null)
            {
                forcedEliminationConfig.SpecificLocationChance = originalSpecificLocationChance!.Value;
            }

            _randomUtil.RandomSource = originalSource;
            ProbabilityRandomSource.Current = originalProbabilitySource;
            SetNativeSeed(generator, null);
        }
    }

    /// <summary>
    /// <c>RepeatableQuestController.GenerateQuestPool</c> (<c>:840-885</c>), which is protected -
    /// what the generators are handed in production, rebuilt here.
    /// </summary>
    private QuestTypePool BuildPool(RepeatableQuestConfig repeatableConfig, int pmcLevel)
    {
        var pool = new QuestTypePool
        {
            Types = _cloner.Clone(repeatableConfig.Types)!,
            Pool = new QuestPool
            {
                Exploration = new ExplorationPool { Locations = new Dictionary<ELocationName, List<string>>() },
                Elimination = new EliminationPool { Targets = new Dictionary<string, TargetLocation>() },
                Pickup = new ExplorationPool { Locations = new Dictionary<ELocationName, List<string>>() },
            },
        };

        foreach (var (location, value) in repeatableConfig.Locations)
        {
            if (location != ELocationName.any)
            {
                pool.Pool.Exploration.Locations![location] = value;
                pool.Pool.Pickup.Locations![location] = value;
            }
        }

        pool.Pool.Pickup.Locations![ELocationName.any] = ["any"];

        var eliminationConfig = _repeatableQuestHelper.GetEliminationConfigByPmcLevel(pmcLevel, repeatableConfig)!;

        foreach (var target in new ProbabilityObjectArray<string, BossInfo>(_cloner, eliminationConfig.Targets))
        {
            if (target.Data?.IsBoss ?? false)
            {
                pool.Pool.Elimination.Targets!.Add(target.Key!, new TargetLocation { Locations = ["any"] });

                continue;
            }

            var allowedLocations =
                target.Key == "Savage"
                    ? repeatableConfig.Locations.Keys.Where(location => location != ELocationName.laboratory)
                    : repeatableConfig.Locations.Keys;

            pool.Pool.Elimination.Targets!.Add(
                target.Key!,
                new TargetLocation { Locations = allowedLocations.Select(location => location.ToString()).ToList() }
            );
        }

        return pool;
    }

    /// <summary>
    /// The midpoint of the <paramref name="bandIndex"/>'th shipped level band for the type, so the
    /// cases straddle bands the data declares rather than edges this fixture invents. Pickup ships no
    /// bands of its own, but <see cref="BuildPool"/> still reads the elimination bands for every
    /// type, so its level has to land inside one of those.
    /// </summary>
    private static int LevelForBand(RepeatableQuestConfig repeatableConfig, string questType, int bandIndex)
    {
        var bands = questType switch
        {
            "Elimination" or "Pickup" => repeatableConfig.QuestConfig.Elimination.Select(config => config.LevelRange).ToList(),
            "Completion" => repeatableConfig.QuestConfig.CompletionConfig.Select(config => config.LevelRange).ToList(),
            "Exploration" => repeatableConfig.QuestConfig.ExplorationConfig.Select(config => config.LevelRange).ToList(),
            _ => throw new ArgumentOutOfRangeException(nameof(questType), questType, "no level bands for this type"),
        };

        Assert.That(bands, Has.Count.GreaterThan(bandIndex), $"{questType} ships fewer than {bandIndex + 1} level bands");

        return (bands[bandIndex].Min + bands[bandIndex].Max) / 2;
    }

    private MongoId TraderForType(string questType, int traderIndex)
    {
        var traders = _pmcDaily.TraderWhitelist.Where(whitelist => whitelist.QuestTypes.Contains(questType)).ToList();

        Assert.That(traders, Has.Count.GreaterThan(traderIndex), $"{questType} is whitelisted for fewer than {traderIndex + 1} traders");

        return traders[traderIndex].TraderId;
    }

    private IRepeatableQuestGenerator GeneratorFor(string questType)
    {
        return questType switch
        {
            "Elimination" => _eliminationQuestGenerator,
            "Completion" => _completionQuestGenerator,
            "Exploration" => _explorationQuestGenerator,
            "Pickup" => _pickupQuestGenerator,
            _ => throw new ArgumentOutOfRangeException(nameof(questType), questType, "no generator for this type"),
        };
    }

    // The four generators share no base type, so the two Task 15 seams are reached by pattern match
    private static void SetNativeSeed(IRepeatableQuestGenerator generator, ulong? seed)
    {
        switch (generator)
        {
            case EliminationQuestGenerator elimination:
                elimination.NativeTestSeed = seed;
                break;
            case CompletionQuestGenerator completion:
                completion.NativeTestSeed = seed;
                break;
            case ExplorationQuestGenerator exploration:
                exploration.NativeTestSeed = seed;
                break;
            case PickupQuestGenerator pickup:
                pickup.NativeTestSeed = seed;
                break;
        }
    }

    private static LootGenerationPath PathTaken(IRepeatableQuestGenerator generator)
    {
        return generator switch
        {
            EliminationQuestGenerator elimination => elimination.LastPathTaken,
            CompletionQuestGenerator completion => completion.LastPathTaken,
            ExplorationQuestGenerator exploration => exploration.LastPathTaken,
            PickupQuestGenerator pickup => pickup.LastPathTaken,
            _ => throw new ArgumentOutOfRangeException(nameof(generator), generator, "no path seam on this generator"),
        };
    }

    /// <summary>
    /// Both sides go through the options the native wrapper itself uses, so <c>ELocationName</c> map
    /// keys render the same way and a pool difference is a real pool difference.
    /// </summary>
    private static string Serialize(object? payload)
    {
        return JsonSerializer.Serialize(payload, SptNative.QuestJsonOptions);
    }

    /// <summary>
    /// <see cref="LootIdNormalizer"/> first - it maps every <c>_id</c> to a positional placeholder and
    /// rewrites the members that point at one, <c>id</c> included. What survives is the minted ids
    /// nothing anchors: a <c>Reward</c>'s own <c>id</c>, the <c>QuestStatus</c>'s, and the condition
    /// and counter ids the generators re-mint. Those are masked positionally too, but only when the
    /// shipped templates never carried the value - masking a template id, like leaving an anchored
    /// one for this pass, would hide a path that mints where the other copies.
    /// </summary>
    private string Canonicalise(string json)
    {
        if (json == "null")
        {
            return "null";
        }

        var normalized = JsonNode.Parse(LootIdNormalizer.Normalize(json))!;
        MaskMintedIds(normalized, []);

        return normalized.ToJsonString();
    }

    private void MaskMintedIds(JsonNode node, Dictionary<string, string> placeholders)
    {
        switch (node)
        {
            case JsonObject obj:
                // Materialize the keys first: assigning obj[key] while enumerating throws
                foreach (var key in obj.Select(pair => pair.Key).ToList())
                {
                    var child = obj[key];
                    if (key == "id" && child is JsonValue value && value.TryGetValue<string>(out var id) && IsMintedId(id))
                    {
                        if (!placeholders.TryGetValue(id, out var placeholder))
                        {
                            placeholder = $"minted-{placeholders.Count}";
                            placeholders[id] = placeholder;
                        }

                        obj[key] = placeholder;
                    }
                    else if (child is not null)
                    {
                        MaskMintedIds(child, placeholders);
                    }
                }
                break;
            case JsonArray array:
                foreach (var child in array)
                {
                    if (child is not null)
                    {
                        MaskMintedIds(child, placeholders);
                    }
                }
                break;
        }
    }

    private bool IsMintedId(string value)
    {
        return _mongoIdShape.IsMatch(value) && !_templateIds.Contains(value);
    }
}
