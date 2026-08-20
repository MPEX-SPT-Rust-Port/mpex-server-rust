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

use indexmap::{IndexMap, IndexSet};

use crate::db::models::{ConfigsRoot, ItemConfigLift, QuestConfigLift};
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
    /// `ItemConfig.RewardItemBlacklist`. An `IndexSet`, unlike the two `HashSet` members either
    /// side of it, because that is the shape both arms hand over — the configs root's
    /// [`ItemConfigLift`] and the override wire that mirrors it. Membership is the only thing
    /// asked of it (`reward_generator::is_valid_reward_item`), so the order it keeps is unread.
    pub reward_item_blacklist: &'a IndexSet<String>,
    /// `ItemConfig.BossItems`, an `IndexSet` for the same reason.
    pub boss_items: &'a IndexSet<String>,
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

/// The resolved view source for one pass: the resident DB's derived bundle plus the configs root
/// its config-backed members come out of, or the transient wire override a residency-ineligible
/// caller sent. The override is boxed so the two variants stay pointer-sized.
pub enum QuestViews {
    Resident {
        views: Arc<QuestDbViews>,
        /// The resident configs root. [`resolve_quest_views`] has already proved both stems this
        /// family reads are present, so the borrows [`QuestContext::new`] takes cannot miss one.
        configs: Arc<ConfigsRoot>,
    },
    Override(Box<QuestViewsWire>),
}

/// The `spt-item` stem, present because [`resolve_quest_views`] refused the request without it.
fn item_config(configs: &ConfigsRoot) -> &ItemConfigLift {
    configs
        .item
        .as_ref()
        .expect("resolve_quest_views proved the spt-item stem present")
}

/// The `spt-quest` stem, present for the same reason.
fn quest_config(configs: &ConfigsRoot) -> &QuestConfigLift {
    configs
        .quest
        .as_ref()
        .expect("resolve_quest_views proved the spt-quest stem present")
}

