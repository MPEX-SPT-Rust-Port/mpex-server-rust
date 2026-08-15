//! Wire models for the bot generation family.
//!
//! Same two families as `loot::models`: DB/EFT models mirroring the C# records (wire names pinned
//! to the `JsonPropertyName`, or the member name verbatim where the record carries none, and a
//! `#[serde(flatten)] extra` map so mod-added fields survive the trip), and request/response
//! envelopes, which are a fresh contract between the C# caller and this crate and so are plain
//! camelCase.
//!
//! Every request block grew its real type in the task that first read it, the way the loot port
//! grew its models; the orchestrator typed the last of them, so nothing here is a
//! `serde_json::Value` any more.

use indexmap::{IndexMap, IndexSet};
use serde::{Deserialize, Serialize};

use crate::bot::repair_service::MinMax;
use crate::loot::models::{Diagnostic, Item, ItemView, PresetView};

/// Mod-added fields captured on the way in and replayed on the way out.
type Extra = serde_json::Map<String, serde_json::Value>;

// ---------------------------------------------------------------------------
// DB/EFT wire models
// ---------------------------------------------------------------------------

/// The three `BotType` blocks the inventory generator reads (`Models/Eft/Common/Tables/BotType.cs`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BotTemplateWire {
    #[serde(rename = "inventory")]
    pub inventory: BotTypeInventoryWire,
    #[serde(rename = "chances")]
    pub chances: ChancesWire,
    #[serde(rename = "generation")]
    pub generation: GenerationWire,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `BotTypeInventory` (`BotType.cs:228-240`). `IndexMap` throughout: every one of these maps is
/// enumerated to build a weighted pool, so the iteration order reaches the RNG.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BotTypeInventoryWire {
    /// `Dictionary<EquipmentSlots, Dictionary<MongoId, double>>`. The key is the `EquipmentSlots`
    /// member name (`Headwear`, `TacticalVest`, …), kept as a string here.
    #[serde(rename = "equipment", default)]
    pub equipment: IndexMap<String, IndexMap<String, f64>>,
    /// Carries no `JsonPropertyName`, and nothing sets a naming policy, so the wire name is the
    /// C# member name verbatim — PascalCase, unlike its three siblings.
    #[serde(rename = "Ammo", default)]
    pub ammo: IndexMap<String, IndexMap<String, f64>>,
    #[serde(rename = "items", default)]
    pub items: ItemPoolsWire,
    /// `GlobalMods` = `Dictionary<MongoId, Dictionary<string, HashSet<MongoId>>>`
    /// (`GlobalTablesUsings.cs`). An `IndexSet` because it is drawn from by index and a `HashSet`
    /// deserialized from a JSON array keeps that array's order in C# too — and because
    /// `bot_weapon_generator` hands this very map to
    /// [`GenerateWeaponRequestWire::mod_pool`], which C# passes by reference.
    #[serde(rename = "mods", default)]
    pub mods: IndexMap<String, IndexMap<String, IndexSet<String>>>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `ItemPools` (`BotType.cs:242-253`) — no `JsonPropertyName` on any member, so the wire names are
/// the property names verbatim.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ItemPoolsWire {
    #[serde(rename = "Backpack", default)]
    pub backpack: IndexMap<String, f64>,
    #[serde(rename = "Pockets", default)]
    pub pockets: IndexMap<String, f64>,
    #[serde(rename = "SecuredContainer", default)]
    pub secured_container: IndexMap<String, f64>,
    #[serde(rename = "SpecialLoot", default)]
    pub special_loot: IndexMap<String, f64>,
    #[serde(rename = "TacticalVest", default)]
    pub tactical_vest: IndexMap<String, f64>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `Models/Spt/Config/BotConfig.cs:465-472` — `BotConfig.LootItemResourceRandomization[botRole]`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RandomisedResourceDetails {
    #[serde(rename = "food")]
    pub food: Option<RandomisedResourceValues>,
    #[serde(rename = "meds")]
    pub meds: Option<RandomisedResourceValues>,
}

/// `Models/Spt/Config/BotConfig.cs:474-487`. Both members are non-nullable `float`s in C#, so a key
/// missing from the config lands on 0 rather than disabling randomisation — `#[serde(default)]`
/// reproduces that.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RandomisedResourceValues {
    #[serde(rename = "resourcePercent", default)]
    pub resource_percent: f64,
    #[serde(rename = "chanceMaxResourcePercent", default)]
    pub chance_max_resource_percent: f64,
}

/// The six `EquipmentFilters` chance percentages `BotGeneratorHelper.GenerateExtraPropertiesForItem`
/// reads (`Models/Spt/Config/BotConfig.cs:287-318`). All nullable in C#; each call site supplies its
/// own literal fallback, so the `Option`s are carried through rather than defaulted here.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct EquipmentFilters {
    #[serde(rename = "faceShieldIsActiveChancePercent")]
    pub face_shield_is_active_chance_percent: Option<f64>,
    #[serde(rename = "lightIsActiveDayChancePercent")]
    pub light_is_active_day_chance_percent: Option<f64>,
    #[serde(rename = "lightIsActiveNightChancePercent")]
    pub light_is_active_night_chance_percent: Option<f64>,
    #[serde(rename = "laserIsActiveChancePercent")]
    pub laser_is_active_chance_percent: Option<f64>,
    #[serde(rename = "nvgIsActiveChanceDayPercent")]
    pub nvg_is_active_chance_day_percent: Option<f64>,
    #[serde(rename = "nvgIsActiveChanceNightPercent")]
    pub nvg_is_active_chance_night_percent: Option<f64>,

    // -- Read by `bot::bot_equipment_mod_generator` (`BotConfig.cs:226-282`).
    /// Skip the back plate slot when no front plate spawned.
    #[serde(rename = "skipBackPlateIfFrontPlateMissing")]
    pub skip_back_plate_if_front_plate_missing: Option<bool>,
    /// Match the back plate's class to the front plate's instead of letting it roll higher.
    #[serde(rename = "limitPlateClassToFrontPlateClass")]
    pub limit_plate_class_to_front_plate_class: Option<bool>,
    #[serde(rename = "filterPlatesByLevel")]
    pub filter_plates_by_level: Option<bool>,
    /// Extra slot ids treated as required when choosing a mod (`ShouldModBeSpawned`).
    #[serde(rename = "weaponSlotIdsToMakeRequired")]
    pub weapon_slot_ids_to_make_required: Option<IndexSet<String>>,
    #[serde(rename = "armorPlateWeighting")]
    pub armor_plate_weighting: Option<Vec<ArmorPlateWeights>>,

    // -- Read by the weapon half of `bot::bot_equipment_mod_generator` (`BotConfig.cs:221-281`).
    /// Force the stock slot chances to 100% (`ShouldForceSubStockSlots`, `:781`).
    #[serde(rename = "forceStock")]
    pub force_stock: Option<bool>,
    /// The level-banded randomisation blocks `BotHelper.GetBotRandomizationDetails` picks from.
    #[serde(rename = "randomisation")]
    pub randomisation: Option<Vec<RandomisationDetails>>,
    /// `BotConfig.cs:214-215` — the two caps `BotWeaponModLimitService.GetWeaponModLimits` reads.
    #[serde(rename = "weaponModLimits")]
    pub weapon_mod_limits: Option<ModLimitsWire>,
    /// Weapon base-class tpl → the sight base-class tpls allowed on it
    /// (`BotEquipmentFilterService.GetBotWeaponSightWhitelist`, `:124-129`). A `Vec` because
    /// `is_of_baseclasses` takes a slice; the C# `HashSet` is only ever membership-tested through it.
    #[serde(rename = "weaponSightWhitelist")]
    pub weapon_sight_whitelist: Option<IndexMap<String, Vec<String>>>,

    // -- Read by `bot::bot_inventory_generator` (`BotConfig.cs:223-224,320-321`).
    /// Narrow the vest pool to armored rigs when the bot rolled no armor vest (`:378`).
    #[serde(rename = "forceOnlyArmoredRigWhenNoArmor")]
    pub force_only_armored_rig_when_no_armor: Option<bool>,
    /// Force the `TacticalVest` spawn chance to 100% when the bot rolled no armor vest (`:392`).
    #[serde(rename = "forceRigWhenNoVest")]
    pub force_rig_when_no_vest: Option<bool>,
}

