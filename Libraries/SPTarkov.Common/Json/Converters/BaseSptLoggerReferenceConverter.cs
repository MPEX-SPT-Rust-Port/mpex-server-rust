using System.Text.Json;
using System.Text.Json.Serialization;
using SPTarkov.Common.Models.Logging;

namespace SPTarkov.Common.Json.Converters;

public sealed class BaseSptLoggerReferenceConverter : JsonConverter<BaseSptLoggerReference>
{
    public override BaseSptLoggerReference? Read(ref Utf8JsonReader reader, Type typeToConvert, JsonSerializerOptions options)
    {
        using (var jsonDocument = JsonDocument.ParseValue(ref reader))
        {
            if (!jsonDocument.RootElement.TryGetProperty("type", out var typeElement))
            {
                throw new Exception("One of the loggers doesnt have a type property defined.");
            }

            return typeElement.GetString() switch
            {
                "File" => jsonDocument.Deserialize<FileSptLoggerReference>(options),
                "Console" => jsonDocument.Deserialize<ConsoleSptLoggerReference>(options),
                _ => throw new Exception($"The logger type '{typeElement.GetString()}' does not exist."),
            };
        }
    }

    public override void Write(Utf8JsonWriter writer, BaseSptLoggerReference value, JsonSerializerOptions options)
    {
        writer.WriteStartObject();
        writer.WriteString("type", value.Type.ToString());
        writer.WriteString("logLevel", value.LogLevel.ToString());
        writer.WriteString("format", value.Format);
        writer.WritePropertyName("filters");
        JsonSerializer.Serialize(writer, value.Filters, options);

        if (value is FileSptLoggerReference file)
        {
            writer.WriteString("filePath", file.FilePath);
            writer.WriteString("filePattern", file.FilePattern);
            writer.WriteNumber("maxFileSizeMB", file.MaxFileSizeMb);
            writer.WriteNumber("maxRollingFiles", file.MaxRollingFiles);
        }

        writer.WriteEndObject();
    }
}
