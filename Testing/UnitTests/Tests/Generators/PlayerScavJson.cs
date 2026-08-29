using System.Text.Json.Nodes;

namespace UnitTests.Tests.Generators;

/// <summary>
/// The two sanctioned masks every player scav comparison applies to a serialized scav: SavageLockTime
/// is derived from the wall clock at the moment the generation ran, and every fresh MongoId becomes a
/// positional placeholder. Shared by <see cref="PlayerScavParityTests"/> (legacy vs native) and
/// <see cref="PlayerScavResidentDbTests"/> (resident vs override). The wall-clock mask counter-asserts
/// presence: SavageLockTime is nullable and serialized WhenWritingNull, and SetScavCooldownTimer runs
/// on both arms - so if it ever stopped running, the key would vanish from both sides of every
/// comparison and no diff would notice without this throw.
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

        if (Remove(root) == 0)
        {
            throw new InvalidOperationException("no SavageLockTime to mask - did SetScavCooldownTimer stop running?");
        }

        return root.ToJsonString();
    }

    private static int Remove(JsonNode node)
    {
        var removed = 0;
        switch (node)
        {
            case JsonObject obj:
                // Materialize the keys first: mutating obj while enumerating throws.
                foreach (var key in obj.Select(pair => pair.Key).ToList())
                {
                    if (string.Equals(key, "SavageLockTime", StringComparison.OrdinalIgnoreCase))
                    {
                        obj.Remove(key);
                        removed++;
                        continue;
                    }

                    if (obj[key] is { } child)
                    {
                        removed += Remove(child);
                    }
                }
                break;
            case JsonArray array:
                foreach (var child in array)
                {
                    if (child is not null)
                    {
                        removed += Remove(child);
                    }
                }
                break;
        }

        return removed;
    }
}
