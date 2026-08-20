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
/// The distrust fallback for the four reward exports: the C#-built database half plus the four
/// <c>ItemConfig</c> sets the resident arm reads off the configs root's <c>spt-item</c> stem,
/// present iff the caller is ineligible for residency. <see cref="PresetsByTpl"/> rides only on
/// sealed-case sends and <see cref="PresetTpls"/> only on reward-container sends, mirroring the old
/// per-envelope members. <see cref="ItemsView"/> and <see cref="DefaultPresetsByTpl"/> are drawn
/// from in iteration order and must be built in the order the C# generator would have walked them.
/// </summary>
public record RewardViewsOverride
{
    /// <summary>
    /// The slice of every <c>TemplateItem</c> the generator reads, keyed by tpl.
    /// </summary>
    [JsonPropertyName("itemsView")]
    public required Dictionary<MongoId, ItemView> ItemsView { get; set; }

    /// <summary>
    /// <c>PresetHelper.GetDefaultPresets().Values</c>, order preserved. Only random loot reads it;
    /// the other envelopes send an empty list.
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
    /// Sealed only: <c>PresetHelper.GetPresets(tpl)</c> per weapon tpl; the inner list is drawn
    /// from, so its order matters.
    /// </summary>
    [JsonPropertyName("presetsByTpl")]
    public Dictionary<MongoId, List<PresetView>>? PresetsByTpl { get; set; }

    /// <summary>
    /// Container only: <c>PresetHelper.HasPreset(tpl)</c> as a set, which is a superset of
    /// <see cref="DefaultPresetsByTpl"/>'s keys.
    /// </summary>
    [JsonPropertyName("presetTpls")]
    public HashSet<MongoId>? PresetTpls { get; set; }

    /// <summary>
    /// The blacklist the reward pool unions in: <c>ItemFilterService.GetBlacklistedItems()</c>, which
    /// is <c>config/item.json</c>'s list itself and not the cache
    /// <see cref="RewardLootVarying.GlobalBlacklist"/> holds. The two are equal until a mod adds to
    /// the cache, which is exactly why one is config-backed and the other stays varying.
    /// </summary>
    [JsonPropertyName("configBlacklist")]
    public required HashSet<MongoId> ConfigBlacklist { get; set; }

    /// <summary>
    /// <c>ItemFilterService.GetItemRewardBlacklist()</c> - <c>ItemConfig.RewardItemBlacklist</c>.
    /// </summary>
    [JsonPropertyName("rewardItemBlacklist")]
    public required HashSet<MongoId> RewardItemBlacklist { get; set; }

    /// <summary>
    /// <c>ItemFilterService.GetItemRewardBaseTypeBlacklist()</c> -
    /// <c>ItemConfig.RewardItemTypeBlacklist</c>.
    /// </summary>
    [JsonPropertyName("rewardBaseTypeBlacklist")]
    public required HashSet<MongoId> RewardBaseTypeBlacklist { get; set; }

    /// <summary>
    /// <c>ItemFilterService.GetBossItems()</c> - <c>ItemConfig.BossItems</c>.
    /// </summary>
    [JsonPropertyName("bossItems")]
    public required HashSet<MongoId> BossItems { get; set; }
}

/// <summary>
/// The per-call half every reward export carries: the service-backed blacklists/sets (a mod can
/// extend them at runtime) and the test seed. The four config-backed sets that used to ride beside
/// them live on <see cref="RewardViewsOverride"/> now.
/// </summary>
public record RewardLootVarying
{
    /// <summary>
    /// The blacklist the sealed container filters test: <c>ItemFilterService.IsItemBlacklisted</c>'s
    /// backing cache, which mods extend at runtime through <c>AddItemToBlacklistCache</c>. Stays
    /// varying where <see cref="RewardViewsOverride.ConfigBlacklist"/> did not, precisely because it
    /// is that mutable cache and not the config value.
    /// </summary>
    [JsonPropertyName("globalBlacklist")]
    public required HashSet<MongoId> GlobalBlacklist { get; set; }

    /// <summary>
    /// <c>SeasonalEventService.GetInactiveSeasonalEventItems()</c> - a service's own cache, so it
    /// rides every send.
    /// </summary>
    [JsonPropertyName("inactiveSeasonalItems")]
    public required HashSet<MongoId> InactiveSeasonalItems { get; set; }

    /// <inheritdoc cref="LootVarying.TestSeed"/>
    [JsonPropertyName("testSeed")]
    public ulong? TestSeed { get; set; }
}

public record CreateRandomLootRequest
{
    /// <summary>
    ///     Resident-DB epoch this request was built against; 0 with <see cref="ViewsOverride"/>
    ///     present (spec § Exports).
    /// </summary>
    [JsonPropertyName("epoch")]
    public required ulong Epoch { get; set; }

