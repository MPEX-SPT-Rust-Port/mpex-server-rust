//! `Services/Ragfair/RagfairPriceService.cs` — the pricing math one dynamic offer needs.
//!
//! **The draw checklist.** One `get_dynamic_item_price` run consumes **exactly one** draw, and it
//! is always consumed:
//!
//! 1. [`get_flea_price_for_item`] — map lookups, no draw.
//! 2. [`adjust_price_if_below_handbook`] — arithmetic, no draw.
//! 3. the trader-price arm — a map lookup, no draw.
//! 4. the weapon-preset arm ([`get_preset_price_by_children`] / [`get_weapon_preset_price`]) —
//!    more of the same lookups, no draw.
//! 5. the `itemPriceMultiplier`, quality-modifier and `unreasonableModPrices` stages — no draw.
//! 6. [`randomise_offer_price`] — **the one draw**, a `get_biased_random_number` over the range
//!    [`get_offer_type_range_values`] picked. A range whose `min == max` returns without touching
//!    the stream (the C# `GetBiasedRandomNumber` guard), which is what the tests below pin the
//!    stage arithmetic with; any other range costs `2 * attempts` `next_double48` draws.
//! 7. the currency conversion and the `<= 0` floor — no draw.
//!
//! So [`get_dynamic_offer_price_for_offer`] draws once per priced item, and the weapon-preset break
//! at `:275-281` means a preset offer draws once in total, not once per mod.
//!
//! Two shape divergences from the C#, both forced:
//! - the functions that push diagnostics take `&mut RagfairContext`, per the `BotContext`
//!   borrow discipline;
//! - the two unguarded C# null dereferences on this path (`GetWeaponPreset`'s `nonDefaultPresets[0]`
//!   and `IsPresetBaseClass`'s `Encyclopedia!.Value`) surface as [`LootError`], so everything
//!   downstream of them returns `Result` where the C# returns a bare `double`.

use super::models::{MinMaxDoubleWire, UnreasonableModPricesWire};
use super::{RagfairContext, plain};
use crate::loot::item_helper::{
    BUILT_IN_INSERTS, LootError, WEAPON, get_item_quality_modifier, is_of_baseclass,
};
use crate::loot::models::{DEBUG, Item, PresetView};
use crate::loot::random_util::{get_biased_random_number, round_half_even};

/// `Models/Enums/Money.cs` — the four currency tpls, kept here because this is the module that
/// already converts between them.
pub(crate) const ROUBLES: &str = "5449016a4bdc2d6f028b456f";
pub(crate) const EUROS: &str = "569668774bdc2da2298b4568";
pub(crate) const DOLLARS: &str = "5696686a4bdc2da3298b456a";
pub(crate) const GP: &str = "5d235b4d86f7742e017bc88a";

/// `RagfairPriceService.GetFleaPriceForItem` (`:171-193`).
///
/// The C#'s warning arm (`:175-184`, the localisation key
/// `ragfair-unable_to_find_item_price_for_item_in_flea_handbook`) is **dead code and stays dead**:
/// its trigger is `itemPrice is null`, but the coalesce that produces `itemPrice` ends in
/// `GetStaticPriceForItem` → `HandbookHelper.GetTemplatePrice`, a non-nullable `double` that
/// answers `0` for a tpl it has never seen (`HandbookHelper.cs:106-134`). No SPT server can emit
/// that line, so neither does this port. A tpl in neither table takes the same route the C# does:
/// `0` from the handbook, then the `0 -> 1` floor at `:186-190`.
pub fn get_flea_price_for_item(ctx: &RagfairContext, tpl: &str) -> f64 {
    // Get dynamic price (templates/prices), if that doesn't exist get price from static array
    // (templates/handbook)
    let mut item_price = match ctx.flea_prices.get(tpl).copied() {
        Some(price) => price,
        None => get_static_price_for_item(ctx, tpl),
    };

    // If no price in dynamic/static, set to 1. The C# compares to zero exactly, so this does too.
    if item_price == 0.0 {
        item_price = 1.0;
    }

    item_price
}

/// `RagfairPriceService.GetStaticPriceForItem` (`:212-215`), i.e. `HandbookHelper.GetTemplatePrice`
/// (`HandbookHelper.cs:106-134`) — never null, `0` for a tpl with no handbook entry.
pub fn get_static_price_for_item(ctx: &RagfairContext, tpl: &str) -> f64 {
    ctx.handbook_prices.get(tpl).copied().unwrap_or(0.0)
}

/// `RagfairPriceService.GetDynamicOfferPriceForOffer` (`:258-285`).
pub fn get_dynamic_offer_price_for_offer(
    ctx: &mut RagfairContext,
    offer_items: &[Item],
    desired_currency: &str,
    is_pack_offer: bool,
) -> Result<f64, LootError> {
    // Price to return.
    let mut price = 0.0;

    // Iterate over each item in the offer.
    for item in offer_items {
        // Skip over armor inserts as those are not factored into item prices.
        if is_of_baseclass(ctx.items, &item.template, BUILT_IN_INSERTS) {
            continue;
        }

        price += get_dynamic_item_price(
            ctx,
            &item.template,
            desired_currency,
            Some(item),
            Some(offer_items),
            Some(is_pack_offer),
        )?;

        // Check if the item is a weapon preset. Its price already accounts for the mods in the
        // preset, so the rest of the offer's items are skipped.
        if let Some(preset_id) = spt_preset_id(item)
            && is_preset_base_class(ctx, preset_id, WEAPON)?
        {
            break;
        }
    }

    Ok(round_half_even(price))
}

