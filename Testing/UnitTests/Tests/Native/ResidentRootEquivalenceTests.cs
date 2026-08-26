using NUnit.Framework;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Native;
using SPTarkov.Server.Core.Native.Db;
using SPTarkov.Server.Core.Services.Server;
using SPTarkov.Server.Core.Utils;
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
        // next EnsureCurrent() republishes for whichever fixture runs after this one.
        DI.GetInstance().GetService<DatabaseMutationStamp>().Bump();
    }

    [Test]
    public async Task LoadInstalledRootsMatchAProjectionPublishOfTheSameTree()
    {
        var di = DI.GetInstance();
        var importerUtil = di.GetService<ImporterUtil>();

        var load = SptNative.DbLoad(SptDataPath, verify: false);
        var loadDigests = SptNative.DbResidentDigest();
        Assert.That(loadDigests.Epoch, Is.GreaterThan(0UL), "the load must leave a resident DB");
        Assert.That(loadDigests.Roots.Keys, Is.SupersetOf(LoadInstalledRoots), "the load must install all five roots");

        // No handbook hydration on this side either: the raw-file mappings are what compare here
        // (Task 6 extends this test with the override merge).
        var tables = await importerUtil.LoadRecursiveAsync<DatabaseTables>($"{SptDataPath}database/", load.Files);
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
