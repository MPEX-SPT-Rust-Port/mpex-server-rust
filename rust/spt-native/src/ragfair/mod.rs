pub mod assort_generator;
pub mod models;
pub mod offer_generator;
pub mod price_service;
pub mod server_helper;
pub mod slice_cache;

use std::collections::HashSet;

use indexmap::IndexMap;

use crate::loot::item_helper::ItemBaseClassCache;
use crate::loot::models::{Diagnostic, ItemView, PresetView};
use crate::ragfair::models::DynamicConfigWire;

/// The read-only views one dynamic ragfair pass consults, plus the diagnostics the C# caller
/// replays through its logger — the ragfair family's analog of [`crate::bot::BotContext`].
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
    pub diagnostics: Vec<Diagnostic>,
}

impl<'a> RagfairContext<'a> {
    /// A worker's view of the same pass: every shared reference copied, a fresh diagnostics
    /// buffer of its own — what lets the batch walk fan out without sharing `&mut self`.
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
            seasonal_item_tpl_blacklist: self.seasonal_item_tpl_blacklist,
            pmc_names_usec: self.pmc_names_usec,
            pmc_names_bear: self.pmc_names_bear,
            timestamp: self.timestamp,
            seasonal_event_active: self.seasonal_event_active,
            diagnostics: Vec::new(),
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

/// The [`Diagnostic`] constructor the ragfair modules share. The bot modules re-declare their own
/// per file; here it lives once and is imported.
pub(crate) fn plain(level: &str, message: String) -> Diagnostic {
    Diagnostic {
        level: level.to_owned(),
        locale_key: None,
        args: None,
        message: Some(message),
    }
}
