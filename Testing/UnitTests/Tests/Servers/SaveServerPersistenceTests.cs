using System.Text.Json.Nodes;
using NUnit.Framework;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Profile;
using SPTarkov.Server.Core.Servers;
using SPTarkov.Server.Core.Utils;

namespace UnitTests.Tests.Servers;

/// <summary>
/// <see cref="SaveServer"/>'s disk boundary, now that both directions run through
/// <c>rust/spt-native</c>'s <c>spt_profile_*</c> exports: a profile written, read back, deleted, the
/// corrupt-recovery arm, and the BOM parity pin the native load's <c>strip_bom</c> exists for.
///
/// These tests write into the <c>user/profiles/</c> the test process itself reads —
/// <c>profileFilepath</c> is a <c>private const</c> behind a frozen surface, so there is nowhere
/// else for them to go, and <c>DI</c> runs every <c>IOnLoad</c> once per test process, which means
/// <c>SaveCallbacks.OnLoadAsync</c> loads that directory on every run. A file leaked by one run is a
/// file the next run tries to load, and a leaked zero-byte profile turns the whole suite red from
/// inside DI construction. Hence: fixed ids (a leak the next run can reach), the assembly-level
/// <see cref="ProfileDirectorySetUp"/> clearing the whole directory before the run's first
/// <c>LoadAsync</c>, and a snapshot here so profiles that <c>SaveAsync</c> flushed on other
/// fixtures' behalf go at teardown too.
/// </summary>
[TestFixture]
[NonParallelizable]
public class SaveServerPersistenceTests
{
    private const string ProfileDir = "user/profiles";

    // Fixed, hard-coded, one per test. A random id would leak a file nothing ever cleans.
    private const string SaveAndRemoveId = "5f5f5f5f5f5f5f5f5f5f5f01";
    private const string UnknownId = "5f5f5f5f5f5f5f5f5f5f5f02";
    private const string EmptyFileId = "5f5f5f5f5f5f5f5f5f5f5f03";
    private const string BomId = "5f5f5f5f5f5f5f5f5f5f5f04";
    private const string UnwritableId = "5f5f5f5f5f5f5f5f5f5f5f05";
    private const string SurvivorId = "5f5f5f5f5f5f5f5f5f5f5f06";
    private const string IntactId = "5f5f5f5f5f5f5f5f5f5f5f07";
    private const string RestoredId = "5f5f5f5f5f5f5f5f5f5f5f08";
    private const string ConcurrentId = "5f5f5f5f5f5f5f5f5f5f5f09";

    private static readonly string[] OwnedIds =
    [
        SaveAndRemoveId,
        UnknownId,
        EmptyFileId,
        BomId,
        UnwritableId,
        SurvivorId,
        IntactId,
        RestoredId,
        ConcurrentId,
    ];

    private SaveServer _saveServer = default!;
    private JsonUtil _jsonUtil = default!;
    private HashSet<string> _preexistingFiles = default!;

    [OneTimeSetUp]
    public void Initialize()
    {
        // The leaked-file sweep is ProfileDirectorySetUp's: by the time this runs, DI has very
        // likely already been built - and with it LoadAsync - by an earlier fixture.
        var di = DI.GetInstance();
        _saveServer = di.GetService<SaveServer>();
        _jsonUtil = di.GetService<JsonUtil>();

        _preexistingFiles = SnapshotProfileDir();
    }

    /// <summary>
    /// Unconditional and outside any assertion: <c>RemoveProfile</c> is under test here, so it cannot
    /// be relied on to clean up after a test that just proved it broken.
    /// </summary>
    [TearDown]
    public void Cleanup()
    {
        foreach (var id in OwnedIds)
        {
            // Memory-only by design ("Does not remove the profile file!"), so this only drops the
            // fixture's profiles out of the shared SaveServer; the files go below.
            _saveServer.DeleteProfileById(new MongoId(id));
        }

        // CleanOwnedFiles can throw - Directory.Delete on the parked .json.bak, File.Delete on a
        // locked path - and the sweep below is the only cleanup that reaches the random-id
        // profiles, so it must not be conditional on the fixed-id one succeeding.
        try
        {
            CleanOwnedFiles();
        }
        finally
        {
            // SaveAsync writes every in-memory profile, including ones other fixtures created under
            // random ids that no fixed-id cleanup could reach.
            foreach (var file in SnapshotProfileDir())
            {
                if (!_preexistingFiles.Contains(file))
                {
                    File.Delete(file);
                }
            }
        }
    }

