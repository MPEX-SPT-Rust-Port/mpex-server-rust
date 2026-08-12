using System.Text.Json.Nodes;
using NUnit.Framework;

namespace UnitTests.Tests.Generators;

[TestFixture]
public class LootIdNormalizerTests
{
    [Test]
    public void IdsArePlaceholderedInDocumentOrder()
    {
        var json = """[{"_id":"aaaaaaaaaaaaaaaaaaaaaaaa"},{"_id":"bbbbbbbbbbbbbbbbbbbbbbbb"}]""";

        var normalized = LootIdNormalizer.Normalize(json);

        Assert.That(normalized, Is.EqualTo("""[{"_id":"id-0"},{"_id":"id-1"}]"""));
    }

    [Test]
    public void RepeatedIdsShareOnePlaceholder()
    {
        var json = """[{"_id":"aaaaaaaaaaaaaaaaaaaaaaaa"},{"_id":"aaaaaaaaaaaaaaaaaaaaaaaa"}]""";

        var normalized = LootIdNormalizer.Normalize(json);

        Assert.That(normalized, Is.EqualTo("""[{"_id":"id-0"},{"_id":"id-0"}]"""));
    }

    [Test]
    public void ParentIdReferencesFollowTheirItem()
    {
        var json = """[{"_id":"aaaaaaaaaaaaaaaaaaaaaaaa"},{"_id":"bbbbbbbbbbbbbbbbbbbbbbbb","parentId":"aaaaaaaaaaaaaaaaaaaaaaaa"}]""";

        var normalized = LootIdNormalizer.Normalize(json);

        Assert.That(
            normalized,
            Is.EqualTo("""[{"_id":"id-0"},{"_id":"id-1","parentId":"id-0"}]""")
        );
    }

    [Test]
    public void RootReferencesAreRewrittenEvenThoughRootPrecedesItemsInDocumentOrder()
    {
        // SpawnpointTemplate serializes Root before Items - the map must be built from _id
        // fields in a first pass, then applied everywhere in a second pass.
        var json = """{"Root":"aaaaaaaaaaaaaaaaaaaaaaaa","Items":[{"_id":"aaaaaaaaaaaaaaaaaaaaaaaa"}]}""";

        var normalized = LootIdNormalizer.Normalize(json);

        Assert.That(normalized, Is.EqualTo("""{"Root":"id-0","Items":[{"_id":"id-0"}]}"""));
    }

    [Test]
    public void TplValuesAndUnmappedReferencesAreUntouched()
    {
        // parentId "hideout" and a _tpl equal to no _id must pass through unchanged.
        var json = """[{"_id":"aaaaaaaaaaaaaaaaaaaaaaaa","_tpl":"5449016a4bdc2d6f028b456f","parentId":"hideout"}]""";

        var normalized = LootIdNormalizer.Normalize(json);

        Assert.That(
            normalized,
            Is.EqualTo("""[{"_id":"id-0","_tpl":"5449016a4bdc2d6f028b456f","parentId":"hideout"}]""")
        );
    }

    [Test]
    public void NonStringAndNullFieldsSurviveUntouched()
    {
        var json = """{"Id":"(2244)crate_spawn","IsContainer":true,"Position":{"x":1.5,"y":0,"z":-3},"Items":null}""";

        var normalized = LootIdNormalizer.Normalize(json);

        Assert.That(
            JsonNode.Parse(normalized)!.ToJsonString(),
            Is.EqualTo(JsonNode.Parse(json)!.ToJsonString())
        );
    }
}
