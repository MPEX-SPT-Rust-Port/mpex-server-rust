using System.Text.Json.Serialization;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Common.Tables;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Services;

namespace SPTarkov.Server.Core.Native.Loot;

/// <summary>
/// The request/response envelopes of the four native reward loot exports, mirroring the
/// reward-loot section of <c>rust/spt-native/src/loot/models.rs</c> member for member. Conventions
/// are <see cref="LootCommon"/>'s: an explicit <see cref="JsonPropertyNameAttribute"/> on every
/// member, members Rust declares as <c>Option&lt;T&gt;</c> nullable and everything else
/// <c>required</c>.
///
/// The database slice every entry point needs, flattened into each request the way
/// <see cref="LootCommon"/> is. All five blacklists are membership tests on the native side, so
/// their order never reaches the RNG; <see cref="ItemsView"/> and
/// <see cref="DefaultPresetsByTpl"/> are drawn from in iteration order and must be built in the
/// order the C# generator would have walked them.
/// </summary>
public record RewardLootDb
{
    /// <summary>
    /// The slice of every <c>TemplateItem</c> the generator reads, keyed by tpl.
    /// </summary>
    [JsonPropertyName("itemsView")]
    public required Dictionary<MongoId, ItemView> ItemsView { get; set; }

    /// <summary>
    /// <c>PresetHelper.GetDefaultPresets().Values</c>, order preserved.
    /// </summary>
    [JsonPropertyName("defaultPresets")]
    public required List<PresetView> DefaultPresets { get; set; }

    /// <summary>
    /// The tpl to default-preset map, which is not the same method at every call site:
    /// <c>PresetHelper.GetDefaultPresetsByTplKey()</c> for forced loot,
    /// <c>PresetHelper.GetDefaultPresetByTpl()</c> for the sealed case and the reward container.
    /// </summary>
    [JsonPropertyName("defaultPresetsByTpl")]
    public required Dictionary<MongoId, PresetView> DefaultPresetsByTpl { get; set; }

    /// <summary>
    /// The blacklist the sealed container filters test: <c>ItemFilterService.IsItemBlacklisted</c>'s
    /// backing cache, which mods extend at runtime through <c>AddItemToBlacklistCache</c>.
    /// </summary>
    [JsonPropertyName("globalBlacklist")]
    public required HashSet<MongoId> GlobalBlacklist { get; set; }

    /// <summary>
    /// The blacklist the reward pool unions in: <c>ItemFilterService.GetBlacklistedItems()</c>, which
    /// is <c>config/item.json</c>'s list itself and not the cache <see cref="GlobalBlacklist"/> holds.
    /// The two are equal until a mod adds to the cache.
    /// </summary>
    [JsonPropertyName("configBlacklist")]
    public required HashSet<MongoId> ConfigBlacklist { get; set; }

    /// <summary>
    /// <c>ItemFilterService.GetItemRewardBlacklist()</c>.
    /// </summary>
    [JsonPropertyName("rewardItemBlacklist")]
    public required HashSet<MongoId> RewardItemBlacklist { get; set; }

    /// <summary>
    /// <c>ItemFilterService.GetItemRewardBaseTypeBlacklist()</c>.
    /// </summary>
    [JsonPropertyName("rewardBaseTypeBlacklist")]
    public required HashSet<MongoId> RewardBaseTypeBlacklist { get; set; }

    /// <summary>
    /// <c>ItemFilterService.GetBossItems()</c>.
    /// </summary>
    [JsonPropertyName("bossItems")]
    public required HashSet<MongoId> BossItems { get; set; }

    /// <summary>
    /// <c>SeasonalEventService.GetInactiveSeasonalEventItems()</c>.
    /// </summary>
    [JsonPropertyName("inactiveSeasonalItems")]
    public required HashSet<MongoId> InactiveSeasonalItems { get; set; }

    /// <inheritdoc cref="LootCommon.TestSeed"/>
    [JsonPropertyName("testSeed")]
    public ulong? TestSeed { get; set; }
}

public record CreateRandomLootRequest : RewardLootDb
{
    /// <summary>
    /// <c>UseForcedLoot</c>/<c>ForcedLoot</c> are ignored by the native side - the caller branches on
    /// them before it gets here, and forced loot has its own envelope.
    /// </summary>
    [JsonPropertyName("lootRequest")]
    public required LootRequest LootRequest { get; set; }
}

public record CreateForcedLootRequest : RewardLootDb
{
    /// <summary>
    /// Walked in order, drawing a count per entry.
    /// </summary>
    [JsonPropertyName("forcedLoot")]
    public required Dictionary<MongoId, MinMax<int>> ForcedLoot { get; set; }
}

public record SealedWeaponCaseRequest : RewardLootDb
{
    /// <summary>
    /// <c>FoundInRaid</c> is ignored by the native side - the caller applies it to the result.
    /// </summary>
    [JsonPropertyName("containerSettings")]
    public required SealedAirdropContainerSettings ContainerSettings { get; set; }

    /// <summary>
    /// <c>PresetHelper.GetPresets(tpl)</c> per weapon tpl; the inner list is drawn from, so its order
    /// matters.
    /// </summary>
    [JsonPropertyName("presetsByTpl")]
    public required Dictionary<MongoId, List<PresetView>> PresetsByTpl { get; set; }

    /// <summary>
    /// <c>RagfairLinkedItemService.GetLinkedDbItems(tpl)</c> per <c>WeaponRewardWeight</c> key; the
    /// inner list is drawn from, so its order matters.
    /// </summary>
    [JsonPropertyName("linkedItems")]
    public required Dictionary<MongoId, List<MongoId>> LinkedItems { get; set; }
}

public record RandomLootContainerRequest : RewardLootDb
{
    /// <summary>
    /// <c>_type</c>/<c>FoundInRaid</c> are ignored by the native side - the caller applies them to
    /// the result.
    /// </summary>
    [JsonPropertyName("rewardDetails")]
    public required RewardDetails RewardDetails { get; set; }

    /// <summary>
    /// <c>PresetHelper.HasPreset(tpl)</c> as a set, which is a superset of
    /// <see cref="RewardLootDb.DefaultPresetsByTpl"/>'s keys.
    /// </summary>
    [JsonPropertyName("presetTpls")]
    public required HashSet<MongoId> PresetTpls { get; set; }
}

/// <summary>
/// Reward loot as the groups the C# builds: one list per reward, root item first.
/// </summary>
public record RewardLootResult
{
    [JsonPropertyName("items")]
    public required List<List<Item>> Items { get; set; }
}
