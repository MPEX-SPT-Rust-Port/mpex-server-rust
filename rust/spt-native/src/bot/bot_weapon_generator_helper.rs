//! `Helpers/Bot/BotWeaponGeneratorHelper.cs` — magazine and bullet counts, and the magazine+ammo
//! item pair a bot's weapon is handed.
//!
//! # RNG calls, in C# source order — the parity contract
//!
//! - [`get_randomized_magazine_count`] — one `GetWeightedValue` (`:70`), which is itself 0 draws
//!   for a single-entry map, 1 `GetInt` when the weights sum to the entry count, and 1 `GetDouble`
//!   otherwise.
//! - [`get_randomized_bullet_count`] — that same draw and nothing else. It happens **first**
//!   (`:28`), before the parent lookup, so the failure path below still consumes it.
//! - [`create_magazine_with_ammo`] — one `GetInt`, inside `FillMagazineWithCartridge`.
//! - [`magazine_is_cylinder_related`] and [`add_ammo_into_equipment_slots`] draw nothing.
use indexmap::IndexMap;

use crate::bot::BotContext;
use crate::bot::bot_generator_helper::{ContainerGrids, ItemAddedResult};
use crate::loot::item_helper::{
    LAUNCHER, LootError, fill_magazine_with_cartridge, get_item, split_stack,
};
use crate::loot::models::{DEBUG, Diagnostic, ERROR, Item, Upd};
use crate::loot::{mongo_id, random_util};

/// `BotWeaponGeneratorHelper._magCheck` (`:18`).
const MAG_CHECK: [&str; 2] = ["CylinderMagazine", "SpringDrivenCylinder"];

/// The `AddAmmoIntoEquipmentSlots` default (`:116`), and the pair `UbglExternalMagGen` and
/// `ExternalInventoryMagGen` pass explicitly. `EquipmentSlots` member names as strings, in the
/// order the C# `HashSet` collection-expression preserves.
pub const VEST_AND_POCKETS: [&str; 2] = ["TacticalVest", "Pockets"];

/// `BotWeaponGeneratorHelper.MagazineIsCylinderRelated` (`:78-81`).
pub fn magazine_is_cylinder_related(magazine_parent_name: &str) -> bool {
    MAG_CHECK.contains(&magazine_parent_name)
}

/// `BotWeaponGeneratorHelper.GetRandomizedMagazineCount` (`:68-71`).
///
/// `GenerationData.Weights` is a `Dictionary<double, double>` in C# and an `IndexMap<String, f64>`
/// here — JSON object keys are strings and `f64` is not hashable — so the drawn key is parsed back
/// out. C#'s `(int)` cast truncates towards zero; that truncation is applied before returning, and
/// the result is an `f64` only because every caller multiplies it into one.
///
/// # Errors
///
/// Where the C# throws (an empty weights map), plus the key parse the C# deserializer does up
/// front.
pub fn get_randomized_magazine_count(mag_counts: &IndexMap<String, f64>) -> Result<f64, LootError> {
    let chosen = random_util::get_weighted_value(mag_counts)?;

    let count: f64 = chosen.parse().map_err(|_| {
        LootError::new(format!(
            "Magazine count weighting key is not a number: {chosen}"
        ))
    })?;

    Ok(count.trunc())
}

