//! `Helpers/Bot/BotGeneratorHelper.cs` plus the per-bot container bookkeeping of
//! `Services/Bot/BotInventoryContainerService.cs`.
//!
//! Three things live here: the `Upd` block bolted onto every generated item
//! ([`generate_extra_properties_for_item`]), the equipment compatibility probe
//! ([`is_item_incompatible_with_current_items`]), and the occupancy grids an item+children is
//! packed into ([`ContainerGrids`]). The C# service is a DI singleton keyed by bot id; one native
//! call generates one bot, so the grids are a plain value owned by the caller and handed back
//! through [`crate::bot::models::BotInventoryResult::container_grids`].
//!
//! # RNG calls, in C# source order — the parity contract
//!
//! [`is_item_incompatible_with_current_items`] and the whole [`ContainerGrids`] surface draw
//! **nothing**. Every draw in this module comes from
//! `GenerateExtraPropertiesForItem` (`BotGeneratorHelper.cs:54-172`), in this order:
//!
//! 1. **Durability**, only when `MaxDurability > 0` (`:69`), and only down one of two arms:
//!    - `WeapClass` present (`:71`) → `GenerateWeaponRepairableProperties` (`:215-221`):
//!      `GetRandomizedMaxWeaponDurability` (1 `GetInt`) then `GetRandomizedWeaponDurability`
//!      (1 `GetInt`) — **2 draws**, always.
//!    - else `ArmorClass` present (`:77`) → `GenerateArmorRepairableProperties` (`:229-245`): an
//!      `ArmorClass` of exactly 0 short-circuits to the template's own max durability with
//!      **0 draws**; otherwise `GetRandomizedMaxArmorDurability` (1 `GetDouble`, **PMC roles
//!      only**) then `GetRandomizedArmorDurability` (1 `GetInt`) — 2 draws for a PMC, 1 for
//!      everyone else.
//!    - neither → 0 draws. `HasHinge` (`:85`) and `Foldable` (`:91`) never draw.
//! 2. `WeapFireType` (`:97-103`) — `GetArrayValue`. Unreachable in practice and fatal when
//!    reached, see [`generate_extra_properties_for_item`].
//! 3. `MaxHpResource` non-null and non-zero (`:106`) → `GetRandomizedResourceValue` with
//!    `randomisationSettings?.Meds`.
//! 4. `MaxResource` and `FoodUseTime` both non-null (`:115`) → `GetRandomizedResourceValue` with
//!    `randomisationSettings?.Food`.
//! 5. `Parent == FLASHLIGHT` (`:125`) → 1 `GetChance100`, **else if** `Parent == TACTICAL_COMBO`
//!    (`:135`) → 1 `GetChance100`. Mutually exclusive.
//! 6. `Parent == NIGHT_VISION` (`:144`) → 1 `GetChance100`. Independent of 5.
//! 7. `HasHinge && FaceShieldComponent` (`:157`) → 1 `GetChance100`.
//!
//! `GetRandomizedResourceValue` (`:180-197`) is itself 0, 1 or 2 draws: a null config slice
//! short-circuits `||` before the roll and draws **nothing**; otherwise 1 `GetChance100`, and if
//! that fails and the max is not ~1, a further `GetDouble`. `GetPercentOfValue` between them draws
//! nothing.
//!
//! `GetBotEquipmentSettingFromConfig` (`:204-207`) is resolved between steps 4 and 5 and draws
//! nothing.
use indexmap::IndexMap;

use crate::bot::BotContext;
use crate::bot::durability_limits_helper::{
    get_randomized_armor_durability, get_randomized_max_armor_durability,
    get_randomized_max_weapon_durability, get_randomized_weapon_durability,
};
use crate::bot::models::{
    ChooseRandomCompatibleModResult, ContainerDetailsWire, ContainerMapDetailsWire,
    EquipmentFilters, RandomisedResourceValues,
};
use crate::loot::container_extensions::{container_is_full, find_slot_for_item};
use crate::loot::item_helper::{LootError, get_item};
use crate::loot::models::{
    Diagnostic, ERROR, Item, ItemLocation, ItemRotation, ItemView, Upd, UpdFoldable, UpdFoodDrink,
    UpdLight, UpdMedKit, UpdRepairable, UpdTogglable, WARNING,
};
use crate::loot::random_util::{get_chance_100, get_double, get_percent_of_value, round_to_digits};

/// `BaseClasses.FLASHLIGHT` (`Models/Enums/BaseClasses.cs:39`).
pub(crate) const FLASHLIGHT: &str = "55818b084bdc2d5b648b4571";
/// `BaseClasses.TACTICAL_COMBO` (`:120`).
pub(crate) const TACTICAL_COMBO: &str = "55818b164bdc2ddc698b456c";
/// `BaseClasses.NIGHT_VISION` (`:85`).
const NIGHT_VISION: &str = "5a2c3a9486f774688b05e574";
/// `BaseClasses.ITEM` (`:57`).
const ITEM: &str = "54009119af1c881c07000029";
/// `BaseClasses.FUNCTIONAL_MOD` (`:45`).
const FUNCTIONAL_MOD: &str = "550aa4154bdc2dd8348b456b";
/// `BaseClasses.MOD` (`:79`).
const MOD: &str = "5448fe124bdc2da5018b4567";

/// `BotGeneratorHelper._slotsWithNoCompatIssues` (`BotGeneratorHelper.cs:35-42`).
const SLOTS_WITH_NO_COMPAT_ISSUES: [&str; 5] = [
    "Scabbard",
    "Backpack",
    "SecuredContainer",
    "Holster",
    "ArmBand",
];

/// `BotGeneratorHelper._pmcTypes` (`BotGeneratorHelper.cs:44`) — `Sides.PmcBear`/`Sides.PmcUsec`
/// lowercased. Deliberately *not* `BotHelper._pmcTypeIds`, which also carries bare `usec`/`bear`;
/// `durability_limits_helper::is_bot_pmc` is the one that uses those.
const PMC_TYPES: [&str; 2] = ["pmcbear", "pmcusec"];

// ---------------------------------------------------------------------------
// Upd generation
// ---------------------------------------------------------------------------