/// `Models/Spt/Config/BotConfig.cs:320-337` (`ModLimits`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ModLimitsWire {
    #[serde(rename = "scopeLimit")]
    pub scope_limit: Option<i32>,
    #[serde(rename = "lightLaserLimit")]
    pub light_laser_limit: Option<i32>,
}

/// `Models/Spt/Config/BotConfig.cs:339-388`, narrowed to the members the bot generators read.
/// `LevelRange` is `required` in C#; the rest of the record belongs to tasks that read it.
#[derive(Debug, Clone, Deserialize)]
pub struct RandomisationDetails {
    #[serde(rename = "levelRange")]
    pub level_range: MinMax<i32>,
    /// Slots whose pool is rebuilt from `items.json` instead of the bot's own mod pool.
    #[serde(rename = "randomisedWeaponModSlots")]
    pub randomised_weapon_mod_slots: Option<IndexSet<String>>,
    /// Equipment slots whose mod pool is rebuilt from the gear pool and filtered
    /// (`BotInventoryGenerator.cs:592`).
    #[serde(rename = "randomisedArmorSlots")]
    pub randomised_armor_slots: Option<IndexSet<String>>,
    /// Equipment *mod* slot name → chance. Written by the nighttime clamp
    /// (`BotInventoryGenerator.cs:204`) and read by nothing — see
    /// [`crate::bot::bot_inventory_generator`].
    #[serde(rename = "equipmentMods")]
    pub equipment_mods: Option<IndexMap<String, f64>>,
    #[serde(rename = "nighttimeChanges")]
    pub nighttime_changes: Option<NighttimeChanges>,
    /// Weapon tpl → smallest magazine capacity allowed on it.
    #[serde(rename = "minimumMagazineSize")]
    pub minimum_magazine_size: Option<IndexMap<String, f64>>,
}

/// `Models/Spt/Config/BotConfig.cs:390-397`. `EquipmentModsModifiers` is `required` in C#, so an
/// absent key is a deserialization failure there and here.
#[derive(Debug, Clone, Deserialize)]
pub struct NighttimeChanges {
    /// Equipment mod slot name → the delta added to the matching `equipmentMods` chance.
    #[serde(rename = "equipmentModsModifiers")]
    pub equipment_mods_modifiers: IndexMap<String, f64>,
}

/// `Models/Spt/Config/BotConfig.cs:456-463`. `MinMax` is reused from
/// [`crate::bot::repair_service`], which ported it first; both are the same `Models/Common/MinMax.cs`.
#[derive(Debug, Clone, Deserialize)]
pub struct ArmorPlateWeights {
    #[serde(rename = "levelRange")]
    pub level_range: MinMax<i32>,
    /// Plate slot name (`front_plate`, …) → armor class as a string → weight. Ordered: the inner map
    /// is what `get_weighted_value` scans.
    #[serde(rename = "values")]
    pub values: IndexMap<String, IndexMap<String, f64>>,
}

/// `Models/Spt/Config/BotConfig.cs:399-418`, narrowed to the one member the mod generators read.
/// `LevelRange` is resolved by the C# caller (`GetBotEquipmentBlacklist`) before the call, and
/// `Cartridge` grows a field in the task that first reads it.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct EquipmentFilterDetails {
    /// Mod slot name → blacklisted tpls.
    #[serde(rename = "equipment")]
    pub equipment: Option<IndexMap<String, IndexSet<String>>>,
}

/// `Models/Spt/Bots/GenerateEquipmentProperties.cs`, narrowed to what the equipment-mod path reads.
/// The remaining members (`BotId`, `RootEquipmentSlot`, `RootEquipmentPool`, `Inventory`,
/// `RandomisationDetails`, `GenerateModsBlacklist`, `GeneratingPlayerLevel`) belong to the
/// orchestrator, which is a later task; they land here when it reads them.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct GenerateEquipmentPropertiesWire {
    /// `GlobalMods` — item tpl → mod slot name → compatible tpls, in
    /// `mod_pool_service::get_mods_for_gear_slot` shape.
    #[serde(rename = "modPool", default)]
    pub mod_pool: IndexMap<String, IndexMap<String, IndexSet<String>>>,
    #[serde(rename = "spawnChances", default)]
    pub spawn_chances: ChancesWire,
    #[serde(rename = "botData", default)]
    pub bot_data: BotDataWire,
    #[serde(rename = "botEquipmentConfig", default)]
    pub bot_equipment_config: EquipmentFilters,
}

/// `Models/Eft/Common/Tables/BotType.cs:63-73` (`Chances`) — all three maps.
///
/// Mutable for the whole of one bot's generation: the armband forcing (`:223`) and the
/// no-vest forcing (`:394`) both write into `equipment`, and `GenerateEquipment` reads it back on
/// every later slot.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChancesWire {
    /// `Chances.EquipmentChances` — equipment *slot* name → spawn chance.
    #[serde(rename = "equipment", default)]
    pub equipment: IndexMap<String, f64>,
    /// `Chances.WeaponModsChances` — weapon mod slot name → spawn chance.
    #[serde(rename = "weaponMods", default)]
    pub weapon_mods: IndexMap<String, f64>,
    /// `Chances.EquipmentModsChances` — equipment mod slot name → spawn chance.
    #[serde(rename = "equipmentMods", default)]
    pub equipment_mods: IndexMap<String, f64>,
}

