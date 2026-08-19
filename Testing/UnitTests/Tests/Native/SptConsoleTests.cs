using NUnit.Framework;
using SPTarkov.Common.Native;

namespace UnitTests.Tests.Native;

[TestFixture]
public class SptConsoleTests
{
    // ReadLine is deliberately untested here: it blocks on the test runner's stdin when one is
    // attached. Its native line-strip logic is covered by console.rs tests, the live path by
    // scripts/smoke-mpex-server.sh.

    [Test]
    public void TryWriteForwardsToTheNativeLibrary()
    {
        // No pipeline init needed: without a console sink the native side writes stdout directly.
        Assert.That(SptConsole.TryWrite("spt-console test line\n"u8.ToArray(), toStdErr: false), Is.True);
        Assert.That(SptConsole.TryWrite("spt-console test err\n"u8.ToArray(), toStdErr: true), Is.True);
        Assert.That(SptConsole.TryWrite([], toStdErr: false), Is.True);
    }

    [Test]
    public void TitleAndClearAreBestEffortNoOpsWithoutATerminal()
    {
        Assert.DoesNotThrow(() => SptConsole.SetTitle("SPT test title"));
        Assert.DoesNotThrow(() => SptConsole.Clear());
    }
}
