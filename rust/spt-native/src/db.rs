//! The process-global resident database (spec § The epoch protocol).
//!
//! One `RwLock<Option<Arc<ResidentDb>>>`: readers clone the `Arc` at entry and never hold the
//! lock across a generation; a publish builds the merged replacement and swaps it under the
//! write lock. Epoch starts at 1 on the first successful publish and increments on every
//! publish, full or partial. A failed publish leaves the previous resident DB fully intact.

pub mod load;
pub mod models;

use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::{Arc, RwLock};

use crate::bot::views::BotDbViews;
use crate::db::models::{
    ConfigsRoot, GlobalsRoot, HideoutRoot, LocationsRoot, PublishRequest, TemplatesRoot,
    TradersRoot,
};
use crate::quest::views::QuestDbViews;
use crate::ragfair::views::RagfairDbViews;

pub struct ResidentDb {
    pub epoch: u64,
    pub templates: Option<Arc<TemplatesRoot>>,
    pub traders: Option<Arc<TradersRoot>>,
    pub globals: Option<Arc<GlobalsRoot>>,
    pub locations: Option<Arc<LocationsRoot>>,
    pub hideout: Option<Arc<HideoutRoot>>,
    pub configs: Option<Arc<ConfigsRoot>>,
    /// `Some` whenever all three source roots are resident — re-derived on every such publish,
    /// `None` otherwise.
    pub ragfair_views: Option<Arc<RagfairDbViews>>,
    /// `Some` whenever templates+globals+locations are resident **and** [`Self::ragfair_views`]
    /// derived (which additionally needs traders). C# always publishes all four roots together,
    /// so in practice both view sets derive on the same publish.
    pub quest_views: Option<Arc<QuestDbViews>>,
    /// `Some` whenever templates+globals are resident **and** [`Self::ragfair_views`] derived —
    /// in practice the same publishes as the other view sets.
    pub bot_views: Option<Arc<BotDbViews>>,
}

#[derive(Debug)]
pub enum PublishError {
    Schema(String),
    /// A view derivation failure — ragfair, quest or bot. Aborts the publish before the swap —
    /// the previous resident DB stays fully intact.
    Views(String),
}

/// An override-less request named an epoch this process does not hold (or the root it needs is
/// not resident). The caller republishes and retries once — the epoch protocol's self-heal.
#[derive(Debug)]
pub struct StaleEpoch;

static DB: RwLock<Option<Arc<ResidentDb>>> = RwLock::new(None);

/// The resident DB right now, or `None` before the first publish. Arc clone under a read lock,
/// released before returning — callers never block a publish beyond the clone itself.
pub fn current() -> Option<Arc<ResidentDb>> {
    DB.read().unwrap().clone()
}

