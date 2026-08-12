using System.Runtime.InteropServices;

namespace SPTarkov.Server.Core.Native;

internal static unsafe partial class NativeMethods
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

    [LibraryImport(LibraryName, EntryPoint = "spt_native_abi_version")]
    internal static partial uint AbiVersion();

    [LibraryImport(LibraryName, EntryPoint = "spt_verify_database")]
    internal static partial int VerifyDatabase(byte* dirUtf8, nuint dirLen, byte** outPtr, nuint* outLen);

    [LibraryImport(LibraryName, EntryPoint = "spt_generate_static_containers")]
    internal static partial int GenerateStaticContainers(byte* requestUtf8, nuint requestLen, byte** outPtr, nuint* outLen);

    [LibraryImport(LibraryName, EntryPoint = "spt_generate_dynamic_loot")]
    internal static partial int GenerateDynamicLoot(byte* requestUtf8, nuint requestLen, byte** outPtr, nuint* outLen);

    [LibraryImport(LibraryName, EntryPoint = "spt_buf_free")]
    internal static partial void BufFree(byte* ptr, nuint len);
}