/// `Models/Eft/Common/Tables/BotType.cs:136-140` (`Generation`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GenerationWire {
    #[serde(rename = "items", default)]
    pub items: ItemCountsWire,
}

/// `Models/Spt/Bots/GenerateWeaponRequest.cs:70-89`. Every member is nullable in C#; `role` is
/// interpolated into log lines and passed to `GenerateExtraPropertiesForItem`, so an absent one is
/// the empty string rather than a missing value.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct BotDataWire {
    #[serde(rename = "role", default)]
    pub role: String,
    #[serde(rename = "level", default)]
    pub level: i32,
    #[serde(rename = "equipmentRole", default)]
    pub equipment_role: String,
}

/// `Models/Spt/Bots/GenerateWeaponRequest.cs:7-67`, the request
/// [`generate_mods_for_weapon`](crate::bot::bot_equipment_mod_generator::generate_mods_for_weapon)
/// mutates in place.
///
/// **Deviation:** `ParentTemplate` is a `TemplateItem` in C# and rides as its **tpl** here, the same
/// swap the equipment path made — a flattened [`ItemView`] row carries no id of its own, and the tpl
/// is what the mod-pool lookups and half the log arguments want. Every C# member is nullable and
/// every one of them is dereferenced unguarded on this path, so none of them is an `Option` here.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct GenerateWeaponRequestWire {
    /// Weapon to add mods to / result that is returned.
    #[serde(rename = "weapon", default)]
    pub weapon: Vec<Item>,
    /// `GlobalMods` — item tpl → mod slot name → compatible tpls.
    #[serde(rename = "modPool", default)]
    pub mod_pool: IndexMap<String, IndexMap<String, IndexSet<String>>>,
    /// `_id` of the item mods are being added to (not its tpl).
    #[serde(rename = "weaponId", default)]
    pub weapon_id: String,
    /// Tpl of the item mods are being added to.
    #[serde(rename = "parentTemplate", default)]
    pub parent_template: String,
    #[serde(rename = "modSpawnChances", default)]
    pub mod_spawn_chances: IndexMap<String, f64>,
    #[serde(rename = "ammoTpl", default)]
    pub ammo_tpl: String,
    #[serde(rename = "botData", default)]
    pub bot_data: BotDataWire,
    #[serde(rename = "modLimits", default)]
    pub mod_limits: BotModLimitsWire,
    #[serde(rename = "weaponStats", default)]
    pub weapon_stats: WeaponStatsWire,
    #[serde(rename = "conflictingItemTpls", default)]
    pub conflicting_item_tpls: IndexSet<String>,
}

/// `Models/Spt/Bots/GenerateWeaponRequest.cs:91-100`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct WeaponStatsWire {
    #[serde(rename = "hasOptic")]
    pub has_optic: Option<bool>,
    #[serde(rename = "hasFrontIronSight")]
    pub has_front_iron_sight: Option<bool>,
    #[serde(rename = "hasRearIronSight")]
    pub has_rear_iron_sight: Option<bool>,
}

/// `Models/Spt/Bots/GenerateWeaponRequest.cs:102-121`, built by
/// `BotWeaponModLimitService.GetWeaponModLimits`. The two counters are mutated as mods are added —
/// C# does it through a shared `ItemCount` object, which is why they are a nested struct there and
/// here.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct BotModLimitsWire {
    #[serde(rename = "scope", default)]
    pub scope: ItemCountWire,
    #[serde(rename = "scopeMax")]
    pub scope_max: Option<i32>,
    #[serde(rename = "scopeBaseTypes", default)]
    pub scope_base_types: Vec<String>,
    #[serde(rename = "flashlightLaser", default)]
    pub flashlight_laser: ItemCountWire,
    #[serde(rename = "flashlightLaserMax")]
    pub flashlight_laser_max: Option<i32>,
    #[serde(rename = "flashlightLaserBaseTypes", default)]
    pub flashlight_laser_base_types: Vec<String>,
}

/// `Models/Spt/Bots/GenerateWeaponRequest.cs:123-127`.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ItemCountWire {
    #[serde(rename = "count")]
    pub count: Option<i32>,
}

/// `Models/Eft/Common/Tables/BotType.cs:142-156` — one item-count weighting block.
///
/// `Weights` is a `Dictionary<double, double>` in C# and an `IndexMap<String, f64>` here: JSON
/// object keys are strings and `f64` is not hashable. `bot_weapon_generator_helper` parses the
/// drawn key back out.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GenerationDataWire {
    /// Key: number of items, value: weighting.
    #[serde(rename = "weights", default)]
    pub weights: IndexMap<String, f64>,
    /// Item tpl → weight. Untouched by the magazine path; carried for the loot generator.
    #[serde(rename = "whitelist", default)]
    pub whitelist: IndexMap<String, f64>,
}

/// `Models/Spt/Bots/GenerateWeaponResult.cs`.
///
/// Two deviations:
/// - `WeaponTemplate` is a `TemplateItem` in C# and rides as its **tpl** here, the same swap
///   [`GenerateWeaponRequestWire::parent_template`] made.
/// - `WeaponMods` is not carried. C# stores a reference to the bot template's own mod pool and no
///   consumer ever reads it back off the result (`BotInventoryGenerator.cs:757-774` uses only
///   `Weapon`); cloning the whole pool per weapon to preserve a dead field is not worth the copy.
#[derive(Debug, Clone)]
pub struct GenerateWeaponResultWire {
    pub weapon: Vec<Item>,
    pub chosen_ammo_template: String,
    /// `null` only when the weapon has no UBGL — a UBGL whose ammo could not be resolved lands on
    /// `MongoId.Empty`, i.e. the empty string, which is what `AddExtraMagazinesToInventory` tests.
    pub chosen_ubgl_ammo_template: Option<String>,
    pub weapon_template: String,
}