    [Test]
    public async Task SaveWritesAndRemoveDeletes()
    {
        var id = new MongoId(SaveAndRemoveId);
        var path = ProfilePath(SaveAndRemoveId);
        _saveServer.CreateProfile(new Info { ProfileId = id, Username = "save-and-remove" });

        await _saveServer.SaveProfileAsync(id);

        Assert.That(File.Exists(path), Is.True, $"{path} was not written");
        var written = _jsonUtil.Deserialize<JsonObject>(File.ReadAllText(path));
        Assert.That(written?["info"]?["id"]?.GetValue<string>(), Is.EqualTo(SaveAndRemoveId));

        Assert.That(_saveServer.RemoveProfile(id), Is.True);
        Assert.That(File.Exists(path), Is.False, $"{path} outlived RemoveProfile");
        Assert.That(_saveServer.ProfileExists(id), Is.False);
    }

    /// <summary>
    /// The return value answers "is the file gone", not "did I delete something", so an id that was
    /// never in memory and never on disk still reports true, and nothing is thrown.
    /// </summary>
    [Test]
    public void RemoveProfileForUnknownIdReportsFileState()
    {
        var id = new MongoId(UnknownId);
        Assert.That(_saveServer.ProfileExists(id), Is.False);
        Assert.That(File.Exists(ProfilePath(UnknownId)), Is.False);

        Assert.That(_saveServer.RemoveProfile(id), Is.True);
    }

    /// <summary>
    /// The corrupt copy is written before <c>BackupService.RestoreProfile</c> is consulted, so it is
    /// the observable that proves the JsonException arm was entered rather than the profile being
    /// silently dropped. No backup exists for this id, so the arm rethrows — but not always as the
    /// same type: <c>RestoreProfile</c> returns false and yields "Failed to restore profile backup"
    /// once a <c>user/profiles/backups</c> directory exists, and throws
    /// <c>DirectoryNotFoundException</c> out of <c>GetBackupPaths</c> before one ever has (this
    /// suite's usual state, since the startup backup finds no profiles and returns before creating
    /// it). Both rethrow, which is what this pins; neither drops the profile.
    /// </summary>
    [Test]
    public void EmptyProfileFileTakesTheRecoveryArm()
    {
        var id = new MongoId(EmptyFileId);
        File.WriteAllBytes(ProfilePath(EmptyFileId), []);

        Assert.CatchAsync<Exception>(async () => await _saveServer.LoadProfileAsync(id));

        Assert.That(File.Exists(CorruptPath(EmptyFileId)), Is.True, "the corrupt copy was never written");
        Assert.That(_saveServer.ProfileExists(id), Is.False, "the empty file was loaded as a profile");
    }

    /// <summary>
    /// The C#-side pin for the BOM Global Constraint. Task 4 moved profile loading onto
    /// <c>JsonUtil</c>'s <c>ReadOnlySpan&lt;byte&gt;</c> overload, whose <c>Utf8JsonReader</c> calls a
    /// leading BOM an invalid start of a value, where the <c>Stream</c> overload it replaced consumed
    /// one. Without <c>profile.rs</c>'s strip this fails by writing <c>{id}-corrupt.json</c> — exactly
    /// the silent rollback to an older backup the constraint exists to prevent.
    /// </summary>
    [Test]
    public async Task BomProfileStillLoads()
    {
        var id = new MongoId(BomId);
        var path = ProfilePath(BomId);
        _saveServer.CreateProfile(new Info { ProfileId = id, Username = "bom" });
        await _saveServer.SaveProfileAsync(id);
        var profileJson = File.ReadAllBytes(path);

        // Memory-only removal, so ProfileExists below can only come back true by way of the file.
        Assert.That(_saveServer.DeleteProfileById(id), Is.True);
        Assert.That(_saveServer.ProfileExists(id), Is.False);

        File.WriteAllBytes(path, [0xEF, 0xBB, 0xBF, .. profileJson]);

        Assert.DoesNotThrowAsync(async () => await _saveServer.LoadProfileAsync(id));

        Assert.That(_saveServer.ProfileExists(id), Is.True, "the BOM'd profile did not load");
        Assert.That(File.Exists(CorruptPath(BomId)), Is.False, "the BOM took the corrupt-recovery arm");
    }

