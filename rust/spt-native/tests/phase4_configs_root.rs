//! Phase 4 configs root — the C#-projected configs parse into the Rust wire models.
//!
//! The two serializers this pair gates never meet anywhere else: `ConfigLoader` reads
//! SPT_Data/configs with its own bespoke `JsonSerializerOptions`, while
//! `DbPayloadProjection.BuildPublishEnvelope` writes what it read with the server's shared
//! `JsonUtil.JsonSerializerOptionsNoIndent`. A round trip through the pair is the only place a
//! divergence between the two shows up before a live publish does.
//!
//! What it proves today is that every kind arrives and the envelope parses; `ConfigsRoot` still
//! holds `extra: IndexMap<String, Value>` only, so no config *body* is type-checked yet. Tasks
//! 5-10 lift typed stems out of `extra` and add their kind to `LIFTED_KINDS` below — that is
//! where the parse grows teeth.
//!
//! Run by hand, after `DbPublishFixtureTests.WriteConfigsRootFixture` has written the dump:
//!   cargo test -p spt-native --test phase4_configs_root -- --ignored --nocapture

use std::collections::BTreeSet;
use std::path::PathBuf;

use spt_native::db::models::PublishRequest;

/// Every `ConfigTypes.GetValue()` arm, copied literally from
/// `Libraries/SPTarkov.Server.Core/Models/Enums/ConfigTypes.cs:11-41`. One file per kind ships in
/// SPT_Data/configs, so a publish carries all 28.
const EXPECTED_KINDS: [&str; 28] = [
    "spt-airdrop",
    "spt-backup",
    "spt-bot",
    "spt-btrdelivery",
    "spt-pmc",
    "spt-core",
    "spt-health",
    "spt-hideout",
    "spt-http",
    "spt-inraid",
    "spt-insurance",
    "spt-inventory",
    "spt-item",
    "spt-locale",
    "spt-location",
    "spt-loot",
    "spt-match",
    "spt-playerscav",
    "spt-pmcchatresponse",
    "spt-quest",
    "spt-ragfair",
    "spt-repair",
    "spt-scavcase",
    "spt-trader",
    "spt-weather",
    "spt-seasonalevents",
    "spt-lostondeath",
    "spt-gifts",
];

#[test]
#[ignore = "phase 4 configs root — needs the dump from DbPublishFixtureTests.WriteConfigsRootFixture"]
fn projected_configs_parse_with_every_kind_present() {
    let path = std::env::var_os("SPT_PHASE4_CONFIGS")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("spt-phase4-configs.json"));

    let bytes = std::fs::read(&path).unwrap_or_else(|error| {
        panic!(
            "run DbPublishFixtureTests.WriteConfigsRootFixture first — {}: {error}",
            path.display()
        )
    });

    let request: PublishRequest =
        serde_json::from_slice(&bytes).expect("the projected envelope parses into the typed roots");
    let configs = request.roots.configs.expect("envelope has a configs root");

    // A kind whose typed stem has been lifted out of the flatten map: it is no longer an `extra`
    // key, so it is named here instead. Empty until Task 5 lifts the first stem.
    const LIFTED_KINDS: [&str; 0] = [];

    let mut present: BTreeSet<&str> = configs.extra.keys().map(String::as_str).collect();
    present.extend(LIFTED_KINDS);

    assert_eq!(
        present,
        BTreeSet::from(EXPECTED_KINDS),
        "the configs root must carry exactly the ConfigTypes kinds"
    );
    println!("configs root: {} kinds, all accounted for ✓", present.len());
}
