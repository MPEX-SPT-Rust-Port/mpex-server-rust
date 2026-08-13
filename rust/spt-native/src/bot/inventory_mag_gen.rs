//! `Generators/Weapons/` — the `IInventoryMagGen` strategy family that puts a bot's spare
//! magazines (or loose rounds, or grenades) into its vest and pockets.
//!
//! The C# is four `[Injectable]` classes behind an interface, resolved as an `IEnumerable` and
//! sorted once by `GetPriority()` in `BotWeaponGenerator.MagGenSetUp` (`:47-52`). There is no
//! runtime extensibility to preserve here — the DI container hands over exactly these four — so
//! they are one enum ([`MagGenKind`]) plus a `can_handle`/`process` pair, and the sorted list is
//! the [`MAG_GEN_ORDER`] constant. Dispatch is [`process_mag_gen`], which takes the order as a
//! parameter so a caller (and the tests) can pin it.
//!
//! `InventoryMagGen`, the C# parameter object, is [`InventoryMagGen`] here with its four
//! `TemplateItem`s replaced by tpls: a flattened [`crate::loot::models::ItemView`] row carries no
//! id of its own, and every consumer either looks the row up or wants the tpl. `BotBaseInventory`
//! and the bot id ride as the `inventory`/`grids` parameters the rest of the bot port already uses.
//!
//! # RNG calls, in C# source order — the parity contract
//!
//! Dispatch itself draws nothing: `CanHandleInventoryMagGen` is a pure read on all four. Then, for
//! the one implementation that handles the magazine:
//!
//! - [`MagGenKind::InternalMagazine`] (`InternalMagazineInventoryMagGen.cs:21-34`) and
//!   [`MagGenKind::Ubgl`] (`UbglExternalMagGen.cs:21-34`) — one `GetRandomizedBulletCount`, i.e.
//!   one `GetWeightedValue` over the magazine weights. `AddAmmoIntoEquipmentSlots` draws nothing.
//! - [`MagGenKind::Barrel`] (`BarrelInventoryMagGen.cs:24-48`) — one `GetInt`: `(3, 6)` when the
//!   ammo does not stack (`:31`), else `(StackMinRandom, StackMaxRandom)` (`:35`).
//! - [`MagGenKind::External`] (`ExternalInventoryMagGen.cs:37-164`) — one `GetRandomizedMagazineCount`
//!   (`:50`), then per loop pass one `GetInt` inside `CreateMagazineWithAmmo`'s
//!   `FillMagazineWithCartridge`, plus one `GetArrayValue` (`:200`) each time the internal-magazine
//!   fallback has to pick a replacement magazine. A pass that fails to fit repeats without
//!   advancing the counter, so it draws again.
use indexmap::IndexMap;

use crate::bot::BotContext;
use crate::bot::bot_generator_helper::{ContainerGrids, ItemAddedResult};
use crate::bot::bot_weapon_generator_helper::{
    VEST_AND_POCKETS, add_ammo_into_equipment_slots, create_magazine_with_ammo,
    get_randomized_bullet_count, get_randomized_magazine_count, item_added_result_name,
};
use crate::loot::item_helper::{LAUNCHER, LootError, MAGAZINE, SHOTGUN, get_item, is_of_baseclass};
use crate::loot::models::{DEBUG, Diagnostic, ERROR, Item};
use crate::loot::random_util::{get_array_value, get_int};

/// `ReloadMode.InternalMagazine`.
const INTERNAL_MAGAZINE: &str = "InternalMagazine";
/// `ReloadMode.OnlyBarrel`.
const ONLY_BARREL: &str = "OnlyBarrel";

/// `Generators/Weapons/InventoryMagGen.cs` — the parameter object every implementation reads.
#[derive(Debug, Clone, Copy)]
pub struct InventoryMagGen<'a> {
    /// `GenerationData.Weights` — the whole `GenerationData` in C#, but only `Weights` is read.
    pub mag_counts: &'a IndexMap<String, f64>,
    pub magazine_tpl: &'a str,
    pub weapon_tpl: &'a str,
    pub ammo_tpl: &'a str,
}

