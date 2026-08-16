using System.Text;
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
}
