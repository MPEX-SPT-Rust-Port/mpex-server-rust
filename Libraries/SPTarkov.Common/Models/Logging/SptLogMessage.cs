using Spectre.Console;

namespace SPTarkov.Common.Models.Logging;

/// <summary>
/// TextColor/BackgroundColor are retained for 4.1.2 mod compatibility and are never rendered.
/// </summary>
public record SptLogMessage(
    string Logger,
    DateTime LogTime,
    LogLevel LogLevel,
    int threadId,
    string? threadName,
    string Message,
    Exception? Exception = null,
    Color? TextColor = null,
    Color? BackgroundColor = null
);
