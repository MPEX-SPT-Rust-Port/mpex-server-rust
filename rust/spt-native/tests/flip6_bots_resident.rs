//! Resident-arm integration for the bot generation flip (#6): publish a minimal
//! templates+traders+globals DB — a rifle whose slots give it a multi-entry mod pool (one slot
//! randomised, so the draw walks `derive_pool`), two presets (one default via `_encyclopedia`),
//! and a three-band exp table — then prove an `{epoch}` send generates identically to the same
//! data sent as `viewsOverride`, for both bot exports, and that a wrong epoch is a stale error,
//! not a wrong answer.
//!
//! Its own process (integration tests build their own binary), so the process-global store races
//! with nothing; the whole protocol lives in one `#[test]` fn, sequential, to keep it that way.

// The two big `json!` fixture maps out-recurse the default limit of 128.
#![recursion_limit = "256"]

use serde_json::{Value, json};
use spt_native::ffi::{
    STATUS_OK, STATUS_STALE_EPOCH, spt_buf_free, spt_db_publish, spt_generate_bot_inventory,
    spt_generate_bot_inventory_batch,
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

// The randomised-slot fixture (`mod_mount` on the rifle): a mount whose own two slots make the
// recursion iterate a `derive_pool`-derived entry, with a multi-candidate slot so the drawn grip
// depends on the inner set order as well. All non-hex, so a drifted draw stays visible through
// `strip_mongo_ids`.
const MOUNT_TPL: &str = "mount_rail";
const LIGHT_TPL: &str = "light_torch";
const GRIP_STUBBY_TPL: &str = "grip_stubby";
const GRIP_LONG_TPL: &str = "grip_long";

/// Per-slice seeds. The two PMC draws are the first thing on each bot's seeded stream, so the
/// drawn level is a pure function of the seed: 4 lands on level 1 (band A, fractional exp draw)
/// and 1 lands on level 2 (band B, whose template has no weapon mod pool, forcing the preset
/// fallback). The exact levels are asserted below — a level that moves means the RNG stream
/// changed and this fixture's band coverage must be re-pinned.
const SEED_ASSAULT: u64 = 1;
const SEED_PMC_LOW: u64 = 4;
const SEED_PMC_HIGH: u64 = 1;

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

/// Strips MongoIds (24 hex chars) — item ids are minted from the process-wide MongoId counter,
/// not the seeded RNG, so they legitimately differ between two seeded runs.
fn strip_mongo_ids(json: &str) -> String {
    let mut out = String::with_capacity(json.len());
    let mut run = String::new();
    for c in json.chars() {
        if c.is_ascii_hexdigit() {
            run.push(c);
            continue;
        }
        if run.len() == 24 {
            out.push_str("<id>");
        } else {
            out.push_str(&run);
        }
        run.clear();
        out.push(c);
    }
    if run.len() == 24 {
        out.push_str("<id>");
    } else {
        out.push_str(&run);
    }

    out
}

/// The two presets' item lists, shared verbatim by the publish's `_items` and the override's
/// `itemPresets` view so both arms clone identical bytes. p1 (root + magazine) is the default —
/// `_encyclopedia` names the rifle — and what `GetPresetWeaponMods` finds first; p2 differs in
/// length so a preset resolution that picked the wrong entry is visible in the response shape.
fn preset_p1_items() -> Value {
    json!([
        {"_id": "root_p1", "_tpl": RIFLE_TPL},
        {"_id": "mod_p1", "_tpl": MAG_TPL, "parentId": "root_p1", "slotId": "mod_magazine"},
    ])
}

fn preset_p2_items() -> Value {
    json!([{"_id": "root_p2", "_tpl": RIFLE_TPL}])
}

/// The raw `templates.items` the publish carries. The rifle's slots are the mod-pool fixture:
/// `mod_stock`'s filter is empty (never pooled), so the pool holds the other two slots and the
/// pooled pair sits behind a slot the pool drops.
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
    })
}

/// `BotConfig.Durability`, shared verbatim by the override bundle and the published `spt-bot`
/// stem: `lowestMax` 60 / `highestMax` 100 give the durability roll something to draw, and a draw
/// that read a different number on one arm would fail the byte comparison.
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

