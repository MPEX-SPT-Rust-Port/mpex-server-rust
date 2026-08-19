//! Ragfair database views derived natively at publish time (Phase 1 ragfair flip).
//!
//! Bug-for-bug ports of the C# that built the pre-flip `RagfairInvariantSlice` database members
//! (now `RagfairViewsOverride`) — the C# bodies are the authority and every quirk is preserved at
//! its port site:
//!
//! * `RagfairPayloadProjection.BuildViewsOverride` (`Native/Ragfair/RagfairPayloadProjection.cs`)
//!   — the assembly shape and the one-pass items-table loop.
//! * `PayloadProjection.BuildItemsView` / `ToPresetView` (`Native/Loot/PayloadProjection.cs`).
//! * `PresetController.Initialize` (`Controllers/PresetController.cs`) + `PresetHelper`
//!   (`Helpers/Items/PresetHelper.cs`) — the preset cache and the four preset views.
//! * `HandbookHelper.GetTemplatePrice` over a hydrated cache
//!   (`Helpers/Profile/HandbookHelper.cs`).
//! * `TraderHelper.GetHighestSellToTraderPrice` (`Helpers/Traders/TraderHelper.cs`), cold-cache.
//!
//! `HandbookHelper.HydrateHandbookCache` also applies `ItemConfig.HandbookPriceOverride` — but it
//! applies it *into* `templateTable.Handbook` itself (`HandbookHelper.cs:30-49`), so a templates
//! root serialized after any C# price lookup already carries the overrides and this port reads
//! the root alone. The publish sequencing that guarantees that is surfaced in the task report.

use std::collections::HashMap;

use indexmap::IndexMap;

use crate::db::models::{
    GlobalsRoot, Grid, HandbookBase, Preset, Slot, TemplateItem, TemplatesRoot, TraderEntry,
    TradersRoot,
};
use crate::loot::item_helper::{ARMOR, HEADWEAR, ItemBaseClassCache, VEST, WEAPON};
use crate::loot::models::{GridFilterView, GridView, ItemView, PresetView, SlotView};

/// The eight database views a dynamic ragfair pass consults ([`crate::ragfair::models::RagfairViewsWire`]
/// on the wire) plus the prepared base-class cache.
#[derive(Debug)]
pub struct RagfairDbViews {
    /// `PayloadProjection.BuildItemsView` over the whole items table.
    pub items: IndexMap<String, ItemView>,
    /// `TemplateTable.Prices` verbatim, source order (`RagfairPayloadProjection.cs:70`).
    pub flea_prices: IndexMap<String, f64>,
    /// `HandbookHelper.GetTemplatePrice` for every items-table key, in items-table order.
    pub handbook_prices: IndexMap<String, f64>,
    /// `TraderHelper.GetHighestSellToTraderPrice`, same loop.
    pub highest_trader_prices: IndexMap<String, f64>,
    /// `GlobalTable.ItemPresets` projected — the globals map's key domain, not each preset's
    /// `_id` (`RagfairPayloadProjection.cs:59-63`).
    pub item_presets: IndexMap<String, PresetView>,
    /// `PresetHelper.GetDefaultPresets().Values` — weapon defaults then equipment defaults.
    pub default_presets: Vec<PresetView>,
    /// `PresetHelper.GetDefaultPresetByTpl()`.
    pub default_presets_by_tpl: IndexMap<String, PresetView>,
    /// `PresetHelper.GetDefaultPresetsByTplKey()` (`PresetHelper.cs:42-52`) — the loot flip's
    /// forced-loot map. Weapon then equipment default *values*, skipping items-less presets,
    /// keyed by each preset's first item's tpl.
    pub default_presets_by_tpl_key: IndexMap<String, PresetView>,
    /// `PresetHelper.GetPresets(tpl)` for every tpl with presets, in items-table order.
    pub presets_by_tpl: IndexMap<String, Vec<PresetView>>,
    /// [`ItemBaseClassCache::build`] over [`Self::items`], the same call
    /// `From<RagfairViewsWire>` makes for an override.
    pub base_classes: ItemBaseClassCache,
}