/// `RagfairPriceService.GetDynamicItemPrice` (`:295-377`). Never null in the C# either: every arm
/// returns a value, and a non-positive result becomes `0.1`.
pub fn get_dynamic_item_price(
    ctx: &mut RagfairContext,
    item_template_id: &str,
    desired_currency: &str,
    item: Option<&Item>,
    offer_items: Option<&[Item]>,
    is_pack_offer: Option<bool>,
) -> Result<f64, LootError> {
    let dynamic = ctx.dynamic;
    let mut is_preset = false;
    let mut price = get_flea_price_for_item(ctx, item_template_id);

    // Adjust price if below handbook price, based on config.
    if dynamic
        .offer_adjustment
        .adjust_price_when_below_handbook_price
    {
        price = adjust_price_if_below_handbook(ctx, price, item_template_id);
    }

    // Use trader price if higher, based on config.
    if dynamic.use_trader_price_for_offers_if_higher {
        let trader_price = get_highest_sell_to_trader_price(ctx, item_template_id);
        if trader_price > price {
            price = trader_price;
        }
    }

    // Prices for weapon presets are handled differently.
    if let (Some(item), Some(offer_items)) = (item, offer_items)
        && let Some(preset_id) = spt_preset_id(item)
        && is_preset_base_class(ctx, preset_id, WEAPON)?
    {
        price = if dynamic.generate_base_flea_prices.use_handbook_price
            && dynamic
                .generate_base_flea_prices
                .generate_preset_price_by_children
        {
            get_preset_price_by_children(ctx, offer_items)
        } else {
            get_weapon_preset_price(ctx, item, offer_items, price)?
        };
        is_preset = true;
    }

    // Check for existence of manual price adjustment multiplier
    if let Some(multiplier) = dynamic
        .item_price_multiplier
        .as_ref()
        .and_then(|multipliers| multipliers.get(item_template_id))
    {
        price *= multiplier;
    }

    // The quality of the item affects the price + not on the ignore list
    if let Some(item) = item
        && !dynamic
            .ignore_quality_price_variance_blacklist
            .iter()
            .any(|blacklisted| blacklisted == item_template_id)
    {
        let quality_modifier = get_item_quality_modifier(ctx.items, item, false);
        price *= quality_modifier;
    }

    // Make adjustments for unreasonably priced items.
    for (key, value) in &dynamic.unreasonable_mod_prices {
        if !value.enabled || !is_of_baseclass(ctx.items, item_template_id, key) {
            continue;
        }

        price = adjust_unreasonable_price(ctx, value, item_template_id, price);
    }

    // Vary the price based on the type of offer.
    let range = get_offer_type_range_values(ctx, is_preset, is_pack_offer.unwrap_or(false));
    price = randomise_offer_price(price, range);

    // Convert to different currency if required.
    if desired_currency != ROUBLES {
        price = from_roubles(ctx, price, desired_currency);
    }

    if price <= 0.0 {
        return Ok(0.1);
    }

    Ok(price)
}

/// `RagfairPriceService.AdjustUnreasonablePrice` (`:386-404`).
fn adjust_unreasonable_price(
    ctx: &RagfairContext,
    unreasonable_item_change: &UnreasonableModPricesWire,
    item_tpl: &str,
    price: f64,
) -> f64 {
    let item_handbook_price = get_static_price_for_item(ctx, item_tpl);

    // Flea price is over handbook price
    if price
        > item_handbook_price * f64::from(unreasonable_item_change.handbook_price_over_multiplier)
    {
        // Skip extreme values
        if price <= 1.0 {
            return price;
        }

        // Price is over limit, adjust
        return item_handbook_price
            * f64::from(unreasonable_item_change.new_price_handbook_multiplier);
    }

    price
}

/// `RagfairPriceService.GetOfferTypeRangeValues` (`:412-427`).
fn get_offer_type_range_values<'a>(
    ctx: &RagfairContext<'a>,
    is_preset: bool,
    is_pack: bool,
) -> &'a MinMaxDoubleWire {
    // Use different min/max values if the item is a preset or pack
    let price_ranges = &ctx.dynamic.price_ranges;
    if is_preset {
        return &price_ranges.preset;
    }

    if is_pack {
        return &price_ranges.pack;
    }

    &price_ranges.default
}

/// `RagfairPriceService.AdjustPriceIfBelowHandbook` (`:435-453`).
fn adjust_price_if_below_handbook(ctx: &RagfairContext, item_price: f64, item_tpl: &str) -> f64 {
    let item_handbook_price = get_static_price_for_item(ctx, item_tpl);
    let price_difference_percent = get_price_difference(item_handbook_price, item_price);
    let offer_adjustment_settings = &ctx.dynamic.offer_adjustment;

    // Only adjust price if difference is > a percent AND item price passes threshold set in config.
    // Two zero prices make the difference `NaN`, and `NaN > x` is false in C# and Rust alike, so
    // the branch is simply not taken — transcribed, not fixed.
    if price_difference_percent
        > offer_adjustment_settings.max_price_difference_below_handbook_percent
        && item_price >= offer_adjustment_settings.price_threshold_rub
    {
        return round_half_even(
            item_handbook_price * offer_adjustment_settings.handbook_price_multiplier,
        );
    }

    item_price
}

/// `RagfairPriceService.RandomiseOfferPrice` (`:461-468`) — the only draw in the module.
fn randomise_offer_price(existing_price: f64, range_values: &MinMaxDoubleWire) -> f64 {
    // Multiply by 100 to get 2 decimal places of precision
    let multiplier =
        get_biased_random_number(range_values.min * 100.0, range_values.max * 100.0, 2.0, 2.0);

    // return multiplier back to its original decimal place location
    existing_price * (multiplier / 100.0)
}

/// `RagfairPriceService.GetWeaponPresetPrice` (`:477-520`).
///
/// The C#'s `newOrReplacedModsInPresetVsDefault` is a lazy LINQ query enumerated three times; a
/// materialized `Vec` is output-equivalent because nothing mutates the source in between.
fn get_weapon_preset_price(
    ctx: &mut RagfairContext,
    weapon_root_item: &Item,
    weapon_with_children: &[Item],
    existing_price: f64,
) -> Result<f64, LootError> {
    // Get the default preset for this weapon
    let preset_result = get_weapon_preset(ctx, weapon_root_item)?;
    if preset_result.is_default {
        return Ok(get_flea_price_for_item(ctx, &weapon_root_item.template));
    }

    // Get mods on current gun not in default preset — matched by template
    let new_or_replaced_mods_in_preset_vs_default: Vec<&Item> = weapon_with_children
        .iter()
        .filter(|current| {
            !preset_result
                .preset
                .items
                .iter()
                .any(|in_preset| in_preset.template == current.template)
        })
        .collect();

    // Add up extra mods price. Use handbook or trader price, whatever is higher (dont use dynamic
    // flea price as purchased item cannot be relisted)
    let mut extra_mods_price = 0.0;
    for item in &new_or_replaced_mods_in_preset_vs_default {
        extra_mods_price += get_highest_handbook_or_trader_price_as_rouble(ctx, &item.template);
    }

    // Only deduct cost of replaced mods if there's replaced/new mods
    if !new_or_replaced_mods_in_preset_vs_default.is_empty() {
        // Add up cost of mods replaced — matched by slot id
        let mut replaced_mods_price = 0.0;
        for replaced_mod in new_or_replaced_mods_in_preset_vs_default
            .iter()
            .filter(|current| {
                preset_result
                    .preset
                    .items
                    .iter()
                    .any(|in_preset| in_preset.slot_id == current.slot_id)
            })
        {
            replaced_mods_price +=
                get_highest_handbook_or_trader_price_as_rouble(ctx, &replaced_mod.template);
        }

        // Subtract replaced mods total from extra mods total
        extra_mods_price -= replaced_mods_price;
    }

    // return extra mods price + base gun price
    Ok(existing_price + extra_mods_price)
}

