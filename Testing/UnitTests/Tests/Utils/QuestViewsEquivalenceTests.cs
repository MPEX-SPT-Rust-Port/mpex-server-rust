using System.Text.Json;
using NUnit.Framework;
using SPTarkov.Server.Core.Helpers.Items;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Native.Db;
using SPTarkov.Server.Core.Native.RepeatableQuests;
using SPTarkov.Server.Core.Utils;

namespace UnitTests.Tests.Utils;

[TestFixture]
[Explicit("Phase 1 quest flip: writes the four-root publish envelope and the C#-built expected views for the Rust equivalence test")]
public class QuestViewsEquivalenceTests
{
    [Test]
    public void WriteQuestRootsAndExpectedViews()
    {
        var di = DI.GetInstance();

        // Publishes the static JsonSerializerOptions (the SptNativeRagfairWireTests precedent)
        di.GetService<JsonUtil>();
        var options = JsonUtil.JsonSerializerOptionsNoIndent ?? throw new InvalidOperationException("JsonUtil has not been built yet.");

        // Hydrate the weapon/equipment default caches the way the live server does before any
        // quest can generate: GetDefaultPresetOrItemPrice's Rust derivation matches the
        // hydrated-cache semantics only - a virgin PresetHelper would first-preset-fallback the
        // whole price map (PresetHelper.cs:232-238)
        di.GetService<PresetHelper>().GetDefaultPresets();

        // Built BEFORE the roots are serialized: the first handbook price lookup hydrates
        // HandbookHelper's cache, which applies ItemConfig.HandbookPriceOverride INTO
        // templateTable.Handbook itself (HandbookHelper.cs:26-49). Serializing the roots after
        // guarantees the envelope carries the same handbook the views priced from.
        var views = di.GetService<RepeatableQuestNativeRequestBuilder>().BuildViewsOverride();

        var rootsPath = Path.Combine(Path.GetTempPath(), "spt-phase1-quest-roots.json");
        File.WriteAllBytes(
            rootsPath,
            DbPayloadProjection.BuildPublishEnvelope(
                di.GetService<TemplateTable>(),
                di.GetService<TradersTable>(),
                di.GetService<GlobalTable>(),
                di.GetService<LocationTable>(),
                di.GetService<HideoutTable>()
            )
        );

        // Exactly the ten views the resident DB derives natively - the views override carries
        // them and nothing else
        var viewsPath = Path.Combine(Path.GetTempPath(), "spt-phase1-quest-views-expected.json");
        var expectedViews = new (string Name, byte[] Bytes)[]
        {
            ("items", JsonSerializer.SerializeToUtf8Bytes(views.Items, options)),
            ("handbookPrices", JsonSerializer.SerializeToUtf8Bytes(views.HandbookPrices, options)),
            ("fleaPrices", JsonSerializer.SerializeToUtf8Bytes(views.FleaPrices, options)),
            ("defaultWeaponPresets", JsonSerializer.SerializeToUtf8Bytes(views.DefaultWeaponPresets, options)),
            ("defaultPresetOrItemPrices", JsonSerializer.SerializeToUtf8Bytes(views.DefaultPresetOrItemPrices, options)),
            ("repeatableQuestTemplates", JsonSerializer.SerializeToUtf8Bytes(views.RepeatableQuestTemplates, options)),
            ("completionItemsWhitelist", JsonSerializer.SerializeToUtf8Bytes(views.CompletionItemsWhitelist, options)),
            ("completionItemsBlacklist", JsonSerializer.SerializeToUtf8Bytes(views.CompletionItemsBlacklist, options)),
            ("bossSpawnsByLocation", JsonSerializer.SerializeToUtf8Bytes(views.BossSpawnsByLocation, options)),
            ("extractsByLocation", JsonSerializer.SerializeToUtf8Bytes(views.ExtractsByLocation, options)),
        };
        using (var stream = File.Create(viewsPath))
        using (var writer = new Utf8JsonWriter(stream))
        {
            writer.WriteStartObject();
            foreach (var (name, bytes) in expectedViews)
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
