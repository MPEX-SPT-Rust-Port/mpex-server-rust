using NUnit.Framework;
using SPTarkov.Server.Core.Models.Spt.Tables;
using SPTarkov.Server.Core.Native;
using SPTarkov.Server.Core.Native.ScavCase;

namespace UnitTests.Tests.Generators;

/// <summary>
/// Pins the wire contract between the scav case payload records and
/// <c>spt_generate_scav_case_rewards</c>. Everything the native side needs is required there, so a
/// misspelled <c>JsonPropertyName</c> or a member the projections drop fails the parse here rather
/// than surfacing later as a parity mismatch.
/// </summary>
[TestFixture]
public class ScavCaseWireContractTests
{
    /// <summary>
    /// The first shipped recipe asks for one rare and three to five superrare rewards, so an empty
    /// result means the reward pool never reached the far side.
    /// </summary>
    [Test]
    public void ScavCaseRewardsRoundTripThroughTheNativeLibrary()
    {
        var di = DI.GetInstance();
        var builder = di.GetService<ScavCaseNativeRequestBuilder>();
        var recipeId = di.GetService<HideoutTable>().Production.ScavRecipes!.First().Id;

        var response = SptNative.GenerateScavCaseRewards(builder.Build(recipeId, 42));

        Assert.That(response.Result, Is.Not.Empty);
        Assert.That(response.Result[0], Is.Not.Empty);
    }
}