/// `BotGeneratorHelper.GenerateExtraPropertiesForItem` (`BotGeneratorHelper.cs:54-172`).
///
/// The C# `itemTemplate` is nullable and every read null-propagates through it; no call site passes
/// null, so it is a plain reference here. `botRole` stays optional because the durability roll
/// branches on it, and `forceStackObjectsCount` is the flag `PlayerScavGenerator.cs:177`,
/// `BotInventoryGenerator.cs:580` and `BotLootGenerator.cs:516` set.
///
/// # Errors
///
/// Where the C# throws: any durability config error from
/// [`crate::bot::durability_limits_helper`], and the `WeapFireType` arm. That arm is
/// `WeapFireType?.Count == 0` (`:97`) — it fires only for a template whose fire-type list is
/// present *and empty*, where `Contains("fullauto")` is then necessarily false and
/// `GetArrayValue` hits `RandomUtil.GetRandomElement`'s
/// `throw new InvalidOperationException("Sequence contains no elements.")`. So no real weapon ever
/// gets a `FireMode`, and any template with an empty list aborts generation. Ported as-is: the
/// condition is almost certainly a `> 0` typo upstream, but "fix" it and every weapon starts
/// consuming a draw that the C# does not.
pub fn generate_extra_properties_for_item(
    ctx: &BotContext,
    item_template: &ItemView,
    bot_role: Option<&str>,
    force_stack_objects_count: bool,
) -> Result<Option<Upd>, LootError> {
    // BotRole property exists, we have specific bot randomisation values to make use of
    let randomisation_settings =
        bot_role.and_then(|role| ctx.loot_item_resource_randomization.get(role));

    let mut item_upd = Upd::default();
    let mut has_properties = false;

    if item_template.max_durability.is_some_and(|max| max > 0.0) {
        if item_template.weap_class.is_some() {
            // Is weapon
            item_upd.repairable = Some(generate_weapon_repairable_properties(ctx, bot_role)?);
            has_properties = true;
        } else if item_template.armor_class.is_some() {
            // Is armor
            item_upd.repairable = Some(generate_armor_repairable_properties(
                ctx,
                item_template,
                bot_role,
            )?);
            has_properties = true;
        }
    }

    if item_template.has_hinge.unwrap_or(false) {
        item_upd.togglable = Some(UpdTogglable { on: Some(true) });
        has_properties = true;
    }

    if item_template.foldable.unwrap_or(false) {
        item_upd.foldable = Some(UpdFoldable {
            folded: Some(false),
        });
        has_properties = true;
    }

    if item_template
        .weap_fire_type
        .as_ref()
        .is_some_and(Vec::is_empty)
    {
        // `Count == 0`, so `Contains("fullauto")` is false and the ternary always takes the
        // `GetArrayValue` arm — over an empty collection, which throws. No `FireMode` is reachable.
        return Err(LootError::new("Sequence contains no elements."));
    }

    // Must have value + not be 0 (e.g. Esmarch tourniquet) as they're single use
    if let Some(max_hp_resource) = item_template.max_hp_resource.filter(|max| *max != 0) {
        item_upd.med_kit = Some(UpdMedKit {
            hp_resource: Some(get_randomized_resource_value(
                f64::from(max_hp_resource),
                randomisation_settings.and_then(|settings| settings.meds.as_ref()),
            )),
        });
        has_properties = true;
    }

    if let Some(max_resource) = item_template.max_resource
        && item_template.food_use_time.is_some()
    {
        item_upd.food_drink = Some(UpdFoodDrink {
            hp_percent: Some(get_randomized_resource_value(
                f64::from(max_resource),
                randomisation_settings.and_then(|settings| settings.food.as_ref()),
            )),
        });
        has_properties = true;
    }

    let equipment_settings = get_bot_equipment_setting_from_config(ctx, bot_role);
    let parent = item_template.parent.as_deref();
    if parent == Some(FLASHLIGHT) {
        // Higher chance of laser/light at night
        let light_laser_active_chance = if ctx.is_night_time {
            equipment_settings
                .and_then(|filters| filters.light_is_active_night_chance_percent)
                .unwrap_or(50.0)
        } else {
            equipment_settings
                .and_then(|filters| filters.light_is_active_day_chance_percent)
                .unwrap_or(25.0)
        };

        item_upd.light = Some(UpdLight {
            is_active: Some(get_chance_100(light_laser_active_chance)),
            selected_mode: Some(0),
        });
        has_properties = true;
    } else if parent == Some(TACTICAL_COMBO) {
        // Get chance from botconfig for bot type, use 50% if no value found
        let light_laser_active_chance = equipment_settings
            .and_then(|filters| filters.laser_is_active_chance_percent)
            .unwrap_or(50.0);

        item_upd.light = Some(UpdLight {
            is_active: Some(get_chance_100(light_laser_active_chance)),
            selected_mode: Some(0),
        });
        has_properties = true;
    }

    if parent == Some(NIGHT_VISION) {
        // Get chance from botconfig for bot type
        let nvg_active_chance = if ctx.is_night_time {
            equipment_settings
                .and_then(|filters| filters.nvg_is_active_chance_night_percent)
                .unwrap_or(90.0)
        } else {
            equipment_settings
                .and_then(|filters| filters.nvg_is_active_chance_day_percent)
                .unwrap_or(15.0)
        };

        item_upd.togglable = Some(UpdTogglable {
            on: Some(get_chance_100(nvg_active_chance)),
        });
        has_properties = true;
    }

    // Togglable face shield — overwrites the `On: true` the HasHinge arm above set
    if item_template.has_hinge.unwrap_or(false)
        && item_template.face_shield_component.unwrap_or(false)
    {
        let face_shield_active_chance = equipment_settings
            .and_then(|filters| filters.face_shield_is_active_chance_percent)
            .unwrap_or(75.0);

        item_upd.togglable = Some(UpdTogglable {
            on: Some(get_chance_100(face_shield_active_chance)),
        });
        has_properties = true;
    }

    if force_stack_objects_count {
        // Ensure property is set
        item_upd.stack_objects_count.get_or_insert(1.0);
    }

    // Some items (weapon mods) may not have any props, and we don't want an empty Upd object
    Ok(if has_properties || force_stack_objects_count {
        Some(item_upd)
    } else {
        None
    })
}

/// `BotGeneratorHelper.GetBotEquipmentSettingFromConfig` (`BotGeneratorHelper.cs:204-207`).
///
/// The C# dereferences `botRole` unconditionally through `GetBotEquipmentRole`, so a null role is
/// an NRE there; no call site passes one, and `None` here falls through to the same literal
/// defaults an unmapped role gets.
fn get_bot_equipment_setting_from_config<'a>(
    ctx: &'a BotContext,
    bot_role: Option<&str>,
) -> Option<&'a EquipmentFilters> {
    ctx.equipment.get(get_bot_equipment_role(bot_role?))
}

/// `BotGeneratorHelper.GetBotEquipmentRole` (`BotGeneratorHelper.cs:440-443`) — `pmcBEAR`/`pmcUSEC`
/// collapse to `pmc`, everything else passes through.
pub fn get_bot_equipment_role(bot_role: &str) -> &str {
    if PMC_TYPES.contains(&bot_role.to_ascii_lowercase().as_str()) {
        "pmc"
    } else {
        bot_role
    }
}

/// `BotGeneratorHelper.GetRandomizedResourceValue` (`BotGeneratorHelper.cs:180-197`).
///
/// The `||` short-circuit is load-bearing: a null config slice returns the max **without drawing**.
fn get_randomized_resource_value(
    max_resource: f64,
    randomization_values: Option<&RandomisedResourceValues>,
) -> f64 {
    let Some(randomization_values) = randomization_values else {
        return max_resource;
    };

    if get_chance_100(randomization_values.chance_max_resource_percent) {
        return max_resource;
    }

    // `MathExtensions.Approx(value, 1)` — `Math.Abs(value - 1) <= 0.001`.
    if (max_resource - 1.0).abs() <= 0.001 {
        return 1.0;
    }

    // Generate a randomised min value the resource could have
    let min = 1.0f64.max(get_percent_of_value(
        randomization_values.resource_percent,
        max_resource,
        0,
    ));

    // Choose value from randomised min and resource max possible
    get_double(min, max_resource)
}

/// `BotGeneratorHelper.GenerateWeaponRepairableProperties` (`BotGeneratorHelper.cs:215-221`).
///
/// The `Math.Round(x, 5)` on both values lives here, not in `durability_limits_helper` — the C#
/// applies it at this layer and the raw roll is what the helper hands back.
fn generate_weapon_repairable_properties(
    ctx: &BotContext,
    bot_role: Option<&str>,
) -> Result<UpdRepairable, LootError> {
    let max_durability =
        get_randomized_max_weapon_durability(bot_role, ctx.bosses, ctx.durability)?;
    let current_durability =
        get_randomized_weapon_durability(bot_role, max_durability, ctx.bosses, ctx.durability)?;

    Ok(UpdRepairable {
        durability: Some(round_to_digits(current_durability, 5)),
        max_durability: Some(round_to_digits(max_durability, 5)),
        ..Default::default()
    })
}

/// `BotGeneratorHelper.GenerateArmorRepairableProperties` (`BotGeneratorHelper.cs:229-245`).
fn generate_armor_repairable_properties(
    ctx: &BotContext,
    item_template: &ItemView,
    bot_role: Option<&str>,
) -> Result<UpdRepairable, LootError> {
    let (max_durability, current_durability) = if item_template.armor_class == Some(0) {
        // C# reads `MaxDurability.Value` twice; the caller already proved it is `> 0`.
        let max = item_template.max_durability.unwrap_or_default();

        (max, max)
    } else {
        let max = get_randomized_max_armor_durability(
            item_template.max_durability,
            bot_role,
            ctx.durability,
        )?;
        let current = get_randomized_armor_durability(bot_role, max, ctx.bosses, ctx.durability)?;

        (max, current)
    };

    Ok(UpdRepairable {
        durability: Some(round_to_digits(current_durability, 5)),
        max_durability: Some(round_to_digits(max_durability, 5)),
        ..Default::default()
    })
}

// ---------------------------------------------------------------------------
// Incompatibility
// ---------------------------------------------------------------------------

