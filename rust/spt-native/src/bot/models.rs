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
use crate::loot::models::{Item, ItemView, PresetView};

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
    #[serde(flatten, skip_serializing_if = "crate::db::skip_extra_for_digest")]
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
    #[serde(flatten, skip_serializing_if = "crate::db::skip_extra_for_digest")]
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
    #[serde(flatten, skip_serializing_if = "crate::db::skip_extra_for_digest")]
    pub extra: Extra,
}

/// `BotType.BotAppearance` weight maps (`Models/Eft/Common/Tables/BotType.cs` `Appearance`) —
/// wire names are the C# `JsonPropertyName`s, all lowercase.
#[derive(Debug, Clone, Deserialize)]
pub struct AppearanceWire {
    #[serde(rename = "body")]
    pub body: IndexMap<String, f64>,
    #[serde(rename = "feet")]
    pub feet: IndexMap<String, f64>,
    #[serde(rename = "hands")]
    pub hands: IndexMap<String, f64>,
    #[serde(rename = "head")]
    pub head: IndexMap<String, f64>,
    #[serde(rename = "voice")]
    pub voice: IndexMap<String, f64>,
}

/// `BotTypeHealth` — unattributed C# properties serialize PascalCase (no global naming policy).
#[derive(Debug, Clone, Deserialize)]
pub struct BotTypeHealthWire {
    #[serde(rename = "BodyParts")]
    pub body_parts: Vec<BodyPartTemplateWire>,
    #[serde(rename = "Energy")]
    pub energy: MinMax<f64>,
    #[serde(rename = "Hydration")]
    pub hydration: MinMax<f64>,
    #[serde(rename = "Temperature")]
    pub temperature: MinMax<f64>,
}

/// One `BodyPart` band of the health template.
#[derive(Debug, Clone, Deserialize)]
pub struct BodyPartTemplateWire {
    #[serde(rename = "Chest")]
    pub chest: MinMax<f64>,
    #[serde(rename = "Head")]
    pub head: MinMax<f64>,
    #[serde(rename = "LeftArm")]
    pub left_arm: MinMax<f64>,
    #[serde(rename = "LeftLeg")]
    pub left_leg: MinMax<f64>,
    #[serde(rename = "RightArm")]
    pub right_arm: MinMax<f64>,
    #[serde(rename = "RightLeg")]
    pub right_leg: MinMax<f64>,
    #[serde(rename = "Stomach")]
    pub stomach: MinMax<f64>,
}

/// `BotDbSkills`. Values are `Option` because the C# dictionaries carry nulls, which
/// `GetCommonSkillsWithRandomisedProgressValue` skips without drawing.
#[derive(Debug, Clone, Deserialize)]
pub struct BotDbSkillsWire {
    #[serde(rename = "Common")]
    pub common: IndexMap<String, Option<MinMax<f64>>>,
    #[serde(rename = "Mastering", default)]
    pub mastering: Option<IndexMap<String, Option<MinMax<f64>>>>,
}

/// `Models/Spt/Config/BotConfig.cs:465-472` — `BotConfig.LootItemResourceRandomization[botRole]`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct RandomisedResourceDetails {
    #[serde(rename = "food")]
    pub food: Option<RandomisedResourceValues>,
    #[serde(rename = "meds")]
    pub meds: Option<RandomisedResourceValues>,
}

/// `Models/Spt/Config/BotConfig.cs:474-487`. Both members are non-nullable `float`s in C#, so a key
/// missing from the config lands on 0 rather than disabling randomisation — `#[serde(default)]`
/// reproduces that.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct RandomisedResourceValues {
    #[serde(rename = "resourcePercent", default)]
    pub resource_percent: f64,
    #[serde(rename = "chanceMaxResourcePercent", default)]
    pub chance_max_resource_percent: f64,
}

/// The six `EquipmentFilters` chance percentages `BotGeneratorHelper.GenerateExtraPropertiesForItem`
/// reads (`Models/Spt/Config/BotConfig.cs:287-318`). All nullable in C#; each call site supplies its
/// own literal fallback, so the `Option`s are carried through rather than defaulted here.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
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
    /// The level-banded equipment blacklists `BotEquipmentFilterService.GetBotEquipmentBlacklist`
    /// picks from — see [`crate::bot::select_equipment_blacklist`], which does that pick natively
    /// for both the equipment and the weapon-mod path. `Whitelist` and
    /// `WeightingAdjustmentsByBotLevel` are the C# caller's: they filter the template *before* the
    /// call and never cross.
    #[serde(rename = "blacklist")]
    pub blacklist: Option<Vec<EquipmentFilterDetails>>,
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
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ModLimitsWire {
    #[serde(rename = "scopeLimit")]
    pub scope_limit: Option<i32>,
    #[serde(rename = "lightLaserLimit")]
    pub light_laser_limit: Option<i32>,
}

/// `Models/Spt/Config/BotConfig.cs:339-388`, narrowed to the members the bot generators read.
/// `LevelRange` is `required` in C#; the rest of the record belongs to tasks that read it.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RandomisationDetails {
    /// `#[serde(default)]` for [`EquipmentFilterDetails::level_range`]'s reason, which applies with
    /// more force here: these bands are resident now, so a mod-authored band with no `levelRange`
    /// would fail the *publish* rather than one call. The same tolerance reaches the override arm
    /// (`BotViewsWire.equipment` shares this type): such a band becomes a `(0, 0)` band that
    /// matches no level instead of failing the request loudly.
    #[serde(default, rename = "levelRange")]
    pub level_range: MinMax<i32>,
    /// Slots whose pool is rebuilt from `items.json` instead of the bot's own mod pool.
    #[serde(rename = "randomisedWeaponModSlots")]
    pub randomised_weapon_mod_slots: Option<IndexSet<String>>,
    /// Equipment slots whose mod pool is rebuilt from the gear pool and filtered
    /// (`BotInventoryGenerator.cs:592`).
    #[serde(rename = "randomisedArmorSlots")]
    pub randomised_armor_slots: Option<IndexSet<String>>,
    /// Equipment *mod* slot name → chance. The one cell of the equipment graph a runtime writer
    /// touches: the nighttime clamp (`BotInventoryGenerator.cs:204`) assigns into it after every
    /// send, tripping no write barrier — see [`crate::bot::bot_inventory_generator`]. Nothing in
    /// *this* call reads it back, but the next bot's C# prelude does
    /// (`BotEquipmentFilterService.cs:63`), so the live values arrive on the request as
    /// [`SharedBotVaryingWire::live_equipment_mods`] and
    /// [`crate::bot::resolve_equipment`] overlays them onto the resident bands.
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
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct NighttimeChanges {
    /// Equipment mod slot name → the delta added to the matching `equipmentMods` chance.
    #[serde(rename = "equipmentModsModifiers")]
    pub equipment_mods_modifiers: IndexMap<String, f64>,
}

