//! The slice of `Helpers/Items/ItemHelper.cs` and `Extensions/ItemExtensions.cs` the loot generator
//! leans on: template lookups, base-class tests, item-tree cloning/re-iding, and container sizing.

use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;

use super::models::{
    CounterState, DEBUG, Diagnostic, ERROR, Item, ItemView, LootConfigView, PresetView,
    SeasonalView, SptLootItem, StaticAmmoDetails, Upd, WARNING,
};
use super::probability_object_array::{ProbabilityObject, ProbabilityObjectArray};
use super::{mongo_id, random_util};

// Base-class tpls, copied verbatim from `Models/Enums/BaseClasses.cs`. They live here rather than in
// their own module because `item_helper` is the only place base classes are ever tested against.

/// `BaseClasses.MONEY`
pub const MONEY: &str = "543be5dd4bdc2deb348b4569";
/// `BaseClasses.AMMO`
pub const AMMO: &str = "5485a8684bdc2da71d8b4567";
/// `BaseClasses.AMMO_BOX`
pub const AMMO_BOX: &str = "543be5cb4bdc2deb348b4568";
/// `BaseClasses.MAGAZINE`
pub const MAGAZINE: &str = "5448bc234bdc2d3c308b4569";
/// `BaseClasses.WEAPON`
pub const WEAPON: &str = "5422acb9af1c889c16000029";
/// `BaseClasses.LAUNCHER`
pub const LAUNCHER: &str = "55818b014bdc2ddc698b456b";
/// `BaseClasses.SPRING_DRIVEN_CYLINDER`
pub const SPRING_DRIVEN_CYLINDER: &str = "627a137bf21bc425b06ab944";
/// `BaseClasses.HEADWEAR`
pub const HEADWEAR: &str = "5a341c4086f77401f2541505";
/// `BaseClasses.VEST`
pub const VEST: &str = "5448e5284bdc2dcb718b4567";
/// `BaseClasses.ARMOR`
pub const ARMOR: &str = "5448e54d4bdc2dcc718b4568";
/// `BaseClasses.ARMORED_EQUIPMENT`
pub const ARMORED_EQUIPMENT: &str = "57bef4c42459772e8d35a53b";
/// `BaseClasses.MOD`
pub const MOD: &str = "5448fe124bdc2da5018b4567";
/// `BaseClasses.MOUNT`
pub const MOUNT: &str = "55818b224bdc2dde698b456f";
/// `BaseClasses.IRON_SIGHT`
pub const IRON_SIGHT: &str = "55818ac54bdc2d5b648b456e";
/// `BaseClasses.SIGHTS`
pub const SIGHTS: &str = "5448fe7a4bdc2d6f028b456b";
/// `BaseClasses.ASSAULT_SCOPE`
pub const ASSAULT_SCOPE: &str = "55818add4bdc2d5b648b456f";
/// `BaseClasses.OPTIC_SCOPE`
pub const OPTIC_SCOPE: &str = "55818ae44bdc2dde698b456c";
/// `BaseClasses.SPECIAL_SCOPE`
pub const SPECIAL_SCOPE: &str = "55818aeb4bdc2ddc698b456a";
/// `BaseClasses.SHOTGUN`
pub const SHOTGUN: &str = "5447b6094bdc2dc3278b4567";
/// `BaseClasses.COLLIMATOR`
pub const COLLIMATOR: &str = "55818ad54bdc2ddc698b4569";
/// `BaseClasses.COMPACT_COLLIMATOR`
pub const COMPACT_COLLIMATOR: &str = "55818acf4bdc2dde698b456b";
/// `BaseClasses.PORTABLE_RANGE_FINDER`
pub const PORTABLE_RANGE_FINDER: &str = "61605ddea09d851a0a0c1bbc";

/// `ItemHelper.GetItem` (`ItemHelper.cs:491-501`) — a plain lookup, absent tpl included.
pub fn get_item<'a>(items_view: &'a IndexMap<String, ItemView>, tpl: &str) -> Option<&'a ItemView> {
    items_view.get(tpl)
}

/// `ItemHelper.IsOfBaseclass` (`ItemHelper.cs:296-299`).
pub fn is_of_baseclass(
    items_view: &IndexMap<String, ItemView>,
    tpl: &str,
    base_class_tpl: &str,
) -> bool {
    is_of_baseclasses(items_view, tpl, &[base_class_tpl])
}

/// `ItemHelper.IsOfBaseclasses` (`ItemHelper.cs:307-310`).
///
/// C# answers this from `ItemBaseClassService`'s precomputed cache; this walks the parent chain
/// instead, which yields the same answers: `AddBaseItems` (`ItemBaseClassService.cs:71-80`) seeds
/// each item's set from `item.Parent` and climbs until a parent is missing or is itself parentless,
/// so an item is never its own base class, the walk reaches the root node id, and an unknown tpl
/// answers false.
///
/// The one C# behaviour not reproducible here is the `_rootNodeIds` short-circuit — the cache only
/// covers templates with `_type == "Item"`, so C# returns false for a *node* tpl even though its
/// parent chain would match. `ItemView` carries no `_type`, and the loot generator only ever asks
/// about real item tpls, so nothing observable hangs on it.
pub fn is_of_baseclasses(
    items_view: &IndexMap<String, ItemView>,
    tpl: &str,
    base_class_tpls: &[&str],
) -> bool {
    let mut current = items_view.get(tpl);

    while let Some(item) = current {
        let parent = match item.parent.as_deref() {
            Some(parent) if !parent.is_empty() => parent,
            // Root node reached, chain exhausted.
            _ => return false,
        };

        if base_class_tpls.contains(&parent) {
            return true;
        }

        // A parent that is not in the view ends the walk, exactly as the C# recursion does.
        current = items_view.get(parent);
    }

    false
}

/// `ItemHelper.ArmorItemCanHoldMods` (`ItemHelper.cs:319-322`) — `_armorSlotsThatCanHoldMods`
/// (`ItemHelper.cs:102`).
pub fn armor_item_can_hold_mods(items_view: &IndexMap<String, ItemView>, tpl: &str) -> bool {
    is_of_baseclasses(items_view, tpl, &[HEADWEAR, VEST, ARMOR])
}

/// `ItemHelper._softInsertIds` (`ItemHelper.cs:82-98`).
const SOFT_INSERT_IDS: [&str; 14] = [
    "groin",
    "groin_back",
    "soft_armor_back",
    "soft_armor_front",
    "soft_armor_left",
    "soft_armor_right",
    "shoulder_l",
    "shoulder_r",
    "collar",
    "helmet_top",
    "helmet_back",
    "helmet_eyes",
    "helmet_jaw",
    "helmet_ears",
];

/// `ItemHelper.ItemRequiresSoftInserts` (`ItemHelper.cs:369-392`). The `GetItem` miss and the
/// no-slots exit collapse into the same `is_none_or` here — both return false there too.
pub fn item_requires_soft_inserts(items_view: &IndexMap<String, ItemView>, item_tpl: &str) -> bool {
    // Not a slot that takes soft-inserts
    if !armor_item_can_hold_mods(items_view, item_tpl) {
        return false;
    }

    get_item(items_view, item_tpl)
        .and_then(|details| details.slots.as_deref())
        .is_some_and(|slots| {
            slots.iter().any(|slot| {
                SOFT_INSERT_IDS.contains(
                    &slot
                        .name
                        .as_deref()
                        .unwrap_or_default()
                        .to_lowercase()
                        .as_str(),
                )
            })
        })
}

/// `ItemHelper.GetRandomisedAmmoStackSize` (`ItemHelper.cs:1767-1775`) with its `maxLimit` default
/// of 60, which is the only value any caller passes.
pub fn get_randomised_ammo_stack_size(ammo_item_template: &ItemView) -> i32 {
    const MAX_LIMIT: i32 = 60;

    if ammo_item_template.stack_max_size == Some(1) {
        // Max is one, nothing to randomise
        return 1;
    }

    random_util::get_int(
        ammo_item_template.stack_min_random.unwrap_or(1),
        ammo_item_template
            .stack_max_random
            .unwrap_or(1)
            .min(MAX_LIMIT),
    )
}

/// `ItemExtensions.GetItemWithChildren` (`ItemExtensions.cs:240-278`) — a stack walk that emits the
/// root first and then pops children last-in-first-out, cloning as it goes.
pub fn get_item_with_children(items: &[Item], base_item_id: &str) -> Vec<Item> {
    let mut children_by_parent: HashMap<&str, Vec<&Item>> = HashMap::new();
    let mut root_item = None;

    for item in items {
        if item.id == base_item_id {
            root_item = Some(item);
        }

        if let Some(parent_id) = item.parent_id.as_deref() {
            children_by_parent.entry(parent_id).or_default().push(item);
        }
    }

    // Root not found, nothing to return, exit.
    let Some(root_item) = root_item else {
        return Vec::new();
    };

    let mut result = Vec::new();
    let mut processing_stack = vec![root_item];

    while let Some(current) = processing_stack.pop() {
        result.push(current.clone());

        if let Some(children) = children_by_parent.get(current.id.as_str()) {
            processing_stack.extend(children.iter().copied());
        }
    }

    result
}

/// `ItemExtensions.ReplaceIDs` (`ItemExtensions.cs:394-416`) — sequential in-place walk: each item
/// takes a fresh id and every item still pointing at its old id is reparented, so children listed
/// either side of their parent stay attached.
pub fn replace_ids(items: &mut [Item]) {
    for index in 0..items.len() {
        let new_id = mongo_id::generate();
        let original_id = std::mem::replace(&mut items[index].id, new_id.clone());

        for item in items.iter_mut() {
            if item.parent_id.as_deref() == Some(original_id.as_str()) {
                item.parent_id = Some(new_id.clone());
            }
        }
    }
}

/// `ItemExtensions.RemapRootItemId` (`ItemExtensions.cs:424-448`) — the root is the first element;
/// only its id and its direct children's `parentId` move.
///
/// **Empty input is the C# throw path** — `ItemExtensions.cs:428` dereferences `FirstOrDefault()`
/// unguarded and throws. This returns a fresh id with nothing remapped instead of panicking behind
/// the FFI boundary, so callers must branch to the error/fallback path themselves.
pub fn remap_root_item_id(items: &mut [Item]) -> String {
    let new_id = mongo_id::generate();

    let Some(root_item_existing_id) = items.first().map(|item| item.id.clone()) else {
        return new_id;
    };

    for item in items.iter_mut() {
        if item.id == root_item_existing_id {
            item.id = new_id.clone();

            continue;
        }

        if item.parent_id.as_deref() == Some(root_item_existing_id.as_str()) {
            item.parent_id = Some(new_id.clone());
        }
    }

    new_id
}

/// `ItemHelper.ReparentItemAndChildren` (`ItemHelper.cs:1680-1717`) — re-id the whole tree under
/// `root_item`'s id, then replace element 0 with `root_item` itself. The slice takes a `&mut Vec` at
/// the call site unchanged — clippy rejects the `Vec` in the signature since nothing here resizes.
///
/// **Empty input is the C# throw path** — `ItemHelper.cs:1682` indexes `itemWithChildren[0]`
/// unguarded and throws, which `LocationLootGenerator.cs:1152-1172` catches to log
/// `location-preset_not_found` (naming three production tpls that hit it) before rethrowing. This
/// returns an empty `Vec` rather than panicking behind the FFI boundary, so callers must branch to
/// that error path themselves.
///
/// C# returns the very list it mutated; this returns a **detached deep copy**, so later mutations to
/// the returned `Vec` do not reach the input slice (and vice versa).
pub fn reparent_item_and_children(root_item: &Item, item_with_children: &mut [Item]) -> Vec<Item> {
    let Some(old_root_id) = item_with_children.first().map(|item| item.id.clone()) else {
        return Vec::new();
    };

    let mut id_mappings: HashMap<String, String> = HashMap::new();
    id_mappings.insert(old_root_id, root_item.id.clone());

    for item in item_with_children.iter_mut() {
        let new_id = id_mappings
            .entry(item.id.clone())
            .or_insert_with(mongo_id::generate)
            .clone();

        if let Some(parent_id) = item.parent_id.clone() {
            // A parent outside the list gets a mapping of its own, same as C#.
            item.parent_id = Some(
                id_mappings
                    .entry(parent_id)
                    .or_insert_with(mongo_id::generate)
                    .clone(),
            );
        }

        item.id = new_id;
    }

    // Force the item's details into the first position (C# also logs when the templates differ).
    item_with_children[0] = root_item.clone();

    item_with_children.to_owned()
}

