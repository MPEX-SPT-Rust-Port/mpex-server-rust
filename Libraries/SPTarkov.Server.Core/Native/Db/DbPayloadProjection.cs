using System.Text.Json;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Utils;

namespace SPTarkov.Server.Core.Native.Db;

/// <summary>
/// The full-table publish envelope of <c>spt_db_publish</c> — the roots the resident DB holds
/// (flip #1: templates, traders, globals). Serialized with the server's shared options so the
/// models' <c>JsonPropertyName</c>s stay the wire authority, exactly like the per-family
/// payloads before it.
/// </summary>
internal static class DbPayloadProjection
{
    internal static byte[] BuildPublishEnvelope(TemplateTable templateTable, TradersTable tradersTable, GlobalTable globalTable)
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
            writer.WriteEndObject();
            writer.WriteEndObject();
        }

        return stream.ToArray();
    }
}