/// `Models/Spt/Config/BotConfig.cs:456-463`. `MinMax` is reused from
/// [`crate::bot::repair_service`], which ported it first; both are the same `Models/Common/MinMax.cs`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ArmorPlateWeights {
    /// `#[serde(default)]` for [`EquipmentFilterDetails::level_range`]'s reason — see
    /// [`RandomisationDetails::level_range`].
    #[serde(default, rename = "levelRange")]
    pub level_range: MinMax<i32>,
    /// Plate slot name (`front_plate`, …) → armor class as a string → weight. Ordered: the inner map
    /// is what `get_weighted_value` scans.
    #[serde(rename = "values")]
    pub values: IndexMap<String, IndexMap<String, f64>>,
}

/// `Models/Spt/Config/BotConfig.cs:433-452`, narrowed to the two members the native side reads.
/// `Cartridge` grows a field in the task that first reads it — the cartridge filter still runs C#
/// side, before the call.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct EquipmentFilterDetails {
    /// The band this entry covers. Read by [`crate::bot::select_equipment_blacklist`], which is
    /// where `GetBotEquipmentBlacklist`'s `FirstOrDefault` now lives.
    ///
    /// **Divergence on malformed data.** `MinMax<T>` is a `record` — a *reference* type — and
    /// `LevelRange` is a plain non-required auto-property (`BotConfig.cs:438-439`), so an omitted
    /// `levelRange` leaves it **null** in C# and the `FirstOrDefault` predicate throws an NRE. The
    /// `#[serde(default)]` here lands on `(0, 0)` instead, which *matches* a level-0 request — i.e.
    /// the weapon-mod path would silently select such a band where legacy crashes. Unreachable from
    /// stock config (both `bot.json` bands carry a `levelRange`); only mod-authored config can get
    /// here, and answering rather than aborting the publish is the friendlier of the two.
    #[serde(default, rename = "levelRange")]
    pub level_range: MinMax<i32>,
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
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
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
///
/// Soft-despite-`required`: every mirrored member is `required` in C#, but this is the override
/// wire's shape, reused verbatim as the `spt-pmc` stem (one parse, one shape, both arms — the
/// [`DynamicConfigWire`] precedent), and the bot suite's override fixtures publish it partially
/// throughout, so the struct-level `#[serde(default)]` stays. The consequence is
/// [`ItemConfigLift`](crate::db::models::ItemConfigLift)'s: the shipped projection always carries
/// all eleven members, but a hand-built or mod-rewritten stem that omits one silently reads
/// defaults instead of failing the publish. A strict twin struct for the stem would have to
/// mirror this one field for field and would drift; `phase4_configs_root.rs` pins the eleven wire
/// names against the projected dump instead.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
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
    /// `PmcConfig.GameVersionWeight`.
    pub game_version_weight: IndexMap<String, f64>,
    /// `PmcConfig.AccountTypeWeight` — C#-side a `Dictionary<MemberCategory, double>`;
    /// `EftEnumConverter.WriteAsPropertyName` encodes the enum keys as numeric strings on the
    /// wire ("0", "1", "256", "512" shipped).
    pub account_type_weight: IndexMap<String, f64>,
    /// `PmcConfig.DogtagSettings` (wire name `dogtags`): side → gameVersion → tpl → weight.
    #[serde(rename = "dogtags")]
    pub dogtag_settings: IndexMap<String, IndexMap<String, IndexMap<String, f64>>>,
}

/// `Models/Spt/Config/PmcConfig.cs:152-162` (`ForceArmbandSettings`).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ForceArmbandSettingsWire {
    pub enabled: bool,
    pub usec: String,
    pub bear: String,
}

/// `Models/Spt/Config/PmcConfig.cs:164-175` (`PmcLootSettings`).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PmcLootSettingsWire {
    pub pocket: LootContainerSettingsWire,
    pub vest: LootContainerSettingsWire,
    pub backpack: LootContainerSettingsWire,
}

/// `Models/Spt/Config/PmcConfig.cs:177-185` (`LootContainerSettings`), read through
/// `Extensions/LootContainerSettingsExtensions.cs:10-50`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LootContainerSettingsWire {
    pub total_rub_by_level: Vec<MinMaxLootValueWire>,
    pub location_multiplier: IndexMap<String, f64>,
}

/// `Models/Spt/Config/PmcConfig.cs:233-237` (`MinMaxLootValue : MinMax<int>`).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MinMaxLootValueWire {
    pub min: i32,
    pub max: i32,
    pub value: f64,
}

/// `Models/Spt/Config/PmcConfig.cs:239-249` (`MinMaxLootItemValue : MinMax<double>`).
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
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