/// `BotConfig.Equipment`, shared verbatim by the override bundle and the published `spt-bot` stem —
/// resident since ABI 34, so a value that differed between the arms would fail the byte comparison.
///
/// Each role carries a `blacklist` band that covers the player level (20) and bans nothing, so
/// `select_equipment_blacklist`'s pick is exercised on both arms.
///
/// The `randomisation` band opens every `mod_pool_service::derive_pool` gate for all three bots:
/// `mod_mount` routes the weapon draw through the dynamic pool (and the drawn mount's sub-mods
/// through the service fetch), and `TacticalVest` routes an armored-vest pick's mod pool through
/// the gear derive.
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

/// The override's views, value-identical to what the publish below derives or lifts: the items
/// view, the presets map (globals key domain, map order), the default re-keyed to the preset's own
/// id, a handbook price per items-table key (0.0 for a handbook miss), the three exp bands, and the
/// thirteen config members that went resident — the `spt-bot`, `spt-pmc`, `spt-repair` and
/// `spt-item` stems the publish below carries. The rifle's non-identity slot order, the raid's
/// daylight, the player's level and the live `EquipmentMods` bands are *not* views — they ride the
/// shared varying block on both arms ([`shared`]).
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
/// value. `spt-bot`'s members are all strict, so this is also the shape a real `bot.json` publish
/// has to satisfy.
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
/// the loot phase contributes neither items nor draws (one spare magazine excepted).
fn zero_loot_counts() -> Value {
    let zero = json!({"weights": {"0": 1}});

    json!({
        "grenades": zero, "healing": zero, "drugs": zero, "food": zero, "drink": zero,
        "currency": zero, "stims": zero, "backpackLoot": zero, "pocketLoot": zero,
        "vestLoot": zero, "specialItems": zero,
        "magazines": {"weights": {"1": 1}},
    })
}

