pub mod completion;
pub mod elimination;
pub mod exploration;
pub mod helper;
pub mod models;
pub mod pickup;
pub mod reward_generator;
pub mod slice_cache;

use std::any::Any;
use std::collections::HashSet;
use std::panic::{AssertUnwindSafe, catch_unwind};

use indexmap::IndexMap;

use crate::loot::models::{Diagnostic, ItemView, PresetView};
use crate::loot::random_util::TestSeedGuard;
use crate::quest::models::{
    ExitView, LevelledItemFilter, QuestInvariantSlice, QuestNativeRequest, QuestNativeResponse,
    QuestVaryingRequest, RepeatableQuestTemplates, RepeatableQuestType, RepeatableTemplates,
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

/// What a repeatable-quest pass can fail with: a C#-sanctioned throw, ported as a panic and caught
/// here so its message crosses the boundary the way every other family's error message does, or a
/// slice-less request naming a stamp this process has not stored.
#[derive(Debug)]
pub enum QuestError {
    Failed(String),
    StaleSlice,
}

/// `RepeatableQuestController.GenerateRepeatableQuest` (`:390-404`) — one quest of the requested
/// type, plus the pool the generator mutated on the way.
///
/// A `null` quest is a normal outcome, not a failure: the pool can be exhausted, or a generator can
/// give up and log why. The mutated pool and the diagnostics ride back either way.
///
/// # Errors
///
/// [`QuestError::StaleSlice`] when a slice-less request names a stamp the cache does not hold, or
/// [`QuestError::Failed`] carrying the message of a generator's C#-sanctioned throw.
pub fn generate_repeatable_quest(
    request: QuestNativeRequest,
) -> Result<QuestNativeResponse, QuestError> {
    let slice = slice_cache::take_or_stale(request.invariant_stamp, request.invariant)
        .ok_or(QuestError::StaleSlice)?;

    let QuestVaryingRequest {
        quest_type,
        session_id,
        pmc_level,
        trader_id,
        quest_type_pool: mut pool,
        repeatable_config,
        seed,
    } = request.varying;
    let _seed_guard = seed.map(TestSeedGuard::install);

    // `:396` dispatches on the config's type name. `Pickup` is reachable here but not from the C#
    // caller, whose switch spells the case `PickUp` and falls through to `null` for the pool's
    // `Pickup`.
    let generate = match quest_type {
        RepeatableQuestType::Elimination => elimination::generate,
        RepeatableQuestType::Completion => completion::generate,
        RepeatableQuestType::Exploration => exploration::generate,
        RepeatableQuestType::Pickup => pickup::generate,
    };

    let mut ctx = QuestContext::from_slice(&slice);
    // The generators panic where the C# throws; the message is the failure the caller reports.
    // Diagnostics gathered before the throw are dropped, as they are on every other export.
    let quest = catch_unwind(AssertUnwindSafe(|| {
        generate(
            &mut ctx,
            &session_id,
            pmc_level,
            &trader_id,
            &mut pool,
            &repeatable_config,
        )
    }))
    .map_err(panic_message)?;

    Ok(QuestNativeResponse {
        quest,
        pool,
        diagnostics: ctx.diagnostics,
    })
}

/// The text a caught panic carries — `expect`/`panic!` payloads are a `String` or a `&str`.
fn panic_message(payload: Box<dyn Any + Send>) -> QuestError {
    let message = payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| {
            payload
                .downcast_ref::<&str>()
                .map(|text| (*text).to_owned())
        })
        .unwrap_or_else(|| "repeatable quest generation panicked".to_owned());

    QuestError::Failed(message)
}
