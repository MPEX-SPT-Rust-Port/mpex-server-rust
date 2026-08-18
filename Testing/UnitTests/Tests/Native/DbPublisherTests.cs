using NUnit.Framework;
using SPTarkov.Server.Core.Native.Db;
using SPTarkov.Server.Core.Services.Server;

namespace UnitTests.Tests.Native;

[TestFixture]
[NonParallelizable]
public class DbPublisherTests
{
    [Test]
    public void EnsureCurrentPublishesOncePerStampAndRepublishesOnBump()
    {
        var di = DI.GetInstance();
        var publisher = di.GetService<DbPublisher>();
        var stamp = di.GetService<DatabaseMutationStamp>();

        // Epochs are process-global and other fixtures may publish too: assert relative
        // movement, never absolute values.
        var first = publisher.EnsureCurrent();
        var second = publisher.EnsureCurrent();
        Assert.That(second, Is.EqualTo(first), "no stamp movement, no republish");

        stamp.Bump();
        var third = publisher.EnsureCurrent();
        Assert.That(third, Is.GreaterThan(second), "a bumped stamp republishes");

        Assert.That(publisher.ForcePublish(), Is.GreaterThan(third));
    }
}
