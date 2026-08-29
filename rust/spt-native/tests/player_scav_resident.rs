//! Resident-arm integration for the player scav export: publish the same minimal
//! templates+traders+globals DB the bot flip publishes — plus a 1x1 item the karma extra-loot pass
//! can place — then prove an `{epoch}` send generates identically to the same data sent as
//! `viewsOverride`, that every karma wire member reaches the generation, and that a wrong epoch is
//! a stale error, not a wrong answer.
//!
//! Its own process (integration tests build their own binary), so the process-global store races
//! with nothing; the whole protocol lives in one `#[test]` fn, sequential, to keep it that way.
//!
//! The fixture is `flip6_bots_resident.rs`'s, copied rather than shared: there is no `tests/common/`
//! and each integration file is its own binary. It has drifted where the player scav differs: the
//! keycard tpl the extra-loot pass places (`raw_items`, `view_items` and `handbookPrices`, last key
//! in all three so both arms keep one key order), the karma block itself, a single non-PMC template
//! instead of flip6's two level-band variants, and no level/exp assertions — the single-bot path
//! keeps its levelling C#-side.

// The two big `json!` fixture maps out-recurse the default limit of 128.
#![recursion_limit = "256"]

use serde_json::{Value, json};
use spt_native::ffi::{
    STATUS_OK, STATUS_STALE_EPOCH, spt_buf_free, spt_db_publish, spt_generate_player_scav,
};
use spt_native::loot::item_helper::{AMMO, MAGAZINE, MOD, WEAPON};

// Deliberately non-hex so `strip_mongo_ids` leaves them visible — a draw that diverged between
// the two arms would then fail the byte comparison.
const HEADWEAR_TPL: &str = "headwear_cap";
const EARPIECE_TPL: &str = "earpiece_comtac";
const FACE_COVER_TPL: &str = "facecover_shemagh";
const ARMOR_TPL: &str = "armor_paca";
const VEST_ARMORED_TPL: &str = "vest_armored";
const VEST_PLAIN_TPL: &str = "vest_plain";
const PLATE_TPL: &str = "plate_front";
const BACKPACK_TPL: &str = "backpack_daypack";
const POCKETS_TPL: &str = "pockets_default";
const SECURE_TPL: &str = "secure_alpha";
const ARMBAND_TPL: &str = "armband_yellow";
const RIFLE_TPL: &str = "rifle_akm";
const MAG_TPL: &str = "mag_akm";
const AMMO_TPL: &str = "ammo_ps";
const CALIBER: &str = "Caliber762x39";

/// In a rifle slot filter but in no pool and no items table: it keeps `mod_scope` in the slot
/// pool, so the rifle's pool holds more than one entry and the mod draw has something to walk.
const SCOPE_GHOST: &str = "scope_ghost";

// The randomised-slot fixture (`mod_mount` on the rifle) — here it is the *karma* fixture: the
// template gives the slot a 100% chance and `modModifiers` takes it back to 0, so the mount and its
// two children are the visible half of `AdjustWeaponModWeights`.
const MOUNT_TPL: &str = "mount_rail";
const LIGHT_TPL: &str = "light_torch";
const GRIP_STUBBY_TPL: &str = "grip_stubby";
const GRIP_LONG_TPL: &str = "grip_long";

/// The 1x1 item `lootItemsToAddChancePercent` places into a worn container — nothing else in the
/// fixture can draw it, so its presence in the response is the extra-loot pass and nothing else.
const EXTRA_KEYCARD_TPL: &str = "extra_keycard";

/// The seed. The player scav takes no level draw (the single-bot path keeps C#-side levelling), so
/// this seeds the equipment/mod/loot stream only.
const SEED_PSCAV: u64 = 1;

type Export = unsafe extern "C" fn(*const u8, usize, *mut *mut u8, *mut usize) -> i32;