impl<'a> QuestContext<'a> {
    /// Borrow the database views and the four config-backed members off `views` and the two
    /// service-state sets off `varying`, emitting straight to the log pipeline — one pass's
    /// context.
    pub fn new(views: &'a QuestViews, varying: &'a QuestVaryingRequest) -> Self {
        match views {
            QuestViews::Resident { views, configs } => QuestContext {
                items: &views.ragfair.items,
                base_classes: &views.ragfair.base_classes,
                handbook_prices: &views.ragfair.handbook_prices,
                flea_prices: &views.ragfair.flea_prices,
                default_weapon_presets: &views.default_weapon_presets,
                default_preset_or_item_prices: &views.default_preset_or_item_prices,
                item_blacklist: &varying.item_blacklist,
                reward_item_blacklist: &item_config(configs).reward_item_blacklist,
                boss_items: &item_config(configs).boss_items,
                seasonal_item_tpl_blacklist: &varying.seasonal_item_tpl_blacklist,
                repeatable_quest_templates: &views.repeatable_quest_templates,
                completion_items_whitelist: &views.completion_items_whitelist,
                completion_items_blacklist: &views.completion_items_blacklist,
                boss_spawns_by_location: &views.boss_spawns_by_location,
                extracts_by_location: &views.extracts_by_location,
                repeatable_quest_template_ids: &quest_config(configs).repeatable_quest_template_ids,
                location_id_map: &quest_config(configs).location_id_map,
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
                reward_item_blacklist: &wire.reward_item_blacklist,
                boss_items: &wire.boss_items,
                seasonal_item_tpl_blacklist: &varying.seasonal_item_tpl_blacklist,
                repeatable_quest_templates: &wire.repeatable_quest_templates,
                completion_items_whitelist: &wire.completion_items_whitelist,
                completion_items_blacklist: &wire.completion_items_blacklist,
                boss_spawns_by_location: &wire.boss_spawns_by_location,
                extracts_by_location: &wire.extracts_by_location,
                repeatable_quest_template_ids: &wire.repeatable_quest_template_ids,
                location_id_map: &wire.location_id_map,
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

impl QuestError {
    pub fn new(message: impl Into<String>) -> Self {
        Self::Failed(message.into())
    }
}

/// The override arm resolves without consulting the process-global store; the resident arm needs
/// the named epoch resident with the derived quest views and the configs root, the shape
/// [`crate::scav_case::resolve_scav_case_views`] uses.
///
/// A missing root is a stale epoch — the publish never carried it, so a republish is the fix. A
/// configs root that *is* resident but has no stem this family reads is a different failure and
/// gets a different answer: an error naming the stem, per call, rather than a silent default.
///
/// # Errors
///
/// [`QuestError::StaleEpoch`] as above, or [`QuestError::Failed`] naming the absent stem.
pub fn resolve_quest_views(
    epoch: u64,
    views_override: Option<QuestViewsWire>,
) -> Result<QuestViews, QuestError> {
    match views_override {
        Some(wire) => Ok(QuestViews::Override(Box::new(wire))),
        None => {
            let db = crate::db::current().ok_or(QuestError::StaleEpoch)?;
            if db.epoch != epoch {
                return Err(QuestError::StaleEpoch);
            }

            let views = db.quest_views.clone().ok_or(QuestError::StaleEpoch)?;

            let configs = db.configs.clone().ok_or(QuestError::StaleEpoch)?;
            if configs.quest.is_none() {
                return Err(QuestError::new("configs root has no spt-quest stem"));
            }
            if configs.item.is_none() {
                return Err(QuestError::new("configs root has no spt-item stem"));
            }

            Ok(QuestViews::Resident { views, configs })
        }
    }
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
/// not hold, or [`QuestError::Failed`] carrying the message of a generator's C#-sanctioned throw —
/// or naming a configs-root stem this family reads that the resident root does not carry.
pub fn generate_repeatable_quest(
    request: QuestNativeRequest,
) -> Result<QuestNativeResponse, QuestError> {
    let views = resolve_quest_views(request.epoch, request.views_override)?;

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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// Publishes the four roots the quest views derive off plus whatever configs root the caller
    /// hands in, and answers the epoch.
    fn publish_with_configs(configs: serde_json::Value) -> u64 {
        crate::db::publish(
            serde_json::from_value(json!({
                "schema": 1,
                "roots": {
                    "templates": {}, "traders": {}, "globals": {}, "locations": {},
                    "configs": configs
                }
            }))
            .unwrap(),
        )
        .unwrap()
    }

    /// The `spt-quest` stem as the shipped record carries it, `kind` included, so the parse has to
    /// ignore that and the unlifted members alike.
    fn quest_stem() -> serde_json::Value {
        json!({
            "kind": "spt-quest",
            "repeatableQuestTemplateIds": {
                "pmc": {"Elimination": "616052ea3054fc0e2c24ce6e"},
                "scav": {"Elimination": "62825ef60e88d037dc1eb428"}
            },
            "locationIdMap": {"bigmap": "55f2d3fd4bdc2d5f408b4567"},
            "repeatableQuests": []
        })
    }

    /// The resident arm's config-backed members come out of the two configs-root stems, and a
    /// resident configs root missing one is a per-call failure that names it — never a silent
    /// default, and never the stale-epoch answer a *missing root* gets (a republish would not fix
    /// a stem the publish does not carry).
    #[test]
    fn a_resident_resolve_reads_the_config_stems_and_names_a_missing_one() {
        let _guard = crate::db::tests::DB_TEST_LOCK.lock().unwrap();
        crate::db::clear();

        // spt-item present, spt-quest absent
        let epoch = publish_with_configs(json!({"spt-item": {"bossItems": ["boss_tpl"]}}));
        let Err(QuestError::Failed(message)) = resolve_quest_views(epoch, None) else {
            panic!("expected a failure naming the absent stem");
        };
        assert!(message.contains("spt-quest"), "{message}");

        // the mirror image: spt-quest present, spt-item absent
        let epoch = publish_with_configs(json!({"spt-quest": quest_stem()}));
        let Err(QuestError::Failed(message)) = resolve_quest_views(epoch, None) else {
            panic!("expected a failure naming the absent stem");
        };
        assert!(message.contains("spt-item"), "{message}");

        // both present: the context reads the stems' own values
        let epoch = publish_with_configs(json!({
            "spt-quest": quest_stem(),
            "spt-item": {"kind": "spt-item", "rewardItemBlacklist": ["reward_blacklisted"],
                "bossItems": ["boss_tpl"]}
        }));
        let views = resolve_quest_views(epoch, None).unwrap();
        let varying = crate::quest::models::tests::varying();
        let ctx = QuestContext::new(&views, &varying);

        assert!(ctx.reward_item_blacklist.contains("reward_blacklisted"));
        assert!(ctx.boss_items.contains("boss_tpl"));
        assert_eq!(
            ctx.repeatable_quest_template_ids.pmc["Elimination"],
            "616052ea3054fc0e2c24ce6e"
        );
        assert_eq!(ctx.location_id_map["bigmap"], "55f2d3fd4bdc2d5f408b4567");

        // A configs root that never arrived is stale, not a stem failure
        crate::db::clear();
        assert!(matches!(
            resolve_quest_views(epoch, None),
            Err(QuestError::StaleEpoch)
        ));
    }
}
