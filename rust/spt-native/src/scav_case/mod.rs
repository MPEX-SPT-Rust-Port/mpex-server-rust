//! `Generators/ScavCaseRewardGenerator.cs`.

pub mod generator;
pub mod models;

use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};

use crate::diag::DiagSink;
use crate::loot::random_util::TestSeedGuard;
use crate::scav_case::models::{ScavCaseRequest, ScavCaseResponse};

/// What a scav case pass can fail with: the message of a C#-sanctioned throw, carried back to the
/// caller instead of unwinding. Shaped like [`crate::loot::item_helper::LootError`], which the loot
/// family uses for the same job.
#[derive(Debug)]
pub struct ScavCaseError {
    pub message: String,
}

impl ScavCaseError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
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
/// [`ScavCaseError`] carrying the message of a C#-sanctioned throw, thrown or panicked.
pub fn generate_scav_case_rewards(
    request: ScavCaseRequest,
    diagnostics: &mut DiagSink,
) -> Result<ScavCaseResponse, ScavCaseError> {
    let _seed_guard = request.test_seed.map(TestSeedGuard::install);

    // Diagnostics gathered before a panic are dropped, as they are on every other export.
    catch_unwind(AssertUnwindSafe(|| {
        generator::generate(&request, diagnostics)
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

    /// The one request member the entry point reads itself; everything else is the generator's.
    fn request(recipe_id: &str) -> ScavCaseRequest {
        serde_json::from_value(json!({
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
        }))
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

        assert_ne!(error.message, "scav case reward generation panicked");
        assert!(error.message.contains("key"), "{}", error.message);
    }
}
