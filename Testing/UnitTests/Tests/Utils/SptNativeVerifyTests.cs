using System.IO.Hashing;
using System.Text;
using System.Text.Json;
using NUnit.Framework;
using SPTarkov.Server.Core.Native;

namespace UnitTests.Tests.Utils;

[TestFixture]
public class SptNativeVerifyTests
{
    private string _sptDataDir = string.Empty;

    [SetUp]
    public void SetUp()
    {
        _sptDataDir = Path.Combine(Path.GetTempPath(), $"spt-native-test-{Guid.NewGuid():N}");
        Directory.CreateDirectory(Path.Combine(_sptDataDir, "database"));
    }

    [TearDown]
    public void TearDown()
    {
        Directory.Delete(_sptDataDir, true);
    }

    [Test]
    public async Task CleanTreePassesVerification()
    {
        // Everything under manifest-named roots is verified — configs/, non-json files and the
        // ImporterUtil-ignored locale dirs included. Top-level entries the manifest never names
        // (images/, the relocated dotnet/ and wwwroot/ build artifacts) are not.
        WriteDatabaseFile("database/globals.json", """{"a":1}""");
        WriteDatabaseFile("database/templates/items.json", """{"b":2}""");
        WriteDatabaseFile("database/locales/server/en.json", """{"c":3}""");
        WriteDatabaseFile("configs/core.json", """{"d":4}""");
        WriteDatabaseFile("images/icon.png", "not hashed");
        WriteDatabaseFile("dotnet/de/Spectre.Console.Cli.resources.dll", "not hashed");
        WriteDatabaseFile("wwwroot/index.html", "not hashed");
        WriteChecksDat("database/globals.json", "database/templates/items.json", "database/locales/server/en.json", "configs/core.json");

        var result = await SptNative.VerifyDatabaseAsync(_sptDataDir);

        Assert.That(result.Ok, Is.True);
        Assert.That(result.Checked, Is.EqualTo(4));
        Assert.That(result.Failures, Is.Empty);
    }

    [Test]
    public async Task DeletedFileFailsVerification()
    {
        WriteDatabaseFile("database/globals.json", """{"a":1}""");
        WriteDatabaseFile("database/deleted.json", """{"gone":true}""");
        WriteChecksDat("database/globals.json", "database/deleted.json");
        File.Delete(Path.Combine(_sptDataDir, "database", "deleted.json"));

        var result = await SptNative.VerifyDatabaseAsync(_sptDataDir);

        Assert.That(result.Ok, Is.False);
        Assert.That(result.Failures[0].Path, Is.EqualTo("database/deleted.json"));
        Assert.That(result.Failures[0].Reason, Is.EqualTo("missing_from_disk"));
    }

    [Test]
    public async Task TamperedFileFailsVerification()
    {
        WriteDatabaseFile("database/globals.json", """{"a":1}""");
        WriteChecksDat("database/globals.json");
        File.AppendAllText(Path.Combine(_sptDataDir, "database", "globals.json"), " ");

        var result = await SptNative.VerifyDatabaseAsync(_sptDataDir);

        Assert.That(result.Ok, Is.False);
        Assert.That(result.Failures[0].Path, Is.EqualTo("database/globals.json"));
        Assert.That(result.Failures[0].Reason, Is.EqualTo("hash_mismatch"));
    }

    [Test]
    public async Task FileMissingFromManifestFailsVerification()
    {
        WriteDatabaseFile("database/globals.json", """{"a":1}""");
        WriteDatabaseFile("database/extra.json", """{"x":9}""");
        WriteChecksDat("database/globals.json");

        var result = await SptNative.VerifyDatabaseAsync(_sptDataDir);

        Assert.That(result.Ok, Is.False);
        Assert.That(result.Failures[0].Path, Is.EqualTo("database/extra.json"));
        Assert.That(result.Failures[0].Reason, Is.EqualTo("missing_from_manifest"));
    }

    [Test]
    public async Task MissingChecksDatFailsVerification()
    {
        WriteDatabaseFile("database/globals.json", """{"a":1}""");

        var result = await SptNative.VerifyDatabaseAsync(_sptDataDir);

        Assert.That(result.Ok, Is.False);
        Assert.That(result.Failures[0].Path, Is.EqualTo("checks.dat"));
    }

    private void WriteDatabaseFile(string relativePath, string content)
    {
        var fullPath = Path.Combine(_sptDataDir, relativePath.Replace('/', Path.DirectorySeparatorChar));
        Directory.CreateDirectory(Path.GetDirectoryName(fullPath)!);
        File.WriteAllText(fullPath, content);
    }

    private void WriteChecksDat(params string[] relativePaths)
    {
        var entries = relativePaths
            .Select(relativePath =>
            {
                var fullPath = Path.Combine(_sptDataDir, relativePath.Replace('/', Path.DirectorySeparatorChar));
                var hash = Convert.ToHexString(XxHash128.Hash(File.ReadAllBytes(fullPath)));
                return new { Path = relativePath, Hash = hash };
            })
            .ToList();
        var json = JsonSerializer.Serialize(entries);
        File.WriteAllText(Path.Combine(_sptDataDir, "checks.dat"), Convert.ToBase64String(Encoding.UTF8.GetBytes(json)), Encoding.ASCII);
    }
}