/// The four `IInventoryMagGen` implementations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MagGenKind {
    /// `InternalMagazineInventoryMagGen`, priority 0.
    InternalMagazine,
    /// `UbglExternalMagGen`, priority 1.
    Ubgl,
    /// `BarrelInventoryMagGen`, priority 50.
    Barrel,
    /// `ExternalInventoryMagGen`, priority 99 — the catch-all.
    External,
}

impl MagGenKind {
    /// `IInventoryMagGen.GetPriority()`, the key `MagGenSetUp` sorts on.
    #[allow(
        dead_code,
        reason = "nothing sorts at runtime — `MAG_GEN_ORDER` pins the sorted order; kept so the tests can check the ported priorities against it"
    )]
    pub fn priority(self) -> i32 {
        match self {
            Self::InternalMagazine => 0,
            Self::Ubgl => 1,
            Self::Barrel => 50,
            Self::External => 99,
        }
    }
}

/// The four implementations in `MagGenSetUp` order (`BotWeaponGenerator.cs:47-52`) — ascending
/// `GetPriority()`, which is the order `FirstOrDefault(CanHandle)` walks.
pub const MAG_GEN_ORDER: [MagGenKind; 4] = [
    MagGenKind::InternalMagazine,
    MagGenKind::Ubgl,
    MagGenKind::Barrel,
    MagGenKind::External,
];

/// `CanHandleInventoryMagGen` for each implementation.
///
/// Every one of them dereferences a resolved `TemplateItem`, so a tpl missing from the items view
/// is a C# `NullReferenceException`; here it simply fails to match and the next implementation is
/// offered the magazine. Both templates were resolved by the caller a few lines earlier, so the
/// case is unreachable in practice.
pub fn can_handle(ctx: &BotContext, kind: MagGenKind, mag_gen: &InventoryMagGen) -> bool {
    match kind {
        // `InternalMagazineInventoryMagGen.cs:16-19`
        MagGenKind::InternalMagazine => {
            get_item(ctx.items, mag_gen.magazine_tpl)
                .and_then(|magazine| magazine.reload_mag_type.as_deref())
                == Some(INTERNAL_MAGAZINE)
        }
        // `UbglExternalMagGen.cs:16-19`
        MagGenKind::Ubgl => {
            get_item(ctx.items, mag_gen.weapon_tpl).and_then(|weapon| weapon.parent.as_deref())
                == Some(LAUNCHER)
        }
        // `BarrelInventoryMagGen.cs:19-22`
        MagGenKind::Barrel => {
            get_item(ctx.items, mag_gen.weapon_tpl).and_then(|weapon| weapon.reload_mode.as_deref())
                == Some(ONLY_BARREL)
        }
        // `ExternalInventoryMagGen.cs:32-35` — the fallback
        MagGenKind::External => true,
    }
}

/// `InventoryMagGenComponents.FirstOrDefault(v => v.CanHandleInventoryMagGen(m)).Process(m)`
/// (`BotWeaponGenerator.cs:464`, `:506`).
///
/// # Errors
///
/// The unguarded `FirstOrDefault` in both call sites: with no handler the C# dereferences null.
/// [`MAG_GEN_ORDER`] always ends in the catch-all, so only a caller that passes a narrowed order
/// can reach it. Plus whatever the chosen implementation raises.
pub fn process_mag_gen(
    ctx: &mut BotContext,
    grids: &mut ContainerGrids,
    kinds: &[MagGenKind],
    mag_gen: &InventoryMagGen,
    inventory: &mut Vec<Item>,
) -> Result<(), LootError> {
    let Some(kind) = kinds
        .iter()
        .copied()
        .find(|kind| can_handle(ctx, *kind, mag_gen))
    else {
        return Err(LootError::new(format!(
            "No IInventoryMagGen can handle magazine: {} of weapon: {}",
            mag_gen.magazine_tpl, mag_gen.weapon_tpl
        )));
    };

    process(ctx, grids, kind, mag_gen, inventory)
}

/// `Process` for each implementation.
///
/// # Errors
///
/// See each `process_*` below.
pub fn process(
    ctx: &mut BotContext,
    grids: &mut ContainerGrids,
    kind: MagGenKind,
    mag_gen: &InventoryMagGen,
    inventory: &mut Vec<Item>,
) -> Result<(), LootError> {
    match kind {
        MagGenKind::InternalMagazine => {
            process_internal_magazine(ctx, grids, mag_gen, inventory, None)
        }
        MagGenKind::Ubgl => {
            let slots: [String; 2] = VEST_AND_POCKETS.map(str::to_owned);

            process_internal_magazine(ctx, grids, mag_gen, inventory, Some(&slots))
        }
        MagGenKind::Barrel => process_barrel(ctx, grids, mag_gen, inventory),
        MagGenKind::External => process_external(ctx, grids, mag_gen, inventory),
    }
}

