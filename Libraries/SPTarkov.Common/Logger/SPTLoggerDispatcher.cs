using System.Runtime.InteropServices;
using System.Text;
using System.Text.Json;
using SPTarkov.Common.Models.Logging;
using SPTarkov.Common.Native;

namespace SPTarkov.Common.Logger;

/// <summary>
/// Shim over the native log pipeline: builds the flat spt_log_emit call from an SptLogMessage.
/// Filtering, level gating, formatting and the console/file writers live in spt_native;
/// spt_logger_init has already run from AddSptLogger. Mod-registered ILogHandlers are routed to
/// again (restored 4.1.2 behaviour): C#-originated lines fan out here with their original
/// Exception object, Rust-originated lines arrive through the spt_log_set_tap callback.
/// <para>
/// A mod resolves this singleton from DI and calls RegisterHandler — AddSptLogger builds the
/// dispatcher from its own service collection, so an ILogHandler registered in the host container
/// is never constructor-injected. Routing is per configuration reference (exclude filters, then
/// the include gate, then the level), and a reference only reaches handlers whose LoggerType
/// matches it.
/// </para>
/// <para>
/// Dispose clears the tap, but a native callback already in flight can still deliver to a handler
/// during or after disposal; the fan-out's per-handler catch contains whatever that handler then
/// throws.
/// </para>
/// </summary>
public sealed class SPTLoggerDispatcher : IAsyncDisposable
{
    // ffi.rs
    private const int StatusOk = 0;

    /// <summary>
    /// The dispatcher the native tap fans out through. Static because the callback crosses the
    /// FFI boundary without instance state; the library is loaded once per process, so there is
    /// one tap slot — last registered wins, cleared on dispose.
    /// </summary>
    private static SPTLoggerDispatcher? _tapTarget;

    /// <summary>
    /// Rooted for the process lifetime: the native side holds the function pointer produced
    /// from this delegate, so it must never be collected.
    /// </summary>
    private static readonly NativeTapDelegate TapCallback = OnNativeLine;

    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    private delegate void NativeTapDelegate(
        nint categoryPtr,
        nuint categoryLen,
        nint messagePtr,
        nuint messageLen,
        nint threadNamePtr,
        nuint threadNameLen,
        int level,
        int tid,
        long unixMillis
    );

    [ThreadStatic]
    private static bool _inHandlerFanOut;

    private readonly SptLoggerConfiguration _config;

    /// <summary>
    /// Copy-on-write under _handlerLock: RegisterHandler swaps a new array in, readers snapshot the
    /// reference into a local and iterate that, so a registration during a fan-out cannot tear.
    /// </summary>
    private readonly Lock _handlerLock = new();
    private ILogHandler[] _logHandlers;
    private bool _tapRegistered;

    /// <summary>
    /// Set when the native side fails: a panic status, or an unloadable library. Reported once to
    /// stderr; after that every native emit is a no-op instead of a per-line error storm. Handler
    /// fan-out is C#-only and keeps working. The unsynchronised read is deliberate — the worst
    /// case is one extra report per racing thread.
    /// </summary>
    private bool _disabled;

    public SPTLoggerDispatcher(SptLoggerConfiguration config, IEnumerable<ILogHandler> logHandlers)
    {
        _config = config;
        _logHandlers = logHandlers.ToArray();

        if (_logHandlers.Length > 0)
        {
            EnsureTapRegistered();
        }
    }

    /// <summary>
    /// Adds a handler to the fan-out, and arms the native tap if this is the first one. The
    /// mod-facing entry point: resolve this dispatcher from DI and call this from your mod's
    /// startup, because AddSptLogger constructs the dispatcher before any mod service exists.
    /// </summary>
    public void RegisterHandler(ILogHandler handler)
    {
        ArgumentNullException.ThrowIfNull(handler);

        lock (_handlerLock)
        {
            _logHandlers = [.. _logHandlers, handler];
        }

        EnsureTapRegistered();
    }

    /// <summary>
    /// Idempotent: the native tap slot is process-wide, so re-registering the same callback is
    /// harmless, and an unloadable library leaves the C# fan-out working on its own.
    /// </summary>
    private void EnsureTapRegistered()
    {
        // Set first: the tap can fire from another thread the instant LogSetTap returns, and a
        // null target would drop that line.
        _tapTarget = this;

        try
        {
            NativeMethods.LogSetTap(Marshal.GetFunctionPointerForDelegate(TapCallback));
            _tapRegistered = true;
        }
        catch (Exception failure) when (failure is DllNotFoundException or EntryPointNotFoundException)
        {
            // No native library means no native-origin lines; don't root this instance for nothing.
            _tapTarget = null;
        }
    }

    public bool IsLogEnabled(LogLevel level)
    {
        try
        {
            var enabled = NativeMethods.LogEnabled((int)level);

            if (enabled >= 0)
            {
                return enabled != 0;
            }
        }
        catch (Exception failure) when (failure is DllNotFoundException or EntryPointNotFoundException)
        {
            // Fall through: no library, same answer source as no pipeline.
        }

        // No applied pipeline to ask (before init, after close, unloadable library): the C#
        // configuration object still gates handler-only fan-out.
        return _config.Loggers.Any(logger => logger.LogLevel.CanLog(level));
    }