/// The database half of both bot requests — [`crate::bot::views::BotDbViews`] plus the four
/// resident config stems on the wire, for the override arm of
/// [`crate::bot::resolve_bot_views`]. An override-less request reads all of it off the resident DB
/// instead ([`crate::db::models::BotConfigLift`], the `spt-pmc` [`PmcConfigWire`] stem,
/// [`crate::db::models::RepairConfigLift`] and [`crate::db::models::ItemConfigLift::blacklist`]).
///
/// [`Self::equipment`] is the override arm's `BotConfig.Equipment`; the live `EquipmentMods` bands
/// ride [`SharedBotVaryingWire::live_equipment_mods`] on *both* arms and
/// [`crate::bot::resolve_equipment`] overlays them onto whichever arm answered.
///
/// Every config member below is `required` on the C# record, so all are strict here: the override
/// arm is always C#-built and always carries them.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BotViewsWire {
    /// The `TemplateItem` slice, flattened by the C# caller exactly as the loot envelopes take it.
    pub items: IndexMap<String, ItemView>,
    /// `GlobalTable.ItemPresets`, keyed by preset id. Also the map `PresetHelper.GetPreset(id)`
    /// reads, so it stands in for the `presetsById` the payload used to carry separately.
    pub item_presets: IndexMap<String, PresetView>,
    /// Keyed by tpl, valued by the default preset's own id - resolve through
    /// [`Self::item_presets`], which is the map `PresetHelper` resolves every default out of.
    pub default_presets_by_tpl: IndexMap<String, String>,
    /// `HandbookHelper.GetTemplatePrice` per tpl (`BotPayloadProjection.BuildHandbookPrices`,
    /// `:295-311`) — a tpl missing from the map prices at 0, which is what `GetTemplatePrice`
    /// returns for a tpl the handbook does not know.
    #[serde(default)]
    pub handbook_prices: IndexMap<String, f64>,
    /// `GlobalTable.ExperienceTable` projected to plain ints, in order — what the PMC level draw
    /// sums out of (`BotLevelGenerator.cs:39`).
    #[serde(default)]
    pub exp_table: Vec<i32>,

    // -- The `spt-bot` config slice (`BotConfigLift`).
    pub bosses: Vec<String>,
    pub bot_roles_with_dog_tags: std::collections::HashSet<String>,
    /// bodyTpl → fixed hands tpl, the resident [`crate::bot::views::BotDbViews::body_to_fixed_hands`]
    /// derive's twin on the wire (`BotPayloadProjection.BuildBodyToFixedHands`).
    pub body_to_fixed_hands: IndexMap<String, String>,
    pub durability: crate::bot::durability_limits_helper::BotDurability,
    pub item_spawn_limits: IndexMap<String, IndexMap<String, f64>>,
    pub wallet_loot: WalletLootSettingsWire,
    pub currency_stack_size: IndexMap<String, IndexMap<String, IndexMap<String, f64>>>,
    pub secure_container_ammo_stack_count: i32,
    pub disable_loot_on_bot_types: std::collections::HashSet<String>,
    pub low_profile_gas_block_tpls: std::collections::HashSet<String>,
    pub loot_item_resource_randomization: IndexMap<String, RandomisedResourceDetails>,
    /// `BotConfig.Equipment`, minus the null values the C# projection drops
    /// (`BotPayloadProjection.cs:149`) — the resident arm's [`crate::db::models::BotConfigLift`]
    /// keeps them as `None` and filters at resolve time instead.
    pub equipment: IndexMap<String, EquipmentFilters>,

    // -- The `spt-pmc` and `spt-repair` slices, and `ItemConfig.Blacklist`.
    pub pmc_config: PmcConfigWire,
    /// `RepairConfig.RepairKit.Weapon` — the one `BonusSettings` bot generation passes.
    pub repair_kit_weapon: crate::bot::repair_service::BonusSettings,
    /// `ItemFilterService.GetBlacklistedItems()`, i.e. `ItemConfig.Blacklist` verbatim
    /// (`ItemFilterService.cs:51-54`) — *not* the runtime-augmented `ItemBlacklistCache`.
    pub config_blacklist: std::collections::HashSet<String>,
}

/// The single-bot request: the `{epoch, viewsOverride?, …}` envelope around one varying block, one
/// bot slice and that bot's pre-filtered template and loot pools. This path keeps C# level
/// generation and C# filtering — no draw, no variant pick.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateBotInventoryRequest {
    /// The resident-DB epoch the caller last published. Checked only when
    /// [`Self::views_override`] is absent; a mismatch is `STATUS_STALE_EPOCH`.
    pub epoch: u64,
    /// When present, the database views ride the wire and the resident store is never consulted.
    #[serde(default)]
    pub views_override: Option<Box<BotViewsWire>>,
    pub shared: SharedBotVaryingWire,
    pub bot: BotSliceWire,
    pub template: BotTemplateWire,
    /// The 13 resolved `BotLootCacheService` pools.
    pub loot_pools: BotLootCacheWire,
}

/// `BotLevelGenerator.GetRelativePmcBotLevelRange` (`BotLevelGenerator.cs:67-101`), resolved once
/// per wave because its inputs are all wave-constant. The exp table the draw sums out of (`:39`)
/// lives on the views ([`BotViewsWire::exp_table`] / the resident `BotDbViews::exp_table`). Only
/// the draw itself varies per bot, and that is what
/// [`crate::bot::level_generator::generate_bot_level`] does natively.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LevelGenerationWire {
    pub level_min: i32,
    pub level_max: i32,
}

