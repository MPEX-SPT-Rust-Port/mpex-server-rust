use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct ManifestEntry {
    #[serde(rename = "Path")]
    path: String,
    #[serde(rename = "Hash")]
    hash: String,
}

pub fn parse_manifest(raw: &[u8]) -> Result<HashMap<String, String>, String> {
    use base64::Engine;

    let text = std::str::from_utf8(raw).map_err(|e| format!("checks.dat is not UTF-8: {e}"))?;
    let json = base64::engine::general_purpose::STANDARD
        .decode(text.trim())
        .map_err(|e| format!("checks.dat is not valid base64: {e}"))?;
    let entries: Vec<ManifestEntry> =
        serde_json::from_slice(&json).map_err(|e| format!("checks.dat JSON is invalid: {e}"))?;
    Ok(entries.into_iter().map(|e| (e.path, e.hash)).collect())
}

fn relative_key(spt_data: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(spt_data).ok()?;
    Some(rel.to_string_lossy().replace('\\', "/"))
}

// The verified universe is every top-level SPT_Data entry the manifest names (configs/,
// database/): within those roots, disk and manifest must match in both directions. Scope is
// derived from the manifest instead of walking all of SPT_Data because the build relocates
// unhashed artifacts into the output SPT_Data (satellite assemblies under dotnet/, admin-panel
// static assets under wwwroot/ — see RelocateSatelliteAssemblies in SPTarkov.Server.csproj),
// and generate() deliberately leaves images/ and checks.dat out of the manifest.
pub fn collect_files(
    spt_data: &Path,
    manifest: &HashMap<String, String>,
) -> Vec<(PathBuf, String)> {
    let roots: HashSet<&str> = manifest
        .keys()
        .map(|key| key.split('/').next().unwrap_or(key))
        .collect();

    let mut files = Vec::new();
    for root in roots {
        for entry in walkdir::WalkDir::new(spt_data.join(root))
            .into_iter()
            .flatten()
        {
            if !entry.file_type().is_file() {
                continue;
            }
            if let Some(key) = relative_key(spt_data, entry.path()) {
                files.push((entry.path().to_path_buf(), key));
            }
        }
    }

    files
}

#[derive(Debug, Serialize)]
pub struct VerifyReport {
    pub ok: bool,
    pub failures: Vec<Failure>,
    pub checked: usize,
}

#[derive(Debug, Serialize)]
pub struct Failure {
    pub path: String,
    pub reason: String,
}

const MAX_CONCURRENT_HASHES: usize = 32;

pub async fn verify(spt_data: PathBuf) -> VerifyReport {
    verify_collecting(spt_data, |_| false).await.0
}

// `verify`, plus the bytes of every file `want` accepts: those are whole-read and hashed from the
// buffer instead of streamed, so a caller that needs the contents anyway (the fused SPT_Data load)
// reads each file once. Everything else — failure semantics, ordering, the reverse pass — is
// identical to a plain verify. Returned bytes are raw: any BOM is still on them.
pub(crate) async fn verify_collecting(
    spt_data: PathBuf,
    want: fn(&str) -> bool,
) -> (VerifyReport, Vec<(String, Vec<u8>)>) {
    use std::sync::Arc;

    let manifest_path = spt_data.join("checks.dat");
    let manifest: HashMap<String, String> = match tokio::fs::read(&manifest_path).await {
        Ok(raw) => match parse_manifest(&raw) {
            Ok(manifest) => manifest,
            Err(e) => return (manifest_failure(e), Vec::new()),
        },
        Err(e) => {
            return (
                manifest_failure(format!("cannot read checks.dat: {e}")),
                Vec::new(),
            );
        }
    };

    // An empty manifest would otherwise verify nothing and pass vacuously.
    if manifest.is_empty() {
        return (manifest_failure("manifest is empty".into()), Vec::new());
    }

    let files = collect_files(&spt_data, &manifest);
    let checked = files.len();

    // Reverse direction: a manifest entry with no walked file means the file was deleted or
    // replaced by something the walk skips (e.g. a symlink) — fail, don't silently pass.
    let missing_from_disk: Vec<Failure> = {
        let walked: HashSet<&str> = files.iter().map(|(_, key)| key.as_str()).collect();
        manifest
            .keys()
            .filter(|key| !walked.contains(key.as_str()))
            .map(|key| Failure {
                path: key.clone(),
                reason: "missing_from_disk".into(),
            })
            .collect()
    };

    let manifest = Arc::new(manifest);
    let semaphore = Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_HASHES));
    let mut tasks = tokio::task::JoinSet::new();

    for (path, key) in files {
        let manifest = Arc::clone(&manifest);
        let semaphore = Arc::clone(&semaphore);
        tasks.spawn(async move {
            let _permit = semaphore.acquire_owned().await.expect("semaphore closed");
            check_one(&manifest, &path, key, want).await
        });
    }

    let mut failures: Vec<Failure> = Vec::new();
    let mut collected: Vec<(String, Vec<u8>)> = Vec::new();
    for (failure, bytes) in tasks.join_all().await {
        failures.extend(failure);
        collected.extend(bytes);
    }
    failures.extend(missing_from_disk);
    failures.sort_by(|a, b| a.path.cmp(&b.path));
    collected.sort_by(|a, b| a.0.cmp(&b.0));

    (
        VerifyReport {
            ok: failures.is_empty(),
            failures,
            checked,
        },
        collected,
    )
}