    public void Log(SptLogMessage message)
    {
        FanOutToHandlers(message);

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

    /// <summary>
    /// Re-hands the current configuration object — including any runtime mutation a mod made to
    /// Loggers — to the native pipeline, so what is written matches what IsLogEnabled reads again.
    /// On success the failure latch resets; on failure (native rejects the config, or the library
    /// is unloadable) one stderr notice, false, and the running pipeline is unchanged.
    /// </summary>
    public bool ReloadConfiguration()
    {
        var configBytes = JsonSerializer.SerializeToUtf8Bytes(_config);
        nint messagePtr = 0;
        nuint messageLen = 0;

        try
        {
            var status = NativeMethods.LoggerReinit(configBytes, (nuint)configBytes.Length, out messagePtr, out messageLen);

            if (status != StatusOk)
            {
                var message = messagePtr == 0 ? $"internal status {status}" : Marshal.PtrToStringUTF8(messagePtr, checked((int)messageLen));

                Console.Error.WriteLine($"Failed to reload the native log pipeline configuration: {message}.");

                return false;
            }

            _disabled = false;

            return true;
        }
        catch (Exception failure) when (failure is DllNotFoundException or EntryPointNotFoundException)
        {
            Console.Error.WriteLine(
                $"Failed to load spt_native for logging: {failure.Message}. "
                    + "Rebuild the native library (dotnet build runs cargo automatically)."
            );

            return false;
        }
        finally
        {
            // messagePtr stays 0 on the unloadable-library path, so this cannot re-throw there.
            if (messagePtr != 0)
            {
                NativeMethods.BufFree(messagePtr, messageLen);
            }
        }
    }

    /// <summary>
    /// The pre-af58d5f routing loop, restored: per config reference — exclude filters, include
    /// gate, level — each handler of the reference's type receives the message. A throwing
    /// handler is skipped, and a handler that logs re-entrantly skips nested fan-out (its line
    /// still reaches the native sinks) — logging must never recurse or die on a mod's account.
    /// </summary>
    private void FanOutToHandlers(SptLogMessage message)
    {
        var handlers = _logHandlers;

        if (handlers.Length == 0 || _inHandlerFanOut)
        {
            return;
        }

        _inHandlerFanOut = true;

        try
        {
            foreach (var reference in _config.Loggers)
            {
                if (!ShouldRoute(reference, message))
                {
                    continue;
                }

                foreach (var handler in handlers)
                {
                    if (handler.LoggerType != reference.Type)
                    {
                        continue;
                    }

                    try
                    {
                        handler.Log(message, reference);
                    }
                    catch
                    {
                        // A broken mod handler must not take logging down.
                    }
                }
            }
        }
        finally
        {
            _inHandlerFanOut = false;
        }
    }

    /// <summary>
    /// The per-reference decision, same contract as logger.rs should_emit: any exclude match
    /// drops, if includes exist at least one must match, then the level gate.
    /// </summary>
    private static bool ShouldRoute(BaseSptLoggerReference reference, SptLogMessage message)
    {
        var hasInclude = false;
        var includeMatched = false;

        foreach (var filter in reference.Filters)
        {
            if (filter.Type == SptLoggerFilterType.Exclude)
            {
                if (filter.Match(message))
                {
                    return false;
                }
            }
            else
            {
                hasInclude = true;
                includeMatched = includeMatched || filter.Match(message);
            }
        }

        if (hasInclude && !includeMatched)
        {
            return false;
        }

        return reference.LogLevel.CanLog(message.LogLevel);
    }

    /// <summary>
    /// The native tap: Rust-originated lines (generator diagnostics) re-enter here so handlers
    /// see the full stream. The text is already rendered — there is no Exception object and the
    /// tid is the native emitter's counter, accepted divergences both. Nothing may unwind across
    /// the FFI boundary into Rust.
    /// </summary>
    private static void OnNativeLine(
        nint categoryPtr,
        nuint categoryLen,
        nint messagePtr,
        nuint messageLen,
        nint threadNamePtr,
        nuint threadNameLen,
        int level,
        int tid,
        long unixMillis
    )
    {
        try
        {
            var target = _tapTarget;

            if (target == null)
            {
                return;
            }

            var message = new SptLogMessage(
                categoryPtr == 0 ? string.Empty : Marshal.PtrToStringUTF8(categoryPtr, checked((int)categoryLen)),
                DateTime.UnixEpoch.AddMilliseconds(unixMillis),
                (LogLevel)level,
                tid,
                threadNamePtr == 0 ? null : Marshal.PtrToStringUTF8(threadNamePtr, checked((int)threadNameLen)),
                messagePtr == 0 ? string.Empty : Marshal.PtrToStringUTF8(messagePtr, checked((int)messageLen))
            );

            target.FanOutToHandlers(message);
        }
        catch
        {
            // Swallow everything: an exception crossing into Rust would abort the process.
        }
    }

    public async ValueTask DisposeAsync()
    {
        // Only the instance that currently owns the process-wide slot may clear it; a dispatcher
        // registered after this one must keep receiving native lines.
        if (_tapRegistered && ReferenceEquals(_tapTarget, this))
        {
            _tapTarget = null;

            NativeMethods.LogSetTap(0);
        }

        try
        {
            NativeMethods.LoggerClose();
        }
        catch (Exception failure) when (failure is DllNotFoundException or EntryPointNotFoundException)
        {
            // Never loaded, so there is nothing to flush.
        }

        foreach (var handler in _logHandlers)
        {
            await handler.DisposeAsync();
        }
    }
}
