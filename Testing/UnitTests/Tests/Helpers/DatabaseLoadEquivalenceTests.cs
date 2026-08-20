using System.Text;
using System.Text.Json;
using NUnit.Framework;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Native;
using SPTarkov.Server.Core.Services.Server;
using SPTarkov.Server.Core.Utils;
using SPTarkov.Server.Core.Utils.Json;
using SPTarkov.Server.Helpers;

namespace UnitTests.Tests.Helpers;

/// <summary>
/// The flip's golden: the real SPT_Data walked twice - once reading every file off disk, once
/// materialising from the fused native load's buffers - must produce byte-identical tables. This is
/// bug-for-bug equivalence, not correctness: a difference here is a regression whichever arm is
/// "right". Compares <see cref="DatabaseTables"/> on both sides only; the resident roots the native
/// load also installs are a different shape by design and are not this test's business.
/// </summary>
[TestFixture]
[NonParallelizable]
public class DatabaseLoadEquivalenceTests
{
    private const string SptDataPath = "./SPT_Data/";

    /// One eager file per <see cref="DatabaseTables"/> root, named rather than counted so SPT_Data
    /// gaining or losing files does not make the fixture brittle.
    private static readonly string[] BufferFedPerRoot =
    [
        "database/bots/types/assault.json",
        "database/globals.json",
        "database/hideout/production.json",
        "database/locales/menu/en.json",
        "database/locations/bigmap/statics.json",
        "database/match/metrics.json",
        "database/server.json",
        "database/settings.json",
        "database/templates/items.json",
        "database/traders/54cb50c76803fa8b248b4571/base.json",
    ];

    private ImporterUtil _importerUtil = default!;

    [OneTimeSetUp]
    public void Initialize()
    {
        _importerUtil = DI.GetInstance().GetService<ImporterUtil>();
    }

    [OneTimeTearDown]
    public void OneTimeTearDown()
    {
        // DbLoad reinstalls the resident roots straight off disk, behind DbPublisher's bookkeeping,
        // so what is resident no longer reflects post-OnLoad state. Move the stamp and the next
        // EnsureCurrent() republishes for whichever fixture runs after this one.
        DI.GetInstance().GetService<DatabaseMutationStamp>().Bump();
    }

    [Test]
    public async Task NativeAndLegacyLoadsProduceIdenticalTables()
    {
        var legacy = await _importerUtil.LoadRecursiveAsync<DatabaseTables>($"{SptDataPath}database/");
        var load = SptNative.DbLoad(SptDataPath, verify: false);

        // Anti-vacuity. A key the map lacks falls back to disk silently (ImporterUtil.DeserializeFileAsync),
        // so a native filter that drops a subtree would turn this arm into a second legacy arm for that
        // subtree and every comparison below would still pass. One file per compared root, three of them
        // two levels down, pins that every root is really buffer-fed.
        Assert.That(load.Files.Keys, Is.SupersetOf(BufferFedPerRoot), "the fused load stopped feeding a root the golden compares");

        var native = await _importerUtil.LoadRecursiveAsync<DatabaseTables>($"{SptDataPath}database/", load.Files);

        // Roots with no LazyLoad anywhere below them: whole-root byte equality. Only globals.json,
        // server.json and settings.json are root-level files - the rest reach these tables through
        // the directory recursion, so the map has to be threaded down to hit them.
        AssertSameSerialized("Hideout", legacy.Hideout, native.Hideout);
        AssertSameSerialized("Match", legacy.Match, native.Match);
        AssertSameSerialized("Templates", legacy.Templates, native.Templates);
        AssertSameSerialized("Globals", legacy.Globals, native.Globals);
        AssertSameSerialized("Server", legacy.Server, native.Server);
        AssertSameSerialized("Settings", legacy.Settings, native.Settings);

        AssertSameSerialized("Bots.Base", legacy.Bots.Base, native.Bots.Base);
        AssertSameSerialized("Bots.Core", legacy.Bots.Core, native.Bots.Core);
        AssertSameSerializedByKey("Bots.Types", legacy.Bots.Types, native.Bots.Types);
        AssertSameSerializedByKey("Traders", legacy.Traders, native.Traders);

        AssertLocationsEqual(legacy.Locations, native.Locations);
        AssertLocalesEqual(legacy.Locales, native.Locales);
    }

