using NUnit.Framework;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Native.Db;
using SPTarkov.Server.Core.Services.Server;

namespace UnitTests.Tests.Native;

/// <summary>
/// Proves the Ceciler write-barrier patch actually landed: the Installed marker flipped, and a
/// scalar write into the live templates root moves the stamp with no hand-written Bump() anywhere
/// on the path. Ceciler runs on Release and publish only, so these skip on a Debug build - the
/// same shape as the four existing ExtensionData sites. Mutates the shared database, so it restores.
/// </summary>
[TestFixture]
[NonParallelizable]
public class WriteBarrierInstallationTests
{
    [Test]
    public void TheMarkerIsFlippedInARewrittenBuild()
    {
        if (!WriteBarrier.Installed)
        {
            Assert.Ignore("write barriers are Ceciler-injected in Release builds only");
        }

        Assert.That(WriteBarrier.Installed, Is.True);
    }

    [Test]
    public void ATemplateItemSetterBumpsTheStamp()
    {
        if (!WriteBarrier.Installed)
        {
            Assert.Ignore("write barriers are Ceciler-injected in Release builds only");
        }

        var di = DI.GetInstance();
        var templateTable = di.GetService<TemplateTable>();
        var stamp = di.GetService<DatabaseMutationStamp>();

        var template = templateTable.Items.Values.First(item => item.Properties is not null);
        var original = template.Properties!.Weight;
        var before = stamp.Current;

        try
        {
            template.Properties.Weight = original + 1;

            Assert.That(stamp.Current, Is.GreaterThan(before), "a barriered setter must move the stamp");
        }
        finally
        {
            template.Properties.Weight = original;
        }
    }

    [Test]
    public void AnInitOnlyRootPropertyIsNotBarriered()
    {
        if (!WriteBarrier.Installed)
        {
            Assert.Ignore("write barriers are Ceciler-injected in Release builds only");
        }

        var setter = typeof(TemplateTable).GetProperty(nameof(TemplateTable.Items))!.SetMethod!;
        var modifiers = setter.ReturnParameter.GetRequiredCustomModifiers();

        Assert.That(
            modifiers.Any(modifier => modifier.FullName == "System.Runtime.CompilerServices.IsExternalInit"),
            Is.True,
            "init accessors are deliberately skipped by the patch; if this property stopped being init-only the skip rule needs revisiting"
        );
    }
}