/// `RagfairPriceService.GetPresetPriceByChildren` (`:527-544`).
fn get_preset_price_by_children(ctx: &RagfairContext, weapon_with_children: &[Item]) -> f64 {
    let mut price_total = 0.0;
    for item in weapon_with_children {
        // Root item uses static price
        if item
            .parent_id
            .as_deref()
            .is_none_or(|parent_id| parent_id.eq_ignore_ascii_case("hideout"))
        {
            price_total += get_static_price_for_item(ctx, &item.template);

            continue;
        }

        price_total += get_flea_price_for_item(ctx, &item.template);
    }

    price_total
}

/// `RagfairPriceService.GetHighestHandbookOrTraderPriceAsRouble` (`:551-561`).
fn get_highest_handbook_or_trader_price_as_rouble(ctx: &RagfairContext, item_tpl: &str) -> f64 {
    let price = get_static_price_for_item(ctx, item_tpl);
    let trader_price = get_highest_sell_to_trader_price(ctx, item_tpl);
    if trader_price > price {
        return trader_price;
    }

    price
}

/// `RagfairPriceService.WeaponPreset` (`:591-596`).
struct WeaponPreset<'a> {
    is_default: bool,
    preset: &'a PresetView,
}

/// `RagfairPriceService.GetWeaponPreset` (`:569-589`).
///
/// A tpl with neither a default preset nor any preset at all indexes an empty list in the C# and
/// throws; that becomes a [`LootError`] here.
fn get_weapon_preset<'a>(
    ctx: &mut RagfairContext<'a>,
    weapon: &Item,
) -> Result<WeaponPreset<'a>, LootError> {
    if let Some(default_preset) = ctx.default_presets_by_tpl.get(&weapon.template) {
        return Ok(WeaponPreset {
            is_default: true,
            preset: default_preset,
        });
    }

    let non_default_presets = ctx
        .presets_by_tpl
        .get(&weapon.template)
        .map_or(&[][..], Vec::as_slice);

    let Some(first_preset) = non_default_presets.first() else {
        return Err(LootError::new(format!(
            "Index was out of range: item {} has no default preset and no presets at all",
            weapon.template
        )));
    };

    let name = first_preset.name.clone().unwrap_or_default();
    ctx.diagnostics.push(plain(
        DEBUG,
        if non_default_presets.len() == 1 {
            format!(
                "Item Id: {} has no default encyclopedia entry but only one preset: ({name}), choosing preset: ({name})",
                weapon.template
            )
        } else {
            format!(
                "Item Id: {} has no default encyclopedia entry, choosing first preset({name}) of {}",
                weapon.template,
                non_default_presets.len()
            )
        },
    ));

    Ok(WeaponPreset {
        is_default: false,
        preset: first_preset,
    })
}

/// `RagfairPriceService.GetPriceDifference` (`:246-249`). Two zeroes divide by zero, which is
/// `NaN` for doubles in both languages.
fn get_price_difference(a: f64, b: f64) -> f64 {
    100.0 * a / (a + b)
}

/// `HandbookHelper.FromRoubles` (`HandbookHelper.cs:215-225`).
fn from_roubles(ctx: &RagfairContext, rouble_currency_count: f64, currency_type_to: &str) -> f64 {
    if currency_type_to == ROUBLES {
        return rouble_currency_count;
    }

    // Get price of currency from handbook
    let price = get_static_price_for_item(ctx, currency_type_to);
    if price > 0.0 {
        f64::max(1.0, round_half_even(rouble_currency_count / price))
    } else {
        0.0
    }
}

/// `TraderHelper.GetHighestSellToTraderPrice` (`TraderHelper.cs:516-546`), resolved per template by
/// the C# caller and handed over as a map. That method's floor is its `1d` default price, so a tpl
/// missing from the map answers `1`, not `0`.
fn get_highest_sell_to_trader_price(ctx: &RagfairContext, item_tpl: &str) -> f64 {
    ctx.highest_trader_prices
        .get(item_tpl)
        .copied()
        .unwrap_or(1.0)
}

/// `PresetHelper.IsPresetBaseClass` (`PresetHelper.cs:145-148`). The C# dereferences
/// `Encyclopedia!.Value` unguarded, so a preset without one throws there and errors here.
fn is_preset_base_class(
    ctx: &RagfairContext,
    preset_id: &str,
    base_class: &str,
) -> Result<bool, LootError> {
    let Some(preset) = ctx.item_presets.get(preset_id) else {
        return Ok(false);
    };

    let Some(encyclopedia) = preset.encyclopedia.as_deref() else {
        return Err(LootError::new(format!(
            "Nullable object must have a value: preset {preset_id} has no encyclopedia entry"
        )));
    };

    Ok(is_of_baseclass(ctx.items, encyclopedia, base_class))
}

/// `Item.Upd.SptPresetId` — `loot::models::Upd` does not name the member, so it rides in its
/// passthrough map under the C# wire name.
fn spt_preset_id(item: &Item) -> Option<&str> {
    item.upd.as_ref()?.extra.get("sptPresetId")?.as_str()
}

#[cfg(test)]
mod tests {
    use indexmap::IndexMap;
    use serde_json::json;

    use super::*;
    use crate::loot::item_helper::{AMMO_BOX, BUILT_IN_INSERTS, LootError, MOD, MONEY, WEAPON};
    use crate::loot::models::{ItemView, Upd, UpdRepairable};
    use crate::loot::random_util::{TestSeedGuard, get_biased_random_number, get_double};
    use crate::ragfair::models::DynamicConfigWire;
    use crate::ragfair::{NO_BLACKLIST, NO_DEFAULT_PRESETS, NO_NAMES};