/// Calls an export and takes ownership of whatever it wrote — a result, or an error message.
fn call(export: Export, request: &[u8]) -> (i32, Vec<u8>) {
    let mut out_ptr: *mut u8 = std::ptr::null_mut();
    let mut out_len: usize = 0;
    let status = unsafe { export(request.as_ptr(), request.len(), &mut out_ptr, &mut out_len) };
    if out_ptr.is_null() {
        return (status, Vec::new());
    }
    let out = unsafe { std::slice::from_raw_parts(out_ptr, out_len) }.to_vec();
    unsafe { spt_buf_free(out_ptr, out_len) };

    (status, out)
}

/// Replaces MongoIds (24 hex chars) with positional placeholders — item ids are minted from the
/// process-wide MongoId counter, not the seeded RNG, so their raw values legitimately differ
/// between two seeded runs. Each *distinct* id becomes `<id-N>` for its first-appearance order in
/// the scanned string, so a `parentId` still names the item it points at: the parent→child linkage
/// stays visible to the comparison instead of collapsing to one indistinguishable placeholder.
fn strip_mongo_ids(json: &str) -> String {
    let mut out = String::with_capacity(json.len());
    let mut seen: Vec<String> = Vec::new();
    let mut run = String::new();
    let mut flush = |out: &mut String, run: &str| {
        if run.len() != 24 {
            out.push_str(run);
            return;
        }
        let n = seen.iter().position(|id| id == run).unwrap_or_else(|| {
            seen.push(run.to_owned());
            seen.len() - 1
        });
        out.push_str(&format!("<id-{n}>"));
    };
    for c in json.chars() {
        if c.is_ascii_hexdigit() {
            run.push(c);
            continue;
        }
        flush(&mut out, &run);
        run.clear();
        out.push(c);
    }
    flush(&mut out, &run);

    out
}

/// The two presets' item lists, shared verbatim by the publish's `_items` and the override's
/// `itemPresets` view so both arms clone identical bytes.
fn preset_p1_items() -> Value {
    json!([
        {"_id": "root_p1", "_tpl": RIFLE_TPL},
        {"_id": "mod_p1", "_tpl": MAG_TPL, "parentId": "root_p1", "slotId": "mod_magazine"},
    ])
}

fn preset_p2_items() -> Value {
    json!([{"_id": "root_p2", "_tpl": RIFLE_TPL}])
}

