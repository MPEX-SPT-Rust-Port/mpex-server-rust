using Microsoft.Extensions.DependencyInjection;
using NUnit.Framework;
using SPTarkov.Server.Core.DI;
using SPTarkov.Server.Core.Utils;
using TestMod;

namespace UnitTests.Tests;

/// <summary>
/// Asserts the mod-pipeline guarantees modders rely on, using scenario classes from the TestMod
/// assembly. Builds its own isolated provider so the Watermark override can never leak into the
/// shared <see cref="DI" /> singleton the rest of the suite resolves from.
/// </summary>
[TestFixture]
[NonParallelizable]
public class ModCompatibilityTests
{
    private IServiceProvider _provider = default!;

    [OneTimeSetUp]
    public void BuildProviderWithTestModRegistered()
    {
        _provider = DI.BuildIsolatedProvider(typeof(TestModWatermarkOverride).Assembly);
    }

    [OneTimeTearDown]
    public void DisposeProvider()
    {
        (_provider as IDisposable)?.Dispose();
    }

    [Test]
    public void ModWithHigherTypePriority_OverridesBuiltInSingleton()
    {
        var watermark = _provider.GetRequiredService<Watermark>();

        Assert.That(watermark.GetType(), Is.EqualTo(typeof(TestModWatermarkOverride)));
    }

    [Test]
    public void ModImplementingIOnUpdate_IsRegisteredInTheUpdateCollection()
    {
        var updatables = _provider.GetServices<IOnUpdate>();

        Assert.That(updatables.Any(updatable => updatable is TestModOnUpdate), Is.True);
    }

    [Test]
    public void ModStaticRouter_IsRegisteredInTheRouterCollection()
    {
        var routers = _provider.GetServices<StaticRouter>();

        Assert.That(routers.Any(router => router.CanHandle("/testmod/ping")), Is.True);
    }

    [Test]
    public void ModHarmonyPatch_PatchesAndUnpatchesCleanly()
    {
        var target = new TestModHarmonyPatchTarget();
        var patch = new TestModHarmonyPatch();

        try
        {
            patch.Activate();
            Assert.That(target.GetValue(), Is.EqualTo(2), "postfix should rewrite the return value while the patch is active");
        }
        finally
        {
            patch.Deactivate();
        }

        Assert.That(target.GetValue(), Is.EqualTo(1), "original behavior should be restored after Disable");
    }
}