/// `InternalMagazineInventoryMagGen.Process` (`:21-34`) and `UbglExternalMagGen.Process`
/// (`:21-34`), which differ only in the equipment slots they name — the internal one passes `null`
/// and so takes `AddAmmoIntoEquipmentSlots`'s own vest+pockets default, the UBGL one passes the
/// same pair explicitly.
///
/// # Errors
///
/// `(int)bulletCount` on the `double?` `GetRandomizedBulletCount` returns: a magazine whose parent
/// is missing from the items view yields null there and the C# cast throws.
fn process_internal_magazine(
    ctx: &mut BotContext,
    grids: &mut ContainerGrids,
    mag_gen: &InventoryMagGen,
    inventory: &mut Vec<Item>,
    equipment_slots: Option<&[String]>,
) -> Result<(), LootError> {
    let bullet_count = get_randomized_bullet_count(ctx, mag_gen.mag_counts, mag_gen.magazine_tpl)?;
    let Some(bullet_count) = bullet_count else {
        return Err(LootError::new(format!(
            "Nullable object must have a value: bullet count for magazine: {}",
            mag_gen.magazine_tpl
        )));
    };

    add_ammo_into_equipment_slots(
        ctx,
        grids,
        mag_gen.ammo_tpl,
        bullet_count as i32,
        inventory,
        equipment_slots,
    )
}

/// `BarrelInventoryMagGen.Process` (`:24-48`).
///
/// # Errors
///
/// The unguarded `StackMinRandom.Value`/`StackMaxRandom.Value` at `:36-37`, and an ammo tpl missing
/// from the items view, which is a `NullReferenceException` one line earlier.
fn process_barrel(
    ctx: &mut BotContext,
    grids: &mut ContainerGrids,
    mag_gen: &InventoryMagGen,
    inventory: &mut Vec<Item>,
) -> Result<(), LootError> {
    let Some(ammo_template) = get_item(ctx.items, mag_gen.ammo_tpl) else {
        return Err(LootError::new(format!(
            "Ammo: {} is missing from the database, unable to generate barrel loads",
            mag_gen.ammo_tpl
        )));
    };

    // Can't be done by _props.ammoType as grenade launcher shoots grenades with ammoType of
    // "buckshot"
    let randomised_ammo_stack_size = if ammo_template.stack_max_random == Some(1) {
        // Doesn't stack
        get_int(3, 6)
    } else {
        let (Some(min), Some(max)) = (
            ammo_template.stack_min_random,
            ammo_template.stack_max_random,
        ) else {
            return Err(LootError::new(format!(
                "Nullable object must have a value: stack randomisation of ammo: {}",
                mag_gen.ammo_tpl
            )));
        };

        get_int(min, max)
    };

    add_ammo_into_equipment_slots(
        ctx,
        grids,
        mag_gen.ammo_tpl,
        randomised_ammo_stack_size,
        inventory,
        None,
    )
}