/// The bot template and the loot pools resolved from the bot's level, for one band of levels.
///
/// Every level-dependent step the C# prelude runs before the native call is a band lookup —
/// `FirstOrDefault(x => level >= x.LevelRange.Min && level <= x.LevelRange.Max)` at
/// `BotEquipmentFilterService.cs:137-189` and `BotHelper.cs:83-90` — and none of them draws. So
/// the caller runs the *unchanged* C# filter and pool hydration once per segment on which all of
/// those lookups are constant and ships one variant per segment, instead of one filtered template
/// per bot. A non-PMC or playerscav wave is always a single `[1..1]` variant, because non-PMC
/// level is the constant 1 (`BotLevelGenerator.cs:23-26`).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TemplateVariantWire {
    pub level_min: i32,
    pub level_max: i32,
    pub template: BotTemplateWire,
    pub loot_pools: BotLootCacheWire,
    /// The four `BotType` blocks the per-bot prelude draws from. They sit beside
    /// [`Self::template`] rather than inside [`BotTemplateWire`], which the single-bot and
    /// player-scav requests share and whose bytes stay unchanged. Required, not `Option`: the C#
    /// batcher always sends them, and a silent default would skip draws and shift the RNG stream
    /// invisibly.
    pub appearance: AppearanceWire,
    pub health: BotTypeHealthWire,
    pub skills: BotDbSkillsWire,
    pub experience_reward: IndexMap<String, MinMax<i32>>,
}

/// The request members that do not vary between the bots of one wave and are not database views:
/// live C# process state (the player's level, the raid's daylight, the one equipment cell a
/// barrier-invisible runtime writer keeps live — [`Self::live_equipment_mods`]), and (as
/// [`Self::template_variants`]) the templates and loot pools, which vary by level *band* rather
/// than by bot. Every config slice lives on [`BotViewsWire`] / the resident DB, `BotConfig.Equipment`
/// included since ABI 34.
///
/// Deserialized as a nested object rather than `#[serde(flatten)]`: flatten routes the whole map
/// through serde's buffering path, which would cost more than the duplication it removes.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SharedBotVaryingWire {
    /// `pmcProfile?.Info?.Level` **raw** — absent when the session has no PMC profile, or when
    /// that profile carries no level. The two blacklist resolutions default it differently
    /// (`?? 1` for the equipment path, written at `BotInventoryGenerator.cs:614` and its six
    /// siblings and reaching the call as `GetValueOrDefault(1)` at `:937-939`; `?? 0` for the
    /// weapon-mod path at `BotEquipmentModGenerator.cs:546`), and level 0 matches no `levelRange`
    /// where level 1 may, so the *nullable* level is what has to cross — a pre-defaulted `1` could
    /// not tell "level 1 with a profile" from "no profile" and would collapse the divergence.
    /// [`crate::bot::select_equipment_blacklists`] applies both defaults.
    #[serde(default)]
    pub generating_player_level: Option<i32>,
    pub is_night_time: bool,
    /// Live role → band EquipmentMods, both arms, every send. The one cell in the equipment
    /// graph that a barrier-invisible runtime writer (ReplayRandomisationClamps /
    /// GenerateAndAddEquipmentToBot) touches AND Rust reads. Everything else is resident.
    pub live_equipment_mods: IndexMap<String, Vec<LiveEquipmentModsBandWire>>,
    /// The wave's level-draw inputs. Present iff the wave is PMC: every other bot takes the
    /// constant `(1, 0)` without drawing (`BotLevelGenerator.cs:23-26`), so there is nothing to
    /// send. A PMC slice that arrives without it is an error envelope, never a panic.
    #[serde(default)]
    pub level_generation: Option<LevelGenerationWire>,
    /// Ascending, contiguous, covering `[level_min..level_max]`; exactly one `[1..1]` entry for a
    /// non-PMC or playerscav wave. `#[serde(default)]` because the single-bot request carries its
    /// template and loot pools at the top level instead.
    #[serde(default)]
    pub template_variants: Vec<TemplateVariantWire>,
}

/// One band of [`SharedBotVaryingWire::live_equipment_mods`]: the `levelRange` that identifies
/// which resident [`RandomisationDetails`] it overlays, and that band's live `EquipmentMods` map.
/// The C# sender enumerates the live `Randomisation` list the resident copy was published from and
/// sends the bands that *carry* an `EquipmentMods` map (`BotPayloadProjection.cs:101`), so what
/// arrives is that list's mod-carrying subsequence, in order — which is why
/// [`crate::bot::resolve_equipment`] pairs duplicate ranges positionally against the resident bands
/// whose [`RandomisationDetails::equipment_mods`] is `Some`, the matching subsequence.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveEquipmentModsBandWire {
    /// `#[serde(default)]` to match [`RandomisationDetails::level_range`]: the serializer omits a
    /// mod-nulled `LevelRange` outright (`JsonUtil` sets `WhenWritingNull`), so a strict field here
    /// would fail every `GenerateBotInventory` request for the very data whose *publish* the
    /// resident default just absorbed. The defaulted `(0, 0)` range matches no resident band, so
    /// the band drops, consistent with its two siblings.
    #[serde(default)]
    pub level_range: MinMax<i32>,
    pub equipment_mods: IndexMap<String, f64>,
}

/// The three request members that do vary per bot: identity, the test seed and the generation
/// details. The template and loot pools hydrated from the bot's level live on the shared block
/// as [`SharedBotVaryingWire::template_variants`] — the batch path draws the level natively and
/// picks the variant whose band covers it, so a wave ships one template per level segment
/// (typically one to three, up to ~8 for a full 1..79 range on shipped config) rather than one
/// per bot.
///
/// `details.bot_level` still rides the wire because the single-bot request reuses this view; the
/// batch projection sends 0 and the drawn level overwrites it before any consumer reads it.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BotSliceWire {
    pub bot_id: String,
    #[serde(default)]
    pub test_seed: Option<u64>,
    pub details: BotGenerationDetailsWire,
    /// True when the C# prelude drew the nickname "nikita" (case-insensitive) — the game-version
    /// draw's special case. The single-bot request sends `false` (the C# member is a non-nullable
    /// bool and `JsonUtil` only omits nulls); default false for anything that omits it.
    #[serde(default)]
    pub is_nikita: bool,
}

