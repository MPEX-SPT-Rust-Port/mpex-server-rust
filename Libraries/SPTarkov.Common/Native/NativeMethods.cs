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

                var fileName =
                    OperatingSystem.IsWindows() ? "spt_native.dll"
                    : OperatingSystem.IsMacOS() ? "libspt_native.dylib"
                    : "libspt_native.so";
                NativeLibrary.TryLoad(Path.Combine(AppContext.BaseDirectory, fileName), out var handle);
                return handle;
            }
        );
    }

    [LibraryImport(LibraryName, EntryPoint = "spt_log_open")]
    internal static partial int LogOpen(
        ReadOnlySpan<byte> dirUtf8,
        nuint dirLen,
        ReadOnlySpan<byte> patternUtf8,
        nuint patternLen,
        uint maxFileSizeMb,
        uint maxRollingFiles,
        out nint outHandle,
        out nint outPtr,
        out nuint outLen
    );

    [LibraryImport(LibraryName, EntryPoint = "spt_log_write")]
    internal static partial int LogWrite(nint handle, ReadOnlySpan<byte> lineUtf8, nuint lineLen);

    [LibraryImport(LibraryName, EntryPoint = "spt_log_close")]
    internal static partial int LogClose(nint handle);

    [LibraryImport(LibraryName, EntryPoint = "spt_logger_init")]
    internal static partial int LoggerInit(ReadOnlySpan<byte> configUtf8, nuint configLen, out nint outPtr, out nuint outLen);

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
}