/// `ExternalInventoryMagGen.Process` (`:37-164`).
///
/// The C# loop decrements its own counter (`:156`) so a failed fit is retried without costing a
/// magazine; the `while` below does the same by skipping the increment. `i < randomizedMagazineCount`
/// at `:75` is trivially true inside the loop and is not reproduced as a separate test.
///
/// # Errors
///
/// From `GetRandomizedMagazineCount` (an unusable weights map), `CreateMagazineWithAmmo` (a
/// cartridge with no `StackMaxSize`) and the magazine-slot filter deref in
/// [`get_random_external_magazine_for_internal_magazine_gun`].
fn process_external(
    ctx: &mut BotContext,
    grids: &mut ContainerGrids,
    mag_gen: &InventoryMagGen,
    inventory: &mut Vec<Item>,
) -> Result<(), LootError> {
    let items = ctx.items;

    // Count of attempts to fit a magazine into bot inventory
    let mut fit_attempts = 0;

    let mut magazine_tpl = mag_gen.magazine_tpl.to_owned();
    let mut attempted_mag_blacklist: Vec<String> = Vec::new();
    let weapon = get_item(items, mag_gen.weapon_tpl);
    let weapon_name = weapon
        .and_then(|weapon| weapon.name.clone())
        .unwrap_or_default();
    let default_magazine_tpl = weapon.and_then(|weapon| weapon.def_mag_type.clone());
    let is_shotgun = is_of_baseclass(items, mag_gen.weapon_tpl, SHOTGUN);

    let randomized_magazine_count = get_randomized_magazine_count(mag_gen.mag_counts)? as i32;
    let equipment_slots: [String; 2] = VEST_AND_POCKETS.map(str::to_owned);

    let mut index = 0;
    while index < randomized_magazine_count {
        let mut magazine_with_ammo =
            create_magazine_with_ammo(ctx, &magazine_tpl, mag_gen.ammo_tpl)?;
        let root_id = magazine_with_ammo[0].id.clone();

        let fits_into_inventory = grids.add_item_with_children_to_equipment_slot(
            ctx,
            &equipment_slots,
            &root_id,
            &magazine_tpl,
            &mut magazine_with_ammo,
            inventory,
        );

        if fits_into_inventory == ItemAddedResult::NoContainers {
            // No containers to fit magazines, stop trying
            break;
        }

        // No space for magazine and we haven't reached desired magazine count
        let mut retry_this_magazine = false;
        if fits_into_inventory == ItemAddedResult::NoSpace {
            // Prevent infinite loop by only allowing 5 attempts at fitting a magazine into inventory
            if fit_attempts > 5 {
                ctx.diagnostics.push(debug(format!(
                    "Failed {fit_attempts} times to add magazine {magazine_tpl} to bot inventory, stopping"
                )));

                break;
            }

            // We were unable to fit at least the minimum amount of magazines, fall back to the
            // default magazine and try again
            if Some(&magazine_tpl) == default_magazine_tpl.as_ref() {
                // We were already on default - stop here to prevent infinite loop
                break;
            }

            // Add failed magazine tpl to blacklist
            attempted_mag_blacklist.push(magazine_tpl.clone());

            let Some(default_magazine_tpl) = default_magazine_tpl.as_deref() else {
                // No default to fall back to, stop trying to add mags
                break;
            };

            if default_magazine_tpl == MAGAZINE {
                // Magazine base type, do not use
                break;
            }

            // Set chosen magazine tpl to the weapons default magazine tpl and try to fit into
            // inventory next loop
            magazine_tpl = default_magazine_tpl.to_owned();
            let Some(mag_template) = get_item(items, &magazine_tpl) else {
                ctx.diagnostics.push(Diagnostic {
                    level: ERROR.to_owned(),
                    locale_key: Some("bot-unable_to_find_default_magazine_item".to_owned()),
                    args: Some(serde_json::Value::String(magazine_tpl.clone())),
                    message: None,
                });

                break;
            };

            // Edge case - some weapons (SKS + shotguns) have an internal magazine as default,
            // choose random non-internal magazine to add to bot instead
            if mag_template.reload_mag_type.as_deref() == Some(INTERNAL_MAGAZINE) {
                let result = get_random_external_magazine_for_internal_magazine_gun(
                    ctx,
                    mag_gen.weapon_tpl,
                    &attempted_mag_blacklist,
                )?;

                let Some(result) = result else {
                    // Highly likely shotgun has no external mags
                    if is_shotgun {
                        break;
                    }

                    ctx.diagnostics.push(debug(format!(
                        "Unable to add additional magazine into bot inventory: vest/pockets for weapon: {weapon_name}, attempted: {fit_attempts} times. Reason: {}",
                        item_added_result_name(fits_into_inventory)
                    )));

                    break;
                };

                magazine_tpl = result;
            }

            fit_attempts += 1;

            // Reduce loop counter by 1 to ensure we get full cout of desired magazines
            retry_this_magazine = true;
        }

        if fits_into_inventory == ItemAddedResult::Success {
            // Reset fit counter now it succeeded
            fit_attempts = 0;
        }

        if !retry_this_magazine {
            index += 1;
        }
    }

    Ok(())
}