/// One wave: the `{epoch, viewsOverride?, …}` envelope around the shared varying block once, then
/// a slice per bot.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateBotInventoryBatchRequest {
    /// See [`GenerateBotInventoryRequest::epoch`].
    pub epoch: u64,
    /// See [`GenerateBotInventoryRequest::views_override`].
    #[serde(default)]
    pub views_override: Option<Box<BotViewsWire>>,
    pub shared: SharedBotVaryingWire,
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

/// The customization the batch drew (`SetBotAppearance` + the voice line) — batch-only, like
/// [`BotInventoryResult::level`].
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BotCustomizationResult {
    pub head: String,
    pub body: String,
    pub feet: String,
    pub hands: String,
    pub voice: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CurrentMaxWire {
    pub current: f64,
    pub maximum: f64,
}

/// `GenerateHealth`'s output. `updateTime: 0` / `immortal: false` are constants the C# side
/// writes; only the drawn values cross.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BotHealthResult {
    pub hydration: CurrentMaxWire,
    pub energy: CurrentMaxWire,
    pub temperature: CurrentMaxWire,
    /// Insertion order is the C# initializer order: Head, Chest, Stomach, LeftArm, RightArm,
    /// LeftLeg, RightLeg.
    pub body_parts: IndexMap<String, CurrentMaxWire>,
}

/// One randomised skill. The id is the raw template key — C# parses it into `SkillTypes` on
/// hydration, throwing per bot exactly where the legacy prelude threw.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillResult {
    pub id: String,
    pub progress: f64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BotSkillsResult {
    pub common: Vec<SkillResult>,
    pub mastering: Vec<SkillResult>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BotInventoryResult {
    pub inventory: BotBaseInventoryWire,
    /// Slot → grid state, from `bot_generator_helper::ContainerGrids::into_wire`. Empty when the
    /// request asked for the cache to be cleared (`BotInventoryGenerator.cs:114-117`).
    pub container_grids: IndexMap<String, ContainerDetailsWire>,
    /// Equipment *mod* slot → the chance the nighttime clamp (`:204`) left behind, for the C#
    /// caller to write back into its shared `BotConfig` object.
    pub randomisation_clamps: IndexMap<String, f64>,
    /// The level this bot drew, for the caller to write into `details.BotLevel` and `Info.Level`
    /// (`BotGenerator.cs:222-225`, `:270`). `None` on the single-bot path, which keeps its C# level
    /// generation — so that response's bytes are unchanged.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<i32>,
    /// The experience total that goes with [`Self::level`] (`BotLevelGenerator.cs:39-44`) →
    /// `Info.Experience`. `None` alongside it, for the same reason.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exp: Option<i32>,
    /// The prelude draws the batch arm owns. `None` on the single-bot and player-scav paths, for
    /// [`Self::level`]'s reason.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub customization: Option<BotCustomizationResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub health: Option<BotHealthResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skills: Option<BotSkillsResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settings_experience: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub game_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub member_category: Option<i32>,
    /// Absent on the nikita branch — C# leaves `SelectedMemberCategory` untouched there (quirk).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected_member_category: Option<i32>,
}

/// The native slice of one `KarmaLevel` entry from `PlayerScavConfig`. Ships per call —
/// override sends have no resident config to read, and the payload is small on a cold path.
/// `itemLimits` deliberately does not cross (applied C#-side; spec § Seam). `#[serde(default)]`
/// on all four members is defensive rather than required — it lands a missing member on an empty
/// map instead of `STATUS_BAD_ARGS`, the way every sibling bot wire struct defaults its fields.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KarmaSettingsWire {
    /// `KarmaLevel.Modifiers.Equipment`.
    #[serde(default)]
    pub equipment_modifiers: IndexMap<String, f64>,
    /// `KarmaLevel.Modifiers.Mod`.
    #[serde(default)]
    pub mod_modifiers: IndexMap<String, f64>,
    /// `KarmaLevel.EquipmentBlacklist`, re-keyed C#-side from `EquipmentSlots` to
    /// `slot.ToString()` (the STJ numeric-enum-key hazard `BotTypeInventoryView.Equipment`
    /// dodges the same way).
    #[serde(default)]
    pub equipment_blacklist: IndexMap<String, Vec<String>>,
    /// `KarmaLevel.LootItemsToAddChancePercent` — tpl → % chance, iterated in insertion order.
    #[serde(default)]
    pub loot_items_to_add_chance_percent: IndexMap<String, f64>,
}