/// `Models/Eft/Common/Tables/BotType.cs:158-198` (`GenerationWeightingItems`), narrowed to the
/// eleven blocks `BotLootGenerator.GenerateLoot` draws from (`:97-107`) plus the `magazines` block
/// `BotInventoryGenerator.cs:771` hands to `AddExtraMagazinesToInventory`. `looseLoot` is read by
/// nothing this port carries.
///
/// Every block is an `Option` because C# leaves the unset ones null: `itemCounts?.BackpackLoot.Weights
/// is null` (`:80`) null-checks only the *outer* `Items`, so a bot json missing one of these blocks
/// is an NRE there. Here it lands on the same warn-and-return exit as an empty weights map, which is
/// a deviation from a crash, not from an outcome.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ItemCountsWire {
    #[serde(rename = "grenades")]
    pub grenades: Option<GenerationDataWire>,
    #[serde(rename = "healing")]
    pub healing: Option<GenerationDataWire>,
    #[serde(rename = "drugs")]
    pub drugs: Option<GenerationDataWire>,
    #[serde(rename = "food")]
    pub food: Option<GenerationDataWire>,
    #[serde(rename = "drink")]
    pub drink: Option<GenerationDataWire>,
    #[serde(rename = "currency")]
    pub currency: Option<GenerationDataWire>,
    #[serde(rename = "stims")]
    pub stims: Option<GenerationDataWire>,
    #[serde(rename = "backpackLoot")]
    pub backpack_loot: Option<GenerationDataWire>,
    #[serde(rename = "pocketLoot")]
    pub pocket_loot: Option<GenerationDataWire>,
    #[serde(rename = "vestLoot")]
    pub vest_loot: Option<GenerationDataWire>,
    #[serde(rename = "specialItems")]
    pub special_items: Option<GenerationDataWire>,
    /// Spare-magazine counts. `BotInventoryGenerator.cs:771` passes it on unguarded, so an absent
    /// block is an NRE the moment a weapon spawns.
    #[serde(rename = "magazines")]
    pub magazines: Option<GenerationDataWire>,
}

/// `Models/Spt/Bots/BotLootCache.cs:6-46` — the thirteen pools `BotLootCacheService` resolves, sent
/// pre-built because the service itself (and the PMC pool generation behind it) stays C#-side.
///
/// `combined_pool_loot` is carried for completeness; `GenerateLoot` never reads it. See the module
/// doc of [`crate::bot::bot_loot_generator`] for the twelve reads it does perform.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct BotLootCacheWire {
    pub backpack_loot: IndexMap<String, f64>,
    pub pocket_loot: IndexMap<String, f64>,
    pub vest_loot: IndexMap<String, f64>,
    pub secure_loot: IndexMap<String, f64>,
    pub combined_pool_loot: IndexMap<String, f64>,
    pub special_items: IndexMap<String, f64>,
    pub healing_items: IndexMap<String, f64>,
    pub drug_items: IndexMap<String, f64>,
    pub food_items: IndexMap<String, f64>,
    pub drink_items: IndexMap<String, f64>,
    pub currency_items: IndexMap<String, f64>,
    pub stim_items: IndexMap<String, f64>,
    pub grenade_items: IndexMap<String, f64>,
}

/// `Models/Spt/Bots/ItemSpawnLimitSettings.cs` — the pair `GetItemSpawnLimitsForBot`
/// (`BotLootGenerator.cs:47-58`) builds: a zeroed running total and the untouched reference copy.
/// Both are owned here; C# clones the first and hands out a second live read of the config for the
/// second, which nothing mutates.
#[derive(Debug, Clone, Default)]
pub struct ItemSpawnLimitSettingsWire {
    pub current_limits: IndexMap<String, f64>,
    pub global_limits: IndexMap<String, f64>,
}

/// `Models/Spt/Config/BotConfig.cs:185-207` (`WalletLootSettings`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct WalletLootSettingsWire {
    pub chance_percent: f64,
    pub item_count: MinMax<i32>,
    /// Stack size → weight. The key is parsed back out with `int.Parse` (`:633`).
    pub stack_size_weight: IndexMap<String, f64>,
    /// Currency tpl → weight.
    pub currency_weight: IndexMap<String, f64>,
    /// Wallet tpls that get currency put in them.
    pub wallet_tpl_pool: std::collections::HashSet<String>,
}

/// `Models/Spt/Config/PmcConfig.cs`, narrowed to what bot generation reads.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PmcConfigWire {
    pub force_healing_items_into_secure: bool,
    pub loose_weapon_in_backpack_chance_percent: f64,
    pub loose_weapon_in_backpack_loot_min_max: MinMax<i32>,
    pub loot_settings: PmcLootSettingsWire,
    pub add_secure_container_loot_from_bot_config: bool,
    pub loot_item_limits_rub: Vec<MinMaxLootItemValueWire>,
    /// `PmcConfig.ForceArmband` (`:152-162`).
    pub force_armband: ForceArmbandSettingsWire,
    /// `PmcConfig.WeaponHasEnhancementChancePercent`, hoisted into
    /// [`crate::bot::BotContext::weapon_has_enhancement_chance_percent`].
    pub weapon_has_enhancement_chance_percent: f64,
}

/// `Models/Spt/Config/PmcConfig.cs:152-162` (`ForceArmbandSettings`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ForceArmbandSettingsWire {
    pub enabled: bool,
    pub usec: String,
    pub bear: String,
}

/// `Models/Spt/Config/PmcConfig.cs:164-175` (`PmcLootSettings`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PmcLootSettingsWire {
    pub pocket: LootContainerSettingsWire,
    pub vest: LootContainerSettingsWire,
    pub backpack: LootContainerSettingsWire,
}

/// `Models/Spt/Config/PmcConfig.cs:177-185` (`LootContainerSettings`), read through
/// `Extensions/LootContainerSettingsExtensions.cs:10-50`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LootContainerSettingsWire {
    pub total_rub_by_level: Vec<MinMaxLootValueWire>,
    pub location_multiplier: IndexMap<String, f64>,
}

/// `Models/Spt/Config/PmcConfig.cs:233-237` (`MinMaxLootValue : MinMax<int>`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MinMaxLootValueWire {
    pub min: i32,
    pub max: i32,
    pub value: f64,
}

/// `Models/Spt/Config/PmcConfig.cs:239-249` (`MinMaxLootItemValue : MinMax<double>`).
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MinMaxLootItemValueWire {
    pub min: f64,
    pub max: f64,
    pub backpack: MinMax<f64>,
    pub pocket: MinMax<f64>,
    pub vest: MinMax<f64>,
}

/// `Models/Spt/Bots/ChooseRandomCompatibleModResult.cs`. Every member is nullable there and the
/// four `IsItemIncompatibleWithCurrentItems` exits each set a different subset, so the `Option`s
/// are load-bearing — `found` and `slotBlocked` are absent, not false, on the final compatible
/// return.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ChooseRandomCompatibleModResult {
    #[serde(rename = "incompatible", skip_serializing_if = "Option::is_none")]
    pub incompatible: Option<bool>,
    #[serde(rename = "found", skip_serializing_if = "Option::is_none")]
    pub found: Option<bool>,
    #[serde(rename = "chosenTpl", skip_serializing_if = "Option::is_none")]
    pub chosen_template: Option<String>,
    #[serde(rename = "reason", skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(rename = "slotBlocked", skip_serializing_if = "Option::is_none")]
    pub slot_blocked: Option<bool>,
}

