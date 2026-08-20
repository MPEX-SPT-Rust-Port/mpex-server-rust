pub mod assort_generator;
pub mod models;
pub mod offer_generator;
pub mod price_service;
pub mod server_helper;
pub mod views;

use std::collections::HashSet;
use std::sync::{Arc, LazyLock};

use indexmap::{IndexMap, IndexSet};

use crate::db::models::{ConfigsRoot, ItemConfigLift, RagfairConfigLift};
use crate::diag::DiagSink;
use crate::loot::item_helper::ItemBaseClassCache;
use crate::loot::models::{ItemView, PresetView};
use crate::ragfair::models::DynamicConfigWire;

/// What the resident arm reads for `customMoneyTpls` when the configs root carries no
/// `spt-inventory` stem: nothing, which is exactly what the offer path saw before the lift. Also
/// the stand-in for every fixture that exercises no custom currency.
pub(crate) static NO_CUSTOM_MONEY_TPLS: LazyLock<IndexSet<String>> = LazyLock::new(IndexSet::new);

/// The config-backed half of a dynamic ragfair pass — `RagfairConfig.Dynamic`,
/// `ItemConfig.Blacklist` and `InventoryConfig.CustomMoneyTpls`. The override arm owns the values
/// the C# builder sent for this call only; the resident arm holds the configs root itself rather
/// than copying sets out of it per call.
pub enum RagfairConfigs {
    Override {
        dynamic: Box<DynamicConfigWire>,
        config_blacklist: HashSet<String>,
        custom_money_tpls: IndexSet<String>,
    },
    /// The resident configs root. [`offer_generator::generate_dynamic_offers`] has already proved
    /// both stems this family requires are present, so the accessors below cannot miss one.
    Resident(Arc<ConfigsRoot>),
}

impl RagfairConfigs {
    pub(crate) fn dynamic(&self) -> &DynamicConfigWire {
        match self {
            Self::Override { dynamic, .. } => dynamic,
            Self::Resident(configs) => &ragfair_config(configs).dynamic,
        }
    }

    pub(crate) fn config_blacklist(&self) -> &HashSet<String> {
        match self {
            Self::Override {
                config_blacklist, ..
            } => config_blacklist,
            Self::Resident(configs) => &item_config(configs).blacklist,
        }
    }

    /// Soft where the other two are strict: a configs root with no `spt-inventory` stem answers the
    /// empty set rather than failing the call (see [`crate::db::models::InventoryConfigLift`]).
    pub(crate) fn custom_money_tpls(&self) -> &IndexSet<String> {
        match self {
            Self::Override {
                custom_money_tpls, ..
            } => custom_money_tpls,
            Self::Resident(configs) => configs
                .inventory
                .as_ref()
                .map_or(&NO_CUSTOM_MONEY_TPLS, |inventory| {
                    &inventory.custom_money_tpls
                }),
        }
    }
}

/// The `spt-ragfair` stem, present because [`offer_generator::generate_dynamic_offers`] refused the
/// request without it.
fn ragfair_config(configs: &ConfigsRoot) -> &RagfairConfigLift {
    configs
        .ragfair
        .as_ref()
        .expect("generate_dynamic_offers proved the spt-ragfair stem present")
}

/// The `spt-item` stem, present for the same reason.
fn item_config(configs: &ConfigsRoot) -> &ItemConfigLift {
    configs
        .item
        .as_ref()
        .expect("generate_dynamic_offers proved the spt-item stem present")
}

