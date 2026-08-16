pub mod helper;
pub mod models;
pub mod reward_generator;
pub mod slice_cache;

use std::collections::HashSet;

use indexmap::IndexMap;

use crate::loot::models::{Diagnostic, ItemView, PresetView};
use crate::quest::models::{
    ExitView, LevelledItemFilter, QuestInvariantSlice, RepeatableQuestTemplates,
    RepeatableTemplates,
};

/// The read-only views one repeatable-quest pass consults, plus the diagnostics the C# caller
/// replays through its logger — the quest family's analog of [`crate::ragfair::RagfairContext`].
///
/// Every view is borrowed for `'a` off the cached [`QuestInvariantSlice`], so copying one out
/// (`let items = ctx.items;`) releases the `&mut ctx` and leaves the diagnostics writable.
pub struct QuestContext<'a> {
    pub items: &'a IndexMap<String, ItemView>,
    pub handbook_prices: &'a IndexMap<String, f64>,
    pub flea_prices: &'a IndexMap<String, f64>,
    pub default_weapon_presets: &'a [PresetView],
    pub default_preset_or_item_prices: &'a IndexMap<String, f64>,
    pub item_blacklist: &'a HashSet<String>,
    pub reward_item_blacklist: &'a HashSet<String>,
    pub boss_items: &'a HashSet<String>,
    pub seasonal_item_tpl_blacklist: &'a HashSet<String>,
    pub repeatable_quest_templates: &'a RepeatableTemplates,
    pub completion_items_whitelist: &'a [LevelledItemFilter],
    pub completion_items_blacklist: &'a [LevelledItemFilter],
    pub boss_spawns_by_location: &'a IndexMap<String, Vec<String>>,
    pub extracts_by_location: &'a IndexMap<String, Vec<ExitView>>,
    pub repeatable_quest_template_ids: &'a RepeatableQuestTemplates,
    pub location_id_map: &'a IndexMap<String, String>,
    pub diagnostics: Vec<Diagnostic>,
}

impl<'a> QuestContext<'a> {
    /// Borrow every view off `slice` and start a fresh diagnostics buffer — one pass's context.
    pub fn from_slice(slice: &'a QuestInvariantSlice) -> Self {
        QuestContext {
            items: &slice.items,
            handbook_prices: &slice.handbook_prices,
            flea_prices: &slice.flea_prices,
            default_weapon_presets: &slice.default_weapon_presets,
            default_preset_or_item_prices: &slice.default_preset_or_item_prices,
            item_blacklist: &slice.item_blacklist,
            reward_item_blacklist: &slice.reward_item_blacklist,
            boss_items: &slice.boss_items,
            seasonal_item_tpl_blacklist: &slice.seasonal_item_tpl_blacklist,
            repeatable_quest_templates: &slice.repeatable_quest_templates,
            completion_items_whitelist: &slice.completion_items_whitelist,
            completion_items_blacklist: &slice.completion_items_blacklist,
            boss_spawns_by_location: &slice.boss_spawns_by_location,
            extracts_by_location: &slice.extracts_by_location,
            repeatable_quest_template_ids: &slice.repeatable_quest_template_ids,
            location_id_map: &slice.location_id_map,
            diagnostics: Vec::new(),
        }
    }
}
