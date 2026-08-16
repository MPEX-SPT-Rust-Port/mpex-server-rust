using System.Text;
using System.Text.Json;
using Microsoft.Extensions.Logging;
using NUnit.Framework;
using SPTarkov.Common.Logger;
using SPTarkov.Common.Models.Logging;
using SPTarkov.Common.Native;

namespace UnitTests.Tests.Logger;

[TestFixture]
[NonParallelizable]
public class SptLoggerDispatcherTests
{
    private string _directory = null!;

    [SetUp]
    public void SetUp()
    {
        _directory = Path.Combine(Path.GetTempPath(), $"spt-log-{Guid.NewGuid():N}");

        // The pipeline is process-global and another fixture's AddSptLogger call may have
        // initialised it against the real config; close so this fixture's config takes.
        NativeMethods.LoggerClose();

        var configJson = $$"""
            {
                "loggers": [
                    {
                        "type": "File",
                        "logLevel": "Information",
                        "format": "[%level%] %message%",
                        "filePath": {{JsonSerializer.Serialize(_directory)}},
                        "filePattern": "spt.log",
                        "maxFileSizeMB": 10,
                        "maxRollingFiles": 10,
                        "filters": []
                    }
                ]
            }
            """;
        var configBytes = Encoding.UTF8.GetBytes(configJson);
        var status = NativeMethods.LoggerInit(configBytes, (nuint)configBytes.Length, out var messagePtr, out var messageLen);
        NativeMethods.BufFree(messagePtr, messageLen);

        Assert.That(status, Is.EqualTo(0), "native log pipeline init failed");
    }

    [TearDown]
    public void TearDown()
    {
        NativeMethods.LoggerClose();

        if (Directory.Exists(_directory))
        {
            Directory.Delete(_directory, true);
        }
    }

    private static SptLogMessage Message(LogLevel level, string text, Exception? exception = null)
    {
        return new SptLogMessage("UnitTests.Category", DateTime.UtcNow, level, 1, "test", text, exception);
    }

    [Test]
    public async Task LogEmitsThroughTheNativePipeline()
    {
        var dispatcher = new SPTLoggerDispatcher(new SptLoggerConfiguration(), []);

        dispatcher.Log(Message(LogLevel.Information, "kept"));
        dispatcher.Log(Message(LogLevel.Debug, "below the configured level"));
        await dispatcher.DisposeAsync();

        var contents = await File.ReadAllTextAsync(Path.Combine(_directory, "spt.log"));

        Assert.That(contents, Is.EqualTo("[Information] kept\n"));
    }

    [Test]
    public async Task AnExceptionIsAppendedAfterTheFormattedLine()
    {
        var dispatcher = new SPTLoggerDispatcher(new SptLoggerConfiguration(), []);
        Exception thrown;

        try
        {
            throw new InvalidOperationException("kaput");
        }
        catch (InvalidOperationException caught)
        {
            thrown = caught;
        }

        dispatcher.Log(Message(LogLevel.Error, "boom", thrown));
        await dispatcher.DisposeAsync();

        var contents = await File.ReadAllTextAsync(Path.Combine(_directory, "spt.log"));

        Assert.That(contents, Does.StartWith("[Error] boom\nkaput\n"));
        Assert.That(contents, Does.Contain("AnExceptionIsAppendedAfterTheFormattedLine"));
    }
}