    /// <summary>
    /// The autosave tick's per-profile guard, and the retry that guard exists to make possible.
    /// Without the guard the first failure propagates straight out of <c>SaveAsync</c>, so the
    /// no-throw is the pin regardless of the order <c>ConcurrentDictionary</c> hands the profiles
    /// over in.
    ///
    /// The second <c>SaveAsync</c> is the half with the data-loss consequence. <c>saveMd5</c> is a
    /// content hash recorded <i>after</i> the write; record it before, as this file did until
    /// <c>e7d3a4b</c>, and a failed write marks that version persisted, the next tick skips it, and
    /// the profile is lost with the process while the file silently disagrees with memory. Nothing
    /// pinned that, and reintroducing the old ordering left the suite green.
    /// </summary>
    [Test]
    public void SaveAsyncSurvivesOneUnwritableProfileAndRetriesIt()
    {
        _saveServer.CreateProfile(new Info { ProfileId = new MongoId(UnwritableId), Username = "unwritable" });
        _saveServer.CreateProfile(new Info { ProfileId = new MongoId(SurvivorId), Username = "survivor" });

        // The native save writes {id}.json.bak first and renames it over the live file; a directory
        // squatting on that name fails the create.
        Directory.CreateDirectory(TempPath(UnwritableId));

        Assert.DoesNotThrowAsync(async () => await _saveServer.SaveAsync());

        Assert.That(File.Exists(ProfilePath(SurvivorId)), Is.True, "the second profile was starved by the first");
        Assert.That(File.Exists(ProfilePath(UnwritableId)), Is.False, "the unwritable profile somehow landed");

        // Unblock it and tick again. The profile is untouched since the failed attempt, so its md5
        // is unchanged - the write can only happen at all if the failure left saveMd5 unpoisoned.
        Directory.Delete(TempPath(UnwritableId), true);

        Assert.DoesNotThrowAsync(async () => await _saveServer.SaveAsync());

        Assert.That(File.Exists(ProfilePath(UnwritableId)), Is.True, "the failed save was recorded as persisted and never retried");
    }

    /// <summary>
    /// What temp-then-rename is for: a save that fails must not damage the profile already on disk.
    /// The mutation is load-bearing - an unchanged profile hashes the same and
    /// <c>SaveProfileAsync</c> returns without writing, so without it this would pass on a path that
    /// never attempted the overwrite.
    /// </summary>
    [Test]
    public async Task FailedOverwriteLeavesThePreviousProfileIntact()
    {
        var id = new MongoId(IntactId);
        _saveServer.CreateProfile(new Info { ProfileId = id, Username = "intact" });
        await _saveServer.SaveProfileAsync(id);
        var persisted = await File.ReadAllBytesAsync(ProfilePath(IntactId));

        Directory.CreateDirectory(TempPath(IntactId));
        _saveServer.GetProfile(id).ProfileInfo!.Username = "mutated";

        Assert.ThrowsAsync<InvalidOperationException>(async () => await _saveServer.SaveProfileAsync(id));

        Assert.That(
            await File.ReadAllBytesAsync(ProfilePath(IntactId)),
            Is.EqualTo(persisted),
            "a failed overwrite damaged the profile already on disk"
        );
    }

    /// <summary>
    /// The corrupt-recovery arm's success half, which <see cref="EmptyProfileFileTakesTheRecoveryArm"/>
    /// does not reach - it pins only the rethrow. Phase 5 changed this path: after
    /// <c>BackupService.RestoreProfile</c> returns true the re-read goes through
    /// <c>SptNative.ProfileLoadAsync</c> rather than the file, and nothing covered it.
    /// </summary>
    [Test]
    public async Task CorruptProfileIsRestoredFromBackupThroughTheNativeReload()
    {
        var id = new MongoId(RestoredId);
        _saveServer.CreateProfile(new Info { ProfileId = id, Username = "restored" });
        await _saveServer.SaveProfileAsync(id);

        // BackupService reads user/profiles/backups/<yyyy-MM-dd_HH-mm-ss>/, newest folder first.
        Directory.CreateDirectory(BackupFolder);
        File.Copy(ProfilePath(RestoredId), Path.Combine(BackupFolder, $"{RestoredId}.json"), true);

        Assert.That(_saveServer.DeleteProfileById(id), Is.True);
        await File.WriteAllTextAsync(ProfilePath(RestoredId), "{ not json");

        Assert.DoesNotThrowAsync(async () => await _saveServer.LoadProfileAsync(id));

        Assert.That(_saveServer.ProfileExists(id), Is.True, "the backup was never loaded back");
        Assert.That(File.Exists(CorruptPath(RestoredId)), Is.True, "the corrupt copy was never written");
    }

