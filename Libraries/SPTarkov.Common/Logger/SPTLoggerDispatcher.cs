using System.Text;
using SPTarkov.Common.Models.Logging;
using SPTarkov.Common.Native;

namespace SPTarkov.Common.Logger;

/// <summary>
/// Shim over the native log pipeline: builds the flat spt_log_emit call from an SptLogMessage.
/// Filtering, level gating, formatting and the console/file writers all live in spt_native;
/// spt_logger_init has already run from AddSptLogger. The ILogHandler parameter is retained for
/// the frozen 4.1.2 surface — handlers are still disposed, but messages no longer route to them.
/// </summary>
public sealed class SPTLoggerDispatcher(SptLoggerConfiguration config, IEnumerable<ILogHandler> logHandlers) : IAsyncDisposable
{
    // ffi.rs
    private const int StatusOk = 0;

    /// <summary>
    /// Set when the native side fails: a panic status, or an unloadable library. Reported once to
    /// stderr; after that every Log call is a no-op instead of a per-line error storm. The unsynchronised
    /// read is deliberate — the worst case is one extra report per racing thread.
    /// </summary>
    private bool _disabled;

    public bool IsLogEnabled(LogLevel level)
    {
        return config.Loggers.Any(logger => logger.LogLevel.CanLog(level));
    }

    public void Log(SptLogMessage message)
    {
        if (_disabled)
        {
            return;
        }

        var category = Encoding.UTF8.GetBytes(message.Logger ?? string.Empty);
        var body = Encoding.UTF8.GetBytes(message.Message ?? string.Empty);
        var exception =
            message.Exception == null ? [] : Encoding.UTF8.GetBytes($"{message.Exception.Message}\n{message.Exception.StackTrace}");
        var threadName = message.threadName == null ? [] : Encoding.UTF8.GetBytes(message.threadName);

        // LogTime is DateTime.UtcNow throughout the server; render it as-is, exactly as the C#
        // handlers' LogTime.ToString did.
        var unixMillis = (long)(message.LogTime - DateTime.UnixEpoch).TotalMilliseconds;

        int status;

        try
        {
            status = NativeMethods.LogEmit(
                category,
                (nuint)category.Length,
                body,
                (nuint)body.Length,
                exception,
                (nuint)exception.Length,
                threadName,
                (nuint)threadName.Length,
                (int)message.LogLevel,
                message.threadId,
                unixMillis
            );
        }
        catch (Exception failure) when (failure is DllNotFoundException or EntryPointNotFoundException)
        {
            _disabled = true;

            Console.Error.WriteLine(
                $"Failed to load spt_native for logging: {failure.Message}. "
                    + "Rebuild the native library (dotnet build runs cargo automatically). Logging is disabled."
            );

            return;
        }

        if (status != StatusOk)
        {
            // A panic crossing the FFI boundary poisons the pipeline; disable it rather than
            // calling into it again on every line.
            _disabled = true;

            Console.Error.WriteLine("The native log pipeline failed; logging is disabled.");
        }
    }

    public async ValueTask DisposeAsync()
    {
        try
        {
            NativeMethods.LoggerClose();
        }
        catch (Exception failure) when (failure is DllNotFoundException or EntryPointNotFoundException)
        {
            // Never loaded, so there is nothing to flush.
        }

        foreach (var handler in logHandlers)
        {
            await handler.DisposeAsync();
        }
    }
}
