using System.Reflection;
using NUnit.Framework;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Spt.Mod;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Services.Items;
using SPTarkov.Server.Core.Services.Modding.Custom;
using SPTarkov.Server.Core.Services.Ragfair;
using SPTarkov.Server.Core.Services.Server;
using SPTarkov.Server.Core.Utils.Cloners;

namespace UnitTests.Tests.Services;

/// <summary>
/// Pins the stamp itself and the hand-written bump sites that are safe to fire against the shared
/// container. Since Phase 2 most mutation visibility comes from the Ceciler-injected barriers
/// instead - covered by WriteBarrierCoverageTests, which needs a Release build. The
/// SeasonalEventService and CustomItemService bump sites remain verified by review: exercising
/// them would force a seasonal event / add an item to the shared database and pollute every
/// fixture after this one.
/// </summary>
[TestFixture]
[NonParallelizable]
public class DatabaseMutationStampTests
{
    [Test]
    public void BumpAdvancesCurrent()
    {
        var stamp = new DatabaseMutationStamp();
        var before = stamp.Current;

        stamp.Bump();

        // The counter is process-global since Phase 2 (the injected barriers have no container to
        // reach), so assert movement, never an exact value - the house rule for epochs applies here now
        Assert.That(stamp.Current, Is.GreaterThan(before));
    }

    [Test]
    public void AddItemToBlacklistCacheBumps()
    {
        var di = DI.GetInstance();
        var stamp = di.GetService<DatabaseMutationStamp>();
        var before = stamp.Current;

        di.GetService<ItemFilterService>().AddItemToBlacklistCache([new MongoId()]);

        Assert.That(stamp.Current, Is.GreaterThan(before));
    }

    [Test]
    public void AddItemToLootableBlacklistCacheBumps()
    {
        var di = DI.GetInstance();
        var stamp = di.GetService<DatabaseMutationStamp>();
        var before = stamp.Current;

        di.GetService<ItemFilterService>().AddItemToLootableBlacklistCache([new MongoId()]);

        Assert.That(stamp.Current, Is.GreaterThan(before));
    }

    [Test]
    public void ReplaceFleaBasePricesBumps()
    {
        var di = DI.GetInstance();
        var stamp = di.GetService<DatabaseMutationStamp>();
        var priceService = di.GetService<RagfairPriceService>();

        // Swap the static price cache for a single generated tpl: rewriting the real cache would
        // recost every item on the shared database, and a generated id polluting templates/prices
        // cannot affect another fixture
        var staticPrices = typeof(RagfairPriceService).GetField("StaticPrices", BindingFlags.Instance | BindingFlags.NonPublic)!;
        var realStaticPrices = staticPrices.GetValue(priceService);
        staticPrices.SetValue(priceService, new Dictionary<MongoId, double> { { new MongoId(), 1000 } });

        try
        {
            var before = stamp.Current;

            priceService.ReplaceFleaBasePrices();

            Assert.That(stamp.Current, Is.GreaterThan(before));
        }
        finally
        {
            staticPrices.SetValue(priceService, realStaticPrices);
        }
    }

    [Test]
    public void CreateQuestBumps()
    {
        var di = DI.GetInstance();
        var stamp = di.GetService<DatabaseMutationStamp>();
        var quests = di.GetService<TemplateTable>().Quests;

        // A real quest cloned onto a generated id - the clone's own setters are barriered in a
        // Release build, so every write it costs lands before the count is captured
        var quest = di.GetService<ICloner>().Clone(quests.First().Value)!;
        quest.Id = new MongoId();

        // No locales, so CreateQuest returns right after adding the quest: the table is already
        // mutated at that point, and a registered locale transformer cannot be unregistered again
        var details = new NewQuestDetails { NewQuest = quest, Locales = [] };

        try
        {
            var before = stamp.Current;

            di.GetService<CustomQuestService>().CreateQuest(details);

            Assert.That(stamp.Current, Is.GreaterThan(before));
        }
        finally
        {
            quests.Remove(quest.Id);
        }
    }
}
