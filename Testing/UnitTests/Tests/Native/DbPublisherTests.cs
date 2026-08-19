using System.Text.Json;
using NUnit.Framework;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Native.Db;
using SPTarkov.Server.Core.Services.Server;

namespace UnitTests.Tests.Native;

[TestFixture]
[NonParallelizable]
public class DbPublisherTests
{
    [Test]
    public void EnsureCurrentPublishesOncePerStampAndRepublishesOnBump()
    {
        var di = DI.GetInstance();
        var publisher = di.GetService<DbPublisher>();
        var stamp = di.GetService<DatabaseMutationStamp>();

        // Epochs are process-global and other fixtures may publish too: assert relative
        // movement, never absolute values.
        var first = publisher.EnsureCurrent();
        var second = publisher.EnsureCurrent();
        Assert.That(second, Is.EqualTo(first), "no stamp movement, no republish");

        stamp.Bump();
        var third = publisher.EnsureCurrent();
        Assert.That(third, Is.GreaterThan(second), "a bumped stamp republishes");

        Assert.That(publisher.ForcePublish(), Is.GreaterThan(third));
    }

    [Test]
    public void BuildPublishEnvelopeCarriesTheStaticsBearingLocationsRoot()
    {
        var di = DI.GetInstance();
        var locationTable = di.GetService<LocationTable>();

        var envelope = DbPayloadProjection.BuildPublishEnvelope(
            di.GetService<TemplateTable>(),
            di.GetService<TradersTable>(),
            di.GetService<GlobalTable>(),
            locationTable,
            di.GetService<HideoutTable>()
        );

        using var document = JsonDocument.Parse(envelope);
        var locations = document.RootElement.GetProperty("roots").GetProperty("locations");

        // A known map serializes under its wire key with its own base — the raw-table key the
        // Rust quest derives look up.
        var factory = locations.GetProperty("factory4_day");
        Assert.That(
            factory.GetProperty("base").GetProperty("Id").GetString(),
            Is.EqualTo(locationTable.GetLocation("factory4_day")!.Base.Id),
            "factory4_day base must round-trip the table's own LocationBase"
        );

        // Flip #4: a loot-bearing entry carries the three statics the loot family reads.
        Assert.That(factory.GetProperty("staticLoot").ValueKind, Is.EqualTo(JsonValueKind.Object));
        Assert.That(factory.GetProperty("staticContainers").ValueKind, Is.EqualTo(JsonValueKind.Object));
        Assert.That(factory.GetProperty("statics").ValueKind, Is.EqualTo(JsonValueKind.Object));

        // Base + AllExtracts + the three statics, by construction: looseLoot must never serialize
        // (549 MiB resident was rejected), staticAmmo stays a per-call parameter, and no entry may
        // carry anything else.
        foreach (var location in locations.EnumerateObject())
        {
            Assert.That(
                location.Value.EnumerateObject().Select(member => member.Name),
                Is.EquivalentTo(new[] { "base", "allExtracts", "staticLoot", "staticContainers", "statics" }),
                $"locations[{location.Name}] must carry base + allExtracts + the three statics only"
            );
        }
    }

    [Test]
    public void BuildPublishEnvelopeCarriesTheScavRecipesBearingHideoutRoot()
    {
        var di = DI.GetInstance();

        var envelope = DbPayloadProjection.BuildPublishEnvelope(
            di.GetService<TemplateTable>(),
            di.GetService<TradersTable>(),
            di.GetService<GlobalTable>(),
            di.GetService<LocationTable>(),
            di.GetService<HideoutTable>()
        );

        using var document = JsonDocument.Parse(envelope);

        // Flip #5: the hideout root carries production.scavRecipes only, serialized from the live
        // recipe objects so the models' JsonPropertyNames stay the wire authority.
        var recipes = document.RootElement.GetProperty("roots").GetProperty("hideout").GetProperty("production").GetProperty("scavRecipes");
        Assert.That(recipes.GetArrayLength(), Is.GreaterThan(0));
        Assert.That(recipes[0].TryGetProperty("_id", out _), Is.True);
        Assert.That(recipes[0].GetProperty("endProducts").TryGetProperty("Common", out _), Is.True);
    }
}
