//! Phase 3: fused SPT_Data load. One walk hashes (when verifying) and reads database/,
//! installs the five resident roots via db::publish (first epoch of the process), and
//! returns the eager file bytes for the C# replica.
//! Callable with no CLR alive — the post-6b exe loads SPT_Data before booting the runtime.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::db::models::PublishRequest;
use crate::verify::{self, VerifyReport};

/// Mirror of ImporterUtil's skip lists (ImporterUtil.cs:17-19). Compared lowercased.
const SKIPPED_FILES: [&str; 3] = ["bearsuits.json", "usecsuits.json", "archivedquests.json"];
const SKIPPED_DIR_PREFIXES: [&str; 2] = ["database/locales/server/", "database/locales/web/"];

/// The per-map members `DbPayloadProjection` writes (`DbPayloadProjection.cs:60-80`) — the
/// locations root is exactly these, never `looseLoot`/`staticAmmo`.
const LOCATION_MEMBERS: [&str; 5] = [
    "base",
    "allExtracts",
    "staticLoot",
    "staticContainers",
    "statics",
];

const GLOBALS_KEY: &str = "database/globals.json";
const HANDBOOK_KEY: &str = "database/templates/handbook.json";

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadRequest {
    /// Must be 1.
    pub schema: u32,
    /// e.g. `"./SPT_Data/"` — the same relative-path contract as `spt_verify_database`.
    pub dir: String,
    pub verify: bool,
    /// `ItemConfig.HandbookPriceOverride`, applied to `templates/handbook.json` exactly as
    /// `HydrateHandbookCache` does (HandbookHelper.cs:26-49) so the epoch-1 handbook equals a
    /// published one — visible to the publish envelope only, never to the returned `files`.
    /// Absent when no CLR is alive to supply live config values (post-6b pre-load).
    #[serde(default)]
    pub handbook_price_override: Option<indexmap::IndexMap<String, HandbookPriceOverrideWire>>,
}

/// One `ItemConfig.HandbookPriceOverride` entry as the C# wrapper sends it.
#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HandbookPriceOverrideWire {
    pub parent_id: String,
    /// `serde_json::Number` so the value crosses with C#'s own formatting, no float round-trip.
    pub price: serde_json::Number,
}

#[derive(Debug)]
pub struct LoadResponse {
    /// `Some` iff the request asked to verify.
    pub verify: Option<VerifyReport>,
    /// `None` iff verification failed — nothing was installed.
    pub epoch: Option<u64>,
    /// Manifest-style keys (`database/…`), BOM-stripped, sorted.
    pub files: Vec<(String, Vec<u8>)>,
}

#[derive(Debug)]
pub enum LoadError {
    /// `schema != 1`.
    BadArgs(String),
    /// Missing `database/` tree, an unreadable file outside the verify arm, or a missing
    /// required root source.
    Io(String),
    /// The assembled roots failed to parse or to derive.
    Publish(crate::db::PublishError),
}

/// What the fused walk does with a `database/` file, keyed by its manifest-style relative key.
#[derive(Debug, PartialEq)]
enum FileClass {
    /// Hash-only (verify arm): importer-skipped, non-`.json`, or outside `database/`.
    HashOnly,
    /// Never read: C# holds it as a disk-path `LazyLoad` and re-reads per access.
    /// `locales/global/*` and `locations/*/looseLoot.json` (residency declined — see plan).
    LazyNeverRead,
    /// Read for resident-root assembly, NOT returned (C#'s `LazyLoad` ignores bytes):
    /// `locations/*/staticLoot.json`, `locations/*/staticContainers.json`.
    AssemblyOnly,
    /// Read, returned to C#, and spliced into a resident root where one claims it.
    Eager,
}

fn classify(rel: &str) -> FileClass {
    let Some(tail) = rel.strip_prefix("database/") else {
        return FileClass::HashOnly;
    };
    if !rel.ends_with(".json")
        || SKIPPED_DIR_PREFIXES
            .iter()
            .any(|prefix| rel.starts_with(prefix))
    {
        return FileClass::HashOnly;
    }

    let name = tail.rsplit('/').next().unwrap_or(tail);
    if SKIPPED_FILES
        .iter()
        .any(|skipped| name.eq_ignore_ascii_case(skipped))
    {
        return FileClass::HashOnly;
    }
    if tail.starts_with("locales/global/") {
        return FileClass::LazyNeverRead;
    }
    if tail.starts_with("locations/") {
        return match name {
            "looseLoot.json" => FileClass::LazyNeverRead,
            "staticLoot.json" | "staticContainers.json" => FileClass::AssemblyOnly,
            _ => FileClass::Eager,
        };
    }

    FileClass::Eager
}

