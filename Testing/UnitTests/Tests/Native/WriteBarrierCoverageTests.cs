using NUnit.Framework;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Native.Db;
using SPTarkov.Server.Core.Services.Server;

namespace UnitTests.Tests.Native;

/// <summary>
/// The spec's Phase 2 acceptance gate: a mod-simulation that mutates each published root through
/// public surface and asserts the stamp moved. One test per root DbPublisher ships - templates,
/// traders, globals, locations, hideout, and the configs root. Table roots that are not published
/// (bots, locales, match, server, settings) are deliberately absent: a barrier there could only buy
/// a republish that changes nothing.
///
/// The traders case is the only committed evidence that WriteBarriersPatch's BaseType edge works.
/// TradersTable declares no properties of its own - it is a `Dictionary&lt;MongoId, Trader&gt;`
/// subclass, so the whole traders subgraph is reachable only through the base type's generic
/// argument. Any replacement for this test has to stay on Trader or below.
///
/// What this fixture cannot cover, by construction, is container mutation - a mod calling
/// Add/Remove/indexer-set on a table collection. That gap is documented in RUST-ROADMAP.md's
/// Broken ledger and covered by the kill switches; see WriteBarrierChurnTests for the other side
/// of the trade.
/// </summary>
[TestFixture]
[NonParallelizable]
public class WriteBarrierCoverageTests
{
    private DatabaseMutationStamp _stamp = default!;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        _stamp = DI.GetInstance().GetService<DatabaseMutationStamp>();
    }

    [SetUp]
    public void RequireBarriers()
    {
        if (!WriteBarrier.Installed)
        {
            Assert.Ignore("write barriers are Ceciler-injected in Release builds only");
        }
    }

    [Test]
    public void AWriteIntoTheTemplatesRootBumps()
    {
        var properties = DI.GetInstance().GetService<TemplateTable>().Items.Values.First(item => item.Properties is not null).Properties!;
        var original = properties.Weight;

        AssertBumps(() => properties.Weight = original + 1, () => properties.Weight = original);
    }

    [Test]
    public void AWriteIntoTheTradersRootBumps()
    {
        var traderBase = DI.GetInstance().GetService<TradersTable>().Values.First().Base;
        var original = traderBase.Name;

        AssertBumps(() => traderBase.Name = "write-barrier-probe", () => traderBase.Name = original);
    }

    [Test]
    public void AWriteIntoTheGlobalsRootBumps()
    {
        var stamina = DI.GetInstance().GetService<GlobalTable>().Configuration.Stamina;
        var original = stamina.Capacity;

        AssertBumps(() => stamina.Capacity = original + 1, () => stamina.Capacity = original);
    }

    [Test]
    public void AWriteIntoTheLocationsRootBumps()
    {
        var locationBase = DI.GetInstance().GetService<LocationTable>().Bigmap.Base;
        var original = locationBase.Name;

        AssertBumps(() => locationBase.Name = "write-barrier-probe", () => locationBase.Name = original);
    }

    [Test]
    public void AWriteIntoTheHideoutRootBumps()
    {
        var recipe = DI.GetInstance().GetService<HideoutTable>().Production.ScavRecipes!.First();
        var original = recipe.ProductionTime;

        AssertBumps(() => recipe.ProductionTime = original + 1, () => recipe.ProductionTime = original);
    }

    /// <summary>
    /// The configs root (Phase 4). Both halves matter: the scalar proves the config type itself is a
    /// walk root, and the nested record proves the BFS descends through config records the way it
    /// descends through table records - RagfairConfig.Sell.Chance is three types deep.
    /// </summary>
    [Test]
    public void AWriteIntoAConfigBumps()
    {
        var ragfairConfig = DI.GetInstance().GetService<RagfairConfig>();
        var originalInterval = ragfairConfig.RunIntervalSeconds;

        AssertBumps(
            () => ragfairConfig.RunIntervalSeconds = originalInterval + 1,
            () => ragfairConfig.RunIntervalSeconds = originalInterval
        );

        var chance = ragfairConfig.Sell.Chance;
        var originalBase = chance.Base;

        AssertBumps(() => chance.Base = originalBase + 1, () => chance.Base = originalBase);
    }

    private void AssertBumps(Action mutate, Action restore)
    {
        var before = _stamp.Current;
        try
        {
            mutate();

            Assert.That(_stamp.Current, Is.GreaterThan(before), "a public-surface write into a published root must move the stamp");
        }
        finally
        {
            restore();
        }
    }
}
