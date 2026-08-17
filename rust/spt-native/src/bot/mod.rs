pub(crate) mod bot_equipment_mod_generator;
pub(crate) mod bot_generator_helper;
pub(crate) mod bot_inventory_generator;
pub(crate) mod bot_loot_generator;
pub(crate) mod bot_weapon_generator;
pub(crate) mod bot_weapon_generator_helper;
pub(crate) mod durability_limits_helper;
pub(crate) mod exhaustable_array;
pub(crate) mod inventory_mag_gen;
pub(crate) mod mod_pool_service;
pub mod models;
pub(crate) mod repair_service;

use indexmap::IndexMap;

use crate::bot::durability_limits_helper::BotDurability;
use crate::bot::models::{EquipmentFilterDetails, EquipmentFilters, RandomisedResourceDetails};
use crate::bot::repair_service::BonusSettings;
use crate::diag::DiagSink;
use crate::loot::models::{ItemView, PresetView};

use std::collections::HashSet;

/// The read-only views one bot generation run consults, plus the [`DiagSink`] its diagnostics
/// emit through — the bot family's analog of [`crate::loot::item_helper::LootContext`].
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
    /// `PresetHelper.GetDefaultPresetByTpl()`, keyed by the tpl the preset is the default for and
    /// valued by the preset's own id — resolve it through [`Self::item_presets`], which is what
    /// `PresetHelper` resolves every default out of. `GetDefaultPresetArmorSlot` reads it.
    pub default_presets_by_tpl: &'a IndexMap<String, String>,
    /// `GlobalTable.ItemPresets`, keyed by preset `_id` — scanned **in order** by
    /// `BotWeaponGenerator.GetPresetWeaponMods` (`:337`), which takes the first preset whose root
    /// item matches the weapon tpl, so the map has to stay ordered.
    pub item_presets: &'a IndexMap<String, PresetView>,
    /// `GetBotEquipmentBlacklist(equipmentRole, playerLevel)`, resolved by the C# caller. The
    /// equipment path takes its blacklist as a parameter because the C# does.
    pub equipment_blacklist: &'a EquipmentFilterDetails,
    /// The weapon path's own blacklist — it resolves one internally (`:546`) with the player level
    /// defaulted to 0 rather than 1, which matches no `levelRange`, so it is a different object
    /// from [`Self::equipment_blacklist`] and cannot be shared with the equipment path.
    pub weapon_mod_equipment_blacklist: &'a EquipmentFilterDetails,
    /// `BotConfig.LowProfileGasBlockTpls` — membership tests only (`:1063`, `:1072`).
    pub low_profile_gas_block_tpls: &'a HashSet<String>,
    /// `PmcConfig.WeaponHasEnhancementChancePercent` — the gate on `RepairService.AddBuff`
    /// (`BotWeaponGenerator.cs:154`).
    pub weapon_has_enhancement_chance_percent: f64,
    /// `RepairConfig.RepairKit.Weapon` — the only `BonusSettings` bot generation passes to
    /// [`crate::bot::repair_service::add_buff`].
    pub repair_kit_weapon: &'a BonusSettings,
    /// `BotConfig.SecureContainerAmmoStackCount` (`BotConfig.cs:85`).
    pub secure_container_ammo_stack_count: i32,
    /// `modPoolSlotOrder` — the C# `BotEquipmentModPoolService` pools' slot-name enumeration
    /// order per template, as indices into that template's `slots`. Only order crosses the wire;
    /// membership is still derived by [`crate::bot::mod_pool_service`]. Missing entry = database
    /// order.
    pub mod_pool_slot_order: &'a IndexMap<String, Vec<usize>>,
    pub diagnostics: DiagSink,
}

/// Empty stand-ins for the views a fixture that exercises none of them still has to supply.
#[cfg(test)]
pub(crate) static NO_BLACKLIST: std::sync::LazyLock<HashSet<String>> =
    std::sync::LazyLock::new(HashSet::new);
#[cfg(test)]
pub(crate) static NO_PRESETS: std::sync::LazyLock<IndexMap<String, PresetView>> =
    std::sync::LazyLock::new(IndexMap::new);
#[cfg(test)]
pub(crate) static NO_DEFAULT_PRESETS: std::sync::LazyLock<IndexMap<String, String>> =
    std::sync::LazyLock::new(IndexMap::new);
#[cfg(test)]
pub(crate) static NO_EQUIP_BLACKLIST: std::sync::LazyLock<EquipmentFilterDetails> =
    std::sync::LazyLock::new(EquipmentFilterDetails::default);
#[cfg(test)]
pub(crate) static NO_MOD_POOL_ORDER: std::sync::LazyLock<IndexMap<String, Vec<usize>>> =
    std::sync::LazyLock::new(IndexMap::new);
#[cfg(test)]
pub(crate) static NO_BUFFS: std::sync::LazyLock<BonusSettings> = std::sync::LazyLock::new(|| {
    serde_json::from_value(serde_json::json!({
        "rarityWeight": {}, "bonusTypeWeight": {}, "Common": {}, "Rare": {}
    }))
    .expect("empty bonus settings parse")
});
