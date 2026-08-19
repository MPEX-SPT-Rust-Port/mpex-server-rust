using System.Text;
using NUnit.Framework;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Mod;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Native;
using SPTarkov.Server.Core.Native.Db;
using SPTarkov.Server.Core.Native.Ragfair;

namespace UnitTests.Tests.Services;

/// <summary>
/// Flip #3: the eligible table build rides the resident DB (test DI loads no mods); the
/// per-family kill switch, untrusted mods, and the frozen constructor force the views override;
/// the trust flag keeps residency live with mods; and a native-side epoch desync self-heals
/// through one republish plus retry.
/// </summary>
[TestFixture]
[NonParallelizable]
public class RagfairLinkedItemResidentDbTests
{
    [Test]
    public void EligibleBuildRidesTheResidentDb()
    {
        var builder = DI.GetInstance().GetService<RagfairLinkedItemNativeRequestBuilder>();

        var result = builder.Send();

        Assert.That(builder.LastSendIncludedViewsOverride, Is.False, "no mods loaded - the send must ride the resident DB");
        Assert.That(result.LinkedItems, Is.Not.Empty);
        Assert.That(result.LinkedItems.Values, Has.Some.Not.Empty);
    }

    [Test]
    public void KillSwitchForcesTheViewsOverride()
    {
        var di = DI.GetInstance();
        var ragfairConfig = di.GetService<RagfairConfig>();
        var builder = di.GetService<RagfairLinkedItemNativeRequestBuilder>();

        ragfairConfig.DisableNativeRequestCache = true;
        try
        {
            var result = builder.Send();

            Assert.That(builder.LastSendIncludedViewsOverride, Is.True);
            Assert.That(result.LinkedItems, Is.Not.Empty);
        }
        finally
        {
            ragfairConfig.DisableNativeRequestCache = false;
        }
    }

    [Test]
    public void ModsLoadedWithoutTheTrustFlagForceTheViewsOverride()
    {
        var di = DI.GetInstance();
        var ragfairConfig = di.GetService<RagfairConfig>();
        // The gate only reads Count, so a placeholder element stands in for a real mod
        var modded = new RagfairLinkedItemNativeRequestBuilder(
            di.GetService<TemplateTable>(),
            ragfairConfig,
            new SptMod[] { null! },
            di.GetService<DbPublisher>()
        );

        ragfairConfig.TrustNativeRequestCacheWithMods = false;
        try
        {
            modded.Send();

            Assert.That(modded.LastSendIncludedViewsOverride, Is.True, "a loaded mod without the trust flag disables residency");
        }
        finally
        {
            ragfairConfig.TrustNativeRequestCacheWithMods = true;
        }
    }

    [Test]
    public void TheTrustFlagKeepsTheResidentPathLiveWithModsLoaded()
    {
        if (!WriteBarrier.Installed)
        {
            Assert.Ignore("write barriers are Ceciler-injected in Release builds only");
        }

        var di = DI.GetInstance();
        var ragfairConfig = di.GetService<RagfairConfig>();
        var modded = new RagfairLinkedItemNativeRequestBuilder(
            di.GetService<TemplateTable>(),
            ragfairConfig,
            new SptMod[] { null! },
            di.GetService<DbPublisher>()
        );

        ragfairConfig.TrustNativeRequestCacheWithMods = true;
        try
        {
            modded.Send();

            Assert.That(
                modded.LastSendIncludedViewsOverride,
                Is.False,
                "the trust flag should keep the resident path live despite the mod"
            );
        }
        finally
        {
            ragfairConfig.TrustNativeRequestCacheWithMods = true;
        }
    }

    [Test]
    public void ABuilderBuiltOnTheFrozenConstructorAlwaysSendsTheOverride()
    {
        var frozen = new RagfairLinkedItemNativeRequestBuilder(DI.GetInstance().GetService<TemplateTable>());

        frozen.Send();

        Assert.That(frozen.LastSendIncludedViewsOverride, Is.True, "no publisher means no residency eligibility");
    }

    [Test]
    public void ANativeSideEpochDesyncSelfHealsThroughOneRetry()
    {
        var builder = DI.GetInstance().GetService<RagfairLinkedItemNativeRequestBuilder>();
        // Settle: after one eligible send the publisher's remembered epoch matches the resident DB
        builder.Send();

        // Desync: a direct native publish the publisher never sees moves the resident epoch out
        // from under the epoch it remembers
        SptNative.DbPublish(Encoding.UTF8.GetBytes("{\"schema\":1,\"roots\":{}}"));

        var result = builder.Send();

        Assert.That(builder.LastSendIncludedViewsOverride, Is.False, "the stale-epoch miss should have republished and retried");
        Assert.That(result.LinkedItems, Is.Not.Empty);
    }
}
