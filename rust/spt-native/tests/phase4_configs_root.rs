//! Phase 4 configs root — the C#-projected configs parse into the Rust wire models.
//!
//! The two serializers this pair gates never meet anywhere else: `ConfigLoader` reads
//! SPT_Data/configs with its own bespoke `JsonSerializerOptions`, while
//! `DbPayloadProjection.BuildPublishEnvelope` writes what it read with the server's shared
//! `JsonUtil.JsonSerializerOptionsNoIndent`. A round trip through the pair is the only place a
//! divergence between the two shows up before a live publish does.
//!
//! What it proves: every kind arrives, the envelope parses, and every stem lifted out of `extra`
//! so far parses into its typed shape rather than raw `Value`. Tasks 7-10 lift the rest; each adds
//! its kind to `LIFTED_KINDS` below plus a stem-is-`Some` assertion, or the union assertion fails
//! loudly.
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
    // key, so it is named here instead. Task 5 lifted the scav case family's two, Task 6 the
    // ragfair family's two, Task 7 the repeatable-quest family's one, Task 9 the location-loot
    // family's two, Task 10 the bot family's three.
    const LIFTED_KINDS: [&str; 10] = [
        "spt-item",
        "spt-scavcase",
        "spt-ragfair",
        "spt-inventory",
        "spt-quest",
        "spt-location",
        "spt-seasonalevents",
        "spt-bot",
        "spt-pmc",
        "spt-repair",
    ];

    // The lift's own half of the fidelity claim: the projected bodies parse into the typed stems,
    // not just into `Value`. A stem that failed to parse would have failed the whole envelope
    // above, so reaching here with a `None` means the wire name drifted off the record's `Kind`.
    assert!(
        configs.item.is_some(),
        "the spt-item stem did not bind — check ItemConfig.Kind against the rename"
    );
    assert!(
        configs.scavcase.is_some(),
        "the spt-scavcase stem did not bind — check ScavCaseConfig.Kind against the rename"
    );
    assert!(
        configs.ragfair.is_some(),
        "the spt-ragfair stem did not bind — check RagfairConfig.Kind against the rename"
    );
    assert!(
        configs.inventory.is_some(),
        "the spt-inventory stem did not bind — check InventoryConfig.Kind against the rename"
    );
    assert!(
        configs.quest.is_some(),
        "the spt-quest stem did not bind — check QuestConfig.Kind against the rename"
    );
    assert!(
        configs.location.is_some(),
        "the spt-location stem did not bind — check LocationConfig.Kind against the rename"
    );
    assert!(
        configs.seasonalevents.is_some(),
        "the spt-seasonalevents stem did not bind — check SeasonalEventConfig.Kind against the rename"
    );
    assert!(
        configs.bot.is_some(),
        "the spt-bot stem did not bind — check BotConfig.Kind against the rename"
    );
    assert!(
        configs.pmc.is_some(),
        "the spt-pmc stem did not bind — check PmcConfig.Kind against the rename"
    );
    assert!(
        configs.repair.is_some(),
        "the spt-repair stem did not bind — check RepairConfig.Kind against the rename"
    );

    let mut present: BTreeSet<&str> = configs.extra.keys().map(String::as_str).collect();
    present.extend(LIFTED_KINDS);

    assert_eq!(
        present,
        BTreeSet::from(EXPECTED_KINDS),
        "the configs root must carry exactly the ConfigTypes kinds"
    );
    println!("configs root: {} kinds, all accounted for ✓", present.len());
}
