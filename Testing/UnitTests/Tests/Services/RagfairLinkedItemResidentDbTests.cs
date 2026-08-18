using NUnit.Framework;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Native.Ragfair;

namespace UnitTests.Tests.Services;

/// <summary>
/// Flip #3: the eligible table build rides the resident DB (test DI loads no mods), and the
/// per-family kill switch forces the views override.
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
}
