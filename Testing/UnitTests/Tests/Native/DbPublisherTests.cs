using System.Text.Json;
using NUnit.Framework;
using SPTarkov.Common.Models.Logging;
using SPTarkov.Server.Core.Helpers.Profile;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Mod;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Native;
using SPTarkov.Server.Core.Native.Db;
using SPTarkov.Server.Core.Services.Server;

namespace UnitTests.Tests.Native;

[TestFixture]
[NonParallelizable]
public class DbPublisherTests
{
    [OneTimeTearDown]
    public void OneTimeTearDown()
    {
        // These tests publish behind the DI publisher's bookkeeping; move the stamp so the next
        // EnsureCurrent() republishes real state over whatever they left resident.
        DI.GetInstance().GetService<DatabaseMutationStamp>().Bump();
    }

    [TearDown]
    public void TearDown()
    {
        DbLoadSeed.TryTake();
    }

    private static DbPublisher BuildPublisher(IReadOnlyList<SptMod> loadedMods)
    {
        var di = DI.GetInstance();

        return new DbPublisher(
            di.GetService<DatabaseMutationStamp>(),
            di.GetService<HandbookHelper>(),
            di.GetService<TemplateTable>(),
            di.GetService<TradersTable>(),
            di.GetService<GlobalTable>(),
            di.GetService<LocationTable>(),
            di.GetService<HideoutTable>(),
            di.GetService<IReadOnlyDictionary<Type, BaseConfig>>(),
            loadedMods,
            di.GetService<ISptLogger<DbPublisher>>()
        );
    }

    [Test]
    public void ASeededModlessPublisherSkipsThePublishUntilTheStampMoves()
    {
        var di = DI.GetInstance();
        var stamp = di.GetService<DatabaseMutationStamp>();

        // Make the resident DB real and current, exactly as the importer's load + configs publish
        // would, then hand its coordinates to a fresh publisher.
        var residentEpoch = di.GetService<DbPublisher>().ForcePublish();
        DbLoadSeed.Set(residentEpoch, stamp.Current);

        var seeded = BuildPublisher([]);
        Assert.That(seeded.EnsureCurrent(), Is.EqualTo(residentEpoch), "the seed is the current state");
        Assert.That(
            seeded.EnsureCurrent(),
            Is.EqualTo(residentEpoch),
            "a settled seeded publisher stays settled (the churn invariant, seeded)"
        );
        Assert.That(SptNative.DbResidentDigest().Epoch, Is.EqualTo(residentEpoch), "no publish happened");

        stamp.Bump();
        Assert.That(seeded.EnsureCurrent(), Is.GreaterThan(residentEpoch), "a moved stamp republishes as always");
    }

    [Test]
    public void AVoidedSeedRepublishesOnTheFirstEnsureCurrent()
    {
        var di = DI.GetInstance();
        var stamp = di.GetService<DatabaseMutationStamp>();

        var residentEpoch = di.GetService<DbPublisher>().ForcePublish();
        DbLoadSeed.Set(residentEpoch, stamp.Current);

        // The stamp moves between the seed and the first EnsureCurrent - the seed no longer
        // describes the tables, so it must be voided and the publish must be real (spec § Part 3's
        // tripwire).
        stamp.Bump();
        Assert.That(BuildPublisher([]).EnsureCurrent(), Is.GreaterThan(residentEpoch), "a voided seed republishes");
    }