/// `BotGeneratorHelper.IsItemIncompatibleWithCurrentItems` (`BotGeneratorHelper.cs:254-428`).
/// Draws nothing.
///
/// Two deviations, both from the flattened [`ItemView`]:
/// - The `itemToEquip?.Properties is null` arm (`:293-313`) has no analog — a view row *is* the
///   flattened `_props`, so "present but propless" is not expressible. Every template in the
///   database that reaches this call has `_props`.
/// - `blockingItem.Id` in the two blocked-by messages is the equipped item's *tpl*, which is what
///   `TemplateItem.Id` holds.
pub fn is_item_incompatible_with_current_items(
    ctx: &mut BotContext,
    items_equipped: &[Item],
    tpl_to_check: &str,
    equipment_slot: &str,
) -> ChooseRandomCompatibleModResult {
    // Skip slots that have no incompatibilities
    if SLOTS_WITH_NO_COMPAT_ISSUES.contains(&equipment_slot) {
        return ChooseRandomCompatibleModResult {
            incompatible: Some(false),
            found: Some(false),
            reason: Some(String::new()),
            ..Default::default()
        };
    }

    let items = ctx.items;

    let Some(item_to_equip) = get_item(items, tpl_to_check) else {
        ctx.diagnostics.push(Diagnostic {
            level: WARNING.to_owned(),
            locale_key: Some("bot-invalid_item_compatibility_check".to_owned()),
            args: Some(serde_json::json!({
                "itemTpl": tpl_to_check,
                "slot": equipment_slot,
            })),
            message: None,
        });

        return ChooseRandomCompatibleModResult {
            incompatible: Some(true),
            found: Some(false),
            reason: Some(format!(
                "item: {tpl_to_check} does not exist in the database"
            )),
            ..Default::default()
        };
    };

    let item_to_equip_name = item_to_equip.name.clone().unwrap_or_default();

    // Does an equipped item have a property that blocks the desired item - check for prop "BlocksX"
    // .e.g BlocksEarpiece / BlocksFaceCover
    let blocking_item = items_equipped.iter().find(|equipped| {
        has_blocking_property(get_item(items, &equipped.template), equipment_slot)
    });
    if let Some(blocking_item) = blocking_item {
        return blocked_by(
            tpl_to_check,
            &item_to_equip_name,
            equipment_slot,
            &blocking_item.template,
            blocking_name(items, &blocking_item.template),
        );
    }

    // Check if any of the current inventory templates have the incoming item defined as incompatible
    let blocking_item = items_equipped.iter().find(|equipped| {
        get_item(items, &equipped.template).is_some_and(|template| {
            template
                .conflicting_items
                .as_ref()
                .is_some_and(|conflicting| conflicting.iter().any(|tpl| tpl == tpl_to_check))
        })
    });
    if let Some(blocking_item) = blocking_item {
        return blocked_by(
            tpl_to_check,
            &item_to_equip_name,
            equipment_slot,
            &blocking_item.template,
            blocking_name(items, &blocking_item.template),
        );
    }

    // Does item being checked get blocked/block existing item
    for (blocks, container) in [
        (item_to_equip.blocks_headwear, "Headwear"),
        (item_to_equip.blocks_face_cover, "FaceCover"),
        (item_to_equip.blocks_earpiece, "Earpiece"),
        (item_to_equip.blocks_armor_vest, "ArmorVest"),
    ] {
        if !blocks.unwrap_or(false) {
            continue;
        }

        if let Some(existing) = items_equipped
            .iter()
            .find(|item| item.slot_id.as_deref() == Some(container))
        {
            let existing_slot = existing.slot_id.clone().unwrap_or_default();

            return ChooseRandomCompatibleModResult {
                incompatible: Some(true),
                found: Some(false),
                reason: Some(format!(
                    "{tpl_to_check} {item_to_equip_name} is blocked by: {} in slot: {existing_slot}",
                    existing.template
                )),
                slot_blocked: Some(true),
                ..Default::default()
            };
        }
    }

    // Check if the incoming item has any inventory items defined as incompatible
    let blocking_inventory_item = items_equipped.iter().find(|item| {
        item_to_equip
            .conflicting_items
            .as_ref()
            .is_some_and(|conflicting| conflicting.contains(&item.template))
    });
    if let Some(blocking_inventory_item) = blocking_inventory_item {
        let slot_id = blocking_inventory_item.slot_id.clone().unwrap_or_default();

        return ChooseRandomCompatibleModResult {
            incompatible: Some(true),
            reason: Some(format!(
                "{tpl_to_check} blocks existing item {} in slot {slot_id}",
                blocking_inventory_item.template
            )),
            ..Default::default()
        };
    }

    ChooseRandomCompatibleModResult {
        incompatible: Some(false),
        reason: Some(String::new()),
        ..Default::default()
    }
}

/// The identical result the two `blockingItem` arms (`BotGeneratorHelper.cs:321-342`) build.
fn blocked_by(
    tpl_to_check: &str,
    item_to_equip_name: &str,
    equipment_slot: &str,
    blocking_tpl: &str,
    blocking_name: String,
) -> ChooseRandomCompatibleModResult {
    ChooseRandomCompatibleModResult {
        incompatible: Some(true),
        found: Some(false),
        reason: Some(format!(
            "{tpl_to_check} {item_to_equip_name} in slot: {equipment_slot} blocked by: \
             {blocking_tpl} {blocking_name}"
        )),
        slot_blocked: Some(true),
        ..Default::default()
    }
}

fn blocking_name(items: &IndexMap<String, ItemView>, tpl: &str) -> String {
    get_item(items, tpl)
        .and_then(|template| template.name.clone())
        .unwrap_or_default()
}

/// `BotGeneratorHelper.HasBlockingProperty` (`BotGeneratorHelper.cs:430-433`) reading
/// `TemplateItem.Blocks` (`TemplateItem.cs:48-63`).
///
/// That dictionary has exactly seven keys, so only `LeftStance`, `Collapsible`, `Earpiece`,
/// `Eyewear`, `FaceCover`, `Folding` and `Headwear` can ever match an `equipmentSlot`; of those
/// only four are `EquipmentSlots` members, and `ArmorVest` is *not* among them despite
/// `BlocksArmorVest` existing. `TryGetValue` on any other slot name misses and returns false.
fn has_blocking_property(item: Option<&ItemView>, blocking_property_name: &str) -> bool {
    let Some(item) = item else {
        return false;
    };

    match blocking_property_name {
        "LeftStance" => item.block_left_stance,
        "Collapsible" => item.blocks_collapsible,
        "Earpiece" => item.blocks_earpiece,
        "Eyewear" => item.blocks_eyewear,
        "FaceCover" => item.blocks_face_cover,
        "Folding" => item.blocks_folding,
        "Headwear" => item.blocks_headwear,
        _ => return false,
    }
    .unwrap_or(false)
}

// ---------------------------------------------------------------------------
// Container grids
// ---------------------------------------------------------------------------

/// `Models/Enums/ItemAddedResult.cs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemAddedResult {
    Unknown,
    Success,
    NoSpace,
    NoContainers,
    IncompatibleItem,
}

/// The per-bot half of `Services/Bot/BotInventoryContainerService.cs`.
///
/// The C# is a singleton holding `ConcurrentDictionary<botId, Dictionary<EquipmentSlots, …>>`; one
/// native call generates one bot, so this is that inner dictionary and nothing more. Slot names are
/// the `EquipmentSlots` member names as strings, and insertion order is preserved so the wire form
/// round-trips deterministically.
///
/// `AddItemToBotContainerFixedPosition` (`:159-263`) is not ported — its only caller is the C#
/// wallet path, which stays C#-side.
#[derive(Debug, Default)]
pub struct ContainerGrids {
    containers: IndexMap<String, ContainerDetailsWire>,
}

impl ContainerGrids {
    /// `BotInventoryContainerService.AddEmptyContainerToBot` (`:26-40`) plus the `ContainerDetails`
    /// constructor (`:417-432`), which sizes one `int[CellsV, CellsH]` per grid on the container's
    /// template.
    ///
    /// C# NREs when the container tpl is missing from the database or carries no `Grids`; both
    /// yield an entry with zero grids here, which `try_add_item_to_bot_container` then reports as
    /// `NoContainers`.
    pub fn add_empty_container(
        &mut self,
        ctx: &BotContext,
        container_name: &str,
        container_inventory_item: &Item,
    ) {
        if self.containers.contains_key(container_name) {
            return;
        }

        let grids = get_item(ctx.items, &container_inventory_item.template)
            .and_then(|template| template.grids.as_ref())
            .map(|grids| {
                grids
                    .iter()
                    .map(|grid| ContainerMapDetailsWire {
                        // Rows = CellsV, columns = CellsH. Clamped: a negative count would abort
                        // the process on allocation.
                        grid_map: vec![
                            vec![0u8; grid.cells_h.unwrap_or(0).max(0) as usize];
                            grid.cells_v.unwrap_or(0).max(0) as usize
                        ],
                        grid_full: false,
                    })
                    .collect()
            })
            .unwrap_or_default();

        self.containers.insert(
            container_name.to_owned(),
            ContainerDetailsWire {
                container_tpl: container_inventory_item.template.clone(),
                container_item_id: container_inventory_item.id.clone(),
                grids,
            },
        );
    }

    /// `BotGeneratorHelper.AddItemWithChildrenToEquipmentSlot` (`BotGeneratorHelper.cs:455-529`).
    ///
    /// `equipmentSlots` is a `HashSet<EquipmentSlots>` in C# and a slice here — the caller controls
    /// the iteration order, which decides which container is tried first. `rootItemId` and
    /// `rootItemTplId` are separate parameters there too, and every call site passes the first
    /// item's id and tpl.
    ///
    /// On success `item_with_children[0]` is mutated (parent, slot, location) and the whole list is
    /// appended to `inventory`, exactly as the C# adds the same object references it just edited.
    pub fn add_item_with_children_to_equipment_slot(
        &mut self,
        ctx: &mut BotContext,
        equipment_slots: &[String],
        root_item_id: &str,
        root_item_tpl_id: &str,
        item_with_children: &mut [Item],
        inventory: &mut Vec<Item>,
    ) -> ItemAddedResult {
        // Track how many containers are unable to be found
        let mut missing_container_count = 0;
        for equipment_slot_id in equipment_slots {
            // Get container from inventory to put item into
            let container = inventory
                .iter()
                .find(|item| item.slot_id.as_deref() == Some(equipment_slot_id.as_str()))
                .cloned();
            let Some(container) = container else {
                missing_container_count += 1;
                if missing_container_count == equipment_slots.len() {
                    // Bot doesn't have any containers we want to add item to
                    ctx.diagnostics.push(debug_diagnostic(format!(
                        "Unable to add item: {} to bot as it lacks the following containers: {}",
                        item_with_children
                            .first()
                            .map(|item| item.template.as_str())
                            .unwrap_or_default(),
                        equipment_slots.join(",")
                    )));

                    return ItemAddedResult::NoContainers;
                }

                // No container of desired type found, skip to next container type
                continue;
            };

            // Get container details from db
            let Some(container_db_details) = get_item(ctx.items, &container.template) else {
                ctx.diagnostics.push(Diagnostic {
                    level: WARNING.to_owned(),
                    locale_key: Some("bot-missing_container_with_tpl".to_owned()),
                    args: Some(serde_json::Value::String(container.template.clone())),
                    message: None,
                });

                // Bad item, skip
                continue;
            };

            if container_db_details
                .grids
                .as_ref()
                .is_none_or(Vec::is_empty)
            {
                // Container has no slots to hold items, skip to next container
                continue;
            }

            // Get x/y grid size of item
            let (item_width, item_height) = get_item_size(
                ctx.items,
                root_item_tpl_id,
                root_item_id,
                item_with_children,
                &mut ctx.diagnostics,
            );

            let result = self.try_add_item_to_bot_container(
                ctx,
                equipment_slot_id,
                item_with_children,
                inventory,
                item_width,
                item_height,
            );
            if result != ItemAddedResult::Success {
                // Failed to add to container, try next
                continue;
            }

            return result;
        }

        ItemAddedResult::NoSpace
    }

