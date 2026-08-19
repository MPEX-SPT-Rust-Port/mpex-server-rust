using System.IO.Hashing;
using System.Text;
using System.Text.Json;
using NUnit.Framework;
using SPTarkov.Server.Core.Native;

namespace UnitTests.Tests.Utils;

/// <summary>
/// The managed end of the fused load: a real SPT_Data-shaped tree on disk, through spt_db_load,
/// back as decoded file bytes. The tree is the flip #4 publish envelope's root bodies written back
/// out as the files the importer would have read them from (rust/spt-native/src/db/load.rs).
/// </summary>
[TestFixture]
public class SptNativeDbLoadTests
{
    /// Prapor, so the trader directory carries a name the importer's MongoId branch accepts.
    private const string TraderId = "54cb50c76803fa8b248b4571";

    // Ids verbatim from the Rust fixture: 54009119… is the item node, 543be5dd… MONEY and
    // 5422acb9… WEAPON (item_helper.rs), the rest are the fixture's own made-up templates.
    private const string ItemsJson = """
        {
            "54009119af1c881c07000029":{"_type":"Node","_parent":"","_props":{}},
            "543be5dd4bdc2deb348b4569":{"_type":"Node","_parent":"54009119af1c881c07000029","_props":{}},
            "5422acb9af1c889c16000029":{"_type":"Node","_parent":"54009119af1c881c07000029","_props":{}},
            "misc_node":{"_type":"Node","_parent":"54009119af1c881c07000029","_props":{}},
            "111111111111111111111111":{"_parent":"54009119af1c881c07000029","_props":{"Width":1,"Height":1,
                "Grids":[{"_props":{"cellsH":2,"cellsV":2}}]}},
            "333333333333333333333333":{"_parent":"543be5dd4bdc2deb348b4569","_props":{"Width":1,"Height":1,
                "StackMaxSize":500000,"StackMinRandom":100,"StackMaxRandom":200}},
            "weapon_tpl":{"_parent":"5422acb9af1c889c16000029","_props":{}},
            "weapon_mod_a":{"_parent":"misc_node","_props":{}},
            "weapon_mod_b":{"_parent":"misc_node","_props":{}}
        }
        """;

    private const string GlobalsJson = """
        {"ItemPresets":{
            "p1":{"_id":"p1","_name":"weapon_default","_encyclopedia":"weapon_tpl",
                "_items":[{"_id":"root_p1","_tpl":"weapon_tpl"},
                    {"_id":"mod_p1","_tpl":"weapon_mod_a","parentId":"root_p1","slotId":"mod_stock"}]},
            "p2":{"_id":"p2","_name":"weapon_alt",
                "_items":[{"_id":"root_p2","_tpl":"weapon_tpl"}]}
        }}
        """;

    private const string StaticLootJson = """
        {"111111111111111111111111":{
            "itemcountDistribution":[{"count":2,"relativeProbability":1}],
            "itemDistribution":[{"tpl":"333333333333333333333333","relativeProbability":1}]}}
        """;

    private const string StaticContainersJson = """
        {"staticWeapons":[],"staticForced":[],"staticContainers":[
            {"probability":1.0,"template":{"Id":"c1","IsContainer":true,
                "Root":"aaaaaaaaaaaaaaaaaaaaaaa1",
                "Items":[{"_id":"aaaaaaaaaaaaaaaaaaaaaaa1","_tpl":"111111111111111111111111"}]}}]}
        """;

    private const string ProductionJson = """
        {"scavRecipes":[
            {"_id":"6662e9aca7e0b43baa3d5f9c","endProducts":{"Common":{"min":3,"max":3},
                "Rare":{"min":1,"max":1},"Superrare":{"min":0,"max":0}}}]}
        """;

    private const string StaticsJson = """
        {"containersGroups":{"g1":{"minContainers":1,"maxContainers":2}},
        "containers":{"c1":{"groupId":"g1"}}}
        """;

    private string _sptDataDir = string.Empty;

    [SetUp]
    public void SetUp()
    {
        _sptDataDir = Path.Combine(Path.GetTempPath(), $"spt-native-test-{Guid.NewGuid():N}");
        Directory.CreateDirectory(Path.Combine(_sptDataDir, "database"));
        WriteMiniTree();
    }

    [TearDown]
    public void TearDown()
    {
        Directory.Delete(_sptDataDir, true);
    }

