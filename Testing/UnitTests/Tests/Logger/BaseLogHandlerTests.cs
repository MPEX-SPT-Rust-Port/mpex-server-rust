using Microsoft.Extensions.Logging;
using NUnit.Framework;
using SPTarkov.Common.Logger.Handlers;
using SPTarkov.Common.Models.Logging;

namespace UnitTests.Tests.Logger;

[TestFixture]
public class BaseLogHandlerTests
{
    private sealed class ProbeHandler : BaseLogHandler
    {
        public override LoggerType LoggerType
        {
            get { return LoggerType.Console; }
        }

        public override void Log(SptLogMessage message, BaseSptLoggerReference reference) { }

        public string Format(string processedMessage, SptLogMessage message, BaseSptLoggerReference reference)
        {
            return FormatMessage(processedMessage, message, reference);
        }

        public override ValueTask DisposeAsync()
        {
            return ValueTask.CompletedTask;
        }
    }

    private static SptLogMessage Message(string text)
    {
        // 2026-08-16 17:10:05.123 UTC, tid 7, thread "main" — mirrors the ffi.rs render test.
        return new SptLogMessage(
            "SPTarkov.Server.Core.Utils.App",
            DateTime.UnixEpoch.AddMilliseconds(1_786_900_205_123),
            LogLevel.Information,
            7,
            "main",
            text
        );
    }

    [Test]
    public void FormatMessageRendersThroughTheNativeTokenExpansion()
    {
        var handler = new ProbeHandler();
        var reference = new ConsoleSptLoggerReference
        {
            Type = LoggerType.Console,
            LogLevel = LogLevel.Information,
            Format = "[%date% %time%][%level%][%loggerShort%] %message%",
        };

        var line = handler.Format("hi", Message("hi"), reference);

        Assert.That(line, Is.EqualTo("[2026-08-16 17:10:05.123][Information][App] hi"));
    }

    [Test]
    public void FormatMessageStillAppendsTheExceptionOnTheCSharpSide()
    {
        var handler = new ProbeHandler();
        var reference = new ConsoleSptLoggerReference
        {
            Type = LoggerType.Console,
            LogLevel = LogLevel.Information,
            Format = "%message%",
        };
        Exception thrown;

        try
        {
            throw new InvalidOperationException("kaput");
        }
        catch (InvalidOperationException caught)
        {
            thrown = caught;
        }

        var message = Message("boom") with { Exception = thrown };
        var line = handler.Format("boom", message, reference);

        Assert.That(line, Does.StartWith("boom\nkaput\n"));
    }
}
