using NUnit.Framework;
using SPTarkov.Common.Models.Logging;
using SPTarkov.Server.Core.Helpers.Profile;
using SPTarkov.Server.Core.Models.Enums;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Native;
using SPTarkov.Server.Core.Native.Db;
using SPTarkov.Server.Core.Services.Server;
using SPTarkov.Server.Core.Utils;
using SPTarkov.Server.Core.Utils.Cloners;
using SPTarkov.Server.Helpers;

namespace UnitTests.Tests.Native;

/// <summary>
/// The load-epoch seed's gate: the resident roots spt_db_load installs from raw file bytes must
/// equal the roots a DbPayloadProjection publish of the same tree installs — compared post-parse
/// over the typed lift surface via spt_db_resident_digest (extra maps are excluded: the envelope
/// texts legitimately differ in member order, number formatting, explicit nulls, and Debug-build
/// model coverage, and all of that rides extra). Red here means the two file→wire mappings
/// diverged on something a Rust consumer reads; fix the mapping, do not weaken the gate
/// (spec § Part 0).
///
/// The load arm sends ItemConfig.HandbookPriceOverride and the publish arm hydrates the same
/// overrides through HandbookHelper, so the merge compares too — but only the half the digest
/// sees: the overridden Price values, the appended Ids, and where an appended entry lands.
/// HandbookItem.ParentId rides the Rust extra map, which digest mode skips, so this gate does
/// not cover the ParentId half of the merge.
/// </summary>
[TestFixture]
[NonParallelizable]
public class ResidentRootEquivalenceTests
{
    private const string SptDataPath = "./SPT_Data/";

    private static readonly string[] LoadInstalledRoots = ["templates", "traders", "globals", "locations", "hideout"];

    [OneTimeTearDown]
    public void OneTimeTearDown()
    {
        // Both arms of this test publish behind DbPublisher's bookkeeping; move the stamp so the
        // next EnsureCurrent() republishes for whichever fixture runs after this one. That
        // republish also restores the configs root this fixture leaves resident-but-empty (the
        // projection arm publishes an empty configs dictionary, and a present root replaces rather
        // than carries forward) - a fixture reading via SptNative with a previously captured epoch
        // would see empty configs as a wrong answer, not an exception, so route through
        // DbPublisher.EnsureCurrent() as every current fixture does.
        DI.GetInstance().GetService<DatabaseMutationStamp>().Bump();
    }

    [Test]
    public async Task LoadInstalledRootsMatchAProjectionPublishOfTheSameTree()
    {
        var di = DI.GetInstance();
        var importerUtil = di.GetService<ImporterUtil>();

        var itemConfig = di.GetService<ItemConfig>();
        Assert.That(itemConfig.HandbookPriceOverride, Is.Not.Empty, "the shipped tree must exercise the override merge");

        var load = SptNative.DbLoad(SptDataPath, verify: false, itemConfig.HandbookPriceOverride);
        var loadDigests = SptNative.DbResidentDigest();
        Assert.That(loadDigests.Epoch, Is.GreaterThan(0UL), "the load must leave a resident DB");
        Assert.That(loadDigests.Roots.Keys, Is.SupersetOf(LoadInstalledRoots), "the load must install all five roots");

        var tables = await importerUtil.LoadRecursiveAsync<DatabaseTables>($"{SptDataPath}database/", load.Files);

        // The returned files are raw, so this arm must reproduce PublishLocked's forced
        // hydration itself: HydrateHandbookCache upserts the overrides into
        // tables.Templates.Handbook exactly as the Rust merge did at load time.
        var rawHandbookIds = tables.Templates.Handbook.Items.Select(item => item.Id).ToHashSet();
        Assert.That(
            itemConfig.HandbookPriceOverride.Keys.Any(id => !rawHandbookIds.Contains(id)),
            Is.True,
            "at least one override must be absent from the raw handbook - the append path is the ordering property this gate pins"
        );

        var handbookHelper = new HandbookHelper(
            di.GetService<ISptLogger<HandbookHelper>>(),
            tables.Templates,
            itemConfig,
            di.GetService<ICloner>()
        );
        handbookHelper.IsCategory(Money.ROUBLES);

        var envelope = DbPayloadProjection.BuildPublishEnvelope(
            tables.Templates,
            tables.Traders,
            tables.Globals,
            tables.Locations,
            tables.Hideout,
            new Dictionary<Type, BaseConfig>()
        );
        SptNative.DbPublish(envelope);
        var publishDigests = SptNative.DbResidentDigest();
        Assert.That(publishDigests.Epoch, Is.GreaterThan(loadDigests.Epoch), "the projection publish must supersede the load");

        foreach (var root in LoadInstalledRoots)
        {
            Assert.That(
                publishDigests.Roots[root],
                Is.EqualTo(loadDigests.Roots[root]),
                $"{root}: the load-installed root differs from a DbPayloadProjection publish of the same tree"
            );
        }
    }
}