/// The raw `templates.items` the publish carries.
fn raw_items() -> Value {
    json!({
        WEAPON: {"_name": "weapon", "_type": "Node", "_parent": "", "_props": {}},
        MAGAZINE: {"_name": "magazine", "_type": "Node", "_parent": "", "_props": {}},
        AMMO: {"_name": "ammo", "_type": "Node", "_parent": "", "_props": {}},
        MOD: {"_name": "mod", "_type": "Node", "_parent": "", "_props": {}},
        HEADWEAR_TPL: {"_name": "cap", "_type": "Item",
            "_props": {"Width": 1, "Height": 1}},
        EARPIECE_TPL: {"_name": "comtac", "_type": "Item",
            "_props": {"Width": 1, "Height": 1}},
        FACE_COVER_TPL: {"_name": "shemagh", "_type": "Item",
            "_props": {"Width": 1, "Height": 1}},
        ARMOR_TPL: {"_name": "paca", "_type": "Item",
            "_props": {"Width": 2, "Height": 2}},
        VEST_ARMORED_TPL: {"_name": "armored rig", "_type": "Item", "_props": {
            "Width": 2, "Height": 2,
            "Slots": [{"_name": "front_plate",
                       "_props": {"filters": [{"Filter": [PLATE_TPL]}]}}],
            "Grids": [{"_name": "main", "_props": {"cellsH": 2, "cellsV": 2}}]}},
        VEST_PLAIN_TPL: {"_name": "rig", "_type": "Item", "_props": {
            "Width": 2, "Height": 2,
            "Grids": [{"_name": "main", "_props": {"cellsH": 3, "cellsV": 2}}]}},
        PLATE_TPL: {"_name": "front plate", "_type": "Item",
            "_props": {"Width": 1, "Height": 1, "armorClass": 4}},
        BACKPACK_TPL: {"_name": "daypack", "_type": "Item", "_props": {
            "Width": 3, "Height": 3,
            "Grids": [{"_name": "main", "_props": {"cellsH": 4, "cellsV": 4}}]}},
        POCKETS_TPL: {"_name": "pockets", "_type": "Item", "_props": {
            "Grids": [{"_name": "main", "_props": {"cellsH": 4, "cellsV": 1}}]}},
        SECURE_TPL: {"_name": "alpha", "_type": "Item", "_props": {
            "Grids": [{"_name": "main", "_props": {"cellsH": 2, "cellsV": 2}}]}},
        ARMBAND_TPL: {"_name": "armband", "_type": "Item",
            "_props": {"Width": 1, "Height": 1}},
        RIFLE_TPL: {"_name": "akm", "_type": "Item", "_parent": WEAPON, "_props": {
            "Width": 3, "Height": 1, "weapClass": "assaultRifle", "MaxDurability": 100.0,
            "Caliber": CALIBER, "defAmmo": AMMO_TPL, "defMagType": MAG_TPL,
            "ReloadMode": "ExternalMagazine", "isChamberLoad": false,
            "Chambers": [{"_name": "patron_in_weapon",
                          "_props": {"filters": [{"Filter": [AMMO_TPL]}]}}],
            "Slots": [
                {"_name": "mod_stock", "_props": {"filters": [{"Filter": []}]}},
                {"_name": "mod_magazine", "_required": true,
                 "_props": {"filters": [{"Filter": [MAG_TPL]}]}},
                {"_name": "mod_scope", "_props": {"filters": [{"Filter": [SCOPE_GHOST]}]}},
                {"_name": "mod_mount", "_props": {"filters": [{"Filter": [MOUNT_TPL]}]}},
            ]}},
        MAG_TPL: {"_name": "akm mag", "_type": "Item", "_parent": MAGAZINE, "_props": {
            "Width": 1, "Height": 1, "ReloadMagType": "ExternalMagazine",
            "Cartridges": [{"_name": "cartridges", "_max_count": 30,
                            "_props": {"filters": [{"Filter": [AMMO_TPL]}]}}]}},
        AMMO_TPL: {"_name": "PS", "_type": "Item", "_parent": AMMO, "_props": {
            "Width": 1, "Height": 1, "Caliber": CALIBER,
            "StackMaxSize": 60, "StackMinRandom": 30, "StackMaxRandom": 30}},
        MOUNT_TPL: {"_name": "rail mount", "_type": "Item", "_parent": MOD, "_props": {
            "Width": 1, "Height": 1,
            "Slots": [
                {"_name": "mod_flashlight", "_props": {"filters": [{"Filter": [LIGHT_TPL]}]}},
                {"_name": "mod_foregrip",
                 "_props": {"filters": [{"Filter": [GRIP_STUBBY_TPL, GRIP_LONG_TPL]}]}},
            ]}},
        LIGHT_TPL: {"_name": "torch", "_type": "Item", "_parent": MOD,
            "_props": {"Width": 1, "Height": 1}},
        GRIP_STUBBY_TPL: {"_name": "stubby grip", "_type": "Item", "_parent": MOD,
            "_props": {"Width": 1, "Height": 1}},
        GRIP_LONG_TPL: {"_name": "long grip", "_type": "Item", "_parent": MOD,
            "_props": {"Width": 1, "Height": 1}},
        EXTRA_KEYCARD_TPL: {"_name": "keycard", "_type": "Item",
            "_props": {"Width": 1, "Height": 1}},
    })
}

