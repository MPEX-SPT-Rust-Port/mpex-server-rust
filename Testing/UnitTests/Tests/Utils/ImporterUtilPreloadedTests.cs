using System.Text;
using System.Text.Json.Serialization;
using NUnit.Framework;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Utils;
using SPTarkov.Server.Core.Utils.Json;

namespace UnitTests.Tests.Utils;

/// <summary>
/// The importer's buffer seam: a small on-disk tree walked with a preloaded map beside it. Every file
/// is written with a body the buffers do not carry, so a value that came from disk is distinguishable
/// from one that came from the map.
/// </summary>
[TestFixture]
public class ImporterUtilPreloadedTests
{
    private const string FromDisk = """{"v":1}""";
    private const string FromBuffer = """{"v":2}""";

    /// Prapor: the importer's MongoId directory branch only fires on a 24-hex directory name.
    private const string TraderId = "54cb50c76803fa8b248b4571";

    private ImporterUtil _importerUtil = default!;
    private string _rootDirectory = string.Empty;

    [SetUp]
    public void SetUp()
    {
        _importerUtil = DI.GetInstance().GetService<ImporterUtil>();

        // PreloadKey resolves off the last "database/" segment, so the temp tree needs one.
        _rootDirectory = Path.Combine(Path.GetTempPath(), $"spt-import-test-{Guid.NewGuid():N}", "database");
        WriteFile("alpha.json", FromDisk);
        WriteFile("beta.json", FromDisk);
        WriteFile("lazy.json", FromDisk);
        WriteFile(Path.Combine("nested", "gamma.json"), FromDisk);
        WriteFile(Path.Combine("traders", TraderId, "delta.json"), FromDisk);
    }

    [TearDown]
    public void TearDown()
    {
        Directory.Delete(Path.GetDirectoryName(_rootDirectory)!, true);
    }

    [Test]
    public async Task APreloadedBufferBeatsTheFileOnDisk()
    {
        var result = await LoadAsync(Preloaded("database/alpha.json"));

        Assert.That(result.Alpha!.V, Is.EqualTo(2), "the map holds alpha.json, so disk's {\"v\":1} must not be read");
    }

    [Test]
    public async Task AFileAbsentFromTheMapFallsBackToDisk()
    {
        var result = await LoadAsync(Preloaded("database/alpha.json"));

        Assert.That(result.Beta!.V, Is.EqualTo(1), "beta.json is absent from the map");
        Assert.That(result.Nested!.Gamma!.V, Is.EqualTo(1), "a subdirectory miss still hits disk");
    }

    [Test]
    public async Task APreloadedBufferInASubdirectoryBeatsTheFileOnDisk()
    {
        var result = await LoadAsync(Preloaded("database/nested/gamma.json"));

        Assert.That(result.Nested!.Gamma!.V, Is.EqualTo(2), "the directory recursion carries the map down to nested/gamma.json");
    }

    [Test]
    public async Task APreloadedBufferInAMongoIdSubdirectoryBeatsTheFileOnDisk()
    {
        var result = await LoadAsync(Preloaded($"database/traders/{TraderId}/delta.json"));

        Assert.That(
            result.Traders[new MongoId(TraderId)].Delta!.V,
            Is.EqualTo(2),
            "the MongoId recursion carries the map down to traders/<id>/delta.json"
        );
    }

    [Test]
    public async Task ALazyLoadTargetIgnoresTheBufferAndStaysDiskBacked()
    {
        var result = await LoadAsync(Preloaded("database/lazy.json"));

        Assert.That(result.Lazy!.Value!.V, Is.EqualTo(1), "a LazyLoad stays a disk-path closure even when the map holds its file");
        Assert.That(Encoding.UTF8.GetString(result.Lazy.ReadRawJson()!.Value.Span), Is.EqualTo(FromDisk));
    }

    private async Task<TestRoot> LoadAsync(IReadOnlyDictionary<string, ReadOnlyMemory<byte>> preloadedFiles)
    {
        return await _importerUtil.LoadRecursiveAsync<TestRoot>($"{_rootDirectory.Replace('\\', '/')}/", preloadedFiles);
    }

    private static Dictionary<string, ReadOnlyMemory<byte>> Preloaded(params string[] keys)
    {
        return keys.ToDictionary(key => key, _ => (ReadOnlyMemory<byte>)Encoding.UTF8.GetBytes(FromBuffer));
    }

    private void WriteFile(string relativePath, string content)
    {
        var fullPath = Path.Combine(_rootDirectory, relativePath);
        Directory.CreateDirectory(Path.GetDirectoryName(fullPath)!);
        File.WriteAllText(fullPath, content);
    }

    private record TestRoot
    {
        public TestPayload? Alpha { get; set; }
        public TestPayload? Beta { get; set; }
        public LazyLoad<TestPayload>? Lazy { get; set; }
        public TestNested? Nested { get; set; }

        // The traders shape: a MongoId-keyed dictionary the importer fills from 24-hex subdirectories.
        public Dictionary<MongoId, TestTrader> Traders { get; set; } = new();
    }

    private record TestNested
    {
        public TestPayload? Gamma { get; set; }
    }

    private record TestTrader
    {
        public TestPayload? Delta { get; set; }
    }

    private record TestPayload
    {
        [JsonPropertyName("v")]
        public int V { get; set; }
    }
}