pub struct GeneratedManifest {
    pub base64: String,
    pub hashed: usize,
}

// The generate counterpart to `verify`, replacing PostBuild.cs: walk all of SPT_Data except
// `images/` and any `checks.dat` (the same exclusions the C# generator applied), hash each file,
// and emit the base64-wrapped JSON manifest `parse_manifest` reads back.
pub async fn generate(spt_data: PathBuf) -> Result<GeneratedManifest, String> {
    use base64::Engine;
    use std::sync::Arc;

    let images = spt_data.join("images");
    let mut files = Vec::new();
    for entry in walkdir::WalkDir::new(&spt_data).into_iter().flatten() {
        if !entry.file_type().is_file()
            || entry.path().starts_with(&images)
            || entry.file_name().eq_ignore_ascii_case("checks.dat")
        {
            continue;
        }
        if let Some(key) = relative_key(&spt_data, entry.path()) {
            files.push((entry.path().to_path_buf(), key));
        }
    }
    if files.is_empty() {
        return Err(format!("no files to hash under {}", spt_data.display()));
    }

    let semaphore = Arc::new(tokio::sync::Semaphore::new(MAX_CONCURRENT_HASHES));
    let mut tasks = tokio::task::JoinSet::new();
    for (path, key) in files {
        let semaphore = Arc::clone(&semaphore);
        tasks.spawn(async move {
            let _permit = semaphore.acquire_owned().await.expect("semaphore closed");
            match xxh3_file(&path).await {
                Ok(hash) => Ok(ManifestEntry { path: key, hash }),
                Err(e) => Err(format!("cannot hash {}: {e}", path.display())),
            }
        });
    }

    let mut entries: Vec<ManifestEntry> = tasks
        .join_all()
        .await
        .into_iter()
        .collect::<Result<_, _>>()?;
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    let hashed = entries.len();

    let json =
        serde_json::to_string(&entries).map_err(|e| format!("cannot serialize manifest: {e}"))?;
    let base64 = base64::engine::general_purpose::STANDARD.encode(json.as_bytes());
    Ok(GeneratedManifest { base64, hashed })
}

// Bytes come back only for a wanted file that also verified: a mismatching or unreadable file
// fails the report, and its contents are of no use to anyone.
async fn check_one(
    manifest: &std::collections::HashMap<String, String>,
    path: &Path,
    key: String,
    want: fn(&str) -> bool,
) -> (Option<Failure>, Option<(String, Vec<u8>)>) {
    let Some(expected) = manifest.get(&key) else {
        return (
            Some(Failure {
                path: key,
                reason: "missing_from_manifest".into(),
            }),
            None,
        );
    };
    let io_error = |e: std::io::Error, key: String| {
        (
            Some(Failure {
                path: key,
                reason: format!("io_error: {e}"),
            }),
            None,
        )
    };
    let (actual, bytes) = if want(&key) {
        match tokio::fs::read(path).await {
            Ok(bytes) => (xxh3_bytes(&bytes), Some(bytes)),
            Err(e) => return io_error(e, key),
        }
    } else {
        match xxh3_file(path).await {
            Ok(hash) => (hash, None),
            Err(e) => return io_error(e, key),
        }
    };
    if &actual != expected {
        return (
            Some(Failure {
                path: key,
                reason: "hash_mismatch".into(),
            }),
            None,
        );
    }
    (None, bytes.map(|bytes| (key, bytes)))
}

