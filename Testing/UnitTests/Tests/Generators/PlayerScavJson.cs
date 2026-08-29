using System.Text.Json.Nodes;

namespace UnitTests.Tests.Generators;

/// <summary>
/// The two sanctioned masks every player scav comparison applies to a serialized scav: SavageLockTime
/// is derived from the wall clock at the moment the generation ran, and every fresh MongoId becomes a
/// positional placeholder. Shared by <see cref="PlayerScavParityTests"/> (legacy vs native) and
/// <see cref="PlayerScavResidentDbTests"/> (resident vs override).
/// </summary>
internal static class PlayerScavJson
{
    internal static string Normalize(string json)
    {
        return LootIdNormalizer.Normalize(RemoveWallClock(json));
    }

    private static string RemoveWallClock(string json)
    {
        var root = JsonNode.Parse(json) ?? throw new InvalidOperationException("player scav output parsed to null");

        Remove(root);

        return root.ToJsonString();
    }

    private static void Remove(JsonNode node)
    {
        switch (node)
        {
            case JsonObject obj:
                // Materialize the keys first: mutating obj while enumerating throws.
                foreach (var key in obj.Select(pair => pair.Key).ToList())
                {
                    if (string.Equals(key, "SavageLockTime", StringComparison.OrdinalIgnoreCase))
                    {
                        obj.Remove(key);
                        continue;
                    }

                    if (obj[key] is { } child)
                    {
                        Remove(child);
                    }
                }
                break;
            case JsonArray array:
                foreach (var child in array)
                {
                    if (child is not null)
                    {
                        Remove(child);
                    }
                }
                break;
        }
    }
}
