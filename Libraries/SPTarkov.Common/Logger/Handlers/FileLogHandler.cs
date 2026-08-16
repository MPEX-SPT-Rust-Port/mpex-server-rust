using System.Runtime.InteropServices;
using System.Text;
using SPTarkov.Common.Models.Logging;
using SPTarkov.Common.Native;

namespace SPTarkov.Common.Logger.Handlers;

/// <summary>
/// Writes formatted lines to spt_native's file sink, which owns the file handle, the day/size
/// rolling and the archive cap in one place. Each configured target gets one sink, and one writer
/// thread behind it.
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

            if (NativeMethods.LogWrite(sink, line, (nuint)line.Length) != StatusOk)
            {
                // A panic crossing the FFI boundary poisons the sink; disable it rather
                // than calling into it again on every line.
                NativeMethods.LogClose(sink);
                _sinks[SinkKey(config)] = 0;

                Console.Error.WriteLine(
                    $"The native log sink for '{Path.Combine(config.FilePath, config.FilePattern)}' failed. "
                        + "File logging for this target is disabled."
                );
            }
        }
    }

    /// <summary>
    /// Must be called under <see cref="_sinksLock"/>: opening twice would leak a native handle and
    /// leave two writer threads appending to the same file.
    /// </summary>
    private nint GetOrCreateSink(FileSptLoggerReference config)
    {
        var key = SinkKey(config);

        if (_sinks.TryGetValue(key, out var existingSink))
        {
            return existingSink;
        }

        var sink = OpenSink(config);
        _sinks.Add(key, sink);

        return sink;
    }

    private static string SinkKey(FileSptLoggerReference config)
    {
        return $"{config.FilePath}|{config.FilePattern}|{config.MaxFileSizeMb}|{config.MaxRollingFiles}";
    }

    private static nint OpenSink(FileSptLoggerReference config)
    {
        var directory = Encoding.UTF8.GetBytes(config.FilePath);
        var pattern = Encoding.UTF8.GetBytes(config.FilePattern);

        // Initialised rather than left to the out params: the catch below returns before they are
        // assigned, and initialisers keep definite-assignment analysis out of the question.
        nint handle = 0;
        nint messagePtr = 0;
        nuint messageLen = 0;
        int status;

        try
        {
            status = NativeMethods.LogOpen(
                directory,
                (nuint)directory.Length,
                pattern,
                (nuint)pattern.Length,
                (uint)config.MaxFileSizeMb,
                (uint)config.MaxRollingFiles,
                out handle,
                out messagePtr,
                out messageLen
            );
        }
        catch (Exception exception) when (exception is DllNotFoundException or EntryPointNotFoundException)
        {
            // The file logger makes the process's first native call, well before
            // SptNative.EnsureLoadable() gets to report an ABI mismatch, so a missing or stale
            // library surfaces here first. Without this it throws out of the dispatcher on every
            // single log line.
            Console.Error.WriteLine(
                $"Failed to load spt_native for log file '{Path.Combine(config.FilePath, config.FilePattern)}': "
                    + $"{exception.Message}. Rebuild the native library (dotnet build runs cargo automatically). "
                    + "File logging for this target is disabled."
            );

            return 0;
        }

        if (status == StatusOk)
        {
            return handle;
        }

        try
        {
            // The logger cannot report its own failure through itself, so this goes to stderr.
            // Marshal.PtrToStringUTF8 is the safe read here - a ReadOnlySpan<byte> over a raw
            // pointer would need the unsafe flag this task exists to remove.
            var message = messagePtr == 0 ? $"internal status {status}" : Marshal.PtrToStringUTF8(messagePtr, checked((int)messageLen));

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

        return ValueTask.CompletedTask;
    }
}
