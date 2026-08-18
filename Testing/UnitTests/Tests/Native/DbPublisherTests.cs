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
    public void BuildPublishEnvelopeCarriesTheBaseOnlyLocationsRoot()
    {
        var di = DI.GetInstance();
        var locationTable = di.GetService<LocationTable>();

        var envelope = DbPayloadProjection.BuildPublishEnvelope(
            di.GetService<TemplateTable>(),
            di.GetService<TradersTable>(),
            di.GetService<GlobalTable>(),
            locationTable
        );

        using var document = JsonDocument.Parse(envelope);
        var locations = document.RootElement.GetProperty("roots").GetProperty("locations");

        // A known map serializes under its wire key with its own base — the raw-table key the
        // Rust quest derives look up.
        var factoryBase = locations.GetProperty("factory4_day").GetProperty("base");
        Assert.That(
            factoryBase.GetProperty("Id").GetString(),
            Is.EqualTo(locationTable.GetLocation("factory4_day")!.Base.Id),
            "factory4_day base must round-trip the table's own LocationBase"
        );

        // Base + AllExtracts only, by construction: LazyLoad members (looseLoot, staticLoot,
        // staticContainers) must never serialize, and no entry may carry anything else.
        foreach (var location in locations.EnumerateObject())
        {
            Assert.That(
                location.Value.EnumerateObject().Select(member => member.Name),
                Is.EquivalentTo(new[] { "base", "allExtracts" }),
                $"locations[{location.Name}] must carry base + allExtracts only"
            );
        }
    }
}