/// Install `request`'s roots over the currently-resident ones and bump the epoch.
// ponytail: whole merge under the write lock (publish is startup + stamp-bump cadence); split
// the derive out of the lock if reader stalls ever measure
pub fn publish(request: PublishRequest) -> Result<u64, PublishError> {
    if request.schema != 1 {
        return Err(PublishError::Schema(format!(
            "unsupported publish schema {}",
            request.schema
        )));
    }

    let mut slot = DB.write().unwrap();
    let previous = slot.as_ref();
    let epoch = previous.map_or(1, |db| db.epoch + 1);
    let templates = request
        .roots
        .templates
        .map(Arc::new)
        .or_else(|| previous.and_then(|db| db.templates.clone()));
    let traders = request
        .roots
        .traders
        .map(Arc::new)
        .or_else(|| previous.and_then(|db| db.traders.clone()));
    let globals = request
        .roots
        .globals
        .map(Arc::new)
        .or_else(|| previous.and_then(|db| db.globals.clone()));
    let locations = request
        .roots
        .locations
        .map(Arc::new)
        .or_else(|| previous.and_then(|db| db.locations.clone()));
    let hideout = request
        .roots
        .hideout
        .map(Arc::new)
        .or_else(|| previous.and_then(|db| db.hideout.clone()));
    let configs = request
        .roots
        .configs
        .map(Arc::new)
        .or_else(|| previous.and_then(|db| db.configs.clone()));

    // Derived before the swap: a derivation error aborts the publish and leaves the previous
    // resident DB fully intact. The derive runs under the write guard, so a panic in it must
    // not unwind past the guard — that would poison the static lock and take down every later
    // resident call, including the C# self-heal republish. Caught and mapped to the same
    // abort-before-swap path (the panic hook has already logged the payload to stderr).
    let ragfair_views = match (&templates, &traders, &globals) {
        (Some(templates), Some(traders), Some(globals)) => {
            let derived = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                #[cfg(test)]
                if tests::PANIC_ON_DERIVE.load(std::sync::atomic::Ordering::Relaxed) {
                    panic!("injected derive panic");
                }
                crate::ragfair::views::derive(templates, traders, globals)
            }))
            .unwrap_or_else(|_| Err("ragfair view derivation panicked".to_string()));
            Some(Arc::new(derived.map_err(PublishError::Views)?))
        }
        _ => None,
    };

    // Same containment as the ragfair derive above: caught panic, abort before the swap.
    let quest_views = match (&templates, &globals, &locations, &ragfair_views) {
        (Some(templates), Some(globals), Some(locations), Some(ragfair_views)) => {
            let derived = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                #[cfg(test)]
                if tests::PANIC_ON_QUEST_DERIVE.load(std::sync::atomic::Ordering::Relaxed) {
                    panic!("injected quest derive panic");
                }
                crate::quest::views::derive(templates, globals, locations, ragfair_views)
            }))
            .unwrap_or_else(|_| Err("quest view derivation panicked".to_string()));
            Some(Arc::new(derived.map_err(PublishError::Views)?))
        }
        _ => None,
    };

    // Same containment as the ragfair derive above: caught panic, abort before the swap.
    // `ragfair_views` is `Some` only when templates+traders+globals all are, so it subsumes the
    // templates half of the gate; `globals` stays because the derive reads it.
    let bot_views = match (&globals, &ragfair_views) {
        (Some(globals), Some(ragfair_views)) => {
            let derived = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                #[cfg(test)]
                if tests::PANIC_ON_BOT_DERIVE.load(std::sync::atomic::Ordering::Relaxed) {
                    panic!("injected bot derive panic");
                }
                crate::bot::views::derive(globals, ragfair_views)
            }))
            .unwrap_or_else(|_| Err("bot view derivation panicked".to_string()));
            Some(Arc::new(derived.map_err(PublishError::Views)?))
        }
        _ => None,
    };

    *slot = Some(Arc::new(ResidentDb {
        epoch,
        templates,
        traders,
        globals,
        locations,
        hideout,
        configs,
        ragfair_views,
        quest_views,
        bot_views,
    }));

    Ok(epoch)
}