/// The files worth reading: everything a resident root needs, plus everything C# gets back.
fn want(rel: &str) -> bool {
    matches!(classify(rel), FileClass::AssemblyOnly | FileClass::Eager)
}

pub(crate) fn strip_bom(mut bytes: Vec<u8>) -> Vec<u8> {
    if bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        bytes.drain(..3);
    }
    bytes
}

pub async fn load(request: LoadRequest) -> Result<LoadResponse, LoadError> {
    if request.schema != 1 {
        return Err(LoadError::BadArgs(format!(
            "unsupported load schema {}",
            request.schema
        )));
    }

    let spt_data = PathBuf::from(&request.dir);
    let (report, collected) = if request.verify {
        let (report, collected) = verify::verify_collecting(spt_data, want).await;
        // A failing report may have collected only part of the tree (bytes are kept solely for
        // files that hashed clean), so gate on `ok`, never on what came back.
        if !report.ok {
            return Ok(LoadResponse {
                verify: Some(report),
                epoch: None,
                files: Vec::new(),
            });
        }
        (Some(report), collected)
    } else {
        (None, read_database(&spt_data).await?)
    };

    let mut raw: BTreeMap<String, Vec<u8>> = collected
        .into_iter()
        .map(|(key, bytes)| (key, strip_bom(bytes)))
        .collect();
    require_root_sources(&raw)?;

    // Merged for the envelope only: the returned `files` must stay the raw disk bytes
    // (spec non-goal — and the equivalence gate's C# arm must hydrate from raw bytes).
    let original_handbook = match &request.handbook_price_override {
        Some(overrides) => apply_handbook_overrides(&mut raw, overrides)?,
        None => None,
    };

    let envelope = assemble_publish_envelope(&raw);
    let publish_request: PublishRequest = serde_json::from_slice(&envelope)
        .map_err(|e| LoadError::Publish(crate::db::PublishError::Schema(e.to_string())))?;
    let epoch = crate::db::publish(publish_request).map_err(LoadError::Publish)?;

    if let Some(original) = original_handbook {
        raw.insert(HANDBOOK_KEY.to_string(), original);
    }

    Ok(LoadResponse {
        verify: report,
        epoch: Some(epoch),
        files: raw
            .into_iter()
            .filter(|(key, _)| classify(key) == FileClass::Eager)
            .collect(),
    })
}

/// The no-verify arm (Debug builds ship no `checks.dat`): the same walk and the same
/// relativization `verify::collect_files` does, minus the hashing.
// ponytail: sequential reads — this arm only runs in Debug, where nothing measures; the verify
// arm keeps verify.rs's 32-way concurrency.
async fn read_database(spt_data: &Path) -> Result<Vec<(String, Vec<u8>)>, LoadError> {
    let database = spt_data.join("database");
    let mut collected = Vec::new();
    for entry in walkdir::WalkDir::new(&database) {
        let entry =
            entry.map_err(|e| LoadError::Io(format!("cannot walk {}: {e}", database.display())))?;
        if !entry.file_type().is_file() {
            continue;
        }
        let Ok(rel) = entry.path().strip_prefix(spt_data) else {
            continue;
        };
        let key = rel.to_string_lossy().replace('\\', "/");
        if !want(&key) {
            continue;
        }
        let bytes = tokio::fs::read(entry.path())
            .await
            .map_err(|e| LoadError::Io(format!("cannot read {key}: {e}")))?;
        collected.push((key, bytes));
    }

    Ok(collected)
}

/// A partial tree must fail loudly rather than install junk roots: every resident root needs at
/// least one source file on disk.
fn require_root_sources(raw: &BTreeMap<String, Vec<u8>>) -> Result<(), LoadError> {
    let under = |dir: &str, segments: usize| {
        raw.keys()
            .any(|key| key.starts_with(dir) && key.split('/').count() == segments)
    };
    let require = |what: &str, present: bool| {
        if present {
            Ok(())
        } else {
            Err(LoadError::Io(format!("missing {what}")))
        }
    };

    require("database/templates/*.json", under("database/templates/", 3))?;
    require("database/traders/*/*.json", under("database/traders/", 4))?;
    require(GLOBALS_KEY, raw.contains_key(GLOBALS_KEY))?;
    require(
        "database/locations/*/*.json",
        under("database/locations/", 4),
    )?;
    require("database/hideout/*.json", under("database/hideout/", 3))?;

    Ok(())
}

