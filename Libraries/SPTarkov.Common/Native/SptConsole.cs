using System.Runtime.InteropServices;

namespace SPTarkov.Common.Native;

/// <summary>
/// The terminal, owned by spt_native: raw byte writes ordered behind the log pipeline's console
/// queue, terminal control, and stdin reads. Every method is best-effort about the native library
/// itself — an unloadable one degrades to a false return or a plain-C# fallback rather than an
/// exception, because these are the paths that report failures. Bad arguments are not covered by
/// that: a null argument throws as it would from any other call.
/// </summary>
public static class SptConsole
{
    public static bool TryWrite(byte[] bytes, bool toStdErr)
    {
        try
        {
            return NativeMethods.ConsoleWrite(bytes, (nuint)bytes.Length, toStdErr ? 1 : 0) == 0;
        }
        catch (Exception failure) when (failure is DllNotFoundException or EntryPointNotFoundException)
        {
            return false;
        }
    }

    public static void SetTitle(string title)
    {
        var titleBytes = System.Text.Encoding.UTF8.GetBytes(title);

        try
        {
            NativeMethods.ConsoleSetTitle(titleBytes, (nuint)titleBytes.Length);
        }
        catch (Exception failure) when (failure is DllNotFoundException or EntryPointNotFoundException)
        {
            // No native library, no title: the server is already running degraded.
        }
    }

    public static void Clear()
    {
        try
        {
            NativeMethods.ConsoleClear();
        }
        catch (Exception failure) when (failure is DllNotFoundException or EntryPointNotFoundException)
        {
            // Same contract as SetTitle.
        }
    }

    /// <summary>
    /// Console.ReadLine through the native side: the console queue is flushed first, so a prompt
    /// written just before is guaranteed visible. Null on EOF (and, as an accepted quirk, on an
    /// empty line — every current caller discards the value).
    /// </summary>
    public static string? ReadLine()
    {
        nint outPtr = 0;
        nuint outLen = 0;

        try
        {
            var status = NativeMethods.ConsoleReadLine(out outPtr, out outLen);

            if (status != 0 || outPtr == 0)
            {
                return null;
            }

            return Marshal.PtrToStringUTF8(outPtr, checked((int)outLen));
        }
        catch (Exception failure) when (failure is DllNotFoundException or EntryPointNotFoundException)
        {
            return Console.In.ReadLine();
        }
        finally
        {
            if (outPtr != 0)
            {
                NativeMethods.BufFree(outPtr, outLen);
            }
        }
    }
}