/// Derive every ragfair view off the three resident roots. Total over empty roots; the only
/// error is the one hard failure the C# has — a preset with no items, which is a
/// `NullReferenceException` in `PresetController.Initialize` (`PresetController.cs:33-34`).
pub fn derive(
    templates: &TemplatesRoot,
    traders: &TradersRoot,
    globals: &GlobalsRoot,
) -> Result<RagfairDbViews, String> {
    let items = build_items_view(&templates.items);
    // The same call `From<RagfairViewsWire>` makes for an override; also what the preset
    // classification below answers `ItemHelper.IsOfBaseclass(es)` from, standing in for
    // `ItemBaseClassService` (whose answers agree for every real item tpl — see item_helper.rs).
    let base_classes = ItemBaseClassCache::build(&items);

    let preset_cache = build_preset_cache(globals)?;

    // PresetHelper.GetDefaultWeaponPresets / GetDefaultEquipmentPresets (PresetHelper.cs:94-122):
    // the globals map filtered on encyclopedia presence + base class, keyed by the *map key* —
    // an entry whose key mismatches its `_id` is still eligible here even though the preset
    // cache skipped it.
    let weapon_defaults: IndexMap<&String, &Preset> = globals
        .item_presets
        .iter()
        .filter(|(_, preset)| {
            preset
                .encyclopedia
                .as_deref()
                .is_some_and(|encyclopedia| base_classes.is_of_baseclass(encyclopedia, WEAPON))
        })
        .collect();
    // ItemHelper.ArmorItemCanHoldMods (`ItemHelper.cs:329-332`) — `_armorSlotsThatCanHoldMods`.
    let equipment_defaults: IndexMap<&String, &Preset> = globals
        .item_presets
        .iter()
        .filter(|(_, preset)| {
            preset.encyclopedia.as_deref().is_some_and(|encyclopedia| {
                base_classes.is_of_baseclasses(encyclopedia, &[HEADWEAR, VEST, ARMOR])
            })
        })
        .collect();

    // PresetHelper.GetDefaultPresets (PresetHelper.cs:30-36): weapons.UnionBy(equipment, key) —
    // every weapon default in map order, then the equipment defaults whose key is new.
    let mut default_presets: Vec<PresetView> = weapon_defaults
        .values()
        .map(|preset| to_preset_view(preset))
        .collect();
    default_presets.extend(
        equipment_defaults
            .iter()
            .filter(|(key, _)| !weapon_defaults.contains_key(*key))
            .map(|(_, preset)| to_preset_view(preset)),
    );

    // PresetHelper.GetDefaultPresetByTpl (PresetHelper.cs:60-88): per cached tpl with a default
    // id, the weapon or equipment default under that id, else the first preset in the tpl's list.
    let mut default_presets_by_tpl = IndexMap::new();
    for (template_id, details) in &preset_cache {
        let Some(default_id) = &details.default_id else {
            continue;
        };
        let default_preset = weapon_defaults
            .get(default_id)
            .or_else(|| equipment_defaults.get(default_id))
            .copied()
            .unwrap_or_else(|| &globals.item_presets[details.preset_ids[0].as_str()]);
        default_presets_by_tpl.insert(template_id.clone(), to_preset_view(default_preset));
    }

    // PresetHelper.GetDefaultPresetsByTplKey (PresetHelper.cs:42-52): C#'s ToDictionary throws
    // on a duplicate first-item tpl at every forced-loot call; here it aborts the publish
    // naming the culprit instead (spec-sanctioned strictness — the lazy crash becomes loud).
    let mut default_presets_by_tpl_key = IndexMap::new();
    for preset in weapon_defaults.values().chain(equipment_defaults.values()) {
        let Some(tpl) = preset.items.first().map(|item| item.template.clone()) else {
            continue; // .Where(preset => preset.Items.Count > 0)
        };
        if default_presets_by_tpl_key
            .insert(tpl.clone(), to_preset_view(preset))
            .is_some()
        {
            return Err(format!(
                "two default presets share first-item tpl '{tpl}' — C# GetDefaultPresetsByTplKey throws ArgumentException here"
            ));
        }
    }

    // One pass over the items table (RagfairPayloadProjection.cs:43-54): the pricing math
    // reaches arbitrary tpls through barter schemes and preset children, so both price maps
    // cover the whole table rather than a pool — and presets group under the same keys.
    let handbook_by_id = build_handbook_price_map(&templates.handbook);
    let mut handbook_prices = IndexMap::with_capacity(templates.items.len());
    let mut highest_trader_prices = IndexMap::with_capacity(templates.items.len());
    let mut presets_by_tpl: IndexMap<String, Vec<PresetView>> = IndexMap::new();
    for tpl in templates.items.keys() {
        // HandbookHelper.GetTemplatePrice (HandbookHelper.cs:106-134): the hydrated cache covers
        // every handbook id, so a miss means "not in the handbook" and prices at 0.
        let handbook_price = handbook_by_id.get(tpl.as_str()).copied().unwrap_or(0.0);
        handbook_prices.insert(tpl.clone(), handbook_price);
        highest_trader_prices.insert(
            tpl.clone(),
            highest_sell_to_trader_price(traders, handbook_price),
        );
        // PresetHelper.HasPreset + GetPresets (PresetHelper.cs:155-158, 200-211).
        if let Some(details) = preset_cache.get(tpl) {
            presets_by_tpl.insert(
                tpl.clone(),
                details
                    .preset_ids
                    .iter()
                    .map(|preset_id| to_preset_view(&globals.item_presets[preset_id.as_str()]))
                    .collect(),
            );
        }
    }

    // The globals map itself, keys included (RagfairPayloadProjection.cs:59-63): the key domain
    // is that map's keys, not each preset's own `_id`.
    let item_presets = globals
        .item_presets
        .iter()
        .map(|(preset_id, preset)| (preset_id.clone(), to_preset_view(preset)))
        .collect();

    Ok(RagfairDbViews {
        items,
        // The whole flea price table, in source order (RagfairPayloadProjection.cs:69-70).
        flea_prices: templates.prices.clone(),
        handbook_prices,
        highest_trader_prices,
        item_presets,
        default_presets,
        default_presets_by_tpl,
        default_presets_by_tpl_key,
        presets_by_tpl,
        base_classes,
    })
}

