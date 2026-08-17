using NUnit.Framework;
using SPTarkov.Server.Core.Native;
using SPTarkov.Server.Core.Native.BaseClass;

namespace UnitTests.Tests.Services;

/// <summary>
/// Pins the wire contract between the item base class payload records and
/// <c>spt_build_item_base_class_cache</c>. A misspelled <c>JsonPropertyName</c> on either side
/// empties the walk, so it fails here rather than surfacing later as a parity mismatch.
/// </summary>
[TestFixture]
public class ItemBaseClassWireContractTests
{
    [Test]
    public void NativeBuildReturnsChainsForTheShippedTable()
    {
        var requestBuilder = DI.GetInstance().GetService<ItemBaseClassNativeRequestBuilder>();

        var request = requestBuilder.Build();
        var result = SptNative.BuildItemBaseClassCache(request);

        Assert.That(result.ItemBaseClasses, Is.Not.Empty);
        Assert.That(result.RootNodeIds, Is.Not.Empty);
        // A misspelled `parent` would still fill the map, with every chain empty.
        Assert.That(result.ItemBaseClasses.Values, Has.Some.Not.Empty);
    }
}