/// [`raw_items`] as the resident derive projects it (`ragfair::views::build_items_view`) — the
/// override arm's `items`, mirrored key for key in table order.
fn view_items() -> Value {
    json!({
        WEAPON: {"name": "weapon", "type": "Node"},
        MAGAZINE: {"name": "magazine", "type": "Node"},
        AMMO: {"name": "ammo", "type": "Node"},
        MOD: {"name": "mod", "type": "Node"},
        HEADWEAR_TPL: {"name": "cap", "type": "Item", "width": 1, "height": 1},
        EARPIECE_TPL: {"name": "comtac", "type": "Item", "width": 1, "height": 1},
        FACE_COVER_TPL: {"name": "shemagh", "type": "Item", "width": 1, "height": 1},
        ARMOR_TPL: {"name": "paca", "type": "Item", "width": 2, "height": 2},
        VEST_ARMORED_TPL: {"name": "armored rig", "type": "Item", "width": 2, "height": 2,
            "gridCellsH": 2, "gridCellsV": 2,
            "slots": [{"name": "front_plate", "filter": [PLATE_TPL]}],
            "grids": [{"name": "main", "cellsH": 2, "cellsV": 2}]},
        VEST_PLAIN_TPL: {"name": "rig", "type": "Item", "width": 2, "height": 2,
            "gridCellsH": 3, "gridCellsV": 2,
            "grids": [{"name": "main", "cellsH": 3, "cellsV": 2}]},
        PLATE_TPL: {"name": "front plate", "type": "Item", "width": 1, "height": 1,
            "armorClass": 4},
        BACKPACK_TPL: {"name": "daypack", "type": "Item", "width": 3, "height": 3,
            "gridCellsH": 4, "gridCellsV": 4,
            "grids": [{"name": "main", "cellsH": 4, "cellsV": 4}]},
        POCKETS_TPL: {"name": "pockets", "type": "Item", "gridCellsH": 4, "gridCellsV": 1,
            "grids": [{"name": "main", "cellsH": 4, "cellsV": 1}]},
        SECURE_TPL: {"name": "alpha", "type": "Item", "gridCellsH": 2, "gridCellsV": 2,
            "grids": [{"name": "main", "cellsH": 2, "cellsV": 2}]},
        ARMBAND_TPL: {"name": "armband", "type": "Item", "width": 1, "height": 1},
        RIFLE_TPL: {"parent": WEAPON, "name": "akm", "type": "Item", "width": 3, "height": 1,
            "weapClass": "assaultRifle", "maxDurability": 100.0,
            "caliber": CALIBER, "defAmmo": AMMO_TPL, "defMagType": MAG_TPL,
            "reloadMode": "ExternalMagazine", "isChamberLoad": false,
            "chambersFirstFilter": [AMMO_TPL],
            "chambers": [{"name": "patron_in_weapon", "filter": [AMMO_TPL]}],
            "slots": [
                {"name": "mod_stock", "filter": []},
                {"name": "mod_magazine", "required": true, "filter": [MAG_TPL]},
                {"name": "mod_scope", "filter": [SCOPE_GHOST]},
                {"name": "mod_mount", "filter": [MOUNT_TPL]},
            ]},
        MAG_TPL: {"parent": MAGAZINE, "name": "akm mag", "type": "Item",
            "width": 1, "height": 1, "reloadMagType": "ExternalMagazine",
            "cartridgesMaxCount": 30.0, "cartridgesFirstFilter": [AMMO_TPL],
            "cartridges": [{"name": "cartridges", "filter": [AMMO_TPL]}]},
        AMMO_TPL: {"parent": AMMO, "name": "PS", "type": "Item", "width": 1, "height": 1,
            "caliber": CALIBER,
            "stackMaxSize": 60, "stackMinRandom": 30, "stackMaxRandom": 30},
        MOUNT_TPL: {"parent": MOD, "name": "rail mount", "type": "Item",
            "width": 1, "height": 1,
            "slots": [
                {"name": "mod_flashlight", "filter": [LIGHT_TPL]},
                {"name": "mod_foregrip", "filter": [GRIP_STUBBY_TPL, GRIP_LONG_TPL]},
            ]},
        LIGHT_TPL: {"parent": MOD, "name": "torch", "type": "Item", "width": 1, "height": 1},
        GRIP_STUBBY_TPL: {"parent": MOD, "name": "stubby grip", "type": "Item",
            "width": 1, "height": 1},
        GRIP_LONG_TPL: {"parent": MOD, "name": "long grip", "type": "Item",
            "width": 1, "height": 1},
        EXTRA_KEYCARD_TPL: {"name": "keycard", "type": "Item", "width": 1, "height": 1},
    })
}