/// `ItemHelper.GetItemSize` (`ItemHelper.cs:1179-1234`) — `(width, height)`. Non-forced child extra
/// size takes the largest per direction, forced extra size sums across every child.
pub fn get_item_size(
    items_view: &IndexMap<String, ItemView>,
    items: &[Item],
    root_item_id: &str,
) -> Option<(i32, i32)> {
    let root_item = items.iter().find(|item| item.id == root_item_id)?;
    let root_template = get_item(items_view, &root_item.template)?;

    let width = root_template.width.unwrap_or(0);
    let height = root_template.height.unwrap_or(0);

    let (mut size_up, mut size_down, mut size_left, mut size_right) = (0, 0, 0, 0);
    let (mut forced_up, mut forced_down, mut forced_left, mut forced_right) = (0, 0, 0, 0);

    for item in get_item_with_children(items, root_item_id) {
        // A template missing from the view contributes nothing, matching C#'s null-propagation.
        let Some(item_db_template) = get_item(items_view, &item.template) else {
            continue;
        };

        if item_db_template.extra_size_force_add.unwrap_or(false) {
            // Deviation: C# uses `ExtraSizeUp!.Value` here and throws on a force-add template with a
            // null ExtraSize; unreachable with real data, and a panic behind FFI is worse.
            forced_up += item_db_template.extra_size_up.unwrap_or(0);
            forced_down += item_db_template.extra_size_down.unwrap_or(0);
            forced_left += item_db_template.extra_size_left.unwrap_or(0);
            forced_right += item_db_template.extra_size_right.unwrap_or(0);
        } else {
            size_up = size_up.max(item_db_template.extra_size_up.unwrap_or(0));
            size_down = size_down.max(item_db_template.extra_size_down.unwrap_or(0));
            size_left = size_left.max(item_db_template.extra_size_left.unwrap_or(0));
            size_right = size_right.max(item_db_template.extra_size_right.unwrap_or(0));
        }
    }

    Some((
        width + size_left + size_right + forced_left + forced_right,
        height + size_up + size_down + forced_up + forced_down,
    ))
}

/// `ItemHelper.GetContainerMapping` (`ItemHelper.cs:1771-1786`) plus `GetBlankContainerMap`
/// (`ItemHelper.cs:1794-1798`) — `CellsV` rows of `CellsH` free cells. C# throws
/// `ItemHelperException` when either is missing; this returns the same message as an `Err`.
pub fn get_container_mapping(
    items_view: &IndexMap<String, ItemView>,
    container_tpl: &str,
) -> Result<Vec<Vec<u8>>, String> {
    let container_template = get_item(items_view, container_tpl);
    let height = container_template.and_then(|template| template.grid_cells_v);
    let width = container_template.and_then(|template| template.grid_cells_h);

    let (Some(height), Some(width)) = (height, width) else {
        return Err(
            "Height or width is null when trying to calculate container mapping".to_owned(),
        );
    };

    // Rows / columns. Clamped because a negative cell count would abort the process on allocation.
    Ok(vec![
        vec![0u8; width.max(0) as usize];
        height.max(0) as usize
    ])
}

/// `ItemExtensions.ToLootItem` (`ItemExtensions.cs:332-350`) — every field plus the extension data,
/// with a null `composedKey`.
pub fn to_loot_item(item: &Item) -> SptLootItem {
    SptLootItem {
        item: item.clone(),
        composed_key: None,
    }
}

/// `ItemHelper.SplitStack` (`ItemHelper.cs:753-784`) — chops `Upd.StackObjectsCount` into
/// `StackMaxSize`-sized stacks, each one a clone of the input carrying a fresh [`mongo_id`]. A count
/// that already fits comes back as a single clone under the *original* id.
///
/// C# hands back the very item it was given when the count is null and a clone in every other case;
/// here every element is an owned clone, so a caller mutating the result never reaches the input.
///
/// The `Upd.StackObjectsCount` setter's `MidpointRounding.AwayFromZero` is not replicated (see
/// `models::Upd`): every count that reaches this function has already been through that setter, so
/// the chunks are integral and the rounding is a no-op.
///
/// **`Err` is the C# hang path.** With a null or non-positive `StackMaxSize` — a tpl missing from the
/// view, or a template without the property — `remainingCount <= maxStackSize` is false and
/// `ItemHelper.cs:773` then subtracts `Math.Min(remaining, maxStackSize ?? 0)`, zero or negative,
/// from `remainingCount` on every pass: the loop never ends and clones pile up until the process
/// dies. A non-finite count loops forever the same way.
pub fn split_stack(
    items_view: &IndexMap<String, ItemView>,
    item_to_split: &Item,
) -> Result<Vec<Item>, LootError> {
    // No count to split by — the template is never consulted.
    let Some(remaining_count) = item_to_split
        .upd
        .as_ref()
        .and_then(|upd| upd.stack_objects_count)
    else {
        return Ok(vec![item_to_split.clone()]);
    };

    let max_stack_size =
        get_item(items_view, &item_to_split.template).and_then(|template| template.stack_max_size);

    // If the current count is already equal or less than the max return the item as is. A lifted
    // comparison in C#, so a null max answers false rather than true.
    if max_stack_size.is_some_and(|max_stack_size| remaining_count <= f64::from(max_stack_size)) {
        return Ok(vec![item_to_split.clone()]);
    }

    // `while (remainingCount > 0)` never runs, so C# returns the list it built: empty. A NaN count
    // fails that comparison too, in C# as here.
    if remaining_count <= 0.0 || remaining_count.is_nan() {
        return Ok(Vec::new());
    }

    let Some(max_stack_size) = max_stack_size.filter(|max_stack_size| *max_stack_size > 0) else {
        return Err(LootError::new(format!(
            "StackMaxSize is null or not positive when trying to split stack of item: {}",
            item_to_split.template
        )));
    };

    if !remaining_count.is_finite() {
        return Err(LootError::new(format!(
            "StackObjectsCount is not finite when trying to split stack of item: {}",
            item_to_split.template
        )));
    }

    let mut remaining_count = remaining_count;
    let mut root_and_children = Vec::new();

    while remaining_count > 0.0 {
        let amount = remaining_count.min(f64::from(max_stack_size));
        let mut new_stack_clone = item_to_split.clone();

        new_stack_clone.id = mongo_id::generate();
        // Upd is present — the count came out of it.
        if let Some(upd) = new_stack_clone.upd.as_mut() {
            upd.stack_objects_count = Some(amount);
        }

        remaining_count -= amount;
        root_and_children.push(new_stack_clone);
    }

    Ok(root_and_children)
}

/// `ItemHelper.SetFoundInRaid(IEnumerable<Item>)` (`ItemHelper.cs:1033-1050`) — flags every item as
/// found in raid, except money and ammo, which have any existing flag *cleared* instead (and never
/// gain an `Upd` they did not already have).
///
/// C# assigns `null` to `Upd.SpawnedInSession` and `WhenWritingNull` drops it on the way out, so
/// removing the key is what reproduces the C# JSON.
pub fn set_found_in_raid(items_view: &IndexMap<String, ItemView>, items: &mut [Item]) {
    for item in items.iter_mut() {
        if is_of_baseclasses(items_view, &item.template, &[MONEY, AMMO]) {
            if let Some(upd) = item.upd.as_mut() {
                upd.extra.remove("SpawnedInSession");
            }

            continue;
        }

        item.upd
            .get_or_insert_default()
            .extra
            .insert("SpawnedInSession".to_owned(), serde_json::Value::Bool(true));
    }
}

// ---------------------------------------------------------------------------
// Cartridge / magazine / child-slot assembly
// ---------------------------------------------------------------------------

/// The read-only views a generation run consults, plus the two things it mutates as it goes: the
/// spawn-limit counters and the diagnostics the C# caller replays through its logger.
///
/// Every view is borrowed for `'a`, so copying one out (`let items_view = ctx.items_view;`) releases
/// the `&mut ctx` and leaves the diagnostics writable — the ported functions lean on that.
pub struct LootContext<'a> {
    pub items_view: &'a IndexMap<String, ItemView>,
    pub static_ammo_dist: &'a HashMap<String, Vec<StaticAmmoDetails>>,
    pub default_presets: &'a HashMap<String, PresetView>,
    pub money_tpls: &'a [String],
    pub lootable_item_blacklist: &'a HashSet<String>,
    pub config: &'a LootConfigView,
    pub seasonal: &'a SeasonalView,
    /// `CounterTrackerHelper`'s state, moved in for the run and handed back in the result.
    pub counter: CounterState,
    pub diagnostics: Vec<Diagnostic>,
}

/// A fatal failure — the C# equivalent throws (`ItemHelperException`) or dereferences a null and
/// crashes. Distinct from a [`Diagnostic`], which is logged while generation carries on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LootError {
    pub message: String,
}

impl LootError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// A plain interpolated log line, the shape most of the ported call sites use.
fn diagnostic(level: &str, message: String) -> Diagnostic {
    Diagnostic {
        level: level.to_owned(),
        locale_key: None,
        args: None,
        message: Some(message),
    }
}

/// C# types `Item.Location` as `object?`, and System.Text.Json writes a whole `double` as `3` where
/// serde_json would write `3.0`. Every location this module produces is whole, so integral values go
/// out as integers and the two serializers stay byte-identical.
fn location_value(location: f64) -> serde_json::Value {
    if location.is_finite() && location.fract() == 0.0 {
        return serde_json::Value::from(location as i64);
    }

    serde_json::Value::from(location)
}

