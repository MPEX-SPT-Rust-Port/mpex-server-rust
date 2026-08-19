using NUnit.Framework;
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
/// A failure here is not a flaky test. It names a production path that writes into a published
/// root on every pass, and the fix is a denylist entry in WriteBarriersPatch plus a line in the
/// Broken ledger - never a relaxed assertion.
/// </summary>
[TestFixture]
[NonParallelizable]
public class WriteBarrierChurnTests
{
    private DbPublisher _publisher = default!;
    private DatabaseMutationStamp _stamp = default!;
    private RagfairOfferGenerator _generator = default!;
    private RagfairOfferService _offerService = default!;
    private RandomUtil _randomUtil = default!;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        var di = DI.GetInstance();
        _publisher = di.GetService<DbPublisher>();
        _stamp = di.GetService<DatabaseMutationStamp>();
        _generator = di.GetService<RagfairOfferGenerator>();
        _offerService = di.GetService<RagfairOfferService>();
        _randomUtil = di.GetService<RandomUtil>();
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
        // hydrates the handbook, both writing into the tables being serialized.
        _publisher.EnsureCurrent();
        var settled = _stamp.Current;
        var epoch = _publisher.EnsureCurrent();

        Assert.Multiple(() =>
        {
            Assert.That(_stamp.Current, Is.EqualTo(settled), "the publish must not move the stamp");
            Assert.That(_publisher.EnsureCurrent(), Is.EqualTo(epoch), "a settled publisher must not republish");
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