/// Mirror of HandbookHelper.HydrateHandbookCache's upsert (HandbookHelper.cs:26-49): find each
/// override's handbook item by Id, append `{Id, ParentId, Price}` at the end of Items when
/// missing, then overwrite Price and ParentId — in override document order. Replaces the raw
/// handbook entry in `raw` and returns the original bytes; the caller restores them after
/// envelope assembly so the merge is visible to the publish only and `files` stays raw.
/// Gated by ResidentRootEquivalenceTests.
fn apply_handbook_overrides(
    raw: &mut BTreeMap<String, Vec<u8>>,
    overrides: &indexmap::IndexMap<String, HandbookPriceOverrideWire>,
) -> Result<Option<Vec<u8>>, LoadError> {
    if overrides.is_empty() {
        return Ok(None);
    }
    let Some(bytes) = raw.get(HANDBOOK_KEY) else {
        // No handbook source at all — the C# arm would be hydrating into an absent table too.
        // Unreachable on a real tree (require_root_sources demands templates/ content).
        return Ok(None);
    };
    let mut handbook: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|error| LoadError::Io(format!("{HANDBOOK_KEY}: {error}")))?;
    let items = handbook
        .as_object_mut()
        .and_then(|object| object.get_mut("Items"))
        .and_then(serde_json::Value::as_array_mut)
        .ok_or_else(|| LoadError::Io(format!("{HANDBOOK_KEY} has no Items array")))?;
    for (id, price_override) in overrides {
        let position = items
            .iter()
            .position(|item| item.get("Id").and_then(serde_json::Value::as_str) == Some(id));
        let index = match position {
            Some(index) => index,
            None => {
                items.push(serde_json::json!({ "Id": id }));
                items.len() - 1
            }
        };
        let item = items[index]
            .as_object_mut()
            .ok_or_else(|| LoadError::Io(format!("{HANDBOOK_KEY}: non-object Items entry")))?;
        item.insert(
            "Price".to_string(),
            serde_json::Value::Number(price_override.price.clone()),
        );
        item.insert(
            "ParentId".to_string(),
            serde_json::Value::String(price_override.parent_id.clone()),
        );
    }
    let merged = serde_json::to_vec(&handbook).map_err(|error| LoadError::Io(error.to_string()))?;

    Ok(raw.insert(HANDBOOK_KEY.to_string(), merged))
}