// ---------------------------------------------------------------------------
// Request / response envelopes
// ---------------------------------------------------------------------------

/// `BotInventoryContainerService.ContainerDetails` (`BotInventoryContainerService.cs:415-451`),
/// serialized so the C# side can rebuild the service's per-bot cache after a native call.
///
/// `ContainerDbItem` and `ContainerInventoryItem` ride as ids: the C# rebuild resolves the first
/// through `itemHelper.GetItem` and the second out of the inventory it was just handed, exactly as
/// `AddEmptyContainerToBot` does. `ContainerFull` is not carried — it is initialised `false` and
/// nothing in the codebase ever assigns it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainerDetailsWire {
    /// `ContainerDetails.ContainerDbItem.Id`.
    pub container_tpl: String,
    /// `ContainerDetails.ContainerInventoryItem.Id`.
    pub container_item_id: String,
    /// `ContainerDetails.ContainerGridDetails`, in the container template's grid order.
    pub grids: Vec<ContainerMapDetailsWire>,
}

/// `BotInventoryContainerService.ContainerMapDetails` (`BotInventoryContainerService.cs:453-457`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainerMapDetailsWire {
    /// `int[CellsV, CellsH]` as rows of columns, `1` = occupied. Dimensions are implied, matching
    /// the `[Vec<Vec<u8>>]` grids `loot::container_extensions` already packs into.
    pub grid_map: Vec<Vec<u8>>,
    pub grid_full: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateBotInventoryRequest {
    pub bot_id: String,
    /// Test-only: when present, every draw comes from a seeded xoshiro256** for the duration of
    /// the call (see `loot::random_util::TestSeedGuard`). Never set on the production path.
    #[serde(default)]
    pub test_seed: Option<u64>,
    pub details: BotGenerationDetailsWire,
    pub template: BotTemplateWire,
    /// Hoisted live state — `PlayerProfile.Info.Level` (`BotInventoryGenerator.cs:228`). Read only
    /// C#-side, to resolve [`Self::equipment_blacklist`] and
    /// [`Self::weapon_mod_equipment_blacklist`]; see [`crate::bot::bot_inventory_generator`].
    pub generating_player_level: i32,
    /// Hoisted live state — `RaidTime`/`WeatherHelper.IsNightTime`.
    pub is_night_time: bool,
    /// `BotConfig.Equipment`, keyed by equipment role. The whole map, not one resolved entry: the
    /// orchestrator does its own `GetBotEquipmentRole` lookup (`:180`) *and* a `ContainsKey` on the
    /// bot data's role (`:590`), and `GenerateExtraPropertiesForItem` looks roles up per item.
    pub equipment: IndexMap<String, EquipmentFilters>,
    /// `BotConfig.Bosses`.
    pub bosses: Vec<String>,
    /// `BotConfig.Durability`.
    pub durability: crate::bot::durability_limits_helper::BotDurability,
    pub item_spawn_limits: IndexMap<String, IndexMap<String, f64>>,
    pub wallet_loot: WalletLootSettingsWire,
    /// `BotConfig.CurrencyStackSize` — bot role → money tpl → stack size → weight.
    pub currency_stack_size: IndexMap<String, IndexMap<String, IndexMap<String, f64>>>,
    /// `BotConfig.SecureContainerAmmoStackCount`.
    pub secure_container_ammo_stack_count: i32,
    pub disable_loot_on_bot_types: std::collections::HashSet<String>,
    pub low_profile_gas_block_tpls: std::collections::HashSet<String>,
    pub loot_item_resource_randomization: IndexMap<String, RandomisedResourceDetails>,
    pub pmc_config: PmcConfigWire,
    /// `RepairConfig.RepairKit.Weapon`.
    pub repair_kit_weapon: crate::bot::repair_service::BonusSettings,
    /// `GetBotEquipmentBlacklist(role, level)` result, as the equipment path resolves it
    /// (`BotInventoryGenerator.cs:583`, level defaulted to 1).
    pub equipment_blacklist: EquipmentFilterDetails,
    /// The same call as [`Self::equipment_blacklist`] but as the *weapon-mod* path resolves it
    /// (`BotEquipmentModGenerator.cs:544`, level defaulted to **0**). Legacy is internally
    /// inconsistent about that default and level 0 matches no `levelRange`, so the two blacklists
    /// differ and each path has to use its own.
    pub weapon_mod_equipment_blacklist: EquipmentFilterDetails,
    /// The 13 resolved `BotLootCacheService` pools.
    pub loot_pools: BotLootCacheWire,
    /// `GlobalTable.ItemPresets`, keyed by preset id. Also the map `PresetHelper.GetPreset(id)`
    /// reads, so it stands in for the `presetsById` the payload used to carry separately.
    pub item_presets: IndexMap<String, PresetView>,
    /// Which preset is the default for a tpl, as its id — resolve it through [`Self::item_presets`].
    pub default_presets_by_tpl: IndexMap<String, String>,
    /// `ItemFilterService.GetBlacklistedItems()`.
    pub config_blacklist: std::collections::HashSet<String>,
    pub handbook_prices: IndexMap<String, f64>,
    /// The `TemplateItem` slice, flattened by the C# caller exactly as the loot envelopes take it.
    pub items: IndexMap<String, ItemView>,
    /// The C# `BotEquipmentModPoolService` pools' slot-name enumeration order per template, as
    /// indices into that template's projected `slots` array. `#[serde(default)]` so an absent
    /// field means database order — today's behavior.
    #[serde(default)]
    pub mod_pool_slot_order: IndexMap<String, Vec<usize>>,
}

/// The 20 request members that do not vary between the bots of one wave — every database view,
/// every config slice, and the blacklist the caller resolved from the wave's role and the player's
/// level. 95.7% of a single-bot request's bytes by measurement.
///
/// Deserialized as a nested object rather than `#[serde(flatten)]`: flatten routes the whole map
/// through serde's buffering path, which would cost more than the duplication it removes.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedBotViewsWire {
    pub generating_player_level: i32,
    pub is_night_time: bool,
    pub equipment: IndexMap<String, EquipmentFilters>,
    pub bosses: Vec<String>,
    pub durability: crate::bot::durability_limits_helper::BotDurability,
    pub item_spawn_limits: IndexMap<String, IndexMap<String, f64>>,
    pub wallet_loot: WalletLootSettingsWire,
    pub currency_stack_size: IndexMap<String, IndexMap<String, IndexMap<String, f64>>>,
    pub secure_container_ammo_stack_count: i32,
    pub disable_loot_on_bot_types: std::collections::HashSet<String>,
    pub low_profile_gas_block_tpls: std::collections::HashSet<String>,
    pub loot_item_resource_randomization: IndexMap<String, RandomisedResourceDetails>,
    pub pmc_config: PmcConfigWire,
    pub repair_kit_weapon: crate::bot::repair_service::BonusSettings,
    pub equipment_blacklist: EquipmentFilterDetails,
    /// See [`GenerateBotInventoryRequest::weapon_mod_equipment_blacklist`].
    pub weapon_mod_equipment_blacklist: EquipmentFilterDetails,
    pub item_presets: IndexMap<String, PresetView>,
    /// Keyed by tpl, valued by the default preset's own id - resolve through
    /// [`Self::item_presets`], which is the map `PresetHelper` resolves every default out of.
    pub default_presets_by_tpl: IndexMap<String, String>,
    pub config_blacklist: std::collections::HashSet<String>,
    pub items: IndexMap<String, ItemView>,
    /// The C# `BotEquipmentModPoolService` pools' slot-name enumeration order per template, as
    /// indices into that template's projected `slots` array. `#[serde(default)]` so an absent
    /// field means database order — today's behavior.
    #[serde(default)]
    pub mod_pool_slot_order: IndexMap<String, Vec<usize>>,
}

