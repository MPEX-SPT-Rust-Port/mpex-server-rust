using Microsoft.Extensions.Logging;
using NUnit.Framework;
using SPTarkov.Common.Logger.Handlers;
using SPTarkov.Common.Models.Logging;

namespace UnitTests.Tests.Logger;

[TestFixture]
public class FileLogHandlerTests
{
    private string _directory = null!;

    [SetUp]
    public void SetUp()
    {
        _directory = Path.Combine(Path.GetTempPath(), $"spt-log-{Guid.NewGuid():N}");
    }

    [TearDown]
    public void TearDown()
    {
        if (Directory.Exists(_directory))
        {
            Directory.Delete(_directory, true);
        }
    }

    private static FileSptLoggerReference Reference(string directory, string pattern)
    {
        return new FileSptLoggerReference
        {
            Type = LoggerType.File,
            LogLevel = LogLevel.Information,
            Format = "%message%",
            FilePath = directory,
            FilePattern = pattern,
            MaxFileSizeMb = 10,
            MaxRollingFiles = 10,
        };
    }

    private static SptLogMessage Message(string text)
    {
        return new SptLogMessage("UnitTests", DateTime.UtcNow, LogLevel.Information, 1, "test", text);
    }

    [Test]
    public async Task LoggingAfterDisposeIsANoOp()
    {
        var reference = Reference(_directory, "spt.log");
        var handler = new FileLogHandler();

        handler.Log(Message("before"), reference);
        await handler.DisposeAsync();
        handler.Log(Message("after"), reference);

        var contents = await File.ReadAllTextAsync(Path.Combine(_directory, "spt.log"));

        // The Rust sink terminates every line with \n on both platforms.
        Assert.That(contents, Is.EqualTo("before\n"));
    }

    [Test]
    public async Task AnUnopenableTargetIsDisabledAndReportedOnce()
    {
        // A path under an existing *file* cannot be created as a directory, so spt_log_open fails.
        Directory.CreateDirectory(_directory);
        var blocker = Path.Combine(_directory, "blocker");
        await File.WriteAllTextAsync(blocker, "not a directory");

        var reference = Reference(Path.Combine(blocker, "logs"), "spt.log");
        var handler = new FileLogHandler();

        var captured = new StringWriter();
        var original = Console.Error;
        Console.SetError(captured);

        try
        {
            Assert.DoesNotThrow(() => handler.Log(Message("first"), reference));
            Assert.DoesNotThrow(() => handler.Log(Message("second"), reference));
        }
        finally
        {
            Console.SetError(original);
            await handler.DisposeAsync();
        }

        var reports = captured.ToString().Split("File logging for this target is disabled.");

        Assert.That(reports, Has.Length.EqualTo(2), "the failure should be reported exactly once");
    }
}
