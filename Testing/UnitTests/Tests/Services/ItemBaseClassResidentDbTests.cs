using NUnit.Framework;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Native.BaseClass;

namespace UnitTests.Tests.Services;

/// <summary>
/// Flip #3: the eligible hydrate rides the resident DB (test DI loads no mods), and the
/// per-family kill switch forces the views override.
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
}