thread_local! {
    /// True only while `canonical_digest` serializes. The wire models' flattened `extra` maps
    /// carry `skip_serializing_if = "crate::db::skip_extra_for_digest"`, so digest serialization
    /// sees the typed lift surface only, while production serialization (the loot responses that
    /// round-trip `extra`) is untouched.
    static DIGEST_MODE: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// `skip_serializing_if` hook for every wire model's flattened `extra` map — see [`DIGEST_MODE`].
pub fn skip_extra_for_digest<T>(_: &T) -> bool {
    DIGEST_MODE.with(std::cell::Cell::get)
}

/// Sets digest mode for the guard's lifetime, unwinding included.
struct DigestModeGuard;

impl DigestModeGuard {
    fn set() -> Self {
        DIGEST_MODE.with(|mode| mode.set(true));

        DigestModeGuard
    }
}

impl Drop for DigestModeGuard {
    fn drop(&mut self) {
        DIGEST_MODE.with(|mode| mode.set(false));
    }
}

/// Order-insensitive for object keys (the C#-written and Rust-spliced envelopes legitimately
/// differ in member order — serde_json is preserve_order here), order-sensitive for arrays —
/// except the three order-contract maps below, whose container-order key sequence joins the
/// digest. Serializes in digest mode: `extra` maps are skipped, so this digests the typed lift
/// surface — the read surface — not post-parse byte fidelity (spec § Part 0). std `DefaultHasher`
/// is SipHash-1-3 with fixed keys — stable within a toolchain, but no wire contract; the
/// equivalence gate compares within one process, which needs neither.
pub fn canonical_digest<T: serde::Serialize>(value: &T) -> u64 {
    let _digest_mode = DigestModeGuard::set();
    let value = serde_json::to_value(value).expect("resident roots serialize");
    let mut hasher = DefaultHasher::new();
    hash_value(&value, &mut hasher);
    // hash_value's key sort also sorts away genuine order divergence in the maps whose iteration
    // order is documented read contract: `templates.items` (the ragfair view caches iterate its
    // key order), `templates.prices` (`GetFleaPricesAsArray` draws an RNG index into source
    // order) and `globals.ItemPresets` (last-in-map-order wins the default-preset cache). Mix
    // those maps' container-order key sequence back in. Only these three: each comes from a
    // single JSON file, so both arms carry the same order deterministically — a flatten root's
    // key order (directory-walk) or a raw-Value lift's member order (C# emission) does not, and
    // making those order-sensitive would trade a gate blind spot for gate flakiness. The names
    // are matched at the root level only, where no digested root type has a colliding member.
    for name in ["items", "prices", "ItemPresets"] {
        if let Some(serde_json::Value::Object(map)) = value.get(name) {
            7u8.hash(&mut hasher);
            map.len().hash(&mut hasher);
            for key in map.keys() {
                key.hash(&mut hasher);
            }
        }
    }
    hasher.finish()
}

// Array hashing is order-sensitive — correct for the Vec lifts (both arms carry file order) and
// for the handbook items whose append position the gate pins. Constraint for any future
// extension: a set-typed lift serializes as an array in container order, so anything reachable
// from a gated root (templates/traders/globals/locations/hideout) must be an `IndexSet` — file
// order, which both arms carry, and which two parses of the same bytes reproduce. A `HashSet`
// there is a bug: `RandomState::new` reseeds per instance, so its order differs between two
// parses within one process and the digest stops being a function of its input. `HashSet` is
// fine only under the configs root, which the gate never compares.
fn hash_value(value: &serde_json::Value, hasher: &mut impl Hasher) {
    match value {
        serde_json::Value::Object(map) => {
            1u8.hash(hasher);
            // Length first, as the array arm does: without it `{"a":{},"b":X}` and `{"a":{"b":X}}`
            // feed the hasher the same key/value stream and collide.
            map.len().hash(hasher);
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_unstable();
            for key in keys {
                key.hash(hasher);
                hash_value(&map[key.as_str()], hasher);
            }
        }
        serde_json::Value::Array(items) => {
            2u8.hash(hasher);
            items.len().hash(hasher);
            for item in items {
                hash_value(item, hasher);
            }
        }
        serde_json::Value::String(s) => {
            3u8.hash(hasher);
            s.hash(hasher);
        }
        serde_json::Value::Number(n) => {
            4u8.hash(hasher);
            n.to_string().hash(hasher);
        }
        serde_json::Value::Bool(b) => {
            5u8.hash(hasher);
            b.hash(hasher);
        }
        serde_json::Value::Null => 6u8.hash(hasher),
    }
}

/// `{"epoch":N,"roots":{"templates":"<16-hex>",…}}` — absent roots omitted, `{"epoch":0,"roots":{}}`
/// before the first publish. Test support for the load/projection equivalence gate.
///
/// The `configs` digest is **not** a pure function of the parsed input — `ConfigsRoot`'s `HashSet`
/// lifts serialize in per-instance order, so two parses of the same bytes can digest differently.
/// Compare only the five table roots (templates/traders/globals/locations/hideout) across parses.
pub fn resident_digests_json() -> Vec<u8> {
    let Some(db) = current() else {
        return br#"{"epoch":0,"roots":{}}"#.to_vec();
    };

    fn push<T: serde::Serialize>(
        roots: &mut serde_json::Map<String, serde_json::Value>,
        name: &str,
        root: &Option<Arc<T>>,
    ) {
        if let Some(root) = root {
            roots.insert(
                name.to_string(),
                serde_json::Value::String(format!("{:016x}", canonical_digest(root.as_ref()))),
            );
        }
    }

    let mut roots = serde_json::Map::new();
    push(&mut roots, "templates", &db.templates);
    push(&mut roots, "traders", &db.traders);
    push(&mut roots, "globals", &db.globals);
    push(&mut roots, "locations", &db.locations);
    push(&mut roots, "hideout", &db.hideout);
    push(&mut roots, "configs", &db.configs);

    serde_json::to_vec(&serde_json::json!({"epoch": db.epoch, "roots": roots}))
        .expect("digest report serializes")
}

#[cfg(test)]
pub fn clear() {
    *DB.write().unwrap() = None;
}

#[cfg(test)]
pub mod tests {
    /// Serializes every test that touches the process-global store.
    pub static DB_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Makes the next derive panic inside `publish` — proves the catch keeps the lock unpoisoned.
    pub static PANIC_ON_DERIVE: std::sync::atomic::AtomicBool =
        std::sync::atomic::AtomicBool::new(false);

    /// [`PANIC_ON_DERIVE`]'s twin for the quest derive block — the ragfair injection errors the
    /// publish before the quest derive ever runs, so proving its catch needs its own seam.
    pub static PANIC_ON_QUEST_DERIVE: std::sync::atomic::AtomicBool =
        std::sync::atomic::AtomicBool::new(false);

    /// [`PANIC_ON_DERIVE`]'s twin for the bot derive block, for the same reason.
    pub static PANIC_ON_BOT_DERIVE: std::sync::atomic::AtomicBool =
        std::sync::atomic::AtomicBool::new(false);
}

#[cfg(test)]
mod store_tests {
    use super::*;

    fn request(json: &str) -> models::PublishRequest {
        serde_json::from_str(json).expect("test request parses")
    }

    #[test]
    fn epoch_starts_at_one_and_increments() {
        let _guard = tests::DB_TEST_LOCK.lock().unwrap();
        clear();
        assert!(current().is_none());

        let first = publish(request(
            r#"{"schema":1,"roots":{"templates":{"a":1},"traders":{"b":2},"globals":{"c":3}}}"#,
        ))
        .unwrap();
        let second = publish(request(r#"{"schema":1,"roots":{"templates":{"a":9}}}"#)).unwrap();

        assert_eq!(first, 1);
        assert_eq!(second, 2);
        assert_eq!(current().unwrap().epoch, 2);
    }

    #[test]
    fn partial_publish_keeps_absent_roots() {
        let _guard = tests::DB_TEST_LOCK.lock().unwrap();
        clear();
        publish(request(
            r#"{"schema":1,"roots":{"templates":{"a":1},"traders":{"b":2},"globals":{"c":3},"locations":{"factory4_day":{}}}}"#,
        ))
        .unwrap();
        let before = current().unwrap();
        publish(request(r#"{"schema":1,"roots":{"templates":{"a":9}}}"#)).unwrap();
        let after = current().unwrap();

        // traders/globals/locations survive by Arc identity; templates was replaced
        assert!(Arc::ptr_eq(
            before.traders.as_ref().unwrap(),
            after.traders.as_ref().unwrap()
        ));
        assert!(Arc::ptr_eq(
            before.globals.as_ref().unwrap(),
            after.globals.as_ref().unwrap()
        ));
        assert!(Arc::ptr_eq(
            before.locations.as_ref().unwrap(),
            after.locations.as_ref().unwrap()
        ));
        assert!(!Arc::ptr_eq(
            before.templates.as_ref().unwrap(),
            after.templates.as_ref().unwrap()
        ));
    }

    #[test]
    fn a_hideout_root_publishes_and_survives_a_partial_republish() {
        let _guard = tests::DB_TEST_LOCK.lock().unwrap();
        clear();
        let epoch = publish(request(
            r#"{"schema":1,"roots":{"hideout":{"production":{"scavRecipes":[
                {"_id":"6662e9aca7e0b43baa3d5f9c",
                 "endProducts":{"Common":{"min":1,"max":2},"Rare":{"min":0,"max":1},"Superrare":{"min":0,"max":0}},
                 "productionTime":3.0,"someUnliftedKey":true}
            ]}}}}"#,
        ))
        .unwrap();
        assert_eq!(epoch, 1);
        let db = current().unwrap();
        let recipes = &db.hideout.as_ref().unwrap().production.scav_recipes;
        assert_eq!(recipes.len(), 1);
        assert_eq!(recipes[0].id, "6662e9aca7e0b43baa3d5f9c");
        let common = recipes[0].end_products.as_ref().unwrap().common.unwrap();
        assert_eq!(common.max, 2);

        // partial republish without the root keeps it resident, epoch still moves
        let epoch2 = publish(request(r#"{"schema":1,"roots":{}}"#)).unwrap();
        assert_eq!(epoch2, 2);
        assert!(current().unwrap().hideout.is_some());
    }

    #[test]
    fn a_configs_root_publishes_and_survives_a_partial_republish() {
        let _guard = tests::DB_TEST_LOCK.lock().unwrap();
        clear();
        // An unlifted kind, so the body rides `extra` raw — `spt-ragfair` and `spt-item` are
        // typed stems now (Task 6) and a truncated body would fail the parse
        let epoch = publish(request(
            r#"{"schema":1,"roots":{"configs":{"spt-core":{"kind":"spt-core","profileSaveIntervalSeconds":15}}}}"#,
        ))
        .unwrap();
        assert_eq!(epoch, 1);
        let before = current().unwrap();
        let core = &before.configs.as_ref().unwrap().extra["spt-core"];
        assert_eq!(core["profileSaveIntervalSeconds"], 15);

        // partial republish without the root keeps it resident, epoch still moves
        let epoch2 = publish(request(r#"{"schema":1,"roots":{}}"#)).unwrap();
        assert_eq!(epoch2, 2);
        let after = current().unwrap();
        assert!(Arc::ptr_eq(
            before.configs.as_ref().unwrap(),
            after.configs.as_ref().unwrap()
        ));
    }

    #[test]
    fn a_junk_configs_root_parses_total() {
        let _guard = tests::DB_TEST_LOCK.lock().unwrap();
        clear();
        publish(request(r#"{"schema":1,"roots":{"configs":{"a":1}}}"#)).unwrap();
        let db = current().unwrap();
        assert_eq!(db.configs.as_ref().unwrap().extra["a"], 1);
    }

    #[test]
    fn wrong_schema_is_an_error_and_leaves_store_untouched() {
        let _guard = tests::DB_TEST_LOCK.lock().unwrap();
        clear();
        let error = publish(request(r#"{"schema":2,"roots":{}}"#)).unwrap_err();
        assert!(matches!(error, PublishError::Schema(_)));
        assert!(current().is_none());

        // Populated store: a schema-2 publish leaves epoch and roots untouched too
        publish(request(
            r#"{"schema":1,"roots":{"templates":{"a":1},"traders":{"b":2},"globals":{"c":3}}}"#,
        ))
        .unwrap();
        let before = current().unwrap();
        let error = publish(request(r#"{"schema":2,"roots":{"templates":{"a":9}}}"#)).unwrap_err();
        assert!(matches!(error, PublishError::Schema(_)));
        let after = current().unwrap();
        assert_eq!(after.epoch, before.epoch);
        assert!(Arc::ptr_eq(
            before.templates.as_ref().unwrap(),
            after.templates.as_ref().unwrap()
        ));
        assert!(Arc::ptr_eq(
            before.traders.as_ref().unwrap(),
            after.traders.as_ref().unwrap()
        ));
        assert!(Arc::ptr_eq(
            before.globals.as_ref().unwrap(),
            after.globals.as_ref().unwrap()
        ));
    }

    #[test]
    fn full_publish_derives_ragfair_views_and_a_republish_rederives() {
        let _guard = tests::DB_TEST_LOCK.lock().unwrap();
        clear();

        // Not all three roots resident yet: no views
        publish(request(r#"{"schema":1,"roots":{"templates":{"a":1}}}"#)).unwrap();
        assert!(current().unwrap().ragfair_views.is_none());

        // All three resident (junk roots parse as empty typed containers): views derive
        publish(request(
            r#"{"schema":1,"roots":{"traders":{"b":2},"globals":{"c":3}}}"#,
        ))
        .unwrap();
        let first = current().unwrap();
        let first_views = first.ragfair_views.as_ref().expect("views derived");

        // A templates-only republish re-derives even though traders/globals are unchanged
        publish(request(r#"{"schema":1,"roots":{"templates":{"a":9}}}"#)).unwrap();
        let second = current().unwrap();
        let second_views = second.ragfair_views.as_ref().expect("views re-derived");
        assert!(!Arc::ptr_eq(first_views, second_views));
    }

    #[test]
    fn full_publish_derives_quest_views_and_a_republish_rederives() {
        let _guard = tests::DB_TEST_LOCK.lock().unwrap();
        clear();

        // Ragfair's three roots resident but no locations: ragfair views derive, quest views don't
        publish(request(
            r#"{"schema":1,"roots":{"templates":{"a":1},"traders":{"b":2},"globals":{"c":3}}}"#,
        ))
        .unwrap();
        let partial = current().unwrap();
        assert!(partial.ragfair_views.is_some());
        assert!(partial.quest_views.is_none());

        // Locations arrives: all four roots resident, quest views derive
        publish(request(
            r#"{"schema":1,"roots":{"locations":{"factory4_day":{}}}}"#,
        ))
        .unwrap();
        let first = current().unwrap();
        let first_views = first.quest_views.as_ref().expect("quest views derived");

        // A templates-only republish re-derives even though the other roots are unchanged
        publish(request(r#"{"schema":1,"roots":{"templates":{"a":9}}}"#)).unwrap();
        let second = current().unwrap();
        let second_views = second.quest_views.as_ref().expect("quest views re-derived");
        assert!(!Arc::ptr_eq(first_views, second_views));
    }

    #[test]
    fn full_publish_derives_bot_views_and_a_republish_rederives() {
        let _guard = tests::DB_TEST_LOCK.lock().unwrap();
        clear();

        // Templates+globals resident but no traders: no ragfair views, so no bot views either
        publish(request(
            r#"{"schema":1,"roots":{"templates":{"a":1},"globals":{"c":3,"config":{"exp":{"level":{"exp_table":[{"exp":10},{"exp":20}]}}}}}}"#,
        ))
        .unwrap();
        let partial = current().unwrap();
        assert!(partial.ragfair_views.is_none());
        assert!(partial.bot_views.is_none());

        // Traders arrives: templates+globals+ragfair views all resident, bot views derive
        publish(request(r#"{"schema":1,"roots":{"traders":{"b":2}}}"#)).unwrap();
        let first = current().unwrap();
        let first_views = first.bot_views.as_ref().expect("bot views derived");
        assert_eq!(first_views.exp_table, vec![10, 20]);
        // The embedded ragfair views are the resident Arc, not a second derivation
        assert!(Arc::ptr_eq(
            &first_views.ragfair,
            first.ragfair_views.as_ref().unwrap()
        ));

        // A templates-only republish re-derives even though globals/traders are unchanged
        publish(request(r#"{"schema":1,"roots":{"templates":{"a":9}}}"#)).unwrap();
        let second = current().unwrap();
        let second_views = second.bot_views.as_ref().expect("bot views re-derived");
        assert!(!Arc::ptr_eq(first_views, second_views));
    }

    #[test]
    fn bot_derivation_panic_is_a_views_error_and_does_not_poison_the_lock() {
        let _guard = tests::DB_TEST_LOCK.lock().unwrap();
        use std::sync::atomic::Ordering;
        clear();
        publish(request(
            r#"{"schema":1,"roots":{"templates":{"a":1},"traders":{"b":2},"globals":{"c":3}}}"#,
        ))
        .unwrap();
        let before = current().unwrap();

        tests::PANIC_ON_BOT_DERIVE.store(true, Ordering::Relaxed);
        let error = publish(request(r#"{"schema":1,"roots":{"templates":{"a":9}}}"#)).unwrap_err();
        tests::PANIC_ON_BOT_DERIVE.store(false, Ordering::Relaxed);
        assert!(matches!(error, PublishError::Views(_)));

        // The lock is not poisoned: reads and a follow-up publish still succeed
        let after = current().unwrap();
        assert_eq!(after.epoch, before.epoch);
        let healed = publish(request(r#"{"schema":1,"roots":{"templates":{"a":9}}}"#)).unwrap();
        assert_eq!(healed, before.epoch + 1);
    }

    #[test]
    fn derivation_error_aborts_the_publish_before_the_swap() {
        let _guard = tests::DB_TEST_LOCK.lock().unwrap();
        clear();
        publish(request(
            r#"{"schema":1,"roots":{"templates":{"a":1},"traders":{"b":2},"globals":{"c":3},"locations":{"factory4_day":{}}}}"#,
        ))
        .unwrap();
        let before = current().unwrap();

        // A globals root that parses but fails derivation: an items-less preset is the C#
        // PresetController.Initialize NullReferenceException, surfaced as PublishError::Views
        let error = publish(request(
            r#"{"schema":1,"roots":{"globals":{"ItemPresets":{"bad":{"_id":"bad","_items":[]}}}}}"#,
        ))
        .unwrap_err();
        assert!(matches!(error, PublishError::Views(_)));

        let after = current().unwrap();
        assert_eq!(after.epoch, before.epoch);
        assert!(Arc::ptr_eq(
            before.globals.as_ref().unwrap(),
            after.globals.as_ref().unwrap()
        ));
        assert!(Arc::ptr_eq(
            before.ragfair_views.as_ref().unwrap(),
            after.ragfair_views.as_ref().unwrap()
        ));
        assert!(Arc::ptr_eq(
            before.quest_views.as_ref().unwrap(),
            after.quest_views.as_ref().unwrap()
        ));
    }

    #[test]
    fn quest_derivation_panic_is_a_views_error_and_does_not_poison_the_lock() {
        let _guard = tests::DB_TEST_LOCK.lock().unwrap();
        use std::sync::atomic::Ordering;
        clear();
        publish(request(
            r#"{"schema":1,"roots":{"templates":{"a":1},"traders":{"b":2},"globals":{"c":3},"locations":{"factory4_day":{}}}}"#,
        ))
        .unwrap();
        let before = current().unwrap();

        tests::PANIC_ON_QUEST_DERIVE.store(true, Ordering::Relaxed);
        let error = publish(request(r#"{"schema":1,"roots":{"templates":{"a":9}}}"#)).unwrap_err();
        tests::PANIC_ON_QUEST_DERIVE.store(false, Ordering::Relaxed);
        assert!(matches!(error, PublishError::Views(_)));

        // The lock is not poisoned: reads and a follow-up publish still succeed
        let after = current().unwrap();
        assert_eq!(after.epoch, before.epoch);
        let healed = publish(request(r#"{"schema":1,"roots":{"templates":{"a":9}}}"#)).unwrap();
        assert_eq!(healed, before.epoch + 1);
    }

    #[test]
    fn derivation_panic_is_a_views_error_and_does_not_poison_the_lock() {
        let _guard = tests::DB_TEST_LOCK.lock().unwrap();
        use std::sync::atomic::Ordering;
        clear();
        publish(request(
            r#"{"schema":1,"roots":{"templates":{"a":1},"traders":{"b":2},"globals":{"c":3}}}"#,
        ))
        .unwrap();
        let before = current().unwrap();

        tests::PANIC_ON_DERIVE.store(true, Ordering::Relaxed);
        let error = publish(request(r#"{"schema":1,"roots":{"templates":{"a":9}}}"#)).unwrap_err();
        tests::PANIC_ON_DERIVE.store(false, Ordering::Relaxed);
        assert!(matches!(error, PublishError::Views(_)));

        // The lock is not poisoned: reads and a follow-up publish still succeed
        let after = current().unwrap();
        assert_eq!(after.epoch, before.epoch);
        let healed = publish(request(r#"{"schema":1,"roots":{"templates":{"a":9}}}"#)).unwrap();
        assert_eq!(healed, before.epoch + 1);
    }

    #[test]
    fn unknown_root_name_fails_the_parse() {
        // deny_unknown_fields on PublishRoots: a typo'd root is a deserialize error,
        // which ffi.rs maps to STATUS_BAD_ARGS
        let result: Result<models::PublishRequest, _> =
            serde_json::from_str(r#"{"schema":1,"roots":{"tempaltes":{}}}"#);
        assert!(result.is_err());
    }
}

#[cfg(test)]
mod digest_tests {
    use super::*;

    #[test]
    fn canonical_digest_ignores_object_key_order() {
        let a: serde_json::Value = serde_json::from_str(r#"{"a":1,"b":[{"x":1,"y":2}]}"#).unwrap();
        let b: serde_json::Value = serde_json::from_str(r#"{"b":[{"y":2,"x":1}],"a":1}"#).unwrap();
        assert_eq!(canonical_digest(&a), canonical_digest(&b));
    }

    #[test]
    fn canonical_digest_sees_order_in_the_order_contract_maps() {
        // Same entries, permuted map order, equal digests would let a projection change that
        // reorders these maps pass the equivalence gate while epoch 1 — live since the load-epoch
        // seed — feeds different RNG draws / iteration / last-wins resolution than a republish.
        // Both review reports reproduced that escape; this pins the fix per map.
        let a: models::TemplatesRoot =
            serde_json::from_str(r#"{"prices":{"aaa":1.0,"bbb":2.0}}"#).unwrap();
        let b: models::TemplatesRoot =
            serde_json::from_str(r#"{"prices":{"bbb":2.0,"aaa":1.0}}"#).unwrap();
        assert_ne!(
            canonical_digest(&a),
            canonical_digest(&b),
            "prices order is what GetFleaPricesAsArray draws an index into"
        );

        let a: models::TemplatesRoot =
            serde_json::from_str(r#"{"items":{"aaa":{},"bbb":{}}}"#).unwrap();
        let b: models::TemplatesRoot =
            serde_json::from_str(r#"{"items":{"bbb":{},"aaa":{}}}"#).unwrap();
        assert_ne!(
            canonical_digest(&a),
            canonical_digest(&b),
            "items order is what the ragfair view caches iterate"
        );

        let a: models::GlobalsRoot =
            serde_json::from_str(r#"{"ItemPresets":{"aaa":{},"bbb":{}}}"#).unwrap();
        let b: models::GlobalsRoot =
            serde_json::from_str(r#"{"ItemPresets":{"bbb":{},"aaa":{}}}"#).unwrap();
        assert_ne!(
            canonical_digest(&a),
            canonical_digest(&b),
            "ItemPresets order is last-preset-wins in the default cache"
        );
    }

    #[test]
    fn canonical_digest_distinguishes_array_order_and_values() {
        let a: serde_json::Value = serde_json::from_str(r#"[1,2]"#).unwrap();
        let b: serde_json::Value = serde_json::from_str(r#"[2,1]"#).unwrap();
        assert_ne!(canonical_digest(&a), canonical_digest(&b));

        // Object nesting is distinguished too — without the object arm's length hash these two
        // feed the hasher the same key/value stream.
        let flat: serde_json::Value = serde_json::from_str(r#"{"a":{},"b":1}"#).unwrap();
        let nested: serde_json::Value = serde_json::from_str(r#"{"a":{"b":1}}"#).unwrap();
        assert_ne!(canonical_digest(&flat), canonical_digest(&nested));
    }

    #[test]
    fn canonical_digest_is_a_function_of_its_input() {
        // Two independent parses of the same bytes must digest equal — a set-typed lift with
        // per-instance iteration order (the Task 4 HashSet bug) fails this where same-value
        // re-serialization cannot.
        let bytes = r#"{"repeatableQuests":{"data":{"Completion":{
            "itemsWhitelist":[
                {"minPlayerLevel":1,"itemIds":["54009119af1c881c07000029","5448e54d4bdc2dcc718b4568",
                 "5448e5284bdc2dcb718b4567","543be5cb4bdc2deb348b4568","5448f3a64bdc2d60728b456a",
                 "5447e1d04bdc2dff2f8b4567"]},
                {"minPlayerLevel":15,"itemIds":["5448bc234bdc2d3c308b4569","543be6564bdc2df4348b4568",
                 "5447b5cf4bdc2d65278b4567"]}],
            "itemsBlacklist":[
                {"minPlayerLevel":1,"itemIds":["5448bf274bdc2dfc2f8b456a","5671435f4bdc2d96058b4569",
                 "5448e53e4bdc2d60728b4567"]}]}}}}"#;
        let a: models::TemplatesRoot = serde_json::from_str(bytes).unwrap();
        let b: models::TemplatesRoot = serde_json::from_str(bytes).unwrap();
        assert_eq!(canonical_digest(&a), canonical_digest(&b));
    }

    #[test]
    fn canonical_digest_sees_lifts_not_extra() {
        // Same lift (production.scavRecipes), different unlifted stems: equal digests — the
        // digest surface is the read surface, not post-parse byte fidelity (spec § Part 0).
        let a: models::HideoutRoot =
            serde_json::from_str(r#"{"production":{"scavRecipes":[]},"areas":[1,2]}"#).unwrap();
        let b: models::HideoutRoot =
            serde_json::from_str(r#"{"production":{"scavRecipes":[]},"qte":{"x":1}}"#).unwrap();
        assert_eq!(canonical_digest(&a), canonical_digest(&b));

        // A differing lift is seen (recipe body copied from the known-good store test).
        let c: models::HideoutRoot = serde_json::from_str(
            r#"{"production":{"scavRecipes":[
                {"_id":"6662e9aca7e0b43baa3d5f9c",
                 "endProducts":{"Common":{"min":1,"max":2},"Rare":{"min":0,"max":1},"Superrare":{"min":0,"max":0}},
                 "productionTime":3.0}
            ]}}"#,
        )
        .unwrap();
        assert_ne!(canonical_digest(&a), canonical_digest(&c));
    }

    #[test]
    fn extra_members_still_serialize_outside_digest_mode() {
        // The loot models serialize these types into production responses: the skip must be
        // digest-mode-only.
        let root: models::HideoutRoot =
            serde_json::from_str(r#"{"production":{"scavRecipes":[]},"areas":[1]}"#).unwrap();
        let value = serde_json::to_value(&root).unwrap();
        assert!(
            value.get("areas").is_some(),
            "production serialization keeps extra content"
        );
    }

    #[test]
    fn resident_digests_json_is_empty_before_any_publish_and_filled_after() {
        let _guard = tests::DB_TEST_LOCK.lock().unwrap();
        clear();
        assert_eq!(
            resident_digests_json(),
            br#"{"epoch":0,"roots":{}}"#.to_vec()
        );

        let request: models::PublishRequest = serde_json::from_str(
            r#"{"schema":1,"roots":{"configs":{"spt-item":{"kind":"spt-item"}}}}"#,
        )
        .unwrap();
        publish(request).unwrap();

        let json = resident_digests_json();
        let value: serde_json::Value = serde_json::from_slice(&json).unwrap();
        assert_eq!(value["epoch"], 1);
        assert!(
            value["roots"]["configs"].is_string(),
            "a resident root digests"
        );
        assert!(
            value["roots"].get("templates").is_none(),
            "an absent root is omitted"
        );
        // Deterministic within the process:
        assert_eq!(json, resident_digests_json());
        clear();
    }
}
