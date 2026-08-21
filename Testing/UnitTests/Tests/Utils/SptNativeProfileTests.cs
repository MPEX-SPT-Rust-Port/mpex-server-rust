using System.Text;
using NUnit.Framework;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Native;

namespace UnitTests.Tests.Utils;

/// <summary>
/// The managed end of the profile disk boundary, driven against the real cdylib: a temp profiles
/// directory, through the four spt_profile exports, back as bytes. Byte-fidelity is the contract, so
/// the save and round-trip assertions compare bytes and never parsed values - a re-serialisation
/// that re-escaped the Cyrillic or normalised the whitespace would still compare equal as JSON.
/// The exports are stateless, so nothing here needs the resident DB or [NonParallelizable].
/// </summary>
[TestFixture]
public class SptNativeProfileTests
{
    private const string SessionIdText = "6889d9d1f8ee8ab88c0b8e11";

    /// Indented, oddly spaced, mixed line endings, and Cyrillic - everything a re-encode would
    /// quietly rewrite on the way to disk.
    private const string OddProfileJson = "{\n  \"info\": {\r\n\t\"nickname\":\"Штурман\"  , \"level\": 1 },\n  \"empty\":[ ]\n}";

    private static readonly MongoId SessionId = new(SessionIdText);

    private string _profilesDir = string.Empty;

    [SetUp]
    public void SetUp()
    {
        _profilesDir = Path.Combine(Path.GetTempPath(), $"spt-native-profile-test-{Guid.NewGuid():N}");
        Directory.CreateDirectory(_profilesDir);
    }

    [TearDown]
    public void TearDown()
    {
        Directory.Delete(_profilesDir, true);
    }

    [Test]
    public async Task SaveWritesTheExactBytes()
    {
        await SptNative.ProfileSaveAsync(_profilesDir, SessionId, OddProfileJson);

        Assert.That(
            File.ReadAllBytes(ProfilePath()),
            Is.EqualTo(Encoding.UTF8.GetBytes(OddProfileJson)),
            "the file is what jsonUtil.Serialize produced, byte for byte"
        );
        Assert.That(File.Exists($"{ProfilePath()}.bak"), Is.False, "the temp file is renamed, not left behind");
    }

    [Test]
    public async Task SaveThenLoadRoundTrips()
    {
        await SptNative.ProfileSaveAsync(_profilesDir, SessionId, OddProfileJson);

        var result = await SptNative.ProfileLoadAsync(_profilesDir, SessionId);

        Assert.That(result.Found, Is.True);
        Assert.That(result.Utf8Json.ToArray(), Is.EqualTo(Encoding.UTF8.GetBytes(OddProfileJson)));
    }

    [Test]
    public async Task LoadMissingIsNotFound()
    {
        var result = await SptNative.ProfileLoadAsync(_profilesDir, SessionId);

        Assert.That(result.Found, Is.False);
        Assert.That(result.Utf8Json.IsEmpty, Is.True);
    }

    [Test]
    public async Task ListReturnsFileNamesNotSubdirs()
    {
        File.WriteAllText(ProfilePath(), "{}");
        Directory.CreateDirectory(Path.Combine(_profilesDir, "backups"));

        var files = await SptNative.ProfileListAsync(_profilesDir);

        Assert.That(files, Is.EqualTo(new[] { $"{SessionIdText}.json" }));
    }

    [Test]
    public void DeleteIsTrueThenFalse()
    {
        File.WriteAllText(ProfilePath(), "{}");

        Assert.That(SptNative.ProfileDelete(_profilesDir, SessionId), Is.True);
        Assert.That(File.Exists(ProfilePath()), Is.False);
        Assert.That(SptNative.ProfileDelete(_profilesDir, SessionId), Is.False, "a missing file is not an error");
    }

    /// <summary>
    /// The wrappers take a MongoId, so a traversal string cannot reach the boundary at all - it is
    /// rejected here, one layer up. The Rust-side gate is covered by the ffi.rs transport tests,
    /// which can forge a raw envelope no C# caller can express.
    /// </summary>
    [Test]
    public void TraversalIdIsRejectedBeforeTheBoundary()
    {
        Assert.Throws<ArgumentException>(() => new MongoId("../x"));
    }

    private string ProfilePath()
    {
        return Path.Combine(_profilesDir, $"{SessionIdText}.json");
    }
}
