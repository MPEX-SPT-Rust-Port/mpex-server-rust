pub(crate) mod bot_equipment_mod_generator;
pub(crate) mod bot_generator_helper;
pub(crate) mod bot_weapon_generator_helper;
pub(crate) mod durability_limits_helper;
pub(crate) mod exhaustable_array;
pub(crate) mod mod_pool_service;
pub mod models;
pub(crate) mod repair_service;

use indexmap::IndexMap;

use crate::bot::durability_limits_helper::BotDurability;
use crate::bot::models::{EquipmentFilters, RandomisedResourceDetails};
use crate::loot::models::{Diagnostic, ItemView, PresetView};

use std::collections::HashSet;

/// The read-only views one bot generation run consults, plus the diagnostics the C# caller replays
/// through its logger — the bot family's analog of [`crate::loot::item_helper::LootContext`].
///
/// Every view is borrowed for `'a`, so copying one out (`let items = ctx.items;`) releases the
/// `&mut ctx` and leaves the diagnostics writable.
pub struct BotContext<'a> {
    /// The `TemplateItem` slice, flattened by the C# caller. There is no `ItemsView` type; the map
    /// itself is the view, matching `loot::item_helper`'s helpers.
    pub items: &'a IndexMap<String, ItemView>,
    /// `BotConfig.Bosses` — `BotHelper.IsBotBoss` scans it, so
    /// `durability_limits_helper::get_durability_role` needs it on every durability roll.
    pub bosses: &'a [String],
    /// `BotConfig.Durability`.
    pub durability: &'a BotDurability,
    /// `BotConfig.Equipment`, keyed by *equipment* role — `pmcBEAR`/`pmcUSEC` collapse to `pmc`
    /// through `bot_generator_helper::get_bot_equipment_role` before the lookup. The whole map, not
    /// one resolved entry: `GenerateExtraPropertiesForItem` takes a per-item `botRole` and
    /// `PlayerScavGenerator.cs:177` passes a literal `"assault"` that need not be the bot's own.
    pub equipment: &'a IndexMap<String, EquipmentFilters>,
    /// `BotConfig.LootItemResourceRandomization`, keyed by the raw bot role (no equipment-role
    /// mapping — `BotGeneratorHelper.cs:63` looks it up verbatim).
    pub loot_item_resource_randomization: &'a IndexMap<String, RandomisedResourceDetails>,
    /// `RaidConfiguration.IsNightRaid`, hoisted by the C# caller. C# reads it off
    /// `profileActivityService.GetFirstProfileActivityRaidData()?.RaidConfiguration`, whose absence
    /// (no raid) defaults to day — the caller folds that into this `false`.
    pub is_night_time: bool,
    /// `ItemFilterService.GetBlacklistedItems()` — one half of the union
    /// `BotEquipmentModGenerator.FilterModsByBlacklist` builds.
    pub item_blacklist: &'a HashSet<String>,
    /// `PresetHelper.GetDefaultPresetByTpl()`, keyed by the tpl the preset is the default for — the
    /// projection `GetDefaultPresetArmorSlot` reads.
    pub default_presets_by_tpl: &'a IndexMap<String, PresetView>,
    pub diagnostics: Vec<Diagnostic>,
}

/// Empty stand-ins for the two views a fixture that exercises neither still has to supply.
#[cfg(test)]
pub(crate) static NO_BLACKLIST: std::sync::LazyLock<HashSet<String>> =
    std::sync::LazyLock::new(HashSet::new);
#[cfg(test)]
pub(crate) static NO_PRESETS: std::sync::LazyLock<IndexMap<String, PresetView>> =
    std::sync::LazyLock::new(IndexMap::new);
