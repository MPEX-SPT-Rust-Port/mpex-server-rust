using Spectre.Console;
using SPTarkov.Common.Models.Logging;

namespace SPTarkov.Common.Logger;

public sealed class SptLogger<T>(SptLoggerConfiguration configuration, SPTLoggerDispatcher loggerDispatcher) : ISptLogger<T>
{
    private string _category = typeof(T).FullName;

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
        loggerDispatcher.Log(
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
        return configuration.Loggers.Any(l => l.LogLevel.CanLog(level));
    }
}