/// `PresetCacheDetails` (`Models/Spt/Presets/PresetCacheDetails.cs`).
struct PresetCacheDetails {
    preset_ids: Vec<String>,
    default_id: Option<String>,
}

/// `PresetController.Initialize` (`PresetController.cs:17-46`): tpl → preset ids off the globals
/// map, keyed by each preset's root item tpl. Entries whose map key mismatches their `_id` are
/// logged and skipped in C#; skipped here. The default id is overwritten by every preset that
/// carries an `_encyclopedia`, so the last one in map order wins.
fn build_preset_cache(
    globals: &GlobalsRoot,
) -> Result<IndexMap<String, PresetCacheDetails>, String> {
    let mut result: IndexMap<String, PresetCacheDetails> = IndexMap::new();

    for (preset_id, preset) in &globals.item_presets {
        if *preset_id != preset.id {
            continue;
        }

        // `preset.Items.FirstOrDefault()?.Template` then `.Value` — an items-less preset is a
        // hard NullReferenceException in C# (PresetController.cs:33-34); it aborts the publish
        // here instead of the process.
        let Some(tpl) = preset.items.first().map(|item| item.template.as_str()) else {
            return Err(format!(
                "preset '{preset_id}' has no items — C# PresetController.Initialize throws here"
            ));
        };

        let details = result
            .entry(tpl.to_owned())
            .or_insert_with(|| PresetCacheDetails {
                preset_ids: Vec::new(),
                default_id: None,
            });
        details.preset_ids.push(preset_id.clone());
        if preset.encyclopedia.is_some() {
            details.default_id = Some(preset.id.clone());
        }
    }

    Ok(result)
}

/// The items side of `HandbookHelper.HydrateHandbookCache` (`HandbookHelper.cs:51-68`): first
/// occurrence of a duplicate id wins (`TryAdd`), a null price caches as 0. `pub(crate)`: the
/// quest derive answers `HandbookHelper.GetTemplatePrice` from the same cache
/// (`crate::quest::views`).
pub(crate) fn build_handbook_price_map(handbook: &HandbookBase) -> HashMap<&str, f64> {
    let mut by_id = HashMap::with_capacity(handbook.items.len());
    for item in &handbook.items {
        by_id
            .entry(item.id.as_str())
            .or_insert(item.price.unwrap_or(0.0));
    }
    by_id
}