/// `spt_generate_player_scav` request: the single-bot request plus the karma slice. The template
/// arrives with `generation` already karma-adjusted C#-side (item limits feed C#-side loot-pool
/// hydration) and `chances`/`inventory` raw for the native karma pieces.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneratePlayerScavRequest {
    pub epoch: u64,
    #[serde(default)]
    pub views_override: Option<Box<BotViewsWire>>,
    pub shared: SharedBotVaryingWire,
    pub bot: BotSliceWire,
    pub template: BotTemplateWire,
    pub loot_pools: BotLootCacheWire,
    pub karma: KarmaSettingsWire,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every required [`GenerateBotInventoryRequest`] member, as an override send. `testSeed` is
    /// deliberately absent — its omission is what exercises the missing-field → `None` path.
    const REQUEST_JSON: &str = r#"{
        "epoch":0,
        "viewsOverride":{
            "items":{"aaaaaaaaaaaaaaaaaaaaaab5":{"parent":"aaaaaaaaaaaaaaaaaaaaaab6","width":2,"height":1}},
            "itemPresets":{"p1":{"id":"p1","items":[]},"p2":{"id":"p2","items":[]}},
            "defaultPresetsByTpl":{"aaaaaaaaaaaaaaaaaaaaaab2":"p2"},
            "handbookPrices":{"aaaaaaaaaaaaaaaaaaaaaab4":12500.5},
            "expTable":[10],
            "bosses":["bossknight"],
            "botRolesWithDogTags":["pmcbear","pmcusec"],
            "bodyToFixedHands":{"aaaaaaaaaaaaaaaaaaaaaab7":"aaaaaaaaaaaaaaaaaaaaaab8"},
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
            "equipment":{"assault":{"weaponModLimits":{"scopeLimit":2},
                "forceRigWhenNoVest":true,"forceOnlyArmoredRigWhenNoArmor":false,
                "randomisation":[{"levelRange":{"min":1,"max":99},
                    "randomisedArmorSlots":["Headwear"],"equipmentMods":{"mod_nvg":40},
                    "nighttimeChanges":{"equipmentModsModifiers":{"mod_nvg":90}}}],
                "blacklist":[{"levelRange":{"min":1,"max":99},
                    "equipment":{"Headwear":["aaaaaaaaaaaaaaaaaaaaaaa9"]}}]}},
            "pmcConfig":{"forceHealingItemsIntoSecure":true,
                "forceArmband":{"enabled":true,"usec":"armband_usec","bear":"armband_bear"},
                "weaponHasEnhancementChancePercent":25},
            "repairKitWeapon":{"rarityWeight":{},"bonusTypeWeight":{},"Common":{},"Rare":{}},
            "configBlacklist":["aaaaaaaaaaaaaaaaaaaaaab3"]},
        "bot":{"botId":"bbbbbbbbbbbbbbbbbbbbbbbb",
        "details":{"role":"assault","roleLowercase":"assault","side":"Savage","botLevel":12,
            "isPmc":false,"isPlayerScav":false,"gameVersion":"standard","location":"bigmap",
            "botDifficulty":"normal","clearBotContainerCacheAfterGeneration":true}},
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
        "shared":{
        "generatingPlayerLevel":30,
        "isNightTime":true,
        "liveEquipmentMods":{"assault":[{"levelRange":{"min":1,"max":99},
            "equipmentMods":{"mod_nvg":40}}]}},
        "lootPools":{"backpackLoot":{"aaaaaaaaaaaaaaaaaaaaaab1":4}}
    }"#;

    #[test]
    fn generate_bot_inventory_request_deserializes() {
        let parsed: GenerateBotInventoryRequest = serde_json::from_str(REQUEST_JSON).unwrap();

        assert_eq!(parsed.epoch, 0);
        assert_eq!(parsed.bot.bot_id, "bbbbbbbbbbbbbbbbbbbbbbbb");
        // Absent `testSeed` is the production path.
        assert_eq!(parsed.bot.test_seed, None);

        assert_eq!(parsed.bot.details.role, "assault");
        assert_eq!(parsed.bot.details.role_lowercase, "assault");
        assert_eq!(parsed.bot.details.side, "Savage");
        assert_eq!(parsed.bot.details.bot_level, 12);
        assert!(!parsed.bot.details.is_pmc);
        assert!(!parsed.bot.details.is_player_scav);
        assert_eq!(parsed.bot.details.game_version, "standard");
        assert_eq!(parsed.bot.details.location.as_deref(), Some("bigmap"));
        assert_eq!(parsed.bot.details.bot_difficulty, "normal");
        assert!(
            parsed
                .bot
                .details
                .clear_bot_container_cache_after_generation
        );

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

        assert_eq!(parsed.shared.generating_player_level, Some(30));
        assert!(parsed.shared.is_night_time);
        // The only equipment cell still on the varying block: role → the live bands, keyed by the
        // `levelRange` the merge pairs them on.
        let live_band = &parsed.shared.live_equipment_mods["assault"][0];
        assert_eq!(
            (live_band.level_range.min, live_band.level_range.max),
            (1, 99)
        );
        assert_eq!(live_band.equipment_mods["mod_nvg"], 40.0);

        // Everything else rides the views (or, off the other arm, the resident `spt-bot` stem).
        let assault_equipment = &parsed.views_override.as_ref().unwrap().equipment["assault"];
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
        // The two equipment blacklists are no longer wire members at all: the bands ride the views'
        // `equipment` and `select_equipment_blacklist` picks one per resolution.
        let blacklist_band = &assault_equipment.blacklist.as_ref().unwrap()[0];
        assert_eq!(blacklist_band.level_range.min, 1);
        assert_eq!(blacklist_band.level_range.max, 99);
        assert!(
            blacklist_band.equipment.as_ref().unwrap()["Headwear"]
                .contains("aaaaaaaaaaaaaaaaaaaaaaa9")
        );
        assert_eq!(
            parsed.loot_pools.backpack_loot["aaaaaaaaaaaaaaaaaaaaaab1"],
            4.0
        );
        // Pools the payload omits deserialize empty, not missing.
        assert!(parsed.loot_pools.combined_pool_loot.is_empty());

        let views = parsed.views_override.as_ref().unwrap();
        // The thirteen config members that went resident: on the views block on this arm, off the
        // resident stems on the other.
        assert_eq!(views.bosses, vec!["bossknight"]);
        assert_eq!(views.durability.default.weapon.lowest_max, 60);
        assert_eq!(
            views.item_spawn_limits["assault"]["aaaaaaaaaaaaaaaaaaaaaaa7"],
            1.0
        );
        assert_eq!(views.wallet_loot.chance_percent, 10.0);
        assert_eq!(views.currency_stack_size["default"]["RUB"]["1000"], 1.0);
        assert_eq!(views.secure_container_ammo_stack_count, 3);
        assert!(views.disable_loot_on_bot_types.contains("bosstest"));
        assert!(
            views
                .low_profile_gas_block_tpls
                .contains("aaaaaaaaaaaaaaaaaaaaaaa8")
        );
        let food = views.loot_item_resource_randomization["assault"]
            .food
            .as_ref()
            .unwrap();
        assert_eq!(food.chance_max_resource_percent, 60.0);
        // Absent in the payload; C#'s non-nullable float lands on 0, not on "no randomisation".
        assert_eq!(food.resource_percent, 0.0);
        assert!(
            views.loot_item_resource_randomization["assault"]
                .meds
                .is_none()
        );
        assert!(views.pmc_config.force_healing_items_into_secure);
        assert!(views.pmc_config.force_armband.enabled);
        assert_eq!(views.pmc_config.force_armband.usec, "armband_usec");
        assert_eq!(views.pmc_config.force_armband.bear, "armband_bear");
        assert_eq!(views.pmc_config.weapon_has_enhancement_chance_percent, 25.0);
        assert!(views.repair_kit_weapon.rarity_weight.is_empty());
        assert!(views.config_blacklist.contains("aaaaaaaaaaaaaaaaaaaaaab3"));

        assert_eq!(views.item_presets["p1"].id.as_deref(), Some("p1"));
        // The default rides as an id and is resolved against `item_presets`, not inlined
        assert_eq!(
            views.default_presets_by_tpl["aaaaaaaaaaaaaaaaaaaaaab2"],
            "p2"
        );
        assert_eq!(views.item_presets["p2"].id.as_deref(), Some("p2"));
        assert_eq!(views.handbook_prices["aaaaaaaaaaaaaaaaaaaaaab4"], 12500.5);
        assert_eq!(views.items["aaaaaaaaaaaaaaaaaaaaaab5"].width, Some(2));
        assert_eq!(views.exp_table, vec![10]);
    }

    #[test]
    fn test_seed_is_read_when_present() {
        let json = REQUEST_JSON.replace(
            r#""botId":"bbbbbbbbbbbbbbbbbbbbbbbb","#,
            r#""botId":"bbbbbbbbbbbbbbbbbbbbbbbb","testSeed":42,"#,
        );
        let parsed: GenerateBotInventoryRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.bot.test_seed, Some(42));

        // Explicit null is the same as absent.
        let json = REQUEST_JSON.replace(
            r#""botId":"bbbbbbbbbbbbbbbbbbbbbbbb","#,
            r#""botId":"bbbbbbbbbbbbbbbbbbbbbbbb","testSeed":null,"#,
        );
        let parsed: GenerateBotInventoryRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.bot.test_seed, None);
    }

    /// The four prelude blocks every `templateVariants` entry carries. Every weight map is
    /// multi-entry on purpose: a single-entry map takes the `len() == 1` shortcut instead of the
    /// weighted draw, and an empty one errors the bot.
    fn prelude_blocks() -> serde_json::Map<String, serde_json::Value> {
        let serde_json::Value::Object(blocks) = serde_json::json!({
            "appearance": {
                "body": {"body_a": 1, "body_b": 3},
                "feet": {"feet_a": 1, "feet_b": 3},
                "hands": {"hands_a": 1, "hands_b": 3},
                "head": {"head_a": 1, "head_b": 3},
                "voice": {"voice_a": 1, "voice_b": 3},
            },
            "health": {
                "BodyParts": [
                    {"Chest": {"min": 80, "max": 85}, "Head": {"min": 35, "max": 35},
                     "LeftArm": {"min": 60, "max": 60}, "LeftLeg": {"min": 65, "max": 65},
                     "RightArm": {"min": 60, "max": 60}, "RightLeg": {"min": 65, "max": 65},
                     "Stomach": {"min": 70, "max": 75}},
                    {"Chest": {"min": 70, "max": 75}, "Head": {"min": 30, "max": 30},
                     "LeftArm": {"min": 50, "max": 50}, "LeftLeg": {"min": 55, "max": 55},
                     "RightArm": {"min": 50, "max": 50}, "RightLeg": {"min": 55, "max": 55},
                     "Stomach": {"min": 60, "max": 65}},
                ],
                "Energy": {"min": 80, "max": 100},
                "Hydration": {"min": 80, "max": 100},
                "Temperature": {"min": 36, "max": 40},
            },
            "skills": {
                "Common": {"BotReload": {"min": 100, "max": 200},
                    "BotSound": {"min": 100, "max": 200}},
                // Two entries, one explicitly null: the mastering loop and its `Option<MinMax>`
                // skip (which must not consume a draw) both run.
                "Mastering": {"Assault": {"min": 300, "max": 400}, "Pistol": null},
            },
            // Keyed by *difficulty*, which is what `GetExperienceRewardForKillByDifficulty` looks
            // the bot's `botDifficulty` up under. The three bands are ranges apart, so a lookup on
            // the wrong key lands outside the band the fixture's `normal` bots must draw from.
            "experienceReward": {"easy": {"min": 10, "max": 20},
                "normal": {"min": 100, "max": 200},
                "hard": {"min": 1000, "max": 2000}},
        }) else {
            unreachable!("the literal is an object")
        };

        blocks
    }

    /// The single-bot fixture reshaped into a batch request: the slice rides in `bots`, and the
    /// template and loot pools move into one full-coverage variant on the shared block.
    fn batch_request_json(level_generation: Option<serde_json::Value>) -> serde_json::Value {
        let mut request: serde_json::Value = serde_json::from_str(REQUEST_JSON).unwrap();
        let object = request.as_object_mut().unwrap();

        let slice = object.remove("bot").unwrap();
        let mut variant = serde_json::json!({
            "levelMin": 1,
            "levelMax": 99,
            "template": object.remove("template").unwrap(),
            "lootPools": object.remove("lootPools").unwrap(),
        });
        variant.as_object_mut().unwrap().extend(prelude_blocks());
        let shared = object.get_mut("shared").unwrap().as_object_mut().unwrap();
        shared.insert("templateVariants".to_owned(), serde_json::json!([variant]));
        if let Some(level_generation) = level_generation {
            shared.insert("levelGeneration".to_owned(), level_generation);
        }
        object.insert("bots".to_owned(), serde_json::json!([slice]));

        request
    }

    #[test]
    fn batch_request_deserializes_with_level_inputs() {
        let json = batch_request_json(Some(serde_json::json!({
            "levelMin": 5, "levelMax": 30,
        })));
        let parsed: GenerateBotInventoryBatchRequest = serde_json::from_value(json).unwrap();

        let level_generation = parsed.shared.level_generation.as_ref().unwrap();
        assert_eq!(level_generation.level_min, 5);
        assert_eq!(level_generation.level_max, 30);

        // The template and its loot pools ride once per band, not once per bot.
        let variant = &parsed.shared.template_variants[0];
        assert_eq!((variant.level_min, variant.level_max), (1, 99));
        assert_eq!(variant.template.chances.equipment["Headwear"], 75.0);
        assert_eq!(
            variant.loot_pools.backpack_loot["aaaaaaaaaaaaaaaaaaaaaab1"],
            4.0
        );

        // The slice is down to identity + seed + details.
        let slice = &parsed.bots[0];
        assert_eq!(slice.bot_id, "bbbbbbbbbbbbbbbbbbbbbbbb");
        assert_eq!(slice.test_seed, None);
        assert_eq!(slice.details.bot_level, 12);
    }

    #[test]
    fn a_resident_batch_request_with_epoch_and_varying_only_deserializes() {
        let mut json = batch_request_json(None);
        json["epoch"] = serde_json::json!(3);
        json.as_object_mut().unwrap().remove("viewsOverride");

        let parsed: GenerateBotInventoryBatchRequest = serde_json::from_value(json).unwrap();

        assert_eq!(parsed.epoch, 3);
        assert!(parsed.views_override.is_none());
        assert_eq!(parsed.bots.len(), 1);
        assert_eq!(parsed.bots[0].bot_id, "bbbbbbbbbbbbbbbbbbbbbbbb");
    }

    #[test]
    fn an_override_request_with_epoch_zero_deserializes() {
        let parsed: GenerateBotInventoryBatchRequest =
            serde_json::from_value(batch_request_json(None)).unwrap();

        assert_eq!(parsed.epoch, 0);
        let views = parsed.views_override.as_ref().unwrap();
        assert_eq!(views.items["aaaaaaaaaaaaaaaaaaaaaab5"].width, Some(2));
        assert_eq!(views.item_presets["p1"].id.as_deref(), Some("p1"));
        assert_eq!(
            views.default_presets_by_tpl["aaaaaaaaaaaaaaaaaaaaaab2"],
            "p2"
        );
        assert_eq!(views.handbook_prices["aaaaaaaaaaaaaaaaaaaaaab4"], 12500.5);
        assert_eq!(views.exp_table, vec![10]);

        // The view members a C# override send may omit parse to their empty defaults.
        let mut json = batch_request_json(None);
        let views = json["viewsOverride"].as_object_mut().unwrap();
        for key in ["handbookPrices", "expTable"] {
            views.remove(key);
        }
        let parsed: GenerateBotInventoryBatchRequest = serde_json::from_value(json).unwrap();
        let views = parsed.views_override.as_ref().unwrap();
        assert!(views.handbook_prices.is_empty());
        assert!(views.exp_table.is_empty());
    }

    #[test]
    fn a_single_bot_resident_request_deserializes() {
        let mut json: serde_json::Value = serde_json::from_str(REQUEST_JSON).unwrap();
        json["epoch"] = serde_json::json!(3);
        json.as_object_mut().unwrap().remove("viewsOverride");

        let parsed: GenerateBotInventoryRequest = serde_json::from_value(json).unwrap();

        assert_eq!(parsed.epoch, 3);
        assert!(parsed.views_override.is_none());
        assert_eq!(parsed.bot.bot_id, "bbbbbbbbbbbbbbbbbbbbbbbb");
        assert_eq!(parsed.shared.generating_player_level, Some(30));
        assert_eq!(parsed.template.chances.equipment["Headwear"], 75.0);
        assert_eq!(
            parsed.loot_pools.backpack_loot["aaaaaaaaaaaaaaaaaaaaaab1"],
            4.0
        );
    }

    #[test]
    fn a_batch_request_without_level_inputs_parses_to_none() {
        let parsed: GenerateBotInventoryBatchRequest =
            serde_json::from_value(batch_request_json(None)).unwrap();

        // A non-PMC wave sends no `levelGeneration` at all — the level is the constant 1.
        assert!(parsed.shared.level_generation.is_none());
        assert_eq!(parsed.shared.template_variants.len(), 1);
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
            level: None,
            exp: None,
            customization: None,
            health: None,
            skills: None,
            settings_experience: None,
            game_version: None,
            member_category: None,
            selected_member_category: None,
        })
        .unwrap();

        // The single-bot path leaves level/exp unset, so its response bytes are what they were
        // before the batch started drawing levels.
        assert_eq!(
            out.as_object().unwrap().keys().collect::<Vec<_>>(),
            vec!["inventory", "containerGrids", "randomisationClamps"]
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
        let grids = &out["containerGrids"]["TacticalVest"];
        assert_eq!(grids["containerTpl"], "aaaaaaaaaaaaaaaaaaaaaac1");
        assert_eq!(grids["containerItemId"], "aaaaaaaaaaaaaaaaaaaaaac2");
        assert_eq!(grids["grids"][0]["gridMap"][0][1], 1);
        assert_eq!(grids["grids"][0]["gridFull"], false);
        assert_eq!(out["randomisationClamps"]["Headwear"], 62.5);

        // The batch path sets both, and they ride out camelCase like the rest.
        let out = serde_json::to_value(BotInventoryResult {
            inventory: BotBaseInventoryWire::default(),
            container_grids: IndexMap::new(),
            randomisation_clamps: IndexMap::new(),
            level: Some(23),
            exp: Some(45_600),
            customization: None,
            health: None,
            skills: None,
            settings_experience: None,
            game_version: None,
            member_category: None,
            selected_member_category: None,
        })
        .unwrap();
        assert_eq!(out["level"], 23);
        assert_eq!(out["exp"], 45_600);
    }
}
