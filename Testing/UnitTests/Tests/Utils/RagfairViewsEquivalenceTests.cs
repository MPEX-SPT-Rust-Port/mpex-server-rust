using System.Text.Json;
using NUnit.Framework;
using SPTarkov.Server.Core.Helpers.Items;
using SPTarkov.Server.Core.Helpers.Profile;
using SPTarkov.Server.Core.Helpers.Traders;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Native.Ragfair;
using SPTarkov.Server.Core.Services.Items;
using SPTarkov.Server.Core.Services.Server;
using SPTarkov.Server.Core.Utils;

namespace UnitTests.Tests.Utils;

[TestFixture]
[Explicit("Phase 1 ragfair flip: writes the roots envelope and the C#-built expected views for the Rust equivalence test")]
public class RagfairViewsEquivalenceTests
{
    [Test]
    public void WriteRootsAndExpectedViews()
    {
        var di = DI.GetInstance();

        // Publishes the static JsonSerializerOptions (the SptNativeRagfairWireTests precedent)
        di.GetService<JsonUtil>();
        var options = JsonUtil.JsonSerializerOptionsNoIndent ?? throw new InvalidOperationException("JsonUtil has not been built yet.");

        var templateTable = di.GetService<TemplateTable>();
        var tradersTable = di.GetService<TradersTable>();
        var globalTable = di.GetService<GlobalTable>();

        // Built BEFORE the roots are serialized: the first handbook price lookup hydrates
        // HandbookHelper's cache, which applies ItemConfig.HandbookPriceOverride INTO
        // templateTable.Handbook itself (HandbookHelper.cs:26-49). Serializing the roots after
        // guarantees the envelope carries the same handbook the slice priced from.
        var slice = RagfairPayloadProjection.BuildInvariantSlice(
            templateTable,
            di.GetService<HandbookHelper>(),
            di.GetService<TraderHelper>(),
            di.GetService<PresetHelper>(),
            di.GetService<ItemFilterService>(),
            di.GetService<SeasonalEventService>(),
            di.GetService<BotTable>(),
            di.GetService<ItemHelper>(),
            di.GetService<BotConfig>(),
            di.GetService<RagfairConfig>()
        );

        var rootsPath = Path.Combine(Path.GetTempPath(), "spt-phase1-ragfair-roots.json");
        var roots = new (string Name, byte[] Bytes)[]
        {
            ("templates", JsonSerializer.SerializeToUtf8Bytes(templateTable, options)),
            ("traders", JsonSerializer.SerializeToUtf8Bytes(tradersTable, options)),
            ("globals", JsonSerializer.SerializeToUtf8Bytes(globalTable, options)),
        };
        using (var stream = File.Create(rootsPath))
        using (var writer = new Utf8JsonWriter(stream))
        {
            writer.WriteStartObject();
            writer.WriteNumber("schema", 1);
            writer.WritePropertyName("roots");
            writer.WriteStartObject();
            foreach (var (name, bytes) in roots)
            {
                writer.WritePropertyName(name);
                writer.WriteRawValue(bytes, skipInputValidation: true);
            }
            writer.WriteEndObject();
            writer.WriteEndObject();
        }

        // Exactly the eight views the resident DB derives natively; the slice's other members
        // (dynamic, blacklists, seasonal, pmc names) are varying-block state and deliberately absent
        var viewsPath = Path.Combine(Path.GetTempPath(), "spt-phase1-ragfair-views-expected.json");
        var views = new (string Name, byte[] Bytes)[]
        {
            ("items", JsonSerializer.SerializeToUtf8Bytes(slice.Items, options)),
            ("itemPresets", JsonSerializer.SerializeToUtf8Bytes(slice.ItemPresets, options)),
            ("defaultPresets", JsonSerializer.SerializeToUtf8Bytes(slice.DefaultPresets, options)),
            ("defaultPresetsByTpl", JsonSerializer.SerializeToUtf8Bytes(slice.DefaultPresetsByTpl, options)),
            ("presetsByTpl", JsonSerializer.SerializeToUtf8Bytes(slice.PresetsByTpl, options)),
            ("fleaPrices", JsonSerializer.SerializeToUtf8Bytes(slice.FleaPrices, options)),
            ("handbookPrices", JsonSerializer.SerializeToUtf8Bytes(slice.HandbookPrices, options)),
            ("highestTraderPrices", JsonSerializer.SerializeToUtf8Bytes(slice.HighestTraderPrices, options)),
        };
        using (var stream = File.Create(viewsPath))
        using (var writer = new Utf8JsonWriter(stream))
        {
            writer.WriteStartObject();
            foreach (var (name, bytes) in views)
            {
                writer.WritePropertyName(name);
                writer.WriteRawValue(bytes, skipInputValidation: true);
            }
            writer.WriteEndObject();
        }

        TestContext.Out.WriteLine($"roots envelope written to {rootsPath}");
        TestContext.Out.WriteLine($"expected views written to {viewsPath}");
        Assert.That(new FileInfo(rootsPath).Length, Is.GreaterThan(10_000_000), "roots envelope should be a non-trivial full-DB payload");
        Assert.That(new FileInfo(viewsPath).Length, Is.GreaterThan(1_000_000), "expected views should be non-trivial");
    }
}