/// `TraderHelper.GetHighestSellToTraderPrice` (`TraderHelper.cs:516-547`) on a cold cache — the
/// first-build pass the equivalence fixture also takes.
fn highest_sell_to_trader_price(traders: &TradersRoot, item_handbook_price: f64) -> f64 {
    // C# default price
    let mut highest = 1.0f64;

    for entry in traders.traders.values() {
        // A value the C# TradersTable could never have deserialized; skip (see TraderEntry).
        let TraderEntry::Trader(trader) = entry else {
            continue;
        };

        // `100 - traderBase.LoyaltyLevels?.FirstOrDefault()?.BuyPriceCoefficient` — nullable
        // arithmetic: a missing level or coefficient nulls the whole percent and `?? 0` then
        // zeroes it. (A missing `base` is a C# NRE that can never load; treated as null too.)
        let coefficient = trader
            .base
            .as_ref()
            .and_then(|base| base.loyalty_levels.as_ref())
            .and_then(|levels| levels.first())
            .and_then(|level| level.buy_price_coef);
        let percent = coefficient.map_or(0.0, |coefficient| 100.0 - coefficient);

        // RandomUtil.GetPercentOfValue(percent, price, 0) (`RandomUtil.cs:104-109`):
        // `Math.Round(percent * (number / 100), 0)` — banker's rounding, division first.
        let price = (percent * (item_handbook_price / 100.0)).round_ties_even();
        if price > highest {
            highest = price;
        }
    }

    highest
}

