//! Wire models for the dynamic ragfair offer generator.
//!
//! The same two families as `loot::models` and `bot::models`: the config records mirroring
//! `Models/Spt/Config/RagfairConfig.cs` (wire names pinned to the C# `JsonPropertyName`, each with a
//! `#[serde(flatten)] extra` map so mod-added members survive), and the request/response envelopes,
//! a fresh contract between the C# caller and this crate and so plain camelCase.
//!
//! The game-data types (`Item`, `ItemView`, `PresetView`, `Diagnostic`) are reused from
//! `loot::models` rather than redeclared. There is no `ItemsView` type: the
//! `IndexMap<String, ItemView>` *is* the view, which is what `loot::item_helper`'s helpers take.

use indexmap::{IndexMap, IndexSet};
use serde::{Deserialize, Serialize};

use crate::loot::models::{Diagnostic, Item, ItemView, PresetView};

/// Mod-added fields captured on the way in.
type Extra = serde_json::Map<String, serde_json::Value>;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateDynamicOffersRequest {
    /// Test-only: draws come from a seeded generator when set.
    pub test_seed: Option<u64>,
    /// `TimeUtil.GetTimeStamp()` taken once by the caller. Legacy re-reads the clock per offer
    /// (`RagfairOfferGenerator.cs:491`); one timestamp for the batch is a sanctioned divergence.
    pub timestamp: i64,
    /// The generator's `OfferCounter` (`:59`) before the pass; offers come back numbered from it.
    pub offer_counter_start: i32,
    /// `null` for a full pass; the cloned expired-offer item lists for a regeneration pass
    /// (`RagfairServer.cs:69-79`).
    pub expired_offers: Option<Vec<Vec<Item>>>,
    pub dynamic: DynamicConfigWire,
    /// `GlobalTable.ItemPresets` — `PresetHelper.IsPreset`/`GetPreset` read this map, and
    /// `GetAllPresets()` is its `Values` in insertion order.
    pub item_presets: IndexMap<String, PresetView>,
    /// `PresetHelper.GetDefaultPresets().Values.ToList()` — the assort walk's preset source when
    /// `showDefaultPresetsOnly` is set (`RagfairAssortGenerator.cs:115-117`).
    pub default_presets: Vec<PresetView>,
    /// `PresetHelper.GetDefaultPresetByTpl()` — `GetDefaultPreset(tpl)` for the weapon-preset price.
    pub default_presets_by_tpl: IndexMap<String, PresetView>,
    /// `PresetHelper.GetPresets(tpl)` resolved for every tpl that has presets — the fallback arm of
    /// `RagfairPriceService.GetWeaponPreset` (`:577`).
    pub presets_by_tpl: IndexMap<String, Vec<PresetView>>,
    /// `templateTable.Prices` — the whole flea base price table, insertion ordered: it is the
    /// source order of `GetFleaPricesAsArray` (`RagfairOfferGenerator.cs:938`), which feeds an
    /// index draw.
    pub flea_prices: IndexMap<String, f64>,
    /// `HandbookHelper.GetTemplatePrice` for the whole items table.
    pub handbook_prices: IndexMap<String, f64>,
    /// `TraderHelper.GetHighestSellToTraderPrice` resolved per template (a cache-backed C# loop, so
    /// it stays on the C# side and crosses as a map).
    pub highest_trader_prices: IndexMap<String, f64>,
    /// `ItemFilterService.GetBlacklistedItems()` — read by `ItemHelper.IsValidItem`.
    pub config_blacklist: Vec<String>,
    /// `SeasonalEventService.SeasonalEventEnabled()` (`RagfairAssortGenerator.cs:57`).
    pub seasonal_event_active: bool,
    /// `SeasonalEventService.GetInactiveSeasonalEventItems()` (`:58`).
    pub seasonal_item_tpl_blacklist: Vec<String>,
    /// `BotHelper.GatherPmcNamesOfLength` for each faction at `botConfig.BotNameLengthLimit`,
    /// pre-filtered. The faction is still drawn natively (`BotHelper.cs:151`, `GetInt(0, 1)`).
    pub pmc_names_usec: Vec<String>,
    pub pmc_names_bear: Vec<String>,
    pub items: IndexMap<String, ItemView>,
}