/// `BotConfig.Durability`, shared verbatim by the override bundle and the published `spt-bot` stem.
fn durability() -> Value {
    json!({
        "default": {"armor": {"maxDelta": 10, "minDelta": 0, "minLimitPercent": 15},
            "weapon": {"lowestMax": 60, "highestMax": 100, "maxDelta": 10, "minDelta": 0,
                       "minLimitPercent": 15}},
        "botDurabilities": {},
        "pmc": {"armor": {"lowestMaxPercent": 90, "highestMaxPercent": 100, "maxDelta": 10,
                          "minDelta": 0, "minLimitPercent": 15},
            "weapon": {"lowestMax": 95, "highestMax": 100, "maxDelta": 5, "minDelta": 0,
                       "minLimitPercent": 15}}
    })
}

/// `BotConfig.Equipment`, shared verbatim by the override bundle and the published `spt-bot` stem.
/// The `randomisation` band opens the `mod_pool_service::derive_pool` gates the scav's weapon and
/// armored-vest draws walk.
fn equipment() -> Value {
    let filters = json!({
        "blacklist": [
            {"levelRange": {"min": 1, "max": 99}, "equipment": {"Earpiece": []}},
        ],
        "randomisation": [
            {"levelRange": {"min": 1, "max": 99},
             "randomisedWeaponModSlots": ["mod_mount"],
             "randomisedArmorSlots": ["TacticalVest"]},
        ],
    });

    json!({"assault": filters, "pmc": filters})
}

/// The override's views, value-identical to what the publish below derives or lifts.
fn views_override() -> Value {
    json!({
        "items": view_items(),
        "itemPresets": {
            "p1": {"items": preset_p1_items(), "id": "p1", "name": "rifle_default",
                   "encyclopedia": RIFLE_TPL},
            "p2": {"items": preset_p2_items(), "id": "p2", "name": "rifle_alt"},
        },
        "defaultPresetsByTpl": {RIFLE_TPL: "p1"},
        "handbookPrices": {
            WEAPON: 0.0, MAGAZINE: 0.0, AMMO: 0.0, MOD: 0.0,
            HEADWEAR_TPL: 0.0, EARPIECE_TPL: 0.0, FACE_COVER_TPL: 0.0, ARMOR_TPL: 0.0,
            VEST_ARMORED_TPL: 0.0, VEST_PLAIN_TPL: 0.0, PLATE_TPL: 0.0, BACKPACK_TPL: 0.0,
            POCKETS_TPL: 0.0, SECURE_TPL: 0.0, ARMBAND_TPL: 0.0,
            RIFLE_TPL: 50000.0, MAG_TPL: 500.0, AMMO_TPL: 50.0,
            MOUNT_TPL: 0.0, LIGHT_TPL: 0.0, GRIP_STUBBY_TPL: 0.0, GRIP_LONG_TPL: 0.0,
            EXTRA_KEYCARD_TPL: 0.0,
        },
        "expTable": [100, 200, 400],
        "bosses": [],
        "durability": durability(),
        "itemSpawnLimits": {"assault": {}, "pmc": {}},
        "walletLoot": {"chancePercent": 0, "itemCount": {"min": 0, "max": 0},
            "stackSizeWeight": {}, "currencyWeight": {}, "walletTplPool": []},
        "currencyStackSize": {},
        "secureContainerAmmoStackCount": 0,
        "disableLootOnBotTypes": [],
        "lowProfileGasBlockTpls": [],
        "lootItemResourceRandomization": {},
        "equipment": equipment(),
        "pmcConfig": {},
        "repairKitWeapon": {"rarityWeight": {}, "bonusTypeWeight": {}, "Common": {}, "Rare": {}},
        "configBlacklist": [],
    })
}

