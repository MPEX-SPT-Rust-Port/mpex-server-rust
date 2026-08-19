using System.Text;
using NUnit.Framework;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Mod;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Native;
using SPTarkov.Server.Core.Native.BaseClass;
using SPTarkov.Server.Core.Native.Db;

namespace UnitTests.Tests.Services;

/// <summary>
/// Flip #3: the eligible hydrate rides the resident DB (test DI loads no mods); the per-family
/// kill switch, untrusted mods, and the frozen constructor force the views override; the trust
/// flag keeps residency live with mods; and a native-side epoch desync self-heals through one
/// republish plus retry.
/// </summary>
[TestFixture]
[NonParallelizable]
public class ItemBaseClassResidentDbTests
{
    [Test]
    public void EligibleHydrateBuildsOffTheResidentDb()
    {
        var builder = DI.GetInstance().GetService<ItemBaseClassNativeRequestBuilder>();

        var result = builder.Send();

        Assert.That(builder.LastSendIncludedViewsOverride, Is.False, "no mods loaded - the send must ride the resident DB");
        Assert.That(result.ItemBaseClasses, Is.Not.Empty);
        Assert.That(result.ItemBaseClasses.Values, Has.Some.Not.Empty);
    }

    [Test]
    public void KillSwitchForcesTheViewsOverride()
    {
        var di = DI.GetInstance();
        var itemConfig = di.GetService<ItemConfig>();
        var builder = di.GetService<ItemBaseClassNativeRequestBuilder>();

        itemConfig.DisableNativeRequestCache = true;
        try
        {
            var result = builder.Send();

            Assert.That(builder.LastSendIncludedViewsOverride, Is.True);
            Assert.That(result.ItemBaseClasses, Is.Not.Empty);
        }
        finally
        {
            itemConfig.DisableNativeRequestCache = false;
        }
    }

    [Test]
    public void ModsLoadedWithoutTheTrustFlagForceTheViewsOverride()
    {
        var di = DI.GetInstance();
        var itemConfig = di.GetService<ItemConfig>();
        // The gate only reads Count, so a placeholder element stands in for a real mod
        var modded = new ItemBaseClassNativeRequestBuilder(
            di.GetService<TemplateTable>(),
            itemConfig,
            new SptMod[] { null! },
            di.GetService<DbPublisher>()
        );

        itemConfig.TrustNativeRequestCacheWithMods = false;
        try
        {
            modded.Send();

            Assert.That(modded.LastSendIncludedViewsOverride, Is.True, "a loaded mod without the trust flag disables residency");
        }
        finally
        {
            itemConfig.TrustNativeRequestCacheWithMods = true;
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
        var itemConfig = di.GetService<ItemConfig>();
        var modded = new ItemBaseClassNativeRequestBuilder(
            di.GetService<TemplateTable>(),
            itemConfig,
            new SptMod[] { null! },
            di.GetService<DbPublisher>()
        );

        itemConfig.TrustNativeRequestCacheWithMods = true;
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
            itemConfig.TrustNativeRequestCacheWithMods = true;
        }
    }

    [Test]
    public void ABuilderBuiltOnTheFrozenConstructorAlwaysSendsTheOverride()
    {
        var frozen = new ItemBaseClassNativeRequestBuilder(DI.GetInstance().GetService<TemplateTable>());

        frozen.Send();

        Assert.That(frozen.LastSendIncludedViewsOverride, Is.True, "no publisher means no residency eligibility");
    }

    [Test]
    public void ANativeSideEpochDesyncSelfHealsThroughOneRetry()
    {
        var builder = DI.GetInstance().GetService<ItemBaseClassNativeRequestBuilder>();
        // Settle: after one eligible send the publisher's remembered epoch matches the resident DB
        builder.Send();

        // Desync: a direct native publish the publisher never sees moves the resident epoch out
        // from under the epoch it remembers
        SptNative.DbPublish(Encoding.UTF8.GetBytes("{\"schema\":1,\"roots\":{}}"));

        var result = builder.Send();

        Assert.That(builder.LastSendIncludedViewsOverride, Is.False, "the stale-epoch miss should have republished and retried");
        Assert.That(result.ItemBaseClasses, Is.Not.Empty);
    }
}