/// `ExternalInventoryMagGen.GetRandomExternalMagazineForInternalMagazineGun` (`:173-201`) — the
/// chosen magazine's tpl, or `None` where the C# returns null.
///
/// **Deviation:** C# `:176` calls `.Properties.Slots.FirstOrDefault(...)` unguarded, so a weapon
/// missing from the database or declaring no `Slots` is an NRE/`ArgumentNullException` there; both
/// return `None` here, which is the same "no magazine slot" path the C# `magSlot is null` check at
/// `:177` takes two lines later.
///
/// # Errors
///
/// The two unguarded derefs at `:184-186`: a weapon whose `mod_magazine` slot has no filter
/// (`.Filters.First()`), and a filter naming a tpl that is not in the database (`.Value.Properties`
/// on the null lookup result).
fn get_random_external_magazine_for_internal_magazine_gun(
    ctx: &BotContext,
    weapon_tpl: &str,
    magazine_blacklist: &[String],
) -> Result<Option<String>, LootError> {
    let items = ctx.items;

    // The mag Slot data for the weapon
    let mag_slot = get_item(items, weapon_tpl)
        .and_then(|weapon| weapon.slots.as_ref())
        .and_then(|slots| {
            slots
                .iter()
                .find(|slot| slot.name.as_deref() == Some("mod_magazine"))
        });
    let Some(mag_slot) = mag_slot else {
        return Ok(None);
    };

    let Some(filter) = mag_slot.filter.as_deref() else {
        return Err(LootError::new(format!(
            "Sequence contains no elements: mod_magazine filters of weapon: {weapon_tpl}"
        )));
    };

    // All possible mags that fit into the weapon excluding blacklisted, minus the internal ones
    let mut external_magazine_only_pool = Vec::new();
    for tpl in filter {
        if magazine_blacklist
            .iter()
            .any(|blacklisted| blacklisted == tpl)
        {
            continue;
        }

        let Some(magazine) = get_item(items, tpl) else {
            return Err(LootError::new(format!(
                "Magazine: {tpl} of weapon: {weapon_tpl} is missing from the database"
            )));
        };

        if magazine.reload_mag_type.as_deref() != Some(INTERNAL_MAGAZINE) {
            external_magazine_only_pool.push(tpl.clone());
        }
    }

    if external_magazine_only_pool.is_empty() {
        return Ok(None);
    }

    // Randomly chosen external magazine
    Ok(Some(get_array_value(&external_magazine_only_pool).clone()))
}