    /// <summary>
    ///     The distrust fallback (spec § Exports): the C#-built view bundle, used for this call
    ///     only and never made resident. Present iff the caller is ineligible for residency.
    /// </summary>
    [JsonPropertyName("viewsOverride")]
    public RewardViewsOverride? ViewsOverride { get; set; }

    [JsonPropertyName("varying")]
    public required CreateRandomLootVarying Varying { get; set; }
}

public record CreateRandomLootVarying : RewardLootVarying
{
    /// <summary>
    /// <c>UseForcedLoot</c>/<c>ForcedLoot</c> are ignored by the native side - the caller branches on
    /// them before it gets here, and forced loot has its own envelope.
    /// </summary>
    [JsonPropertyName("lootRequest")]
    public required LootRequest LootRequest { get; set; }
}

public record CreateForcedLootRequest
{
    /// <summary>
    ///     Resident-DB epoch this request was built against; 0 with <see cref="ViewsOverride"/>
    ///     present (spec § Exports).
    /// </summary>
    [JsonPropertyName("epoch")]
    public required ulong Epoch { get; set; }

    /// <summary>
    ///     The distrust fallback (spec § Exports): the C#-built view bundle, used for this call
    ///     only and never made resident. Present iff the caller is ineligible for residency.
    /// </summary>
    [JsonPropertyName("viewsOverride")]
    public RewardViewsOverride? ViewsOverride { get; set; }

    [JsonPropertyName("varying")]
    public required CreateForcedLootVarying Varying { get; set; }
}

public record CreateForcedLootVarying : RewardLootVarying
{
    /// <summary>
    /// Walked in order, drawing a count per entry.
    /// </summary>
    [JsonPropertyName("forcedLoot")]
    public required Dictionary<MongoId, MinMax<int>> ForcedLoot { get; set; }
}

public record SealedWeaponCaseRequest
{
    /// <summary>
    ///     Resident-DB epoch this request was built against; 0 with <see cref="ViewsOverride"/>
    ///     present (spec § Exports).
    /// </summary>
    [JsonPropertyName("epoch")]
    public required ulong Epoch { get; set; }

    /// <summary>
    ///     The distrust fallback (spec § Exports): the C#-built view bundle, used for this call
    ///     only and never made resident. Present iff the caller is ineligible for residency.
    /// </summary>
    [JsonPropertyName("viewsOverride")]
    public RewardViewsOverride? ViewsOverride { get; set; }

    [JsonPropertyName("varying")]
    public required SealedWeaponCaseVarying Varying { get; set; }
}

public record SealedWeaponCaseVarying : RewardLootVarying
{
    /// <summary>
    /// <c>FoundInRaid</c> is ignored by the native side - the caller applies it to the result.
    /// </summary>
    [JsonPropertyName("containerSettings")]
    public required SealedAirdropContainerSettings ContainerSettings { get; set; }

    /// <summary>
    /// <c>RagfairLinkedItemService.GetLinkedDbItems(tpl)</c> per <c>WeaponRewardWeight</c> key; the
    /// inner list is drawn from, so its order matters. Service-backed (the linked-item cache is
    /// mod-extendable at runtime), so it rides every sealed send.
    /// </summary>
    [JsonPropertyName("linkedItems")]
    public required Dictionary<MongoId, List<MongoId>> LinkedItems { get; set; }
}

public record RandomLootContainerRequest
{
    /// <summary>
    ///     Resident-DB epoch this request was built against; 0 with <see cref="ViewsOverride"/>
    ///     present (spec § Exports).
    /// </summary>
    [JsonPropertyName("epoch")]
    public required ulong Epoch { get; set; }

    /// <summary>
    ///     The distrust fallback (spec § Exports): the C#-built view bundle, used for this call
    ///     only and never made resident. Present iff the caller is ineligible for residency.
    /// </summary>
    [JsonPropertyName("viewsOverride")]
    public RewardViewsOverride? ViewsOverride { get; set; }

    [JsonPropertyName("varying")]
    public required RandomLootContainerVarying Varying { get; set; }
}

public record RandomLootContainerVarying : RewardLootVarying
{
    /// <summary>
    /// <c>_type</c>/<c>FoundInRaid</c> are ignored by the native side - the caller applies them to
    /// the result.
    /// </summary>
    [JsonPropertyName("rewardDetails")]
    public required RewardDetails RewardDetails { get; set; }
}

/// <summary>
/// Reward loot as the groups the C# builds: one list per reward, root item first.
/// </summary>
public record RewardLootResult
{
    [JsonPropertyName("items")]
    public required List<List<Item>> Items { get; set; }
}
