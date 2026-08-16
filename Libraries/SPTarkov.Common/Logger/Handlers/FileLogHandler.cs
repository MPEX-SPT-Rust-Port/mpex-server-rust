using System.Text;
using SPTarkov.Common.Models.Logging;
using SPTarkov.Common.Native;

namespace SPTarkov.Common.Logger.Handlers;

/// <summary>
/// Writes formatted lines to spt_native's file sink, which owns the file handle, the day/size
/// rolling and the retention sweep. Each configured target gets one sink, and one writer thread
/// behind it, so a log call costs a channel send rather than a filesystem write.
/// </summary>
internal sealed class FileLogHandler : BaseLogHandler
{
    // ffi.rs
    private const int StatusOk = 0;

    private readonly Lock _sinksLock = new();

    /// <summary>
    /// Native sink handles keyed by target config. A zero handle marks a target whose open failed,
    /// so the failure is reported once instead of on every line.
    /// </summary>
    private readonly Dictionary<string, nint> _sinks = [];

    /// <summary>
    /// Set under <see cref="_sinksLock"/> by <see cref="DisposeAsync"/>. Every native call happens
    /// under that lock, so a handle can never be closed out from under an in-flight write.
    /// </summary>
    private bool _disposed;

    public override LoggerType LoggerType { get; } = LoggerType.File;

    public override void Log(SptLogMessage message, BaseSptLoggerReference reference)
    {
        var config = (reference as FileSptLoggerReference)!;

        if (string.IsNullOrEmpty(config.FilePath) || string.IsNullOrEmpty(config.FilePattern))
        {
            throw new Exception("FilePath and FilePattern are required to use FileLogger");
        }

        var line = Encoding.UTF8.GetBytes(FormatMessage(message.Message, message, reference));

        lock (_sinksLock)
        {
            if (_disposed)
            {
                return;
            }

            var sink = GetOrCreateSink(config);

            if (sink == 0)
            {
                return;
            }

            unsafe
            {
                fixed (byte* linePtr = line)
                {
                    NativeMethods.LogWrite(sink, linePtr, (nuint)line.Length);
                }
            }
        }
    }

    /// <summary>
    /// Must be called under <see cref="_sinksLock"/>: opening twice would leak a native handle and
    /// leave two writer threads appending to the same file.
    /// </summary>
    private nint GetOrCreateSink(FileSptLoggerReference config)
    {
        var key = $"{config.FilePath}|{config.FilePattern}|{config.MaxFileSizeMb}|{config.MaxRollingFiles}";

        if (_sinks.TryGetValue(key, out var existingSink))
        {
            return existingSink;
        }

        var sink = OpenSink(config);
        _sinks.Add(key, sink);

        return sink;
    }

    private static unsafe nint OpenSink(FileSptLoggerReference config)
    {
        var directory = Encoding.UTF8.GetBytes(config.FilePath);
        var pattern = Encoding.UTF8.GetBytes(config.FilePattern);

        nint handle = 0;
        byte* messagePtr = null;
        nuint messageLen = 0;
        int status;

        fixed (byte* directoryPtr = directory)
        fixed (byte* patternPtr = pattern)
        {
            status = NativeMethods.LogOpen(
                directoryPtr,
                (nuint)directory.Length,
                patternPtr,
                (nuint)pattern.Length,
                (uint)config.MaxFileSizeMb,
                (uint)config.MaxRollingFiles,
                &handle,
                &messagePtr,
                &messageLen
            );
        }

        if (status == StatusOk)
        {
            return handle;
        }

        try
        {
            // The logger cannot report its own failure through itself, so this goes to stderr.
            var message = messagePtr == null ? $"internal status {status}" : Encoding.UTF8.GetString(messagePtr, checked((int)messageLen));

            Console.Error.WriteLine(
                $"Failed to open log file '{Path.Combine(config.FilePath, config.FilePattern)}': {message}. File logging for this target is disabled."
            );
        }
        finally
        {
            NativeMethods.BufFree(messagePtr, messageLen);
        }

        return 0;
    }

    public override ValueTask DisposeAsync()
    {
        lock (_sinksLock)
        {
            if (_disposed)
            {
                return ValueTask.CompletedTask;
            }

            _disposed = true;

            foreach (var sink in _sinks.Values)
            {
                if (sink != 0)
                {
                    NativeMethods.LogClose(sink);
                }
            }
        }

        GC.SuppressFinalize(this);

        return ValueTask.CompletedTask;
    }
}
