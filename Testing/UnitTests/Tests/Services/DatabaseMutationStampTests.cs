using NUnit.Framework;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Services.Items;
using SPTarkov.Server.Core.Services.Server;

namespace UnitTests.Tests.Services;

/// <summary>
/// Pins the stamp itself and the one bump site that is safe to fire against the shared
/// container (a random tpl added to the blacklist matches nothing). The SeasonalEventService and
/// CustomItemService bump sites are one-line <c>_databaseMutationStamp?.Bump()</c> calls verified
/// by review: exercising them would force a seasonal event / add an item to the shared database
/// and pollute every fixture after this one.
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

        Assert.That(stamp.Current, Is.EqualTo(before + 1));
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
}