    /// <summary>
    /// <c>profile.rs::save</c> uses a temp name fixed per id, so two concurrent saves of one id would
    /// interleave - the second truncating the temp under the first, the first's rename publishing
    /// partial bytes. Exact parity with the C# it replaced, and <c>SaveProfileAsync</c>'s per-session
    /// <c>SemaphoreSlim</c> is what prevents it; that guard now sits on the near side of an FFI call
    /// whose write runs on a threadpool thread, which makes the interleaving more reachable, not less.
    ///
    /// A pass does not prove the guard is present - the race is not deterministic - but a partial
    /// file here never parses, so this fails loudly if the lock is ever dropped.
    /// </summary>
    [Test]
    public async Task ConcurrentSavesOfOneProfileLeaveAWellFormedFile()
    {
        var id = new MongoId(ConcurrentId);
        _saveServer.CreateProfile(new Info { ProfileId = id, Username = "concurrent" });

        await Task.WhenAll(_saveServer.SaveProfileAsync(id), _saveServer.SaveProfileAsync(id));

        var written = _jsonUtil.Deserialize<JsonObject>(await File.ReadAllTextAsync(ProfilePath(ConcurrentId)));
        Assert.That(written?["info"]?["id"]?.GetValue<string>(), Is.EqualTo(ConcurrentId));
        Assert.That(File.Exists(TempPath(ConcurrentId)), Is.False, "the temp file outlived the save");
    }

    private static string ProfilePath(string id)
    {
        return Path.Combine(ProfileDir, $"{id}.json");
    }

    private static string TempPath(string id)
    {
        return Path.Combine(ProfileDir, $"{id}.json.bak");
    }

    private static string CorruptPath(string id)
    {
        return Path.Combine(ProfileDir, $"{id}-corrupt.json");
    }

    // BackupConfig.Directory, plus one folder named the way ExtractDateFromFolderName parses:
    // yyyy-MM-dd_HH-mm-ss, between 1900 and five years out. Fixed, so a leak is reachable next run.
    private static readonly string BackupFolder = Path.Combine(ProfileDir, "backups", "2020-01-01_00-00-00");

    private static HashSet<string> SnapshotProfileDir()
    {
        return Directory.Exists(ProfileDir) ? [.. Directory.GetFiles(ProfileDir)] : [];
    }

    /// <summary>
    /// This fixture's own teardown, after every test. The cross-run sweep is
    /// <see cref="ProfileDirectorySetUp"/>'s and is broader — see its remarks.
    /// </summary>
    private static void CleanOwnedFiles()
    {
        if (!Directory.Exists(ProfileDir))
        {
            return;
        }

        // CorruptProfileIsRestoredFromBackupThroughTheNativeReload seeds one; leaving it would let a
        // later run's EmptyProfileFileTakesTheRecoveryArm restore instead of rethrowing.
        if (Directory.Exists(BackupFolder))
        {
            Directory.Delete(BackupFolder, true);
        }

        var backupDir = Path.Combine(ProfileDir, "backups");
        foreach (var id in OwnedIds)
        {
            foreach (var path in new[] { ProfilePath(id), TempPath(id), CorruptPath(id) })
            {
                // SaveAsyncSurvivesOneUnwritableProfile parks a directory on the temp name.
                if (Directory.Exists(path))
                {
                    Directory.Delete(path, true);
                }
                else
                {
                    File.Delete(path);
                }
            }

            // A backup of an owned id would let EmptyProfileFileTakesTheRecoveryArm restore instead
            // of rethrowing. One can only exist if a hard kill leaked a file past a previous run.
            if (Directory.Exists(backupDir))
            {
                foreach (var stale in Directory.GetFiles(backupDir, $"{id}.json", SearchOption.AllDirectories))
                {
                    File.Delete(stale);
                }
            }
        }
    }
}