/// The read-only views one dynamic ragfair pass consults, plus the [`DiagSink`] its diagnostics
/// emit through — the ragfair family's analog of [`crate::bot::BotContext`].
///
/// Every view is borrowed for `'a`, so copying one out (`let items = ctx.items;`) releases the
/// `&mut ctx` and leaves the diagnostics writable.
pub struct RagfairContext<'a> {
    /// The `TemplateItem` slice, flattened by the C# caller. There is no `ItemsView` type; the map
    /// itself is the view, matching `loot::item_helper`'s helpers.
    pub items: &'a IndexMap<String, ItemView>,
    /// [`ItemBaseClassCache`] over [`Self::items`] — what `ItemHelper.IsOfBaseclass(es)` answers
    /// from in C# (`ItemBaseClassService`), so the ported call sites probe it instead of walking.
    pub base_classes: &'a ItemBaseClassCache,
    /// `RagfairConfig.Dynamic`, whole.
    pub dynamic: &'a DynamicConfigWire,
    /// `GlobalTable.ItemPresets`, keyed by preset `_id` — what `PresetHelper.IsPreset`/`GetPreset`
    /// read, so `IsPresetBaseClass` resolves through it.
    pub item_presets: &'a IndexMap<String, PresetView>,
    /// `PresetHelper.GetDefaultPresets().Values.ToList()` — the assort walk's preset source when
    /// `showDefaultPresetsOnly` is set.
    pub default_presets: &'a [PresetView],
    /// `PresetHelper.GetDefaultPresetByTpl()`, keyed by the tpl the preset is the default for —
    /// what `GetDefaultPreset(tpl)` answers from.
    pub default_presets_by_tpl: &'a IndexMap<String, PresetView>,
    /// `PresetHelper.GetPresets(tpl)` resolved for every tpl that has presets. Order is
    /// load-bearing: the fallback arm of `GetWeaponPreset` takes element `0`.
    pub presets_by_tpl: &'a IndexMap<String, Vec<PresetView>>,
    /// `templateTable.Prices` — the flea base price table, insertion ordered because
    /// `GetFleaPricesAsArray` draws from it by index.
    pub flea_prices: &'a IndexMap<String, f64>,
    /// `HandbookHelper.GetTemplatePrice` for the whole items table.
    pub handbook_prices: &'a IndexMap<String, f64>,
    /// `TraderHelper.GetHighestSellToTraderPrice` resolved per template.
    pub highest_trader_prices: &'a IndexMap<String, f64>,
    /// `ItemFilterService.GetBlacklistedItems()` — read by `ItemHelper.IsValidItem`.
    pub config_blacklist: &'a HashSet<String>,
    /// `InventoryConfig.CustomMoneyTpls` — the currencies `PaymentHelper.IsMoneyTpl` unions onto
    /// the four `Money` constants (`PaymentHelper.cs:19-33`).
    pub custom_money_tpls: &'a IndexSet<String>,
    /// `SeasonalEventService.GetInactiveSeasonalEventItems()`.
    pub seasonal_item_tpl_blacklist: &'a HashSet<String>,
    /// `BotHelper.GatherPmcNamesOfLength(Usec)`, pre-filtered by the C# caller.
    pub pmc_names_usec: &'a [String],
    /// `BotHelper.GatherPmcNamesOfLength(Bear)`, pre-filtered by the C# caller.
    pub pmc_names_bear: &'a [String],
    /// `TimeUtil.GetTimeStamp()` taken once for the batch.
    pub timestamp: i64,
    /// `SeasonalEventService.SeasonalEventEnabled()`.
    pub seasonal_event_active: bool,
    pub diagnostics: DiagSink,
}

impl<'a> RagfairContext<'a> {
    /// A worker's view of the same pass: every shared reference copied, a forked sink of its own
    /// — what lets the batch walk fan out without sharing `&mut self`.
    pub fn fork(&self) -> RagfairContext<'a> {
        RagfairContext {
            items: self.items,
            base_classes: self.base_classes,
            dynamic: self.dynamic,
            item_presets: self.item_presets,
            default_presets: self.default_presets,
            default_presets_by_tpl: self.default_presets_by_tpl,
            presets_by_tpl: self.presets_by_tpl,
            flea_prices: self.flea_prices,
            handbook_prices: self.handbook_prices,
            highest_trader_prices: self.highest_trader_prices,
            config_blacklist: self.config_blacklist,
            custom_money_tpls: self.custom_money_tpls,
            seasonal_item_tpl_blacklist: self.seasonal_item_tpl_blacklist,
            pmc_names_usec: self.pmc_names_usec,
            pmc_names_bear: self.pmc_names_bear,
            timestamp: self.timestamp,
            seasonal_event_active: self.seasonal_event_active,
            diagnostics: self.diagnostics.fork(),
        }
    }
}

/// Empty stand-ins for the views a fixture that exercises none of them still has to supply.
#[cfg(test)]
pub(crate) static NO_BLACKLIST: std::sync::LazyLock<HashSet<String>> =
    std::sync::LazyLock::new(HashSet::new);
#[cfg(test)]
pub(crate) static NO_NAMES: std::sync::LazyLock<Vec<String>> = std::sync::LazyLock::new(Vec::new);
#[cfg(test)]
pub(crate) static NO_DEFAULT_PRESETS: std::sync::LazyLock<Vec<PresetView>> =
    std::sync::LazyLock::new(Vec::new);
