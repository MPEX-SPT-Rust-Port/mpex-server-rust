using System.Text.Json.Nodes;

namespace UnitTests.Tests.Generators;

/// <summary>
/// Rewrites item IDs in serialized loot output to positional placeholders so the two loot paths
/// can be compared: fresh MongoIds are time/PID/counter-derived on both sides and never match.
/// Every _id value maps to "id-N" in document order of first appearance; _id, parentId and Root
/// values found in the map are rewritten; everything else (notably _tpl) is untouched.
/// </summary>
internal static class LootIdNormalizer
{
    private static readonly string[] _idFields = ["_id", "parentId", "Root"];

    internal static string Normalize(string json)
    {
        var root = JsonNode.Parse(json) ?? throw new InvalidOperationException("loot output parsed to null");
        var placeholders = new Dictionary<string, string>();

        CollectIds(root, placeholders);
        Rewrite(root, placeholders);

        return root.ToJsonString();
    }

    private static void CollectIds(JsonNode node, Dictionary<string, string> placeholders)
    {
        switch (node)
        {
            case JsonObject obj:
                foreach (var (key, child) in obj)
                {
                    if (key == "_id" && child is JsonValue value && value.TryGetValue<string>(out var id))
                    {
                        if (!placeholders.ContainsKey(id))
                        {
                            placeholders[id] = $"id-{placeholders.Count}";
                        }
                    }
                    else if (child is not null)
                    {
                        CollectIds(child, placeholders);
                    }
                }
                break;
            case JsonArray array:
                foreach (var child in array)
                {
                    if (child is not null)
                    {
                        CollectIds(child, placeholders);
                    }
                }
                break;
        }
    }

    private static void Rewrite(JsonNode node, Dictionary<string, string> placeholders)
    {
        switch (node)
        {
            case JsonObject obj:
                // Materialize the keys first: assigning obj[key] while enumerating throws.
                foreach (var key in obj.Select(pair => pair.Key).ToList())
                {
                    var child = obj[key];
                    if (
                        _idFields.Contains(key)
                        && child is JsonValue value
                        && value.TryGetValue<string>(out var id)
                        && placeholders.TryGetValue(id, out var placeholder)
                    )
                    {
                        obj[key] = placeholder;
                    }
                    else if (child is not null)
                    {
                        Rewrite(child, placeholders);
                    }
                }
                break;
            case JsonArray array:
                foreach (var child in array)
                {
                    if (child is not null)
                    {
                        Rewrite(child, placeholders);
                    }
                }
                break;
        }
    }
}
