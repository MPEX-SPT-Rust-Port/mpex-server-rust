//! `Generators/ScavCaseRewardGenerator.cs`.
//!
//! Citation convention for this module: a bare `` `:N` `` is a line of the 4.1.2 body this port was
//! written against, which now lives on in the C# file as `GenerateLegacy`. The native seam was
//! inserted above it, so those numbers sit ~131 lines higher than the same code does in the current
//! `ScavCaseRewardGenerator.cs`. Citations naming a file (`ItemHelper.cs:1245`) are current.

pub mod generator;
pub mod models;

use std::any::Any;
use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;

use indexmap::IndexMap;

use crate::db::models::HideoutRoot;
use crate::diag::DiagSink;
use crate::loot::models::{ItemView, PresetView};
use crate::loot::random_util::TestSeedGuard;
use crate::ragfair::views::RagfairDbViews;
use crate::scav_case::models::{
    EndProductsView, ScavCaseResponse, ScavCaseRewardsRequest, ScavCaseViewsWire, ScavRecipeView,
};

/// What a scav case pass can fail with: the message of a C#-sanctioned throw carried back to the
/// caller instead of unwinding (shaped like [`crate::quest::QuestError`]), or an override-less
/// request naming a resident-DB epoch this process does not hold.
#[derive(Debug)]
pub enum ScavCaseError {
    Failed(String),
    StaleEpoch,
}

impl ScavCaseError {
    pub fn new(message: impl Into<String>) -> Self {
        Self::Failed(message.into())
    }
}

/// The database half of a scav case request. The resident arm is the shared ragfair views — the
/// items view, handbook prices and default-preset map are the same projections the C# override
/// builder reads — plus the recipe views derived from the hideout root at request time.
pub enum ScavCaseViews {
    Override(Box<ScavCaseViewsWire>),
    Resident {
        ragfair: Arc<RagfairDbViews>,
        /// Derived at request time from the hideout root: recipes with all three end-product
        /// bands present, raw wire names mapped to the view.
        recipes: Vec<ScavRecipeView>,
    },
}

impl ScavCaseViews {
    pub(crate) fn scav_recipes(&self) -> &[ScavRecipeView] {
        match self {
            Self::Override(wire) => &wire.scav_recipes,
            Self::Resident { recipes, .. } => recipes,
        }
    }

    pub(crate) fn items_view(&self) -> &IndexMap<String, ItemView> {
        match self {
            Self::Override(wire) => &wire.items_view,
            Self::Resident { ragfair, .. } => &ragfair.items,
        }
    }

    pub(crate) fn static_prices(&self) -> StaticPrices<'_> {
        match self {
            Self::Override(wire) => StaticPrices::Wire(&wire.static_prices),
            Self::Resident { ragfair, .. } => StaticPrices::Resident(&ragfair.handbook_prices),
        }
    }

    pub(crate) fn default_presets_by_tpl(&self) -> &IndexMap<String, PresetView> {
        match self {
            Self::Override(wire) => &wire.default_presets_by_tpl,
            Self::Resident { ragfair, .. } => &ragfair.default_presets_by_tpl,
        }
    }
}

/// The two map types the static-price view arrives in: the wire override's `HashMap`, the
/// resident ragfair views' `IndexMap` (`handbook_prices`). The generator only ever keys into it,
/// so the map type cannot change any draw.
#[derive(Clone, Copy)]
pub enum StaticPrices<'a> {
    Wire(&'a HashMap<String, f64>),
    Resident(&'a IndexMap<String, f64>),
}

impl StaticPrices<'_> {
    pub fn get(&self, tpl: &str) -> Option<f64> {
        match self {
            Self::Wire(prices) => prices.get(tpl).copied(),
            Self::Resident(prices) => prices.get(tpl).copied(),
        }
    }
}