/// Splices the raw file bytes into exactly the publish shape `DbPayloadProjection` emits — the
/// only file→wire mapping Rust owns. Every member name is a filename or directory stem, which
/// already equals the wire name `db/models.rs` pins.
fn assemble_publish_envelope(raw: &BTreeMap<String, Vec<u8>>) -> Vec<u8> {
    // ~60 MB on the shipped tree, so grow it once. Every body is spliced at most once, which makes
    // the raw total an upper bound on file bytes; 64 per entry covers the spliced key and its
    // punctuation (trader ids, the widest keys, are 24 characters).
    let capacity = raw.values().map(Vec::len).sum::<usize>() + raw.len() * 64 + 256;
    let mut out = Vec::with_capacity(capacity);
    out.extend_from_slice(br#"{"schema":1,"roots":{"templates":"#);
    write_stem_object(&mut out, raw, "database/templates/");
    out.extend_from_slice(br#","traders":"#);
    write_directory_object(&mut out, raw, "database/traders/", write_stem_object);
    out.extend_from_slice(br#","globals":"#);
    // require_root_sources ran first, so globals is present.
    out.extend_from_slice(&raw[GLOBALS_KEY]);
    out.extend_from_slice(br#","locations":"#);
    write_directory_object(&mut out, raw, "database/locations/", write_location);
    out.extend_from_slice(br#","hideout":"#);
    write_stem_object(&mut out, raw, "database/hideout/");
    out.extend_from_slice(b"}}");

    out
}

/// `"name":`, with the separating comma when it is not the first member written.
fn push_key(out: &mut Vec<u8>, first: &mut bool, name: &str) {
    if !*first {
        out.push(b',');
    }
    *first = false;
    let escaped = serde_json::to_string(name).expect("a string always serializes");
    out.extend_from_slice(escaped.as_bytes());
    out.push(b':');
}

/// One member per `<dir><stem>.json` file, keyed by the stem — `templates`, `hideout`, and each
/// trader directory. Files in nested directories are not members.
fn write_stem_object(out: &mut Vec<u8>, raw: &BTreeMap<String, Vec<u8>>, dir: &str) {
    out.push(b'{');
    let mut first = true;
    for (key, bytes) in raw {
        let Some(stem) = key
            .strip_prefix(dir)
            .and_then(|name| name.strip_suffix(".json"))
        else {
            continue;
        };
        if stem.contains('/') {
            continue;
        }
        push_key(out, &mut first, stem);
        out.extend_from_slice(bytes);
    }
    out.push(b'}');
}

/// One member per immediate subdirectory of `dir`, its body written by `write_body`. Files
/// directly under `dir` are skipped — that is how `database/locations/base.json`, the
/// UI-linkage key `DbPayloadProjection` never serializes, stays out of the locations root.
fn write_directory_object(
    out: &mut Vec<u8>,
    raw: &BTreeMap<String, Vec<u8>>,
    dir: &str,
    write_body: fn(&mut Vec<u8>, &BTreeMap<String, Vec<u8>>, &str),
) {
    // Sorted keys group each subdirectory's files together, so dedup collapses them to names.
    let mut names: Vec<&str> = raw
        .keys()
        .filter_map(|key| key.strip_prefix(dir))
        .filter_map(|rest| rest.split_once('/').map(|(name, _)| name))
        .collect();
    names.dedup();

    out.push(b'{');
    let mut first = true;
    for name in names {
        push_key(out, &mut first, name);
        write_body(out, raw, &format!("{dir}{name}/"));
    }
    out.push(b'}');
}

/// One map of the locations root: [`LOCATION_MEMBERS`] only, absent members left absent (serde
/// `default` gives the same views the publish's `[]`/`null` do).
fn write_location(out: &mut Vec<u8>, raw: &BTreeMap<String, Vec<u8>>, dir: &str) {
    out.push(b'{');
    let mut first = true;
    for member in LOCATION_MEMBERS {
        let Some(bytes) = raw.get(&format!("{dir}{member}.json")) else {
            continue;
        };
        push_key(out, &mut first, member);
        out.extend_from_slice(bytes);
    }
    out.push(b'}');
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use crate::loot::item_helper::{MONEY, WEAPON};
    use std::fs;
    use tempfile::TempDir;

    // --- The flip #4 publish envelope's root bodies (tests/flip4_loot_resident.rs), written back
    // --- out as the files the C# importer would have loaded them from. flip #4 publishes
    // --- `"traders":{}` and no hideout root, neither of which a file tree can express: the empty
    // --- object moves down to the one trader's base.json, and the hideout root is flip #5's.

    const ITEM_NODE: &str = "54009119af1c881c07000029";
    const CONTAINER_TPL: &str = "111111111111111111111111";
    const MONEY_TPL: &str = "333333333333333333333333";
    const MISC_NODE: &str = "misc_node";
    const WEAPON_TPL: &str = "weapon_tpl";
    const MOD_A_TPL: &str = "weapon_mod_a";
    const MOD_B_TPL: &str = "weapon_mod_b";
    /// Prapor, so the trader directory carries a name the C# importer's MongoId branch accepts.
    const TRADER_ID: &str = "54cb50c76803fa8b248b4571";

    fn items_json() -> String {
        format!(
            r#"{{
                "{ITEM_NODE}":{{"_type":"Node","_parent":"","_props":{{}}}},
                "{MONEY}":{{"_type":"Node","_parent":"{ITEM_NODE}","_props":{{}}}},
                "{WEAPON}":{{"_type":"Node","_parent":"{ITEM_NODE}","_props":{{}}}},
                "{MISC_NODE}":{{"_type":"Node","_parent":"{ITEM_NODE}","_props":{{}}}},
                "{CONTAINER_TPL}":{{"_parent":"{ITEM_NODE}","_props":{{"Width":1,"Height":1,
                    "Grids":[{{"_props":{{"cellsH":2,"cellsV":2}}}}]}}}},
                "{MONEY_TPL}":{{"_parent":"{MONEY}","_props":{{"Width":1,"Height":1,
                    "StackMaxSize":500000,"StackMinRandom":100,"StackMaxRandom":200}}}},
                "{WEAPON_TPL}":{{"_parent":"{WEAPON}","_props":{{}}}},
                "{MOD_A_TPL}":{{"_parent":"{MISC_NODE}","_props":{{}}}},
                "{MOD_B_TPL}":{{"_parent":"{MISC_NODE}","_props":{{}}}}
            }}"#
        )
    }

    fn globals_json() -> String {
        format!(
            r#"{{"ItemPresets":{{
                "p1":{{"_id":"p1","_name":"weapon_default","_encyclopedia":"{WEAPON_TPL}",
                    "_items":[{{"_id":"root_p1","_tpl":"{WEAPON_TPL}"}},
                        {{"_id":"mod_p1","_tpl":"{MOD_A_TPL}","parentId":"root_p1","slotId":"mod_stock"}}]}},
                "p2":{{"_id":"p2","_name":"weapon_alt",
                    "_items":[{{"_id":"root_p2","_tpl":"{WEAPON_TPL}"}}]}}
            }}}}"#
        )
    }

    fn static_loot_json() -> String {
        format!(
            r#"{{"{CONTAINER_TPL}":{{
                "itemcountDistribution":[{{"count":2,"relativeProbability":1}}],
                "itemDistribution":[{{"tpl":"{MONEY_TPL}","relativeProbability":1}}]}}}}"#
        )
    }

    fn static_containers_json() -> String {
        format!(
            r#"{{"staticWeapons":[],"staticForced":[],"staticContainers":[
                {{"probability":1.0,"template":{{"Id":"c1","IsContainer":true,
                    "Root":"aaaaaaaaaaaaaaaaaaaaaaa1",
                    "Items":[{{"_id":"aaaaaaaaaaaaaaaaaaaaaaa1","_tpl":"{CONTAINER_TPL}"}}]}}}}]}}"#
        )
    }

    /// flip #5's hideout root body, as `database/hideout/production.json`.
    const PRODUCTION_JSON: &str = r#"{"scavRecipes":[
        {"_id":"6662e9aca7e0b43baa3d5f9c","endProducts":{"Common":{"min":3,"max":3},
            "Rare":{"min":1,"max":1},"Superrare":{"min":0,"max":0}}}]}"#;

    const STATICS_JSON: &str = r#"{"containersGroups":{"g1":{"minContainers":1,"maxContainers":2}},
        "containers":{"c1":{"groupId":"g1"}}}"#;

    /// Every file class in one tree: eager roots, the assembly-only statics, the lazy
    /// never-read pair, and one file from each of the importer's two skip rules.
    /// `pub` for `ffi.rs`'s transport tests, which need a tree that really installs.
    pub fn mini_tree() -> TempDir {
        let dir = TempDir::new().unwrap();
        let files: Vec<(String, String)> = vec![
            ("database/templates/items.json".into(), items_json()),
            (
                "database/templates/handbook.json".into(),
                r#"{"Items":[]}"#.into(),
            ),
            ("database/templates/prices.json".into(), "{}".into()),
            ("database/templates/archivedQuests.json".into(), "{}".into()),
            (
                format!("database/traders/{TRADER_ID}/base.json"),
                "{}".into(),
            ),
            (
                format!("database/traders/{TRADER_ID}/bearsuits.json"),
                "[]".into(),
            ),
            ("database/globals.json".into(), globals_json()),
            ("database/locations/base.json".into(), "{}".into()),
            (
                "database/locations/bigmap/base.json".into(),
                r#"{"Id":"bigmap"}"#.into(),
            ),
            (
                "database/locations/bigmap/allExtracts.json".into(),
                "[]".into(),
            ),
            (
                "database/locations/bigmap/staticLoot.json".into(),
                static_loot_json(),
            ),
            (
                "database/locations/bigmap/staticContainers.json".into(),
                static_containers_json(),
            ),
            (
                "database/locations/bigmap/statics.json".into(),
                STATICS_JSON.into(),
            ),
            (
                "database/locations/bigmap/looseLoot.json".into(),
                "{}".into(),
            ),
            (
                "database/hideout/production.json".into(),
                PRODUCTION_JSON.into(),
            ),
            ("database/locales/menu/en.json".into(), "{}".into()),
            ("database/locales/global/en.json".into(), "{}".into()),
            ("database/locales/server/en.json".into(), "{}".into()),
        ];
        for (rel, body) in files {
            let path = dir.path().join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, body).unwrap();
        }

        dir
    }

    fn request(dir: &TempDir, verify: bool) -> LoadRequest {
        LoadRequest {
            schema: 1,
            dir: dir.path().to_string_lossy().into_owned(),
            verify,
            handbook_price_override: None,
        }
    }

    fn keys(response: &LoadResponse) -> Vec<String> {
        response.files.iter().map(|(key, _)| key.clone()).collect()
    }

    /// Store-touching tests hold `DB_TEST_LOCK` (a plain `Mutex`) for their whole body, so they
    /// stay synchronous and drive `load` through a runtime of their own.
    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a test runtime")
            .block_on(future)
    }

    /// The same recipe `verify.rs`'s own tests use: base64 of `[{"Path":…,"Hash":…}]`, over every
    /// file in the tree (verification is bidirectional).
    fn write_manifest(dir: &TempDir, tampered: &str) {
        use base64::Engine;

        let mut entries = Vec::new();
        for entry in walkdir::WalkDir::new(dir.path().join("database"))
            .into_iter()
            .flatten()
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let key = entry
                .path()
                .strip_prefix(dir.path())
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            let hash = if key == tampered {
                "00000000000000000000000000000000".to_string()
            } else {
                verify::xxh3_bytes(&fs::read(entry.path()).unwrap())
            };
            entries.push(serde_json::json!({ "Path": key, "Hash": hash }));
        }

        let json = serde_json::to_string(&entries).unwrap();
        let base64 = base64::engine::general_purpose::STANDARD.encode(json.as_bytes());
        fs::write(dir.path().join("checks.dat"), base64).unwrap();
    }

    #[test]
    fn classify_matches_the_importer_skip_and_lazy_rules() {
        let cases = [
            ("database/templates/items.json", FileClass::Eager),
            ("database/bots/types/assault.json", FileClass::Eager),
            ("database/locales/menu/en.json", FileClass::Eager),
            ("database/server.json", FileClass::Eager),
            ("database/locations/base.json", FileClass::Eager),
            ("database/locations/bigmap/statics.json", FileClass::Eager),
            (
                "database/locations/bigmap/staticAmmo.json",
                FileClass::Eager,
            ),
            (
                "database/locations/bigmap/staticLoot.json",
                FileClass::AssemblyOnly,
            ),
            (
                "database/locations/bigmap/staticContainers.json",
                FileClass::AssemblyOnly,
            ),
            (
                "database/locations/bigmap/looseLoot.json",
                FileClass::LazyNeverRead,
            ),
            ("database/locales/global/en.json", FileClass::LazyNeverRead),
            // ImporterUtil's two skip rules, plus non-json and everything outside database/
            ("database/locales/server/en.json", FileClass::HashOnly),
            ("database/locales/web/en.json", FileClass::HashOnly),
            (
                "database/templates/archivedQuests.json",
                FileClass::HashOnly,
            ),
            (
                "database/traders/54cb50c76803fa8b248b4571/bearsuits.json",
                FileClass::HashOnly,
            ),
            (
                "database/traders/54cb50c76803fa8b248b4571/usecsuits.json",
                FileClass::HashOnly,
            ),
            // suits.json is NOT on the skip list — only the two faction files are
            (
                "database/traders/54cb50c76803fa8b248b4571/suits.json",
                FileClass::Eager,
            ),
            ("database/readme.txt", FileClass::HashOnly),
            ("configs/core.json", FileClass::HashOnly),
            ("checks.dat", FileClass::HashOnly),
        ];

        for (rel, expected) in cases {
            assert_eq!(classify(rel), expected, "{rel}");
        }
    }

    #[test]
    fn strip_bom_removes_exactly_the_utf8_bom() {
        assert_eq!(strip_bom(b"\xEF\xBB\xBF{}".to_vec()), b"{}".to_vec());
        assert_eq!(strip_bom(b"{}".to_vec()), b"{}".to_vec());
        // A partial or misplaced BOM is data, not a marker
        assert_eq!(strip_bom(b"\xEF\xBB".to_vec()), b"\xEF\xBB".to_vec());
        assert_eq!(
            strip_bom(b"{\xEF\xBB\xBF}".to_vec()),
            b"{\xEF\xBB\xBF}".to_vec()
        );
    }

    #[test]
    fn load_without_verify_installs_an_epoch_and_returns_eager_files() {
        let _guard = crate::db::tests::DB_TEST_LOCK.lock().unwrap();
        crate::db::clear();

        let dir = mini_tree();
        let response = block_on(load(request(&dir, false))).expect("load succeeds");

        assert!(response.verify.is_none());
        assert_eq!(response.epoch, Some(1));

        let db = crate::db::current().expect("a resident DB");
        assert!(db.templates.is_some());
        assert!(db.traders.is_some());
        assert!(db.globals.is_some());
        assert!(db.locations.is_some());
        assert!(db.hideout.is_some());
        assert!(db.ragfair_views.is_some());
        assert!(db.quest_views.is_some());
        assert!(db.bot_views.is_some());

        // The roots really carry the tree's data, not empty containers
        assert_eq!(db.templates.as_ref().unwrap().items.len(), 9);
        assert_eq!(db.traders.as_ref().unwrap().traders.len(), 1);
        assert_eq!(db.locations.as_ref().unwrap().locations.len(), 1);
        assert_eq!(
            db.hideout.as_ref().unwrap().production.scav_recipes.len(),
            1
        );

        // Sorted, and exactly the eager set: no staticLoot/staticContainers (assembly-only), no
        // looseLoot or locales/global (lazy), no archivedQuests/bearsuits/locales-server (skipped).
        assert_eq!(
            keys(&response),
            vec![
                "database/globals.json".to_string(),
                "database/hideout/production.json".to_string(),
                "database/locales/menu/en.json".to_string(),
                "database/locations/base.json".to_string(),
                "database/locations/bigmap/allExtracts.json".to_string(),
                "database/locations/bigmap/base.json".to_string(),
                "database/locations/bigmap/statics.json".to_string(),
                "database/templates/handbook.json".to_string(),
                "database/templates/items.json".to_string(),
                "database/templates/prices.json".to_string(),
                format!("database/traders/{TRADER_ID}/base.json"),
            ]
        );

        let items = response
            .files
            .iter()
            .find(|(key, _)| key == "database/templates/items.json")
            .expect("items ships to C#");
        assert_eq!(
            items.1,
            fs::read(dir.path().join("database/templates/items.json")).unwrap()
        );
    }

    #[test]
    fn load_with_verify_failure_installs_nothing_and_returns_the_report() {
        let _guard = crate::db::tests::DB_TEST_LOCK.lock().unwrap();
        crate::db::clear();

        let dir = mini_tree();
        // A resident DB to protect: the failing verify must leave this epoch exactly as it is.
        let installed = block_on(load(request(&dir, false))).expect("load succeeds");
        assert_eq!(installed.epoch, Some(1));
        let before = crate::db::current().expect("a resident DB");

        write_manifest(&dir, GLOBALS_KEY);
        let response = block_on(load(request(&dir, true))).expect("load answers");

        let report = response.verify.expect("the report comes back");
        assert!(!report.ok);
        assert_eq!(report.failures[0].path, GLOBALS_KEY);
        assert_eq!(report.failures[0].reason, "hash_mismatch");
        assert_eq!(response.epoch, None);
        assert!(response.files.is_empty());

        let after = crate::db::current().expect("the resident DB survives");
        assert_eq!(after.epoch, before.epoch);
        assert!(std::sync::Arc::ptr_eq(
            before.globals.as_ref().unwrap(),
            after.globals.as_ref().unwrap()
        ));
    }

    #[test]
    fn a_clean_manifest_verifies_and_installs() {
        let _guard = crate::db::tests::DB_TEST_LOCK.lock().unwrap();
        crate::db::clear();

        let dir = mini_tree();
        let plain = block_on(load(request(&dir, false))).expect("load succeeds");

        write_manifest(&dir, "");
        let response = block_on(load(request(&dir, true))).expect("load succeeds");

        let report = response.verify.expect("the report comes back");
        assert!(
            report.ok,
            "failures: {:?}",
            report
                .failures
                .iter()
                .map(|failure| (&failure.path, &failure.reason))
                .collect::<Vec<_>>()
        );
        assert_eq!(response.epoch, Some(2), "the second publish of this test");
        // The two arms read exactly the same files, bytes included
        assert_eq!(response.files, plain.files);
    }

    #[test]
    fn a_missing_required_root_source_is_a_loud_io_error() {
        let _guard = crate::db::tests::DB_TEST_LOCK.lock().unwrap();
        crate::db::clear();

        let dir = mini_tree();
        fs::remove_file(dir.path().join(GLOBALS_KEY)).unwrap();
        let error = block_on(load(request(&dir, false))).unwrap_err();

        match error {
            LoadError::Io(message) => assert_eq!(message, format!("missing {GLOBALS_KEY}")),
            other => panic!("expected an Io error, got {other:?}"),
        }
        assert!(crate::db::current().is_none(), "nothing was installed");
    }

    #[tokio::test]
    async fn a_bad_schema_is_bad_args() {
        let dir = mini_tree();
        let error = load(LoadRequest {
            schema: 2,
            ..request(&dir, false)
        })
        .await
        .unwrap_err();
        assert!(matches!(error, LoadError::BadArgs(_)), "{error:?}");
    }

    #[tokio::test]
    async fn assembled_locations_root_excludes_the_ui_linkage_base_and_loose_loot() {
        let dir = mini_tree();
        let raw: BTreeMap<String, Vec<u8>> = read_database(dir.path())
            .await
            .unwrap()
            .into_iter()
            .map(|(key, bytes)| (key, strip_bom(bytes)))
            .collect();
        let bytes = assemble_publish_envelope(&raw);
        assert!(
            bytes.len() <= raw.values().map(Vec::len).sum::<usize>() + raw.len() * 64 + 256,
            "the envelope outgrew the capacity it preallocates, so it reallocs on the shipped tree"
        );
        let envelope: serde_json::Value =
            serde_json::from_slice(&bytes).expect("the envelope parses");

        let locations = &envelope["roots"]["locations"];
        assert!(
            locations.get("base").is_none(),
            "locations/base.json is the UI-linkage key, never a map"
        );
        let bigmap = &locations["bigmap"];
        assert!(bigmap.get("looseLoot").is_none());
        assert!(bigmap.get("staticAmmo").is_none());
        assert_eq!(bigmap["base"]["Id"], "bigmap");
        assert!(bigmap.get("allExtracts").is_some());
        assert!(bigmap.get("staticLoot").is_some());
        assert!(bigmap.get("staticContainers").is_some());
        assert!(bigmap.get("statics").is_some());

        // The other four roots, keyed by their file/directory stems
        assert!(envelope["roots"]["templates"].get("items").is_some());
        assert!(
            envelope["roots"]["templates"]
                .get("archivedQuests")
                .is_none(),
            "the importer skip list keeps archivedQuests out of the root"
        );
        assert!(envelope["roots"]["globals"].get("ItemPresets").is_some());
        assert!(envelope["roots"]["hideout"].get("production").is_some());
        let trader = &envelope["roots"]["traders"][TRADER_ID];
        assert!(trader.get("base").is_some());
        assert!(
            trader.get("bearsuits").is_none(),
            "the importer skip list keeps bearsuits out of the root"
        );
    }

    #[test]
    fn handbook_overrides_upsert_by_id_and_append_missing_at_the_end() {
        let mut raw = std::collections::BTreeMap::new();
        let original = br#"{"Categories":[],"Items":[{"Id":"a","ParentId":"p","Price":1},{"Id":"c","ParentId":"p","Price":3}]}"#.to_vec();
        raw.insert(HANDBOOK_KEY.to_string(), original.clone());
        let overrides: indexmap::IndexMap<String, HandbookPriceOverrideWire> =
            serde_json::from_str(
                r#"{"a":{"parentId":"pp","price":5},"b":{"parentId":"bp","price":7.5}}"#,
            )
            .unwrap();

        let replaced = apply_handbook_overrides(&mut raw, &overrides).unwrap();
        assert_eq!(
            replaced,
            Some(original),
            "the raw bytes come back for the caller to restore"
        );

        let merged: serde_json::Value = serde_json::from_slice(&raw[HANDBOOK_KEY]).unwrap();
        let items = merged["Items"].as_array().unwrap();
        // Existing entry updated in place (HandbookHelper.cs:26-49: find by Id, set Price+ParentId).
        assert_eq!(items[0]["Id"], "a");
        assert_eq!(items[0]["Price"], 5);
        assert_eq!(items[0]["ParentId"], "pp");
        // Untouched entry intact.
        assert_eq!(items[1]["Id"], "c");
        assert_eq!(items[1]["Price"], 3);
        // Missing entry appended at the END, in override document order.
        assert_eq!(items.len(), 3);
        assert_eq!(items[2]["Id"], "b");
        assert_eq!(items[2]["Price"], 7.5);
        assert_eq!(items[2]["ParentId"], "bp");
    }

    #[test]
    fn handbook_overrides_with_no_handbook_file_are_a_no_op() {
        let mut raw = std::collections::BTreeMap::new();
        let overrides: indexmap::IndexMap<String, HandbookPriceOverrideWire> =
            serde_json::from_str(r#"{"a":{"parentId":"p","price":1}}"#).unwrap();
        assert_eq!(
            apply_handbook_overrides(&mut raw, &overrides).unwrap(),
            None
        );
        assert!(raw.is_empty());
    }

    #[test]
    fn overrides_merge_into_the_resident_root_but_files_stay_raw() {
        let _guard = crate::db::tests::DB_TEST_LOCK.lock().unwrap();
        crate::db::clear();

        let dir = mini_tree();
        let mut with_overrides = request(&dir, false);
        with_overrides.handbook_price_override =
            Some(serde_json::from_str(r#"{"appended":{"parentId":"p","price":42}}"#).unwrap());
        let response = block_on(load(with_overrides)).expect("load succeeds");
        assert_eq!(response.epoch, Some(1));

        // The resident lift carries the merge — mini_tree's handbook is {"Items":[]}, so this is the
        // append path, the one the shipped tree's absent override exercises on every real boot.
        let db = crate::db::current().expect("a resident DB");
        let items = &db.templates.as_ref().unwrap().handbook.items;
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "appended");

        // The returned files are the raw disk bytes — the merge never leaks to C#
        // (spec non-goal: no change to what spt_db_load returns to the importer).
        let handbook = response
            .files
            .iter()
            .find(|(key, _)| key == HANDBOOK_KEY)
            .expect("handbook ships to C#");
        assert_eq!(handbook.1, fs::read(dir.path().join(HANDBOOK_KEY)).unwrap());
    }
}
