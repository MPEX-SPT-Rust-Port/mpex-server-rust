using NUnit.Framework;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Native;
using SPTarkov.Server.Core.Native.Ragfair;

namespace UnitTests.Tests.Services;

/// <summary>
/// Pins the wire contract between the ragfair linked item payload records and
/// <c>spt_build_ragfair_linked_item_table</c>. A misspelled <c>JsonPropertyName</c> on either side
/// empties the walk, so it fails here rather than surfacing later as a parity mismatch.
/// </summary>
[TestFixture]
public class RagfairLinkedItemWireContractTests
{
    private TemplateTable _templateTable = default!;
    private RagfairLinkedItemNativeRequestBuilder _requestBuilder = default!;

    [OneTimeSetUp]
    public void OneTimeSetUp()
    {
        var di = DI.GetInstance();

        _templateTable = di.GetService<TemplateTable>();
        _requestBuilder = di.GetService<RagfairLinkedItemNativeRequestBuilder>();
    }

    [Test]
    public void NativeBuildReturnsLinkedSetsForTheShippedTable()
    {
        var request = _requestBuilder.Build();
        var result = SptNative.BuildRagfairLinkedItemTable(request);

        // Every table template keys an entry (unlinked ones an empty set); reverse edges may
        // add keys beyond the table itself.
        Assert.That(result.LinkedItems.Count, Is.GreaterThanOrEqualTo(_templateTable.Items.Count));
        Assert.That(result.LinkedItems.Values, Has.All.Not.Null);

        // A template with a populated slot filter must come back linked.
        var slotted = _templateTable.Items.Values.First(item =>
            item.Properties?.Slots?.Any(slot => slot.Properties?.Filters?.Any(filterGroup => filterGroup.Filter?.Count > 0) == true) == true
        );
        Assert.That(result.LinkedItems[slotted.Id], Is.Not.Empty);

        // By identity, not just non-emptiness: a weapon is linked through its chambers too, so a
        // misspelled `slots` alone still leaves the set above populated.
        var slotFilterId = slotted
            .Properties!.Slots!.SelectMany(slot => slot.Properties?.Filters ?? [])
            .SelectMany(filterGroup => filterGroup.Filter ?? [])
            .First();
        Assert.That(result.LinkedItems[slotted.Id], Does.Contain(slotFilterId));
    }
}