/// The six request members that do vary per bot. `template` is per-bot because
/// `BotEquipmentFilterService.FilterBotEquipment` mutates a fresh clone for each one, and
/// `loot_pools`/`handbook_prices` because the loot price bands are resolved from the bot's level.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BotSliceWire {
    pub bot_id: String,
    #[serde(default)]
    pub test_seed: Option<u64>,
    pub details: BotGenerationDetailsWire,
    pub template: BotTemplateWire,
    pub loot_pools: BotLootCacheWire,
    pub handbook_prices: IndexMap<String, f64>,
}

/// One wave: the shared views once, then a slice per bot.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateBotInventoryBatchRequest {
    pub shared: SharedBotViewsWire,
    pub bots: Vec<BotSliceWire>,
}

/// One entry per requested bot, in request order: exactly one of `result` or `error` is set.
/// A bot that fails no longer aborts the wave — `BotController.TryGenerateSingleBot` skips a
/// failed bot with one Critical log, and the batch has to offer the caller the same choice.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BotResultEnvelope {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<BotInventoryResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// One envelope per requested bot, in request order.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BotInventoryBatchResult {
    pub bots: Vec<BotResultEnvelope>,
}

/// `Models/Spt/Bots/BotGenerationDetails.cs`, narrowed to what the generator reads.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BotGenerationDetailsWire {
    pub role: String,
    pub role_lowercase: String,
    pub side: String,
    pub bot_level: i32,
    pub is_pmc: bool,
    pub is_player_scav: bool,
    pub game_version: String,
    pub location: Option<String>,
    pub bot_difficulty: String,
    pub clear_bot_container_cache_after_generation: bool,
}

/// `Models/Eft/Common/Tables/BotBase.cs:358-401` (`BotBaseInventory`), in C# member order so the
/// serialized key order matches too. Write-only — the C# caller deserializes it straight into the
/// bot it is building — so there is no passthrough map and every member is populated.
#[derive(Debug, Clone, Default, Serialize)]
pub struct BotBaseInventoryWire {
    #[serde(rename = "items")]
    pub items: Vec<Item>,
    #[serde(rename = "equipment")]
    pub equipment: String,
    #[serde(rename = "stash")]
    pub stash: String,
    #[serde(rename = "sortingTable")]
    pub sorting_table: String,
    #[serde(rename = "questRaidItems")]
    pub quest_raid_items: String,
    #[serde(rename = "questStashItems")]
    pub quest_stash_items: String,
    #[serde(rename = "hideoutAreaStashes")]
    pub hideout_area_stashes: IndexMap<String, String>,
    #[serde(rename = "fastPanel")]
    pub fast_panel: IndexMap<String, String>,
    #[serde(rename = "favoriteItems")]
    pub favorite_items: Vec<String>,
    #[serde(rename = "hideoutCustomizationStashId")]
    pub hideout_customization_stash_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BotInventoryResult {
    pub inventory: BotBaseInventoryWire,
    pub diagnostics: Vec<Diagnostic>,
    /// Slot → grid state, from `bot_generator_helper::ContainerGrids::into_wire`. Empty when the
    /// request asked for the cache to be cleared (`BotInventoryGenerator.cs:114-117`).
    pub container_grids: IndexMap<String, ContainerDetailsWire>,
    /// Equipment *mod* slot → the chance the nighttime clamp (`:204`) left behind, for the C#
    /// caller to write back into its shared `BotConfig` object.
    pub randomisation_clamps: IndexMap<String, f64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every required [`GenerateBotInventoryRequest`] member. `testSeed` is deliberately absent —
    /// its omission is what exercises the missing-field → `None` path.
    const REQUEST_JSON: &str = r#"{
        "botId":"bbbbbbbbbbbbbbbbbbbbbbbb",
        "details":{"role":"assault","roleLowercase":"assault","side":"Savage","botLevel":12,
            "isPmc":false,"isPlayerScav":false,"gameVersion":"standard","location":"bigmap",
            "botDifficulty":"normal","clearBotContainerCacheAfterGeneration":true},
        "template":{
            "inventory":{
                "equipment":{"Headwear":{"aaaaaaaaaaaaaaaaaaaaaaa1":3.5,"aaaaaaaaaaaaaaaaaaaaaaa2":1}},
                "Ammo":{"Caliber762x39":{"aaaaaaaaaaaaaaaaaaaaaaa3":5}},
                "items":{"Backpack":{"aaaaaaaaaaaaaaaaaaaaaaa4":2},"Pockets":{},
                    "SecuredContainer":{},"SpecialLoot":{},"TacticalVest":{}},
                "mods":{"aaaaaaaaaaaaaaaaaaaaaaa5":{"mod_magazine":["aaaaaaaaaaaaaaaaaaaaaaa6"]}},
                "modAddedInventoryField":"kept"},
            "chances":{"equipment":{"Headwear":75},"weaponMods":{"mod_scope":30},
                "equipmentMods":{"mod_nvg":40}},
            "generation":{"items":{"backpackLoot":{"weights":{"1":1}},
                "magazines":{"weights":{"2":1}}}},
            "modAddedTemplateField":7},
        "generatingPlayerLevel":30,
        "isNightTime":true,
        "equipment":{"assault":{"weaponModLimits":{"scopeLimit":2},
            "forceRigWhenNoVest":true,"forceOnlyArmoredRigWhenNoArmor":false,
            "randomisation":[{"levelRange":{"min":1,"max":99},
                "randomisedArmorSlots":["Headwear"],"equipmentMods":{"mod_nvg":40},
                "nighttimeChanges":{"equipmentModsModifiers":{"mod_nvg":90}}}]}},
        "bosses":["bossknight"],
        "durability":{"default":{"armor":{"maxDelta":10,"minDelta":0,"minLimitPercent":15},
                "weapon":{"lowestMax":60,"highestMax":100,"maxDelta":10,"minDelta":0,
                          "minLimitPercent":15}},
            "botDurabilities":{},
            "pmc":{"armor":{"lowestMaxPercent":90,"highestMaxPercent":100,"maxDelta":10,
                            "minDelta":0,"minLimitPercent":15},
                "weapon":{"lowestMax":95,"highestMax":100,"maxDelta":5,"minDelta":0,
                          "minLimitPercent":15}}},
        "itemSpawnLimits":{"assault":{"aaaaaaaaaaaaaaaaaaaaaaa7":1}},
        "walletLoot":{"chancePercent":10},
        "currencyStackSize":{"default":{"RUB":{"1000":1}}},
        "secureContainerAmmoStackCount":3,
        "disableLootOnBotTypes":["bosstest"],
        "lowProfileGasBlockTpls":["aaaaaaaaaaaaaaaaaaaaaaa8"],
        "lootItemResourceRandomization":{"assault":{"food":{"chanceMaxResourcePercent":60}}},
        "pmcConfig":{"forceHealingItemsIntoSecure":true,
            "forceArmband":{"enabled":true,"usec":"armband_usec","bear":"armband_bear"},
            "weaponHasEnhancementChancePercent":25},
        "repairKitWeapon":{"rarityWeight":{},"bonusTypeWeight":{},"Common":{},"Rare":{}},
        "equipmentBlacklist":{"equipment":{"Headwear":["aaaaaaaaaaaaaaaaaaaaaaa9"]}},
        "weaponModEquipmentBlacklist":{},
        "lootPools":{"backpackLoot":{"aaaaaaaaaaaaaaaaaaaaaab1":4}},
        "itemPresets":{"p1":{"id":"p1","items":[]},"p2":{"id":"p2","items":[]}},
        "defaultPresetsByTpl":{"aaaaaaaaaaaaaaaaaaaaaab2":"p2"},
        "configBlacklist":["aaaaaaaaaaaaaaaaaaaaaab3"],
        "handbookPrices":{"aaaaaaaaaaaaaaaaaaaaaab4":12500.5},
        "items":{"aaaaaaaaaaaaaaaaaaaaaab5":{"parent":"aaaaaaaaaaaaaaaaaaaaaab6","width":2,"height":1}}
    }"#;

