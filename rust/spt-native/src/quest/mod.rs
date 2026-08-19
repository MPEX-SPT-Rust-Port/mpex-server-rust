pub mod completion;
pub mod elimination;
pub mod exploration;
pub mod helper;
pub mod models;
pub mod pickup;
pub mod reward_generator;
pub mod views;

use std::any::Any;
use std::collections::HashSet;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use indexmap::IndexMap;

use crate::diag::DiagSink;
use crate::loot::item_helper::ItemBaseClassCache;
use crate::loot::models::{ItemView, PresetView};
use crate::loot::random_util::TestSeedGuard;
use crate::quest::models::{
    ExitView, LevelledItemFilter, QuestNativeRequest, QuestNativeResponse, QuestVaryingRequest,
    QuestViewsWire, RepeatableQuestTemplates, RepeatableQuestType, RepeatableTemplates,
};
use crate::quest::views::QuestDbViews;

/// The read-only views one repeatable-quest pass consults, plus the [`DiagSink`] its diagnostics
/// emit through — the quest family's analog of [`crate::ragfair::RagfairContext`].
///
/// Every view is borrowed for `'a` off the resolved [`QuestViews`] source and the varying half of
/// the request, so copying one out (`let items = ctx.items;`) releases the `&mut ctx` and leaves
/// the diagnostics writable.
pub struct QuestContext<'a> {
    pub items: &'a IndexMap<String, ItemView>,
    /// [`ItemBaseClassCache`] over [`Self::items`] — what `ItemHelper.IsOfBaseclass(es)` answers
    /// from in C# (`ItemBaseClassService`), so the ported call sites probe it instead of walking.
    pub base_classes: &'a ItemBaseClassCache,
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
    pub diagnostics: DiagSink,
}

/// The resolved database-view source for one pass: the resident DB's derived bundle, or the
/// transient wire override a residency-ineligible caller sent. The override is boxed so the two
/// variants stay pointer-sized.
pub enum QuestViews {
    Resident(Arc<QuestDbViews>),
    Override(Box<QuestViewsWire>),
}

impl<'a> QuestContext<'a> {
    /// Borrow the database views off `views` and the six moved service/config members off
    /// `varying`, emitting straight to the log pipeline — one pass's context.
    pub fn new(views: &'a QuestViews, varying: &'a QuestVaryingRequest) -> Self {
        match views {
            QuestViews::Resident(views) => QuestContext {
                items: &views.ragfair.items,
                base_classes: &views.ragfair.base_classes,
                handbook_prices: &views.ragfair.handbook_prices,
                flea_prices: &views.ragfair.flea_prices,
                default_weapon_presets: &views.default_weapon_presets,
                default_preset_or_item_prices: &views.default_preset_or_item_prices,
                item_blacklist: &varying.item_blacklist,
                reward_item_blacklist: &varying.reward_item_blacklist,
                boss_items: &varying.boss_items,
                seasonal_item_tpl_blacklist: &varying.seasonal_item_tpl_blacklist,
                repeatable_quest_templates: &views.repeatable_quest_templates,
                completion_items_whitelist: &views.completion_items_whitelist,
                completion_items_blacklist: &views.completion_items_blacklist,
                boss_spawns_by_location: &views.boss_spawns_by_location,
                extracts_by_location: &views.extracts_by_location,
                repeatable_quest_template_ids: &varying.repeatable_quest_template_ids,
                location_id_map: &varying.location_id_map,
                diagnostics: DiagSink::Pipeline,
            },
            QuestViews::Override(wire) => QuestContext {
                items: &wire.items,
                base_classes: wire.base_classes(),
                handbook_prices: &wire.handbook_prices,
                flea_prices: &wire.flea_prices,
                default_weapon_presets: &wire.default_weapon_presets,
                default_preset_or_item_prices: &wire.default_preset_or_item_prices,
                item_blacklist: &varying.item_blacklist,
                reward_item_blacklist: &varying.reward_item_blacklist,
                boss_items: &varying.boss_items,
                seasonal_item_tpl_blacklist: &varying.seasonal_item_tpl_blacklist,
                repeatable_quest_templates: &wire.repeatable_quest_templates,
                completion_items_whitelist: &wire.completion_items_whitelist,
                completion_items_blacklist: &wire.completion_items_blacklist,
                boss_spawns_by_location: &wire.boss_spawns_by_location,
                extracts_by_location: &wire.extracts_by_location,
                repeatable_quest_template_ids: &varying.repeatable_quest_template_ids,
                location_id_map: &varying.location_id_map,
                diagnostics: DiagSink::Pipeline,
            },
        }
    }
}

/// What a repeatable-quest pass can fail with: a C#-sanctioned throw, ported as a panic and caught
/// here so its message crosses the boundary the way every other family's error message does, or an
/// override-less request naming a resident-DB epoch this process does not hold.
#[derive(Debug)]
pub enum QuestError {
    Failed(String),
    StaleEpoch,
}

/// `RepeatableQuestController.PickAndGenerateRandomRepeatableQuest` (`:390-397`) — one quest of the
/// requested type, plus the pool the generator mutated on the way.
///
/// A `null` quest is a normal outcome, not a failure: the pool can be exhausted, or a generator can
/// give up and log why. The mutated pool and the diagnostics ride back either way.
///
/// # Errors
///
/// [`QuestError::StaleEpoch`] when an override-less request names an epoch the resident DB does
/// not hold, or [`QuestError::Failed`] carrying the message of a generator's C#-sanctioned throw.
pub fn generate_repeatable_quest(
    request: QuestNativeRequest,
) -> Result<QuestNativeResponse, QuestError> {
    let views = match request.views_override {
        Some(wire) => QuestViews::Override(Box::new(wire)),
        None => {
            let db = crate::db::current().ok_or(QuestError::StaleEpoch)?;
            if db.epoch != request.epoch {
                return Err(QuestError::StaleEpoch);
            }

            QuestViews::Resident(db.quest_views.clone().ok_or(QuestError::StaleEpoch)?)
        }
    };

    let mut varying = request.varying;
    let _seed_guard = varying.seed.map(TestSeedGuard::install);
    // The generators mutate the pool while the context borrows the rest of the varying half, so
    // the pool moves out before the borrows start.
    let mut pool = std::mem::take(&mut varying.quest_type_pool);

    // `:395` dispatches on the config's type name, `Pickup` included — the case is spelled the way
    // the pool spells it and does call the generator. The arm is dead only because no shipped
    // `quest.json` config lists `Pickup` in its `types`, so nothing draws that name to dispatch on.
    let generate = match varying.quest_type {
        RepeatableQuestType::Elimination => elimination::generate,
        RepeatableQuestType::Completion => completion::generate,
        RepeatableQuestType::Exploration => exploration::generate,
        RepeatableQuestType::Pickup => pickup::generate,
    };

    let mut ctx = QuestContext::new(&views, &varying);
    // The generators panic where the C# throws; the message is the failure the caller reports.
    // Diagnostics emitted before the throw are already in the log — DiagSink emits live.
    let quest = catch_unwind(AssertUnwindSafe(|| {
        generate(
            &mut ctx,
            &varying.session_id,
            varying.pmc_level,
            &varying.trader_id,
            &mut pool,
            &varying.repeatable_config,
        )
    }))
    .map_err(panic_message)?;

    Ok(QuestNativeResponse { quest, pool })
}

/// The text a caught panic carries, wrapped as this family's sanctioned-throw failure.
fn panic_message(payload: Box<dyn Any + Send>) -> QuestError {
    QuestError::Failed(crate::ffi::panic_message(payload))
}