/// `PayloadProjection.BuildItemsView` (`PayloadProjection.cs:18-130`). Templates without props
/// are dropped — their absence is how the native side says "lacks _props".
fn build_items_view(items: &IndexMap<String, TemplateItem>) -> IndexMap<String, ItemView> {
    let mut items_view = IndexMap::with_capacity(items.len());

    for (tpl, template) in items {
        let Some(props) = &template.properties else {
            continue;
        };

        let first_grid = props.grids.as_ref().and_then(|grids| grids.first());
        let first_stack_slot = props.stack_slots.as_ref().and_then(|slots| slots.first());
        let first_cartridge_slot = props.cartridges.as_ref().and_then(|slots| slots.first());
        let first_chamber = props.chambers.as_ref().and_then(|slots| slots.first());
        let stack_slot_filter = first_stack_slot
            .and_then(|slot| slot.properties.as_ref())
            .and_then(|slot_props| slot_props.filters.as_ref())
            .and_then(|filters| filters.first())
            .and_then(|filter| filter.filter.as_ref());

        items_view.insert(
            tpl.clone(),
            ItemView {
                // The MongoId cast quirk (PayloadProjection.cs:38-40): an empty `_parent` is an
                // absent member, not an empty id.
                parent: (!template.parent.is_empty()).then(|| template.parent.clone()),
                width: props.width,
                height: props.height,
                stack_max_size: props.stack_max_size,
                stack_min_random: props.stack_min_random,
                stack_max_random: props.stack_max_random,
                // The five extra-size members are read only through `unwrap_or(0)`/
                // `unwrap_or(false)` natively, so the default value is dropped
                // (PayloadProjection.cs:46-54).
                extra_size_up: null_if_zero(props.extra_size_up),
                extra_size_down: null_if_zero(props.extra_size_down),
                extra_size_left: null_if_zero(props.extra_size_left),
                extra_size_right: null_if_zero(props.extra_size_right),
                extra_size_force_add: null_if_false(props.extra_size_force_add),
                grid_cells_h: first_grid
                    .and_then(|grid| grid.properties.as_ref())
                    .and_then(|grid_props| grid_props.cells_h),
                grid_cells_v: first_grid
                    .and_then(|grid| grid.properties.as_ref())
                    .and_then(|grid_props| grid_props.cells_v),
                stack_slot_max_count: first_stack_slot.and_then(|slot| slot.max_count),
                // Deliberate divergence preserved (PayloadProjection.cs:58-61): an empty filter
                // set is null, not the empty MongoId `FirstOrDefault()` would have produced.
                stack_slot_first_filter_first: stack_slot_filter
                    .and_then(|filter| filter.first().cloned()),
                cartridges_max_count: first_cartridge_slot.and_then(|slot| slot.max_count),
                cartridges_first_filter: first_cartridge_slot
                    .and_then(|slot| slot.properties.as_ref())
                    .and_then(|slot_props| slot_props.filters.as_ref())
                    .and_then(|filters| filters.first())
                    .and_then(|filter| filter.filter.as_ref())
                    .map(|filter| filter.iter().cloned().collect()),
                chambers_first_filter: first_chamber
                    .and_then(|slot| slot.properties.as_ref())
                    .and_then(|slot_props| slot_props.filters.as_ref())
                    .and_then(|filters| filters.first())
                    .and_then(|filter| filter.filter.as_ref())
                    .map(|filter| filter.iter().cloned().collect()),
                slots: to_slot_views(props.slots.as_ref()),
                // Projected verbatim — an empty chamber list is not the same as no chamber list.
                chambers: to_slot_views(props.chambers.as_ref()),
                cartridges: to_slot_views(props.cartridges.as_ref()),
                conflicting_items: props
                    .conflicting_items
                    .as_ref()
                    .map(|conflicting| conflicting.iter().cloned().collect()),
                caliber: props.caliber.clone(),
                ammo_caliber: props.ammo_caliber.clone(),
                def_ammo: props.def_ammo.clone(),
                name: template.name.clone(),
                item_type: template.item_type.clone(),
                armor_class: props.armor_class,
                // Not coalesced: `false` and `null` stay distinguishable (PayloadProjection.cs:76-78).
                quest_item: props.quest_item,
                // Enum member names, not numeric values — normalized at root parse time.
                reload_mode: props.reload_mode.clone(),
                reload_mag_type: props.reload_mag_type.clone(),
                is_chamber_load: props.is_chamber_load,
                def_mag_type: props.def_mag_type.clone(),
                linked_weapon: props.linked_weapon.clone(),
                max_durability: props.max_durability,
                weap_class: props.weap_class.clone(),
                has_hinge: null_if_false(props.has_hinge),
                foldable: props.foldable,
                folded_slot: props.folded_slot.clone(),
                size_reduce_right: null_if_zero(props.size_reduce_right),
                weap_fire_type: props
                    .weap_fire_type
                    .as_ref()
                    .map(|fire_types| fire_types.iter().cloned().collect()),
                max_hp_resource: props.max_hp_resource,
                max_resource: props.max_resource,
                food_use_time: props.food_use_time,
                // The blocking family reads through `unwrap_or(false)` natively, so `false` and
                // absent are indistinguishable there (PayloadProjection.cs:95-106).
                face_shield_component: null_if_false(props.face_shield_component),
                blocks_earpiece: null_if_false(props.blocks_earpiece),
                blocks_eyewear: null_if_false(props.blocks_eyewear),
                blocks_face_cover: null_if_false(props.blocks_face_cover),
                blocks_headwear: null_if_false(props.blocks_headwear),
                blocks_folding: null_if_false(props.blocks_folding),
                blocks_collapsible: null_if_false(props.blocks_collapsible),
                block_left_stance: null_if_false(props.block_left_stance),
                blocks_armor_vest: null_if_false(props.blocks_armor_vest),
                grids: props
                    .grids
                    .as_ref()
                    .map(|grids| grids.iter().map(to_grid_view).collect()),
                durability: props.durability,
                maximum_number_of_usage: props.maximum_number_of_usage,
                max_repair_resource: props.max_repair_resource,
                can_sell_on_ragfair: props.can_sell_on_ragfair,
            },
        );
    }

    items_view
}

/// `PayloadProjection.NullIfZero` (`PayloadProjection.cs:139-142`).
fn null_if_zero(value: Option<i32>) -> Option<i32> {
    value.filter(|&value| value != 0)
}

/// `PayloadProjection.NullIfFalse` (`PayloadProjection.cs:145-148`).
fn null_if_false(value: Option<bool>) -> Option<bool> {
    value.filter(|&value| value)
}

/// `PayloadProjection.ToSlotViews` (`PayloadProjection.cs:150-161`).
fn to_slot_views(slots: Option<&Vec<Slot>>) -> Option<Vec<SlotView>> {
    slots.map(|slots| {
        slots
            .iter()
            .map(|slot| {
                let first_filter = slot
                    .properties
                    .as_ref()
                    .and_then(|slot_props| slot_props.filters.as_ref())
                    .and_then(|filters| filters.first());
                SlotView {
                    name: slot.name.clone(),
                    required: null_if_false(slot.required),
                    filter: first_filter
                        .and_then(|filter| filter.filter.as_ref())
                        .map(|filter| filter.iter().cloned().collect()),
                    plate: first_filter.and_then(|filter| filter.plate.clone()),
                }
            })
            .collect()
    })
}