    /// <summary>
    /// Locations carries the lazy trio, so it is compared field by field: serializing the root would
    /// pull LazyLoad.Value and materialise 549 MiB of looseLoot.
    /// </summary>
    private static void AssertLocationsEqual(LocationTable legacy, LocationTable native)
    {
        AssertSameSerialized("Locations.Base", legacy.Base, native.Base);

        var legacyMaps = legacy.GetDictionary();
        var nativeMaps = native.GetDictionary();

        Assert.That(nativeMaps.Keys, Is.EquivalentTo(legacyMaps.Keys), "Locations: map set differs");

        foreach (var (name, legacyMap) in legacyMaps)
        {
            var nativeMap = nativeMaps[name];

            if (legacyMap is null || nativeMap is null)
            {
                Assert.That(nativeMap is null, Is.EqualTo(legacyMap is null), $"Locations.{name}: present on one arm only");

                continue;
            }

            AssertSameSerialized($"Locations.{name}.Base", legacyMap.Base, nativeMap.Base);
            AssertSameSerialized($"Locations.{name}.AllExtracts", legacyMap.AllExtracts, nativeMap.AllExtracts);
            AssertSameSerialized($"Locations.{name}.StaticAmmo", legacyMap.StaticAmmo, nativeMap.StaticAmmo);
            AssertSameSerialized($"Locations.{name}.Statics", legacyMap.Statics, nativeMap.Statics);

            // The lazy trio short-circuits before the buffer lookup and stays a disk-path closure on
            // both arms, so only its shape is comparable without reading the files it defers.
            AssertSameLazyShape($"Locations.{name}.LooseLoot", legacyMap.LooseLoot, nativeMap.LooseLoot);
            AssertSameLazyShape($"Locations.{name}.StaticLoot", legacyMap.StaticLoot, nativeMap.StaticLoot);
            AssertSameLazyShape($"Locations.{name}.StaticContainers", legacyMap.StaticContainers, nativeMap.StaticContainers);
        }
    }

    private static void AssertLocalesEqual(LocaleTable legacy, LocaleTable native)
    {
        AssertSameSerializedByKey("Locales.Menu", legacy.Menu, native.Menu);
        AssertSameSerialized("Locales.Languages", legacy.Languages, native.Languages);

        // Global is a dictionary of lazy loads; its values are disk-backed on both arms.
        Assert.That(native.Global.Keys, Is.EquivalentTo(legacy.Global.Keys), "Locales.Global: language set differs");
    }

    private static void AssertSameLazyShape<T>(string name, LazyLoad<T>? legacy, LazyLoad<T>? native)
    {
        Assert.That(native is null, Is.EqualTo(legacy is null), $"{name}: present on one arm only");

        if (legacy is null || native is null)
        {
            return;
        }

        Assert.That(native.HasRawJson, Is.EqualTo(legacy.HasRawJson), $"{name}: raw-JSON backing differs");
    }

    /// <summary>
    /// Dictionary-shaped roots are filled by the walk's per-file tasks, so their insertion order is
    /// whatever the scheduler did - non-deterministic run to run on a single arm, never mind between
    /// two. Sort by key first; the values still go byte for byte.
    /// </summary>
    private static void AssertSameSerializedByKey<TKey, TValue>(
        string name,
        IEnumerable<KeyValuePair<TKey, TValue>> legacy,
        IEnumerable<KeyValuePair<TKey, TValue>> native
    )
        where TKey : notnull
    {
        AssertSameSerialized(name, SortedByKey(legacy), SortedByKey(native));
    }

    private static SortedDictionary<string, TValue> SortedByKey<TKey, TValue>(IEnumerable<KeyValuePair<TKey, TValue>> source)
        where TKey : notnull
    {
        var sorted = new SortedDictionary<string, TValue>(StringComparer.Ordinal);

        foreach (var (key, value) in source)
        {
            sorted[key.ToString()!] = value;
        }

        return sorted;
    }

    private static void AssertSameSerialized<T>(string name, T legacy, T native)
    {
        if (legacy is null || native is null)
        {
            Assert.That(native is null, Is.EqualTo(legacy is null), $"{name}: present on one arm only");

            return;
        }

        var legacyBytes = JsonSerializer.SerializeToUtf8Bytes(legacy, typeof(T), JsonUtil.JsonSerializerOptionsNoIndent);
        var nativeBytes = JsonSerializer.SerializeToUtf8Bytes(native, typeof(T), JsonUtil.JsonSerializerOptionsNoIndent);

        if (legacyBytes.AsSpan().SequenceEqual(nativeBytes))
        {
            return;
        }

        Assert.Fail(DescribeDifference(name, legacyBytes, nativeBytes));
    }

    private static string DescribeDifference(string name, byte[] legacyBytes, byte[] nativeBytes)
    {
        var shared = Math.Min(legacyBytes.Length, nativeBytes.Length);
        var offset = 0;

        while (offset < shared && legacyBytes[offset] == nativeBytes[offset])
        {
            offset++;
        }

        return $"{name}: legacy is {legacyBytes.Length} bytes, native is {nativeBytes.Length}, first differ at byte {offset}"
            + $"{Environment.NewLine}  legacy: {ContextAround(legacyBytes, offset)}"
            + $"{Environment.NewLine}  native: {ContextAround(nativeBytes, offset)}";
    }

    private static string ContextAround(byte[] bytes, int offset)
    {
        var start = Math.Max(0, offset - 40);
        var end = Math.Min(bytes.Length, offset + 40);

        return Encoding.UTF8.GetString(bytes, start, end - start);
    }
}