    /// `BotInventoryContainerService.TryAddItemToBotContainer` (`:52-146`).
    ///
    /// The grid walk is row-major: `FindSlotForItem` iterates rows outermost and columns innermost
    /// (see `loot::container_extensions`), and the grids themselves are walked in the container
    /// template's declared order with `gridIndex` tracked separately from the loop variable.
    pub fn try_add_item_to_bot_container(
        &mut self,
        ctx: &BotContext,
        container_name: &str,
        item_and_children: &mut [Item],
        bot_inventory: &mut Vec<Item>,
        item_width: i32,
        item_height: i32,
    ) -> ItemAddedResult {
        if item_and_children.is_empty() {
            return ItemAddedResult::IncompatibleItem;
        }

        let mut add_result = ItemAddedResult::Unknown;

        // Find bot and the container we will attempt to add into
        let Some(container_details) = self.containers.get_mut(container_name) else {
            return ItemAddedResult::NoContainers;
        };
        if container_details.grids.is_empty() {
            // No grids, cannot add item
            return ItemAddedResult::NoContainers;
        }

        // Multiple containers, maybe next one allows item, only break out of loop for the
        // containers grids
        if !item_allowed_in_container(ctx, &container_details.container_tpl, item_and_children) {
            return ItemAddedResult::IncompatibleItem;
        }

        // The db grids the details were built from, in the same order
        let Some(grids_db) = get_item(ctx.items, &container_details.container_tpl)
            .and_then(|template| template.grids.as_ref())
        else {
            return add_result;
        };

        // Try to fit item into one of the containers' grids
        for (grid_index, grid_db) in grids_db.iter().enumerate() {
            let Some(grid_details) = container_details.grids.get_mut(grid_index) else {
                // C# indexes ContainerGridDetails[gridIndex] and would throw; the two lists are
                // built from the same template so this is unreachable with a stable database.
                break;
            };

            if grid_details.grid_full {
                // Skip to next grid
                continue;
            }

            if is_item_bigger_than_grid(&grid_details.grid_map, item_width, item_height) {
                // Skip to next grid
                continue;
            }

            // Look for a slot in the grid to place item
            let find_slot_result =
                find_slot_for_item(&grid_details.grid_map, item_width, item_height);
            if find_slot_result.success {
                // It Fits!

                // Set items parent to Id of container
                let root_item = &mut item_and_children[0];
                root_item.parent_id = Some(container_details.container_item_id.clone());
                // Can be name of container e.g. "Backpack" OR "2"/"3"/"4"/"5" depending on which
                // grid of a container item is added to
                root_item.slot_id = grid_db.name.clone();
                root_item.location = serde_json::to_value(ItemLocation {
                    x: Some(find_slot_result.x),
                    y: Some(find_slot_result.y),
                    r: if find_slot_result.rotation {
                        ItemRotation::Vertical
                    } else {
                        ItemRotation::Horizontal
                    },
                    is_searched: None,
                    rotation: None,
                })
                .ok();

                // Flag result as success to report to caller
                add_result = ItemAddedResult::Success;

                // Update grid with slots taken up by above item
                fill_grid_region(
                    &mut grid_details.grid_map,
                    find_slot_result.x,
                    find_slot_result.y,
                    if find_slot_result.rotation {
                        item_height
                    } else {
                        item_width
                    },
                    if find_slot_result.rotation {
                        item_width
                    } else {
                        item_height
                    },
                );

                // Add item into bots inventory
                bot_inventory.extend_from_slice(item_and_children);

                // Exit loop, we've found a slot for item
                break;
            }

            // Didn't fit, flag as no space, hopefully next grid has space
            add_result = ItemAddedResult::NoSpace;

            flag_grid_if_full(grid_details, item_width, item_height);
        }

        add_result
    }

    /// `BotInventoryContainerService.GetBotContainer` (`:393-396`).
    #[allow(
        dead_code,
        reason = "no production reader: the generators hold the `ContainerGrids` they built and the C# side reads the state back off `into_wire`, so only the tests call this"
    )]
    pub fn get(&self, container_name: &str) -> Option<&ContainerDetailsWire> {
        self.containers.get(container_name)
    }

    /// The final grid state, for `BotInventoryResult.container_grids`. Task 14 rebuilds the C#
    /// service's cache from this.
    pub fn into_wire(self) -> IndexMap<String, ContainerDetailsWire> {
        self.containers
    }
}

/// `BotInventoryContainerService.FillGridRegion` (`:289-300`) — rows outer, columns inner, no
/// bounds or collision check. Not `try_fill_container_map_with_item`: this one never fails and
/// never reads what it overwrites.
fn fill_grid_region(grid: &mut [Vec<u8>], x: i32, y: i32, item_width: i32, item_height: i32) {
    for row in y..y + item_height {
        for column in x..x + item_width {
            grid[row as usize][column as usize] = 1;
        }
    }
}

/// `BotInventoryContainerService.FlagGridIfFull` (`:308-323`).
fn flag_grid_if_full(
    grid_details: &mut ContainerMapDetailsWire,
    item_width: i32,
    item_height: i32,
) {
    // If item is 1x1 and it failed to fit, grid must be full
    if item_height == 1 && item_width == 1 {
        grid_details.grid_full = true; // Flag now so later items can skip grid

        return;
    }

    // Check if grid is full and flag
    if container_is_full(&grid_details.grid_map) {
        grid_details.grid_full = true;
    }
}

/// `BotInventoryContainerService.IsItemBiggerThanGrid` (`:375-386`).
fn is_item_bigger_than_grid(grid: &[Vec<u8>], item_width: i32, item_height: i32) -> bool {
    let grid_height = grid.len() as i32;
    let grid_width = grid.first().map_or(0, Vec::len) as i32;

    // Check if it can fit in either orientation
    let fits_normally = item_width <= grid_width && item_height <= grid_height;
    let fits_rotated = item_height <= grid_width && item_width <= grid_height;

    // Fails both checks
    !fits_normally && !fits_rotated
}

/// `BotInventoryContainerService.ItemAllowedInContainer` (`:331-366`) — only the *first* grid's
/// *first* filter is consulted, on the comment's assumption that all grids share limitations.
fn item_allowed_in_container(
    ctx: &BotContext,
    container_tpl: &str,
    item_and_children: &[Item],
) -> bool {
    // Assume all grids have same limitations
    let first_slot_grid = get_item(ctx.items, container_tpl)
        .and_then(|template| template.grids.as_ref())
        .and_then(|grids| grids.first());
    let prop_filters = first_slot_grid.and_then(|grid| grid.filters.as_ref());

    // No filters, item is fine to add
    let Some(prop_filters) = prop_filters.filter(|filters| !filters.is_empty()) else {
        return true;
    };

    // Check if item base type is excluded
    let item_parent = item_and_children
        .first()
        .and_then(|item| get_item(ctx.items, &item.template))
        .and_then(|template| template.parent.as_deref())
        .unwrap_or_default();

    // if item to add is found in exclude filter, not allowed
    let first_filter = prop_filters.first();
    let excluded_filter = first_filter.and_then(|filter| filter.excluded_filter.as_deref());
    if excluded_filter.is_some_and(|excluded| excluded.iter().any(|tpl| tpl == item_parent)) {
        return false;
    }

    let filter = first_filter
        .and_then(|filter| filter.filter.as_deref())
        .unwrap_or_default();

    // If Filter array only contains 1 filter and it is for basetype 'item', allow it
    if filter.len() == 1 && filter.iter().any(|tpl| tpl == ITEM) {
        return true;
    }

    // If allowed filter has something in it + filter doesn't have basetype 'item', not allowed
    if !filter.is_empty() && !filter.iter().any(|tpl| tpl == item_parent) {
        return false;
    }

    true
}

