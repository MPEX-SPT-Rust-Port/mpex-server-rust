using Spectre.Console;
using SPTarkov.Common.Models.Logging;

namespace SPTarkov.Common.Logger;

public sealed class SptLogger<T> : ISptLogger<T>
{
    private readonly SPTLoggerDispatcher _loggerDispatcher;

    private string _category = typeof(T).FullName;

    // configuration is part of the frozen 4.1.2 constructor shape; the level gate itself lives on
    // the dispatcher (whose C# body is the twin of logger.rs's should_emit level check).
    public SptLogger(SptLoggerConfiguration configuration, SPTLoggerDispatcher loggerDispatcher)
    {
        _loggerDispatcher = loggerDispatcher;
    }

    public void OverrideCategory(string category)
    {
        _category = category;
    }

    public void LogWithColor(string data, Color? textColor = null, Color? backgroundColor = null, Exception? ex = null)
    {
        Log(LogLevel.Information, data, textColor, backgroundColor, ex);
    }

    public void Success(string data, Exception? ex = null)
    {
        Log(LogLevel.Information, data, ex: ex);
    }

    public void Error(string data, Exception? ex = null)
    {
        Log(LogLevel.Error, data, ex: ex);
    }

    public void Warning(string data, Exception? ex = null)
    {
        Log(LogLevel.Warning, data, ex: ex);
    }

    public void Info(string data, Exception? ex = null)
    {
        Log(LogLevel.Information, data, ex: ex);
    }

    public void Debug(string data, Exception? ex = null)
    {
        Log(LogLevel.Debug, data, ex: ex);
    }

    public void Critical(string data, Exception? ex = null)
    {
        Log(LogLevel.Critical, data, ex: ex);
    }

    public void Log(LogLevel level, string data, Color? textColor = null, Color? backgroundColor = null, Exception? ex = null)
    {
        _loggerDispatcher.Log(
            new SptLogMessage(
                _category,
                DateTime.UtcNow,
                level,
                Environment.CurrentManagedThreadId,
                Thread.CurrentThread.Name,
                data,
                ex,
                textColor,
                backgroundColor
            )
        );
    }

    public bool IsLogEnabled(LogLevel level)
    {
        return _loggerDispatcher.IsLogEnabled(level);
    }
}