    #[test]
    fn generate_bot_inventory_request_deserializes() {
        let parsed: GenerateBotInventoryRequest = serde_json::from_str(REQUEST_JSON).unwrap();

        assert_eq!(parsed.bot_id, "bbbbbbbbbbbbbbbbbbbbbbbb");
        // Absent `testSeed` is the production path.
        assert_eq!(parsed.test_seed, None);

        assert_eq!(parsed.details.role, "assault");
        assert_eq!(parsed.details.role_lowercase, "assault");
        assert_eq!(parsed.details.side, "Savage");
        assert_eq!(parsed.details.bot_level, 12);
        assert!(!parsed.details.is_pmc);
        assert!(!parsed.details.is_player_scav);
        assert_eq!(parsed.details.game_version, "standard");
        assert_eq!(parsed.details.location.as_deref(), Some("bigmap"));
        assert_eq!(parsed.details.bot_difficulty, "normal");
        assert!(parsed.details.clear_bot_container_cache_after_generation);

        let inventory = &parsed.template.inventory;
        assert_eq!(
            inventory.equipment["Headwear"].keys().collect::<Vec<_>>(),
            vec!["aaaaaaaaaaaaaaaaaaaaaaa1", "aaaaaaaaaaaaaaaaaaaaaaa2"]
        );
        assert_eq!(
            inventory.equipment["Headwear"]["aaaaaaaaaaaaaaaaaaaaaaa1"],
            3.5
        );
        assert_eq!(
            inventory.ammo["Caliber762x39"]["aaaaaaaaaaaaaaaaaaaaaaa3"],
            5.0
        );
        assert_eq!(inventory.items.backpack["aaaaaaaaaaaaaaaaaaaaaaa4"], 2.0);
        assert!(inventory.items.tactical_vest.is_empty());
        assert_eq!(
            inventory.mods["aaaaaaaaaaaaaaaaaaaaaaa5"]["mod_magazine"]
                .iter()
                .collect::<Vec<_>>(),
            vec!["aaaaaaaaaaaaaaaaaaaaaaa6"]
        );
        assert_eq!(parsed.template.chances.equipment["Headwear"], 75.0);
        assert_eq!(parsed.template.chances.weapon_mods["mod_scope"], 30.0);
        assert_eq!(parsed.template.chances.equipment_mods["mod_nvg"], 40.0);
        let generation_items = &parsed.template.generation.items;
        assert_eq!(
            generation_items.backpack_loot.as_ref().unwrap().weights["1"],
            1.0
        );
        assert_eq!(
            generation_items.magazines.as_ref().unwrap().weights["2"],
            1.0
        );

        assert_eq!(parsed.generating_player_level, 30);
        assert!(parsed.is_night_time);
        let assault_equipment = &parsed.equipment["assault"];
        assert_eq!(
            assault_equipment
                .weapon_mod_limits
                .as_ref()
                .unwrap()
                .scope_limit,
            Some(2)
        );
        assert_eq!(assault_equipment.force_rig_when_no_vest, Some(true));
        assert_eq!(
            assault_equipment.force_only_armored_rig_when_no_armor,
            Some(false)
        );
        let randomisation = &assault_equipment.randomisation.as_ref().unwrap()[0];
        assert!(
            randomisation
                .randomised_armor_slots
                .as_ref()
                .unwrap()
                .contains("Headwear")
        );
        assert_eq!(
            randomisation.equipment_mods.as_ref().unwrap()["mod_nvg"],
            40.0
        );
        assert_eq!(
            randomisation
                .nighttime_changes
                .as_ref()
                .unwrap()
                .equipment_mods_modifiers["mod_nvg"],
            90.0
        );
        assert_eq!(parsed.bosses, vec!["bossknight"]);
        assert_eq!(parsed.durability.default.weapon.lowest_max, 60);
        assert_eq!(
            parsed.item_spawn_limits["assault"]["aaaaaaaaaaaaaaaaaaaaaaa7"],
            1.0
        );
        assert_eq!(parsed.wallet_loot.chance_percent, 10.0);
        assert_eq!(parsed.currency_stack_size["default"]["RUB"]["1000"], 1.0);
        assert_eq!(parsed.secure_container_ammo_stack_count, 3);
        assert!(parsed.disable_loot_on_bot_types.contains("bosstest"));
        assert!(
            parsed
                .low_profile_gas_block_tpls
                .contains("aaaaaaaaaaaaaaaaaaaaaaa8")
        );
        let food = parsed.loot_item_resource_randomization["assault"]
            .food
            .as_ref()
            .unwrap();
        assert_eq!(food.chance_max_resource_percent, 60.0);
        // Absent in the payload; C#'s non-nullable float lands on 0, not on "no randomisation".
        assert_eq!(food.resource_percent, 0.0);
        assert!(
            parsed.loot_item_resource_randomization["assault"]
                .meds
                .is_none()
        );
        assert!(parsed.pmc_config.force_healing_items_into_secure);
        assert!(parsed.pmc_config.force_armband.enabled);
        assert_eq!(parsed.pmc_config.force_armband.usec, "armband_usec");
        assert_eq!(parsed.pmc_config.force_armband.bear, "armband_bear");
        assert_eq!(
            parsed.pmc_config.weapon_has_enhancement_chance_percent,
            25.0
        );
        assert!(parsed.repair_kit_weapon.rarity_weight.is_empty());
        assert!(
            parsed.equipment_blacklist.equipment.as_ref().unwrap()["Headwear"]
                .contains("aaaaaaaaaaaaaaaaaaaaaaa9")
        );
        assert_eq!(
            parsed.loot_pools.backpack_loot["aaaaaaaaaaaaaaaaaaaaaab1"],
            4.0
        );
        // Pools the payload omits deserialize empty, not missing.
        assert!(parsed.loot_pools.combined_pool_loot.is_empty());
        assert_eq!(parsed.item_presets["p1"].id.as_deref(), Some("p1"));
        // The default rides as an id and is resolved against `item_presets`, not inlined
        assert_eq!(
            parsed.default_presets_by_tpl["aaaaaaaaaaaaaaaaaaaaaab2"],
            "p2"
        );
        assert_eq!(parsed.item_presets["p2"].id.as_deref(), Some("p2"));
        assert!(parsed.config_blacklist.contains("aaaaaaaaaaaaaaaaaaaaaab3"));
        assert_eq!(parsed.handbook_prices["aaaaaaaaaaaaaaaaaaaaaab4"], 12500.5);
        assert_eq!(parsed.items["aaaaaaaaaaaaaaaaaaaaaab5"].width, Some(2));
    }

