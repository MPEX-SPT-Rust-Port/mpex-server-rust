using System.Diagnostics;
using System.IO.Hashing;
using System.Text;
using System.Text.Json;
using NUnit.Framework;
using SPTarkov.Server.Core.Native;
using SPTarkov.Server.Core.Services.Server;
using SPTarkov.Server.Core.Utils;
using SPTarkov.Server.Helpers;

namespace UnitTests.Tests.Helpers;

/// <summary>
/// What the Phase 3 flip bought at startup: both <see cref="DatabaseImporter"/>
/// arms, driven here directly so each half is timed on its own. Legacy hashes the tree, then walks it
/// a second time off disk; native does one native walk that hashes, reads and installs the resident
/// roots, and the managed walk materializes from its buffers. Run in Release; the cargo dev profile
/// makes Debug numbers meaningless.
/// </summary>
[TestFixture]
[Explicit("Phase 3 import benchmark: run by hand in Release")]
[NonParallelizable]
public class DatabaseImportBenchmarkTests
{
    private const string SptDataPath = "./SPT_Data/";
    private const int WarmRuns = 3;

    private ImporterUtil _importerUtil = default!;
    private string _checksDatPath = string.Empty;
    private bool _generatedChecksDat;

    /// One arm's run: its verify/read half, its materialize half, and their sum.
    private readonly record struct Run(double Hash, double Materialize)
    {
        public double Total
        {
            get { return Hash + Materialize; }
        }
    }

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        _importerUtil = DI.GetInstance().GetService<ImporterUtil>();
        _checksDatPath = Path.Combine(SptDataPath, "checks.dat");

        // A Release build already writes this one (PreBuildHashFile -> gen_checks) and copies it
        // beside the SPT_Data the test host runs against, so the timed verify hashes exactly what
        // production hashes. Build one only when it is absent - a Debug smoke run of this fixture -
        // and delete only what was built here.
        _generatedChecksDat = !File.Exists(_checksDatPath);

