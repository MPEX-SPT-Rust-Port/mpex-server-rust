pub mod models;

use std::marker::PhantomData;

use crate::loot::models::Diagnostic;

/// The read-only views one repeatable-quest pass consults, plus the diagnostics the C# caller
/// replays through its logger — the quest family's analog of [`crate::ragfair::RagfairContext`].
///
/// The borrowed slice views land in Task 5 alongside `QuestInvariantSlice`; only the diagnostics
/// sink exists so far, so the lifetime is carried by a marker until then.
pub struct QuestContext<'a> {
    pub diagnostics: Vec<Diagnostic>,
    pub slice: PhantomData<&'a ()>,
}