/// `BotWeaponGeneratorHelper.GetRandomizedBulletCount` (`:26-61`). `None` is the C# `null` return:
/// the magazine's parent is missing from the items view.
///
/// C# is handed the resolved `TemplateItem`; this takes the tpl and looks it up, so a magazine tpl
/// that is itself missing takes that same `null` path rather than the C# `NullReferenceException`.
///
/// # Errors
///
/// From [`get_randomized_magazine_count`], drawn before anything else is read.
pub fn get_randomized_bullet_count(
    ctx: &mut BotContext,
    mag_counts: &IndexMap<String, f64>,
    mag_tpl: &str,
) -> Result<Option<f64>, LootError> {
    // Never return lower than 1 to prevent a multiplication by 0
    let randomized_magazine_count = get_randomized_magazine_count(mag_counts)?.max(1.0);

    let items = ctx.items;
    let mag_template = get_item(items, mag_tpl);
    let parent_tpl = mag_template
        .and_then(|template| template.parent.as_deref())
        .unwrap_or_default();

    let (Some(mag_template), Some(parent_item)) = (mag_template, get_item(items, parent_tpl))
    else {
        ctx.diagnostics.push(Diagnostic {
            level: ERROR.to_owned(),
            locale_key: None,
            args: None,
            message: Some(format!(
                "Parent item null when trying to get randomized bullet count for: {mag_tpl}"
            )),
        });

        return Ok(None);
    };

    let parent_name = parent_item.name.as_deref().unwrap_or_default();
    let chamber_bullet_count = if magazine_is_cylinder_related(parent_name) {
        let first_slot_ammo_tpl = mag_template
            .cartridges_first_filter
            .as_deref()
            .and_then(|filter| filter.first())
            .map_or("", String::as_str);
        let ammo_max_stack_size = get_item(items, first_slot_ammo_tpl)
            .and_then(|ammo| ammo.stack_max_size)
            .unwrap_or(1);

        if ammo_max_stack_size == 1 {
            // Rotating grenade launcher
            Some(1.0)
        } else {
            // Shotguns/revolvers. We count the number of camoras as the _max_count of the magazine
            // is 0
            mag_template.slots.as_ref().map(|slots| slots.len() as f64)
        }
    } else if parent_tpl == LAUNCHER {
        // Underbarrel launchers can only have 1 chambered grenade
        Some(1.0)
    } else {
        mag_template.cartridges_max_count
    };

    // Get the amount of bullets that would fit in the internal magazine
    // and multiply by how many magazines were supposed to be created
    Ok(chamber_bullet_count.map(|count| count * randomized_magazine_count))
}

/// `BotWeaponGeneratorHelper.CreateMagazineWithAmmo` (`:90-97`) — the magazine root plus the
/// cartridge stacks `FillMagazineWithCartridge` fills it to capacity with.
///
/// # Errors
///
/// From `fill_magazine_with_cartridge`: a cartridge tpl with no `StackMaxSize`.
pub fn create_magazine_with_ammo(
    ctx: &mut BotContext,
    magazine_tpl: &str,
    ammo_tpl: &str,
) -> Result<Vec<Item>, LootError> {
    let items = ctx.items;
    let mut magazine = vec![Item {
        id: mongo_id::generate(),
        template: magazine_tpl.to_owned(),
        ..Default::default()
    }];

    fill_magazine_with_cartridge(
        items,
        &mut ctx.diagnostics,
        &mut magazine,
        magazine_tpl,
        ammo_tpl,
        1.0,
    )?;

    Ok(magazine)
}

/// `BotWeaponGeneratorHelper.AddAmmoIntoEquipmentSlots` (`:107-149`) — split the requested count
/// into stacks and drop each into the first container that will take it.
///
/// `equipment_slots_to_add_to` is `None` for the C# default of tactical vest then pockets.
///
/// # Errors
///
/// From `split_stack`: the C# hang on a cartridge tpl with no usable `StackMaxSize`.
pub fn add_ammo_into_equipment_slots(
    ctx: &mut BotContext,
    grids: &mut ContainerGrids,
    ammo_tpl: &str,
    cartridge_count: i32,
    inventory: &mut Vec<Item>,
    equipment_slots_to_add_to: Option<&[String]>,
) -> Result<(), LootError> {
    // null guard input param
    let default_slots: [String; 2] = VEST_AND_POCKETS.map(str::to_owned);
    let equipment_slots = equipment_slots_to_add_to.unwrap_or(&default_slots);

    let ammo_items = split_stack(
        ctx.items,
        &Item {
            id: mongo_id::generate(),
            template: ammo_tpl.to_owned(),
            upd: Some(Upd {
                stack_objects_count: Some(f64::from(cartridge_count)),
                ..Default::default()
            }),
            ..Default::default()
        },
    )?;

    for ammo_item in ammo_items {
        let mut ammo_item = [ammo_item];
        let result = grids.add_item_with_children_to_equipment_slot(
            ctx,
            equipment_slots,
            &ammo_item[0].id.clone(),
            &ammo_item[0].template.clone(),
            &mut ammo_item,
            inventory,
        );

        if result != ItemAddedResult::Success {
            ctx.diagnostics.push(Diagnostic {
                level: DEBUG.to_owned(),
                locale_key: None,
                args: None,
                message: Some(format!(
                    "Unable to add ammo: {} to bot inventory, {}",
                    ammo_item[0].template,
                    item_added_result_name(result)
                )),
            });

            // If there's no space for 1 stack or no containers to hold item, there's no space for
            // the others
            if matches!(
                result,
                ItemAddedResult::NoSpace | ItemAddedResult::NoContainers
            ) {
                break;
            }
        }
    }

    Ok(())
}

