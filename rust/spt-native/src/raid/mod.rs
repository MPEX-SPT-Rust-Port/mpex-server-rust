//! The raid-setup family: `Services/InRaid/RaidTimeAdjustmentService.cs`, ported bug-for-bug.
//!
//! Citation convention for this module: a bare `` `:N` `` is a line of
//! `RaidTimeAdjustmentService.cs`; citations naming a file (`LocationConfig.cs:264`) are that
//! file's.

pub mod adjustments;
pub mod models;

use std::any::Any;
use std::panic::{AssertUnwindSafe, catch_unwind};

use crate::loot::random_util::TestSeedGuard;
use crate::raid::models::{
    GetRaidAdjustmentsRequest, GetRaidAdjustmentsResponse, MakeAdjustmentsRequest,
    MakeAdjustmentsResponse,
};

/// What a raid-setup pass can fail with: the message of a C#-sanctioned throw carried back to the
/// caller instead of unwinding (shaped like [`crate::scav_case::ScavCaseError`], minus its
/// resident-DB arm — this family carries no epoch).
#[derive(Debug)]
pub enum RaidError {
    Failed(String),
}

impl RaidError {
    pub fn new(message: impl Into<String>) -> Self {
        Self::Failed(message.into())
    }
}

/// The module boundary: one scav raid's time adjustment, on the request's seeded stream when it
/// carries one. The guard is installed here rather than in `ffi.rs` so the first draw
/// ([`get_chance_100`](crate::loot::random_util::get_chance_100)) cannot precede it.
///
/// # Errors
///
/// [`RaidError::Failed`] carrying the message of a C#-sanctioned throw — the missing map key and
/// the non-numeric weight key — thrown or panicked.
pub fn get_raid_adjustments(
    request: GetRaidAdjustmentsRequest,
) -> Result<GetRaidAdjustmentsResponse, RaidError> {
    let _seed_guard = request.test_seed.map(TestSeedGuard::install);

    catch_unwind(AssertUnwindSafe(|| adjustments::get_adjustments(&request)))
        .unwrap_or_else(|payload| Err(panic_message(payload)))
}

/// The module boundary: one map's raid-setup deltas, off the changes [`get_raid_adjustments`]
/// produced. No seed guard, unlike its sibling — this pass draws nothing, so every value it emits
/// is a function of the request alone. The `catch_unwind` still stands, to keep a port bug off
/// `STATUS_PANIC`.
///
/// # Errors
///
/// [`RaidError::Failed`] carrying the message of the one C#-sanctioned throw this pass can reach,
/// the missing map key — thrown or panicked.
pub fn make_adjustments_to_map(
    request: MakeAdjustmentsRequest,
) -> Result<MakeAdjustmentsResponse, RaidError> {
    catch_unwind(AssertUnwindSafe(|| adjustments::make_adjustments(&request)))
        .unwrap_or_else(|payload| Err(panic_message(payload)))
}

/// The text a caught panic carries — `panic!`/`expect` payloads are a `String` or a `&str`.
fn panic_message(payload: Box<dyn Any + Send>) -> RaidError {
    let message = payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| {
            payload
                .downcast_ref::<&str>()
                .map(|text| (*text).to_owned())
        })
        .unwrap_or_else(|| "raid adjustment generation panicked".to_owned());

    RaidError::new(message)
}
