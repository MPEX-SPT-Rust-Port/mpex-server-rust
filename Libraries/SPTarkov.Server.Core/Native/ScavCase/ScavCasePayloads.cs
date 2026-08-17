using System.Text.Json.Serialization;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Common.Tables;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Native.Loot;

namespace SPTarkov.Server.Core.Native.ScavCase;

/// <summary>
/// The request/response envelopes of <c>spt_generate_scav_case_rewards</c>, mirroring
/// <c>rust/spt-native/src/scav_case/models.rs</c> member for member. Conventions are
/// <see cref="RewardLootDb"/>'s: an explicit <see cref="JsonPropertyNameAttribute"/> on every
/// member, members Rust declares as <c>Option&lt;T&gt;</c> nullable and everything else
/// <c>required</c>.
///
/// The blacklists are membership tests on the native side, so their order never reaches the RNG;
/// <see cref="ItemsView"/> and <see cref="DefaultPresetsByTpl"/> are drawn from in iteration order
/// and must be built in the order the C# generator would have walked them.
/// </summary>
public record ScavCaseRewardsRequest
{
    /// <summary>
    /// The recipe <see cref="ScavRecipes"/> is searched for. Absent from that list, the native side
    /// fails the request where the C# threw an NRE dereferencing <c>EndProducts</c>.
    /// </summary>
    [JsonPropertyName("recipeId")]
    public required MongoId RecipeId { get; set; }

    /// <summary>
    /// <c>hideoutTable.Production.ScavRecipes</c>, projected: the model's own JSON names are
    /// <c>_id</c> and capitalised <c>Common</c>/<c>Rare</c>/<c>Superrare</c>, which the native side
    /// does not bind.
    /// </summary>
    [JsonPropertyName("scavRecipes")]
    public required List<ScavCaseRecipeView> ScavRecipes { get; set; }

    /// <summary>
    /// The live <c>ScavCaseConfig</c>, sent whole: its JSON names already match the native view, and
    /// the three members that view omits - <c>kind</c>, <c>ammoRewards.ammoRewardBlacklist</c> (dead
    /// config, nothing reads it) and <c>MinMax.type</c> - are ignored on arrival.
    /// </summary>
    [JsonPropertyName("config")]
    public required ScavCaseConfig Config { get; set; }

    /// <summary>
    /// The slice of every <c>TemplateItem</c> the generator reads, keyed by tpl.
    /// </summary>
    [JsonPropertyName("itemsView")]
    public required Dictionary<MongoId, ItemView> ItemsView { get; set; }

    /// <summary>
    /// <c>RagfairPriceService.GetStaticPriceForItem(tpl)</c> per <see cref="ItemsView"/> tpl. A
    /// handbook miss is <c>0</c> on both sides, so the zeros are carried rather than filtered.
    /// </summary>
    [JsonPropertyName("staticPrices")]
    public required Dictionary<MongoId, double> StaticPrices { get; set; }

    /// <summary>
    /// <c>PresetHelper.GetDefaultPresetByTpl()</c>. A tpl absent from here is the C# <c>null</c> that
    /// warns and skips the reward.
    /// </summary>
    [JsonPropertyName("defaultPresetsByTpl")]
    public required Dictionary<MongoId, PresetView> DefaultPresetsByTpl { get; set; }

    /// <summary>
    /// <c>SeasonalEventService.GetInactiveSeasonalEventItems()</c>.
    /// </summary>
    [JsonPropertyName("inactiveSeasonalItems")]
    public required HashSet<MongoId> InactiveSeasonalItems { get; set; }

    /// <summary>
    /// <c>ItemFilterService.IsItemBlacklisted</c>'s backing cache, which mods extend at runtime
    /// through <c>AddItemToBlacklistCache</c>.
    /// </summary>
    [JsonPropertyName("globalBlacklist")]
    public required HashSet<MongoId> GlobalBlacklist { get; set; }

    /// <summary>
    /// <c>ItemFilterService.GetItemRewardBlacklist()</c> - distinct from the config's own
    /// <c>rewardItemBlacklist</c>, which rides along inside <see cref="Config"/>.
    /// </summary>
    [JsonPropertyName("rewardItemBlacklist")]
    public required HashSet<MongoId> RewardItemBlacklist { get; set; }

    /// <summary>
    /// <c>ItemFilterService.GetBossItems()</c>.
    /// </summary>
    [JsonPropertyName("bossItems")]
    public required HashSet<MongoId> BossItems { get; set; }

    /// <inheritdoc cref="RewardLootDb.TestSeed"/>
    [JsonPropertyName("testSeed")]
    public ulong? TestSeed { get; set; }
}

/// <summary>
/// <c>ScavRecipe</c> reduced to the two members the generator reads, under the names the native side
/// binds.
/// </summary>
public record ScavCaseRecipeView
{
    [JsonPropertyName("id")]
    public required MongoId Id { get; set; }

    [JsonPropertyName("endProducts")]
    public required ScavCaseEndProductsView EndProducts { get; set; }
}

/// <summary>
/// <c>EndProducts</c>, whose three members C# declares nullable and then dereferences
/// unconditionally. Non-nullable here: a recipe missing one cannot be generated at all, so the
/// builder drops it rather than sending a null the native parse would reject for the whole request.
/// </summary>
public record ScavCaseEndProductsView
{
    [JsonPropertyName("common")]
    public required MinMax<int> Common { get; set; }

    [JsonPropertyName("rare")]
    public required MinMax<int> Rare { get; set; }

    [JsonPropertyName("superrare")]
    public required MinMax<int> Superrare { get; set; }
}

/// <summary>
/// The rewards as the groups the C# builds: one list per reward, root item first.
/// </summary>
public record ScavCaseRewardsResponse
{
    [JsonPropertyName("result")]
    public required List<List<Item>> Result { get; set; }
}