/// One bot template. `with_weapon_mod_pool: false` empties `mods`, so `GenerateModsForWeapon` is
/// skipped, the rifle misses its required magazine, and the preset fallback dresses it from p1 —
/// the band-B template below, making the preset views a consumed input, not just a mirrored one.
///
/// `mod_mount` is the randomised slot ([`shared`]'s band): its template pool entry here is what
/// puts the slot on the draw's iteration path, but the candidates come from
/// `mod_pool_service::derive_pool` instead, and so do the mount's own two sub-slots.
fn template(with_weapon_mod_pool: bool) -> Value {
    let mods = if with_weapon_mod_pool {
        json!({RIFLE_TPL: {"mod_magazine": [MAG_TPL], "mod_mount": [MOUNT_TPL]}})
    } else {
        json!({})
    };

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
            "mods": mods,
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

/// The wave-constant block both requests share — live C# process state only, now that
/// `BotConfig.Equipment` is resident too. The batch send appends `levelGeneration` and
/// `templateVariants`; the single send carries its template and loot pools at the top level.
///
/// `liveEquipmentMods` is the projection-correct overlay for [`equipment`]: its bands carry no
/// `equipmentMods` at all, so the C# projection filters every one of them out and each role's band
/// list is **empty** — not a band with an empty `equipmentMods` map.
fn shared() -> Value {
    json!({
        "generatingPlayerLevel": 20,
        "isNightTime": false,
        "liveEquipmentMods": {"assault": [], "pmc": []},
    })
}

fn slice(bot_id: &str, role: &str, role_lowercase: &str, is_pmc: bool, seed: u64) -> Value {
    json!({
        "botId": bot_id,
        "testSeed": seed,
        "details": {"role": role, "roleLowercase": role_lowercase,
            "side": if is_pmc { "Bear" } else { "Savage" },
            "botLevel": if is_pmc { 0 } else { 1 },
            "isPmc": is_pmc, "isPlayerScav": false, "gameVersion": "standard",
            "location": "bigmap", "botDifficulty": "normal",
            "clearBotContainerCacheAfterGeneration": false},
    })
}

/// The batch request at `epoch`: one non-PMC slice and two PMC slices whose seeded level draws
/// land one bot in each band — band A (levels 1..1) with the full weapon mod pool, band B
/// (levels 2..99) with none, so the preset fallback runs there.
fn batch_request(epoch: u64, views_override: Option<Value>) -> Vec<u8> {
    let mut shared = shared();
    shared["levelGeneration"] = json!({"levelMin": 1, "levelMax": 3});
    shared["templateVariants"] = json!([
        {"levelMin": 1, "levelMax": 1, "template": template(true), "lootPools": {}},
        {"levelMin": 2, "levelMax": 99, "template": template(false), "lootPools": {}},
    ]);

    let mut request = json!({
        "epoch": epoch,
        "shared": shared,
        "bots": [
            slice("bot_assault", "assault", "assault", false, SEED_ASSAULT),
            slice("bot_pmc_low", "pmcBEAR", "pmcbear", true, SEED_PMC_LOW),
            slice("bot_pmc_high", "pmcBEAR", "pmcbear", true, SEED_PMC_HIGH),
        ],
    });
    if let Some(views) = views_override {
        request["viewsOverride"] = views;
    }

    serde_json::to_vec(&request).unwrap()
}

/// The single-bot request at `epoch`: no variants, no level generation — the template and loot
/// pools ride at the top level, pre-filtered, exactly as the per-bot path sends them today.
fn single_request(epoch: u64, views_override: Option<Value>) -> Vec<u8> {
    let mut request = json!({
        "epoch": epoch,
        "shared": shared(),
        "bot": slice("bot_single", "assault", "assault", false, SEED_ASSAULT),
        "template": template(true),
        "lootPools": {},
    });
    if let Some(views) = views_override {
        request["viewsOverride"] = views;
    }

    serde_json::to_vec(&request).unwrap()
}

/// xxh3-128 of the resident batch response with minted ids stripped: an exact-output golden over
/// all three bots at fixed seeds — both PMC level bands and band B's preset fallback. Any change to
/// what the native arm generates from this fixture moves it, so it is end-to-end drift detection
/// over the bot FFI pipeline.
///
/// **It pins the mod-pool ordering ABI 32 moved into this crate.** [`equipment`]'s `randomisation`
/// band opens all three gated routes into `mod_pool_service::derive_pool` (each confirmed by
/// making the respective service function panic — the test fails): the randomised `mod_mount` is
/// drawn from the dynamic pool (`get_compatible_mods_for_weapon_slot`), the drawn mount's own
/// sub-mods come from the service fetch (`get_mods_for_weapon_slot`), and an armored-vest pick
/// rebuilds its mod pool through the gear derive (`get_mods_for_gear_slot`). The mount's two
/// derived sub-slots make the derived entry's *key order* observable — the recursion appends their
/// items in iteration order — and `mod_foregrip`'s two candidates make the *inner set order*
/// observable through the seeded pick, so either order regressing moves the hash.
///
/// It is a *Rust-side* golden because a C#-side one is impossible: `MongoId.GetHashCode()`
/// (`MongoId.cs:325`) is `HashCode.Combine(...)`, which .NET seeds per process, so every
/// `Dictionary<MongoId, …>` the C# projection serialises enumerates in a process-random order that
/// the seeded draw then walks. Nothing here has that problem — this fixture drives the exports in
/// its own process, and `src/bot/` has no `HashMap` at all, its four `HashSet`s being
/// membership-tested rather than iterated; everything the draw walks is an `IndexMap`/`IndexSet`.
/// That is the load-bearing result: a Rust-side golden *does* hold across processes where a
/// C#-side one cannot.
///
/// To regenerate after a deliberate generation change: put any wrong value here, run
/// `cargo test --test flip6_bots_resident`, and paste the `left:` value from the failure.
const RESIDENT_BATCH_GOLDEN: &str = "87A743ED988C6A8F7ADEE225F0E28062";

#[test]
fn a_resident_send_matches_the_override_send_and_a_wrong_epoch_is_stale() {
    // (1) The smallest publish that derives the bot views: templates + globals for the views
    // themselves, empty traders because the ragfair derive (whose items/presets maps the bot
    // views embed) gates on all three roots being resident, and the configs root carrying the
    // four stems `resolve_bot_views` demands.
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

    // (2) The batch, resident arm: no viewsOverride, just the published epoch.
    let (status, resident) = call(
        spt_generate_bot_inventory_batch,
        &batch_request(epoch, None),
    );
    assert_eq!(
        status,
        STATUS_OK,
        "resident batch send failed: {}",
        String::from_utf8_lossy(&resident)
    );

    // Three result envelopes that all errored would compare equal vacuously — pin what each bot
    // must have produced before comparing the arms.
    let resident_batch: Value = serde_json::from_slice(&resident).unwrap();
    let bots = resident_batch["bots"].as_array().expect("bots array");
    assert_eq!(bots.len(), 3);
    for (index, bot) in bots.iter().enumerate() {
        assert!(
            bot["error"].is_null(),
            "bot {index} errored: {}",
            bot["error"]
        );
    }
    // The non-PMC constant pair, no draw consumed.
    assert_eq!(bots[0]["result"]["level"], json!(1));
    assert_eq!(bots[0]["result"]["exp"], json!(0));
    // The low PMC drew level 1 off the seeded stream: band A, base exp 100 plus the seed-pinned
    // fractional draw of 126 from exp_table[1] — the resident exp table read end to end.
    assert_eq!(bots[1]["result"]["level"], json!(1));
    assert_eq!(bots[1]["result"]["exp"], json!(226));
    // The high PMC drew level 2: band B, whole-band exp sum (100+200), no fractional at the
    // table's max index — and band B's poolless template forced the p1 preset fallback, whose
    // cloned item ids ride out verbatim.
    assert_eq!(bots[2]["result"]["level"], json!(2));
    assert_eq!(bots[2]["result"]["exp"], json!(300));
    let high = serde_json::to_string(&bots[2]).unwrap();
    assert!(high.contains("root_p1"), "no preset fallback: {high}");
    let resident = String::from_utf8(resident).unwrap();
    assert!(resident.contains(RIFLE_TPL) && resident.contains(MAG_TPL));
    // The randomised-slot chain: the mount only reaches the weapon through the dynamic pool, and
    // its light and grip children only exist in the derived (`derive_pool`) entry the service
    // fetch inserts — their presence keeps the ordering pin non-vacuous.
    assert!(
        resident.contains(MOUNT_TPL) && resident.contains(LIGHT_TPL) && resident.contains("grip_"),
        "the randomised mount chain was not drawn"
    );

    // (3) The same batch with the equivalent viewsOverride at epoch 0.
    let (status, override_send) = call(
        spt_generate_bot_inventory_batch,
        &batch_request(0, Some(views_override())),
    );
    assert_eq!(
        status,
        STATUS_OK,
        "override batch send failed: {}",
        String::from_utf8_lossy(&override_send)
    );

    // The flip's promise: identical generation off either arm, minted ids aside.
    let resident_stripped = strip_mongo_ids(&resident);
    assert_eq!(
        resident_stripped,
        strip_mongo_ids(&String::from_utf8(override_send).unwrap())
    );

    // …and the exact-output pin over those same bytes — see [`RESIDENT_BATCH_GOLDEN`].
    assert_eq!(
        format!(
            "{:032X}",
            xxhash_rust::xxh3::xxh3_128(resident_stripped.as_bytes())
        ),
        RESIDENT_BATCH_GOLDEN
    );

    // (4) The single-bot export, same resident-vs-override comparison.
    let (status, resident_single) = call(spt_generate_bot_inventory, &single_request(epoch, None));
    assert_eq!(
        status,
        STATUS_OK,
        "resident single send failed: {}",
        String::from_utf8_lossy(&resident_single)
    );
    let resident_single = String::from_utf8(resident_single).unwrap();
    assert!(
        resident_single.contains(RIFLE_TPL),
        "no weapon generated: {resident_single}"
    );

    let (status, override_single) = call(
        spt_generate_bot_inventory,
        &single_request(0, Some(views_override())),
    );
    assert_eq!(
        status,
        STATUS_OK,
        "override single send failed: {}",
        String::from_utf8_lossy(&override_single)
    );
    assert_eq!(
        strip_mongo_ids(&resident_single),
        strip_mongo_ids(&String::from_utf8(override_single).unwrap())
    );

    // (5) An epoch the store does not hold is stale, never a wrong answer.
    let (status, out) = call(
        spt_generate_bot_inventory_batch,
        &batch_request(epoch + 1, None),
    );
    assert_eq!(status, STATUS_STALE_EPOCH);
    assert_eq!(
        String::from_utf8(out).unwrap(),
        "resident DB epoch mismatch; republish and retry"
    );
}