pub(crate) fn xxh3_bytes(bytes: &[u8]) -> String {
    format!("{:032X}", xxhash_rust::xxh3::xxh3_128(bytes))
}

async fn xxh3_file(path: &Path) -> std::io::Result<String> {
    use tokio::io::AsyncReadExt;
    use xxhash_rust::xxh3::Xxh3;

    let mut file = tokio::fs::File::open(path).await?;
    let mut hasher = Xxh3::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(format!("{:032X}", hasher.digest128()))
}

fn manifest_failure(reason: String) -> VerifyReport {
    VerifyReport {
        ok: false,
        failures: vec![Failure {
            path: "checks.dat".into(),
            reason: format!("manifest_unreadable: {reason}"),
        }],
        checked: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    fn encode_manifest(json: &str) -> Vec<u8> {
        base64::engine::general_purpose::STANDARD
            .encode(json.as_bytes())
            .into_bytes()
    }

    #[test]
    fn parses_valid_manifest() {
        let raw = encode_manifest(
            r#"[{"Path":"database/templates/items.json","Hash":"06B05AB6733A618578AF5F94892F3950"}]"#,
        );
        let map = parse_manifest(&raw).unwrap();
        assert_eq!(
            map.get("database/templates/items.json").map(String::as_str),
            Some("06B05AB6733A618578AF5F94892F3950")
        );
    }

    #[test]
    fn rejects_invalid_base64() {
        assert!(parse_manifest(b"!!! not base64 !!!").is_err());
    }

    #[test]
    fn rejects_invalid_json() {
        let raw = encode_manifest("{ definitely not an array");
        assert!(parse_manifest(&raw).is_err());
    }

    #[test]
    fn tolerates_surrounding_whitespace() {
        let mut raw = b"\n  ".to_vec();
        raw.extend(encode_manifest(r#"[{"Path":"a.json","Hash":"AA"}]"#));
        raw.extend(b"  \n");
        assert!(parse_manifest(&raw).is_ok());
    }

    use std::fs;
    use tempfile::TempDir;

    fn touch(root: &Path, rel: &str) {
        let path = root.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, b"{}").unwrap();
    }

    fn manifest_of(entries: &[&str]) -> HashMap<String, String> {
        entries
            .iter()
            .map(|key| (key.to_string(), String::new()))
            .collect()
    }

    fn keys(spt_data: &Path, manifest: &HashMap<String, String>) -> Vec<String> {
        let mut keys: Vec<String> = collect_files(spt_data, manifest)
            .into_iter()
            .map(|(_, k)| k)
            .collect();
        keys.sort();
        keys
    }

    #[test]
    fn collects_every_file_under_manifest_named_roots() {
        // Non-json files and the importer-ignored locale dirs are in checks.dat, so they
        // must all be walked.
        let dir = TempDir::new().unwrap();
        touch(dir.path(), "database/globals.json");
        touch(dir.path(), "database/readme.txt");
        touch(dir.path(), "database/locales/server/en.json");
        touch(dir.path(), "configs/core.json");
        let manifest = manifest_of(&["database/globals.json", "configs/core.json"]);
        assert_eq!(
            keys(dir.path(), &manifest),
            vec![
                "configs/core.json".to_string(),
                "database/globals.json".to_string(),
                "database/locales/server/en.json".to_string(),
                "database/readme.txt".to_string(),
            ]
        );
    }

    #[test]
    fn ignores_top_level_entries_the_manifest_never_names() {
        // The build relocates unhashed artifacts (dotnet/, wwwroot/) into the output SPT_Data,
        // and the generator skips images/ and checks.dat — none of these may fail verification.
        let dir = TempDir::new().unwrap();
        touch(dir.path(), "dotnet/de/Spectre.Console.Cli.resources.dll");
        touch(dir.path(), "wwwroot/index.html");
        touch(dir.path(), "images/icon.png");
        touch(dir.path(), "database/kept.json");
        fs::write(dir.path().join("checks.dat"), b"x").unwrap();
        let manifest = manifest_of(&["database/kept.json"]);
        assert_eq!(
            keys(dir.path(), &manifest),
            vec!["database/kept.json".to_string()]
        );
    }

    fn write_manifest(spt_data: &Path, entries: &[(&str, &str)]) {
        use base64::Engine;
        let items: Vec<serde_json::Value> = entries
            .iter()
            .map(|(p, h)| serde_json::json!({ "Path": p, "Hash": h }))
            .collect();
        let json = serde_json::to_string(&items).unwrap();
        let b64 = base64::engine::general_purpose::STANDARD.encode(json.as_bytes());
        fs::write(spt_data.join("checks.dat"), b64).unwrap();
    }

    fn xxh3_hex(data: &[u8]) -> String {
        format!("{:032X}", xxhash_rust::xxh3::xxh3_128(data))
    }

    #[test]
    fn xxh3_known_answer() {
        // Cross-checked against Python xxhash and System.IO.Hashing.XxHash128 (canonical big-endian).
        assert_eq!(xxh3_hex(b"abc"), "06B05AB6733A618578AF5F94892F3950");
    }

    #[tokio::test]
    async fn clean_tree_verifies_ok() {
        let dir = TempDir::new().unwrap();
        touch(dir.path(), "database/globals.json");
        touch(dir.path(), "database/templates/items.json");
        write_manifest(
            dir.path(),
            &[
                ("database/globals.json", xxh3_hex(b"{}").as_str()),
                ("database/templates/items.json", xxh3_hex(b"{}").as_str()),
            ],
        );
        let report = verify(dir.path().to_path_buf()).await;
        assert!(report.ok);
        assert_eq!(report.checked, 2);
        assert!(report.failures.is_empty());
    }

    #[tokio::test]
    async fn verify_collecting_returns_wanted_bytes_and_streams_the_rest() {
        let dir = TempDir::new().unwrap();
        touch(dir.path(), "database/a.json");
        touch(dir.path(), "configs/b.json");
        write_manifest(
            dir.path(),
            &[
                ("database/a.json", xxh3_hex(b"{}").as_str()),
                ("configs/b.json", xxh3_hex(b"{}").as_str()),
            ],
        );

        let (report, collected) =
            verify_collecting(dir.path().to_path_buf(), |k| k == "database/a.json").await;
        assert!(report.ok);
        assert_eq!(report.checked, 2);
        assert_eq!(
            collected,
            vec![("database/a.json".to_string(), b"{}".to_vec())]
        );

        let (report, collected) = verify_collecting(dir.path().to_path_buf(), |_| false).await;
        assert!(report.ok);
        assert!(collected.is_empty());
    }

    #[tokio::test]
    async fn tampered_file_fails_with_hash_mismatch() {
        let dir = TempDir::new().unwrap();
        touch(dir.path(), "database/globals.json");
        write_manifest(
            dir.path(),
            &[("database/globals.json", xxh3_hex(b"{}").as_str())],
        );
        fs::write(dir.path().join("database/globals.json"), b"{tampered}").unwrap();
        let report = verify(dir.path().to_path_buf()).await;
        assert!(!report.ok);
        assert_eq!(report.failures[0].path, "database/globals.json");
        assert_eq!(report.failures[0].reason, "hash_mismatch");
    }

    #[tokio::test]
    async fn file_missing_from_manifest_fails() {
        let dir = TempDir::new().unwrap();
        touch(dir.path(), "database/globals.json");
        touch(dir.path(), "database/extra.json");
        write_manifest(
            dir.path(),
            &[("database/globals.json", xxh3_hex(b"{}").as_str())],
        );
        let report = verify(dir.path().to_path_buf()).await;
        assert!(!report.ok);
        assert_eq!(report.failures[0].path, "database/extra.json");
        assert_eq!(report.failures[0].reason, "missing_from_manifest");
    }

    #[tokio::test]
    async fn deleted_file_fails_with_missing_from_disk() {
        let dir = TempDir::new().unwrap();
        touch(dir.path(), "database/globals.json");
        write_manifest(
            dir.path(),
            &[
                ("database/globals.json", xxh3_hex(b"{}").as_str()),
                ("database/deleted.json", xxh3_hex(b"{}").as_str()),
            ],
        );
        let report = verify(dir.path().to_path_buf()).await;
        assert!(!report.ok);
        assert_eq!(report.failures[0].path, "database/deleted.json");
        assert_eq!(report.failures[0].reason, "missing_from_disk");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn symlink_replaced_file_fails_instead_of_passing() {
        let dir = TempDir::new().unwrap();
        touch(dir.path(), "database/globals.json");
        write_manifest(
            dir.path(),
            &[("database/globals.json", xxh3_hex(b"{}").as_str())],
        );
        fs::write(dir.path().join("evil.tmp"), b"{}").unwrap();
        fs::remove_file(dir.path().join("database/globals.json")).unwrap();
        std::os::unix::fs::symlink(
            dir.path().join("evil.tmp"),
            dir.path().join("database/globals.json"),
        )
        .unwrap();

        let report = verify(dir.path().to_path_buf()).await;
        assert!(!report.ok);
        assert_eq!(report.failures[0].path, "database/globals.json");
        assert_eq!(report.failures[0].reason, "missing_from_disk");
    }

    #[tokio::test]
    async fn missing_checks_dat_fails() {
        let dir = TempDir::new().unwrap();
        touch(dir.path(), "database/globals.json");
        let report = verify(dir.path().to_path_buf()).await;
        assert!(!report.ok);
        assert_eq!(report.failures[0].path, "checks.dat");
        assert!(report.failures[0].reason.starts_with("manifest_unreadable"));
    }

    #[tokio::test]
    async fn empty_manifest_fails_instead_of_passing_vacuously() {
        let dir = TempDir::new().unwrap();
        write_manifest(dir.path(), &[]);
        let report = verify(dir.path().to_path_buf()).await;
        assert!(!report.ok);
        assert_eq!(report.failures[0].path, "checks.dat");
        assert_eq!(
            report.failures[0].reason,
            "manifest_unreadable: manifest is empty"
        );
    }

    #[tokio::test]
    async fn generated_manifest_round_trips_through_verify() {
        let dir = TempDir::new().unwrap();
        touch(dir.path(), "database/globals.json");
        touch(dir.path(), "configs/core.json");
        touch(dir.path(), "images/icon.png");
        fs::write(dir.path().join("checks.dat"), b"stale").unwrap();

        let generated = generate(dir.path().to_path_buf()).await.unwrap();
        assert_eq!(generated.hashed, 2);
        fs::write(dir.path().join("checks.dat"), &generated.base64).unwrap();

        let manifest = parse_manifest(generated.base64.as_bytes()).unwrap();
        assert_eq!(
            manifest.get("database/globals.json").map(String::as_str),
            Some(xxh3_hex(b"{}").as_str())
        );
        assert!(!manifest.keys().any(|k| k.starts_with("images/")));
        assert!(!manifest.contains_key("checks.dat"));

        let report = verify(dir.path().to_path_buf()).await;
        assert!(
            report.ok,
            "failures: {:?}",
            report
                .failures
                .iter()
                .map(|f| (&f.path, &f.reason))
                .collect::<Vec<_>>()
        );
        assert_eq!(report.checked, 2);
    }

    #[tokio::test]
    async fn generating_an_empty_tree_fails_instead_of_writing_an_empty_manifest() {
        // verify rejects an empty manifest, so generating one must fail loudly at build time too.
        let dir = TempDir::new().unwrap();
        assert!(generate(dir.path().to_path_buf()).await.is_err());
    }

    #[tokio::test]
    async fn multi_chunk_file_hashes_identically_to_one_shot() {
        // xxh3_file streams in 64 KiB chunks; the generator and test helpers hash one-shot.
        let dir = TempDir::new().unwrap();
        let big: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
        let path = dir.path().join("database/big.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, &big).unwrap();

        assert_eq!(xxh3_file(&path).await.unwrap(), xxh3_hex(&big));
    }
}