/// `ItemHelper.CreateCartridges` (`ItemHelper.cs:1502-1513`).
pub fn create_cartridges(parent_id: &str, ammo_tpl: &str, stack_count: i32, location: f64) -> Item {
    Item {
        id: mongo_id::generate(),
        template: ammo_tpl.to_owned(),
        parent_id: Some(parent_id.to_owned()),
        slot_id: Some("cartridges".to_owned()),
        location: Some(location_value(location)),
        upd: Some(Upd {
            stack_objects_count: Some(f64::from(stack_count)),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// `ItemHelper.AddCartridgesToAmmoBox` (`ItemHelper.cs:1241-1279`) — stacks of
/// `min(boxMax, cartridgeMax)` whose locations count *down* to 0, and the one that lands on 0 goes
/// out without a location at all, as live does it.
///
/// The `Err` cases are the two C# crash paths, reported rather than thrown so the caller can skip
/// the box: a stack slot naming no cartridge (`ItemHelper.cs:1245` dereferences `cartridgeTpl!`),
/// and an empty box list (`ItemHelper.cs:1266` indexes `ammoBox[0]`). A cartridge with no
/// `StackMaxSize` joins them as a **deviation** — C# spins forever adding empty stacks, which is not
/// something to reproduce behind an FFI boundary.
// A `Diagnostic` carries a `serde_json::Value` of arguments, which `preserve_order` makes wide
// enough to trip the lint. It is a log line on a skip path, not something worth boxing.
#[allow(clippy::result_large_err)]
pub fn add_cartridges_to_ammo_box(
    items_view: &IndexMap<String, ItemView>,
    ammo_box: &mut Vec<Item>,
    ammo_box_tpl: &str,
) -> Result<(), Diagnostic> {
    let ammo_box_details = get_item(items_view, ammo_box_tpl);
    let ammo_box_max_cartridge_count =
        ammo_box_details.and_then(|details| details.stack_slot_max_count);
    let Some(cartridge_tpl) =
        ammo_box_details.and_then(|details| details.stack_slot_first_filter_first.as_deref())
    else {
        return Err(diagnostic(
            ERROR,
            format!(
                "Ammo box: {ammo_box_tpl} lacks a cartridge in its stack slot filter, unable to add cartridges"
            ),
        ));
    };
    let cartridge_max_stack_size =
        get_item(items_view, cartridge_tpl).and_then(|details| details.stack_max_size);

    // Exit early if ammo already exists in box
    if ammo_box.iter().any(|item| item.template == cartridge_tpl) {
        return Ok(());
    }

    let Some(ammo_box_max_cartridge_count) = ammo_box_max_cartridge_count else {
        // `currentStoredCartridgeCount < null` is false in C#, so nothing is ever added.
        return Ok(());
    };

    // Add new stack-size-correct items to ammo box
    let max_per_stack =
        ammo_box_max_cartridge_count.min(f64::from(cartridge_max_stack_size.unwrap_or(0)));
    if ammo_box_max_cartridge_count > 0.0 && max_per_stack <= 0.0 {
        return Err(diagnostic(
            ERROR,
            format!(
                "Cartridge: {cartridge_tpl} of ammo box: {ammo_box_tpl} lacks a StackMaxSize, unable to add cartridges"
            ),
        ));
    }

    let mut current_stored_cartridge_count = 0.0;
    // Find location based on Max ammo box size
    let mut location = (ammo_box_max_cartridge_count / max_per_stack).ceil() - 1.0;

    while current_stored_cartridge_count < ammo_box_max_cartridge_count {
        let Some(parent_id) = ammo_box.first().map(|item| item.id.clone()) else {
            return Err(diagnostic(
                ERROR,
                format!("Ammo box: {ammo_box_tpl} has no root item, unable to add cartridges"),
            ));
        };

        let remaining_space = ammo_box_max_cartridge_count - current_stored_cartridge_count;
        let cartridge_count_to_add = if remaining_space < max_per_stack {
            remaining_space
        } else {
            max_per_stack
        };

        // Add cartridge item into items array
        let mut cartridge_item_to_add = create_cartridges(
            &parent_id,
            cartridge_tpl,
            cartridge_count_to_add as i32,
            location,
        );

        // In live no ammo box has the first cartridge item with a location
        if location == 0.0 {
            cartridge_item_to_add.location = None;
        }

        ammo_box.push(cartridge_item_to_add);

        current_stored_cartridge_count += cartridge_count_to_add;
        location -= 1.0;
    }

    Ok(())
}

/// `ItemHelper.FillMagazineWithRandomCartridge` (`ItemHelper.cs:1291-1330`), with
/// `GetRandomValidCaliber` and `DrawAmmoTpl` folded in as private helpers.
pub fn fill_magazine_with_random_cartridge(
    ctx: &mut LootContext,
    magazine: &mut Vec<Item>,
    mag_tpl: &str,
    caliber: Option<&str>,
    min_size_percent: f64,
    default_cartridge_tpl: Option<&str>,
    weapon_tpl: Option<&str>,
) -> Result<(), LootError> {
    let items_view = ctx.items_view;

    let resolved_caliber = match caliber {
        Some(caliber) => caliber.to_owned(),
        None => get_random_valid_caliber(items_view, mag_tpl)?,
    };
    // Edge case - Klin pp-9 has a typo in its ammo caliber
    let chosen_caliber = if resolved_caliber == "Caliber9x18PMM" {
        "Caliber9x18PM"
    } else {
        resolved_caliber.as_str()
    };

    // A weapon's chamber, when one is passed in, is a whitelist for the draw.
    let cartridge_whitelist = weapon_tpl
        .and_then(|weapon_tpl| get_item(items_view, weapon_tpl))
        .and_then(|weapon| weapon.chambers_first_filter.as_deref());

    // Chose a randomly weighted cartridge that fits
    let Some(cartridge_tpl) = draw_ammo_tpl(
        ctx,
        chosen_caliber,
        default_cartridge_tpl,
        cartridge_whitelist,
    )?
    else {
        let magazine_id = magazine.first().map_or("", |item| item.id.as_str());
        ctx.diagnostics.push(diagnostic(
            DEBUG,
            format!("Unable to fill item: {magazine_id} {mag_tpl} with cartridges, none found."),
        ));

        return Ok(());
    };

    let diagnostics = &mut ctx.diagnostics;

    fill_magazine_with_cartridge(
        items_view,
        diagnostics,
        magazine,
        mag_tpl,
        &cartridge_tpl,
        min_size_percent,
    )
}

/// `ItemHelper.FillMagazineWithCartridge` (`ItemHelper.cs:1339-1418`) — stacks ascend from location
/// 0, and a magazine that ends up with a single stack has that location removed again.
///
/// The `Err` case is the C# crash at `ItemHelper.cs:1409`: a cartridge with no `StackMaxSize` leaves
/// `cartridgeCountToAdd` null and `+= cartridgeCountToAdd!.Value` throws. Like C#, that is only
/// reached once the loop actually runs.
///
/// Takes the two things it reads rather than a whole [`LootContext`], the way
/// [`add_cartridges_to_ammo_box`] does: `bot::bot_weapon_generator_helper::create_magazine_with_ammo`
/// calls it with a `BotContext`'s fields.
pub fn fill_magazine_with_cartridge(
    items_view: &IndexMap<String, ItemView>,
    diagnostics: &mut Vec<Diagnostic>,
    magazine: &mut Vec<Item>,
    mag_tpl: &str,
    cartridge_tpl: &str,
    min_size_multiplier: f64,
) -> Result<(), LootError> {
    // UBGL don't have mags
    if is_of_baseclass(items_view, mag_tpl, LAUNCHER) {
        return Ok(());
    }

    // Get cartridge properties and max allowed stack size
    let cartridge_details = get_item(items_view, cartridge_tpl);
    if cartridge_details.is_none() {
        diagnostics.push(Diagnostic {
            level: ERROR.to_owned(),
            locale_key: Some("item-invalid_tpl_item".to_owned()),
            args: Some(serde_json::Value::String(cartridge_tpl.to_owned())),
            message: None,
        });
    }

    let cartridge_max_stack_size = cartridge_details.and_then(|details| details.stack_max_size);
    if cartridge_max_stack_size.is_none() {
        diagnostics.push(diagnostic(
            ERROR,
            format!("Item with tpl: {cartridge_tpl} lacks a _props or StackMaxSize property"),
        ));
    }

    // Get max number of cartridges in magazine, choose random value between min/max
    let mag_details = get_item(items_view, mag_tpl);
    let magazine_cartridge_max_count =
        if is_of_baseclass(items_view, mag_tpl, SPRING_DRIVEN_CYLINDER) {
            // Edge case for rotating grenade launcher magazine
            mag_details
                .and_then(|details| details.slots.as_ref())
                .map(|slots| slots.len() as f64)
        } else {
            mag_details.and_then(|details| details.cartridges_max_count)
        };

    let Some(magazine_cartridge_max_count) = magazine_cartridge_max_count else {
        diagnostics.push(diagnostic(
            WARNING,
            format!(
                "Magazine: {mag_tpl} lacks a Cartridges array, unable to fill magazine with ammo"
            ),
        ));

        return Ok(());
    };

    let desired_stack_count = random_util::get_int(
        random_util::round_half_even(min_size_multiplier * magazine_cartridge_max_count) as i32,
        magazine_cartridge_max_count as i32,
    );

    if magazine.len() > 1 {
        diagnostics.push(diagnostic(
            WARNING,
            format!("Magazine {mag_tpl} already has cartridges defined,  this may cause issues"),
        ));
    }

    // Loop over cartridge count and add stacks to magazine
    let mut current_stored_cartridge_count = 0;
    let mut location = 0;

    while current_stored_cartridge_count < desired_stack_count {
        let Some(cartridge_max_stack_size) = cartridge_max_stack_size else {
            return Err(LootError::new(format!(
                "Item with tpl: {cartridge_tpl} lacks a _props or StackMaxSize property"
            )));
        };
        let Some(parent_id) = magazine.first().map(|item| item.id.clone()) else {
            // C# indexes `magazineWithChildCartridges[0]` (`ItemHelper.cs:1406`) and throws.
            return Err(LootError::new(format!(
                "Magazine: {mag_tpl} has no root item, unable to fill it with cartridges"
            )));
        };

        // Get stack size of cartridges
        let mut cartridge_count_to_add = if desired_stack_count <= cartridge_max_stack_size {
            desired_stack_count
        } else {
            cartridge_max_stack_size
        };

        // Ensure we don't go over the max stackCount size
        let remaining_space = desired_stack_count - current_stored_cartridge_count;
        if cartridge_count_to_add > remaining_space {
            cartridge_count_to_add = remaining_space;
        }

        // Add cartridge item object into items array
        magazine.push(create_cartridges(
            &parent_id,
            cartridge_tpl,
            cartridge_count_to_add,
            f64::from(location),
        ));

        current_stored_cartridge_count += cartridge_count_to_add;
        location += 1;
    }

    // Only one cartridge stack added, remove location property as it's only used for 2 or more stacks
    if location == 1 {
        magazine[1].location = None;
    }

    Ok(())
}

/// `ItemHelper.AddChildSlotItems` (`ItemHelper.cs:1557-1636`), minus the `requiredOnly` flag no loot
/// call site passes, with `GetCompatibleTplFromArray` (`ItemHelper.cs:1644-1653`) inlined.
///
/// Takes the two things it reads off the context rather than the context itself: the bot family has
/// its own ([`crate::bot::BotContext`]) and calls this from `AddRequiredChildItemsToParent`.
pub fn add_child_slot_items(
    items_view: &IndexMap<String, ItemView>,
    diagnostics: &mut Vec<Diagnostic>,
    item_to_add: Vec<Item>,
    item_tpl: &str,
    mod_spawn_chance_dict: Option<&HashMap<String, f64>>,
) -> Vec<Item> {
    let mut result = item_to_add;
    let mut incompatible_mod_tpls: HashSet<&str> = HashSet::new();
    // C# reads `result[0]` per slot and throws on an empty list; the root never moves, so it is read
    // once here and an absent parent id stands in for the throw.
    let root_id = result.first().map(|item| item.id.clone());

    let slots = get_item(items_view, item_tpl)
        .and_then(|item| item.slots.as_deref())
        .unwrap_or_default();

    for slot in slots {
        let slot_name = slot.name.as_deref().unwrap_or_default();
        let required = slot.required.unwrap_or(false);

        // Roll chance for non-required slot mods
        if let (Some(mod_spawn_chance_dict), false) = (mod_spawn_chance_dict, required) {
            // only roll chance to not include mod if dict exists and has value for this mod type
            // (e.g. front_plate)
            if let Some(chance) = mod_spawn_chance_dict.get(&slot_name.to_lowercase())
                && !random_util::get_chance_100(*chance)
            {
                continue;
            }
        }

        let item_pool = slot.filter.as_deref().unwrap_or_default();
        if item_pool.is_empty() {
            diagnostics.push(diagnostic(
                DEBUG,
                format!("Unable to choose a mod for slot: {slot_name} on item: {item_tpl}, parents' 'Filter' array is empty, skipping"),
            ));

            continue;
        }

        let compatible_tpls: Vec<&String> = item_pool
            .iter()
            .filter(|tpl| !incompatible_mod_tpls.contains(tpl.as_str()))
            .collect();
        if compatible_tpls.is_empty() {
            diagnostics.push(diagnostic(
                DEBUG,
                format!(
                    "Unable to choose a mod for slot: {slot_name} on item: {item_tpl}, no compatible tpl found in pool of {}, skipping",
                    item_pool.len()
                ),
            ));

            continue;
        }

        let chosen_tpl = *random_util::get_array_value(&compatible_tpls);

        // Create basic item structure ready to add to weapon array
        result.push(Item {
            id: mongo_id::generate(),
            template: chosen_tpl.clone(),
            parent_id: root_id.clone(),
            slot_id: slot.name.clone(),
            ..Default::default()
        });

        // Include conflicting items of newly added mod in pool to be used for next mod choice
        if let Some(conflicting_items) = get_item(items_view, chosen_tpl)
            .and_then(|details| details.conflicting_items.as_deref())
        {
            incompatible_mod_tpls.extend(conflicting_items.iter().map(String::as_str));
        }
    }

    result
}

/// `ItemHelper.GetRandomValidCaliber` (`ItemHelper.cs:1425-1436`). Both of its throw paths — a
/// magazine with no cartridge filter, and a drawn caliber that is null (either an empty list, which
/// makes `DrawRandomFromList` index `RandInt(0)`, or an item with no `Caliber`) — come back as
/// [`LootError`] with the C# message.
fn get_random_valid_caliber(
    items_view: &IndexMap<String, ItemView>,
    mag_tpl: &str,
) -> Result<String, LootError> {
    let Some(ammo_tpls) = get_item(items_view, mag_tpl)
        .and_then(|mag_template| mag_template.cartridges_first_filter.as_deref())
    else {
        return Err(LootError::new(
            "Calibers is null when trying to generate random valid caliber",
        ));
    };

    let calibers: Vec<Option<&String>> = ammo_tpls
        .iter()
        .filter_map(|ammo_tpl| get_item(items_view, ammo_tpl))
        .map(|ammo| ammo.caliber.as_ref())
        .collect();

    let chosen_caliber = if calibers.is_empty() {
        None
    } else {
        *random_util::get_array_value(&calibers)
    };

    chosen_caliber.cloned().ok_or_else(|| {
        LootError::new(format!(
            "Chosen caliber is null when trying to fill magazine with random cartridge (magazine: {mag_tpl})"
        ))
    })
}

/// `ItemHelper.DrawAmmoTpl` (`ItemHelper.cs:1446-1492`) — a weighted draw over the caliber's pool,
/// filtered by the weapon chamber whitelist when one is supplied.
///
/// The `Err` case is the C# crash at `ItemHelper.cs:1487`, which casts `RelativeProbability!.Value`.
fn draw_ammo_tpl(
    ctx: &mut LootContext,
    caliber: &str,
    fallback_cartridge_tpl: Option<&str>,
    cartridge_whitelist: Option<&[String]>,
) -> Result<Option<String>, LootError> {
    let ammos = ctx
        .static_ammo_dist
        .get(caliber)
        .map_or_else(|| [].as_slice(), Vec::as_slice);

    if ammos.is_empty() {
        if let Some(fallback_cartridge_tpl) = fallback_cartridge_tpl {
            ctx.diagnostics.push(diagnostic(
                WARNING,
                format!("Unable to pick a cartridge for caliber: {caliber}, staticAmmoDist has no data. using fallback value of {fallback_cartridge_tpl}"),
            ));

            return Ok(Some(fallback_cartridge_tpl.to_owned()));
        }

        ctx.diagnostics.push(diagnostic(
            WARNING,
            format!("Unable to pick a cartridge for caliber: {caliber}, staticAmmoDist has no data. No fallback value provided"),
        ));

        return Ok(None);
    }

    let mut ammo_array: ProbabilityObjectArray<String, ()> = ProbabilityObjectArray::new();
    for ammo_details in ammos {
        let Some(tpl) = ammo_details.tpl.as_deref() else {
            ctx.diagnostics.push(diagnostic(
                ERROR,
                "Ammo details tpl is null when trying to draw ammo from pool".to_owned(),
            ));

            continue;
        };

        // Whitelist exists and tpl not inside it, skip. Fixes 9x18mm kedr issues
        if cartridge_whitelist
            .is_some_and(|whitelist| !whitelist.iter().any(|allowed| allowed == tpl))
        {
            continue;
        }

        let Some(relative_probability) = ammo_details.relative_probability else {
            return Err(LootError::new(format!(
                "Ammo: {tpl} of caliber: {caliber} lacks a relativeProbability"
            )));
        };

        ammo_array.add(ProbabilityObject {
            key: tpl.to_owned(),
            relative_probability,
            data: None,
        });
    }

    Ok(ammo_array.draw(1).into_iter().next())
}

#[cfg(test)]
mod tests {
    use std::sync::LazyLock;

    use super::*;

    use serde_json::json;

    /// `BaseClasses.ITEM` — the root node every base class below hangs off.
    const ITEM_NODE: &str = "54009119af1c881c07000029";
    const ARMOR_VEST_TPL: &str = "111111111111111111111111";
    const HELMET_TPL: &str = "222222222222222222222222";
    const MOD_PLAIN_A_TPL: &str = "333333333333333333333333";
    const MOD_PLAIN_B_TPL: &str = "444444444444444444444444";
    const MOD_FORCED_A_TPL: &str = "555555555555555555555555";
    const MOD_FORCED_B_TPL: &str = "666666666666666666666666";
    const CONTAINER_TPL: &str = "777777777777777777777777";
    const ORPHAN_TPL: &str = "888888888888888888888888";
    /// Stackable, and money — so it splits, and `set_found_in_raid` clears its flag.
    const MONEY_ROUBLES_TPL: &str = "f0f0f0f0f0f0f0f0f0f0f0f0";
    const AMMO_545_TPL: &str = "f1f1f1f1f1f1f1f1f1f1f1f1";

    /// Every view is built through serde so the tests exercise the same wire shape the C# caller
    /// sends, rather than a hand-rolled struct literal.
    fn fixture() -> IndexMap<String, ItemView> {
        serde_json::from_value(json!({
            // Parent chain: ITEM_NODE <- ARMOR <- ARMOR_VEST_TPL, ITEM_NODE <- HEADWEAR <- HELMET_TPL.
            ITEM_NODE: {},
            ARMOR: { "parent": ITEM_NODE },
            HEADWEAR: { "parent": ITEM_NODE },
            ARMOR_VEST_TPL: { "parent": ARMOR, "width": 3, "height": 4 },
            HELMET_TPL: { "parent": HEADWEAR, "width": 2, "height": 4 },
            // Non-forced mods: the biggest of each direction wins.
            MOD_PLAIN_A_TPL: {
                "parent": HEADWEAR, "extraSizeUp": 1, "extraSizeDown": 0,
                "extraSizeLeft": 1, "extraSizeRight": 0, "extraSizeForceAdd": false
            },
            MOD_PLAIN_B_TPL: {
                "parent": HEADWEAR, "extraSizeUp": 2, "extraSizeDown": 0,
                "extraSizeLeft": 0, "extraSizeRight": 1
            },
            // Forced mods: every direction sums, across items.
            MOD_FORCED_A_TPL: {
                "parent": HEADWEAR, "extraSizeUp": 1, "extraSizeDown": 1,
                "extraSizeLeft": 1, "extraSizeRight": 1, "extraSizeForceAdd": true
            },
            MOD_FORCED_B_TPL: {
                "parent": HEADWEAR, "extraSizeUp": 0, "extraSizeDown": 1,
                "extraSizeLeft": 2, "extraSizeRight": 0, "extraSizeForceAdd": true
            },
            CONTAINER_TPL: { "parent": ITEM_NODE, "gridCellsH": 5, "gridCellsV": 3 },
            MONEY: { "parent": ITEM_NODE },
            AMMO: { "parent": ITEM_NODE },
            MONEY_ROUBLES_TPL: { "parent": MONEY, "stackMaxSize": 100 },
            AMMO_545_TPL: { "parent": AMMO, "stackMaxSize": 60 },
            // Parent points at a tpl that is not in the view at all.
            ORPHAN_TPL: { "parent": "999999999999999999999999" },
        }))
        .unwrap()
    }

    fn item(id: &str, template: &str, parent_id: Option<&str>) -> Item {
        Item {
            id: id.to_owned(),
            template: template.to_owned(),
            parent_id: parent_id.map(str::to_owned),
            ..Default::default()
        }
    }

    fn ids(items: &[Item]) -> Vec<&str> {
        items.iter().map(|item| item.id.as_str()).collect()
    }

    /// Root `r` with children `a` then `b`; `a` has children `a1` then `a2`.
    fn two_level_tree() -> Vec<Item> {
        vec![
            item("r", HELMET_TPL, None),
            item("a", MOD_PLAIN_A_TPL, Some("r")),
            item("b", MOD_PLAIN_B_TPL, Some("r")),
            item("a1", MOD_FORCED_A_TPL, Some("a")),
            item("a2", MOD_FORCED_B_TPL, Some("a")),
        ]
    }

    #[test]
    fn get_item_returns_the_view_for_known_tpls_only() {
        let view = fixture();

        assert_eq!(get_item(&view, HELMET_TPL).unwrap().width, Some(2));
        assert!(get_item(&view, "999999999999999999999999").is_none());
    }

    #[test]
    fn is_of_baseclass_matches_transitively_through_parents() {
        let view = fixture();

        // Direct parent, then the grandparent two links up.
        assert!(is_of_baseclass(&view, ARMOR_VEST_TPL, ARMOR));
        assert!(is_of_baseclass(&view, ARMOR_VEST_TPL, ITEM_NODE));
        assert!(is_of_baseclass(&view, ARMOR, ITEM_NODE));

        assert!(!is_of_baseclass(&view, ARMOR_VEST_TPL, HEADWEAR));
    }

    /// `ItemBaseClassService.AddBaseItems` (`ItemBaseClassService.cs:71-80`) seeds the cache from
    /// `item.Parent`, never from the item's own id, so an item is never its own base class.
    #[test]
    fn is_of_baseclass_is_not_self_inclusive() {
        let view = fixture();

        assert!(!is_of_baseclass(&view, ARMOR_VEST_TPL, ARMOR_VEST_TPL));
        assert!(!is_of_baseclass(&view, ARMOR, ARMOR));
    }

    #[test]
    fn is_of_baseclass_is_false_for_unknown_tpls_and_unknown_parents() {
        let view = fixture();

        // Tpl missing from the view entirely.
        assert!(!is_of_baseclass(&view, "999999999999999999999999", ARMOR));
        // Tpl present, but its parent chain dead-ends outside the view.
        assert!(!is_of_baseclass(&view, ORPHAN_TPL, ITEM_NODE));
        // A root node has no parent to walk.
        assert!(!is_of_baseclass(&view, ITEM_NODE, ARMOR));
    }

    #[test]
    fn is_of_baseclasses_matches_any_of_the_supplied_classes() {
        let view = fixture();

        assert!(is_of_baseclasses(&view, HELMET_TPL, &[ARMOR, HEADWEAR]));
        assert!(!is_of_baseclasses(&view, HELMET_TPL, &[ARMOR, VEST]));
        assert!(!is_of_baseclasses(&view, HELMET_TPL, &[]));
    }

    #[test]
    fn armor_item_can_hold_mods_covers_headwear_vest_and_armor() {
        let view = fixture();

        assert!(armor_item_can_hold_mods(&view, HELMET_TPL));
        assert!(armor_item_can_hold_mods(&view, ARMOR_VEST_TPL));
        assert!(!armor_item_can_hold_mods(&view, CONTAINER_TPL));
    }

    #[test]
    fn get_item_with_children_returns_the_root_then_lifo_order() {
        let result = get_item_with_children(&two_level_tree(), "r");

        // Children are pushed in input order and popped last-in-first-out.
        assert_eq!(ids(&result), vec!["r", "b", "a", "a2", "a1"]);
    }

    #[test]
    fn get_item_with_children_from_a_mid_tree_node_skips_its_siblings() {
        let result = get_item_with_children(&two_level_tree(), "a");

        assert_eq!(ids(&result), vec!["a", "a2", "a1"]);
    }

    #[test]
    fn get_item_with_children_is_empty_when_the_root_is_missing() {
        assert!(get_item_with_children(&two_level_tree(), "nope").is_empty());
    }

    #[test]
    fn replace_ids_gives_every_item_a_fresh_id_and_keeps_the_hierarchy() {
        let mut items = two_level_tree();
        let original = ids(&items)
            .iter()
            .map(|id| (*id).to_owned())
            .collect::<Vec<_>>();

        replace_ids(&mut items);

        for (index, old_id) in original.iter().enumerate() {
            assert_ne!(&items[index].id, old_id);
            assert_eq!(items[index].id.len(), 24);
        }
        // r <- a, r <- b, a <- a1, a <- a2 all still hold, under the new ids.
        assert_eq!(items[1].parent_id.as_ref(), Some(&items[0].id));
        assert_eq!(items[2].parent_id.as_ref(), Some(&items[0].id));
        assert_eq!(items[3].parent_id.as_ref(), Some(&items[1].id));
        assert_eq!(items[4].parent_id.as_ref(), Some(&items[1].id));
    }

    #[test]
    fn replace_ids_reparents_children_listed_before_their_parent() {
        let mut items = vec![
            item("child", MOD_PLAIN_A_TPL, Some("root")),
            item("root", HELMET_TPL, None),
        ];

        replace_ids(&mut items);

        assert_eq!(items[0].parent_id.as_ref(), Some(&items[1].id));
    }

    #[test]
    fn remap_root_item_id_touches_only_the_root_and_its_direct_children() {
        let mut items = two_level_tree();

        let new_root_id = remap_root_item_id(&mut items);

        assert_eq!(new_root_id.len(), 24);
        assert_eq!(items[0].id, new_root_id);
        assert_eq!(items[1].parent_id.as_deref(), Some(new_root_id.as_str()));
        assert_eq!(items[2].parent_id.as_deref(), Some(new_root_id.as_str()));
        // Everything below the first level is left alone.
        assert_eq!(ids(&items)[1..], ["a", "b", "a1", "a2"]);
        assert_eq!(items[3].parent_id.as_deref(), Some("a"));
        assert_eq!(items[4].parent_id.as_deref(), Some("a"));
    }

    #[test]
    fn reparent_item_and_children_grafts_the_tree_under_a_new_root() {
        let mut items = two_level_tree();
        let root_item = item("new_root", ARMOR_VEST_TPL, None);

        let result = reparent_item_and_children(&root_item, &mut items);

        // Element 0 is replaced wholesale by the supplied root, template and all.
        assert_eq!(result[0].id, "new_root");
        assert_eq!(result[0].template, ARMOR_VEST_TPL);
        // Every child got a fresh id but still points at the new root / its remapped parent.
        for child in &result[1..] {
            assert_eq!(child.id.len(), 24);
        }
        assert_eq!(result[1].parent_id.as_deref(), Some("new_root"));
        assert_eq!(result[2].parent_id.as_deref(), Some("new_root"));
        assert_eq!(result[3].parent_id.as_ref(), Some(&result[1].id));
        assert_eq!(result[4].parent_id.as_ref(), Some(&result[1].id));
        // The in-place list and the returned list agree.
        assert_eq!(ids(&items), ids(&result));
    }

    /// The parent mapping is created on demand (`ItemHelper.cs:1697-1701`), so a child listed before
    /// its parent still lands on the id that parent goes on to take.
    #[test]
    fn reparent_item_and_children_maps_a_parent_listed_after_its_child() {
        let mut items = vec![
            item("r", HELMET_TPL, None),
            item("c", MOD_PLAIN_A_TPL, Some("p")),
            item("p", MOD_PLAIN_B_TPL, Some("r")),
        ];
        let root_item = item("new_root", ARMOR_VEST_TPL, None);

        let result = reparent_item_and_children(&root_item, &mut items);

        // "p" had no mapping yet when "c" was processed, and got that same fresh id when its own
        // turn came.
        assert_eq!(result[2].id.len(), 24);
        assert_ne!(result[2].id, "p");
        assert_eq!(result[1].parent_id.as_ref(), Some(&result[2].id));
        assert_eq!(result[2].parent_id.as_deref(), Some("new_root"));
    }

    #[test]
    fn get_item_size_maxes_non_forced_extra_size_and_sums_forced() {
        let view = fixture();
        let items = two_level_tree();

        // Root 2x4. Non-forced: up max(1,2)=2, down 0, left max(1,0)=1, right max(0,1)=1.
        // Forced: up 1+0=1, down 1+1=2, left 1+2=3, right 1+0=1.
        // Width = 2 + 1 + 1 + 3 + 1, height = 4 + 2 + 0 + 1 + 2.
        let size = get_item_size(&view, &items, "r").unwrap();

        assert_eq!(size, (8, 9));
    }

    #[test]
    fn get_item_size_ignores_items_outside_the_requested_subtree() {
        let view = fixture();
        let items = two_level_tree();

        // "b" has no children, no width/height of its own, and extra size up 2 / right 1 — the
        // sibling "a" branch must not count towards it.
        let size = get_item_size(&view, &items, "b").unwrap();

        assert_eq!(size, (1, 2));
    }

    #[test]
    fn get_item_size_is_none_when_the_root_or_its_template_is_unknown() {
        let view = fixture();
        let items = two_level_tree();

        assert!(get_item_size(&view, &items, "nope").is_none());
        assert!(
            get_item_size(&view, &[item("r", "999999999999999999999999", None)], "r").is_none()
        );
    }

    #[test]
    fn get_container_mapping_builds_blank_rows_of_cells() {
        let map = get_container_mapping(&fixture(), CONTAINER_TPL).unwrap();

        // CellsV rows of CellsH cells, all free.
        assert_eq!(map.len(), 3);
        assert!(map.iter().all(|row| row == &vec![0u8; 5]));
    }

    #[test]
    fn get_container_mapping_errors_without_a_grid() {
        let view = fixture();

        assert!(get_container_mapping(&view, HELMET_TPL).is_err());
        assert!(get_container_mapping(&view, "999999999999999999999999").is_err());
    }

    #[test]
    fn to_loot_item_copies_fields_and_extras_with_no_composed_key() {
        let source: Item = serde_json::from_value(json!({
            "_id": "aaaaaaaaaaaaaaaaaaaaaaaa",
            "_tpl": HELMET_TPL,
            "parentId": "bbbbbbbbbbbbbbbbbbbbbbbb",
            "slotId": "main",
            "location": 3,
            "upd": { "StackObjectsCount": 2 },
            "modAddedField": "kept",
        }))
        .unwrap();

        let loot_item = to_loot_item(&source);

        assert!(loot_item.composed_key.is_none());
        let out = serde_json::to_value(&loot_item).unwrap();
        assert_eq!(out["_id"], "aaaaaaaaaaaaaaaaaaaaaaaa");
        assert_eq!(out["_tpl"], HELMET_TPL);
        assert_eq!(out["parentId"], "bbbbbbbbbbbbbbbbbbbbbbbb");
        assert_eq!(out["slotId"], "main");
        assert_eq!(out["location"], 3);
        assert_eq!(out["upd"]["StackObjectsCount"], 2.0);
        assert_eq!(out["modAddedField"], "kept");
        assert!(out.as_object().unwrap().get("composedKey").is_none());
    }

    /// A stack of `count` roubles, id `s`.
    fn stack(template: &str, count: f64) -> Item {
        Item {
            id: "s".to_owned(),
            template: template.to_owned(),
            upd: Some(Upd {
                stack_objects_count: Some(count),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    fn counts(items: &[Item]) -> Vec<Option<f64>> {
        items
            .iter()
            .map(|item| item.upd.as_ref().and_then(|upd| upd.stack_objects_count))
            .collect()
    }

    #[test]
    fn split_stack_chunks_the_count_into_max_size_pieces() {
        let result = split_stack(&fixture(), &stack(MONEY_ROUBLES_TPL, 250.0)).unwrap();

        assert_eq!(counts(&result), vec![Some(100.0), Some(100.0), Some(50.0)]);
        // Every chunk is a fresh MongoId, none of them the original's.
        let new_ids: HashSet<&str> = ids(&result).into_iter().collect();
        assert_eq!(new_ids.len(), 3);
        assert!(!new_ids.contains("s"));
        assert!(result.iter().all(|item| item.id.len() == 24));
        assert!(result.iter().all(|item| item.template == MONEY_ROUBLES_TPL));
    }

    #[test]
    fn split_stack_returns_the_item_unchanged_when_it_fits() {
        let view = fixture();

        for count in [1.0, 99.0, 100.0] {
            let result = split_stack(&view, &stack(MONEY_ROUBLES_TPL, count)).unwrap();

            assert_eq!(counts(&result), vec![Some(count)]);
            assert_eq!(ids(&result), vec!["s"]);
        }
    }

    /// `ItemHelper.cs:755` returns the item as-is when there is no count to split by, without
    /// consulting the template at all.
    #[test]
    fn split_stack_returns_the_item_when_the_count_is_absent() {
        let view = fixture();

        let no_upd = item("s", ORPHAN_TPL, None);
        assert_eq!(ids(&split_stack(&view, &no_upd).unwrap()), vec!["s"]);

        let mut no_count = no_upd.clone();
        no_count.upd = Some(Upd::default());
        assert_eq!(ids(&split_stack(&view, &no_count).unwrap()), vec!["s"]);
    }

    /// The C# hang path: a null (or non-positive) `StackMaxSize` makes `ItemHelper.cs:773` subtract
    /// nothing from `remainingCount` on every pass.
    #[test]
    fn split_stack_errors_when_the_max_size_cannot_end_the_loop() {
        let view = fixture();

        // Template absent from the view entirely.
        assert!(split_stack(&view, &stack("999999999999999999999999", 5.0)).is_err());
        // Template present, no StackMaxSize.
        assert!(split_stack(&view, &stack(HELMET_TPL, 5.0)).is_err());
        // Would never terminate either, for all that JSON cannot carry it.
        assert!(split_stack(&view, &stack(MONEY_ROUBLES_TPL, f64::INFINITY)).is_err());
    }

    /// `remainingCount <= maxStackSize` is false against a null max, and `while (remainingCount > 0)`
    /// is false for a count that is not positive, so C# returns the empty list it just built.
    #[test]
    fn split_stack_is_empty_for_a_non_positive_count_with_no_max_size() {
        let view = fixture();

        assert!(
            split_stack(&view, &stack(HELMET_TPL, 0.0))
                .unwrap()
                .is_empty()
        );
        assert!(
            split_stack(&view, &stack(HELMET_TPL, -5.0))
                .unwrap()
                .is_empty()
        );
        assert!(
            split_stack(&view, &stack(HELMET_TPL, f64::NAN))
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn set_found_in_raid_flags_every_item_in_the_tree() {
        let view = fixture();
        let mut items = vec![
            item("r", HELMET_TPL, None),
            item("a", MOD_PLAIN_A_TPL, Some("r")),
            item("b", MOD_PLAIN_B_TPL, Some("r")),
        ];
        // An item that already has an Upd keeps the rest of it.
        items[1].upd = Some(Upd {
            stack_objects_count: Some(2.0),
            ..Default::default()
        });

        set_found_in_raid(&view, &mut items);

        for item in &items {
            let upd = item.upd.as_ref().expect("Upd is created when missing");
            assert_eq!(upd.extra["SpawnedInSession"], json!(true));
        }
        assert_eq!(counts(&items[1..2]), vec![Some(2.0)]);
    }

    /// `ItemHelper.cs:1037-1045`: money and ammo have the flag cleared, never set, and a missing
    /// `Upd` is left missing.
    #[test]
    fn set_found_in_raid_clears_the_flag_on_money_and_ammo() {
        let view = fixture();
        let mut items = vec![
            stack(MONEY_ROUBLES_TPL, 1.0),
            item("a", AMMO_545_TPL, None),
            item("b", AMMO_545_TPL, None),
        ];
        items[0]
            .upd
            .as_mut()
            .unwrap()
            .extra
            .insert("SpawnedInSession".to_owned(), json!(true));
        items[2].upd = Some(Upd::default());

        set_found_in_raid(&view, &mut items);

        // C# assigns null, and WhenWritingNull drops it — so the key is gone on the wire.
        let money = serde_json::to_value(&items[0]).unwrap();
        assert!(
            money["upd"]
                .as_object()
                .unwrap()
                .get("SpawnedInSession")
                .is_none()
        );
        assert!(items[1].upd.is_none());
        assert!(items[2].upd.as_ref().unwrap().extra.is_empty());
    }

    // -----------------------------------------------------------------------
    // Cartridge / magazine / child-slot assembly
    // -----------------------------------------------------------------------

    const CALIBER: &str = "Caliber762x39";
    const CARTRIDGE_A_TPL: &str = "aaaaaaaaaaaaaaaaaaaaaaaa";
    const CARTRIDGE_B_TPL: &str = "bbbbbbbbbbbbbbbbbbbbbbbb";
    /// No `StackMaxSize`, the property the fill loops crash on in C#.
    const CARTRIDGE_NO_STACK_TPL: &str = "cccccccccccccccccccccccc";
    /// No `Caliber`, so it draws a null out of the magazine's filter.
    const CARTRIDGE_NO_CALIBER_TPL: &str = "dddddddddddddddddddddddd";
    const AMMO_BOX_TPL: &str = "a0a0a0a0a0a0a0a0a0a0a0a0";
    const AMMO_BOX_REMAINDER_TPL: &str = "a1a1a1a1a1a1a1a1a1a1a1a1";
    const AMMO_BOX_SINGLE_TPL: &str = "a2a2a2a2a2a2a2a2a2a2a2a2";
    const AMMO_BOX_NO_FILTER_TPL: &str = "a3a3a3a3a3a3a3a3a3a3a3a3";
    const AMMO_BOX_NO_STACK_SIZE_TPL: &str = "a4a4a4a4a4a4a4a4a4a4a4a4";
    const MAGAZINE_TPL: &str = "b0b0b0b0b0b0b0b0b0b0b0b0";
    const MAGAZINE_SMALL_TPL: &str = "b1b1b1b1b1b1b1b1b1b1b1b1";
    const MAGAZINE_NO_CARTRIDGES_TPL: &str = "b2b2b2b2b2b2b2b2b2b2b2b2";
    const MAGAZINE_NO_CALIBER_TPL: &str = "b3b3b3b3b3b3b3b3b3b3b3b3";
    const UBGL_TPL: &str = "b4b4b4b4b4b4b4b4b4b4b4b4";
    const CYLINDER_TPL: &str = "b5b5b5b5b5b5b5b5b5b5b5b5";
    const WEAPON_TPL: &str = "c0c0c0c0c0c0c0c0c0c0c0c0";
    const MOD_A_TPL: &str = "d0d0d0d0d0d0d0d0d0d0d0d0";
    const MOD_B_TPL: &str = "d1d1d1d1d1d1d1d1d1d1d1d1";
    const MOD_C_TPL: &str = "d2d2d2d2d2d2d2d2d2d2d2d2";
    const ITEM_WITH_SLOTS_TPL: &str = "e0e0e0e0e0e0e0e0e0e0e0e0";
    const ITEM_CONFLICT_TPL: &str = "e1e1e1e1e1e1e1e1e1e1e1e1";
    const ITEM_CONFLICT_DEAD_TPL: &str = "e2e2e2e2e2e2e2e2e2e2e2e2";

    fn ammo_fixture() -> IndexMap<String, ItemView> {
        serde_json::from_value(json!({
            ITEM_NODE: {},
            AMMO_BOX: { "parent": ITEM_NODE },
            MAGAZINE: { "parent": ITEM_NODE },
            LAUNCHER: { "parent": ITEM_NODE },
            SPRING_DRIVEN_CYLINDER: { "parent": MAGAZINE },
            WEAPON: { "parent": ITEM_NODE },

            CARTRIDGE_A_TPL: { "parent": ITEM_NODE, "stackMaxSize": 30, "caliber": CALIBER },
            CARTRIDGE_B_TPL: { "parent": ITEM_NODE, "stackMaxSize": 60, "caliber": CALIBER },
            CARTRIDGE_NO_STACK_TPL: { "parent": ITEM_NODE, "caliber": CALIBER },
            CARTRIDGE_NO_CALIBER_TPL: { "parent": ITEM_NODE, "stackMaxSize": 60 },

            // 90 / 30 = 3 stacks, so locations count down 2, 1, 0.
            AMMO_BOX_TPL: {
                "parent": AMMO_BOX, "stackSlotMaxCount": 90,
                "stackSlotFirstFilterFirst": CARTRIDGE_A_TPL
            },
            // 50 / 30 = a full stack then a 20-cartridge remainder.
            AMMO_BOX_REMAINDER_TPL: {
                "parent": AMMO_BOX, "stackSlotMaxCount": 50,
                "stackSlotFirstFilterFirst": CARTRIDGE_A_TPL
            },
            AMMO_BOX_SINGLE_TPL: {
                "parent": AMMO_BOX, "stackSlotMaxCount": 30,
                "stackSlotFirstFilterFirst": CARTRIDGE_A_TPL
            },
            AMMO_BOX_NO_FILTER_TPL: { "parent": AMMO_BOX, "stackSlotMaxCount": 60 },
            AMMO_BOX_NO_STACK_SIZE_TPL: {
                "parent": AMMO_BOX, "stackSlotMaxCount": 60,
                "stackSlotFirstFilterFirst": CARTRIDGE_NO_STACK_TPL
            },

            MAGAZINE_TPL: {
                "parent": MAGAZINE, "cartridgesMaxCount": 60,
                "cartridgesFirstFilter": [CARTRIDGE_A_TPL]
            },
            MAGAZINE_SMALL_TPL: {
                "parent": MAGAZINE, "cartridgesMaxCount": 5,
                "cartridgesFirstFilter": [CARTRIDGE_B_TPL]
            },
            MAGAZINE_NO_CARTRIDGES_TPL: { "parent": MAGAZINE },
            MAGAZINE_NO_CALIBER_TPL: {
                "parent": MAGAZINE, "cartridgesMaxCount": 60,
                "cartridgesFirstFilter": [CARTRIDGE_NO_CALIBER_TPL]
            },
            UBGL_TPL: { "parent": LAUNCHER, "cartridgesMaxCount": 60 },
            // Rotating grenade launcher magazine: capacity is the slot count, not a Cartridges entry.
            CYLINDER_TPL: {
                "parent": SPRING_DRIVEN_CYLINDER,
                "slots": [
                    { "name": "mod_1" }, { "name": "mod_2" }, { "name": "mod_3" },
                    { "name": "mod_4" }, { "name": "mod_5" }, { "name": "mod_6" }
                ]
            },

            WEAPON_TPL: { "parent": WEAPON, "chambersFirstFilter": [CARTRIDGE_B_TPL] },

            MOD_A_TPL: { "parent": ITEM_NODE, "conflictingItems": [MOD_B_TPL] },
            MOD_B_TPL: { "parent": ITEM_NODE },
            MOD_C_TPL: { "parent": ITEM_NODE },

            ITEM_WITH_SLOTS_TPL: {
                "parent": ITEM_NODE,
                "slots": [
                    { "name": "mod_required", "required": true, "filter": [MOD_C_TPL] },
                    { "name": "Mod_Scope", "required": false, "filter": [MOD_C_TPL] },
                    { "name": "mod_other", "required": false, "filter": [MOD_C_TPL] },
                    { "name": "mod_empty", "required": false, "filter": [] }
                ]
            },
            ITEM_CONFLICT_TPL: {
                "parent": ITEM_NODE,
                "slots": [
                    { "name": "slot_one", "required": true, "filter": [MOD_A_TPL] },
                    { "name": "slot_two", "required": true, "filter": [MOD_B_TPL, MOD_C_TPL] }
                ]
            },
            ITEM_CONFLICT_DEAD_TPL: {
                "parent": ITEM_NODE,
                "slots": [
                    { "name": "slot_one", "required": true, "filter": [MOD_A_TPL] },
                    { "name": "slot_two", "required": true, "filter": [MOD_B_TPL] }
                ]
            },
        }))
        .unwrap()
    }

    fn ammo_dist(value: serde_json::Value) -> HashMap<String, Vec<StaticAmmoDetails>> {
        serde_json::from_value(value).unwrap()
    }

    fn no_ammo_dist() -> HashMap<String, Vec<StaticAmmoDetails>> {
        HashMap::new()
    }

    fn context<'a>(
        items_view: &'a IndexMap<String, ItemView>,
        static_ammo_dist: &'a HashMap<String, Vec<StaticAmmoDetails>>,
    ) -> LootContext<'a> {
        // The assembly functions read neither presets, money, blacklist, config nor season, so
        // those members are stubbed and the fixtures stay about ammo.
        static PRESETS: LazyLock<HashMap<String, PresetView>> = LazyLock::new(HashMap::new);
        static MONEY_TPLS: LazyLock<Vec<String>> = LazyLock::new(Vec::new);
        static BLACKLIST: LazyLock<HashSet<String>> = LazyLock::new(HashSet::new);
        static CONFIG: LazyLock<LootConfigView> = LazyLock::new(LootConfigView::default);
        static SEASONAL: LazyLock<SeasonalView> = LazyLock::new(SeasonalView::default);

        LootContext {
            items_view,
            static_ammo_dist,
            default_presets: &PRESETS,
            money_tpls: &MONEY_TPLS,
            lootable_item_blacklist: &BLACKLIST,
            config: &CONFIG,
            seasonal: &SEASONAL,
            counter: CounterState::default(),
            diagnostics: Vec::new(),
        }
    }

    fn root(template: &str) -> Vec<Item> {
        vec![Item {
            id: mongo_id::generate(),
            template: template.to_owned(),
            ..Default::default()
        }]
    }

    fn location_of(item: &Item) -> Option<i64> {
        item.location.as_ref().and_then(serde_json::Value::as_i64)
    }

    fn stack_count_of(item: &Item) -> Option<f64> {
        item.upd.as_ref().and_then(|upd| upd.stack_objects_count)
    }

    fn levels<'a>(ctx: &'a LootContext<'a>) -> Vec<&'a str> {
        ctx.diagnostics
            .iter()
            .map(|entry| entry.level.as_str())
            .collect()
    }

    fn messages(ctx: &LootContext) -> String {
        ctx.diagnostics
            .iter()
            .filter_map(|entry| entry.message.as_deref())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn create_cartridges_builds_a_cartridges_slot_child() {
        let cartridge = create_cartridges("parent-id", CARTRIDGE_A_TPL, 27, 2.0);

        assert_eq!(cartridge.id.len(), 24);
        assert_eq!(cartridge.template, CARTRIDGE_A_TPL);
        assert_eq!(cartridge.parent_id.as_deref(), Some("parent-id"));
        assert_eq!(cartridge.slot_id.as_deref(), Some("cartridges"));
        assert_eq!(location_of(&cartridge), Some(2));
        assert_eq!(stack_count_of(&cartridge), Some(27.0));
        // C# writes `Location` through `object?`, so a whole double lands on the wire as an integer.
        let out = serde_json::to_value(&cartridge).unwrap();
        assert_eq!(out["location"], json!(2));
        assert_eq!(out["upd"]["StackObjectsCount"], json!(27.0));
    }

    #[test]
    fn add_cartridges_to_ammo_box_counts_locations_down_and_nulls_the_last() {
        let view = ammo_fixture();
        let dist = no_ammo_dist();
        let ctx = context(&view, &dist);
        let mut ammo_box = root(AMMO_BOX_TPL);

        add_cartridges_to_ammo_box(ctx.items_view, &mut ammo_box, AMMO_BOX_TPL).unwrap();

        // 90 capacity / 30 per stack -> three stacks, locations 2, 1, then absent.
        assert_eq!(ammo_box.len(), 4);
        assert_eq!(
            ammo_box[1..].iter().map(location_of).collect::<Vec<_>>(),
            vec![Some(2), Some(1), None]
        );
        for cartridge in &ammo_box[1..] {
            assert_eq!(cartridge.template, CARTRIDGE_A_TPL);
            assert_eq!(stack_count_of(cartridge), Some(30.0));
            assert_eq!(cartridge.slot_id.as_deref(), Some("cartridges"));
            assert_eq!(cartridge.parent_id.as_ref(), Some(&ammo_box[0].id));
        }
    }

    #[test]
    fn add_cartridges_to_ammo_box_trims_the_final_stack_to_the_remaining_space() {
        let view = ammo_fixture();
        let dist = no_ammo_dist();
        let ctx = context(&view, &dist);
        let mut ammo_box = root(AMMO_BOX_REMAINDER_TPL);

        add_cartridges_to_ammo_box(ctx.items_view, &mut ammo_box, AMMO_BOX_REMAINDER_TPL).unwrap();

        assert_eq!(ammo_box.len(), 3);
        assert_eq!(stack_count_of(&ammo_box[1]), Some(30.0));
        assert_eq!(location_of(&ammo_box[1]), Some(1));
        assert_eq!(stack_count_of(&ammo_box[2]), Some(20.0));
        assert_eq!(location_of(&ammo_box[2]), None);
    }

    #[test]
    fn add_cartridges_to_ammo_box_leaves_a_lone_stack_without_a_location() {
        let view = ammo_fixture();
        let dist = no_ammo_dist();
        let ctx = context(&view, &dist);
        let mut ammo_box = root(AMMO_BOX_SINGLE_TPL);

        add_cartridges_to_ammo_box(ctx.items_view, &mut ammo_box, AMMO_BOX_SINGLE_TPL).unwrap();

        assert_eq!(ammo_box.len(), 2);
        assert_eq!(stack_count_of(&ammo_box[1]), Some(30.0));
        assert!(ammo_box[1].location.is_none());
    }

    #[test]
    fn add_cartridges_to_ammo_box_is_a_no_op_when_the_cartridge_is_already_there() {
        let view = ammo_fixture();
        let dist = no_ammo_dist();
        let ctx = context(&view, &dist);
        let mut ammo_box = root(AMMO_BOX_TPL);
        ammo_box.push(Item {
            id: mongo_id::generate(),
            template: CARTRIDGE_A_TPL.to_owned(),
            ..Default::default()
        });

        add_cartridges_to_ammo_box(ctx.items_view, &mut ammo_box, AMMO_BOX_TPL).unwrap();

        assert_eq!(ammo_box.len(), 2);
    }

    #[test]
    fn add_cartridges_to_ammo_box_errors_when_the_box_names_no_cartridge() {
        let view = ammo_fixture();
        let dist = no_ammo_dist();
        let ctx = context(&view, &dist);
        let mut ammo_box = root(AMMO_BOX_NO_FILTER_TPL);

        // C# dereferences `cartridgeTpl!.Value` here and throws.
        let error =
            add_cartridges_to_ammo_box(ctx.items_view, &mut ammo_box, AMMO_BOX_NO_FILTER_TPL)
                .unwrap_err();

        assert_eq!(error.level, ERROR);
        assert_eq!(ammo_box.len(), 1);
    }

    #[test]
    fn add_cartridges_to_ammo_box_errors_instead_of_spinning_on_a_zero_stack_size() {
        let view = ammo_fixture();
        let dist = no_ammo_dist();
        let ctx = context(&view, &dist);
        let mut ammo_box = root(AMMO_BOX_NO_STACK_SIZE_TPL);

        // Deviation: `maxPerStack` of 0 makes the C# `while` loop add empty stacks forever.
        let error =
            add_cartridges_to_ammo_box(ctx.items_view, &mut ammo_box, AMMO_BOX_NO_STACK_SIZE_TPL)
                .unwrap_err();

        assert_eq!(error.level, ERROR);
        assert_eq!(ammo_box.len(), 1);
    }

    #[test]
    fn fill_magazine_with_cartridge_leaves_launchers_alone() {
        let view = ammo_fixture();
        let dist = no_ammo_dist();
        let mut ctx = context(&view, &dist);
        let mut magazine = root(UBGL_TPL);

        fill_magazine_with_cartridge(
            ctx.items_view,
            &mut ctx.diagnostics,
            &mut magazine,
            UBGL_TPL,
            CARTRIDGE_A_TPL,
            1.0,
        )
        .unwrap();

        assert_eq!(magazine.len(), 1);
        assert!(ctx.diagnostics.is_empty());
    }

    #[test]
    fn fill_magazine_with_cartridge_stacks_ascend_from_location_zero() {
        let view = ammo_fixture();
        let dist = no_ammo_dist();
        let mut ctx = context(&view, &dist);
        let mut magazine = root(MAGAZINE_TPL);

        // A multiplier of 1 pins the desired count to the magazine's 60, so 30-round stacks fill it.
        fill_magazine_with_cartridge(
            ctx.items_view,
            &mut ctx.diagnostics,
            &mut magazine,
            MAGAZINE_TPL,
            CARTRIDGE_A_TPL,
            1.0,
        )
        .unwrap();

        assert_eq!(magazine.len(), 3);
        assert_eq!(location_of(&magazine[1]), Some(0));
        assert_eq!(location_of(&magazine[2]), Some(1));
        assert_eq!(stack_count_of(&magazine[1]), Some(30.0));
        assert_eq!(stack_count_of(&magazine[2]), Some(30.0));
        assert_eq!(magazine[1].parent_id.as_ref(), Some(&magazine[0].id));
    }

    #[test]
    fn fill_magazine_with_cartridge_drops_the_location_of_a_lone_stack() {
        let view = ammo_fixture();
        let dist = no_ammo_dist();
        let mut ctx = context(&view, &dist);
        let mut magazine = root(MAGAZINE_TPL);

        // A 60-round stack size swallows the whole 60-round magazine in one go.
        fill_magazine_with_cartridge(
            ctx.items_view,
            &mut ctx.diagnostics,
            &mut magazine,
            MAGAZINE_TPL,
            CARTRIDGE_B_TPL,
            1.0,
        )
        .unwrap();

        assert_eq!(magazine.len(), 2);
        assert_eq!(stack_count_of(&magazine[1]), Some(60.0));
        assert!(magazine[1].location.is_none());
    }

    #[test]
    fn fill_magazine_with_cartridge_takes_a_cylinders_capacity_from_its_slot_count() {
        let view = ammo_fixture();
        let dist = no_ammo_dist();
        let mut ctx = context(&view, &dist);
        let mut magazine = root(CYLINDER_TPL);

        fill_magazine_with_cartridge(
            ctx.items_view,
            &mut ctx.diagnostics,
            &mut magazine,
            CYLINDER_TPL,
            CARTRIDGE_B_TPL,
            1.0,
        )
        .unwrap();

        // Six slots, no Cartridges entry at all.
        assert_eq!(magazine.len(), 2);
        assert_eq!(stack_count_of(&magazine[1]), Some(6.0));
    }

    #[test]
    fn fill_magazine_with_cartridge_warns_when_the_magazine_has_no_cartridges_array() {
        let view = ammo_fixture();
        let dist = no_ammo_dist();
        let mut ctx = context(&view, &dist);
        let mut magazine = root(MAGAZINE_NO_CARTRIDGES_TPL);

        fill_magazine_with_cartridge(
            ctx.items_view,
            &mut ctx.diagnostics,
            &mut magazine,
            MAGAZINE_NO_CARTRIDGES_TPL,
            CARTRIDGE_A_TPL,
            1.0,
        )
        .unwrap();

        assert_eq!(magazine.len(), 1);
        assert_eq!(levels(&ctx), vec![WARNING]);
        assert!(messages(&ctx).contains("lacks a Cartridges array"));
    }

    #[test]
    fn fill_magazine_with_cartridge_errors_when_the_cartridge_has_no_stack_max_size() {
        let view = ammo_fixture();
        let dist = no_ammo_dist();
        let mut ctx = context(&view, &dist);
        let mut magazine = root(MAGAZINE_TPL);

        // C# adds `cartridgeCountToAdd!.Value` on a null and crashes.
        let error = fill_magazine_with_cartridge(
            ctx.items_view,
            &mut ctx.diagnostics,
            &mut magazine,
            MAGAZINE_TPL,
            CARTRIDGE_NO_STACK_TPL,
            1.0,
        )
        .unwrap_err();

        assert!(error.message.contains("StackMaxSize"));
        assert_eq!(levels(&ctx), vec![ERROR]);
    }

    #[test]
    fn fill_magazine_with_cartridge_reports_an_unknown_cartridge_tpl_by_locale_key() {
        let view = ammo_fixture();
        let dist = no_ammo_dist();
        let mut ctx = context(&view, &dist);
        let mut magazine = root(MAGAZINE_TPL);

        let unknown = "999999999999999999999999";
        fill_magazine_with_cartridge(
            ctx.items_view,
            &mut ctx.diagnostics,
            &mut magazine,
            MAGAZINE_TPL,
            unknown,
            1.0,
        )
        .unwrap_err();

        assert_eq!(levels(&ctx), vec![ERROR, ERROR]);
        assert_eq!(
            ctx.diagnostics[0].locale_key.as_deref(),
            Some("item-invalid_tpl_item")
        );
        assert_eq!(ctx.diagnostics[0].args, Some(json!(unknown)));
        assert!(ctx.diagnostics[0].message.is_none());
    }

    #[test]
    fn fill_magazine_with_cartridge_draws_between_the_banker_rounded_minimum_and_the_max() {
        let view = ammo_fixture();
        let dist = no_ammo_dist();
        let mut counts = HashSet::new();

        for _ in 0..500 {
            let mut ctx = context(&view, &dist);
            let mut magazine = root(MAGAZINE_SMALL_TPL);

            // 0.5 * 5 = 2.5, which banker's rounding takes to 2 — away-from-zero would say 3.
            fill_magazine_with_cartridge(
                ctx.items_view,
                &mut ctx.diagnostics,
                &mut magazine,
                MAGAZINE_SMALL_TPL,
                CARTRIDGE_B_TPL,
                0.5,
            )
            .unwrap();

            counts.insert(stack_count_of(&magazine[1]).unwrap() as i32);
        }

        assert_eq!(counts, HashSet::from([2, 3, 4, 5]));
    }

    #[test]
    fn fill_magazine_with_cartridge_warns_when_the_magazine_already_has_children() {
        let view = ammo_fixture();
        let dist = no_ammo_dist();
        let mut ctx = context(&view, &dist);
        let mut magazine = root(MAGAZINE_TPL);
        magazine.push(Item {
            id: mongo_id::generate(),
            template: MOD_C_TPL.to_owned(),
            location: Some(json!(7)),
            ..Default::default()
        });

        fill_magazine_with_cartridge(
            ctx.items_view,
            &mut ctx.diagnostics,
            &mut magazine,
            MAGAZINE_TPL,
            CARTRIDGE_B_TPL,
            1.0,
        )
        .unwrap();

        assert_eq!(levels(&ctx), vec![WARNING]);
        assert!(messages(&ctx).contains("already has cartridges defined"));
        // Bug-compatible: the "one stack only" cleanup blanks index 1 whatever it happens to be.
        assert!(magazine[1].location.is_none());
        assert_eq!(magazine[1].template, MOD_C_TPL);
    }

    #[test]
    fn fill_magazine_with_random_cartridge_picks_a_caliber_from_the_magazines_filter() {
        let view = ammo_fixture();
        let dist = ammo_dist(json!({
            CALIBER: [{ "tpl": CARTRIDGE_A_TPL, "relativeProbability": 1 }],
        }));
        let mut ctx = context(&view, &dist);
        let mut magazine = root(MAGAZINE_TPL);

        fill_magazine_with_random_cartridge(
            &mut ctx,
            &mut magazine,
            MAGAZINE_TPL,
            None,
            1.0,
            None,
            None,
        )
        .unwrap();

        assert_eq!(magazine.len(), 3);
        assert!(
            magazine[1..]
                .iter()
                .all(|item| item.template == CARTRIDGE_A_TPL)
        );
        assert!(ctx.diagnostics.is_empty());
    }

    #[test]
    fn fill_magazine_with_random_cartridge_fixes_the_klin_caliber_typo() {
        let view = ammo_fixture();
        let dist = ammo_dist(json!({
            "Caliber9x18PM": [{ "tpl": CARTRIDGE_B_TPL, "relativeProbability": 1 }],
        }));
        let mut ctx = context(&view, &dist);
        let mut magazine = root(MAGAZINE_TPL);

        fill_magazine_with_random_cartridge(
            &mut ctx,
            &mut magazine,
            MAGAZINE_TPL,
            Some("Caliber9x18PMM"),
            1.0,
            None,
            None,
        )
        .unwrap();

        assert_eq!(magazine.len(), 2);
        assert_eq!(magazine[1].template, CARTRIDGE_B_TPL);
    }

    #[test]
    fn fill_magazine_with_random_cartridge_honours_the_weapon_chamber_whitelist() {
        let view = ammo_fixture();
        let dist = ammo_dist(json!({
            CALIBER: [
                { "tpl": CARTRIDGE_A_TPL, "relativeProbability": 100 },
                { "tpl": CARTRIDGE_B_TPL, "relativeProbability": 1 },
            ],
        }));

        // The chamber only admits B, so the far heavier A must never come out.
        for _ in 0..50 {
            let mut ctx = context(&view, &dist);
            let mut magazine = root(MAGAZINE_TPL);

            fill_magazine_with_random_cartridge(
                &mut ctx,
                &mut magazine,
                MAGAZINE_TPL,
                Some(CALIBER),
                1.0,
                None,
                Some(WEAPON_TPL),
            )
            .unwrap();

            assert_eq!(magazine[1].template, CARTRIDGE_B_TPL);
        }
    }

    #[test]
    fn fill_magazine_with_random_cartridge_falls_back_when_the_pool_is_empty() {
        let view = ammo_fixture();
        let dist = no_ammo_dist();
        let mut ctx = context(&view, &dist);
        let mut magazine = root(MAGAZINE_TPL);

        fill_magazine_with_random_cartridge(
            &mut ctx,
            &mut magazine,
            MAGAZINE_TPL,
            Some(CALIBER),
            1.0,
            Some(CARTRIDGE_B_TPL),
            None,
        )
        .unwrap();

        assert_eq!(magazine.len(), 2);
        assert_eq!(magazine[1].template, CARTRIDGE_B_TPL);
        assert_eq!(levels(&ctx), vec![WARNING]);
        assert!(messages(&ctx).contains("using fallback value of"));
    }

    #[test]
    fn fill_magazine_with_random_cartridge_gives_up_without_a_pool_or_a_fallback() {
        let view = ammo_fixture();
        let dist = no_ammo_dist();
        let mut ctx = context(&view, &dist);
        let mut magazine = root(MAGAZINE_TPL);

        fill_magazine_with_random_cartridge(
            &mut ctx,
            &mut magazine,
            MAGAZINE_TPL,
            Some(CALIBER),
            1.0,
            None,
            None,
        )
        .unwrap();

        assert_eq!(magazine.len(), 1);
        assert_eq!(levels(&ctx), vec![WARNING, DEBUG]);
        assert!(messages(&ctx).contains("No fallback value provided"));
        assert!(messages(&ctx).contains("with cartridges, none found."));
    }

    #[test]
    fn fill_magazine_with_random_cartridge_errors_when_no_caliber_resolves() {
        let view = ammo_fixture();
        let dist = no_ammo_dist();
        let mut ctx = context(&view, &dist);

        // No cartridge filter at all — C# throws "Calibers is null".
        let error = fill_magazine_with_random_cartridge(
            &mut ctx,
            &mut root(MAGAZINE_NO_CARTRIDGES_TPL),
            MAGAZINE_NO_CARTRIDGES_TPL,
            None,
            1.0,
            None,
            None,
        )
        .unwrap_err();
        assert!(error.message.contains("Calibers is null"));

        // A filter whose only cartridge has no Caliber draws a null — C# throws "Chosen caliber".
        let error = fill_magazine_with_random_cartridge(
            &mut ctx,
            &mut root(MAGAZINE_NO_CALIBER_TPL),
            MAGAZINE_NO_CALIBER_TPL,
            None,
            1.0,
            None,
            None,
        )
        .unwrap_err();
        assert!(error.message.contains("Chosen caliber is null"));
    }

    #[test]
    fn fill_magazine_with_random_cartridge_skips_ammo_entries_without_a_tpl() {
        let view = ammo_fixture();
        let dist = ammo_dist(json!({
            CALIBER: [
                { "relativeProbability": 100 },
                { "tpl": CARTRIDGE_B_TPL, "relativeProbability": 1 },
            ],
        }));
        let mut ctx = context(&view, &dist);
        let mut magazine = root(MAGAZINE_TPL);

        fill_magazine_with_random_cartridge(
            &mut ctx,
            &mut magazine,
            MAGAZINE_TPL,
            Some(CALIBER),
            1.0,
            None,
            None,
        )
        .unwrap();

        assert_eq!(magazine[1].template, CARTRIDGE_B_TPL);
        assert_eq!(levels(&ctx), vec![ERROR]);
        assert!(messages(&ctx).contains("Ammo details tpl is null"));
    }

    #[test]
    fn fill_magazine_with_random_cartridge_errors_on_an_ammo_entry_without_a_probability() {
        let view = ammo_fixture();
        let dist = ammo_dist(json!({ CALIBER: [{ "tpl": CARTRIDGE_B_TPL }] }));
        let mut ctx = context(&view, &dist);

        // C# casts `RelativeProbability!.Value` and crashes.
        let error = fill_magazine_with_random_cartridge(
            &mut ctx,
            &mut root(MAGAZINE_TPL),
            MAGAZINE_TPL,
            Some(CALIBER),
            1.0,
            None,
            None,
        )
        .unwrap_err();

        assert!(error.message.contains("relativeProbability"));
    }

    #[test]
    fn add_child_slot_items_fills_every_slot_when_no_chance_dict_is_given() {
        let view = ammo_fixture();
        let dist = no_ammo_dist();
        let mut ctx = context(&view, &dist);

        let result = add_child_slot_items(
            &view,
            &mut ctx.diagnostics,
            root(ITEM_WITH_SLOTS_TPL),
            ITEM_WITH_SLOTS_TPL,
            None,
        );

        // Three filled slots; the empty-filter slot is skipped with a debug line.
        assert_eq!(result.len(), 4);
        assert_eq!(
            result[1..]
                .iter()
                .map(|item| item.slot_id.as_deref().unwrap())
                .collect::<Vec<_>>(),
            vec!["mod_required", "Mod_Scope", "mod_other"]
        );
        for child in &result[1..] {
            assert_eq!(child.template, MOD_C_TPL);
            assert_eq!(child.id.len(), 24);
            assert_eq!(child.parent_id.as_ref(), Some(&result[0].id));
        }
        assert_eq!(levels(&ctx), vec![DEBUG]);
        assert!(messages(&ctx).contains("'Filter' array is empty"));
    }

    #[test]
    fn add_child_slot_items_never_rolls_for_required_slots() {
        let view = ammo_fixture();
        let dist = no_ammo_dist();
        let chances = HashMap::from([
            ("mod_required".to_owned(), 0.0),
            ("mod_scope".to_owned(), 0.0),
            ("mod_other".to_owned(), 0.0),
        ]);

        for _ in 0..50 {
            let mut ctx = context(&view, &dist);

            let result = add_child_slot_items(
                &view,
                &mut ctx.diagnostics,
                root(ITEM_WITH_SLOTS_TPL),
                ITEM_WITH_SLOTS_TPL,
                Some(&chances),
            );

            // A 0% chance never fires, so only the required slot survives.
            assert_eq!(result.len(), 2);
            assert_eq!(result[1].slot_id.as_deref(), Some("mod_required"));
        }
    }

    #[test]
    fn add_child_slot_items_only_rolls_for_slots_the_dict_names_lowercased() {
        let view = ammo_fixture();
        let dist = no_ammo_dist();
        // "Mod_Scope" is looked up lowercased; "mod_other" has no entry, so it is never rolled.
        let chances = HashMap::from([("mod_scope".to_owned(), 0.0)]);

        for _ in 0..50 {
            let mut ctx = context(&view, &dist);

            let result = add_child_slot_items(
                &view,
                &mut ctx.diagnostics,
                root(ITEM_WITH_SLOTS_TPL),
                ITEM_WITH_SLOTS_TPL,
                Some(&chances),
            );

            assert_eq!(
                result[1..]
                    .iter()
                    .map(|item| item.slot_id.as_deref().unwrap())
                    .collect::<Vec<_>>(),
                vec!["mod_required", "mod_other"]
            );
        }
    }

    #[test]
    fn add_child_slot_items_excludes_conflicts_of_already_chosen_mods() {
        let view = ammo_fixture();
        let dist = no_ammo_dist();

        for _ in 0..50 {
            let mut ctx = context(&view, &dist);

            let result = add_child_slot_items(
                &view,
                &mut ctx.diagnostics,
                root(ITEM_CONFLICT_TPL),
                ITEM_CONFLICT_TPL,
                None,
            );

            // MOD_A conflicts with MOD_B, so slot two can only ever land on MOD_C.
            assert_eq!(result.len(), 3);
            assert_eq!(result[1].template, MOD_A_TPL);
            assert_eq!(result[2].template, MOD_C_TPL);
        }
    }

    #[test]
    fn add_child_slot_items_skips_a_slot_whose_whole_pool_conflicts() {
        let view = ammo_fixture();
        let dist = no_ammo_dist();
        let mut ctx = context(&view, &dist);

        let result = add_child_slot_items(
            &view,
            &mut ctx.diagnostics,
            root(ITEM_CONFLICT_DEAD_TPL),
            ITEM_CONFLICT_DEAD_TPL,
            None,
        );

        assert_eq!(result.len(), 2);
        assert_eq!(result[1].template, MOD_A_TPL);
        assert_eq!(levels(&ctx), vec![DEBUG]);
        assert!(messages(&ctx).contains("no compatible tpl found in pool of 1"));
    }
}
