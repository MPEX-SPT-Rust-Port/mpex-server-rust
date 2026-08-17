//! `Generators/ScavCaseRewardGenerator.cs`.

pub mod generator;
pub mod models;

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
