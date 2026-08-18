using System.Reflection;
using System.Text.Json;
using System.Text.Json.Serialization;
using SPTarkov.Server.Core.Models.Eft.Common;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Utils;

namespace SPTarkov.Server.Core.Native.Db;

/// <summary>
/// The full-table publish envelope of <c>spt_db_publish</c> — the roots the resident DB holds
/// (templates, traders, globals, and the Base-only locations root). Serialized with the server's
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
                // Base + AllExtracts only, by construction — the LazyLoad members (looseLoot,
                // staticLoot, staticContainers) must never serialize. A null Base still ships as
                // "base": null (the Rust model tolerates it; the derives skip it), while a null
                // AllExtracts collapses to [] — the Rust Vec rejects an explicit null.
                writer.WritePropertyName(_locationWireKeysByProperty[propertyName]);
                writer.WriteStartObject();
                writer.WritePropertyName("base");
                writer.WriteRawValue(JsonSerializer.SerializeToUtf8Bytes(location?.Base, options), skipInputValidation: true);
                writer.WritePropertyName("allExtracts");
                writer.WriteRawValue(JsonSerializer.SerializeToUtf8Bytes(location?.AllExtracts ?? [], options), skipInputValidation: true);
                writer.WriteEndObject();
            }
            writer.WriteEndObject();
            writer.WriteEndObject();
            writer.WriteEndObject();
        }

        return stream.ToArray();
    }
}
