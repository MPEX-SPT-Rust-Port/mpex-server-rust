using System.Text.Json;
using NUnit.Framework;
using SPTarkov.Server.Core.Helpers.Profile;
using SPTarkov.Server.Core.Models.Enums;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Native.BaseClass;
using SPTarkov.Server.Core.Native.Db;
using SPTarkov.Server.Core.Native.Ragfair;
using SPTarkov.Server.Core.Utils;

namespace UnitTests.Tests.Utils;

[TestFixture]
[Explicit("Phase 1 flip #3: writes the publish envelope and the C#-built override payloads for the Rust equivalence test")]
public class OneShotViewsEquivalenceTests
{
    [Test]
    public void WriteTemplatesRootAndOverridePayloads()
    {
        var di = DI.GetInstance();

        di.GetService<JsonUtil>();
        var options = JsonUtil.JsonSerializerOptionsNoIndent ?? throw new InvalidOperationException("JsonUtil has not been built yet.");

        // The same lazy hydration DbPublisher forces before a live publish, so the envelope here
        // is byte-identical to one the server would send (HandbookHelper's first use writes
        // ItemConfig.HandbookPriceOverride entries INTO templateTable.Handbook)
        di.GetService<HandbookHelper>().IsCategory(Money.ROUBLES);

        var rootsPath = Path.Combine(Path.GetTempPath(), "spt-flip3-roots.json");
        File.WriteAllBytes(
            rootsPath,
            DbPayloadProjection.BuildPublishEnvelope(
                di.GetService<TemplateTable>(),
                di.GetService<TradersTable>(),
                di.GetService<GlobalTable>(),
                di.GetService<LocationTable>(),
                di.GetService<HideoutTable>()
            )
        );

        var baseClassPath = Path.Combine(Path.GetTempPath(), "spt-flip3-baseclass-override.json");
        File.WriteAllBytes(
            baseClassPath,
            JsonSerializer.SerializeToUtf8Bytes(di.GetService<ItemBaseClassNativeRequestBuilder>().Build(), options)
        );

        var linkedPath = Path.Combine(Path.GetTempPath(), "spt-flip3-linkeditems-override.json");
        File.WriteAllBytes(
            linkedPath,
            JsonSerializer.SerializeToUtf8Bytes(di.GetService<RagfairLinkedItemNativeRequestBuilder>().Build(), options)
        );

        TestContext.Out.WriteLine($"roots envelope written to {rootsPath}");
        TestContext.Out.WriteLine($"base class override written to {baseClassPath}");
        TestContext.Out.WriteLine($"linked items override written to {linkedPath}");
        Assert.That(new FileInfo(rootsPath).Length, Is.GreaterThan(10_000_000), "roots envelope should be a non-trivial full-DB payload");
        Assert.That(new FileInfo(baseClassPath).Length, Is.GreaterThan(100_000), "base class override should cover the whole table");
        Assert.That(new FileInfo(linkedPath).Length, Is.GreaterThan(100_000), "linked items override should cover the whole table");
    }
}
