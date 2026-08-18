using System.Text.Json.Serialization;
using SPTarkov.Server.Core.Models.Common;
using SPTarkov.Server.Core.Models.Eft.Common.Tables;
using SPTarkov.Server.Core.Models.Enums;
using SPTarkov.Server.Core.Models.Spt.Config;
using SPTarkov.Server.Core.Models.Spt.Repeatable;
using SPTarkov.Server.Core.Native.Loot;

namespace SPTarkov.Server.Core.Native.RepeatableQuests;

/// <summary>
/// The request/response envelope of <c>spt_generate_repeatable_quest</c>, mirroring
/// <c>rust/spt-native/src/quest/models.rs</c> member for member. The envelope itself is ragfair's
/// resident-DB epoch shape reused: epoch, optional views override, varying half.
///
/// Config and database models are the existing records from <c>Models</c>, whose
/// <c>JsonPropertyName</c>s are what the Rust wire names were pinned to, so their shape stays
/// authoritative by construction. The game-data views (<see cref="ItemView"/>,
/// <see cref="PresetView"/>) are the loot port's, and the quest views override
/// deliberately reuses them so one C# projection serves both families.
///
/// Members Rust declares as <c>Option&lt;T&gt;</c> are nullable, everything else is
/// <c>required</c>: <see cref="Utils.JsonUtil"/> serialises with
/// <see cref="JsonIgnoreCondition.WhenWritingNull"/>, so a null member is omitted and a Rust member
/// that is not an <c>Option</c> would fail the parse.
/// </summary>
internal record GenerateRepeatableQuestRequest
{
    /// <summary>
    ///     Resident-DB epoch this request was built against; 0 with <see cref="ViewsOverride"/>
    ///     present.
    /// </summary>
    [JsonPropertyName("epoch")]
    public required ulong Epoch { get; set; }

    /// <summary>
    ///     The distrust fallback: the C#-built view bundle, used for this call only and never made
    ///     resident. Present iff the caller is ineligible for residency.
    /// </summary>
    [JsonPropertyName("viewsOverride")]
    public QuestViewsOverride? ViewsOverride { get; set; }

    [JsonPropertyName("varying")]
    public required RepeatableQuestVaryingFields Varying { get; set; }
}

/// <summary>
/// The members that change every call - everything the projection does not read off the database.
/// </summary>
internal record RepeatableQuestVaryingFields
{
    /// <summary>
    ///     Which generator to run. A closed enum on the far side, so this is always the calling
    ///     generator's own constant, never a string drawn from the pool.
    /// </summary>
    [JsonPropertyName("questType")]
    [JsonConverter(typeof(JsonStringEnumConverter))]
    public required RepeatableQuestType QuestType { get; set; }

    [JsonPropertyName("sessionId")]
    public required string SessionId { get; set; }

    [JsonPropertyName("pmcLevel")]
    public required int PmcLevel { get; set; }

    [JsonPropertyName("traderId")]
    public required MongoId TraderId { get; set; }

    /// <summary>
    ///     The pool the generators draw from and mutate; the mutated copy comes back on the response.
    /// </summary>
    [JsonPropertyName("questTypePool")]
    public required QuestTypePool QuestTypePool { get; set; }

    [JsonPropertyName("repeatableConfig")]
    public required RepeatableQuestConfig RepeatableConfig { get; set; }

    /// <summary>
    ///     Test-only: draws on the native side come from a seeded generator when set. Null - and
    ///     therefore omitted from the wire JSON - on the production path. Named <c>seed</c>, not
    ///     <c>testSeed</c> like the loot and ragfair families.
    /// </summary>
    [JsonPropertyName("seed")]
    public ulong? Seed { get; set; }

    // Moved off the old invariant slice: service/config state with no resident home until
    // Phases 2/4. Wire names and value shapes are byte-identical to the old slice members.

    /// <summary>
    ///     What <c>ItemFilterService.IsItemBlacklisted</c> answers from: the config/item.json
    ///     blacklist plus anything added at runtime.
    /// </summary>
    [JsonPropertyName("itemBlacklist")]
    public required HashSet<MongoId> ItemBlacklist { get; set; }

    [JsonPropertyName("rewardItemBlacklist")]
    public required HashSet<MongoId> RewardItemBlacklist { get; set; }

    [JsonPropertyName("bossItems")]
    public required HashSet<MongoId> BossItems { get; set; }

    [JsonPropertyName("seasonalItemTplBlacklist")]
    public required HashSet<MongoId> SeasonalItemTplBlacklist { get; set; }

    /// <summary>
    ///     <c>QuestConfig.RepeatableQuestTemplates</c> - the template <b>ids</b> by player group,
    ///     not the quest templates in the views override. Its per-group keys are PascalCase quest
    ///     type names.
    /// </summary>
    [JsonPropertyName("repeatableQuestTemplateIds")]
    public required RepeatableQuestTemplates RepeatableQuestTemplateIds { get; set; }

    /// <summary>
    ///     <c>QuestConfig.LocationIdMap</c>, keyed the way <c>GetQuestLocationByMapId</c> looks it
    ///     up: by the raw <c>ELocationName</c> name, mixed case included.
    /// </summary>
    [JsonPropertyName("locationIdMap")]
    public required Dictionary<string, string> LocationIdMap { get; set; }
}