/// `InventoryHelper.GetItemSize` → `GetSizeByInventoryItemHash`
/// (`Helpers/Profile/InventoryHelper.cs:606-748`).
///
/// **Not** `loot::item_helper::get_item_size`, which ports `ItemHelper.GetItemSize` — the two
/// disagree and this is the one the bot container path calls. The differences that matter:
/// the root's own `ExtraSize*` is ignored (only children contribute), children are only walked at
/// all when the root is a `WEAPON`/`FUNCTIONAL_MOD`/`MOD`, only children whose `slotId` starts
/// `mod_` count, and folding both shrinks the root and drops folded children.
///
/// The C# breadth-first queue is replaced by a scan of `items` per parent; both aggregate with
/// `max`/`+=`, so order cannot change the answer. Where C# dereferences a child template it failed
/// to look up (`:704-710`) this skips the child instead of panicking behind the FFI boundary.
pub(crate) fn get_item_size(
    items_view: &IndexMap<String, ItemView>,
    item_tpl: &str,
    item_id: &str,
    items: &[Item],
    diagnostics: &mut Vec<Diagnostic>,
) -> (i32, i32) {
    // Invalid item
    let Some(item_template) = get_item(items_view, item_tpl) else {
        // Two separate `logger.Error` calls in C# (`:625` and `:639`), both keyed on the tpl.
        for locale_key in [
            "inventory-invalid_item_missing_from_db",
            "inventory-return_default_size",
        ] {
            diagnostics.push(Diagnostic {
                level: ERROR.to_owned(),
                locale_key: Some(locale_key.to_owned()),
                args: Some(serde_json::Value::String(item_tpl.to_owned())),
                message: None,
            });
        }

        // return default size of 1x1
        return (1, 1);
    };

    let Some(root_item) = items.iter().find(|item| item.id == item_id) else {
        diagnostics.push(Diagnostic {
            level: ERROR.to_owned(),
            locale_key: None,
            args: None,
            message: Some(format!(
                "Unable to get root item with Id: {item_id} from player inventory. Defaulting to 1x1"
            )),
        });

        return (1, 1);
    };

    // Does root item support being folded
    let root_can_be_folded = item_template.foldable.unwrap_or(false);

    // The slot that can be folded on root e.g. "mod_stock"
    let folded_slot = item_template.folded_slot.as_deref();

    let (mut size_up, mut size_down, mut size_left, mut size_right) = (0, 0, 0, 0);
    let (mut forced_up, mut forced_down, mut forced_left, mut forced_right) = (0, 0, 0, 0);
    let mut out_x = item_template.width.unwrap_or(0);
    let out_y = item_template.height.unwrap_or(0);

    // Is the root item actively folded
    let root_is_folded = is_folded(root_item);

    // Root can be collapsed and has been collapsed
    if root_can_be_folded && folded_slot.unwrap_or_default().is_empty() && root_is_folded {
        out_x -= item_template.size_reduce_right.unwrap_or(0);
    }

    // Item can have child items that adjust its size
    if crate::loot::item_helper::is_of_baseclasses(
        items_view,
        item_tpl,
        &[crate::loot::item_helper::WEAPON, FUNCTIONAL_MOD, MOD],
    ) {
        let mut to_do = vec![item_id.to_owned()];
        while let Some(parent_id) = to_do.pop() {
            for child_item in items
                .iter()
                .filter(|item| item.parent_id.as_deref() == Some(parent_id.as_str()))
            {
                // Skip mods that don't increase size. e.g. cartridges
                let Some(slot_id) = child_item.slot_id.as_deref() else {
                    continue;
                };
                if !slot_id.to_ascii_lowercase().starts_with("mod_") {
                    continue;
                }

                // Add child to processing queue to be checked for sub-children later
                to_do.push(child_item.id.clone());

                let Some(template) = get_item(items_view, &child_item.template) else {
                    diagnostics.push(Diagnostic {
                        level: ERROR.to_owned(),
                        locale_key: Some(
                            "inventory-get_item_size_item_not_found_by_tpl".to_owned(),
                        ),
                        args: Some(serde_json::Value::String(child_item.template.clone())),
                        message: None,
                    });

                    continue;
                };

                let child_can_be_folded = template.foldable.unwrap_or(false);
                let child_is_folded = is_folded(child_item);

                if root_can_be_folded
                    && folded_slot == Some(slot_id)
                    && (root_is_folded || child_is_folded)
                {
                    continue;
                }

                // Child mod can and is folded, don't include it in size calc
                if child_can_be_folded && root_is_folded && child_is_folded {
                    continue;
                }

                // Calculating child ExtraSize
                if template.extra_size_force_add.unwrap_or(false) {
                    forced_up += template.extra_size_up.unwrap_or(0);
                    forced_down += template.extra_size_down.unwrap_or(0);
                    forced_left += template.extra_size_left.unwrap_or(0);
                    forced_right += template.extra_size_right.unwrap_or(0);
                } else {
                    size_up = size_up.max(template.extra_size_up.unwrap_or(0));
                    size_down = size_down.max(template.extra_size_down.unwrap_or(0));
                    size_left = size_left.max(template.extra_size_left.unwrap_or(0));
                    size_right = size_right.max(template.extra_size_right.unwrap_or(0));
                }
            }
        }
    }

    (
        out_x + size_left + size_right + forced_left + forced_right,
        out_y + size_up + size_down + forced_up + forced_down,
    )
}

/// `item.Upd?.Foldable?.Folded.GetValueOrDefault(false) ?? false`.
fn is_folded(item: &Item) -> bool {
    item.upd
        .as_ref()
        .and_then(|upd| upd.foldable.as_ref())
        .and_then(|foldable| foldable.folded)
        .unwrap_or(false)
}

