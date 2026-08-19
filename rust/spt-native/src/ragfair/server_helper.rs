//! `Helpers/Ragfair/RagfairServerHelper.cs` — the stack-count, offer-count, currency and validity
//! helpers a dynamic ragfair pass leans on.
//!
//! **The draw table.** Every arm's cost in RNG draws, which is the parity contract the tests pin
//! with the read-the-next-draw idiom:
//!
//! | function | arm | draws |
//! |---|---|---|
//! | [`calculate_dynamic_stack_count`] | tpl not in the items view (error) | **0** |
//! | | `is_preset` or a `showAsSingleStack` base class | **0** |
//! | | `StackMaxSize ?? 1 == 1` | **1** — one [`get_int`] |
//! | | stackable | **1** — one [`get_double`]; the [`get_percent_of_value`] that follows is pure |
//! | [`get_offer_count_by_base_type`] | neither the parent nor `"default"` (error) | **0** |
//! | | otherwise | **1** — one [`get_int`] |
//! | [`get_dynamic_offer_currency`] | — | one [`get_weighted_value`], itself **0** draws for a single-entry map and **1** otherwise |
//! | [`is_item_valid_ragfair_item`] | every arm | **0** |
//!
//! Two shape divergences from the C#, both forced:
//! - the two unguarded C# throws on this path (`CalculateDynamicStackCount`'s explicit `throw` and
//!   `GetOfferCountByBaseType`'s null `MinMax` dereference) surface as [`LootError`];
//! - `IsItemValidRagfairItem`'s one database write — `Properties.CanSellOnRagfair = false` at
//!   `:61` — becomes an entry in the caller's `rejected` set, which the C# side replays onto the
//!   live template table. Nothing else in this module touches the views.

use indexmap::IndexSet;

use super::RagfairContext;
use crate::loot::item_helper::{AMMO_BOX, DEFAULT_INVALID_BASE_TYPES, LootError, is_valid_item};
use crate::loot::random_util::{get_double, get_int, get_percent_of_value, get_weighted_value};

/// `RagfairServerHelper.CalculateDynamicStackCount` (`:138-169`).
///
/// The `(int)` cast at `:168` never truncates anything: `GetPercentOfValue` rounds to `0` decimal
/// places first, so its result is already integral. It is transcribed anyway, as is the `Math.Max`
/// floor that keeps a rounded-to-zero percentage from producing an empty stack.
///
/// # Errors
///
/// Where the C# throws `ragfair-item_not_in_db_unable_to_generate_dynamic_stack_count`: a tpl the
/// items view does not know.
pub fn calculate_dynamic_stack_count(
    ctx: &RagfairContext,
    tpl: &str,
    is_preset: bool,
) -> Result<i32, LootError> {
    let config = ctx.dynamic;

    // Lookup item details - check if item not found
    let Some(item_details) = ctx.items.get(tpl) else {
        return Err(LootError::new(format!(
            "Item with tpl: {tpl} not found in db. Unable to generate a dynamic stack count"
        )));
    };

    // Item Types to return one of
    let show_as_single_stack: Vec<&str> = config
        .show_as_single_stack
        .iter()
        .map(String::as_str)
        .collect();
    if is_preset
        || ctx
            .base_classes
            .is_of_baseclasses(tpl, &show_as_single_stack)
    {
        return Ok(1);
    }

    // Get max possible stack count
    let max_stack_size = item_details.stack_max_size.unwrap_or(1);

    // non-stackable - use different values to calculate stack size
    if max_stack_size == 1 {
        return Ok(get_int(
            config.non_stackable_count.min,
            config.non_stackable_count.max,
        ));
    }

    // Get a % to get of stack size
    let stack_percent = get_double(config.stackable_percent.min, config.stackable_percent.max);

    // Min value to return should be no less than 1
    Ok(i32::max(
        get_percent_of_value(stack_percent, f64::from(max_stack_size), 0) as i32,
        1,
    ))
}