    [Test]
    public void AModdedPublisherIgnoresTheSeed()
    {
        var di = DI.GetInstance();
        var stamp = di.GetService<DatabaseMutationStamp>();

        var residentEpoch = di.GetService<DbPublisher>().ForcePublish();
        DbLoadSeed.Set(residentEpoch, stamp.Current);

        // One loaded mod - never dereferenced, only counted (a mod can schedule
        // pre-GameCallbacks writes, and transformer registrations bump no stamp; spec § Part 3).
        var modded = BuildPublisher([null!]);
        Assert.That(modded.EnsureCurrent(), Is.GreaterThan(residentEpoch), "with mods the first call publishes");
    }

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
            di.GetService<HideoutTable>(),
            new Dictionary<Type, BaseConfig>()
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
    public void BuildPublishEnvelopeCarriesTheConfigsRootKeyedByKind()
    {
        var di = DI.GetInstance();

        var envelope = DbPayloadProjection.BuildPublishEnvelope(
            di.GetService<TemplateTable>(),
            di.GetService<TradersTable>(),
            di.GetService<GlobalTable>(),
            di.GetService<LocationTable>(),
            di.GetService<HideoutTable>(),
            // The live singletons, not fresh records: both carry required members, and these are
            // the very instances a publish projects.
            new Dictionary<Type, BaseConfig>
            {
                { typeof(RagfairConfig), di.GetService<RagfairConfig>() },
                { typeof(ItemConfig), di.GetService<ItemConfig>() },
            }
        );

        using var document = JsonDocument.Parse(envelope);
        var configs = document.RootElement.GetProperty("roots").GetProperty("configs");

        // Phase 4: one member per loaded config, keyed by its self-declared kind — nothing else.
        Assert.That(
            configs.EnumerateObject().Select(member => member.Name),
            Is.EquivalentTo(new[] { "spt-ragfair", "spt-item" }),
            "configs must carry one member per dictionary entry, keyed by Kind"
        );

        // The serialized body self-describes: it carries the same kind the key does, so the Rust
        // side can key off either.
        Assert.That(configs.GetProperty("spt-ragfair").GetProperty("kind").GetString(), Is.EqualTo("spt-ragfair"));

        // Each body must serialize as its concrete record, not as BaseConfig: dropping the
        // runtime-type argument would degenerate every body to {"kind":...} — the only member
        // BaseConfig declares — and every assertion above would still pass. RagfairConfig.Dynamic
        // exists only on the concrete record.
        Assert.That(
            configs.GetProperty("spt-ragfair").TryGetProperty("dynamic", out _),
            Is.True,
            "configs bodies must serialize as their concrete records (runtime-type overload), not as BaseConfig"
        );
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
            di.GetService<HideoutTable>(),
            new Dictionary<Type, BaseConfig>()
        );

        using var document = JsonDocument.Parse(envelope);

        // Flip #5: the hideout root carries production.scavRecipes only, serialized from the live
        // recipe objects so the models' JsonPropertyNames stay the wire authority.
        var recipes = document.RootElement.GetProperty("roots").GetProperty("hideout").GetProperty("production").GetProperty("scavRecipes");
        Assert.That(recipes.GetArrayLength(), Is.GreaterThan(0));
        Assert.That(recipes[0].TryGetProperty("_id", out _), Is.True);
        Assert.That(recipes[0].GetProperty("endProducts").TryGetProperty("Common", out _), Is.True);
    }

    [Test]
    public void BuildConfigsOnlyEnvelopeCarriesConfigsAndNoOtherRoot()
    {
        var di = DI.GetInstance();

        var envelope = DbPayloadProjection.BuildConfigsOnlyEnvelope(
            new Dictionary<Type, BaseConfig> { [typeof(RagfairConfig)] = di.GetService<RagfairConfig>() }
        );

        using var document = JsonDocument.Parse(envelope);
        Assert.That(document.RootElement.GetProperty("schema").GetInt32(), Is.EqualTo(1));
        var roots = document.RootElement.GetProperty("roots");
        Assert.That(
            roots.EnumerateObject().Select(member => member.Name),
            Is.EquivalentTo(new[] { "configs" }),
            "a configs-only envelope must name no other root - an absent root keeps the resident one"
        );
        // Runtime-type overload preserved through the extraction: a concrete-record member exists.
        Assert.That(roots.GetProperty("configs").GetProperty("spt-ragfair").TryGetProperty("dynamic", out _), Is.True);
    }

    [Test]
    public void ConfigsOnlyPublishKeepsTheOtherRootsResidentByDigest()
    {
        var di = DI.GetInstance();

        // Make every root resident, then re-publish configs alone: the five table roots must not move.
        di.GetService<DbPublisher>().ForcePublish();
        var before = SptNative.DbResidentDigest();

        SptNative.DbPublish(
            DbPayloadProjection.BuildConfigsOnlyEnvelope(
                new Dictionary<Type, BaseConfig> { [typeof(RagfairConfig)] = di.GetService<RagfairConfig>() }
            )
        );
        var after = SptNative.DbResidentDigest();

        Assert.That(after.Epoch, Is.EqualTo(before.Epoch + 1), "every publish increments the epoch");
        foreach (var root in new[] { "templates", "traders", "globals", "locations", "hideout" })
        {
            Assert.That(after.Roots[root], Is.EqualTo(before.Roots[root]), $"{root} must survive a configs-only publish");
        }
    }
}