/// The `configs` root the publish carries, mirroring [`views_override`]'s config half value for
/// value.
fn configs_root() -> Value {
    json!({
        "spt-bot": {
            "kind": "spt-bot",
            "bosses": [],
            "durability": durability(),
            "itemSpawnLimits": {"assault": {}, "pmc": {}},
            "walletLoot": {"chancePercent": 0, "itemCount": {"min": 0, "max": 0},
                "stackSizeWeight": {}, "currencyWeight": {}, "walletTplPool": []},
            "currencyStackSize": {},
            "secureContainerAmmoStackCount": 0,
            "disableLootOnBotTypes": [],
            "lowProfileGasBlockTpls": [],
            "lootItemResourceRandomization": {},
            "equipment": equipment(),
        },
        "spt-pmc": {"kind": "spt-pmc"},
        "spt-repair": {"kind": "spt-repair", "repairKit": {
            "weapon": {"rarityWeight": {}, "bonusTypeWeight": {}, "Common": {}, "Rare": {}},
        }},
        "spt-item": {"kind": "spt-item"},
    })
}

/// Every count weight a single `0` entry: `GetWeightedValue` short-circuits without drawing, so
/// the loot phase contributes neither items nor draws (one spare magazine excepted) and the only
/// container content left is the karma extra-loot pass's.
fn zero_loot_counts() -> Value {
    let zero = json!({"weights": {"0": 1}});

    json!({
        "grenades": zero, "healing": zero, "drugs": zero, "food": zero, "drink": zero,
        "currency": zero, "stims": zero, "backpackLoot": zero, "pocketLoot": zero,
        "vestLoot": zero, "specialItems": zero,
        "magazines": {"weights": {"1": 1}},
    })
}

/// The scav's template, pre-karma: `Headwear`, `FaceCover` and `mod_mount` all at 100% with a
/// non-empty pool, so each of the three karma modifiers below has something to take away — and the
/// `TacticalVest` the extra-loot pass fills first.
fn template() -> Value {
    json!({
        "inventory": {
            "equipment": {
                "Headwear": {HEADWEAR_TPL: 1},
                "Earpiece": {EARPIECE_TPL: 1},
                "FaceCover": {FACE_COVER_TPL: 1},
                "ArmorVest": {ARMOR_TPL: 1},
                "TacticalVest": {VEST_ARMORED_TPL: 1, VEST_PLAIN_TPL: 1},
                "Backpack": {BACKPACK_TPL: 1},
                "Pockets": {POCKETS_TPL: 1},
                "SecuredContainer": {SECURE_TPL: 1},
                "ArmBand": {ARMBAND_TPL: 1},
                "FirstPrimaryWeapon": {RIFLE_TPL: 1},
                "SecondPrimaryWeapon": {},
                "Holster": {},
            },
            "Ammo": {CALIBER: {AMMO_TPL: 1}},
            "items": {"Backpack": {}, "Pockets": {}, "SecuredContainer": {},
                "SpecialLoot": {}, "TacticalVest": {}},
            "mods": {RIFLE_TPL: {"mod_magazine": [MAG_TPL], "mod_mount": [MOUNT_TPL]}},
        },
        "chances": {
            "equipment": {"Headwear": 100, "Earpiece": 100, "FaceCover": 100,
                "ArmorVest": 100, "TacticalVest": 100, "Backpack": 100, "ArmBand": 100,
                "FirstPrimaryWeapon": 100, "SecondPrimaryWeapon": 0, "Holster": 0},
            "weaponMods": {"mod_magazine": 100, "mod_mount": 100,
                "mod_flashlight": 100, "mod_foregrip": 100},
            "equipmentMods": {"front_plate": 100},
        },
        "generation": {"items": zero_loot_counts()},
    })
}

