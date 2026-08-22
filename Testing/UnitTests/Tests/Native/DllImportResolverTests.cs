using System.Runtime.InteropServices;
using NUnit.Framework;

namespace UnitTests.Tests.Native;

/// <summary>
/// Pins the arm of the spt_native DllImport resolver that this test run must NOT take. Since Phase 6b
/// both NativeMethods classes try NativeLibrary.GetMainProgramHandle() first and use it when
/// spt_native_abi_version is exported there, which is true only under the mpex-server launcher.
/// The test host is not that launcher, so the probe has to come back false and every P/Invoke in the
/// suite has to bind through the cdylib beside the assembly instead. If the probe ever started
/// misfiring here the failure would be a partially-silent one - the log and console paths degrade on
/// EntryPointNotFoundException rather than throwing - so assert it directly.
/// </summary>
[TestFixture]
public class DllImportResolverTests
{
    [Test]
    public void TheTestHostDoesNotExportSptNative()
    {
        var mainProgram = NativeLibrary.GetMainProgramHandle();

        Assert.That(
            NativeLibrary.TryGetExport(mainProgram, "spt_native_abi_version", out _),
            Is.False,
            "the test host is not the mpex-server launcher, so the resolver must fall through to the cdylib"
        );
    }

    [Test]
    public void TheProbeReachesTheGlobalSymbolScope()
    {
        if (!OperatingSystem.IsLinux())
        {
            Assert.Ignore("dlopen(NULL) scope semantics are the Unix half of GetMainProgramHandle");
        }

        var mainProgram = NativeLibrary.GetMainProgramHandle();

        // GetMainProgramHandle is dlopen(NULL), so it sees the executable plus everything loaded
        // into the global scope - libc among them. That is what makes the assertion above load
        // bearing rather than trivially true: the probe is wide, and still finds no spt_native.
        Assert.That(
            NativeLibrary.TryGetExport(mainProgram, "getpid", out _),
            Is.True,
            "if this stops holding, GetMainProgramHandle's scope changed and the resolver's first arm needs rechecking"
        );
    }
}