/// `RagfairServerHelper.GetOfferCountByBaseType` (`:224-232`).
///
/// # Errors
///
/// Where the C# throws: `GetValueOrDefault("default")` on a config with no `"default"` entry yields
/// a null `MinMax`, which `minMaxRange.Min` then dereferences unguarded.
pub fn get_offer_count_by_base_type(
    ctx: &RagfairContext,
    item_parent_type: &str,
) -> Result<i32, LootError> {
    let offer_item_count = &ctx.dynamic.offer_item_count;
    let min_max_range = match offer_item_count.get(item_parent_type) {
        Some(range) => range,
        None => offer_item_count.get("default").ok_or_else(|| {
            LootError::new("Object reference not set to an instance of an object.")
        })?,
    };

    Ok(get_int(min_max_range.min, min_max_range.max))
}

/// `RagfairServerHelper.GetDynamicOfferCurrency` (`:175-178`) — a weighted draw over
/// `offerCurrencyChancePercent` in insertion order.
///
/// # Errors
///
/// Propagates [`get_weighted_value`]'s: an empty map, or a scan that falls off the end.
pub fn get_dynamic_offer_currency(ctx: &RagfairContext) -> Result<String, LootError> {
    get_weighted_value(&ctx.dynamic.offer_currency_change_percent)
}