/// The wave-constant block — live C# process state only.
fn shared() -> Value {
    json!({
        "generatingPlayerLevel": 20,
        "isNightTime": false,
        "liveEquipmentMods": {"assault": [], "pmc": []},
    })
}

/// One `KarmaLevel`, every wire member non-empty and every one of them the *only* reason for what
/// the response then shows: the equipment modifier zeroes `Headwear`, the mod modifier zeroes
/// `mod_mount` (taking its two children with it), the blacklist empties the `FaceCover` pool, and
/// the certain extra-loot chance places the keycard.
///
/// The blacklist deliberately names a `FaceCover` tpl rather than a rig: a `TacticalVest` entry
/// would be a *vacuous* assertion either way, because the bot wears an `ArmorVest` and
/// `filter_rigs_to_those_without_protection` then drops the armored rig from the pool
/// unconditionally — with the plain one blacklisted instead, the pool is empty and no vest
/// generates at all. `FaceCover` has one pooled tpl at a 100% chance and no filter of its own
/// touches it, so its absence below is `apply_equipment_blacklist` and nothing else.
fn karma() -> Value {
    json!({
        "equipmentModifiers": {"Headwear": -100.0},
        "modModifiers": {"mod_mount": -100.0},
        "equipmentBlacklist": {"FaceCover": [FACE_COVER_TPL]},
        "lootItemsToAddChancePercent": {EXTRA_KEYCARD_TPL: 100.0},
    })
}

/// The player scav request at `epoch`: the single-bot shape plus the karma slice.
fn request(epoch: u64, views_override: Option<Value>) -> Vec<u8> {
    let mut request = json!({
        "epoch": epoch,
        "shared": shared(),
        "bot": {
            "botId": "bot_pscav",
            "testSeed": SEED_PSCAV,
            "details": {"role": "assault", "roleLowercase": "assault", "side": "Savage",
                "botLevel": 1, "isPmc": false, "isPlayerScav": true, "gameVersion": "standard",
                "location": "bigmap", "botDifficulty": "normal",
                "clearBotContainerCacheAfterGeneration": false},
        },
        "template": template(),
        "lootPools": {},
        "karma": karma(),
    });
    if let Some(views) = views_override {
        request["viewsOverride"] = views;
    }

    serde_json::to_vec(&request).unwrap()
}

/// xxh3-128 of the resident response with minted ids stripped: an exact-output golden over the
/// whole player-scav pipeline at a fixed seed — karma-adjusted chances, the blacklisted equipment
/// pool and the extra-loot pass. Any change to what the native arm generates from this fixture
/// moves it. Rust-side for the reason `flip6_bots_resident.rs` documents: a C#-side golden cannot
/// hold across processes, this one does.
///
/// To regenerate after a deliberate generation change: put any wrong value here, run
/// `cargo test --test player_scav_resident`, and paste the `left:` value from the failure.
const RESIDENT_GOLDEN: &str = "548C92D608935063C508FD911B389709";