/// <summary>
/// The C#-built override of the database views the native side would otherwise read from its
/// resident DB, sent by callers ineligible for residency.
/// </summary>
internal record QuestViewsOverride
{
    /// <inheritdoc cref="LootCommon.ItemsView"/>
    [JsonPropertyName("items")]
    public required Dictionary<MongoId, ItemView> Items { get; set; }

    /// <summary>
    ///     <c>HandbookHelper.GetTemplatePrice</c> for the whole items table - the static arm of
    ///     <c>ItemHelper.GetItemPrice</c>. The currency tpls are in it by construction, which
    ///     <c>FromRoubles</c> depends on: it reads its conversion rate out of this same map.
    /// </summary>
    [JsonPropertyName("handbookPrices")]
    public required Dictionary<MongoId, double> HandbookPrices { get; set; }

    /// <summary>
    ///     <c>TemplateTable.Prices</c> - the dynamic fallback arm of <c>ItemHelper.GetItemPrice</c>.
    /// </summary>
    [JsonPropertyName("fleaPrices")]
    public required Dictionary<MongoId, double> FleaPrices { get; set; }

    /// <summary>
    ///     <c>PresetHelper.GetDefaultWeaponPresets().Values</c> - the <c>ExhaustableArray</c> pool the
    ///     weapon-reward path draws from.
    /// </summary>
    [JsonPropertyName("defaultWeaponPresets")]
    public required List<PresetView> DefaultWeaponPresets { get; set; }

    /// <summary>
    ///     <c>PresetHelper.GetDefaultPresetOrItemPrice</c> per tpl. It walks the preset caches, so it
    ///     stays a C#-side loop and crosses as a map.
    /// </summary>
    [JsonPropertyName("defaultPresetOrItemPrices")]
    public required Dictionary<MongoId, double> DefaultPresetOrItemPrices { get; set; }

    /// <summary>
    ///     <c>TemplateTable.RepeatableQuests.Templates</c> - the four templates
    ///     <c>GetClonedQuestTemplateForType</c> clones. Keys stay PascalCase: the record's own
    ///     <c>JsonPropertyName</c>s are the wire names.
    /// </summary>
    [JsonPropertyName("repeatableQuestTemplates")]
    public required RepeatableTemplates RepeatableQuestTemplates { get; set; }

    /// <summary>
    ///     <c>...Data.Completion.ItemsWhitelist</c>. Absent, null and empty all take the same C#
    ///     branch, so they collapse into one empty list here.
    /// </summary>
    [JsonPropertyName("completionItemsWhitelist")]
    public required List<ItemsWhitelist> CompletionItemsWhitelist { get; set; }

    /// <summary>
    ///     <c>...Data.Completion.ItemsBlacklist</c>, same collapse.
    /// </summary>
    [JsonPropertyName("completionItemsBlacklist")]
    public required List<ItemsBlacklist> CompletionItemsBlacklist { get; set; }

    /// <summary>
    ///     Boss names per location, keyed by the raw <c>LocationBase.Id</c>. <c>BossName</c> is the
    ///     only member the elimination generator reads off a <c>BossLocationSpawn</c>, and the
    ///     blacklist it filters the locations with is compared against that same key ordinally -
    ///     shipped ids are mixed-case, so these keys are never lowercased.
    /// </summary>
    [JsonPropertyName("bossSpawnsByLocation")]
    public required Dictionary<string, List<string>> BossSpawnsByLocation { get; set; }

    /// <summary>
    ///     <c>LocationBase.AllExtracts</c> per location, keyed by the lowercased pool key
    ///     <c>LocationTable.GetLocation</c> is called with. A location with no extracts carries an
    ///     empty list rather than being omitted: the two cases take different error branches.
    ///     List order is load-bearing - the exit is drawn from the filtered list by index.
    /// </summary>
    [JsonPropertyName("extractsByLocation")]
    public required Dictionary<string, List<ExitView>> ExtractsByLocation { get; set; }
}

/// <summary>
/// The members of <c>LocationBase.Exit</c> the exploration generator reads: the side filter, the
/// spawn-chance and passage-requirement filters, and the name it mints the condition from. A
/// PascalCase passthrough of the whole record would land every member on the Rust side as null.
/// </summary>
internal record ExitView
{
    [JsonPropertyName("name")]
    public string? Name { get; set; }

    [JsonPropertyName("side")]
    public string? Side { get; set; }

    [JsonPropertyName("chance")]
    public double? Chance { get; set; }

    /// <summary>
    ///     <c>RequirementState</c> as the string the C# compares against
    ///     <c>SpecificExits.PassageRequirementWhitelist</c>.
    /// </summary>
    [JsonPropertyName("passageRequirement")]
    public required string PassageRequirement { get; set; }
}

/// <summary>
/// One generated quest and the pool it was drawn from, which the generators mutate. A null quest
/// with an OK status is a valid outcome - the C# returns null from the same paths.
/// </summary>
internal record RepeatableQuestResult
{
    [JsonPropertyName("quest")]
    public RepeatableQuest? Quest { get; set; }

    [JsonPropertyName("pool")]
    public required QuestTypePool Pool { get; set; }
}
