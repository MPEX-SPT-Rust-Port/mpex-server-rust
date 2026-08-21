using System.Runtime.InteropServices;

namespace SPTarkov.Common.Native;

/// <summary>
/// The log sink exports of spt_native. A deliberate twin of the class of the same name in
/// SPTarkov.Server.Core: SetDllImportResolver registers per assembly, so Core's registration never
/// covers imports declared here. Keep the resolver bodies identical.
/// </summary>
internal static partial class NativeMethods
{
    private const string LibraryName = "spt_native";

    // When a prepatcher mod is installed, PrepatchLoadContext loads the rewritten Core via
    // LoadFromStream, so the assembly has no location and default P/Invoke probing never checks
    // the app directory where libspt_native lives. Probe it explicitly; returning IntPtr.Zero
    // falls back to the default search.
    static NativeMethods()
    {
        NativeLibrary.SetDllImportResolver(
            typeof(NativeMethods).Assembly,
            (name, _, _) =>
            {
                if (name != LibraryName)
                {
                    return IntPtr.Zero;
                }

                // SPIKE: mpex-server links spt-native as an rlib and re-exports the symbols from
                // the executable itself, so the resident DB's statics live in the host process.
                // A process started any other way (SPT.Server, dotnet test) has no such exports
                // and falls through to the cdylib beside the assembly.
                var mainProgram = NativeLibrary.GetMainProgramHandle();
                if (NativeLibrary.TryGetExport(mainProgram, "spt_native_abi_version", out _))
                {
                    return mainProgram;
                }

                var fileName =
                    OperatingSystem.IsWindows() ? "spt_native.dll"
                    : OperatingSystem.IsMacOS() ? "libspt_native.dylib"
                    : "libspt_native.so";
                NativeLibrary.TryLoad(Path.Combine(AppContext.BaseDirectory, fileName), out var handle);
                return handle;
            }
        );
    }

    [LibraryImport(LibraryName, EntryPoint = "spt_logger_init")]
    internal static partial int LoggerInit(ReadOnlySpan<byte> configUtf8, nuint configLen, out nint outPtr, out nuint outLen);

    [LibraryImport(LibraryName, EntryPoint = "spt_logger_reinit")]
    internal static partial int LoggerReinit(ReadOnlySpan<byte> configUtf8, nuint configLen, out nint outPtr, out nuint outLen);

    [LibraryImport(LibraryName, EntryPoint = "spt_log_set_tap")]
    internal static partial int LogSetTap(nint tapPtr);

    [LibraryImport(LibraryName, EntryPoint = "spt_log_emit")]
    internal static partial int LogEmit(
        ReadOnlySpan<byte> categoryUtf8,
        nuint categoryLen,
        ReadOnlySpan<byte> messageUtf8,
        nuint messageLen,
        ReadOnlySpan<byte> exceptionUtf8,
        nuint exceptionLen,
        ReadOnlySpan<byte> threadNameUtf8,
        nuint threadNameLen,
        int level,
        int threadId,
        long unixMillis
    );

    [LibraryImport(LibraryName, EntryPoint = "spt_logger_close")]
    internal static partial int LoggerClose();

    [LibraryImport(LibraryName, EntryPoint = "spt_buf_free")]
    internal static partial void BufFree(nint ptr, nuint len);

    [LibraryImport(LibraryName, EntryPoint = "spt_console_write")]
    internal static partial int ConsoleWrite(ReadOnlySpan<byte> bytes, nuint bytesLen, int toStdErr);

    [LibraryImport(LibraryName, EntryPoint = "spt_console_read_line")]
    internal static partial int ConsoleReadLine(out nint outPtr, out nuint outLen);

    [LibraryImport(LibraryName, EntryPoint = "spt_console_set_title")]
    internal static partial int ConsoleSetTitle(ReadOnlySpan<byte> titleUtf8, nuint titleLen);

    [LibraryImport(LibraryName, EntryPoint = "spt_console_clear")]
    internal static partial int ConsoleClear();

    // Tri-state, not a status code: 1 enabled, 0 disabled, -1 no pipeline (fall back to the C#
    // configuration object).
    [LibraryImport(LibraryName, EntryPoint = "spt_log_enabled")]
    internal static partial int LogEnabled(int level);

    [LibraryImport(LibraryName, EntryPoint = "spt_log_format")]
    internal static partial int LogFormat(
        ReadOnlySpan<byte> formatUtf8,
        nuint formatLen,
        ReadOnlySpan<byte> messageUtf8,
        nuint messageLen,
        ReadOnlySpan<byte> loggerUtf8,
        nuint loggerLen,
        ReadOnlySpan<byte> threadNameUtf8,
        nuint threadNameLen,
        int level,
        int threadId,
        long unixMillis,
        out nint outPtr,
        out nuint outLen
    );
}
