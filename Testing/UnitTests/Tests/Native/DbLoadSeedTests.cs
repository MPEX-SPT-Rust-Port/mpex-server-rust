using NUnit.Framework;
using SPTarkov.Server.Core.Native.Db;

namespace UnitTests.Tests.Native;

[TestFixture]
[NonParallelizable]
public class DbLoadSeedTests
{
    [TearDown]
    public void TearDown()
    {
        // Drain whatever a test left behind - the slot is process-global.
        DbLoadSeed.TryTake();
    }

    [Test]
    public void SeedIsConsumedExactlyOnce()
    {
        DbLoadSeed.Set(41, 7);

        Assert.That(DbLoadSeed.TryTake(), Is.EqualTo(((ulong)41, 7L)));
        Assert.That(DbLoadSeed.TryTake(), Is.Null, "a second take answers nothing");
    }

    [Test]
    public void ASecondSetOverwritesTheFirst()
    {
        DbLoadSeed.Set(1, 1);
        DbLoadSeed.Set(2, 9);

        Assert.That(DbLoadSeed.TryTake(), Is.EqualTo(((ulong)2, 9L)));
    }
}