/// The override arm resolves without consulting the process-global store; the resident arm needs
/// the named epoch resident with both the ragfair views and the hideout root, as
/// [`crate::loot::loot_generator::resolve_reward_views`] needs its views.
pub fn resolve_scav_case_views(
    epoch: u64,
    views_override: Option<Box<ScavCaseViewsWire>>,
) -> Result<ScavCaseViews, ScavCaseError> {
    match views_override {
        Some(wire) => Ok(ScavCaseViews::Override(wire)),
        None => {
            let db = crate::db::current().ok_or(ScavCaseError::StaleEpoch)?;
            if db.epoch != epoch {
                return Err(ScavCaseError::StaleEpoch);
            }

            let ragfair = db.ragfair_views.clone().ok_or(ScavCaseError::StaleEpoch)?;
            let recipes =
                derive_recipe_views(db.hideout.as_ref().ok_or(ScavCaseError::StaleEpoch)?);

            Ok(ScavCaseViews::Resident { ragfair, recipes })
        }
    }
}

/// Bug-for-bug with `ScavCaseNativeRequestBuilder.BuildRecipeViews`: a recipe missing
/// `endProducts` or any of the three bands is skipped, not an error.
fn derive_recipe_views(hideout: &HideoutRoot) -> Vec<ScavRecipeView> {
    hideout
        .production
        .scav_recipes
        .iter()
        .filter_map(|recipe| {
            let end_products = recipe.end_products.as_ref()?;

            Some(ScavRecipeView {
                id: recipe.id.clone(),
                end_products: EndProductsView {
                    common: end_products.common?,
                    rare: end_products.rare?,
                    superrare: end_products.superrare?,
                },
            })
        })
        .collect()
}

/// The module boundary: one scav case craft's rewards, on the request's seeded stream when it
/// carries one.
///
/// The generator panics where the C# throws out of a dictionary index — `templateTable.Items[…]`
/// (`:270-273`) and the config's rarity maps (`:405`, `:472`). Caught here so the message crosses
/// the FFI boundary as an error string, the way [`crate::quest::generate_repeatable_quest`] does it,
/// rather than as a bare `STATUS_PANIC`.
///
/// # Errors
///
/// [`ScavCaseError::StaleEpoch`] when an override-less request names an epoch the resident DB
/// does not hold, or [`ScavCaseError::Failed`] carrying the message of a C#-sanctioned throw,
/// thrown or panicked.
pub fn generate_scav_case_rewards(
    request: ScavCaseRewardsRequest,
    diagnostics: &mut DiagSink,
) -> Result<ScavCaseResponse, ScavCaseError> {
    // Resolved before the seed guard and the unwind boundary: a stale epoch answers cleanly,
    // without touching the stream.
    let views = resolve_scav_case_views(request.epoch, request.views_override)?;
    let varying = request.varying;
    let _seed_guard = varying.test_seed.map(TestSeedGuard::install);

    // Diagnostics gathered before a panic are dropped, as they are on every other export.
    catch_unwind(AssertUnwindSafe(|| {
        generator::generate(&varying, &views, diagnostics)
    }))
    .unwrap_or_else(|payload| Err(panic_message(payload)))
}