/// `Models/Spt/Config/RagfairConfig.cs:102-239` `Dynamic`, whole. Reuse the C# record's wire names.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DynamicConfigWire {
    pub use_trader_price_for_offers_if_higher: bool,
    pub barter: BarterDetailsWire,
    pub pack: PackDetailsWire,
    pub offer_adjustment: OfferAdjustmentWire,
    /// Keys are a tpl **or** the literal `"default"` (`RagfairConfig.cs:139`).
    pub offer_item_count: IndexMap<String, MinMaxIntWire>,
    pub price_ranges: PriceRangesWire,
    pub show_default_presets_only: bool,
    pub ignore_quality_price_variance_blacklist: Vec<String>,
    pub end_time_seconds: MinMaxIntWire,
    /// Keyed by base-class tpl; **iteration order is the match order** in
    /// `GetDynamicConditionIdForTpl` (`RagfairOfferGenerator.cs:676-683`).
    pub condition: IndexMap<String, ConditionWire>,
    pub stackable_percent: MinMaxDoubleWire,
    pub non_stackable_count: MinMaxIntWire,
    pub rating: MinMaxDoubleWire,
    pub armor: ArmorSettingsWire,
    pub item_price_multiplier: Option<IndexMap<String, f64>>,
    #[serde(rename = "offerCurrencyChancePercent")]
    pub offer_currency_change_percent: IndexMap<String, f64>,
    pub show_as_single_stack: Vec<String>,
    pub remove_seasonal_items_when_not_in_event: bool,
    pub blacklist: RagfairBlacklistWire,
    pub unreasonable_mod_prices: IndexMap<String, UnreasonableModPricesWire>,
    pub generate_base_flea_prices: GenerateFleaPricesWire,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MinMaxIntWire {
    pub min: i32,
    pub max: i32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MinMaxDoubleWire {
    pub min: f64,
    pub max: f64,
}

/// `RagfairConfig.cs:292-341` `BarterDetails`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BarterDetailsWire {
    pub chance_percent: f64,
    pub item_count_min: i32,
    pub item_count_max: i32,
    pub price_range_variance_percent: f64,
    pub min_rouble_cost_to_become_barter: f64,
    pub make_single_stack_only: bool,
    pub item_tpl_blacklist: IndexSet<String>,
    pub item_type_blacklist: IndexSet<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `RagfairConfig.cs:343-368` `PackDetails`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackDetailsWire {
    pub chance_percent: f64,
    pub item_count_min: i32,
    pub item_count_max: i32,
    pub item_type_whitelist: IndexSet<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `RagfairConfig.cs:370-395` `OfferAdjustment`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OfferAdjustmentWire {
    pub adjust_price_when_below_handbook_price: bool,
    pub max_price_difference_below_handbook_percent: f64,
    pub handbook_price_multiplier: f64,
    pub price_threshold_rub: f64,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `RagfairConfig.cs:280-290` `PriceRanges`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PriceRangesWire {
    pub default: MinMaxDoubleWire,
    pub preset: MinMaxDoubleWire,
    pub pack: MinMaxDoubleWire,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `RagfairConfig.cs:397-413` `Condition`. `_name` is a note to the config author, never read, so
/// it rides in `extra`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConditionWire {
    pub condition_chance: f64,
    pub current: MinMaxDoubleWire,
    pub max: MinMaxDoubleWire,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `RagfairConfig.cs:505-518` `ArmorSettings`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArmorSettingsWire {
    pub remove_removable_plate_chance: i32,
    /// An `IndexSet` because it is drawn from by index and a `HashSet` deserialized from a JSON
    /// array keeps that array's order in C# too.
    pub plate_slot_id_to_remove_pool: IndexSet<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `RagfairConfig.cs:415-464` `RagfairBlacklist`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RagfairBlacklistWire {
    pub damaged_ammo_packs: bool,
    pub custom: IndexSet<String>,
    pub enable_bsg_list: bool,
    pub enable_quest_list: bool,
    pub trader_items: bool,
    pub armor_plate: ArmorPlateBlacklistSettingsWire,
    pub enable_custom_item_category_list: bool,
    pub custom_item_category_list: IndexSet<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `RagfairConfig.cs:466-479` `ArmorPlateBlacklistSettings`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArmorPlateBlacklistSettingsWire {
    pub max_protection_level: i32,
    pub ignore_slots: IndexSet<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `RagfairConfig.cs:481-503` `UnreasonableModPrices`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnreasonableModPricesWire {
    pub enabled: bool,
    pub handbook_price_over_multiplier: i32,
    pub new_price_handbook_multiplier: i32,
    pub item_type: Option<String>,
    #[serde(flatten)]
    pub extra: Extra,
}

/// `RagfairConfig.cs:241-278` `GenerateFleaPrices`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerateFleaPricesWire {
    pub use_handbook_price: bool,
    pub price_multiplier: f64,
    pub prevent_price_being_below_trader_buy_price: bool,
    pub item_tpl_multiplier_override: IndexMap<String, f64>,
    pub item_type_multiplier_override: IndexMap<String, f64>,
    pub use_hideout_craft_multiplier: bool,
    pub hideout_craft_multiplier: f64,
    pub generate_preset_price_by_children: bool,
    #[serde(flatten)]
    pub extra: Extra,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DynamicOffersResult {
    pub offers: Vec<RagfairOfferWire>,
    /// Template ids whose `CanSellOnRagfair` the custom-blacklist arm of `IsItemValidRagfairItem`
    /// (`RagfairServerHelper.cs:61`) set to `false`. The caller replays these onto the live
    /// `templateTable`; nothing else in this port mutates the database.
    pub rejected_can_sell_templates: Vec<String>,
    pub diagnostics: Vec<Diagnostic>,
}

/// The non-offer sections of a framed response — everything except the offer frames.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DynamicOffersHeader {
    pub rejected_can_sell_templates: Vec<String>,
    pub diagnostics: Vec<Diagnostic>,
}

/// `Models/Eft/Ragfair/RagfairOffer.cs:8-91`, only the members `CreateOffer` (`:118-138`) sets.
/// `sellResult`, `unlimitedCount`, `buyRestrictionMax`, `buyRestrictionCurrent` are never set on
/// this path and are omitted rather than sent as null.
#[derive(Debug, Serialize)]
pub struct RagfairOfferWire {
    #[serde(rename = "_id")]
    pub id: String,
    #[serde(rename = "intId")]
    pub internal_id: i32,
    pub user: RagfairOfferUserWire,
    pub root: String,
    pub items: Vec<Item>,
    #[serde(rename = "itemsCost")]
    pub items_cost: f64,
    pub requirements: Vec<OfferRequirementWire>,
    #[serde(rename = "requirementsCost")]
    pub requirements_cost: f64,
    #[serde(rename = "summaryCost")]
    pub summary_cost: f64,
    #[serde(rename = "startTime")]
    pub start_time: i64,
    #[serde(rename = "endTime")]
    pub end_time: i64,
    #[serde(rename = "loyaltyLevel")]
    pub loyalty_level: i32,
    #[serde(rename = "sellInOnePiece")]
    pub sell_in_one_piece: bool,
    pub locked: bool,
    pub quantity: i32,
}

/// `RagfairOffer.cs:111-140`. `memberType` is the numeric `MemberCategory` (`Default` = 0) —
/// `EftEnumConverter` writes enums as numbers, so this must stay an integer on the wire.
#[derive(Debug, Serialize)]
pub struct RagfairOfferUserWire {
    pub id: String,
    pub nickname: Option<String>,
    pub rating: f64,
    #[serde(rename = "memberType")]
    pub member_type: i32,
    pub avatar: Option<String>,
    #[serde(rename = "isRatingGrowing")]
    pub is_rating_growing: bool,
    pub aid: i32,
}

/// `RagfairOffer.cs:93-109`. `level`/`side` are only set for dogtag barters, which the dynamic
/// path never produces (`CreateOffer:97-101` reads `barter.Level`, always null here) — they are
/// `Option` so the wire stays faithful if that ever changes.
#[derive(Debug, Serialize)]
pub struct OfferRequirementWire {
    #[serde(rename = "_tpl")]
    pub template_id: String,
    pub count: f64,
    #[serde(rename = "onlyFunctional")]
    pub only_functional: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub side: Option<i32>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `RagfairConfig.Dynamic` in full, including the three members the wire type leaves to
    /// `extra` (`purchasesAreFoundInRaid`, `expiredOfferThreshold`, `itemPriceOverrideRouble`) and
    /// one mod-added key.
    const DYNAMIC_JSON: &str = r#"{
        "purchasesAreFoundInRaid":true,
        "useTraderPriceForOffersIfHigher":true,
        "barter":{"chancePercent":50.0,"itemCountMin":1,"itemCountMax":3,
            "priceRangeVariancePercent":15.0,"minRoubleCostToBecomeBarter":15000.0,
            "makeSingleStackOnly":false,"itemTplBlacklist":["aaaaaaaaaaaaaaaaaaaaaaaa"],
            "itemTypeBlacklist":["bbbbbbbbbbbbbbbbbbbbbbbb"]},
        "pack":{"chancePercent":5.0,"itemCountMin":2,"itemCountMax":10,
            "itemTypeWhitelist":["cccccccccccccccccccccccc"]},
        "offerAdjustment":{"adjustPriceWhenBelowHandbookPrice":true,
            "maxPriceDifferenceBelowHandbookPercent":70.0,"handbookPriceMultiplier":1.5,
            "priceThresholdRub":6000.0},
        "expiredOfferThreshold":1500,
        "offerItemCount":{"default":{"min":2,"max":5}},
        "priceRanges":{"default":{"min":0.8,"max":1.2},"preset":{"min":0.9,"max":1.4},
            "pack":{"min":0.5,"max":0.8}},
        "showDefaultPresetsOnly":false,
        "ignoreQualityPriceVarianceBlacklist":["dddddddddddddddddddddddd"],
        "endTimeSeconds":{"min":1000,"max":36000},
        "condition":{"eeeeeeeeeeeeeeeeeeeeeeee":{"conditionChance":0.2,
            "current":{"min":0.6,"max":1.0},"max":{"min":0.7,"max":1.0},"_name":"weapons"}},
        "stackablePercent":{"min":10.0,"max":100.0},
        "nonStackableCount":{"min":1,"max":2},
        "rating":{"min":0.2,"max":0.95},
        "armor":{"removeRemovablePlateChance":30,
            "plateSlotIdToRemovePool":["front_plate","back_plate"]},
        "itemPriceMultiplier":{"ffffffffffffffffffffffff":1.5},
        "offerCurrencyChancePercent":{"5449016a4bdc2d6f028b456f":75.0},
        "showAsSingleStack":["111111111111111111111111"],
        "removeSeasonalItemsWhenNotInEvent":true,
        "blacklist":{"damagedAmmoPacks":true,"custom":["222222222222222222222222"],
            "enableBsgList":true,"enableQuestList":true,"traderItems":false,
            "armorPlate":{"maxProtectionLevel":3,"ignoreSlots":["helmet_top"]},
            "enableCustomItemCategoryList":true,
            "customItemCategoryList":["333333333333333333333333"]},
        "unreasonableModPrices":{"444444444444444444444444":{"enabled":true,
            "handbookPriceOverMultiplier":10,"newPriceHandbookMultiplier":4,"itemType":"mod"}},
        "itemPriceOverrideRouble":{"555555555555555555555555":1000.0},
        "generateBaseFleaPrices":{"useHandbookPrice":true,"priceMultiplier":1.1,
            "preventPriceBeingBelowTraderBuyPrice":true,
            "itemTplMultiplierOverride":{"666666666666666666666666":2.0},
            "itemTypeMultiplierOverride":{"777777777777777777777777":3.0},
            "useHideoutCraftMultiplier":false,"hideoutCraftMultiplier":1.0,
            "generatePresetPriceByChildren":true},
        "modAddedDynamicField":42
    }"#;

    /// Every member of the request but `dynamic`, which is spliced in from [`DYNAMIC_JSON`].
    const REQUEST_TAIL: &str = r#"
        "itemPresets":{"preset1":{"items":[{"_id":"aaaaaaaaaaaaaaaaaaaaaaaa",
            "_tpl":"bbbbbbbbbbbbbbbbbbbbbbbb"}],"id":"preset1","name":"AK",
            "encyclopedia":"bbbbbbbbbbbbbbbbbbbbbbbb"}},
        "defaultPresets":[{"items":[{"_id":"aaaaaaaaaaaaaaaaaaaaaaaa",
            "_tpl":"bbbbbbbbbbbbbbbbbbbbbbbb"}],"id":"preset1"}],
        "defaultPresetsByTpl":{"bbbbbbbbbbbbbbbbbbbbbbbb":{"items":[],"id":"preset1"}},
        "presetsByTpl":{"bbbbbbbbbbbbbbbbbbbbbbbb":[{"items":[],"id":"preset1"}]},
        "fleaPrices":{"bbbbbbbbbbbbbbbbbbbbbbbb":25000.0,"cccccccccccccccccccccccc":100.0},
        "handbookPrices":{"bbbbbbbbbbbbbbbbbbbbbbbb":20000.0},
        "highestTraderPrices":{"bbbbbbbbbbbbbbbbbbbbbbbb":12000.0},
        "configBlacklist":["999999999999999999999999"],
        "seasonalEventActive":false,
        "seasonalItemTplBlacklist":["888888888888888888888888"],
        "pmcNamesUsec":["Deagle"],
        "pmcNamesBear":["Kirill"],
        "items":{"bbbbbbbbbbbbbbbbbbbbbbbb":{"parent":"cccccccccccccccccccccccc",
            "stackMaxSize":1,"durability":100.0,"maximumNumberOfUsage":10,
            "maxRepairResource":1200,"canSellOnRagfair":true}}
    "#;

    fn request_json(head: &str) -> String {
        format!("{{{head},\"dynamic\":{DYNAMIC_JSON},{REQUEST_TAIL}}}")
    }

    #[test]
    fn request_deserializes_every_top_level_field() {
        let json = request_json(
            r#""testSeed":42,"timestamp":1700000000,"offerCounterStart":7,
               "expiredOffers":[[{"_id":"aaaaaaaaaaaaaaaaaaaaaaaa","_tpl":"bbbbbbbbbbbbbbbbbbbbbbbb"}]]"#,
        );
        let parsed: GenerateDynamicOffersRequest = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.test_seed, Some(42));
        assert_eq!(parsed.timestamp, 1_700_000_000);
        assert_eq!(parsed.offer_counter_start, 7);
        assert_eq!(
            parsed.expired_offers.as_ref().unwrap()[0][0].template,
            "bbbbbbbbbbbbbbbbbbbbbbbb"
        );
        assert_eq!(
            parsed.item_presets["preset1"].encyclopedia.as_deref(),
            Some("bbbbbbbbbbbbbbbbbbbbbbbb")
        );
        assert_eq!(parsed.default_presets.len(), 1);
        assert!(
            parsed
                .default_presets_by_tpl
                .contains_key("bbbbbbbbbbbbbbbbbbbbbbbb")
        );
        assert_eq!(parsed.presets_by_tpl["bbbbbbbbbbbbbbbbbbbbbbbb"].len(), 1);
        // Insertion order is load-bearing: `GetFleaPricesAsArray` draws by index
        assert_eq!(
            parsed.flea_prices.keys().collect::<Vec<_>>(),
            vec!["bbbbbbbbbbbbbbbbbbbbbbbb", "cccccccccccccccccccccccc"]
        );
        assert_eq!(parsed.handbook_prices["bbbbbbbbbbbbbbbbbbbbbbbb"], 20000.0);
        assert_eq!(
            parsed.highest_trader_prices["bbbbbbbbbbbbbbbbbbbbbbbb"],
            12000.0
        );
        assert_eq!(parsed.config_blacklist, vec!["999999999999999999999999"]);
        assert!(!parsed.seasonal_event_active);
        assert_eq!(
            parsed.seasonal_item_tpl_blacklist,
            vec!["888888888888888888888888"]
        );
        assert_eq!(parsed.pmc_names_usec, vec!["Deagle"]);
        assert_eq!(parsed.pmc_names_bear, vec!["Kirill"]);

        // The four `ItemView` members this port added
        let item = &parsed.items["bbbbbbbbbbbbbbbbbbbbbbbb"];
        assert_eq!(item.durability, Some(100.0));
        assert_eq!(item.maximum_number_of_usage, Some(10));
        assert_eq!(item.max_repair_resource, Some(1200.0));
        assert_eq!(item.can_sell_on_ragfair, Some(true));
    }

    #[test]
    fn dynamic_config_deserializes_every_nested_record() {
        let json = request_json(r#""testSeed":null,"timestamp":1,"offerCounterStart":0"#);
        let dynamic = serde_json::from_str::<GenerateDynamicOffersRequest>(&json)
            .unwrap()
            .dynamic;

        assert!(dynamic.use_trader_price_for_offers_if_higher);
        assert_eq!(dynamic.barter.chance_percent, 50.0);
        assert_eq!(dynamic.barter.item_count_min, 1);
        assert_eq!(dynamic.barter.item_count_max, 3);
        assert_eq!(dynamic.barter.price_range_variance_percent, 15.0);
        assert_eq!(dynamic.barter.min_rouble_cost_to_become_barter, 15000.0);
        assert!(!dynamic.barter.make_single_stack_only);
        assert!(
            dynamic
                .barter
                .item_tpl_blacklist
                .contains("aaaaaaaaaaaaaaaaaaaaaaaa")
        );
        assert!(
            dynamic
                .barter
                .item_type_blacklist
                .contains("bbbbbbbbbbbbbbbbbbbbbbbb")
        );
        assert_eq!(dynamic.pack.chance_percent, 5.0);
        assert_eq!(dynamic.pack.item_count_min, 2);
        assert_eq!(dynamic.pack.item_count_max, 10);
        assert!(
            dynamic
                .pack
                .item_type_whitelist
                .contains("cccccccccccccccccccccccc")
        );
        assert!(
            dynamic
                .offer_adjustment
                .adjust_price_when_below_handbook_price
        );
        assert_eq!(
            dynamic
                .offer_adjustment
                .max_price_difference_below_handbook_percent,
            70.0
        );
        assert_eq!(dynamic.offer_adjustment.handbook_price_multiplier, 1.5);
        assert_eq!(dynamic.offer_adjustment.price_threshold_rub, 6000.0);
        assert_eq!(dynamic.offer_item_count["default"].min, 2);
        assert_eq!(dynamic.offer_item_count["default"].max, 5);
        assert_eq!(dynamic.price_ranges.default.max, 1.2);
        assert_eq!(dynamic.price_ranges.preset.min, 0.9);
        assert_eq!(dynamic.price_ranges.pack.max, 0.8);
        assert!(!dynamic.show_default_presets_only);
        assert_eq!(dynamic.ignore_quality_price_variance_blacklist.len(), 1);
        assert_eq!(dynamic.end_time_seconds.max, 36000);
        let condition = &dynamic.condition["eeeeeeeeeeeeeeeeeeeeeeee"];
        assert_eq!(condition.condition_chance, 0.2);
        assert_eq!(condition.current.min, 0.6);
        assert_eq!(condition.max.min, 0.7);
        assert_eq!(condition.extra["_name"], "weapons");
        assert_eq!(dynamic.stackable_percent.max, 100.0);
        assert_eq!(dynamic.non_stackable_count.max, 2);
        assert_eq!(dynamic.rating.min, 0.2);
        assert_eq!(dynamic.armor.remove_removable_plate_chance, 30);
        assert_eq!(
            dynamic
                .armor
                .plate_slot_id_to_remove_pool
                .iter()
                .collect::<Vec<_>>(),
            vec!["front_plate", "back_plate"]
        );
        assert_eq!(
            dynamic.item_price_multiplier.as_ref().unwrap()["ffffffffffffffffffffffff"],
            1.5
        );
        // C# wire name is `offerCurrencyChancePercent`, the member is `OfferCurrencyChangePercent`
        assert_eq!(
            dynamic.offer_currency_change_percent["5449016a4bdc2d6f028b456f"],
            75.0
        );
        assert_eq!(
            dynamic.show_as_single_stack,
            vec!["111111111111111111111111"]
        );
        assert!(dynamic.remove_seasonal_items_when_not_in_event);
        assert!(dynamic.blacklist.damaged_ammo_packs);
        assert!(
            dynamic
                .blacklist
                .custom
                .contains("222222222222222222222222")
        );
        assert!(dynamic.blacklist.enable_bsg_list);
        assert!(dynamic.blacklist.enable_quest_list);
        assert!(!dynamic.blacklist.trader_items);
        assert_eq!(dynamic.blacklist.armor_plate.max_protection_level, 3);
        assert!(
            dynamic
                .blacklist
                .armor_plate
                .ignore_slots
                .contains("helmet_top")
        );
        assert!(dynamic.blacklist.enable_custom_item_category_list);
        assert!(
            dynamic
                .blacklist
                .custom_item_category_list
                .contains("333333333333333333333333")
        );
        let unreasonable = &dynamic.unreasonable_mod_prices["444444444444444444444444"];
        assert!(unreasonable.enabled);
        assert_eq!(unreasonable.handbook_price_over_multiplier, 10);
        assert_eq!(unreasonable.new_price_handbook_multiplier, 4);
        assert_eq!(unreasonable.item_type.as_deref(), Some("mod"));
        let flea_prices = &dynamic.generate_base_flea_prices;
        assert!(flea_prices.use_handbook_price);
        assert_eq!(flea_prices.price_multiplier, 1.1);
        assert!(flea_prices.prevent_price_being_below_trader_buy_price);
        assert_eq!(
            flea_prices.item_tpl_multiplier_override["666666666666666666666666"],
            2.0
        );
        assert_eq!(
            flea_prices.item_type_multiplier_override["777777777777777777777777"],
            3.0
        );
        assert!(!flea_prices.use_hideout_craft_multiplier);
        assert_eq!(flea_prices.hideout_craft_multiplier, 1.0);
        assert!(flea_prices.generate_preset_price_by_children);
    }

    #[test]
    fn null_seed_and_absent_expired_offers_are_none() {
        let json = request_json(r#""testSeed":null,"timestamp":1,"offerCounterStart":0"#);
        let parsed: GenerateDynamicOffersRequest = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed.test_seed, None);
        assert!(parsed.expired_offers.is_none());
    }

    #[test]
    fn unknown_keys_land_in_the_passthrough_maps_and_round_trip() {
        let json = request_json(
            r#""timestamp":1,"offerCounterStart":0,
               "expiredOffers":[[{"_id":"aaaaaaaaaaaaaaaaaaaaaaaa","_tpl":"bbbbbbbbbbbbbbbbbbbbbbbb",
                                  "modFieldFromAMod":7}]]"#,
        );
        let parsed: GenerateDynamicOffersRequest = serde_json::from_str(&json).unwrap();

        // Config passthrough: members the wire type does not name stay reachable
        assert_eq!(parsed.dynamic.extra["modAddedDynamicField"], 42);
        assert_eq!(parsed.dynamic.extra["purchasesAreFoundInRaid"], true);
        assert_eq!(parsed.dynamic.extra["expiredOfferThreshold"], 1500);

        // Game-data passthrough: an unknown item key survives back out through an offer
        let items = parsed.expired_offers.unwrap().remove(0);
        assert_eq!(items[0].extra["modFieldFromAMod"], 7);
        let out = serde_json::to_value(offer_with(items.clone())).unwrap();
        assert_eq!(out["items"][0]["modFieldFromAMod"], 7);

        // ...and out through MessagePack too: `#[serde(flatten)]` serializes as an unknown-length
        // map, which `to_vec_named` must accept for the framed payloads to carry mod fields.
        let packed = rmp_serde::to_vec_named(&offer_with(items)).unwrap();
        let out: serde_json::Value = rmp_serde::from_slice(&packed).unwrap();
        assert_eq!(out["items"][0]["modFieldFromAMod"], 7);
    }

    #[test]
    fn result_serializes_the_expected_wire_names() {
        let result = DynamicOffersResult {
            offers: vec![offer_with(vec![Item {
                id: "aaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
                template: "bbbbbbbbbbbbbbbbbbbbbbbb".to_owned(),
                ..Default::default()
            }])],
            rejected_can_sell_templates: vec!["cccccccccccccccccccccccc".to_owned()],
            diagnostics: vec![],
        };
        let out = serde_json::to_value(&result).unwrap();

        assert_eq!(
            out.as_object().unwrap().keys().collect::<Vec<_>>(),
            vec!["offers", "rejectedCanSellTemplates", "diagnostics"]
        );
        let offer = &out["offers"][0];
        assert_eq!(
            offer.as_object().unwrap().keys().collect::<Vec<_>>(),
            vec![
                "_id",
                "intId",
                "user",
                "root",
                "items",
                "itemsCost",
                "requirements",
                "requirementsCost",
                "summaryCost",
                "startTime",
                "endTime",
                "loyaltyLevel",
                "sellInOnePiece",
                "locked",
                "quantity"
            ]
        );
        assert_eq!(
            offer["user"]
                .as_object()
                .unwrap()
                .keys()
                .collect::<Vec<_>>(),
            vec![
                "id",
                "nickname",
                "rating",
                "memberType",
                "avatar",
                "isRatingGrowing",
                "aid"
            ]
        );
        // `memberType` stays numeric — `EftEnumConverter` writes enums as numbers
        assert_eq!(offer["user"]["memberType"], 0);
        // Dogtag-only members are absent, not null
        assert_eq!(
            offer["requirements"][0]
                .as_object()
                .unwrap()
                .keys()
                .collect::<Vec<_>>(),
            vec!["_tpl", "count", "onlyFunctional"]
        );
    }

    fn offer_with(items: Vec<Item>) -> RagfairOfferWire {
        RagfairOfferWire {
            id: "dddddddddddddddddddddddd".to_owned(),
            internal_id: 1,
            user: RagfairOfferUserWire {
                id: "eeeeeeeeeeeeeeeeeeeeeeee".to_owned(),
                nickname: Some("Deagle".to_owned()),
                rating: 0.5,
                member_type: 0,
                avatar: None,
                is_rating_growing: true,
                aid: 1234,
            },
            root: "aaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
            items,
            items_cost: 100.0,
            requirements: vec![OfferRequirementWire {
                template_id: "5449016a4bdc2d6f028b456f".to_owned(),
                count: 250.0,
                only_functional: false,
                level: None,
                side: None,
            }],
            requirements_cost: 250.0,
            summary_cost: 250.0,
            start_time: 1,
            end_time: 2,
            loyalty_level: 1,
            sell_in_one_piece: false,
            locked: false,
            quantity: 1,
        }
    }
}