/// The `Grids` arm of `BuildItemsView` (`PayloadProjection.cs:107-121`).
fn to_grid_view(grid: &Grid) -> GridView {
    GridView {
        name: grid.name.clone(),
        cells_h: grid
            .properties
            .as_ref()
            .and_then(|grid_props| grid_props.cells_h),
        cells_v: grid
            .properties
            .as_ref()
            .and_then(|grid_props| grid_props.cells_v),
        filters: grid
            .properties
            .as_ref()
            .and_then(|grid_props| grid_props.filters.as_ref())
            .map(|filters| {
                filters
                    .iter()
                    .map(|filter| GridFilterView {
                        filter: filter
                            .filter
                            .as_ref()
                            .map(|tpls| tpls.iter().cloned().collect()),
                        excluded_filter: filter
                            .excluded_filter
                            .as_ref()
                            .map(|tpls| tpls.iter().cloned().collect()),
                    })
                    .collect()
            }),
    }
}

/// `PayloadProjection.ToPresetView` (`PayloadProjection.cs:167-176`). `pub(crate)`: the same
/// projection the quest slice's `DefaultWeaponPresets` crossed with (`crate::quest::views`).
pub(crate) fn to_preset_view(preset: &Preset) -> PresetView {
    PresetView {
        items: preset.items.clone(),
        // `Preset.Id` is a non-nullable MongoId in C#, so the view member is always present.
        id: Some(preset.id.clone()),
        name: preset.name.clone(),
        encyclopedia: preset.encyclopedia.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `BaseClasses.WEAPON` — a node id real weapon templates parent to.
    const WEAPON_NODE: &str = "5422acb9af1c889c16000029";

    fn templates() -> TemplatesRoot {
        serde_json::from_str(
            r#"{
            "items": {
                "5422acb9af1c889c16000029": {"_name":"weapon","_type":"Node","_parent":"","_props":{}},
                "weapon1": {"_name":"ak","_type":"Item","_parent":"5422acb9af1c889c16000029","_props":{
                    "Width":2,"Height":1,"StackMaxSize":1,"ExtraSizeUp":0,"ExtraSizeForceAdd":false,
                    "ReloadMode":0,
                    "Slots":[{"_name":"mod_magazine","_required":false,
                              "_props":{"filters":[{"Filter":["mod1"]}]}}]
                }},
                "mod1": {"_name":"mag","_type":"Item","_parent":"5448bc234bdc2d3c308b4569",
                         "_props":{"Width":1,"Height":1}},
                "noprops": {"_name":"broken","_type":"Item","_parent":""}
            },
            "handbook": {"Items":[
                {"Id":"weapon1","ParentId":"cat","Price":20000},
                {"Id":"weapon1","ParentId":"cat","Price":99999},
                {"Id":"mod1","ParentId":"cat","Price":null}
            ]},
            "prices": {"mod1":100.0,"weapon1":25000.0}
        }"#,
        )
        .expect("templates fixture parses")
    }

    fn traders() -> TradersRoot {
        serde_json::from_str(
            r#"{
            "trader1": {"base":{"loyaltyLevels":[{"buy_price_coef":30.0},{"buy_price_coef":1.0}]}},
            "trader2": {"base":{"loyaltyLevels":[{"buy_price_coef":49.5}]}},
            "junk": 2
        }"#,
        )
        .expect("traders fixture parses")
    }

    fn globals() -> GlobalsRoot {
        serde_json::from_str(
            r#"{
            "ItemPresets": {
                "preset2": {"_id":"preset2","_name":"ak-mod","_items":[
                    {"_id":"root2","_tpl":"weapon1"},
                    {"_id":"child2","_tpl":"mod1","parentId":"root2","slotId":"mod_magazine"}]},
                "preset1": {"_id":"preset1","_name":"ak-default",
                    "_items":[{"_id":"root1","_tpl":"weapon1"}],"_encyclopedia":"weapon1"},
                "preset3": {"_id":"presetX","_name":"key-mismatch",
                    "_items":[{"_id":"root3","_tpl":"root_tpl"}],"_encyclopedia":"weapon1"},
                "presetM": {"_id":"presetM","_name":"mag-preset",
                    "_items":[{"_id":"rootM","_tpl":"mod1"}],"_encyclopedia":"mod1"}
            }
        }"#,
        )
        .expect("globals fixture parses")
    }

    fn views() -> RagfairDbViews {
        derive(&templates(), &traders(), &globals()).expect("derive succeeds")
    }

    #[test]
    fn flea_prices_preserve_source_order_and_values() {
        let views = views();
        assert_eq!(
            views.flea_prices.iter().collect::<Vec<_>>(),
            vec![
                (&"mod1".to_owned(), &100.0),
                (&"weapon1".to_owned(), &25000.0)
            ]
        );
    }

    #[test]
    fn items_view_covers_every_template_with_props_in_table_order() {
        let views = views();
        assert_eq!(
            views.items.keys().collect::<Vec<_>>(),
            vec![WEAPON_NODE, "weapon1", "mod1"],
            "noprops has no _props and must be dropped"
        );

        let weapon = &views.items["weapon1"];
        assert_eq!(weapon.parent.as_deref(), Some(WEAPON_NODE));
        assert_eq!(weapon.width, Some(2));
        // NullIfZero / NullIfFalse (PayloadProjection.cs:50-54)
        assert_eq!(weapon.extra_size_up, None);
        assert_eq!(weapon.extra_size_force_add, None);
        // EftEnumConverter writes the enum numerically; the view carries the member name
        assert_eq!(weapon.reload_mode.as_deref(), Some("ExternalMagazine"));
        let slots = weapon.slots.as_ref().expect("weapon has slots");
        assert_eq!(slots[0].name.as_deref(), Some("mod_magazine"));
        assert_eq!(slots[0].filter.as_deref(), Some(&["mod1".to_owned()][..]));
        // `_required: false` is dropped, not sent as false (NullIfFalse via ToSlotViews)
        assert_eq!(slots[0].required, None);

        // An empty `_parent` becomes an absent member (the MongoId cast quirk)
        assert_eq!(views.items[WEAPON_NODE].parent, None);
    }

    #[test]
    fn presets_group_under_the_globals_key_domain_with_defaults_split_out() {
        let views = views();

        // itemPresets is the globals map itself: every entry, map order, keyed by the MAP key —
        // preset3's view still carries its own `_id` (presetX)
        assert_eq!(
            views.item_presets.keys().collect::<Vec<_>>(),
            vec!["preset2", "preset1", "preset3", "presetM"]
        );
        assert_eq!(views.item_presets["preset3"].id.as_deref(), Some("presetX"));

        // The preset cache skipped preset3 (map key != _id, PresetController.cs:23-30), so it
        // groups nowhere — but GetDefaultWeaponPresets never applies that filter, so it still
        // reaches defaultPresets
        assert_eq!(
            views
                .default_presets
                .iter()
                .map(|preset| preset.id.as_deref().unwrap())
                .collect::<Vec<_>>(),
            vec!["preset1", "presetX"]
        );

        // defaultPresetsByTpl: preset1 resolves through the weapon-defaults map; presetM's
        // encyclopedia is no weapon, so it resolves through the first-preset-in-list fallback
        // (PresetHelper.cs:75-84)
        assert_eq!(
            views
                .default_presets_by_tpl
                .iter()
                .map(|(tpl, preset)| (tpl.as_str(), preset.id.as_deref().unwrap()))
                .collect::<Vec<_>>(),
            vec![("weapon1", "preset1"), ("mod1", "presetM")]
        );

        // presetsByTpl walks the items table in order; preset ids keep cache order
        assert_eq!(
            views
                .presets_by_tpl
                .iter()
                .map(|(tpl, presets)| (
                    tpl.as_str(),
                    presets
                        .iter()
                        .map(|preset| preset.id.as_deref().unwrap())
                        .collect::<Vec<_>>()
                ))
                .collect::<Vec<_>>(),
            vec![
                ("weapon1", vec!["preset2", "preset1"]),
                ("mod1", vec!["presetM"])
            ]
        );
    }

    #[test]
    fn default_presets_by_tpl_key_keys_defaults_by_first_item_tpl() {
        // One weapon default whose first item's tpl is "root_tpl"; the map keys by that tpl,
        // not by preset id (PresetHelper.GetDefaultPresetsByTplKey, PresetHelper.cs:42-52).
        let views = derive(&templates(), &TradersRoot::default(), &globals()).unwrap();
        let preset = &views.default_presets_by_tpl_key["root_tpl"];
        assert!(!preset.items.is_empty());
    }

    #[test]
    fn duplicate_default_first_item_tpl_aborts_the_derive_naming_the_tpl() {
        // Two weapon defaults whose first items share a tpl — C#'s ToDictionary throws at
        // every forced-loot call (PresetHelper.cs:42-52); the derive aborts the publish
        // loudly instead, naming the culprit tpl.
        let globals: GlobalsRoot = serde_json::from_str(
            r#"{
            "ItemPresets": {
                "preset1": {"_id":"preset1","_name":"ak-default",
                    "_items":[{"_id":"root1","_tpl":"weapon1"}],"_encyclopedia":"weapon1"},
                "presetDup": {"_id":"presetDup","_name":"ak-clone",
                    "_items":[{"_id":"rootD","_tpl":"weapon1"}],"_encyclopedia":"weapon1"}
            }
        }"#,
        )
        .expect("globals fixture parses");

        let error = derive(&templates(), &TradersRoot::default(), &globals)
            .expect_err("duplicate first-item tpl must abort the derive");
        assert!(
            error.contains("'weapon1'"),
            "error names the culprit tpl: {error}"
        );
    }

    #[test]
    fn handbook_and_trader_prices_cover_the_whole_items_table_in_order() {
        let views = views();

        // Every items-table key, table order — noprops included: the C# loop is over
        // templateItems.Keys, not the props-filtered view (RagfairPayloadProjection.cs:45)
        assert_eq!(
            views.handbook_prices.iter().collect::<Vec<_>>(),
            vec![
                (&WEAPON_NODE.to_owned(), &0.0),   // not in handbook
                (&"weapon1".to_owned(), &20000.0), // duplicate id: first entry wins (TryAdd)
                (&"mod1".to_owned(), &0.0),        // null price caches as 0
                (&"noprops".to_owned(), &0.0),
            ]
        );

        // trader1: (100-30)% of 20000 = 14000; trader2: 50.5% = 10100; junk value skipped
        assert_eq!(views.highest_trader_prices["weapon1"], 14000.0);
        // handbook price 0 never beats the default of 1
        assert_eq!(views.highest_trader_prices["mod1"], 1.0);
    }

    #[test]
    fn trader_price_rounds_ties_to_even_like_math_round() {
        // 50% of 5 = 2.5 — C# Math.Round(2.5, 0) is 2, not 3
        let templates: TemplatesRoot = serde_json::from_str(
            r#"{"items":{"coin":{"_parent":"","_props":{}}},
                "handbook":{"Items":[{"Id":"coin","Price":5}]},"prices":{}}"#,
        )
        .unwrap();
        let traders: TradersRoot =
            serde_json::from_str(r#"{"t":{"base":{"loyaltyLevels":[{"buy_price_coef":50.0}]}}}"#)
                .unwrap();
        let views = derive(&templates, &traders, &GlobalsRoot::default()).unwrap();
        assert_eq!(views.highest_trader_prices["coin"], 2.0);
    }

    #[test]
    fn derive_is_total_over_empty_roots() {
        let views = derive(
            &TemplatesRoot::default(),
            &TradersRoot::default(),
            &GlobalsRoot::default(),
        )
        .expect("empty roots derive");
        assert!(views.items.is_empty());
        assert!(views.item_presets.is_empty());
    }

    #[test]
    fn an_items_less_preset_is_a_derivation_error() {
        // The C# NullReferenceException in PresetController.Initialize, surfaced as Err
        let globals: GlobalsRoot =
            serde_json::from_str(r#"{"ItemPresets":{"bad":{"_id":"bad","_items":[]}}}"#).unwrap();
        let error = derive(&TemplatesRoot::default(), &TradersRoot::default(), &globals)
            .expect_err("items-less preset errors");
        assert!(error.contains("bad"), "error names the preset: {error}");
    }
}
