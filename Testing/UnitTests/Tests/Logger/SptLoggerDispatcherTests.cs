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
        // initialised it against the real config; close so this fixture's config takes. Init is
        // ref-counted, so one close only drops one reference - drain a few, extra ones are no-ops.
        for (var i = 0; i < 4; i++)
        {
            NativeMethods.LoggerClose();
        }

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

    [Test]
    public async Task ReloadConfigurationRetargetsTheNativePipeline()
    {
        // SetUp initialised the pipeline at Information into _directory; this config is Debug
        // into a second directory, so the line below proves both the retarget and the level swap.
        var retargetDirectory = Path.Combine(Path.GetTempPath(), $"spt-log-{Guid.NewGuid():N}");
        var configuration = new SptLoggerConfiguration
        {
            Loggers =
            [
                new FileSptLoggerReference
                {
                    Type = LoggerType.File,
                    LogLevel = LogLevel.Debug,
                    Format = "[%level%] %message%",
                    FilePath = retargetDirectory,
                    FilePattern = "spt.log",
                    MaxFileSizeMb = 10,
                    MaxRollingFiles = 10,
                },
            ],
        };
        var dispatcher = new SPTLoggerDispatcher(configuration, []);

        try
        {
            Assert.That(dispatcher.ReloadConfiguration(), Is.True);
            Assert.That(dispatcher.IsLogEnabled(LogLevel.Debug), Is.True);

            dispatcher.Log(Message(LogLevel.Debug, "retargeted"));
            await dispatcher.DisposeAsync();

            var contents = await File.ReadAllTextAsync(Path.Combine(retargetDirectory, "spt.log"));

            Assert.That(contents, Is.EqualTo("[Debug] retargeted\n"));
        }
        finally
        {
            if (Directory.Exists(retargetDirectory))
            {
                Directory.Delete(retargetDirectory, true);
            }
        }
    }

    private sealed class CapturingHandler : ILogHandler
    {
        public List<(SptLogMessage Message, BaseSptLoggerReference Reference)> Received { get; } = [];

        public LoggerType LoggerType
        {
            get { return LoggerType.File; }
        }

        public void Log(SptLogMessage message, BaseSptLoggerReference reference)
        {
            Received.Add((message, reference));
        }

        public ValueTask DisposeAsync()
        {
            return ValueTask.CompletedTask;
        }
    }

    private FileSptLoggerReference HandlerReference(List<SptLoggerFilter>? filters = null)
    {
        return new FileSptLoggerReference
        {
            Type = LoggerType.File,
            LogLevel = LogLevel.Information,
            Format = "[%level%] %message%",
            FilePath = _directory,
            FilePattern = "spt.log",
            MaxFileSizeMb = 10,
            MaxRollingFiles = 10,
            Filters = filters ?? [],
        };
    }

    [Test]
    public async Task HandlersReceiveRoutedMessagesAgain()
    {
        var handler = new CapturingHandler();
        var reference = HandlerReference([
            new SptLoggerFilter
            {
                Type = SptLoggerFilterType.Exclude,
                Name = "Noise.*",
                MatchingType = MatchingType.Regex,
            },
        ]);
        var dispatcher = new SPTLoggerDispatcher(new SptLoggerConfiguration { Loggers = [reference] }, [handler]);

        dispatcher.Log(Message(LogLevel.Information, "routed"));
        dispatcher.Log(Message(LogLevel.Debug, "below the reference level"));
        dispatcher.Log(new SptLogMessage("Noise.Chatter", DateTime.UtcNow, LogLevel.Error, 1, "test", "excluded"));
        await dispatcher.DisposeAsync();

        Assert.That(handler.Received, Has.Count.EqualTo(1));
        Assert.That(handler.Received[0].Message.Message, Is.EqualTo("routed"));
        Assert.That(handler.Received[0].Reference, Is.SameAs(reference));
    }

    [Test]
    public async Task ARegisteredHandlerReceivesMessages()
    {
        // The production path: AddSptLogger builds the dispatcher from its own service collection,
        // so a mod's handler is never constructor-injected and has to register itself.
        var handler = new CapturingHandler();
        var reference = HandlerReference();
        var dispatcher = new SPTLoggerDispatcher(new SptLoggerConfiguration { Loggers = [reference] }, []);

        dispatcher.Log(Message(LogLevel.Information, "before registering"));
        dispatcher.RegisterHandler(handler);
        dispatcher.Log(Message(LogLevel.Information, "after registering"));
        await dispatcher.DisposeAsync();

        Assert.That(handler.Received, Has.Count.EqualTo(1));
        Assert.That(handler.Received[0].Message.Message, Is.EqualTo("after registering"));
        Assert.That(handler.Received[0].Reference, Is.SameAs(reference));
    }

    private sealed class ReentrantHandler : ILogHandler
    {
        public SPTLoggerDispatcher? Dispatcher { get; set; }

        public int Calls { get; private set; }

        public LoggerType LoggerType
        {
            get { return LoggerType.File; }
        }

        public void Log(SptLogMessage message, BaseSptLoggerReference reference)
        {
            Calls++;
            Dispatcher?.Log(new SptLogMessage("UnitTests.Nested", DateTime.UtcNow, LogLevel.Information, 1, "test", "nested"));
        }

        public ValueTask DisposeAsync()
        {
            return ValueTask.CompletedTask;
        }
    }

    [Test]
    public async Task AReentrantHandlerDoesNotRecurseAndItsLineStillLands()
    {
        var handler = new ReentrantHandler();
        var dispatcher = new SPTLoggerDispatcher(new SptLoggerConfiguration { Loggers = [HandlerReference()] }, [handler]);
        handler.Dispatcher = dispatcher;

        dispatcher.Log(Message(LogLevel.Information, "outer"));
        await dispatcher.DisposeAsync();

        Assert.That(handler.Calls, Is.EqualTo(1), "nested fan-out must be skipped");

        var contents = await File.ReadAllTextAsync(Path.Combine(_directory, "spt.log"));

        Assert.That(contents, Does.Contain("outer"));
        Assert.That(contents, Does.Contain("nested"), "the nested line still reaches the native sinks");
    }

    private sealed class ThrowingHandler : ILogHandler
    {
        public LoggerType LoggerType
        {
            get { return LoggerType.File; }
        }

        public void Log(SptLogMessage message, BaseSptLoggerReference reference)
        {
            throw new InvalidOperationException("broken mod handler");
        }

        public ValueTask DisposeAsync()
        {
            return ValueTask.CompletedTask;
        }
    }

    [Test]
    public async Task AThrowingHandlerDoesNotBreakTheEmitPath()
    {
        var dispatcher = new SPTLoggerDispatcher(new SptLoggerConfiguration { Loggers = [HandlerReference()] }, [new ThrowingHandler()]);

        dispatcher.Log(Message(LogLevel.Information, "survives"));
        await dispatcher.DisposeAsync();

        var contents = await File.ReadAllTextAsync(Path.Combine(_directory, "spt.log"));

        Assert.That(contents, Does.Contain("survives"));
    }
}
