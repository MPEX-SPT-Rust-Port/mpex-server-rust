using NUnit.Framework;
using SPTarkov.Server.Core.Generators.Loot;
using SPTarkov.Server.Core.Generators.Ragfair;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Native.Db;
using SPTarkov.Server.Core.Services.Ragfair;
using SPTarkov.Server.Core.Services.Server;
using SPTarkov.Server.Core.Utils;

namespace UnitTests.Tests.Native;

/// <summary>
/// The other half of Phase 2's bargain. Barriers are only worth having if a steady-state workload
/// does not dirty the stamp, because a dirty stamp costs a full five-root republish on the next
/// native call. Each test runs a representative workload twice and asserts the second pass needed
/// no republish - i.e. the workload's own writes are either absent, converged, or suppressed.
///
/// A failure here is not a flaky test. It names a path that writes into a published root on every
/// pass, and there are exactly two sanctioned fixes - never a relaxed assertion. Which one applies
/// depends on what the write touches:
///
/// - a production path mutating an object a published root already points at: a denylist entry in
///   WriteBarriersPatch, plus a line in the Broken ledger for the coverage it gives up;
/// - a materialisation write into objects no published root points at: a WriteBarrier.Suppress()
///   scope around it. The precedent is SptNative.DecodeResult, where deserializing a native response
///   builds fresh Quest condition types and SpawnpointTemplates that nothing in the database can
///   reach (see ANativeResponseDecodeDoesNotMoveTheStamp), and DbPublisher.PublishLocked before it.
///   A scope is a narrower blind spot than a denied type, not the absence of one: a genuine database
///   write inside its extent goes unseen too.
/// </summary>
[TestFixture]
[NonParallelizable]
public class WriteBarrierChurnTests
{
    private const string DecodeLocationId = "factory4_day";

