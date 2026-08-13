using System.Text.Json.Nodes;
using NUnit.Framework;

namespace UnitTests.Tests.Generators;

/// <summary>
/// Deep-equality assertion for the loot parity gates. Shared by the location loot and reward loot
/// fixtures: both compare two normalized loot documents and both need the first difference named
/// rather than a two-megabyte diff dumped into the test log.
/// </summary>
internal static class LootJsonAssert
{
    /// <param name="label">What was generated, e.g. <c>map=bigmap</c> - prefixed to the failure message</param>
    internal static void AssertEqual(string legacy, string native, string label, ulong seed)
    {
        if (legacy == native)
        {
            return;
        }

        var (path, legacyValue, nativeValue) = FirstDifference(JsonNode.Parse(legacy), JsonNode.Parse(native), "$");
        if (path.Length == 0)
        {
            Assert.Fail(
                $"loot parity failure {label} seed={seed}: normalized strings differ "
                    + $"(legacy {legacy.Length} chars, native {native.Length} chars) but the walker "
                    + "found no structural difference - suspect duplicate or reordered keys"
            );
        }

        Assert.Fail($"loot parity failure {label} seed={seed} at {path}\n  legacy: {legacyValue}\n  native: {nativeValue}");
    }

    /// <summary>
    /// Walks both documents to the first structural or value difference. Returns the JSON path
    /// and short renderings of both sides - a readable report instead of a two-megabyte diff.
    /// </summary>
    private static (string Path, string Legacy, string Native) FirstDifference(JsonNode? legacy, JsonNode? native, string path)
    {
        if (legacy is null && native is null)
        {
            return ("", "", "");
        }

        if (legacy is null || native is null)
        {
            return (path, Render(legacy), Render(native));
        }

        if (legacy is JsonObject legacyObj && native is JsonObject nativeObj)
        {
            foreach (var (key, legacyChild) in legacyObj)
            {
                if (!nativeObj.ContainsKey(key))
                {
                    return ($"{path}.{key}", Render(legacyChild), "<missing>");
                }

                var difference = FirstDifference(legacyChild, nativeObj[key], $"{path}.{key}");
                if (difference.Path.Length > 0)
                {
                    return difference;
                }
            }

            foreach (var (key, nativeChild) in nativeObj)
            {
                if (!legacyObj.ContainsKey(key))
                {
                    return ($"{path}.{key}", "<missing>", Render(nativeChild));
                }
            }

            return ("", "", "");
        }

        if (legacy is JsonArray legacyArray && native is JsonArray nativeArray)
        {
            var shared = Math.Min(legacyArray.Count, nativeArray.Count);
            for (var i = 0; i < shared; i++)
            {
                var difference = FirstDifference(legacyArray[i], nativeArray[i], $"{path}[{i}]");
                if (difference.Path.Length > 0)
                {
                    return difference;
                }
            }

            if (legacyArray.Count != nativeArray.Count)
            {
                return ($"{path}.length", legacyArray.Count.ToString(), nativeArray.Count.ToString());
            }

            return ("", "", "");
        }

        var legacyJson = legacy.ToJsonString();
        var nativeJson = native.ToJsonString();

        return legacyJson == nativeJson ? ("", "", "") : (path, Truncate(legacyJson), Truncate(nativeJson));
    }

    private static string Render(JsonNode? node)
    {
        return node is null ? "null" : Truncate(node.ToJsonString());
    }

    private static string Truncate(string value)
    {
        return value.Length <= 200 ? value : value[..200] + "...";
    }
}
