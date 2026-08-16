using Spectre.Console;

namespace SPTarkov.Common.Models.Logging;

public interface ISptLogger<T>
{
    /// <summary>
    /// Colour arguments are accepted for 4.1.2 mod compatibility and ignored; output is plain text.
    /// </summary>
    void LogWithColor(string data, Color? textColor = null, Color? backgroundColor = null, Exception? ex = null);
    void Success(string data, Exception? ex = null);
    void Error(string data, Exception? ex = null);
    void Warning(string data, Exception? ex = null);
    void Info(string data, Exception? ex = null);
    void Debug(string data, Exception? ex = null);
    void Critical(string data, Exception? ex = null);
    void Log(LogLevel level, string data, Color? textColor = null, Color? backgroundColor = null, Exception? ex = null);
    bool IsLogEnabled(LogLevel level);
}