/// The text a caught panic carries — `panic!`/`expect` payloads are a `String` or a `&str`.
fn panic_message(payload: Box<dyn Any + Send>) -> ScavCaseError {
    let message = payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| {
            payload
                .downcast_ref::<&str>()
                .map(|text| (*text).to_owned())
        })
        .unwrap_or_else(|| "scav case reward generation panicked".to_owned());

    ScavCaseError::new(message)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::scav_case::generator::tests::envelope;

    /// The one request member the entry point reads itself; everything else is the generator's.
    fn request(recipe_id: &str) -> ScavCaseRewardsRequest {
        serde_json::from_value(envelope(json!({
            "recipeId": recipe_id,
            "scavRecipes": [{"id": "aaaaaaaaaaaaaaaaaaaaaaaa", "endProducts": {
                "common": {"min": 1, "max": 1}, "rare": {"min": 0, "max": 0},
                "superrare": {"min": 0, "max": 0}}}],
            "config": {
                "rewardItemValueRangeRub": {"common": {"min": 0.0, "max": 100.0},
                    "rare": {"min": 0.0, "max": 100.0}, "superrare": {"min": 0.0, "max": 100.0}},
                "moneyRewards": {"moneyRewardChancePercent": 100,
                    "rubCount": {"common": {"min": 1, "max": 1}, "rare": {"min": 1, "max": 1},
                        "superrare": {"min": 1, "max": 1}},
                    "usdCount": {"common": {"min": 1, "max": 1}, "rare": {"min": 1, "max": 1},
                        "superrare": {"min": 1, "max": 1}},
                    "eurCount": {"common": {"min": 1, "max": 1}, "rare": {"min": 1, "max": 1},
                        "superrare": {"min": 1, "max": 1}},
                    "gpCount": {"common": {"min": 1, "max": 1}, "rare": {"min": 1, "max": 1},
                        "superrare": {"min": 1, "max": 1}}},
                "ammoRewards": {"ammoRewardChancePercent": 0,
                    "ammoRewardValueRangeRub": {}, "minStackSize": 30},
                "rewardItemParentBlacklist": [],
                "rewardItemBlacklist": [],
                "allowMultipleMoneyRewardsPerRarity": false,
                "allowMultipleAmmoRewardsPerRarity": false,
                "allowBossItemsAsRewards": true
            },
            "itemsView": {},
            "staticPrices": {},
            "defaultPresetsByTpl": {},
            "inactiveSeasonalItems": [],
            "globalBlacklist": [],
            "rewardItemBlacklist": [],
            "bossItems": [],
            "testSeed": 42
        })))
        .unwrap()
    }

    /// The money pool indexes the items view (`:270-273`), so an items view without the money
    /// templates is the C# `KeyNotFoundException` — a panic here, which the boundary turns into an
    /// error carrying the panic's own message rather than the generic fallback.
    #[test]
    fn a_panicking_generator_reports_its_message_rather_than_unwinding() {
        let error = generate_scav_case_rewards(
            request("aaaaaaaaaaaaaaaaaaaaaaaa"),
            &mut DiagSink::capture(),
        )
        .unwrap_err();

        let ScavCaseError::Failed(message) = error else {
            panic!("expected the panic's message, got {error:?}");
        };
        assert_ne!(message, "scav case reward generation panicked");
        assert!(message.contains("key"), "{message}");
    }

    /// Bug-for-bug with `ScavCaseNativeRequestBuilder.BuildRecipeViews`: a recipe missing
    /// `endProducts` or any band is skipped, and a missing `_id` (the parse-total default) rides
    /// through as the empty string, as the C# `MongoId` default would.
    #[test]
    fn recipe_derivation_skips_recipes_missing_end_products_or_a_band() {
        let hideout: HideoutRoot = serde_json::from_value(json!({
            "production": {"scavRecipes": [
                {"_id": "aaaaaaaaaaaaaaaaaaaaaaaa", "endProducts": {
                    "Common": {"min": 1, "max": 2}, "Rare": {"min": 3, "max": 4},
                    "Superrare": {"min": 5, "max": 6}}},
                {"_id": "bbbbbbbbbbbbbbbbbbbbbbbb", "endProducts": {
                    "Common": {"min": 1, "max": 2}}},
                {"_id": "cccccccccccccccccccccccc"},
                {"endProducts": {
                    "Common": {"min": 0, "max": 0}, "Rare": {"min": 0, "max": 0},
                    "Superrare": {"min": 0, "max": 0}}}
            ]}
        }))
        .unwrap();

        let recipes = derive_recipe_views(&hideout);

        assert_eq!(recipes.len(), 2);
        assert_eq!(recipes[0].id, "aaaaaaaaaaaaaaaaaaaaaaaa");
        assert_eq!(recipes[0].end_products.superrare.max, 6);
        assert_eq!(recipes[1].id, "");
    }
}