    #[test]
    fn test_seed_is_read_when_present() {
        let json = REQUEST_JSON.replace(
            r#""botId":"bbbbbbbbbbbbbbbbbbbbbbbb","#,
            r#""botId":"bbbbbbbbbbbbbbbbbbbbbbbb","testSeed":42,"#,
        );
        let parsed: GenerateBotInventoryRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.test_seed, Some(42));

        // Explicit null is the same as absent.
        let json = REQUEST_JSON.replace(
            r#""botId":"bbbbbbbbbbbbbbbbbbbbbbbb","#,
            r#""botId":"bbbbbbbbbbbbbbbbbbbbbbbb","testSeed":null,"#,
        );
        let parsed: GenerateBotInventoryRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.test_seed, None);
    }

    #[test]
    fn mod_added_template_fields_survive_the_round_trip() {
        let parsed: GenerateBotInventoryRequest = serde_json::from_str(REQUEST_JSON).unwrap();
        let out = serde_json::to_value(&parsed.template).unwrap();

        assert_eq!(out["modAddedTemplateField"], 7);
        assert_eq!(out["inventory"]["modAddedInventoryField"], "kept");
        // Exact wire casing: `Ammo` is PascalCase in the database, the rest are camelCase.
        let inventory = out["inventory"].as_object().unwrap();
        assert!(inventory.contains_key("Ammo"));
        assert!(inventory.contains_key("equipment"));
        assert!(inventory.contains_key("items"));
        assert!(inventory.contains_key("mods"));
        // `ItemPools` members carry no `JsonPropertyName`, so they ride out PascalCase.
        let pools = out["inventory"]["items"].as_object().unwrap();
        assert!(pools.contains_key("SecuredContainer"));
        assert!(pools.contains_key("SpecialLoot"));
        assert!(pools.contains_key("TacticalVest"));
    }

    #[test]
    fn bot_inventory_result_serializes_with_camel_case_keys() {
        let out = serde_json::to_value(BotInventoryResult {
            inventory: BotBaseInventoryWire {
                equipment: "aaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
                ..Default::default()
            },
            diagnostics: vec![Diagnostic {
                level: crate::loot::models::DEBUG.to_owned(),
                locale_key: Some("bot-missing_item".to_owned()),
                args: Some(serde_json::json!({"tpl":"x"})),
                message: None,
            }],
            container_grids: IndexMap::from([(
                "TacticalVest".to_owned(),
                ContainerDetailsWire {
                    container_tpl: "aaaaaaaaaaaaaaaaaaaaaac1".to_owned(),
                    container_item_id: "aaaaaaaaaaaaaaaaaaaaaac2".to_owned(),
                    grids: vec![ContainerMapDetailsWire {
                        grid_map: vec![vec![0, 1], vec![0, 0]],
                        grid_full: false,
                    }],
                },
            )]),
            randomisation_clamps: IndexMap::from([("Headwear".to_owned(), 62.5)]),
        })
        .unwrap();

        assert_eq!(
            out.as_object().unwrap().keys().collect::<Vec<_>>(),
            vec![
                "inventory",
                "diagnostics",
                "containerGrids",
                "randomisationClamps"
            ]
        );
        assert_eq!(out["inventory"]["equipment"], "aaaaaaaaaaaaaaaaaaaaaaaa");
        // `BotBaseInventory` member order, so the C# deserializer sees its own shape back.
        assert_eq!(
            out["inventory"]
                .as_object()
                .unwrap()
                .keys()
                .collect::<Vec<_>>(),
            vec![
                "items",
                "equipment",
                "stash",
                "sortingTable",
                "questRaidItems",
                "questStashItems",
                "hideoutAreaStashes",
                "fastPanel",
                "favoriteItems",
                "hideoutCustomizationStashId"
            ]
        );
        assert_eq!(out["diagnostics"][0]["localeKey"], "bot-missing_item");
        assert_eq!(out["diagnostics"][0]["args"]["tpl"], "x");
        let grids = &out["containerGrids"]["TacticalVest"];
        assert_eq!(grids["containerTpl"], "aaaaaaaaaaaaaaaaaaaaaac1");
        assert_eq!(grids["containerItemId"], "aaaaaaaaaaaaaaaaaaaaaac2");
        assert_eq!(grids["grids"][0]["gridMap"][0][1], 1);
        assert_eq!(grids["grids"][0]["gridFull"], false);
        assert_eq!(out["randomisationClamps"]["Headwear"], 62.5);
    }
}