fn debug_diagnostic(message: String) -> Diagnostic {
    Diagnostic {
        level: crate::loot::models::DEBUG.to_owned(),
        locale_key: None,
        args: None,
        message: Some(message),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::bot::durability_limits_helper::BotDurability;
    use crate::bot::models::RandomisedResourceDetails;
    use crate::loot::random_util::{TestSeedGuard, get_int};

    const SEED: u64 = 42;

    /// Owned fixtures a [`BotContext`] can borrow from.
    struct Fixture {
        items: IndexMap<String, ItemView>,
        bosses: Vec<String>,
        durability: BotDurability,
        equipment: IndexMap<String, EquipmentFilters>,
        randomization: IndexMap<String, RandomisedResourceDetails>,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                items: serde_json::from_value(json!({
                    // Weapon: MaxDurability + WeapClass -> the two-draw arm.
                    "w1": {"weapClass": "assaultRifle", "maxDurability": 100.0},
                    // Armor: MaxDurability + ArmorClass -> the armor arm.
                    "a1": {"armorClass": 4, "maxDurability": 50.0},
                    // ArmorClass 0 -> template value, no draws.
                    "a0": {"armorClass": 0, "maxDurability": 40.0},
                    "med1": {"maxHpResource": 100},
                    "food1": {"maxResource": 100, "foodUseTime": 5.0},
                    "esmarch": {"maxHpResource": 0},
                    "light1": {"parent": FLASHLIGHT},
                    "laser1": {"parent": TACTICAL_COMBO},
                    "nvg1": {"parent": NIGHT_VISION},
                    "shield1": {"hasHinge": true, "faceShieldComponent": true},
                    "emptyfire": {"weapFireType": []},
                    // Compatibility fixtures.
                    "headset": {"name": "headset", "blocksEarpiece": true},
                    "helmet": {"name": "helmet", "blocksHeadwear": true},
                    "conflicter": {"name": "conflicter", "conflictingItems": ["victim"]},
                    "victim": {"name": "victim"},
                    "plain": {"name": "plain"},
                    // Container fixtures.
                    "cont2x2": {"grids": [{"name": "main", "cellsH": 2, "cellsV": 2}]},
                    "cont1x1": {"grids": [{"name": "solo", "cellsH": 1, "cellsV": 1}]},
                    "contfiltered": {"grids": [{"name": "picky", "cellsH": 2, "cellsV": 2,
                        "filters": [{"filter": ["allowedparent"]}]}]},
                    "i1x1": {"parent": "allowedparent", "width": 1, "height": 1},
                    "i2x1": {"parent": "otherparent", "width": 2, "height": 1},
                }))
                .unwrap(),
                bosses: vec!["bossBully".to_owned()],
                durability: serde_json::from_value(json!({
                    "default": {
                        "armor": {"maxDelta": 10, "minDelta": 0, "minLimitPercent": 15},
                        "weapon": {"lowestMax": 60, "highestMax": 100, "maxDelta": 10,
                            "minDelta": 0, "minLimitPercent": 15.0}
                    },
                    "botDurabilities": {},
                    "pmc": {
                        "armor": {"lowestMaxPercent": 90, "highestMaxPercent": 100, "maxDelta": 10,
                            "minDelta": 0, "minLimitPercent": 15},
                        "weapon": {"lowestMax": 95, "highestMax": 100, "maxDelta": 5,
                            "minDelta": 0, "minLimitPercent": 15.0}
                    }
                }))
                .unwrap(),
                equipment: serde_json::from_value(json!({
                    "assault": {
                        "faceShieldIsActiveChancePercent": 100.0,
                        "lightIsActiveDayChancePercent": 0.0,
                        "lightIsActiveNightChancePercent": 100.0,
                        "laserIsActiveChancePercent": 100.0,
                        "nvgIsActiveChanceDayPercent": 0.0,
                        "nvgIsActiveChanceNightPercent": 100.0
                    },
                    "pmc": {}
                }))
                .unwrap(),
                randomization: serde_json::from_value(json!({
                    "assault": {"food": {"resourcePercent": 30.0, "chanceMaxResourcePercent": 0.0},
                                "meds": {"resourcePercent": 60.0, "chanceMaxResourcePercent": 0.0}}
                }))
                .unwrap(),
            }
        }

        fn ctx(&self, is_night_time: bool) -> BotContext<'_> {
            BotContext {
                items: &self.items,
                bosses: &self.bosses,
                durability: &self.durability,
                equipment: &self.equipment,
                loot_item_resource_randomization: &self.randomization,
                item_blacklist: &crate::bot::NO_BLACKLIST,
                default_presets_by_tpl: &crate::bot::NO_DEFAULT_PRESETS,
                equipment_blacklist: &crate::bot::NO_EQUIP_BLACKLIST,
                weapon_mod_equipment_blacklist: &crate::bot::NO_EQUIP_BLACKLIST,
                low_profile_gas_block_tpls: &crate::bot::NO_BLACKLIST,
                item_presets: &crate::bot::NO_PRESETS,
                weapon_has_enhancement_chance_percent: 0.0,
                repair_kit_weapon: &crate::bot::NO_BUFFS,
                secure_container_ammo_stack_count: 0,
                mod_pool_slot_order: &crate::bot::NO_MOD_POOL_ORDER,
                is_night_time,
                diagnostics: Vec::new(),
            }
        }
    }

    fn template<'a>(fixture: &'a Fixture, tpl: &str) -> &'a ItemView {
        &fixture.items[tpl]
    }

    fn upd(result: Result<Option<Upd>, LootError>) -> serde_json::Value {
        serde_json::to_value(result.unwrap().unwrap()).unwrap()
    }

    /// Where the shared RNG stream sits after `consume` has run against `SEED`. Two runs agree only
    /// if they drew the same number of values *of the same kinds*, so this pins draw order.
    fn stream_position_after(consume: impl FnOnce()) -> f64 {
        let _guard = TestSeedGuard::install(SEED);
        consume();

        get_double(0.0, 1.0)
    }

    fn inventory_item(id: &str, tpl: &str, slot_id: Option<&str>) -> Item {
        serde_json::from_value(json!({"_id": id, "_tpl": tpl, "slotId": slot_id})).unwrap()
    }

    // -----------------------------------------------------------------------
    // generate_extra_properties_for_item
    // -----------------------------------------------------------------------

    #[test]
    fn weapon_gets_a_repairable_from_two_durability_draws() {
        let fixture = Fixture::new();
        let ctx = fixture.ctx(false);
        let _guard = TestSeedGuard::install(SEED);

        let generated = upd(generate_extra_properties_for_item(
            &ctx,
            template(&fixture, "w1"),
            Some("assault"),
            false,
        ));

        // The default weapon slice: GetInt(60, 100) for the max, then GetInt(0, 10) for the delta.
        assert_eq!(
            generated,
            json!({"Repairable": {"Durability": 81.0, "MaxDurability": 82.0}})
        );
    }

    #[test]
    fn weapon_durability_draws_exactly_the_two_get_ints() {
        let fixture = Fixture::new();
        let ctx = fixture.ctx(false);

        let after_generate = stream_position_after(|| {
            generate_extra_properties_for_item(
                &ctx,
                template(&fixture, "w1"),
                Some("assault"),
                false,
            )
            .unwrap();
        });
        let after_manual = stream_position_after(|| {
            get_int(60, 100);
            get_int(0, 10);
        });

        assert_eq!(after_generate, after_manual);
    }

    #[test]
    fn armor_takes_one_draw_for_a_non_pmc_and_two_for_a_pmc() {
        let fixture = Fixture::new();
        let ctx = fixture.ctx(false);

        // Non-PMC: GetRandomizedMaxArmorDurability passes the template value through untouched.
        let after_scav = stream_position_after(|| {
            generate_extra_properties_for_item(
                &ctx,
                template(&fixture, "a1"),
                Some("assault"),
                false,
            )
            .unwrap();
        });
        assert_eq!(
            after_scav,
            stream_position_after(|| {
                get_int(0, 10);
            })
        );

        // PMC: a GetDouble for the max percent, then the GetInt delta.
        let after_pmc = stream_position_after(|| {
            generate_extra_properties_for_item(
                &ctx,
                template(&fixture, "a1"),
                Some("pmcUSEC"),
                false,
            )
            .unwrap();
        });
        assert_eq!(
            after_pmc,
            stream_position_after(|| {
                get_double(90.0, 100.0);
                get_int(0, 10);
            })
        );
    }

    #[test]
    fn armor_class_zero_short_circuits_to_the_template_value_without_drawing() {
        let fixture = Fixture::new();
        let ctx = fixture.ctx(false);
        let _guard = TestSeedGuard::install(SEED);

        let generated = upd(generate_extra_properties_for_item(
            &ctx,
            template(&fixture, "a0"),
            Some("assault"),
            false,
        ));

        assert_eq!(
            generated,
            json!({"Repairable": {"Durability": 40.0, "MaxDurability": 40.0}})
        );
        assert_eq!(
            stream_position_after(|| {
                generate_extra_properties_for_item(
                    &ctx,
                    template(&fixture, "a0"),
                    Some("assault"),
                    false,
                )
                .unwrap();
            }),
            stream_position_after(|| {})
        );
    }

    #[test]
    fn a_role_without_resource_randomisation_takes_the_max_without_drawing() {
        let fixture = Fixture::new();
        let ctx = fixture.ctx(false);
        let _guard = TestSeedGuard::install(SEED);

        // `bossBully` has no LootItemResourceRandomization entry, so the `||` short-circuits.
        let generated = upd(generate_extra_properties_for_item(
            &ctx,
            template(&fixture, "med1"),
            Some("bossBully"),
            false,
        ));

        assert_eq!(generated, json!({"MedKit": {"HpResource": 100.0}}));
        assert_eq!(
            stream_position_after(|| {
                generate_extra_properties_for_item(
                    &ctx,
                    template(&fixture, "med1"),
                    Some("bossBully"),
                    false,
                )
                .unwrap();
            }),
            stream_position_after(|| {})
        );
    }

    #[test]
    fn a_configured_role_randomises_med_and_food_resources() {
        let fixture = Fixture::new();
        let ctx = fixture.ctx(false);

        let _guard = TestSeedGuard::install(SEED);
        let meds = upd(generate_extra_properties_for_item(
            &ctx,
            template(&fixture, "med1"),
            Some("assault"),
            false,
        ));
        drop(_guard);

        let _guard = TestSeedGuard::install(SEED);
        let food = upd(generate_extra_properties_for_item(
            &ctx,
            template(&fixture, "food1"),
            Some("assault"),
            false,
        ));
        drop(_guard);

        // chanceMaxResourcePercent 0 never wins the GetChance100, so both take the GetDouble arm
        // between `max(1, GetPercentOfValue(percent, 100, 0))` and 100.
        assert_eq!(meds, json!({"MedKit": {"HpResource": 93.98829714607899}}));
        assert_eq!(food, json!({"FoodDrink": {"HpPercent": 89.47952000563824}}));

        // Two draws each: the chance roll, then the value.
        assert_eq!(
            stream_position_after(|| {
                generate_extra_properties_for_item(
                    &ctx,
                    template(&fixture, "med1"),
                    Some("assault"),
                    false,
                )
                .unwrap();
            }),
            stream_position_after(|| {
                get_chance_100(0.0);
                get_double(60.0, 100.0);
            })
        );
    }

    #[test]
    fn a_zero_max_hp_resource_item_gets_no_med_kit() {
        let fixture = Fixture::new();
        let ctx = fixture.ctx(false);

        // Esmarch tourniquet: MaxHpResource is 0, so the block is skipped and nothing else applies.
        assert!(
            generate_extra_properties_for_item(
                &ctx,
                template(&fixture, "esmarch"),
                Some("assault"),
                false
            )
            .unwrap()
            .is_none()
        );
    }

    #[test]
    fn light_and_nvg_chances_switch_on_the_night_flag() {
        let fixture = Fixture::new();

        for (is_night, expected_light, expected_nvg) in [(false, false, false), (true, true, true)]
        {
            let ctx = fixture.ctx(is_night);
            let _guard = TestSeedGuard::install(SEED);

            let light = upd(generate_extra_properties_for_item(
                &ctx,
                template(&fixture, "light1"),
                Some("assault"),
                false,
            ));
            let nvg = upd(generate_extra_properties_for_item(
                &ctx,
                template(&fixture, "nvg1"),
                Some("assault"),
                false,
            ));

            // Day chances are pinned to 0% and night chances to 100% in the fixture, so the flag
            // decides the outcome outright rather than the draw.
            assert_eq!(
                light,
                json!({"Light": {"IsActive": expected_light, "SelectedMode": 0}})
            );
            assert_eq!(nvg, json!({"Togglable": {"On": expected_nvg}}));
        }
    }

    #[test]
    fn a_laser_uses_the_single_laser_chance_regardless_of_time_of_day() {
        let fixture = Fixture::new();

        for is_night in [false, true] {
            let ctx = fixture.ctx(is_night);
            let _guard = TestSeedGuard::install(SEED);

            let laser = upd(generate_extra_properties_for_item(
                &ctx,
                template(&fixture, "laser1"),
                Some("assault"),
                false,
            ));

            assert_eq!(
                laser,
                json!({"Light": {"IsActive": true, "SelectedMode": 0}})
            );
        }
    }

    #[test]
    fn an_unmapped_role_falls_back_to_the_literal_chance_defaults() {
        let fixture = Fixture::new();
        let ctx = fixture.ctx(false);
        let _guard = TestSeedGuard::install(SEED);

        // No `madeUpRole` entry in BotConfig.Equipment, so the flashlight day default of 25%
        // runs instead of the fixture's configured 0%, and the same first draw now passes.
        let light = upd(generate_extra_properties_for_item(
            &ctx,
            template(&fixture, "light1"),
            Some("madeUpRole"),
            false,
        ));
        assert_eq!(
            light,
            json!({"Light": {"IsActive": true, "SelectedMode": 0}})
        );

        drop(_guard);
        let _guard = TestSeedGuard::install(SEED);
        let configured = upd(generate_extra_properties_for_item(
            &ctx,
            template(&fixture, "light1"),
            Some("assault"),
            false,
        ));
        assert_eq!(
            configured,
            json!({"Light": {"IsActive": false, "SelectedMode": 0}})
        );
    }

    #[test]
    fn a_face_shield_overwrites_the_hinge_togglable() {
        let fixture = Fixture::new();
        let ctx = fixture.ctx(false);
        let _guard = TestSeedGuard::install(SEED);

        // HasHinge sets `On: true` at :87, then the FaceShieldComponent block at :160 replaces it
        // with the rolled value - 100% here, so still true, but from a draw rather than the literal.
        let generated = upd(generate_extra_properties_for_item(
            &ctx,
            template(&fixture, "shield1"),
            Some("assault"),
            false,
        ));

        assert_eq!(generated, json!({"Togglable": {"On": true}}));
        assert_eq!(
            stream_position_after(|| {
                generate_extra_properties_for_item(
                    &ctx,
                    template(&fixture, "shield1"),
                    Some("assault"),
                    false,
                )
                .unwrap();
            }),
            stream_position_after(|| {
                get_chance_100(100.0);
            })
        );
    }

    #[test]
    fn an_empty_weap_fire_type_is_the_c_sharp_throw() {
        let fixture = Fixture::new();
        let ctx = fixture.ctx(false);

        let error = generate_extra_properties_for_item(
            &ctx,
            template(&fixture, "emptyfire"),
            Some("assault"),
            false,
        )
        .unwrap_err();

        assert_eq!(error.message, "Sequence contains no elements.");
    }

    #[test]
    fn force_stack_objects_count_is_the_only_thing_that_makes_a_propless_upd() {
        let fixture = Fixture::new();
        let ctx = fixture.ctx(false);

        assert!(
            generate_extra_properties_for_item(
                &ctx,
                template(&fixture, "plain"),
                Some("assault"),
                false
            )
            .unwrap()
            .is_none()
        );
        assert_eq!(
            upd(generate_extra_properties_for_item(
                &ctx,
                template(&fixture, "plain"),
                Some("assault"),
                true
            )),
            json!({"StackObjectsCount": 1.0})
        );
    }

    #[test]
    fn bot_equipment_role_collapses_only_the_two_pmc_sides() {
        assert_eq!(get_bot_equipment_role("pmcBEAR"), "pmc");
        assert_eq!(get_bot_equipment_role("pmcUSEC"), "pmc");
        // `BotHelper._pmcTypeIds` also carries bare usec/bear; `_pmcTypes` here does not.
        assert_eq!(get_bot_equipment_role("usec"), "usec");
        assert_eq!(get_bot_equipment_role("assault"), "assault");
    }

    // -----------------------------------------------------------------------
    // is_item_incompatible_with_current_items
    // -----------------------------------------------------------------------

    #[test]
    fn slots_with_no_compat_issues_short_circuit() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx(false);

        let result =
            is_item_incompatible_with_current_items(&mut ctx, &[], "victim", "SecuredContainer");

        assert_eq!(
            result,
            ChooseRandomCompatibleModResult {
                incompatible: Some(false),
                found: Some(false),
                reason: Some(String::new()),
                ..Default::default()
            }
        );
    }

    #[test]
    fn an_equipped_conflicting_item_blocks_the_incoming_one() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx(false);
        let equipped = vec![inventory_item("e1", "conflicter", Some("Eyewear"))];

        let result =
            is_item_incompatible_with_current_items(&mut ctx, &equipped, "victim", "Headwear");

        assert_eq!(
            result,
            ChooseRandomCompatibleModResult {
                incompatible: Some(true),
                found: Some(false),
                reason: Some(
                    "victim victim in slot: Headwear blocked by: conflicter conflicter".to_owned()
                ),
                slot_blocked: Some(true),
                ..Default::default()
            }
        );
    }

    #[test]
    fn an_incoming_item_that_conflicts_with_an_equipped_one_reports_the_other_message() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx(false);
        let equipped = vec![inventory_item("e1", "victim", Some("Eyewear"))];

        let result =
            is_item_incompatible_with_current_items(&mut ctx, &equipped, "conflicter", "Headwear");

        // The last check in the method: no `found`, no `slotBlocked`, and a different wording.
        assert_eq!(
            result,
            ChooseRandomCompatibleModResult {
                incompatible: Some(true),
                reason: Some("conflicter blocks existing item victim in slot Eyewear".to_owned()),
                ..Default::default()
            }
        );
    }

    #[test]
    fn a_blocks_earpiece_headset_blocks_the_earpiece_slot() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx(false);
        let equipped = vec![inventory_item("e1", "headset", Some("Headwear"))];

        // `TemplateItem.Blocks["Earpiece"]` is true on the equipped item, so nothing may go into
        // the Earpiece slot.
        let result =
            is_item_incompatible_with_current_items(&mut ctx, &equipped, "plain", "Earpiece");

        assert_eq!(result.incompatible, Some(true));
        assert_eq!(result.slot_blocked, Some(true));
        assert_eq!(
            result.reason.as_deref(),
            Some("plain plain in slot: Earpiece blocked by: headset headset")
        );

        // ...but the Headwear slot is untouched: the Blocks dictionary is per-slot.
        assert_eq!(
            is_item_incompatible_with_current_items(&mut ctx, &equipped, "plain", "Eyewear")
                .incompatible,
            Some(false)
        );
    }

    #[test]
    fn armor_vest_is_absent_from_the_blocks_dictionary_quirk() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx(false);
        // `helmet` sets BlocksHeadwear; `TemplateItem.Blocks` has a "Headwear" key but no
        // "ArmorVest" one, so probing that slot name can never match.
        let equipped = vec![inventory_item("e1", "helmet", Some("Eyewear"))];

        assert_eq!(
            is_item_incompatible_with_current_items(&mut ctx, &equipped, "plain", "Headwear")
                .incompatible,
            Some(true)
        );
        assert_eq!(
            is_item_incompatible_with_current_items(&mut ctx, &equipped, "plain", "ArmorVest")
                .incompatible,
            Some(false)
        );
    }

    #[test]
    fn an_incoming_blocker_is_stopped_by_the_slot_it_blocks() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx(false);
        let equipped = vec![inventory_item("e1", "plain", Some("Earpiece"))];

        // `headset` has BlocksEarpiece and something already occupies the Earpiece slot.
        let result =
            is_item_incompatible_with_current_items(&mut ctx, &equipped, "headset", "Headwear");

        assert_eq!(
            result,
            ChooseRandomCompatibleModResult {
                incompatible: Some(true),
                found: Some(false),
                reason: Some("headset headset is blocked by: plain in slot: Earpiece".to_owned()),
                slot_blocked: Some(true),
                ..Default::default()
            }
        );
    }

    #[test]
    fn a_compatible_pair_reports_no_incompatibility_and_no_flags() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx(false);
        let equipped = vec![inventory_item("e1", "plain", Some("Eyewear"))];

        assert_eq!(
            is_item_incompatible_with_current_items(&mut ctx, &equipped, "victim", "Headwear"),
            ChooseRandomCompatibleModResult {
                incompatible: Some(false),
                reason: Some(String::new()),
                ..Default::default()
            }
        );
        assert!(ctx.diagnostics.is_empty());
    }

    #[test]
    fn an_unknown_tpl_is_incompatible_and_warns() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx(false);

        let result = is_item_incompatible_with_current_items(&mut ctx, &[], "nope", "Headwear");

        assert_eq!(result.incompatible, Some(true));
        assert_eq!(
            result.reason.as_deref(),
            Some("item: nope does not exist in the database")
        );
        assert_eq!(ctx.diagnostics.len(), 1);
        assert_eq!(
            ctx.diagnostics[0].locale_key.as_deref(),
            Some("bot-invalid_item_compatibility_check")
        );
    }

    // -----------------------------------------------------------------------
    // Container grids
    // -----------------------------------------------------------------------

    /// A bot inventory holding one container per requested `(slot, tpl)`, plus the grids that
    /// track them.
    fn containers(ctx: &BotContext, slots: &[(&str, &str)]) -> (ContainerGrids, Vec<Item>) {
        let mut grids = ContainerGrids::default();
        let mut inventory = Vec::new();

        for (slot, tpl) in slots {
            let container = inventory_item(&format!("{slot}-id"), tpl, Some(slot));
            grids.add_empty_container(ctx, slot, &container);
            inventory.push(container);
        }

        (grids, inventory)
    }

    fn add(
        grids: &mut ContainerGrids,
        ctx: &mut BotContext,
        slots: &[String],
        id: &str,
        tpl: &str,
        inventory: &mut Vec<Item>,
    ) -> (ItemAddedResult, Item) {
        let mut item = vec![inventory_item(id, tpl, None)];
        let result = grids
            .add_item_with_children_to_equipment_slot(ctx, slots, id, tpl, &mut item, inventory);

        (result, item.remove(0))
    }

    #[test]
    fn an_empty_container_is_sized_rows_by_cells_v_and_columns_by_cells_h() {
        let fixture = Fixture::new();
        let ctx = fixture.ctx(false);
        let container = inventory_item("c1", "cont2x2", Some("TacticalVest"));

        let mut grids = ContainerGrids::default();
        grids.add_empty_container(&ctx, "TacticalVest", &container);
        // Second call is a no-op, matching `if (!containers.ContainsKey(containerName))`.
        grids.add_empty_container(&ctx, "TacticalVest", &container);

        let details = grids.get("TacticalVest").unwrap();
        assert_eq!(details.container_tpl, "cont2x2");
        assert_eq!(details.container_item_id, "c1");
        assert_eq!(details.grids.len(), 1);
        assert_eq!(details.grids[0].grid_map, vec![vec![0, 0], vec![0, 0]]);
        assert!(!details.grids[0].grid_full);
    }

    #[test]
    fn a_one_by_one_then_a_two_by_one_pack_row_major_with_rotation() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx(false);
        let (mut grids, mut inventory) = containers(&ctx, &[("TacticalVest", "cont2x2")]);
        let slots = vec!["TacticalVest".to_owned()];

        let (result, placed) = add(&mut grids, &mut ctx, &slots, "i1", "i1x1", &mut inventory);
        assert_eq!(result, ItemAddedResult::Success);
        // Parented to the container item, slotted by the *grid* name, not the container name.
        assert_eq!(placed.parent_id.as_deref(), Some("TacticalVest-id"));
        assert_eq!(placed.slot_id.as_deref(), Some("main"));
        assert_eq!(
            placed.location,
            Some(json!({"x": 0, "y": 0, "r": "Horizontal"}))
        );
        assert_eq!(
            grids.get("TacticalVest").unwrap().grids[0].grid_map,
            vec![vec![1, 0], vec![0, 0]]
        );

        // 2 wide x 1 high cannot start at (0, 0) or fit unrotated at (0, 1); rotated it stands in
        // column 1 across both rows.
        let (result, placed) = add(&mut grids, &mut ctx, &slots, "i2", "i2x1", &mut inventory);
        assert_eq!(result, ItemAddedResult::Success);
        assert_eq!(
            placed.location,
            Some(json!({"x": 1, "y": 0, "r": "Vertical"}))
        );
        assert_eq!(
            grids.get("TacticalVest").unwrap().grids[0].grid_map,
            vec![vec![1, 1], vec![0, 1]]
        );

        assert_eq!(inventory.len(), 3);
    }

    #[test]
    fn a_full_container_reports_no_space_and_latches_the_grid_full_flag() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx(false);
        let (mut grids, mut inventory) = containers(&ctx, &[("TacticalVest", "cont1x1")]);
        let slots = vec!["TacticalVest".to_owned()];

        assert_eq!(
            add(&mut grids, &mut ctx, &slots, "i1", "i1x1", &mut inventory).0,
            ItemAddedResult::Success
        );
        assert_eq!(
            add(&mut grids, &mut ctx, &slots, "i2", "i1x1", &mut inventory).0,
            ItemAddedResult::NoSpace
        );
        // A 1x1 that failed to fit flags the grid full outright.
        assert!(grids.get("TacticalVest").unwrap().grids[0].grid_full);
        assert_eq!(inventory.len(), 2);
    }

    #[test]
    fn slot_iteration_order_decides_which_container_wins() {
        let fixture = Fixture::new();

        for (order, expected_parent) in [
            (["Backpack", "TacticalVest"], "Backpack-id"),
            (["TacticalVest", "Backpack"], "TacticalVest-id"),
        ] {
            let mut ctx = fixture.ctx(false);
            let (mut grids, mut inventory) = containers(
                &ctx,
                &[("TacticalVest", "cont2x2"), ("Backpack", "cont2x2")],
            );
            let slots: Vec<String> = order.iter().map(|slot| (*slot).to_owned()).collect();

            let (result, placed) = add(&mut grids, &mut ctx, &slots, "i1", "i1x1", &mut inventory);

            assert_eq!(result, ItemAddedResult::Success);
            assert_eq!(placed.parent_id.as_deref(), Some(expected_parent));
        }
    }

    #[test]
    fn a_missing_container_for_every_slot_is_no_containers() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx(false);
        let (mut grids, mut inventory) = containers(&ctx, &[]);
        let slots = vec!["TacticalVest".to_owned(), "Backpack".to_owned()];

        assert_eq!(
            add(&mut grids, &mut ctx, &slots, "i1", "i1x1", &mut inventory).0,
            ItemAddedResult::NoContainers
        );
        assert_eq!(ctx.diagnostics.len(), 1);
        assert!(
            ctx.diagnostics[0]
                .message
                .as_deref()
                .unwrap()
                .contains("TacticalVest,Backpack")
        );
    }

    #[test]
    fn a_grid_filter_that_excludes_the_items_parent_rejects_it() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx(false);
        let (mut grids, mut inventory) = containers(&ctx, &[("TacticalVest", "contfiltered")]);
        let slots = vec!["TacticalVest".to_owned()];

        // `i1x1`'s parent is in the grid's Filter, `i2x1`'s is not.
        assert_eq!(
            add(&mut grids, &mut ctx, &slots, "i1", "i1x1", &mut inventory).0,
            ItemAddedResult::Success
        );
        // INCOMPATIBLE_ITEM from TryAdd makes AddItemWithChildren move on and fall out as NO_SPACE.
        assert_eq!(
            add(&mut grids, &mut ctx, &slots, "i2", "i2x1", &mut inventory).0,
            ItemAddedResult::NoSpace
        );
    }

    #[test]
    fn an_unresolvable_item_size_falls_back_to_one_by_one_and_logs_the_c_sharp_errors() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx(false);
        let (mut grids, mut inventory) = containers(&ctx, &[("TacticalVest", "cont2x2")]);
        let slots = vec!["TacticalVest".to_owned()];

        // Tpl missing from the view: `GetSizeByInventoryItemHash` logs `:625` *and* `:639`, both
        // at Error, then returns 1x1 - so the item still lands, in one cell.
        let (result, placed) = add(&mut grids, &mut ctx, &slots, "i1", "ghost", &mut inventory);

        assert_eq!(result, ItemAddedResult::Success);
        assert_eq!(
            placed.location,
            Some(json!({"x": 0, "y": 0, "r": "Horizontal"}))
        );
        assert_eq!(
            ctx.diagnostics
                .iter()
                .map(|diagnostic| (diagnostic.level.as_str(), diagnostic.locale_key.as_deref()))
                .collect::<Vec<_>>(),
            vec![
                (ERROR, Some("inventory-invalid_item_missing_from_db")),
                (ERROR, Some("inventory-return_default_size")),
            ]
        );

        // Root id absent from the item list is the other 1x1 fallback (`:644`), a plain message.
        ctx.diagnostics.clear();
        let mut orphan = vec![inventory_item("i2", "i2x1", None)];
        grids.add_item_with_children_to_equipment_slot(
            &mut ctx,
            &slots,
            "not-in-the-list",
            "i2x1",
            &mut orphan,
            &mut inventory,
        );

        assert_eq!(ctx.diagnostics.len(), 1);
        assert_eq!(ctx.diagnostics[0].level, ERROR);
        assert!(
            ctx.diagnostics[0]
                .message
                .as_deref()
                .unwrap()
                .starts_with("Unable to get root item with Id: not-in-the-list")
        );
    }

    #[test]
    fn container_grids_serialize_to_the_wire_shape_task_14_rebuilds_from() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx(false);
        let (mut grids, mut inventory) = containers(&ctx, &[("TacticalVest", "cont1x1")]);
        let slots = vec!["TacticalVest".to_owned()];
        add(&mut grids, &mut ctx, &slots, "i1", "i1x1", &mut inventory);

        let wire = serde_json::to_value(grids.into_wire()).unwrap();

        assert_eq!(
            wire,
            json!({"TacticalVest": {
                "containerTpl": "cont1x1",
                "containerItemId": "TacticalVest-id",
                "grids": [{"gridMap": [[1]], "gridFull": false}]
            }})
        );
    }
}