    const SEED: u64 = 20260813;

    const PLAIN_TPL: &str = "plain_item";
    const HANDBOOK_ONLY_TPL: &str = "handbook_only_item";
    const ZERO_PRICE_TPL: &str = "zero_priced_item";
    const WEAPON_DEFAULT_TPL: &str = "weapon_with_default_preset";
    const WEAPON_NO_DEFAULT_TPL: &str = "weapon_without_default_preset";
    const AMMO_BOX_TPL: &str = "ammo_box";
    const CURRENCY_TPL: &str = "currency_dollars";
    const QUALITY_BLACKLIST_TPL: &str = "quality_blacklisted_item";
    const STOCK_TPL: &str = "mod_stock";
    const SCOPE_TPL: &str = "mod_scope";
    const INSERT_TPL: &str = "built_in_insert";

    const DEFAULT_PRESET_ID: &str = "preset_default";
    const NON_DEFAULT_PRESET_ID: &str = "preset_non_default";
    const ENCYCLOPEDIA_LESS_PRESET_ID: &str = "preset_without_encyclopedia";

    struct Fixture {
        items: IndexMap<String, ItemView>,
        dynamic: DynamicConfigWire,
        item_presets: IndexMap<String, PresetView>,
        default_presets_by_tpl: IndexMap<String, PresetView>,
        presets_by_tpl: IndexMap<String, Vec<PresetView>>,
        flea_prices: IndexMap<String, f64>,
        handbook_prices: IndexMap<String, f64>,
        highest_trader_prices: IndexMap<String, f64>,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                items: serde_json::from_value(json!({
                    PLAIN_TPL: {"name": "plain", "maxDurability": 100.0},
                    HANDBOOK_ONLY_TPL: {"name": "handbook only"},
                    ZERO_PRICE_TPL: {"name": "zero priced"},
                    WEAPON_DEFAULT_TPL: {"name": "akm", "parent": WEAPON, "maxDurability": 100.0},
                    WEAPON_NO_DEFAULT_TPL: {"name": "mp5", "parent": WEAPON,
                        "maxDurability": 100.0},
                    AMMO_BOX_TPL: {"name": "ammo box", "parent": AMMO_BOX},
                    CURRENCY_TPL: {"name": "dollars", "parent": MONEY},
                    QUALITY_BLACKLIST_TPL: {"name": "blacklisted", "maxDurability": 100.0},
                    STOCK_TPL: {"name": "stock", "parent": MOD},
                    SCOPE_TPL: {"name": "scope", "parent": MOD},
                    INSERT_TPL: {"name": "insert", "parent": BUILT_IN_INSERTS},
                }))
                .expect("items view parses"),
                dynamic: dynamic_config(),
                item_presets: serde_json::from_value(json!({
                    DEFAULT_PRESET_ID: {
                        "id": DEFAULT_PRESET_ID, "name": "akm default",
                        "encyclopedia": WEAPON_DEFAULT_TPL,
                        "items": [{"_id": "p1", "_tpl": WEAPON_DEFAULT_TPL},
                                  {"_id": "p2", "_tpl": STOCK_TPL, "parentId": "p1",
                                   "slotId": "mod_stock"}],
                    },
                    NON_DEFAULT_PRESET_ID: {
                        "id": NON_DEFAULT_PRESET_ID, "name": "mp5 custom",
                        "encyclopedia": WEAPON_NO_DEFAULT_TPL,
                        "items": [{"_id": "q1", "_tpl": WEAPON_NO_DEFAULT_TPL},
                                  {"_id": "q2", "_tpl": STOCK_TPL, "parentId": "q1",
                                   "slotId": "mod_stock"}],
                    },
                    ENCYCLOPEDIA_LESS_PRESET_ID: {
                        "id": ENCYCLOPEDIA_LESS_PRESET_ID, "name": "no encyclopedia",
                        "items": [],
                    },
                }))
                .expect("presets parse"),
                default_presets_by_tpl: serde_json::from_value(json!({
                    WEAPON_DEFAULT_TPL: {
                        "id": DEFAULT_PRESET_ID, "name": "akm default",
                        "encyclopedia": WEAPON_DEFAULT_TPL,
                        "items": [{"_id": "p1", "_tpl": WEAPON_DEFAULT_TPL},
                                  {"_id": "p2", "_tpl": STOCK_TPL, "parentId": "p1",
                                   "slotId": "mod_stock"}],
                    },
                }))
                .expect("default presets parse"),
                presets_by_tpl: serde_json::from_value(json!({
                    WEAPON_NO_DEFAULT_TPL: [{
                        "id": NON_DEFAULT_PRESET_ID, "name": "mp5 custom",
                        "encyclopedia": WEAPON_NO_DEFAULT_TPL,
                        "items": [{"_id": "q1", "_tpl": WEAPON_NO_DEFAULT_TPL},
                                  {"_id": "q2", "_tpl": STOCK_TPL, "parentId": "q1",
                                   "slotId": "mod_stock"}],
                    }],
                }))
                .expect("presets by tpl parse"),
                flea_prices: prices(&[
                    (PLAIN_TPL, 25_000.0),
                    (ZERO_PRICE_TPL, 0.0),
                    (WEAPON_DEFAULT_TPL, 50_000.0),
                    (WEAPON_NO_DEFAULT_TPL, 40_000.0),
                    (QUALITY_BLACKLIST_TPL, 10_000.0),
                    (STOCK_TPL, 3_000.0),
                    (SCOPE_TPL, 8_000.0),
                    (INSERT_TPL, 1_500.0),
                ]),
                handbook_prices: prices(&[
                    (PLAIN_TPL, 20_000.0),
                    (HANDBOOK_ONLY_TPL, 7_000.0),
                    (ZERO_PRICE_TPL, 0.0),
                    (WEAPON_DEFAULT_TPL, 45_000.0),
                    (WEAPON_NO_DEFAULT_TPL, 30_000.0),
                    (QUALITY_BLACKLIST_TPL, 9_000.0),
                    (STOCK_TPL, 2_000.0),
                    (SCOPE_TPL, 6_000.0),
                    (CURRENCY_TPL, 100.0),
                ]),
                highest_trader_prices: prices(&[(PLAIN_TPL, 12_000.0), (SCOPE_TPL, 9_000.0)]),
            }
        }

        fn ctx(&self) -> RagfairContext<'_> {
            RagfairContext {
                items: &self.items,
                dynamic: &self.dynamic,
                item_presets: &self.item_presets,
                default_presets: &NO_DEFAULT_PRESETS,
                default_presets_by_tpl: &self.default_presets_by_tpl,
                presets_by_tpl: &self.presets_by_tpl,
                flea_prices: &self.flea_prices,
                handbook_prices: &self.handbook_prices,
                highest_trader_prices: &self.highest_trader_prices,
                config_blacklist: &NO_BLACKLIST,
                seasonal_item_tpl_blacklist: &NO_BLACKLIST,
                pmc_names_usec: &NO_NAMES,
                pmc_names_bear: &NO_NAMES,
                timestamp: 1_700_000_000,
                seasonal_event_active: false,
                diagnostics: Vec::new(),
            }
        }
    }

    /// Every price range pinned to `min == max`, so `get_biased_random_number` returns without a
    /// draw and each stage's arithmetic can be read off in isolation.
    fn dynamic_config() -> DynamicConfigWire {
        serde_json::from_value(json!({
            "useTraderPriceForOffersIfHigher": false,
            "barter": {"chancePercent": 0.0, "itemCountMin": 1, "itemCountMax": 1,
                "priceRangeVariancePercent": 0.0, "minRoubleCostToBecomeBarter": 0.0,
                "makeSingleStackOnly": false, "itemTplBlacklist": [], "itemTypeBlacklist": []},
            "pack": {"chancePercent": 0.0, "itemCountMin": 1, "itemCountMax": 1,
                "itemTypeWhitelist": []},
            "offerAdjustment": {"adjustPriceWhenBelowHandbookPrice": false,
                "maxPriceDifferenceBelowHandbookPercent": 40.0, "handbookPriceMultiplier": 1.5,
                "priceThresholdRub": 6000.0},
            "offerItemCount": {"default": {"min": 1, "max": 1}},
            "priceRanges": {"default": {"min": 1.0, "max": 1.0},
                "preset": {"min": 1.0, "max": 1.0}, "pack": {"min": 1.0, "max": 1.0}},
            "showDefaultPresetsOnly": false,
            "ignoreQualityPriceVarianceBlacklist": [QUALITY_BLACKLIST_TPL],
            "endTimeSeconds": {"min": 1, "max": 2},
            "condition": {},
            "stackablePercent": {"min": 100.0, "max": 100.0},
            "nonStackableCount": {"min": 1, "max": 1},
            "rating": {"min": 0.0, "max": 1.0},
            "armor": {"removeRemovablePlateChance": 0, "plateSlotIdToRemovePool": []},
            "itemPriceMultiplier": {},
            "offerCurrencyChancePercent": {},
            "showAsSingleStack": [],
            "removeSeasonalItemsWhenNotInEvent": false,
            "blacklist": {"damagedAmmoPacks": false, "custom": [], "enableBsgList": false,
                "enableQuestList": false, "traderItems": false,
                "armorPlate": {"maxProtectionLevel": 0, "ignoreSlots": []},
                "enableCustomItemCategoryList": false, "customItemCategoryList": []},
            "unreasonableModPrices": {},
            "generateBaseFleaPrices": {"useHandbookPrice": false, "priceMultiplier": 1.0,
                "preventPriceBeingBelowTraderBuyPrice": false, "itemTplMultiplierOverride": {},
                "itemTypeMultiplierOverride": {}, "useHideoutCraftMultiplier": false,
                "hideoutCraftMultiplier": 1.0, "generatePresetPriceByChildren": false},
        }))
        .expect("dynamic config parses")
    }

    fn prices(entries: &[(&str, f64)]) -> IndexMap<String, f64> {
        entries
            .iter()
            .map(|(tpl, price)| ((*tpl).to_owned(), *price))
            .collect()
    }

    fn item(id: &str, tpl: &str) -> Item {
        Item {
            id: id.to_owned(),
            template: tpl.to_owned(),
            ..Default::default()
        }
    }

    fn mod_item(id: &str, tpl: &str, parent_id: &str, slot_id: &str) -> Item {
        Item {
            parent_id: Some(parent_id.to_owned()),
            slot_id: Some(slot_id.to_owned()),
            ..item(id, tpl)
        }
    }

    /// A root item carrying the `sptPresetId` the preset arms key off.
    fn preset_root(id: &str, tpl: &str, preset_id: &str) -> Item {
        let mut root = item(id, tpl);
        let mut upd = Upd::default();
        upd.extra.insert("sptPresetId".to_owned(), json!(preset_id));
        root.upd = Some(upd);

        root
    }

    fn repairable(mut item: Item, durability: f64, max_durability: f64) -> Item {
        item.upd = Some(Upd {
            repairable: Some(UpdRepairable {
                durability: Some(durability),
                max_durability: Some(max_durability),
                ..Default::default()
            }),
            ..item.upd.unwrap_or_default()
        });

        item
    }

    /// Where the seeded stream stands after `consume` — the read-the-next-draw idiom the bot
    /// modules use to pin a draw count.
    fn stream_position_after(consume: impl FnOnce()) -> f64 {
        let _guard = TestSeedGuard::install(SEED);
        consume();

        get_double(0.0, 1.0)
    }

    // -----------------------------------------------------------------------
    // get_flea_price_for_item / get_static_price_for_item
    // -----------------------------------------------------------------------

    #[test]
    fn flea_price_prefers_the_dynamic_price() {
        let fixture = Fixture::new();
        let ctx = fixture.ctx();

        assert_eq!(get_flea_price_for_item(&ctx, PLAIN_TPL), 25_000.0);
        assert!(ctx.diagnostics.is_empty());
    }

    #[test]
    fn flea_price_falls_back_to_the_handbook_price() {
        let fixture = Fixture::new();
        let ctx = fixture.ctx();

        assert_eq!(get_flea_price_for_item(&ctx, HANDBOOK_ONLY_TPL), 7_000.0);
        assert!(ctx.diagnostics.is_empty());
    }

    #[test]
    fn a_zero_price_floors_at_one_without_warning() {
        let fixture = Fixture::new();
        let ctx = fixture.ctx();

        // The `0 -> 1` floor is applied *after* the warning check, and a tpl the handbook knows
        // about at a price of zero is not a missing price.
        assert_eq!(get_flea_price_for_item(&ctx, ZERO_PRICE_TPL), 1.0);
        assert!(ctx.diagnostics.is_empty());
    }

    #[test]
    fn a_price_in_neither_table_floors_at_one_and_says_nothing() {
        let fixture = Fixture::new();
        let ctx = fixture.ctx();

        // The ammo box is in the items view but in neither price table. The C# warning arm for
        // this case is unreachable there (its coalesce is non-nullable), so it is unreachable here
        // too: the handbook miss is `0`, and the `0 -> 1` floor takes it from there.
        assert_eq!(get_flea_price_for_item(&ctx, AMMO_BOX_TPL), 1.0);
        assert!(ctx.diagnostics.is_empty());
    }

    #[test]
    fn static_price_of_an_unknown_tpl_is_zero() {
        let fixture = Fixture::new();
        let ctx = fixture.ctx();

        assert_eq!(get_static_price_for_item(&ctx, PLAIN_TPL), 20_000.0);
        assert_eq!(get_static_price_for_item(&ctx, "no_such_tpl"), 0.0);
    }

    // -----------------------------------------------------------------------
    // get_dynamic_item_price, one stage at a time
    // -----------------------------------------------------------------------

    #[test]
    fn a_pinned_range_prices_the_flea_value_unchanged() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx();

        let price = get_dynamic_item_price(&mut ctx, PLAIN_TPL, ROUBLES, None, None, None).unwrap();

        assert_eq!(price, 25_000.0);
    }

    #[test]
    fn a_price_below_handbook_is_raised_only_when_the_flag_is_set() {
        let mut fixture = Fixture::new();
        fixture
            .dynamic
            .offer_adjustment
            .adjust_price_when_below_handbook_price = true;
        let mut ctx = fixture.ctx();

        // 100 * 20000 / (20000 + 25000) = 44.4% > 40%, and 25000 >= 6000 -> round(20000 * 1.5)
        let price = get_dynamic_item_price(&mut ctx, PLAIN_TPL, ROUBLES, None, None, None).unwrap();

        assert_eq!(price, 30_000.0);
    }

    #[test]
    fn a_price_below_the_threshold_is_left_alone() {
        let mut fixture = Fixture::new();
        fixture
            .dynamic
            .offer_adjustment
            .adjust_price_when_below_handbook_price = true;
        fixture.dynamic.offer_adjustment.price_threshold_rub = 30_000.0;
        let mut ctx = fixture.ctx();

        let price = get_dynamic_item_price(&mut ctx, PLAIN_TPL, ROUBLES, None, None, None).unwrap();

        assert_eq!(price, 25_000.0);
    }

    #[test]
    fn two_zero_prices_divide_by_zero_and_the_branch_is_not_taken() {
        let mut fixture = Fixture::new();
        fixture
            .dynamic
            .offer_adjustment
            .adjust_price_when_below_handbook_price = true;
        let ctx = fixture.ctx();

        // `100 * 0 / (0 + 0)` is NaN in C# doubles, and `NaN > x` is false, so the adjustment is
        // skipped rather than applied. Rust `f64` does the same; this is not a bug to fix.
        assert!(get_price_difference(0.0, 0.0).is_nan());
        assert_eq!(
            adjust_price_if_below_handbook(&ctx, 0.0, ZERO_PRICE_TPL),
            0.0
        );
    }

    #[test]
    fn a_higher_trader_price_wins_when_the_flag_is_set() {
        let mut fixture = Fixture::new();
        fixture.dynamic.use_trader_price_for_offers_if_higher = true;
        fixture
            .highest_trader_prices
            .insert(PLAIN_TPL.to_owned(), 60_000.0);
        let mut ctx = fixture.ctx();

        let price = get_dynamic_item_price(&mut ctx, PLAIN_TPL, ROUBLES, None, None, None).unwrap();

        assert_eq!(price, 60_000.0);
    }

    #[test]
    fn a_lower_trader_price_leaves_the_flea_price_alone() {
        let mut fixture = Fixture::new();
        fixture.dynamic.use_trader_price_for_offers_if_higher = true;
        let mut ctx = fixture.ctx();

        let price = get_dynamic_item_price(&mut ctx, PLAIN_TPL, ROUBLES, None, None, None).unwrap();

        assert_eq!(price, 25_000.0);
    }

    #[test]
    fn the_item_price_multiplier_is_applied() {
        let mut fixture = Fixture::new();
        fixture
            .dynamic
            .item_price_multiplier
            .as_mut()
            .expect("multiplier map")
            .insert(PLAIN_TPL.to_owned(), 2.0);
        let mut ctx = fixture.ctx();

        let price = get_dynamic_item_price(&mut ctx, PLAIN_TPL, ROUBLES, None, None, None).unwrap();

        assert_eq!(price, 50_000.0);
    }

    #[test]
    fn the_quality_modifier_is_applied_when_an_item_is_supplied() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx();
        let worn = repairable(item("i1", PLAIN_TPL), 25.0, 100.0);

        // sqrt(25 / 100) = 0.5
        let price =
            get_dynamic_item_price(&mut ctx, PLAIN_TPL, ROUBLES, Some(&worn), None, None).unwrap();

        assert_eq!(price, 12_500.0);
    }

    #[test]
    fn a_blacklisted_tpl_skips_the_quality_modifier() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx();
        let worn = repairable(item("i1", QUALITY_BLACKLIST_TPL), 25.0, 100.0);

        let price = get_dynamic_item_price(
            &mut ctx,
            QUALITY_BLACKLIST_TPL,
            ROUBLES,
            Some(&worn),
            None,
            None,
        )
        .unwrap();

        assert_eq!(price, 10_000.0);
    }

    #[test]
    fn an_unreasonable_price_is_pulled_back_to_a_handbook_multiple() {
        let mut fixture = Fixture::new();
        fixture.dynamic.unreasonable_mod_prices.insert(
            MOD.to_owned(),
            serde_json::from_value(json!({"enabled": true, "handbookPriceOverMultiplier": 1,
                "newPriceHandbookMultiplier": 2, "itemType": "mod"}))
            .expect("unreasonable entry parses"),
        );
        let mut ctx = fixture.ctx();

        // Flea 8000 > handbook 6000 * 1, so the price becomes 6000 * 2.
        let price = get_dynamic_item_price(&mut ctx, SCOPE_TPL, ROUBLES, None, None, None).unwrap();

        assert_eq!(price, 12_000.0);
    }

    #[test]
    fn a_disabled_unreasonable_entry_is_skipped() {
        let mut fixture = Fixture::new();
        fixture.dynamic.unreasonable_mod_prices.insert(
            MOD.to_owned(),
            serde_json::from_value(json!({"enabled": false, "handbookPriceOverMultiplier": 1,
                "newPriceHandbookMultiplier": 2, "itemType": "mod"}))
            .expect("unreasonable entry parses"),
        );
        let mut ctx = fixture.ctx();

        let price = get_dynamic_item_price(&mut ctx, SCOPE_TPL, ROUBLES, None, None, None).unwrap();

        assert_eq!(price, 8_000.0);
    }

    #[test]
    fn a_non_rouble_currency_divides_by_its_handbook_price() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx();

        // 25000 roubles / 100 per dollar
        let price =
            get_dynamic_item_price(&mut ctx, PLAIN_TPL, CURRENCY_TPL, None, None, None).unwrap();

        assert_eq!(price, 250.0);
    }

    #[test]
    fn a_currency_with_no_handbook_price_converts_to_zero_and_floors_at_a_tenth() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx();

        let price =
            get_dynamic_item_price(&mut ctx, PLAIN_TPL, "unknown_currency", None, None, None)
                .unwrap();

        assert_eq!(price, 0.1);
    }

    #[test]
    fn a_zero_multiplier_floors_the_price_at_a_tenth() {
        let mut fixture = Fixture::new();
        fixture
            .dynamic
            .item_price_multiplier
            .as_mut()
            .expect("multiplier map")
            .insert(PLAIN_TPL.to_owned(), 0.0);
        let mut ctx = fixture.ctx();

        let price = get_dynamic_item_price(&mut ctx, PLAIN_TPL, ROUBLES, None, None, None).unwrap();

        assert_eq!(price, 0.1);
    }

    // -----------------------------------------------------------------------
    // The single draw
    // -----------------------------------------------------------------------

    #[test]
    fn a_seeded_price_is_the_flea_price_times_the_one_biased_draw() {
        let mut fixture = Fixture::new();
        fixture.dynamic.price_ranges.default.min = 0.8;
        fixture.dynamic.price_ranges.default.max = 1.2;
        let mut ctx = fixture.ctx();

        let expected_multiplier = {
            let _guard = TestSeedGuard::install(SEED);
            get_biased_random_number(80.0, 120.0, 2.0, 2.0)
        };

        let _guard = TestSeedGuard::install(SEED);
        let price = get_dynamic_item_price(&mut ctx, PLAIN_TPL, ROUBLES, None, None, None).unwrap();

        assert_eq!(price, 25_000.0 * (expected_multiplier / 100.0));
        assert_eq!(price, 24_500.0);
    }

    #[test]
    fn pricing_one_item_consumes_exactly_one_draw() {
        let mut fixture = Fixture::new();
        fixture.dynamic.price_ranges.default.min = 0.8;
        fixture.dynamic.price_ranges.default.max = 1.2;

        let after_price = stream_position_after(|| {
            let mut ctx = fixture.ctx();
            get_dynamic_item_price(&mut ctx, PLAIN_TPL, ROUBLES, None, None, None).unwrap();
        });
        let after_manual = stream_position_after(|| {
            get_biased_random_number(80.0, 120.0, 2.0, 2.0);
        });

        assert_eq!(after_price, after_manual);
    }

    // -----------------------------------------------------------------------
    // get_dynamic_offer_price_for_offer
    // -----------------------------------------------------------------------

    #[test]
    fn an_offer_sums_its_items_and_rounds_half_to_even() {
        let mut fixture = Fixture::new();
        // 25000 * 0.00005 = 1.25, 3000 * 0.00005 = 0.15 -> 1.4; a second scope run below shifts the
        // total onto a tie, which rounds to the even neighbour.
        fixture.dynamic.price_ranges.default.min = 0.00005;
        fixture.dynamic.price_ranges.default.max = 0.00005;
        let mut ctx = fixture.ctx();
        let offer = vec![item("i1", PLAIN_TPL), item("i2", STOCK_TPL)];

        let price = get_dynamic_offer_price_for_offer(&mut ctx, &offer, ROUBLES, false).unwrap();

        assert_eq!(price, 1.0);
    }

    #[test]
    fn built_in_inserts_are_skipped_before_pricing() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx();
        let offer = vec![
            item("i1", PLAIN_TPL),
            mod_item("i2", INSERT_TPL, "i1", "Soft_armor_front"),
        ];

        let price = get_dynamic_offer_price_for_offer(&mut ctx, &offer, ROUBLES, false).unwrap();

        assert_eq!(price, 25_000.0);
    }

    #[test]
    fn a_weapon_preset_stops_the_offer_loop_after_the_root() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx();
        let offer = vec![
            preset_root("i1", WEAPON_DEFAULT_TPL, DEFAULT_PRESET_ID),
            mod_item("i2", STOCK_TPL, "i1", "mod_stock"),
            mod_item("i3", SCOPE_TPL, "i1", "mod_scope"),
        ];

        let price = get_dynamic_offer_price_for_offer(&mut ctx, &offer, ROUBLES, false).unwrap();

        // The default-preset arm prices the root alone; the two mods are never priced.
        assert_eq!(price, 50_000.0);
    }

    #[test]
    fn a_weapon_preset_offer_consumes_exactly_one_draw() {
        let mut fixture = Fixture::new();
        fixture.dynamic.price_ranges.preset.min = 0.9;
        fixture.dynamic.price_ranges.preset.max = 1.4;

        let after_offer = stream_position_after(|| {
            let mut ctx = fixture.ctx();
            let offer = vec![
                preset_root("i1", WEAPON_DEFAULT_TPL, DEFAULT_PRESET_ID),
                mod_item("i2", STOCK_TPL, "i1", "mod_stock"),
                mod_item("i3", SCOPE_TPL, "i1", "mod_scope"),
            ];
            get_dynamic_offer_price_for_offer(&mut ctx, &offer, ROUBLES, false).unwrap();
        });
        let after_manual = stream_position_after(|| {
            get_biased_random_number(90.0, 140.0, 2.0, 2.0);
        });

        assert_eq!(after_offer, after_manual);
    }

    #[test]
    fn a_pack_offer_uses_the_pack_range() {
        let mut fixture = Fixture::new();
        fixture.dynamic.price_ranges.pack.min = 0.5;
        fixture.dynamic.price_ranges.pack.max = 0.5;
        let mut ctx = fixture.ctx();
        let offer = vec![item("i1", PLAIN_TPL)];

        let price = get_dynamic_offer_price_for_offer(&mut ctx, &offer, ROUBLES, true).unwrap();

        assert_eq!(price, 12_500.0);
    }

    // -----------------------------------------------------------------------
    // The preset arms
    // -----------------------------------------------------------------------

    #[test]
    fn a_preset_priced_by_children_uses_the_static_price_for_its_root() {
        let fixture = Fixture::new();
        let ctx = fixture.ctx();
        let children = vec![
            item("i1", WEAPON_DEFAULT_TPL),
            mod_item("i2", STOCK_TPL, "i1", "mod_stock"),
            mod_item("i3", SCOPE_TPL, "i1", "mod_scope"),
        ];

        // handbook 45000 for the root + flea 3000 + flea 8000 for the mods
        assert_eq!(get_preset_price_by_children(&ctx, &children), 56_000.0);
    }

    #[test]
    fn a_hideout_parented_root_is_still_a_root() {
        let fixture = Fixture::new();
        let ctx = fixture.ctx();
        let children = vec![mod_item("i1", WEAPON_DEFAULT_TPL, "HIDEOUT", "hideout")];

        assert_eq!(get_preset_price_by_children(&ctx, &children), 45_000.0);
    }

    #[test]
    fn the_preset_by_children_arm_needs_both_config_flags() {
        let mut fixture = Fixture::new();
        fixture.dynamic.generate_base_flea_prices.use_handbook_price = true;
        fixture
            .dynamic
            .generate_base_flea_prices
            .generate_preset_price_by_children = true;
        let mut ctx = fixture.ctx();
        let root = preset_root("i1", WEAPON_DEFAULT_TPL, DEFAULT_PRESET_ID);
        let offer = vec![root.clone(), mod_item("i2", SCOPE_TPL, "i1", "mod_scope")];

        let price = get_dynamic_item_price(
            &mut ctx,
            WEAPON_DEFAULT_TPL,
            ROUBLES,
            Some(&root),
            Some(&offer),
            None,
        )
        .unwrap();

        // handbook 45000 for the root + flea 8000 for the scope
        assert_eq!(price, 53_000.0);
    }

    #[test]
    fn a_non_default_preset_adds_new_mods_and_subtracts_the_ones_they_replace() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx();
        let root = item("q1", WEAPON_NO_DEFAULT_TPL);
        let children = vec![
            root.clone(),
            // Replaces the preset's `mod_stock` slot with a different template.
            mod_item("q2", SCOPE_TPL, "q1", "mod_stock"),
        ];

        // The scope is new (by template) so its highest handbook/trader price (9000) is added, and
        // it also occupies a slot the preset fills, so the same 9000 is subtracted again.
        let price = get_weapon_preset_price(&mut ctx, &root, &children, 40_000.0).unwrap();

        assert_eq!(price, 40_000.0);
    }

    #[test]
    fn a_non_default_preset_keeps_the_price_of_mods_in_no_preset_slot() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx();
        let root = item("q1", WEAPON_NO_DEFAULT_TPL);
        let children = vec![
            root.clone(),
            mod_item("q2", SCOPE_TPL, "q1", "mod_scope_mount"),
        ];

        // Nothing in the preset sits in `mod_scope_mount`, so the 9000 is not deducted. The root
        // itself is in the preset by template, so it adds nothing.
        let price = get_weapon_preset_price(&mut ctx, &root, &children, 40_000.0).unwrap();

        assert_eq!(price, 49_000.0);
    }

    #[test]
    fn a_default_preset_prices_at_the_roots_flea_price() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx();
        let root = item("p1", WEAPON_DEFAULT_TPL);
        let children = vec![root.clone(), mod_item("p3", SCOPE_TPL, "p1", "mod_scope")];

        let price = get_weapon_preset_price(&mut ctx, &root, &children, 1.0).unwrap();

        assert_eq!(price, 50_000.0);
    }

    #[test]
    fn a_weapon_with_no_default_preset_falls_back_to_the_first_one() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx();

        let preset = get_weapon_preset(&mut ctx, &item("q1", WEAPON_NO_DEFAULT_TPL)).unwrap();

        assert!(!preset.is_default);
        assert_eq!(preset.preset.id.as_deref(), Some(NON_DEFAULT_PRESET_ID));
        assert_eq!(ctx.diagnostics.len(), 1);
        assert_eq!(ctx.diagnostics[0].level, "debug");
    }

    #[test]
    fn a_weapon_with_no_presets_at_all_is_an_error() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx();

        let result = get_weapon_preset(&mut ctx, &item("i1", PLAIN_TPL));

        assert!(matches!(result, Err(LootError { .. })));
    }

    #[test]
    fn a_preset_without_an_encyclopedia_is_an_error() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx();
        let root = preset_root("i1", PLAIN_TPL, ENCYCLOPEDIA_LESS_PRESET_ID);
        let offer = vec![root.clone()];

        let result = get_dynamic_item_price(
            &mut ctx,
            PLAIN_TPL,
            ROUBLES,
            Some(&root),
            Some(&offer),
            None,
        );

        assert!(result.is_err());
    }

    #[test]
    fn an_unknown_preset_id_is_not_a_preset_offer() {
        let fixture = Fixture::new();
        let mut ctx = fixture.ctx();
        let root = preset_root("i1", PLAIN_TPL, "no_such_preset");
        let offer = vec![root.clone()];

        let price = get_dynamic_item_price(
            &mut ctx,
            PLAIN_TPL,
            ROUBLES,
            Some(&root),
            Some(&offer),
            None,
        )
        .unwrap();

        assert_eq!(price, 25_000.0);
    }

    #[test]
    fn the_highest_of_handbook_and_trader_price_wins() {
        let fixture = Fixture::new();
        let ctx = fixture.ctx();

        // Handbook 6000 vs trader 9000
        assert_eq!(
            get_highest_handbook_or_trader_price_as_rouble(&ctx, SCOPE_TPL),
            9_000.0
        );
        // Handbook 20000 vs trader 12000
        assert_eq!(
            get_highest_handbook_or_trader_price_as_rouble(&ctx, PLAIN_TPL),
            20_000.0
        );
    }
}