        if (_generatedChecksDat)
        {
            WriteChecksDat();
        }
    }

    [OneTimeTearDown]
    public void OneTimeTearDown()
    {
        // Unconditional: a mid-run failure must not strand a hand-built manifest that a later
        // DatabaseImporter run would then hash against.
        if (_generatedChecksDat)
        {
            File.Delete(_checksDatPath);
        }

        // Every native run reinstalls the resident roots straight off disk, behind DbPublisher's
        // bookkeeping, so what is resident no longer reflects post-OnLoad state. Move the stamp and
        // the next EnsureCurrent() republishes for whichever fixture runs after this one.
        DI.GetInstance().GetService<DatabaseMutationStamp>().Bump();
    }

    [Test]
    public async Task FusedNativeLoadVersusLegacyImport()
    {
        var legacy = await MeasureLegacy();
        var (native, bufferFiles, bufferBytes) = await MeasureNative();

        TestContext.Out.WriteLine($"{"measure", -34} {"cold ms", 9} {"warm median ms", 15}");
        Report("legacy verify", legacy, run => run.Hash);
        Report("legacy import (disk walk)", legacy, run => run.Materialize);
        Report("legacy total", legacy, run => run.Total);
        Report("native fused load (verify+read)", native, run => run.Hash);
        Report("native replica materialize", native, run => run.Materialize);
        Report("native total", native, run => run.Total);

        TestContext.Out.WriteLine(
            $"returned buffers: {bufferFiles} files, {bufferBytes / 1048576.0:F1} MiB"
                + $"{Environment.NewLine}speedup (warm median legacy total / native total): "
                + $"{WarmMedian(legacy, run => run.Total) / WarmMedian(native, run => run.Total):F2}x"
        );

        Assert.That(
            bufferFiles,
            Is.GreaterThan(0),
            "the fused load returned no buffers - the native arm degenerated into a second legacy arm"
        );
    }

    /// <summary>
    /// The <c>ForceLegacyDatabaseImport</c> arm: a verifying pass over the tree, then the disk-only
    /// reflection walk that reads every eager file a second time.
    /// </summary>
    private async Task<Run[]> MeasureLegacy()
    {
        var runs = new Run[WarmRuns + 1];

        for (var run = 0; run < runs.Length; run++)
        {
            Settle();

            var stopwatch = Stopwatch.StartNew();
            var verify = await SptNative.VerifyDatabaseAsync(SptDataPath);
            var verifyMs = stopwatch.Elapsed.TotalMilliseconds;

            Assert.That(verify.Ok, Is.True, $"legacy verify failed: {verify.Failures.FirstOrDefault()?.Path}");

            stopwatch.Restart();
            _ = await _importerUtil.LoadRecursiveAsync<DatabaseTables>($"{SptDataPath}database/");

            runs[run] = new Run(verifyMs, stopwatch.Elapsed.TotalMilliseconds);
        }

        return runs;
    }

    /// <summary>
    /// The shipped arm: one native walk hashes, reads and installs the resident roots, then the same
    /// reflection walk materializes the managed replica out of the returned buffers.
    /// </summary>
    private async Task<(Run[] Runs, int Files, long Bytes)> MeasureNative()
    {
        var runs = new Run[WarmRuns + 1];
        var files = 0;
        long bytes = 0;

        for (var run = 0; run < runs.Length; run++)
        {
            Settle();

            var stopwatch = Stopwatch.StartNew();
            var load = SptNative.DbLoad(SptDataPath, verify: true);
            var loadMs = stopwatch.Elapsed.TotalMilliseconds;

            Assert.That(load.Verify?.Ok, Is.True, $"fused load verify failed: {load.Verify?.Failures.FirstOrDefault()?.Path}");

            files = load.Files.Count;
            bytes = load.Files.Values.Sum(buffer => (long)buffer.Length);

            stopwatch.Restart();
            _ = await _importerUtil.LoadRecursiveAsync<DatabaseTables>($"{SptDataPath}database/", load.Files);

            runs[run] = new Run(loadMs, stopwatch.Elapsed.TotalMilliseconds);
        }

        return (runs, files, bytes);
    }

    /// <summary>
    /// One full collection between runs, outside every stopwatch. The house convention is one before
    /// the timed phase and none inside it, but a run here drops a whole <see cref="DatabaseTables"/>
    /// - orders of magnitude more garbage per run than the per-call fixtures - so leaving it to fall
    /// due inside a later run's timing would be the larger distortion.
    /// </summary>
    private static void Settle()
    {
        GC.Collect();
        GC.WaitForPendingFinalizers();
        GC.Collect();
    }

    private static void Report(string label, Run[] runs, Func<Run, double> phase)
    {
        TestContext.Out.WriteLine($"{label, -34} {phase(runs[0]), 9:F1} {WarmMedian(runs, phase), 15:F1}");
    }

    /// Run 0 is the cold pass; the median is over the warm ones behind it.
    private static double WarmMedian(Run[] runs, Func<Run, double> phase)
    {
        var sorted = runs.Skip(1).Select(phase).Order().ToArray();

        return sorted.Length % 2 == 1 ? sorted[sorted.Length / 2] : (sorted[sorted.Length / 2 - 1] + sorted[sorted.Length / 2]) / 2;
    }

    /// <summary>
    /// Mirrors <c>verify::generate</c> (rust/spt-native/src/verify.rs): XxHash128 over every file
    /// under SPT_Data except <c>images/</c> and <c>checks.dat</c>, sorted by path, as base64 JSON.
    /// Only reachable when the build supplied no manifest, so it may also name build artifacts the
    /// source tree has no copy of (<c>wwwroot/</c>) - consistent in both directions, but a wider
    /// verify workload than the Release figures were taken against.
    /// </summary>
    private void WriteChecksDat()
    {
        var root = Path.GetFullPath(SptDataPath);
        var images = Path.Combine(root, "images") + Path.DirectorySeparatorChar;

        var entries = Directory
            .EnumerateFiles(root, "*", SearchOption.AllDirectories)
            .Where(path => !path.StartsWith(images, StringComparison.Ordinal))
            .Where(path => !Path.GetFileName(path).Equals("checks.dat", StringComparison.OrdinalIgnoreCase))
            .Select(path => new
            {
                Path = Path.GetRelativePath(root, path).Replace('\\', '/'),
                Hash = Convert.ToHexString(XxHash128.Hash(File.ReadAllBytes(path))),
            })
            .OrderBy(entry => entry.Path, StringComparer.Ordinal)
            .ToList();

        File.WriteAllText(
            _checksDatPath,
            Convert.ToBase64String(Encoding.UTF8.GetBytes(JsonSerializer.Serialize(entries))),
            Encoding.ASCII
        );
    }
}