#[test]
fn a_resident_send_matches_the_override_send_and_a_wrong_epoch_is_stale() {
    // (1) The smallest publish that derives the bot views: templates + globals for the views
    // themselves, empty traders because the ragfair derive gates on all three roots being
    // resident, and the configs root carrying the four stems `resolve_bot_views` demands.
    let publish = json!({"schema": 1, "roots": {
        "templates": {
            "items": raw_items(),
            "handbook": {"Items": [
                {"Id": RIFLE_TPL, "Price": 50000},
                {"Id": MAG_TPL, "Price": 500},
                {"Id": AMMO_TPL, "Price": 50},
            ]},
            "prices": {},
        },
        "traders": {},
        "globals": {
            "ItemPresets": {
                "p1": {"_id": "p1", "_name": "rifle_default", "_encyclopedia": RIFLE_TPL,
                       "_items": preset_p1_items()},
                "p2": {"_id": "p2", "_name": "rifle_alt", "_items": preset_p2_items()},
            },
            "config": {"exp": {"level": {"exp_table": [
                {"exp": 100}, {"exp": 200}, {"exp": 400},
            ]}}},
        },
        "configs": configs_root(),
    }});

    let (status, out) = call(
        spt_db_publish,
        serde_json::to_vec(&publish).unwrap().as_slice(),
    );
    assert_eq!(
        status,
        STATUS_OK,
        "publish failed: {}",
        String::from_utf8_lossy(&out)
    );
    let response: Value = serde_json::from_slice(&out).unwrap();
    let epoch = response["epoch"]
        .as_u64()
        .expect("publish answers the epoch");

    // (2) The resident arm: no viewsOverride, just the published epoch.
    let (status, resident) = call(spt_generate_player_scav, &request(epoch, None));
    assert_eq!(
        status,
        STATUS_OK,
        "resident send failed: {}",
        String::from_utf8_lossy(&resident)
    );
    let resident = String::from_utf8(resident).unwrap();

    // The bot generated at all, and every karma member landed — an empty or errored response would
    // otherwise satisfy the negative assertions vacuously.
    assert!(
        resident.contains(RIFLE_TPL) && resident.contains(MAG_TPL),
        "no weapon generated: {resident}"
    );
    assert!(
        resident.contains(VEST_PLAIN_TPL),
        "no vest generated: {resident}"
    );
    // equipmentModifiers: Headwear 100% - 100 = 0%, and 0 never fires.
    assert!(!resident.contains(HEADWEAR_TPL), "headwear survived karma");
    // modModifiers: the same arithmetic on the mount slot takes its two children with it.
    assert!(
        !resident.contains(MOUNT_TPL)
            && !resident.contains(LIGHT_TPL)
            && !resident.contains(GRIP_STUBBY_TPL)
            && !resident.contains(GRIP_LONG_TPL),
        "the mount chain survived karma"
    );
    // equipmentBlacklist: FaceCover's only pooled tpl is gone, so the 100% slot generates nothing.
    assert!(
        !resident.contains(FACE_COVER_TPL),
        "the face cover survived karma"
    );
    // lootItemsToAddChancePercent: nothing else in this fixture can draw the keycard.
    assert!(
        resident.contains(EXTRA_KEYCARD_TPL),
        "the extra-loot pass added nothing: {resident}"
    );

    // (3) The same request with the equivalent viewsOverride at epoch 0.
    let (status, override_send) = call(
        spt_generate_player_scav,
        &request(0, Some(views_override())),
    );
    assert_eq!(
        status,
        STATUS_OK,
        "override send failed: {}",
        String::from_utf8_lossy(&override_send)
    );

    // The flip's promise: identical generation off either arm, minted ids aside.
    let resident_stripped = strip_mongo_ids(&resident);
    assert_eq!(
        resident_stripped,
        strip_mongo_ids(&String::from_utf8(override_send).unwrap())
    );

    // …and the exact-output pin over those same bytes — see [`RESIDENT_GOLDEN`].
    assert_eq!(
        format!(
            "{:032X}",
            xxhash_rust::xxh3::xxh3_128(resident_stripped.as_bytes())
        ),
        RESIDENT_GOLDEN
    );

    // (4) An epoch the store does not hold is stale, never a wrong answer.
    let (status, out) = call(spt_generate_player_scav, &request(epoch + 1, None));
    assert_eq!(status, STATUS_STALE_EPOCH);
    assert_eq!(
        String::from_utf8(out).unwrap(),
        "resident DB epoch mismatch; republish and retry"
    );
}
