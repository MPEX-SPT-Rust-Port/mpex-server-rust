using System.Reflection;
using System.Text.Json;
using System.Text.Json.Serialization;
using SPTarkov.Server.Core.Models.Eft.Common;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Utils;

namespace SPTarkov.Server.Core.Native.Db;

/// <summary>
/// The full-table publish envelope of <c>spt_db_publish</c> — the roots the resident DB holds
/// (templates, traders, globals, and a locations root of Base + AllExtracts + the three statics
/// the loot flip reads; looseLoot and staticAmmo never serialize). Serialized with the server's
/// shared options so the models' <c>JsonPropertyName</c>s stay the wire authority, exactly like
/// the per-family payloads before it.
/// </summary>
internal static class DbPayloadProjection
{
    // The locations root is bounded by LocationTable.GetDictionary() (typed Location properties
    // only — never the UI-linkage "base" key), but GetDictionary keys by C# property name
    // ("Factory4Day") while the wire root is keyed by the properties' JsonPropertyNames
    // ("factory4_day") — the raw-table keys the Rust quest derives look up
    // (rust/spt-native/src/quest/views.rs build_extracts_by_location).
    private static readonly Dictionary<string, string> _locationWireKeysByProperty = typeof(LocationTable)
        .GetProperties()
        .Where(p => p.PropertyType == typeof(Location))
        .ToDictionary(p => p.Name, p => p.GetCustomAttribute<JsonPropertyNameAttribute>()!.Name);

    internal static byte[] BuildPublishEnvelope(
        TemplateTable templateTable,
        TradersTable tradersTable,
        GlobalTable globalTable,
        LocationTable locationTable
    )
    {
        var options = JsonUtil.JsonSerializerOptionsNoIndent ?? throw new InvalidOperationException("JsonUtil has not been built yet.");

        using var stream = new MemoryStream();
        using (var writer = new Utf8JsonWriter(stream))
        {
            writer.WriteStartObject();
            writer.WriteNumber("schema", 1);
            writer.WritePropertyName("roots");
            writer.WriteStartObject();
            writer.WritePropertyName("templates");
            writer.WriteRawValue(JsonSerializer.SerializeToUtf8Bytes(templateTable, options), skipInputValidation: true);
            writer.WritePropertyName("traders");
            writer.WriteRawValue(JsonSerializer.SerializeToUtf8Bytes(tradersTable, options), skipInputValidation: true);
            writer.WritePropertyName("globals");
            writer.WriteRawValue(JsonSerializer.SerializeToUtf8Bytes(globalTable, options), skipInputValidation: true);
            writer.WritePropertyName("locations");
            writer.WriteStartObject();
            foreach (var (propertyName, location) in locationTable.GetDictionary())
            {
                // Base + AllExtracts + the three statics, by construction. A null Base still
                // ships as "base": null (the Rust model tolerates it; the derives skip it), while
                // a null AllExtracts collapses to [] — the Rust Vec rejects an explicit null.
                writer.WritePropertyName(_locationWireKeysByProperty[propertyName]);
                writer.WriteStartObject();
                writer.WritePropertyName("base");
                writer.WriteRawValue(JsonSerializer.SerializeToUtf8Bytes(location?.Base, options), skipInputValidation: true);
                writer.WritePropertyName("allExtracts");
                writer.WriteRawValue(JsonSerializer.SerializeToUtf8Bytes(location?.AllExtracts ?? [], options), skipInputValidation: true);
                // Flip #4: the three statics the loot family reads, serialized as each
                // LazyLoad.Value so registered transformers (ReduceStaticItemWeight, seasonal)
                // are applied — the same data the per-call path read. looseLoot must still
                // never serialize (549 MiB resident was rejected; the per-call splice stays
                // until Phase 3), and staticAmmo stays a per-call parameter.
                writer.WritePropertyName("staticLoot");
                writer.WriteRawValue(JsonSerializer.SerializeToUtf8Bytes(location?.StaticLoot?.Value, options), skipInputValidation: true);
                writer.WritePropertyName("staticContainers");
                writer.WriteRawValue(
                    JsonSerializer.SerializeToUtf8Bytes(location?.StaticContainers?.Value, options),
                    skipInputValidation: true
                );
                writer.WritePropertyName("statics");
                writer.WriteRawValue(JsonSerializer.SerializeToUtf8Bytes(location?.Statics, options), skipInputValidation: true);
                writer.WriteEndObject();
            }
            writer.WriteEndObject();
            writer.WriteEndObject();
            writer.WriteEndObject();
        }

        return stream.ToArray();
    }
}