    [Test]
    public void DbLoadReturnsEagerFilesAndAnEpoch()
    {
        WriteChecksDat();

        var result = SptNative.DbLoad(_sptDataDir, false);

        Assert.That(result.Verify, Is.Null, "verify:false answers no report even with a manifest on disk");
        Assert.That(result.Epoch, Is.Not.Null);
        Assert.That(result.Files.Keys, Contains.Item("database/templates/items.json"));
        Assert.That(
            Encoding.UTF8.GetString(result.Files["database/templates/items.json"].Span),
            Is.EqualTo(ItemsJson),
            "the blob is the file's bytes, not a re-serialisation"
        );
        // The lazy and skipped classes never ship: C# re-reads looseLoot per access, and the
        // importer ignores the locales/server tree and the faction suit files entirely.
        Assert.That(result.Files.Keys, Does.Not.Contain("database/locations/bigmap/looseLoot.json"));
        Assert.That(result.Files.Keys, Does.Not.Contain("database/locales/server/en.json"));
    }

    [Test]
    public void DbLoadWithVerifyFailureReturnsTheReportAndNoFiles()
    {
        WriteChecksDat();
        File.AppendAllText(Path.Combine(_sptDataDir, "database", "globals.json"), " ");

        var result = SptNative.DbLoad(_sptDataDir, true);

        Assert.That(result.Verify, Is.Not.Null);
        Assert.That(result.Verify!.Ok, Is.False);
        Assert.That(result.Verify.Failures[0].Path, Is.EqualTo("database/globals.json"));
        Assert.That(result.Verify.Failures[0].Reason, Is.EqualTo("hash_mismatch"));
        Assert.That(result.Epoch, Is.Null, "a failed verification installs nothing");
        Assert.That(result.Files, Is.Empty);
    }

    [Test]
    public void DbLoadWithoutChecksDatAndVerifyOffSucceeds()
    {
        // The Debug shape: no checks.dat is shipped, so the no-verify arm must not need one.
        var result = SptNative.DbLoad(_sptDataDir, false);

        Assert.That(result.Verify, Is.Null);
        Assert.That(result.Epoch, Is.Not.Null);
        Assert.That(result.Files.Keys, Contains.Item("database/globals.json"));
    }

    /// <summary>
    /// Every file class in one tree: eager roots, the assembly-only statics, the lazy never-read
    /// pair, and one file from each of the importer's two skip rules.
    /// </summary>
    private void WriteMiniTree()
    {
        WriteDatabaseFile("database/templates/items.json", ItemsJson);
        WriteDatabaseFile("database/templates/handbook.json", """{"Items":[]}""");
        WriteDatabaseFile("database/templates/prices.json", "{}");
        WriteDatabaseFile("database/templates/archivedQuests.json", "{}");
        WriteDatabaseFile($"database/traders/{TraderId}/base.json", "{}");
        WriteDatabaseFile($"database/traders/{TraderId}/bearsuits.json", "[]");
        WriteDatabaseFile("database/globals.json", GlobalsJson);
        WriteDatabaseFile("database/locations/base.json", "{}");
        WriteDatabaseFile("database/locations/bigmap/base.json", """{"Id":"bigmap"}""");
        WriteDatabaseFile("database/locations/bigmap/allExtracts.json", "[]");
        WriteDatabaseFile("database/locations/bigmap/staticLoot.json", StaticLootJson);
        WriteDatabaseFile("database/locations/bigmap/staticContainers.json", StaticContainersJson);
        WriteDatabaseFile("database/locations/bigmap/statics.json", StaticsJson);
        WriteDatabaseFile("database/locations/bigmap/looseLoot.json", "{}");
        WriteDatabaseFile("database/hideout/production.json", ProductionJson);
        WriteDatabaseFile("database/locales/menu/en.json", "{}");
        WriteDatabaseFile("database/locales/global/en.json", "{}");
        WriteDatabaseFile("database/locales/server/en.json", "{}");
    }

    private void WriteDatabaseFile(string relativePath, string content)
    {
        var fullPath = Path.Combine(_sptDataDir, relativePath.Replace('/', Path.DirectorySeparatorChar));
        Directory.CreateDirectory(Path.GetDirectoryName(fullPath)!);
        File.WriteAllText(fullPath, content);
    }

    /// <summary>
    /// Hashes every file under database/ - verification is bidirectional, so a partial manifest
    /// would fail on the files it left out instead of on the one a test tampers with.
    /// </summary>
    private void WriteChecksDat()
    {
        var entries = Directory
            .EnumerateFiles(Path.Combine(_sptDataDir, "database"), "*", SearchOption.AllDirectories)
            .Select(fullPath =>
            {
                var relativePath = Path.GetRelativePath(_sptDataDir, fullPath).Replace(Path.DirectorySeparatorChar, '/');
                return new { Path = relativePath, Hash = Convert.ToHexString(XxHash128.Hash(File.ReadAllBytes(fullPath))) };
            })
            .ToList();
        var json = JsonSerializer.Serialize(entries);
        File.WriteAllText(Path.Combine(_sptDataDir, "checks.dat"), Convert.ToBase64String(Encoding.UTF8.GetBytes(json)), Encoding.ASCII);
    }
}