fn debug(message: String) -> Diagnostic {
    Diagnostic {
        level: DEBUG.to_owned(),
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
    use crate::bot::models::{EquipmentFilters, RandomisedResourceDetails};
    use crate::loot::models::ItemView;
    use crate::loot::random_util::TestSeedGuard;

    const SEED: u64 = 42;

    const MAGAZINE_PARENT: &str = "aaaaaaaaaaaaaaaaaaaaaaa1";
    const EXTERNAL_MAG: &str = "aaaaaaaaaaaaaaaaaaaaaaa2";
    const INTERNAL_MAG: &str = "aaaaaaaaaaaaaaaaaaaaaaa3";
    const RIFLE: &str = "aaaaaaaaaaaaaaaaaaaaaaa4";
    const FLARE_GUN: &str = "aaaaaaaaaaaaaaaaaaaaaaa5";
    const UBGL: &str = "aaaaaaaaaaaaaaaaaaaaaaa6";
    const AMMO: &str = "aaaaaaaaaaaaaaaaaaaaaaa7";
    const GRENADE: &str = "aaaaaaaaaaaaaaaaaaaaaaa8";
    const VEST: &str = "aaaaaaaaaaaaaaaaaaaaaaa9";

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
                    MAGAZINE_PARENT: {"name": "Magazine"},
                    LAUNCHER: {"name": "Launcher"},
                    EXTERNAL_MAG: {"parent": MAGAZINE_PARENT, "cartridgesMaxCount": 30,
                                   "reloadMagType": "ExternalMagazine", "width": 1, "height": 1},
                    INTERNAL_MAG: {"parent": MAGAZINE_PARENT, "cartridgesMaxCount": 10,
                                   "reloadMagType": "InternalMagazine", "width": 1, "height": 1},
                    RIFLE: {"name": "Rifle", "reloadMode": "ExternalMagazine",
                            "defMagType": EXTERNAL_MAG},
                    FLARE_GUN: {"name": "Flare gun", "reloadMode": "OnlyBarrel"},
                    UBGL: {"name": "UBGL", "parent": LAUNCHER, "reloadMode": "ExternalMagazine",
                           "cartridgesMaxCount": 1},
                    AMMO: {"stackMaxSize": 60, "stackMinRandom": 10, "stackMaxRandom": 20,
                           "width": 1, "height": 1},
                    GRENADE: {"stackMaxSize": 1, "stackMinRandom": 1, "stackMaxRandom": 1,
                              "width": 1, "height": 1},
                    VEST: {"grids": [{"name": "main", "cellsH": 4, "cellsV": 4}]},
                }))
                .unwrap(),
                bosses: Vec::new(),
                durability: serde_json::from_value(json!({
                    "default": {"armor": {"maxDelta": 0, "minDelta": 0, "minLimitPercent": 0},
                        "weapon": {"lowestMax": 0, "highestMax": 0, "maxDelta": 0, "minDelta": 0,
                                   "minLimitPercent": 0}},
                    "botDurabilities": {},
                    "pmc": {"armor": {"lowestMaxPercent": 0, "highestMaxPercent": 0, "maxDelta": 0,
                                      "minDelta": 0, "minLimitPercent": 0},
                        "weapon": {"lowestMax": 0, "highestMax": 0, "maxDelta": 0, "minDelta": 0,
                                   "minLimitPercent": 0}},
                }))
                .unwrap(),
                equipment: IndexMap::new(),
                randomization: IndexMap::new(),
            }
        }

        fn ctx(&self) -> BotContext<'_> {
            BotContext {
                items: &self.items,
                bosses: &self.bosses,
                durability: &self.durability,
                equipment: &self.equipment,
                loot_item_resource_randomization: &self.randomization,
                item_blacklist: &crate::bot::NO_BLACKLIST,
                default_presets_by_tpl: &crate::bot::NO_PRESETS,
                presets_by_id: &crate::bot::NO_PRESETS,
                item_presets: &crate::bot::NO_PRESETS,
                equipment_blacklist: &crate::bot::NO_EQUIP_BLACKLIST,
                low_profile_gas_block_tpls: &crate::bot::NO_BLACKLIST,
                weapon_has_enhancement_chance_percent: 0.0,
                repair_kit_weapon: &crate::bot::NO_BUFFS,
                secure_container_ammo_stack_count: 0,
                is_night_time: false,
                diagnostics: Vec::new(),
            }
        }
    }

    fn one_magazine() -> IndexMap<String, f64> {
        IndexMap::from([("1".to_owned(), 1.0)])
    }

    /// A bot with an empty tactical vest, and the grids that track it.
    fn bot_with_a_vest(ctx: &BotContext) -> (ContainerGrids, Vec<Item>) {
        let container: Item =
            serde_json::from_value(json!({"_id": "vest1", "_tpl": VEST, "slotId": "TacticalVest"}))
                .unwrap();
        let mut grids = ContainerGrids::default();
        grids.add_empty_container(ctx, "TacticalVest", &container);

        (grids, vec![container])
    }

    // -----------------------------------------------------------------------
    // Dispatch
    // -----------------------------------------------------------------------

    /// `MagGenSetUp` sorts on `GetPriority()`; the constant has to already be in that order.
    #[test]
    fn mag_gen_order_is_ascending_priority() {
        let priorities: Vec<i32> = MAG_GEN_ORDER.iter().map(|kind| kind.priority()).collect();

        assert_eq!(priorities, vec![0, 1, 50, 99]);
        assert!(priorities.windows(2).all(|pair| pair[0] < pair[1]));
    }

    #[test]
    fn dispatch_routes_each_weapon_to_its_own_implementation() {
        let fixture = Fixture::new();
        let ctx = fixture.ctx();

        let chosen = |magazine_tpl: &str, weapon_tpl: &str| {
            MAG_GEN_ORDER
                .into_iter()
                .find(|kind| {
                    can_handle(
                        &ctx,
                        *kind,
                        &InventoryMagGen {
                            mag_counts: &one_magazine(),
                            magazine_tpl,
                            weapon_tpl,
                            ammo_tpl: AMMO,
                        },
                    )
                })
                .unwrap()
        };

        assert_eq!(chosen(EXTERNAL_MAG, RIFLE), MagGenKind::External);
        assert_eq!(chosen(INTERNAL_MAG, RIFLE), MagGenKind::InternalMagazine);
        assert_eq!(chosen(EXTERNAL_MAG, FLARE_GUN), MagGenKind::Barrel);
        assert_eq!(chosen(UBGL, UBGL), MagGenKind::Ubgl);
        // Priority decides when two can handle it: the internal magazine (0) beats the UBGL (1).
        assert_eq!(chosen(INTERNAL_MAG, UBGL), MagGenKind::InternalMagazine);
    }

    /// `FirstOrDefault(...).Process(...)` at `BotWeaponGenerator.cs:464` is unguarded.
    #[test]
    fn dispatch_with_no_handler_is_the_unguarded_null_deref() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx();
        let (mut grids, mut inventory) = bot_with_a_vest(&ctx);

        let result = process_mag_gen(
            &mut ctx,
            &mut grids,
            &[MagGenKind::InternalMagazine, MagGenKind::Ubgl],
            &InventoryMagGen {
                mag_counts: &one_magazine(),
                magazine_tpl: EXTERNAL_MAG,
                weapon_tpl: RIFLE,
                ammo_tpl: AMMO,
            },
            &mut inventory,
        );

        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // The four implementations
    // -----------------------------------------------------------------------

    #[test]
    fn the_external_generator_packs_whole_magazines_into_the_vest() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx();
        let (mut grids, mut inventory) = bot_with_a_vest(&ctx);
        let _guard = TestSeedGuard::install(SEED);

        process_mag_gen(
            &mut ctx,
            &mut grids,
            &MAG_GEN_ORDER,
            &InventoryMagGen {
                mag_counts: &IndexMap::from([("2".to_owned(), 1.0)]),
                magazine_tpl: EXTERNAL_MAG,
                weapon_tpl: RIFLE,
                ammo_tpl: AMMO,
            },
            &mut inventory,
        )
        .unwrap();

        // Two magazines, each with one cartridge stack, on top of the vest itself.
        let magazines: Vec<&Item> = inventory
            .iter()
            .filter(|item| item.template == EXTERNAL_MAG)
            .collect();
        assert_eq!(magazines.len(), 2);
        assert!(magazines.iter().all(|magazine| {
            magazine.parent_id.as_deref() == Some("vest1")
                && magazine.slot_id.as_deref() == Some("main")
        }));
        assert_eq!(
            inventory
                .iter()
                .filter(|item| item.slot_id.as_deref() == Some("cartridges"))
                .count(),
            2
        );
    }

    /// A vest with no room stops the loop rather than retrying forever: the weapon's default
    /// magazine is the one that just failed, which is `ExternalInventoryMagGen.cs:92`.
    #[test]
    fn the_external_generator_stops_when_the_vest_is_full() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx();
        let container: Item = serde_json::from_value(
            json!({"_id": "vest1", "_tpl": "tinyvest", "slotId": "TacticalVest"}),
        )
        .unwrap();
        let items: IndexMap<String, ItemView> = serde_json::from_value(json!({
            MAGAZINE_PARENT: {"name": "Magazine"},
            EXTERNAL_MAG: {"parent": MAGAZINE_PARENT, "cartridgesMaxCount": 30,
                           "reloadMagType": "ExternalMagazine", "width": 1, "height": 1},
            RIFLE: {"name": "Rifle", "reloadMode": "ExternalMagazine", "defMagType": EXTERNAL_MAG},
            AMMO: {"stackMaxSize": 60, "width": 1, "height": 1},
            "tinyvest": {"grids": [{"name": "main", "cellsH": 1, "cellsV": 1}]},
        }))
        .unwrap();
        ctx.items = &items;

        let mut grids = ContainerGrids::default();
        grids.add_empty_container(&ctx, "TacticalVest", &container);
        let mut inventory = vec![container];
        let _guard = TestSeedGuard::install(SEED);

        process_mag_gen(
            &mut ctx,
            &mut grids,
            &MAG_GEN_ORDER,
            &InventoryMagGen {
                mag_counts: &IndexMap::from([("5".to_owned(), 1.0)]),
                magazine_tpl: EXTERNAL_MAG,
                weapon_tpl: RIFLE,
                ammo_tpl: AMMO,
            },
            &mut inventory,
        )
        .unwrap();

        // One magazine fits; the second finds no space and the weapon is already on its default
        // magazine, so the loop breaks instead of burning the remaining three.
        assert_eq!(
            inventory
                .iter()
                .filter(|item| item.template == EXTERNAL_MAG)
                .count(),
            1
        );
    }

    #[test]
    fn the_internal_generator_adds_loose_rounds() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx();
        let (mut grids, mut inventory) = bot_with_a_vest(&ctx);
        let _guard = TestSeedGuard::install(SEED);

        process_mag_gen(
            &mut ctx,
            &mut grids,
            &MAG_GEN_ORDER,
            &InventoryMagGen {
                mag_counts: &IndexMap::from([("2".to_owned(), 1.0)]),
                magazine_tpl: INTERNAL_MAG,
                weapon_tpl: RIFLE,
                ammo_tpl: AMMO,
            },
            &mut inventory,
        )
        .unwrap();

        // 10 rounds of capacity x 2 magazines, and 60 to a stack, so one stack of 20.
        let rounds: Vec<&Item> = inventory
            .iter()
            .filter(|item| item.template == AMMO)
            .collect();
        assert_eq!(rounds.len(), 1);
        assert_eq!(
            rounds[0]
                .upd
                .as_ref()
                .and_then(|upd| upd.stack_objects_count),
            Some(20.0)
        );
    }

    #[test]
    fn the_barrel_generator_draws_three_to_six_for_ammo_that_does_not_stack() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx();
        let (mut grids, mut inventory) = bot_with_a_vest(&ctx);
        let _guard = TestSeedGuard::install(SEED);

        process_mag_gen(
            &mut ctx,
            &mut grids,
            &MAG_GEN_ORDER,
            &InventoryMagGen {
                mag_counts: &one_magazine(),
                magazine_tpl: EXTERNAL_MAG,
                weapon_tpl: FLARE_GUN,
                ammo_tpl: GRENADE,
            },
            &mut inventory,
        )
        .unwrap();

        // StackMaxRandom of 1 takes the GetInt(3, 6) arm, and a StackMaxSize of 1 then splits the
        // result into that many single-round stacks.
        let rounds = inventory
            .iter()
            .filter(|item| item.template == GRENADE)
            .count();
        assert!((3..=6).contains(&rounds), "{rounds} rounds");
    }

    #[test]
    fn the_barrel_generator_draws_the_stack_range_for_ammo_that_stacks() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx();
        let (mut grids, mut inventory) = bot_with_a_vest(&ctx);
        let _guard = TestSeedGuard::install(SEED);

        process_mag_gen(
            &mut ctx,
            &mut grids,
            &MAG_GEN_ORDER,
            &InventoryMagGen {
                mag_counts: &one_magazine(),
                magazine_tpl: EXTERNAL_MAG,
                weapon_tpl: FLARE_GUN,
                ammo_tpl: AMMO,
            },
            &mut inventory,
        )
        .unwrap();

        let stack = inventory
            .iter()
            .find(|item| item.template == AMMO)
            .and_then(|item| item.upd.as_ref())
            .and_then(|upd| upd.stack_objects_count)
            .unwrap();
        assert!((10.0..=20.0).contains(&stack), "{stack} rounds");
    }

    #[test]
    fn the_ubgl_generator_adds_one_grenade_per_magazine_count() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx();
        let (mut grids, mut inventory) = bot_with_a_vest(&ctx);
        let _guard = TestSeedGuard::install(SEED);

        process_mag_gen(
            &mut ctx,
            &mut grids,
            &MAG_GEN_ORDER,
            &InventoryMagGen {
                mag_counts: &IndexMap::from([("2".to_owned(), 1.0)]),
                magazine_tpl: UBGL,
                weapon_tpl: UBGL,
                ammo_tpl: GRENADE,
            },
            &mut inventory,
        )
        .unwrap();

        // A launcher magazine holds one chambered grenade, x2 magazines, and grenades don't stack.
        assert_eq!(
            inventory
                .iter()
                .filter(|item| item.template == GRENADE)
                .count(),
            2
        );
    }
}
