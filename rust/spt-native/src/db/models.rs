//! Wire models of the resident database (spec § The epoch protocol, as amended 2026-08-18).
//!
//! Task-1 shape rule: every root is a `#[serde(flatten)]` superset map. Typed fields are lifted
//! out of `extra` only when Rust-side derivation reads them (Task 2 onward) — the flatten map is
//! what keeps the root full-fidelity regardless.

use indexmap::IndexMap;
use serde::Deserialize;
use serde_json::Value;

/// `{"schema":1,"roots":{...}}` — the envelope `DbPayloadProjection` (C#) writes.
#[derive(Debug, Deserialize)]
pub struct PublishRequest {
    pub schema: u32,
    pub roots: PublishRoots,
}

/// Every root optional: an absent root keeps the currently-resident one. Unknown root names are
/// a parse error (`deny_unknown_fields`), surfacing as `STATUS_BAD_ARGS` — C# and Rust ship in
/// lockstep, so a typo should fail loudly, not silently install nothing.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublishRoots {
    pub templates: Option<TemplatesRoot>,
    pub traders: Option<TradersRoot>,
    pub globals: Option<GlobalsRoot>,
}

#[derive(Debug, Deserialize)]
pub struct TemplatesRoot {
    #[serde(flatten)]
    pub extra: IndexMap<String, Value>,
}

#[derive(Debug, Deserialize)]
pub struct TradersRoot {
    #[serde(flatten)]
    pub extra: IndexMap<String, Value>,
}

#[derive(Debug, Deserialize)]
pub struct GlobalsRoot {
    #[serde(flatten)]
    pub extra: IndexMap<String, Value>,
}
