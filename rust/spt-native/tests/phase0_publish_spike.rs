//! Phase 0 publish spike — parse-cost and RSS ceiling for the resident DB
//! (docs/superpowers/specs/2026-08-17-rust-state-ownership-design.md § Phase 0).
//!
//! The first test parses the whole envelope into `serde_json::Value` — a deliberately
//! conservative bound: `Value` over-counts RSS versus the typed models Phase 1 will hold.
//! The second parses only the locales root into its real final representation (string maps)
//! and gives the corrected bound if the `Value` bound trips a gate.
//!
//! Run by hand, after `DbPublishSpikeTests` has written the payload:
//!   cargo test --release --test phase0_publish_spike -- --ignored --nocapture

use std::time::Instant;

use indexmap::IndexMap;

fn payload_path() -> std::path::PathBuf {
    std::env::var_os("SPT_PHASE0_PAYLOAD")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("spt-phase0-publish.json"))
}

/// VmRSS in KiB from /proc/self/status. Linux-only, like the shipped library.
fn vm_rss_kib() -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").expect("reading /proc/self/status");
    status
        .lines()
        .find(|line| line.starts_with("VmRSS:"))
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse().ok())
        .expect("VmRSS line in /proc/self/status")
}

#[test]
#[ignore = "phase 0 spike — needs the payload file from DbPublishSpikeTests"]
fn parse_full_publish_envelope_value_bound() {
    let path = payload_path();
    let bytes = std::fs::read(&path)
        .unwrap_or_else(|e| panic!("run DbPublishSpikeTests first — {}: {e}", path.display()));

    let rss_before = vm_rss_kib();
    let start = Instant::now();
    let value: serde_json::Value = serde_json::from_slice(&bytes).expect("parse envelope");
    let parse = start.elapsed();
    let rss_after = vm_rss_kib();

    assert!(value.get("roots").is_some(), "envelope has a roots object");
    println!(
        "payload            {:>8.1} MiB",
        bytes.len() as f64 / 1048576.0
    );
    println!("parse (Value bound) {parse:?}");
    println!(
        "RSS delta (bound)  {:>8.1} MiB",
        (rss_after.saturating_sub(rss_before)) as f64 / 1024.0
    );
    drop(value);
}

/// Only the locales root, into its real final representation. serde skips the sibling roots
/// (unknown fields), so this isolates the typed cost of the biggest string-heavy root.
#[derive(serde::Deserialize)]
struct LocalesEnvelope {
    roots: LocalesRoots,
}

#[derive(serde::Deserialize)]
struct LocalesRoots {
    locales: LocalesRoot,
}

#[derive(serde::Deserialize)]
struct LocalesRoot {
    global: IndexMap<String, IndexMap<String, String>>,
}

#[test]
#[ignore = "phase 0 spike refinement — gives the corrected locales bound (run each test in its own process for valid RSS)"]
fn parse_locales_root_typed() {
    let path = payload_path();
    let bytes = std::fs::read(&path)
        .unwrap_or_else(|e| panic!("run DbPublishSpikeTests first — {}: {e}", path.display()));

    let rss_before = vm_rss_kib();
    let start = Instant::now();
    let envelope: LocalesEnvelope = serde_json::from_slice(&bytes).expect("parse locales root");
    let parse = start.elapsed();
    let rss_after = vm_rss_kib();

    let languages = envelope.roots.locales.global.len();
    assert!(languages > 0, "locales root has languages");
    println!("locales typed parse {parse:?} ({languages} languages)");
    println!(
        "locales typed RSS  {:>8.1} MiB",
        (rss_after.saturating_sub(rss_before)) as f64 / 1024.0
    );
}
