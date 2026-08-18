using System.Diagnostics;
using System.Text.Json;
using NUnit.Framework;
using SPTarkov.Server.Core.Loaders;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Utils;
using UnitTests.Mock;

namespace UnitTests.Tests.Utils;

[TestFixture]
[Explicit("Phase 0 publish spike: run by hand in Release; writes the payload file the Rust half reads")]
public class DbPublishSpikeTests
{
    [Test]
    public void ProjectAndMeasureFullPublishEnvelope()
    {
        var di = DI.GetInstance();
        var options = JsonUtil.JsonSerializerOptionsNoIndent ?? throw new InvalidOperationException("JsonUtil has not been built yet.");

        // Locales serialize with DEFAULT options on purpose: JsonUtil's naming policies would
        // rewrite the locale keys (the SetServerLocales precedent, Native/SptNative.cs:98).
        var roots = new (string Name, Func<byte[]> Project)[]
        {
            ("templates", () => JsonSerializer.SerializeToUtf8Bytes(di.GetService<TemplateTable>(), options)),
            ("bots", () => JsonSerializer.SerializeToUtf8Bytes(di.GetService<BotTable>(), options)),
            ("hideout", () => JsonSerializer.SerializeToUtf8Bytes(di.GetService<HideoutTable>(), options)),
            (
                "locales",
                () =>
                {
                    var table = di.GetService<LocaleTable>();
                    var payload = new Dictionary<string, object?>
                    {
                        ["global"] = table.Global.ToDictionary(kv => kv.Key, kv => kv.Value.Value),
                        ["menu"] = table.Menu,
                        ["languages"] = table.Languages,
                    };
                    return JsonSerializer.SerializeToUtf8Bytes(payload);
                }
            ),
            (
                // Base only: the LooseLoot/StaticLoot/StaticContainers LazyLoads are excluded —
                // looseLoot residency (549 MiB raw) is a separate design-doc decision.
                "locations",
                () =>
                {
                    var bases = di.GetService<LocationTable>().GetDictionary().ToDictionary(kv => kv.Key, kv => kv.Value.Base);
                    return JsonSerializer.SerializeToUtf8Bytes(bases, options);
                }
            ),
            ("match", () => JsonSerializer.SerializeToUtf8Bytes(di.GetService<MatchTable>(), options)),
            ("traders", () => JsonSerializer.SerializeToUtf8Bytes(di.GetService<TradersTable>(), options)),
            ("globals", () => JsonSerializer.SerializeToUtf8Bytes(di.GetService<GlobalTable>(), options)),
            ("server", () => JsonSerializer.SerializeToUtf8Bytes(di.GetService<ServerTable>(), options)),
            ("settings", () => JsonSerializer.SerializeToUtf8Bytes(di.GetService<SettingsTable>(), options)),
            (
                "configs",
                () =>
                {
                    var configuration = ConfigLoader.Initialize(new MockLogger<DbPublishSpikeTests>()).GetAwaiter().GetResult();
                    using var ms = new MemoryStream();
                    using (var w = new Utf8JsonWriter(ms))
                    {
                        w.WriteStartObject();
                        foreach (var entry in configuration)
                        {
                            w.WritePropertyName(entry.Key.Name);
                            w.WriteRawValue(
                                JsonSerializer.SerializeToUtf8Bytes(entry.Value, entry.Key, options),
                                skipInputValidation: true
                            );
                        }
                        w.WriteEndObject();
                    }
                    return ms.ToArray();
                }
            ),
        };

        var projected = new List<(string Name, byte[] Bytes)>();
        double totalCold = 0;
        double totalWarm = 0;

        TestContext.Out.WriteLine($"{"root", -12} {"size MiB", 9} {"cold ms", 9} {"warm ms", 9}");
        foreach (var (name, project) in roots)
        {
            var sw = Stopwatch.StartNew();
            var cold = project();
            var coldMs = sw.Elapsed.TotalMilliseconds;

            sw.Restart();
            var warm = project();
            var warmMs = sw.Elapsed.TotalMilliseconds;

            totalCold += coldMs;
            totalWarm += warmMs;
            projected.Add((name, warm));
            TestContext.Out.WriteLine($"{name, -12} {warm.Length / 1048576.0, 9:F1} {coldMs, 9:F1} {warmMs, 9:F1}");
            Assert.That(cold.Length, Is.EqualTo(warm.Length), $"{name}: cold/warm projections differ in size");
        }

        var payloadPath = Path.Combine(Path.GetTempPath(), "spt-phase0-publish.json");
        var swEnvelope = Stopwatch.StartNew();
        using (var stream = File.Create(payloadPath))
        using (var writer = new Utf8JsonWriter(stream))
        {
            writer.WriteStartObject();
            writer.WriteNumber("schema", 1);
            writer.WritePropertyName("roots");
            writer.WriteStartObject();
            foreach (var (name, bytes) in projected)
            {
                writer.WritePropertyName(name);
                writer.WriteRawValue(bytes, skipInputValidation: true);
            }
            writer.WriteEndObject();
            writer.WriteEndObject();
        }

        var totalBytes = new FileInfo(payloadPath).Length;
        TestContext.Out.WriteLine($"{"TOTAL", -12} {totalBytes / 1048576.0, 9:F1} {totalCold, 9:F1} {totalWarm, 9:F1}");
        TestContext.Out.WriteLine($"envelope assembly: {swEnvelope.Elapsed.TotalMilliseconds:F1} ms");
        TestContext.Out.WriteLine($"envelope written to {payloadPath}");

        Assert.That(totalBytes, Is.GreaterThan(10_000_000), "envelope should be a non-trivial full-DB payload");
    }
}
