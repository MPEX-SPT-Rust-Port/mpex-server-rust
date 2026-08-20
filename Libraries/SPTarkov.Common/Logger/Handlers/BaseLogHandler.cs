using System.Runtime.InteropServices;
using System.Text;
using SPTarkov.Common.Models.Logging;
using SPTarkov.Common.Native;

namespace SPTarkov.Common.Logger.Handlers;

public abstract class BaseLogHandler : ILogHandler
{
    public abstract LoggerType LoggerType { get; }

    public abstract void Log(SptLogMessage message, BaseSptLoggerReference reference);

    protected string FormatMessage(string processedMessage, SptLogMessage message, BaseSptLoggerReference reference)
    {
        var body = processedMessage ?? string.Empty;
        var formattedMessage = FormatNative(reference.Format, body, message) ?? body;

        if (message.Exception != null)
        {
            return string.Concat(formattedMessage, "\n", message.Exception.Message, "\n", message.Exception.StackTrace);
        }

        return formattedMessage;
    }

    /// <summary>
    /// The pipeline's own token expansion (spt_log_format), replacing the CompositeFormat
    /// re-implementation this class used to carry. Null when the native side is unavailable —
    /// the caller then degrades to the unformatted message rather than throwing, because handler
    /// fan-out must survive a broken pipeline.
    /// </summary>
    private static string? FormatNative(string format, string body, SptLogMessage message)
    {
        var formatBytes = Encoding.UTF8.GetBytes(format);
        var messageBytes = Encoding.UTF8.GetBytes(body);
        var loggerBytes = Encoding.UTF8.GetBytes(message.Logger ?? string.Empty);
        var threadNameBytes = message.threadName == null ? [] : Encoding.UTF8.GetBytes(message.threadName);
        var unixMillis = (long)(message.LogTime - DateTime.UnixEpoch).TotalMilliseconds;
        nint outPtr = 0;
        nuint outLen = 0;

        try
        {
            var status = NativeMethods.LogFormat(
                formatBytes,
                (nuint)formatBytes.Length,
                messageBytes,
                (nuint)messageBytes.Length,
                loggerBytes,
                (nuint)loggerBytes.Length,
                threadNameBytes,
                (nuint)threadNameBytes.Length,
                (int)message.LogLevel,
                message.threadId,
                unixMillis,
                out outPtr,
                out outLen
            );

            if (status != 0)
            {
                return null;
            }

            return outPtr == 0 ? string.Empty : Marshal.PtrToStringUTF8(outPtr, checked((int)outLen));
        }
        catch (Exception failure) when (failure is DllNotFoundException or EntryPointNotFoundException)
        {
            return null;
        }
        finally
        {
            if (outPtr != 0)
            {
                NativeMethods.BufFree(outPtr, outLen);
            }
        }
    }

    /// <summary>
    /// Retained for 4.1.2 mod compatibility and never called in-tree: %loggerShort% is expanded by
    /// spt_log_format now.
    /// </summary>
    protected string GetLoggerShortName(string logger)
    {
        var lastDotIndex = logger.AsSpan().LastIndexOf('.');
        return lastDotIndex >= 0 ? logger.Substring(lastDotIndex + 1) : logger;
    }

    public abstract ValueTask DisposeAsync();
}
