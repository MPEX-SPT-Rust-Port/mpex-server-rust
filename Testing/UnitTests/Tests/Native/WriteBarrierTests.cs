using NUnit.Framework;
using SPTarkov.Server.Core.Native.Db;
using SPTarkov.Server.Core.Services.Server;

namespace UnitTests.Tests.Native;

/// <summary>
/// Pins the seam the Ceciler-injected barriers call into: a bump reaches the process-global stamp,
/// a suppression scope silences it, and the scope restores rather than clears so nesting is safe.
/// These run in every configuration - the seam is ordinary source, only its callers are injected.
/// </summary>
[TestFixture]
[NonParallelizable]
public class WriteBarrierTests
{
    [Test]
    public void BumpAdvancesTheStamp()
    {
        var stamp = DI.GetInstance().GetService<DatabaseMutationStamp>();
        var before = stamp.Current;

        WriteBarrier.Bump();

        Assert.That(stamp.Current, Is.GreaterThan(before), "an unsuppressed barrier must reach the stamp");
    }

    [Test]
    public void ASuppressionScopeSilencesBumps()
    {
        var stamp = DI.GetInstance().GetService<DatabaseMutationStamp>();
        var before = stamp.Current;

        using (WriteBarrier.Suppress())
        {
            WriteBarrier.Bump();
            WriteBarrier.Bump();
        }

        Assert.That(stamp.Current, Is.EqualTo(before), "a suppressed barrier must not move the stamp");
    }

    [Test]
    public void ANestedScopeRestoresTheOuterSuppressionRatherThanClearingIt()
    {
        var stamp = DI.GetInstance().GetService<DatabaseMutationStamp>();
        var before = stamp.Current;

        using (WriteBarrier.Suppress())
        {
            using (WriteBarrier.Suppress()) { }

            // Still inside the outer scope: the inner Dispose must not have re-enabled bumps
            WriteBarrier.Bump();
        }

        Assert.That(stamp.Current, Is.EqualTo(before), "the inner scope must restore, not clear");
    }

    [Test]
    public void BumpsResumeAfterTheScopeCloses()
    {
        var stamp = DI.GetInstance().GetService<DatabaseMutationStamp>();

        using (WriteBarrier.Suppress()) { }

        var before = stamp.Current;
        WriteBarrier.Bump();

        Assert.That(stamp.Current, Is.GreaterThan(before));
    }
}
