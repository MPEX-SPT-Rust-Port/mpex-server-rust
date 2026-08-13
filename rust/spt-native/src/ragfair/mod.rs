pub mod models;
pub mod price_service;

use std::collections::HashSet;

use indexmap::IndexMap;

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

/// Empty stand-ins for the views a fixture that exercises none of them still has to supply.
#[cfg(test)]
pub(crate) static NO_BLACKLIST: std::sync::LazyLock<HashSet<String>> =
    std::sync::LazyLock::new(HashSet::new);
#[cfg(test)]
pub(crate) static NO_NAMES: std::sync::LazyLock<Vec<String>> = std::sync::LazyLock::new(Vec::new);
#[cfg(test)]
pub(crate) static NO_DEFAULT_PRESETS: std::sync::LazyLock<Vec<PresetView>> =
    std::sync::LazyLock::new(Vec::new);

/// The two [`Diagnostic`] constructors the ragfair modules share. The bot modules re-declare their
/// own per file; here they live once and are imported.
pub(crate) fn plain(level: &str, message: String) -> Diagnostic {
    Diagnostic {
        level: level.to_owned(),
        locale_key: None,
        args: None,
        message: Some(message),
    }
}

pub(crate) fn localised(level: &str, locale_key: &str, args: serde_json::Value) -> Diagnostic {
    Diagnostic {
        level: level.to_owned(),
        locale_key: Some(locale_key.to_owned()),
        args: Some(args),
        message: None,
    }
}