/// `ItemAddedResult.ToString()` — the C# member names, which are SCREAMING_CASE in
/// `Models/Enums/ItemAddedResult.cs` and reach a log line verbatim.
pub fn item_added_result_name(result: ItemAddedResult) -> &'static str {
    match result {
        ItemAddedResult::Unknown => "UNKNOWN",
        ItemAddedResult::Success => "SUCCESS",
        ItemAddedResult::NoSpace => "NO_SPACE",
        ItemAddedResult::NoContainers => "NO_CONTAINERS",
        ItemAddedResult::IncompatibleItem => "INCOMPATIBLE_ITEM",
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    use crate::bot::durability_limits_helper::BotDurability;
    use crate::bot::models::{EquipmentFilters, RandomisedResourceDetails};
    use crate::loot::models::ItemView;
    use crate::loot::random_util::{TestSeedGuard, get_double, get_int};

    const SEED: u64 = 42;

    const MAGAZINE_PARENT_TPL: &str = "aaaaaaaaaaaaaaaaaaaaaaaa";
    const CYLINDER_PARENT_TPL: &str = "bbbbbbbbbbbbbbbbbbbbbbbb";
    const MAGAZINE_TPL: &str = "cccccccccccccccccccccccc";
    const REVOLVER_CYLINDER_TPL: &str = "dddddddddddddddddddddddd";
    const GRENADE_CYLINDER_TPL: &str = "eeeeeeeeeeeeeeeeeeeeeeee";
    const UBGL_MAGAZINE_TPL: &str = "ffffffffffffffffffffffff";
    const ORPHAN_MAGAZINE_TPL: &str = "111111111111111111111111";
    const CARTRIDGE_TPL: &str = "222222222222222222222222";
    const GRENADE_TPL: &str = "333333333333333333333333";

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
                    // Parents: only `_name` and the tpl itself are read off them.
                    MAGAZINE_PARENT_TPL: {"name": "Magazine"},
                    CYLINDER_PARENT_TPL: {"name": "CylinderMagazine"},
                    LAUNCHER: {"name": "Launcher"},
                    MAGAZINE_TPL: {
                        "parent": MAGAZINE_PARENT_TPL,
                        "cartridgesMaxCount": 30,
                        "cartridgesFirstFilter": [CARTRIDGE_TPL],
                    },
                    // A revolver: stackable ammo, so the camora count is the capacity.
                    REVOLVER_CYLINDER_TPL: {
                        "parent": CYLINDER_PARENT_TPL,
                        "cartridgesMaxCount": 0,
                        "cartridgesFirstFilter": [CARTRIDGE_TPL],
                        "slots": [{"name": "camora_000"}, {"name": "camora_001"},
                                  {"name": "camora_002"}, {"name": "camora_003"},
                                  {"name": "camora_004"}],
                    },
                    // A rotating grenade launcher: its ammo does not stack.
                    GRENADE_CYLINDER_TPL: {
                        "parent": CYLINDER_PARENT_TPL,
                        "cartridgesFirstFilter": [GRENADE_TPL],
                        "slots": [{"name": "camora_000"}, {"name": "camora_001"}],
                    },
                    UBGL_MAGAZINE_TPL: {"parent": LAUNCHER, "cartridgesMaxCount": 40},
                    ORPHAN_MAGAZINE_TPL: {"parent": "999999999999999999999999"},
                    CARTRIDGE_TPL: {"stackMaxSize": 60},
                    GRENADE_TPL: {"stackMaxSize": 1},
                }))
                .unwrap(),
                bosses: Vec::new(),
                durability: serde_json::from_value(json!({
                    "default": {
                        "armor": {"maxDelta": 0, "minDelta": 0, "minLimitPercent": 0},
                        "weapon": {"lowestMax": 0, "highestMax": 0, "maxDelta": 0,
                                   "minDelta": 0, "minLimitPercent": 0}
                    },
                    "botDurabilities": {},
                    "pmc": {
                        "armor": {"lowestMaxPercent": 0, "highestMaxPercent": 0, "maxDelta": 0,
                                  "minDelta": 0, "minLimitPercent": 0},
                        "weapon": {"lowestMax": 0, "highestMax": 0, "maxDelta": 0,
                                   "minDelta": 0, "minLimitPercent": 0}
                    }
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
                default_presets_by_tpl: &crate::bot::NO_DEFAULT_PRESETS,
                equipment_blacklist: &crate::bot::NO_EQUIP_BLACKLIST,
                low_profile_gas_block_tpls: &crate::bot::NO_BLACKLIST,
                item_presets: &crate::bot::NO_PRESETS,
                weapon_has_enhancement_chance_percent: 0.0,
                repair_kit_weapon: &crate::bot::NO_BUFFS,
                secure_container_ammo_stack_count: 0,
                is_night_time: false,
                diagnostics: Vec::new(),
            }
        }
    }

    fn weights(entries: &[(&str, f64)]) -> IndexMap<String, f64> {
        entries
            .iter()
            .map(|(count, weight)| ((*count).to_owned(), *weight))
            .collect()
    }

    /// Where the shared RNG stream sits after `consume` has run against `SEED`.
    fn stream_position_after(consume: impl FnOnce()) -> f64 {
        let _guard = TestSeedGuard::install(SEED);
        consume();

        get_double(0.0, 1.0)
    }

    // -----------------------------------------------------------------------
    // get_randomized_magazine_count
    // -----------------------------------------------------------------------

    #[test]
    fn magazine_count_returns_the_only_key_without_drawing() {
        let counts = weights(&[("3", 5.0)]);

        let after_count = stream_position_after(|| {
            assert_eq!(get_randomized_magazine_count(&counts).unwrap(), 3.0);
        });

        assert_eq!(after_count, stream_position_after(|| {}));
    }

    #[test]
    fn magazine_count_draws_one_weighted_value() {
        let counts = weights(&[("1", 1.0), ("2", 1.0), ("3", 1.0)]);
        let _guard = TestSeedGuard::install(SEED);

        // Weights summing to the entry count take `GetWeightedValue`'s uniform arm: one GetInt,
        // which lands on the third key under this seed.
        assert_eq!(get_randomized_magazine_count(&counts).unwrap(), 3.0);
    }

    #[test]
    fn magazine_count_draws_exactly_the_one_get_int() {
        let counts = weights(&[("1", 1.0), ("2", 1.0), ("3", 1.0)]);

        let after_count = stream_position_after(|| {
            get_randomized_magazine_count(&counts).unwrap();
        });
        let after_manual = stream_position_after(|| {
            get_int(0, 2);
        });

        assert_eq!(after_count, after_manual);
    }

    #[test]
    fn magazine_count_truncates_the_key_towards_zero() {
        assert_eq!(
            get_randomized_magazine_count(&weights(&[("2.9", 1.0)])).unwrap(),
            2.0
        );
        assert!(get_randomized_magazine_count(&weights(&[("two", 1.0)])).is_err());
        assert!(get_randomized_magazine_count(&IndexMap::new()).is_err());
    }

    // -----------------------------------------------------------------------
    // magazine_is_cylinder_related
    // -----------------------------------------------------------------------

    #[test]
    fn cylinder_check_matches_the_two_names_exactly() {
        assert!(magazine_is_cylinder_related("CylinderMagazine"));
        assert!(magazine_is_cylinder_related("SpringDrivenCylinder"));
        assert!(!magazine_is_cylinder_related("cylindermagazine"));
        assert!(!magazine_is_cylinder_related("Magazine"));
        assert!(!magazine_is_cylinder_related(""));
    }

    // -----------------------------------------------------------------------
    // get_randomized_bullet_count
    // -----------------------------------------------------------------------

    /// A single-entry weighting draws nothing, so every count below is `chamber * 3`.
    fn three_magazines() -> IndexMap<String, f64> {
        weights(&[("3", 1.0)])
    }

    #[test]
    fn bullet_count_multiplies_the_magazines_capacity_by_the_magazine_count() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx();

        let count =
            get_randomized_bullet_count(&mut ctx, &three_magazines(), MAGAZINE_TPL).unwrap();

        assert_eq!(count, Some(90.0));
        assert!(ctx.diagnostics.is_empty());
    }

    #[test]
    fn bullet_count_of_a_revolver_counts_its_camoras() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx();

        // CartridgesMaxCount is 0 on a cylinder, so the five camoras are the capacity.
        let count =
            get_randomized_bullet_count(&mut ctx, &three_magazines(), REVOLVER_CYLINDER_TPL)
                .unwrap();

        assert_eq!(count, Some(15.0));
    }

    #[test]
    fn bullet_count_of_a_rotating_grenade_launcher_is_one_chamber() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx();

        // Its grenades have a StackMaxSize of 1, which takes the single-chamber arm.
        let count = get_randomized_bullet_count(&mut ctx, &three_magazines(), GRENADE_CYLINDER_TPL)
            .unwrap();

        assert_eq!(count, Some(3.0));
    }

    #[test]
    fn bullet_count_of_an_underbarrel_launcher_is_one_chamber() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx();

        let count =
            get_randomized_bullet_count(&mut ctx, &three_magazines(), UBGL_MAGAZINE_TPL).unwrap();

        assert_eq!(count, Some(3.0));
    }

    #[test]
    fn bullet_count_never_multiplies_by_a_zero_magazine_count() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx();

        let count =
            get_randomized_bullet_count(&mut ctx, &weights(&[("0", 1.0)]), MAGAZINE_TPL).unwrap();

        assert_eq!(count, Some(30.0));
    }

    #[test]
    fn bullet_count_is_none_without_a_capacity_or_a_parent() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx();

        // Parent tpl missing from the view: the C# `null` return, with its error line.
        assert_eq!(
            get_randomized_bullet_count(&mut ctx, &three_magazines(), ORPHAN_MAGAZINE_TPL).unwrap(),
            None
        );
        // The magazine tpl itself missing takes the same path.
        assert_eq!(
            get_randomized_bullet_count(&mut ctx, &three_magazines(), "999999999999999999999999")
                .unwrap(),
            None
        );

        assert_eq!(ctx.diagnostics.len(), 2);
        assert_eq!(ctx.diagnostics[0].level, ERROR);
        assert_eq!(
            ctx.diagnostics[0].message.as_deref(),
            Some(
                format!(
                    "Parent item null when trying to get randomized bullet count for: {ORPHAN_MAGAZINE_TPL}"
                )
                .as_str()
            )
        );
    }

    /// The magazine count is drawn at `:28`, before the parent is looked up — a magazine that
    /// bails out still moved the stream.
    #[test]
    fn bullet_count_draws_the_magazine_count_even_when_it_gives_up() {
        let fixture = Fixture::new();
        let counts = weights(&[("1", 1.0), ("2", 1.0), ("3", 1.0)]);

        let after_bullet_count = stream_position_after(|| {
            let mut ctx = fixture.ctx();
            get_randomized_bullet_count(&mut ctx, &counts, ORPHAN_MAGAZINE_TPL).unwrap();
        });
        let after_manual = stream_position_after(|| {
            get_int(0, 2);
        });

        assert_eq!(after_bullet_count, after_manual);
    }

    // -----------------------------------------------------------------------
    // create_magazine_with_ammo
    // -----------------------------------------------------------------------

    #[test]
    fn magazine_with_ammo_is_a_root_plus_its_cartridges() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx();
        let _guard = TestSeedGuard::install(SEED);

        let magazine = create_magazine_with_ammo(&mut ctx, MAGAZINE_TPL, CARTRIDGE_TPL).unwrap();

        // A 30-round magazine filled to capacity, 60 to a stack: one stack of 30.
        assert_eq!(magazine.len(), 2);
        assert_eq!(magazine[0].template, MAGAZINE_TPL);
        assert_eq!(magazine[0].id.len(), 24);
        assert!(magazine[0].parent_id.is_none());
        assert!(magazine[0].slot_id.is_none());

        assert_eq!(magazine[1].template, CARTRIDGE_TPL);
        assert_eq!(magazine[1].id.len(), 24);
        assert_eq!(magazine[1].parent_id.as_ref(), Some(&magazine[0].id));
        assert_eq!(magazine[1].slot_id.as_deref(), Some("cartridges"));
        // A lone stack carries no location.
        assert!(magazine[1].location.is_none());
        assert_eq!(
            magazine[1]
                .upd
                .as_ref()
                .and_then(|upd| upd.stack_objects_count),
            Some(30.0)
        );
        assert!(ctx.diagnostics.is_empty());
    }

    #[test]
    fn magazine_with_ammo_gives_every_call_fresh_ids() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx();

        let first = create_magazine_with_ammo(&mut ctx, MAGAZINE_TPL, CARTRIDGE_TPL).unwrap();
        let second = create_magazine_with_ammo(&mut ctx, MAGAZINE_TPL, CARTRIDGE_TPL).unwrap();

        assert_ne!(first[0].id, second[0].id);
        assert_ne!(first[1].id, second[1].id);
    }

    /// One `GetInt` inside `FillMagazineWithCartridge`, and nothing else.
    #[test]
    fn magazine_with_ammo_draws_exactly_one_get_int() {
        let fixture = Fixture::new();

        let after_create = stream_position_after(|| {
            let mut ctx = fixture.ctx();
            create_magazine_with_ammo(&mut ctx, MAGAZINE_TPL, CARTRIDGE_TPL).unwrap();
        });
        let after_manual = stream_position_after(|| {
            get_int(30, 30);
        });

        assert_eq!(after_create, after_manual);
    }
}
