using System.Text;
using SPTarkov.Common.Native;

namespace SPTarkov.Common.Logger;

/// <summary>
/// The Console.Out/Error replacement that makes spt_native the process's only terminal writer:
/// every raw Console.Write* — PatchManager, DI failures, mod code, future call sites — forwards
/// as UTF-8 bytes to spt_console_write, where stdout bytes queue behind the log pipeline's
/// console sink (ordered against log lines, never dropped) and stderr bytes write directly.
/// When the native side declines (before init, after close, unloadable library), the write falls
/// back to the captured original writer — which is exactly the degraded path the dispatcher's
/// stderr fallbacks-of-last-resort need.
/// <para>
/// Every overload that carries a whole string is overridden to forward it as one message: the
/// TextWriter base decomposes to per-char writes, which would interleave fragments between queued
/// log lines. Console.SetOut wraps this in a synchronized writer, so calls arrive serialized.
/// </para>
/// </summary>
public sealed class NativeConsoleWriter : TextWriter
{
    private readonly TextWriter _fallback;
    private readonly bool _toStdErr;
    private readonly Func<byte[], bool, bool> _forward;

    public override Encoding Encoding
    {
        get { return Encoding.UTF8; }
    }

    public NativeConsoleWriter(TextWriter fallback, bool toStdErr)
        : this(fallback, toStdErr, SptConsole.TryWrite) { }

    internal NativeConsoleWriter(TextWriter fallback, bool toStdErr, Func<byte[], bool, bool> forward)
    {
        _fallback = fallback;
        _toStdErr = toStdErr;
        _forward = forward;
    }

    /// <summary>
    /// Replaces Console.Out and Console.Error with forwarding writers, once per process — a second
    /// wrapper would forward every write twice.
    /// <para>
    /// The installed writer is unrecognisable from the outside: Console.SetOut hands back a
    /// TextWriter.Synchronized wrapper, so Console.Out reports SyncTextWriter and never this type.
    /// The marker therefore lives in AppContext, keyed by this type's name rather than by type
    /// identity — the prepatcher's nested Program.Main runs this class from an isolated load
    /// context, where typeof comparison would see two distinct types for the same logical class,
    /// while AppContext and Console both live in the always-shared CoreLib. Storing the writer we
    /// installed (instead of a bare flag) keeps the marker honest: restore Console.Out and the next
    /// Install re-installs rather than silently no-opping.
    /// </para>
    /// </summary>
    public static void Install()
    {
        var marker = typeof(NativeConsoleWriter).FullName!;

        if (ReferenceEquals(AppContext.GetData(marker), Console.Out))
        {
            return;
        }

        Console.SetOut(new NativeConsoleWriter(Console.Out, toStdErr: false));
        Console.SetError(new NativeConsoleWriter(Console.Error, toStdErr: true));
        AppContext.SetData(marker, Console.Out);
    }

    private void Forward(string? value)
    {
        if (string.IsNullOrEmpty(value))
        {
            return;
        }

        if (!_forward(Encoding.UTF8.GetBytes(value), _toStdErr))
        {
            _fallback.Write(value);
        }
    }

    public override void Write(char value)
    {
        Forward(value.ToString());
    }

    public override void Write(string? value)
    {
        Forward(value);
    }

    public override void Write(char[] buffer, int index, int count)
    {
        Forward(new string(buffer, index, count));
    }

    public override void Write(ReadOnlySpan<char> buffer)
    {
        Forward(new string(buffer));
    }

    public override void WriteLine()
    {
        Forward(Environment.NewLine);
    }

    public override void WriteLine(string? value)
    {
        Forward(value + Environment.NewLine);
    }

    public override void WriteLine(ReadOnlySpan<char> buffer)
    {
        Forward(string.Concat(buffer, Environment.NewLine));
    }

    public override void Flush()
    {
        _fallback.Flush();
    }
}