/// `RagfairServerHelper.IsItemValidRagfairItem` (`:37-89`). The arm order is the contract — an
/// item rejected by an earlier arm never reaches a later one.
///
/// Two notes on arms that are not what they look like:
/// - the quest-item arm (`:73`) is **dead code and stays dead**. `IsQuestItem()` is
///   `Properties.QuestItem.GetValueOrDefault(false)`, the very condition `IsValidItem` already
///   rejects on at `:47`, so no template can reach `:73` with it set. Transcribed, not pruned.
/// - the custom-blacklist arm (`:59-64`) is the only one with a side effect: the C# writes
///   `Properties.CanSellOnRagfair = false` onto the live template. This port records the tpl in
///   `rejected` instead and leaves the view untouched, so a second call for the same tpl takes the
///   custom arm again rather than the BSG one — the set dedupes, and the caller replays it once.
///
/// The C# would throw on a template with a null `_name`; that is not reproducible from real data
/// (`_name` is always present) and the mapping table gives this function no error return, so a
/// missing name tests as the empty string here.
pub fn is_item_valid_ragfair_item(
    ctx: &RagfairContext,
    tpl: &str,
    rejected: &mut IndexSet<String>,
) -> bool {
    let blacklist_config = &ctx.dynamic.blacklist;

    // Skip invalid items
    let Some(item_details) = ctx.items.get(tpl) else {
        return false;
    };

    if !is_valid_item(
        ctx.items,
        ctx.base_classes,
        ctx.config_blacklist,
        ctx.handbook_prices,
        ctx.flea_prices,
        tpl,
        &DEFAULT_INVALID_BASE_TYPES,
    ) {
        return false;
    }

    // Skip bsg blacklisted items
    if blacklist_config.enable_bsg_list && !item_details.can_sell_on_ragfair.unwrap_or(false) {
        return false;
    }

    // Skip custom blacklisted items and flag as unsellable by players
    if blacklist_config.custom.contains(tpl) {
        rejected.insert(tpl.to_owned());

        return false;
    }

    let parent = item_details.parent.as_deref().unwrap_or_default();

    // Skip custom category blacklisted items
    if blacklist_config.enable_custom_item_category_list
        && blacklist_config.custom_item_category_list.contains(parent)
    {
        return false;
    }

    // Skip quest items
    if blacklist_config.enable_quest_list && item_details.quest_item.unwrap_or(false) {
        return false;
    }

    // Don't include damaged ammo packs
    if blacklist_config.damaged_ammo_packs
        && parent == AMMO_BOX
        && item_details
            .name
            .as_deref()
            .unwrap_or_default()
            .contains("_damaged")
    {
        return false;
    }

    true
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use indexmap::IndexMap;
    use serde_json::json;

    use super::*;
    use crate::diag::DiagSink;
    use crate::loot::item_helper::{ItemBaseClassCache, STASH};
    use crate::loot::models::{ItemView, PresetView};
    use crate::loot::random_util::TestSeedGuard;
    use crate::ragfair::models::DynamicConfigWire;
    use crate::ragfair::{NO_BLACKLIST, NO_DEFAULT_PRESETS, NO_NAMES};

    const SEED: u64 = 20260813;

    const STACKABLE_TPL: &str = "stackable_item";
    const NON_STACKABLE_TPL: &str = "non_stackable_item";
    const NO_STACK_SIZE_TPL: &str = "item_without_a_stack_size";
    const SINGLE_STACK_TPL: &str = "shown_as_single_stack";
    const CUSTOM_BLACKLIST_TPL: &str = "custom_blacklisted_item";
    const CATEGORY_BLACKLIST_TPL: &str = "category_blacklisted_item";
    const QUEST_ITEM_TPL: &str = "quest_item";
    const DAMAGED_AMMO_BOX_TPL: &str = "ammo_box_damaged";
    const CLEAN_AMMO_BOX_TPL: &str = "ammo_box_clean";
    const NOT_SELLABLE_TPL: &str = "cannot_sell_on_ragfair_item";
    const CONFIG_BLACKLIST_TPL: &str = "config_blacklisted_item";
    const PRICELESS_TPL: &str = "item_with_no_price";
    const STASH_CHILD_TPL: &str = "child_of_stash";

    /// The base class `showAsSingleStack` names.
    const SINGLE_STACK_PARENT: &str = "single_stack_parent";
    /// The base class `customItemCategoryList` names.
    const CATEGORY_PARENT: &str = "blacklisted_category";
    /// A parent with its own `offerItemCount` entry.
    const COUNTED_PARENT: &str = "parent_with_its_own_offer_count";

    const ROUBLES: &str = "5449016a4bdc2d6f028b456f";
    const DOLLARS: &str = "5696686a4bdc2da3298b456a";
    const EUROS: &str = "569668774bdc2da2298b4568";

    struct Fixture {
        items: IndexMap<String, ItemView>,
        base_classes: ItemBaseClassCache,
        dynamic: DynamicConfigWire,
        prices: IndexMap<String, f64>,
        config_blacklist: HashSet<String>,
        presets: IndexMap<String, PresetView>,
        preset_lists: IndexMap<String, Vec<PresetView>>,
    }

    impl Fixture {
        fn new() -> Self {
            let items: IndexMap<String, ItemView> = serde_json::from_value(json!({
                STACKABLE_TPL: {"name": "stackable", "type": "Item", "stackMaxSize": 60,
                    "canSellOnRagfair": true},
                NON_STACKABLE_TPL: {"name": "non stackable", "type": "Item",
                    "stackMaxSize": 1, "canSellOnRagfair": true},
                NO_STACK_SIZE_TPL: {"name": "no stack size", "type": "Item",
                    "canSellOnRagfair": true},
                SINGLE_STACK_TPL: {"name": "single stack", "type": "Item",
                    "parent": SINGLE_STACK_PARENT, "stackMaxSize": 60,
                    "canSellOnRagfair": true},
                CUSTOM_BLACKLIST_TPL: {"name": "custom blacklisted", "type": "Item",
                    "canSellOnRagfair": true},
                CATEGORY_BLACKLIST_TPL: {"name": "category blacklisted", "type": "Item",
                    "parent": CATEGORY_PARENT, "canSellOnRagfair": true},
                QUEST_ITEM_TPL: {"name": "quest item", "type": "Item", "questItem": true,
                    "canSellOnRagfair": true},
                DAMAGED_AMMO_BOX_TPL: {"name": "patron_762x39_damaged", "type": "Item",
                    "parent": AMMO_BOX, "canSellOnRagfair": true},
                CLEAN_AMMO_BOX_TPL: {"name": "patron_762x39", "type": "Item",
                    "parent": AMMO_BOX, "canSellOnRagfair": true},
                NOT_SELLABLE_TPL: {"name": "not sellable", "type": "Item",
                    "canSellOnRagfair": false},
                CONFIG_BLACKLIST_TPL: {"name": "config blacklisted", "type": "Item",
                    "canSellOnRagfair": true},
                PRICELESS_TPL: {"name": "priceless", "type": "Item",
                    "canSellOnRagfair": true},
                STASH_CHILD_TPL: {"name": "a stash", "type": "Item", "parent": STASH,
                    "canSellOnRagfair": true},
                // The parents themselves, so the base-class walk has somewhere to land.
                SINGLE_STACK_PARENT: {"name": "single stack parent", "type": "Node"},
                CATEGORY_PARENT: {"name": "blacklisted category", "type": "Node"},
                AMMO_BOX: {"name": "ammo box base", "type": "Node"},
                STASH: {"name": "stash base", "type": "Node"},
            }))
            .expect("items view parses");
            let base_classes = ItemBaseClassCache::build(&items);

            Self {
                items,
                base_classes,
                dynamic: dynamic_config(),
                // One map behind the flea, handbook and trader views: nothing here prices an
                // offer, the tables only have to answer "has a price" for `is_valid_item`.
                prices: [
                    STACKABLE_TPL,
                    NON_STACKABLE_TPL,
                    NO_STACK_SIZE_TPL,
                    SINGLE_STACK_TPL,
                    CUSTOM_BLACKLIST_TPL,
                    CATEGORY_BLACKLIST_TPL,
                    QUEST_ITEM_TPL,
                    DAMAGED_AMMO_BOX_TPL,
                    CLEAN_AMMO_BOX_TPL,
                    NOT_SELLABLE_TPL,
                    CONFIG_BLACKLIST_TPL,
                    STASH_CHILD_TPL,
                ]
                .iter()
                .map(|tpl| ((*tpl).to_owned(), 1_000.0))
                .collect(),
                config_blacklist: HashSet::from([CONFIG_BLACKLIST_TPL.to_owned()]),
                presets: IndexMap::new(),
                preset_lists: IndexMap::new(),
            }
        }

        fn ctx(&self) -> RagfairContext<'_> {
            RagfairContext {
                items: &self.items,
                base_classes: &self.base_classes,
                dynamic: &self.dynamic,
                item_presets: &self.presets,
                default_presets: &NO_DEFAULT_PRESETS,
                default_presets_by_tpl: &self.presets,
                presets_by_tpl: &self.preset_lists,
                flea_prices: &self.prices,
                handbook_prices: &self.prices,
                highest_trader_prices: &self.prices,
                config_blacklist: &self.config_blacklist,
                seasonal_item_tpl_blacklist: &NO_BLACKLIST,
                pmc_names_usec: &NO_NAMES,
                pmc_names_bear: &NO_NAMES,
                timestamp: 1_700_000_000,
                seasonal_event_active: false,
                diagnostics: DiagSink::capture(),
            }
        }
    }

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
            "offerItemCount": {COUNTED_PARENT: {"min": 7, "max": 9},
                "default": {"min": 2, "max": 5}},
            "priceRanges": {"default": {"min": 1.0, "max": 1.0},
                "preset": {"min": 1.0, "max": 1.0}, "pack": {"min": 1.0, "max": 1.0}},
            "showDefaultPresetsOnly": false,
            "ignoreQualityPriceVarianceBlacklist": [],
            "endTimeSeconds": {"min": 1, "max": 2},
            "condition": {},
            "stackablePercent": {"min": 10.0, "max": 100.0},
            "nonStackableCount": {"min": 1, "max": 4},
            "rating": {"min": 0.0, "max": 1.0},
            "armor": {"removeRemovablePlateChance": 0, "plateSlotIdToRemovePool": []},
            "itemPriceMultiplier": {},
            "offerCurrencyChancePercent": {ROUBLES: 75.0, DOLLARS: 20.0, EUROS: 5.0},
            "showAsSingleStack": [SINGLE_STACK_PARENT],
            "removeSeasonalItemsWhenNotInEvent": false,
            "blacklist": {"damagedAmmoPacks": true, "custom": [CUSTOM_BLACKLIST_TPL],
                "enableBsgList": true, "enableQuestList": true, "traderItems": false,
                "armorPlate": {"maxProtectionLevel": 0, "ignoreSlots": []},
                "enableCustomItemCategoryList": true,
                "customItemCategoryList": [CATEGORY_PARENT]},
            "unreasonableModPrices": {},
            "generateBaseFleaPrices": {"useHandbookPrice": false, "priceMultiplier": 1.0,
                "preventPriceBeingBelowTraderBuyPrice": false, "itemTplMultiplierOverride": {},
                "itemTypeMultiplierOverride": {}, "useHideoutCraftMultiplier": false,
                "hideoutCraftMultiplier": 1.0, "generatePresetPriceByChildren": false},
        }))
        .expect("dynamic config parses")
    }

    /// Where the seeded stream stands after `consume` — the read-the-next-draw idiom
    /// `price_service` uses to pin a draw count.
    fn stream_position_after(consume: impl FnOnce()) -> f64 {
        let _guard = TestSeedGuard::install(SEED);
        consume();

        get_double(0.0, 1.0)
    }

    /// The stream untouched, i.e. what a zero-draw arm has to leave behind.
    fn untouched_stream() -> f64 {
        stream_position_after(|| {})
    }

    // -----------------------------------------------------------------------
    // calculate_dynamic_stack_count
    // -----------------------------------------------------------------------

    #[test]
    fn an_unknown_tpl_is_an_error_and_costs_no_draw() {
        let fixture = Fixture::new();

        let error = calculate_dynamic_stack_count(&fixture.ctx(), "no_such_tpl", false)
            .expect_err("an unknown tpl errors");

        assert_eq!(
            error.message,
            "Item with tpl: no_such_tpl not found in db. Unable to generate a dynamic stack count"
        );
        let after = stream_position_after(|| {
            calculate_dynamic_stack_count(&fixture.ctx(), "no_such_tpl", false).unwrap_err();
        });
        assert_eq!(after, untouched_stream());
    }

    #[test]
    fn a_preset_stacks_to_one_without_drawing() {
        let fixture = Fixture::new();

        let count = calculate_dynamic_stack_count(&fixture.ctx(), STACKABLE_TPL, true).unwrap();

        assert_eq!(count, 1);
        let after = stream_position_after(|| {
            calculate_dynamic_stack_count(&fixture.ctx(), STACKABLE_TPL, true).unwrap();
        });
        assert_eq!(after, untouched_stream());
    }

    #[test]
    fn a_show_as_single_stack_base_class_stacks_to_one_without_drawing() {
        let fixture = Fixture::new();

        // Stackable to 60, but its parent is on `showAsSingleStack`.
        let count = calculate_dynamic_stack_count(&fixture.ctx(), SINGLE_STACK_TPL, false).unwrap();

        assert_eq!(count, 1);
        let after = stream_position_after(|| {
            calculate_dynamic_stack_count(&fixture.ctx(), SINGLE_STACK_TPL, false).unwrap();
        });
        assert_eq!(after, untouched_stream());
    }

    #[test]
    fn a_non_stackable_item_takes_one_int_draw() {
        let fixture = Fixture::new();

        let expected = {
            let _guard = TestSeedGuard::install(SEED);
            get_int(1, 4)
        };

        let count = {
            let _guard = TestSeedGuard::install(SEED);
            calculate_dynamic_stack_count(&fixture.ctx(), NON_STACKABLE_TPL, false).unwrap()
        };

        assert_eq!(count, expected);
        assert!((1..=4).contains(&count));

        let after = stream_position_after(|| {
            calculate_dynamic_stack_count(&fixture.ctx(), NON_STACKABLE_TPL, false).unwrap();
        });
        let after_manual = stream_position_after(|| {
            get_int(1, 4);
        });
        assert_eq!(after, after_manual);
    }

    #[test]
    fn a_missing_stack_max_size_counts_as_non_stackable() {
        let fixture = Fixture::new();

        let after = stream_position_after(|| {
            calculate_dynamic_stack_count(&fixture.ctx(), NO_STACK_SIZE_TPL, false).unwrap();
        });
        let after_manual = stream_position_after(|| {
            get_int(1, 4);
        });

        assert_eq!(after, after_manual);
    }

    #[test]
    fn a_stackable_item_takes_one_double_draw_and_a_pure_percentage() {
        let fixture = Fixture::new();

        let expected = {
            let _guard = TestSeedGuard::install(SEED);
            let percent = get_double(10.0, 100.0);
            get_percent_of_value(percent, 60.0, 0) as i32
        };

        let count = {
            let _guard = TestSeedGuard::install(SEED);
            calculate_dynamic_stack_count(&fixture.ctx(), STACKABLE_TPL, false).unwrap()
        };

        assert_eq!(count, expected);
        assert!((1..=60).contains(&count));

        // `get_percent_of_value` is pure, so the whole arm costs the one `get_double`.
        let after = stream_position_after(|| {
            calculate_dynamic_stack_count(&fixture.ctx(), STACKABLE_TPL, false).unwrap();
        });
        let after_manual = stream_position_after(|| {
            get_double(10.0, 100.0);
        });
        assert_eq!(after, after_manual);
    }

    #[test]
    fn a_percentage_that_rounds_to_zero_floors_at_one() {
        let mut fixture = Fixture::new();
        // A degenerate range still draws; 0.5% of 60 is 0.3, which rounds to 0 at 0 decimals.
        fixture.dynamic.stackable_percent.min = 0.5;
        fixture.dynamic.stackable_percent.max = 0.5;

        let _guard = TestSeedGuard::install(SEED);
        let count = calculate_dynamic_stack_count(&fixture.ctx(), STACKABLE_TPL, false).unwrap();

        assert_eq!(count, 1);
    }

    // -----------------------------------------------------------------------
    // get_offer_count_by_base_type
    // -----------------------------------------------------------------------

    #[test]
    fn an_offer_count_prefers_the_entry_for_the_parent() {
        let fixture = Fixture::new();

        let expected = {
            let _guard = TestSeedGuard::install(SEED);
            get_int(7, 9)
        };

        let count = {
            let _guard = TestSeedGuard::install(SEED);
            get_offer_count_by_base_type(&fixture.ctx(), COUNTED_PARENT).unwrap()
        };

        assert_eq!(count, expected);
        assert!((7..=9).contains(&count));

        let after = stream_position_after(|| {
            get_offer_count_by_base_type(&fixture.ctx(), COUNTED_PARENT).unwrap();
        });
        let after_manual = stream_position_after(|| {
            get_int(7, 9);
        });
        assert_eq!(after, after_manual);
    }

    #[test]
    fn an_unknown_parent_falls_back_to_the_default_entry() {
        let fixture = Fixture::new();

        let expected = {
            let _guard = TestSeedGuard::install(SEED);
            get_int(2, 5)
        };

        let count = {
            let _guard = TestSeedGuard::install(SEED);
            get_offer_count_by_base_type(&fixture.ctx(), "parent_with_no_entry").unwrap()
        };

        assert_eq!(count, expected);
        assert!((2..=5).contains(&count));
    }

    #[test]
    fn a_config_with_neither_entry_is_an_error_and_costs_no_draw() {
        let mut fixture = Fixture::new();
        fixture.dynamic.offer_item_count.shift_remove("default");

        let error = get_offer_count_by_base_type(&fixture.ctx(), "parent_with_no_entry")
            .expect_err("a missing `default` entry errors");

        assert_eq!(
            error.message,
            "Object reference not set to an instance of an object."
        );
        let after = stream_position_after(|| {
            get_offer_count_by_base_type(&fixture.ctx(), "parent_with_no_entry").unwrap_err();
        });
        assert_eq!(after, untouched_stream());
    }

    // -----------------------------------------------------------------------
    // get_dynamic_offer_currency
    // -----------------------------------------------------------------------

    #[test]
    fn the_offer_currency_is_the_weighted_draw_over_the_config_map() {
        let fixture = Fixture::new();
        let ctx = fixture.ctx();

        let expected: Vec<String> = {
            let _guard = TestSeedGuard::install(SEED);
            (0..8)
                .map(|_| get_weighted_value(&ctx.dynamic.offer_currency_change_percent).unwrap())
                .collect()
        };

        let drawn: Vec<String> = {
            let _guard = TestSeedGuard::install(SEED);
            (0..8)
                .map(|_| get_dynamic_offer_currency(&ctx).unwrap())
                .collect()
        };

        assert_eq!(drawn, expected);
        // The weights are 75/20/5 over an insertion-ordered map, so the sequence is pinned.
        assert_eq!(
            drawn,
            vec![
                ROUBLES, ROUBLES, ROUBLES, ROUBLES, ROUBLES, DOLLARS, DOLLARS, DOLLARS
            ]
        );
    }

    #[test]
    fn one_currency_draw_costs_one_double() {
        let fixture = Fixture::new();

        let after = stream_position_after(|| {
            get_dynamic_offer_currency(&fixture.ctx()).unwrap();
        });
        let after_manual = stream_position_after(|| {
            get_double(0.0, 1.0);
        });

        assert_eq!(after, after_manual);
    }

    // -----------------------------------------------------------------------
    // is_item_valid_ragfair_item
    // -----------------------------------------------------------------------

    /// Runs the check with a fresh `rejected` set and asserts the arm cost no draws.
    fn check_validity(fixture: &Fixture, tpl: &str) -> (bool, IndexSet<String>) {
        let mut rejected = IndexSet::new();
        let valid = is_item_valid_ragfair_item(&fixture.ctx(), tpl, &mut rejected);

        let after = stream_position_after(|| {
            let mut throwaway = IndexSet::new();
            is_item_valid_ragfair_item(&fixture.ctx(), tpl, &mut throwaway);
        });
        assert_eq!(after, untouched_stream(), "{tpl} consumed a draw");

        (valid, rejected)
    }

    #[test]
    fn a_plain_sellable_item_is_valid() {
        let fixture = Fixture::new();

        let (valid, rejected) = check_validity(&fixture, STACKABLE_TPL);

        assert!(valid);
        assert!(rejected.is_empty());
    }

    #[test]
    fn a_tpl_the_view_does_not_know_is_invalid() {
        let fixture = Fixture::new();

        let (valid, rejected) = check_validity(&fixture, "no_such_tpl");

        assert!(!valid);
        assert!(rejected.is_empty());
    }

    #[test]
    fn an_item_is_valid_item_rejects_is_invalid() {
        let fixture = Fixture::new();

        // Three different ways past `IsValidItem`: the config blacklist, no price at all, and a
        // default-invalid base type.
        for tpl in [CONFIG_BLACKLIST_TPL, PRICELESS_TPL, STASH_CHILD_TPL] {
            let (valid, rejected) = check_validity(&fixture, tpl);

            assert!(!valid, "{tpl} should be rejected");
            assert!(rejected.is_empty(), "{tpl} should not be recorded");
        }
    }

    #[test]
    fn the_bsg_list_rejects_an_item_that_cannot_be_sold() {
        let fixture = Fixture::new();

        let (valid, rejected) = check_validity(&fixture, NOT_SELLABLE_TPL);

        assert!(!valid);
        assert!(rejected.is_empty());
    }

    #[test]
    fn a_disabled_bsg_list_lets_an_unsellable_item_through() {
        let mut fixture = Fixture::new();
        fixture.dynamic.blacklist.enable_bsg_list = false;

        let (valid, rejected) = check_validity(&fixture, NOT_SELLABLE_TPL);

        assert!(valid);
        assert!(rejected.is_empty());
    }

    #[test]
    fn the_custom_blacklist_rejects_and_records_the_tpl() {
        let fixture = Fixture::new();

        let (valid, rejected) = check_validity(&fixture, CUSTOM_BLACKLIST_TPL);

        assert!(!valid);
        // The one database write the port replays.
        assert_eq!(
            rejected.iter().collect::<Vec<_>>(),
            vec![CUSTOM_BLACKLIST_TPL]
        );
    }

    #[test]
    fn the_custom_category_list_rejects_by_parent() {
        let fixture = Fixture::new();

        let (valid, rejected) = check_validity(&fixture, CATEGORY_BLACKLIST_TPL);

        assert!(!valid);
        assert!(rejected.is_empty());
    }

    #[test]
    fn a_disabled_custom_category_list_lets_the_parent_through() {
        let mut fixture = Fixture::new();
        fixture.dynamic.blacklist.enable_custom_item_category_list = false;

        let (valid, rejected) = check_validity(&fixture, CATEGORY_BLACKLIST_TPL);

        assert!(valid);
        assert!(rejected.is_empty());
    }

    #[test]
    fn a_quest_item_is_rejected_before_the_quest_arm_is_ever_reached() {
        let fixture = Fixture::new();

        let (valid, rejected) = check_validity(&fixture, QUEST_ITEM_TPL);

        assert!(!valid);
        assert!(rejected.is_empty());

        // `IsValidItem` already rejects on `QuestItem`, so `enableQuestList` cannot change the
        // answer — the arm at `:73` is unreachable in the C# too.
        let mut without_the_quest_list = Fixture::new();
        without_the_quest_list.dynamic.blacklist.enable_quest_list = false;
        let (still_invalid, _) = check_validity(&without_the_quest_list, QUEST_ITEM_TPL);

        assert!(!still_invalid);
    }

    #[test]
    fn a_damaged_ammo_pack_is_rejected_by_name() {
        let fixture = Fixture::new();

        let (damaged, rejected) = check_validity(&fixture, DAMAGED_AMMO_BOX_TPL);
        let (clean, _) = check_validity(&fixture, CLEAN_AMMO_BOX_TPL);

        assert!(!damaged);
        assert!(clean);
        assert!(rejected.is_empty());
    }

    #[test]
    fn a_disabled_damaged_ammo_pack_flag_lets_it_through() {
        let mut fixture = Fixture::new();
        fixture.dynamic.blacklist.damaged_ammo_packs = false;

        let (valid, rejected) = check_validity(&fixture, DAMAGED_AMMO_BOX_TPL);

        assert!(valid);
        assert!(rejected.is_empty());
    }

    #[test]
    fn a_damaged_name_outside_an_ammo_box_is_not_rejected() {
        let mut fixture = Fixture::new();
        fixture
            .items
            .get_mut(STACKABLE_TPL)
            .expect("fixture item")
            .name = Some("something_damaged".to_owned());

        let (valid, rejected) = check_validity(&fixture, STACKABLE_TPL);

        assert!(valid);
        assert!(rejected.is_empty());
    }

    #[test]
    fn the_custom_blacklist_is_the_only_arm_that_records_a_tpl() {
        let fixture = Fixture::new();
        let mut rejected = IndexSet::new();

        for tpl in [
            STACKABLE_TPL,
            "no_such_tpl",
            CONFIG_BLACKLIST_TPL,
            PRICELESS_TPL,
            STASH_CHILD_TPL,
            NOT_SELLABLE_TPL,
            CATEGORY_BLACKLIST_TPL,
            QUEST_ITEM_TPL,
            DAMAGED_AMMO_BOX_TPL,
            CLEAN_AMMO_BOX_TPL,
        ] {
            is_item_valid_ragfair_item(&fixture.ctx(), tpl, &mut rejected);
        }

        assert!(rejected.is_empty());

        is_item_valid_ragfair_item(&fixture.ctx(), CUSTOM_BLACKLIST_TPL, &mut rejected);

        assert_eq!(
            rejected.iter().collect::<Vec<_>>(),
            vec![CUSTOM_BLACKLIST_TPL]
        );
    }
}
