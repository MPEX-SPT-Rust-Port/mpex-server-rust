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
/// inside DI construction. Hence: fixed ids (a leak the next run can reach), <c>CleanOwnedFiles</c>
/// hoisted into the assembly-level <see cref="ProfileDirectorySetUp"/> so it genuinely precedes the
/// run's first <c>LoadAsync</c>, and a snapshot of the directory so profiles that <c>SaveAsync</c>
/// flushed on other fixtures' behalf go too.
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

    private static readonly string[] OwnedIds = [SaveAndRemoveId, UnknownId, EmptyFileId, BomId, UnwritableId, SurvivorId];

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
    /// The autosave tick's per-profile guard. Without it the first failure propagates straight out of
    /// <c>SaveAsync</c>, so the no-throw is the pin regardless of the order
    /// <c>ConcurrentDictionary</c> hands the profiles over in.
    /// </summary>
    [Test]
    public void SaveAsyncSurvivesOneUnwritableProfile()
    {
        _saveServer.CreateProfile(new Info { ProfileId = new MongoId(UnwritableId), Username = "unwritable" });
        _saveServer.CreateProfile(new Info { ProfileId = new MongoId(SurvivorId), Username = "survivor" });

        // The native save writes {id}.json.bak first and renames it over the live file; a directory
        // squatting on that name fails the create.
        Directory.CreateDirectory(TempPath(UnwritableId));

        Assert.DoesNotThrowAsync(async () => await _saveServer.SaveAsync());

        Assert.That(File.Exists(ProfilePath(SurvivorId)), Is.True, "the second profile was starved by the first");
        Assert.That(File.Exists(ProfilePath(UnwritableId)), Is.False, "the unwritable profile somehow landed");
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

    private static HashSet<string> SnapshotProfileDir()
    {
        return Directory.Exists(ProfileDir) ? [.. Directory.GetFiles(ProfileDir)] : [];
    }

    /// <summary>
    /// Best-effort, and called from two places: <see cref="ProfileDirectorySetUp"/> before the run's
    /// first <c>LoadAsync</c>, and this fixture's teardown after every test.
    /// </summary>
    internal static void CleanOwnedFiles()
    {
        if (!Directory.Exists(ProfileDir))
        {
            return;
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