    private DbPublisher _publisher = default!;
    private DatabaseMutationStamp _stamp = default!;
    private RagfairOfferGenerator _generator = default!;
    private RagfairOfferService _offerService = default!;
    private RandomUtil _randomUtil = default!;
    private LocationLootGenerator _locationLootGenerator = default!;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        var di = DI.GetInstance();
        _publisher = di.GetService<DbPublisher>();
        _stamp = di.GetService<DatabaseMutationStamp>();
        _generator = di.GetService<RagfairOfferGenerator>();
        _offerService = di.GetService<RagfairOfferService>();
        _randomUtil = di.GetService<RandomUtil>();
        _locationLootGenerator = di.GetService<LocationLootGenerator>();
    }

    [OneTimeTearDown]
    public void OneTimeTearDown()
    {
        _generator.NativeTestSeed = null;
        // A full pass leaves tens of thousands of offers behind, and the holder's per-base-type cap
        // reads what is already in it - a saturated holder would reject the single offer the next
        // fixture's cases expect
        ClearOffers();
        _stamp.Bump();
    }

    [SetUp]
    public void RequireBarriers()
    {
        if (!WriteBarrier.Installed)
        {
            Assert.Ignore("write barriers are Ceciler-injected in Release builds only");
        }

        ClearOffers();
    }

    [Test]
    public void APublishDoesNotDirtyTheStampItJustRead()
    {
        // The loop the suppression scope exists to prevent: projection materialises LazyLoads and
        // hydrates the handbook, both writing into the tables being serialized. The bump is
        // load-bearing - EnsureCurrent only publishes when the stamp has moved, so without it a
        // fixture that left the publisher settled would let this pass without projecting anything.
        _stamp.Bump();
        _publisher.EnsureCurrent();
        var settled = _stamp.Current;
        var epoch = _publisher.EnsureCurrent();

        Assert.Multiple(() =>
        {
            Assert.That(_stamp.Current, Is.EqualTo(settled), "the publish must not move the stamp");
            Assert.That(_publisher.EnsureCurrent(), Is.EqualTo(epoch), "a settled publisher must not republish");
        });
    }

    /// <summary>
    /// The invariant SptNative.DecodeResult's suppression scope rests on, asserted directly rather
    /// than through the republish it would otherwise cost. A static-containers response deserializes
    /// hundreds of SpawnpointTemplates, whose setters are barriered because LocationTable reaches the
    /// type - so without the scope one native call moves the stamp once per decoded property and
    /// every call after it pays a five-root republish. Narrowing or removing that scope has to fail
    /// here by name, not turn up as a perf regression in LocationLootGeneratorTests.
    /// </summary>
    [Test]
    public void ANativeResponseDecodeDoesNotMoveTheStamp()
    {
        // factory4_day is the cheapest shipped map; the first call pays the LazyLoad and the JIT
        _locationLootGenerator.GenerateLocationLoot(DecodeLocationId);
        var before = _stamp.Current;

        var loot = _locationLootGenerator.GenerateLocationLoot(DecodeLocationId);

        Assert.Multiple(() =>
        {
            Assert.That(loot, Is.Not.Empty, "the decode produced no spawn points, so it barriered nothing and proves nothing");
            Assert.That(_stamp.Current, Is.EqualTo(before), "a native response decode moved the stamp");
        });
    }

    [Test]
    public void ASteadyStateRagfairPassConvergesToNoRepublish()
    {
        var ragfairConfig = DI.GetInstance().GetService<RagfairConfig>();
        var trusted = ragfairConfig.TrustNativeRequestCacheWithMods;
        ragfairConfig.TrustNativeRequestCacheWithMods = true;
        _generator.NativeTestSeed = 424242;

        try
        {
            // First pass may legitimately flip CanSellOnRagfair on some templates (true->false,
            // one-way), so let it settle before measuring.
            GenerateOnePass();
            _publisher.EnsureCurrent();
            var epoch = _publisher.EnsureCurrent();

            GenerateOnePass();

            Assert.That(_publisher.EnsureCurrent(), Is.EqualTo(epoch), "a settled ragfair pass must not force a republish");
        }
        finally
        {
            ragfairConfig.TrustNativeRequestCacheWithMods = trusted;
        }
    }

    /// <summary>
    /// The configs root's side of the bargain (Phase 4). A config edit has to cost exactly one
    /// republish, not one per pass: the barrier fires on the write, the next native call pays for it,
    /// and the workload that reads the new value must not keep dirtying the stamp afterwards.
    /// RunIntervalSeconds is deliberately a property the generation path never reads - the subject
    /// here is the barrier's churn, not the config's effect.
    /// </summary>
    [Test]
    public void AConfigWriteCostsExactlyOneRepublish()
    {
        var ragfairConfig = DI.GetInstance().GetService<RagfairConfig>();
        var trusted = ragfairConfig.TrustNativeRequestCacheWithMods;
        var interval = ragfairConfig.RunIntervalSeconds;
        ragfairConfig.TrustNativeRequestCacheWithMods = true;
        _generator.NativeTestSeed = 424242;

        try
        {
            // Same settling as the sibling test - the first pass may legitimately flip
            // CanSellOnRagfair, and this test can only measure a settled baseline.
            GenerateOnePass();
            _publisher.EnsureCurrent();
            var settled = _publisher.EnsureCurrent();

            ragfairConfig.RunIntervalSeconds = interval + 1;
            var afterWrite = _publisher.EnsureCurrent();

            GenerateOnePass();

            Assert.Multiple(() =>
            {
                Assert.That(afterWrite, Is.GreaterThan(settled), "a config write must dirty the stamp and force one republish");
                Assert.That(
                    _publisher.EnsureCurrent(),
                    Is.EqualTo(afterWrite),
                    "the config write must converge - one republish, not one per pass"
                );
            });
        }
        finally
        {
            ragfairConfig.RunIntervalSeconds = interval;
            ragfairConfig.TrustNativeRequestCacheWithMods = trusted;
        }
    }

    /// <summary>
    /// One dynamic-offer pass, driven exactly as <c>RagfairResidentDbTests</c> drives it: the
    /// unseeded per-template cap draw in <c>RagfairOfferHolder</c> runs off a fresh fixed stream, and
    /// the holder starts empty so the second pass pays the same draws as the first.
    /// </summary>
    private void GenerateOnePass()
    {
        ClearOffers();

        var original = _randomUtil.RandomSource;
        try
        {
            _randomUtil.RandomSource = new SeededRandomSource(424242);
            _generator.GenerateDynamicOffers();
        }
        finally
        {
            _randomUtil.RandomSource = original;
        }
    }

    private void ClearOffers()
    {
        foreach (var offer in _offerService.GetOffers().ToList())
        {
            _offerService.RemoveOfferById(offer.Id);
        }
    }
}
