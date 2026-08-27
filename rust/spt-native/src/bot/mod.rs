pub(crate) mod bot_equipment_mod_generator;
pub(crate) mod bot_generator_helper;
pub(crate) mod bot_inventory_generator;
pub(crate) mod bot_loot_generator;
pub(crate) mod bot_weapon_generator;
pub(crate) mod bot_weapon_generator_helper;
pub(crate) mod durability_limits_helper;
pub(crate) mod exhaustable_array;
pub(crate) mod inventory_mag_gen;
pub mod level_generator;
pub(crate) mod mod_pool_service;
pub mod models;
pub(crate) mod repair_service;
pub mod views;

use indexmap::IndexMap;

use crate::bot::durability_limits_helper::BotDurability;
use crate::bot::models::{
    BotViewsWire, EquipmentFilterDetails, EquipmentFilters, LiveEquipmentModsBandWire,
    PmcConfigWire, RandomisedResourceDetails, WalletLootSettingsWire,
};
use crate::bot::repair_service::BonusSettings;
use crate::bot::views::BotDbViews;
use crate::db::models::{BotConfigLift, ConfigsRoot, ItemConfigLift, RepairConfigLift};
use crate::diag::DiagSink;
use crate::loot::item_helper::{LootEpochError, LootError};
use crate::loot::models::{ItemView, PresetView};

use std::collections::HashSet;
use std::sync::Arc;

/// The database half of a bot request. The resident arm is the published [`BotDbViews`] — the
/// items and preset views it shares with ragfair plus the bot-only derivations — together with the
/// resident configs root the family's four stems come out of; the override arm is all of it on the
/// wire. The raid's daylight, the player's level and the live `EquipmentMods` bands are not views —
/// they are live C# process state — so they ride the shared varying block on every send. The rest
/// of `BotConfig.Equipment` *is* a view;
/// [`resolve_equipment`] puts the two halves back together per call.
pub enum BotViews {
    Override(Box<BotViewsWire>),
    Resident {
        views: Arc<BotDbViews>,
        /// The resident configs root. [`resolve_bot_views`] has already proved all four stems this
        /// family reads are present, so the accessors below cannot miss them.
        configs: Arc<ConfigsRoot>,
    },
}

/// The `spt-bot` stem, present because [`resolve_bot_views`] refused the request without it.
fn bot_config(configs: &ConfigsRoot) -> &BotConfigLift {
    configs
        .bot
        .as_ref()
        .expect("resolve_bot_views proved the spt-bot stem present")
}

/// The `spt-pmc` stem, present because [`resolve_bot_views`] refused the request without it.
fn pmc_config_stem(configs: &ConfigsRoot) -> &PmcConfigWire {
    configs
        .pmc
        .as_ref()
        .expect("resolve_bot_views proved the spt-pmc stem present")
}

/// The `spt-repair` stem, present because [`resolve_bot_views`] refused the request without it.
fn repair_config(configs: &ConfigsRoot) -> &RepairConfigLift {
    configs
        .repair
        .as_ref()
        .expect("resolve_bot_views proved the spt-repair stem present")
}

/// The `spt-item` stem, present because [`resolve_bot_views`] refused the request without it.
fn item_config(configs: &ConfigsRoot) -> &ItemConfigLift {
    configs
        .item
        .as_ref()
        .expect("resolve_bot_views proved the spt-item stem present")
}

impl BotViews {
    pub(crate) fn items(&self) -> &IndexMap<String, ItemView> {
        match self {
            Self::Override(wire) => &wire.items,
            Self::Resident { views, .. } => &views.ragfair.items,
        }
    }

    pub(crate) fn item_presets(&self) -> &IndexMap<String, PresetView> {
        match self {
            Self::Override(wire) => &wire.item_presets,
            Self::Resident { views, .. } => &views.ragfair.item_presets,
        }
    }

    /// Keyed by the tpl the preset is the default for, valued by the preset's own id.
    pub(crate) fn default_preset_ids(&self) -> &IndexMap<String, String> {
        match self {
            Self::Override(wire) => &wire.default_presets_by_tpl,
            Self::Resident { views, .. } => &views.default_preset_ids_by_tpl,
        }
    }

    /// A tpl missing from either arm prices at 0 at the consumer, which is what
    /// `HandbookHelper.GetTemplatePrice` returns for a tpl the handbook does not know.
    pub(crate) fn handbook_prices(&self) -> &IndexMap<String, f64> {
        match self {
            Self::Override(wire) => &wire.handbook_prices,
            Self::Resident { views, .. } => &views.ragfair.handbook_prices,
        }
    }

    pub(crate) fn exp_table(&self) -> &[i32] {
        match self {
            Self::Override(wire) => &wire.exp_table,
            Self::Resident { views, .. } => &views.exp_table,
        }
    }

    /// `BotConfig.Bosses`.
    pub(crate) fn bosses(&self) -> &[String] {
        match self {
            Self::Override(wire) => &wire.bosses,
            Self::Resident { configs, .. } => &bot_config(configs).bosses,
        }
    }

    /// `BotConfig.Durability`.
    pub(crate) fn durability(&self) -> &BotDurability {
        match self {
            Self::Override(wire) => &wire.durability,
            Self::Resident { configs, .. } => &bot_config(configs).durability,
        }
    }

    /// `BotConfig.ItemSpawnLimits`.
    pub(crate) fn item_spawn_limits(&self) -> &IndexMap<String, IndexMap<String, f64>> {
        match self {
            Self::Override(wire) => &wire.item_spawn_limits,
            Self::Resident { configs, .. } => &bot_config(configs).item_spawn_limits,
        }
    }

    /// `BotConfig.WalletLoot`.
    pub(crate) fn wallet_loot(&self) -> &WalletLootSettingsWire {
        match self {
            Self::Override(wire) => &wire.wallet_loot,
            Self::Resident { configs, .. } => &bot_config(configs).wallet_loot,
        }
    }

    /// `BotConfig.CurrencyStackSize`.
    pub(crate) fn currency_stack_size(
        &self,
    ) -> &IndexMap<String, IndexMap<String, IndexMap<String, f64>>> {
        match self {
            Self::Override(wire) => &wire.currency_stack_size,
            Self::Resident { configs, .. } => &bot_config(configs).currency_stack_size,
        }
    }

    /// `BotConfig.SecureContainerAmmoStackCount`.
    pub(crate) fn secure_container_ammo_stack_count(&self) -> i32 {
        match self {
            Self::Override(wire) => wire.secure_container_ammo_stack_count,
            Self::Resident { configs, .. } => bot_config(configs).secure_container_ammo_stack_count,
        }
    }

    /// `BotConfig.DisableLootOnBotTypes`.
    pub(crate) fn disable_loot_on_bot_types(&self) -> &HashSet<String> {
        match self {
            Self::Override(wire) => &wire.disable_loot_on_bot_types,
            Self::Resident { configs, .. } => &bot_config(configs).disable_loot_on_bot_types,
        }
    }

    /// `BotConfig.LowProfileGasBlockTpls`.
    pub(crate) fn low_profile_gas_block_tpls(&self) -> &HashSet<String> {
        match self {
            Self::Override(wire) => &wire.low_profile_gas_block_tpls,
            Self::Resident { configs, .. } => &bot_config(configs).low_profile_gas_block_tpls,
        }
    }

    /// `BotConfig.LootItemResourceRandomization`.
    pub(crate) fn loot_item_resource_randomization(
        &self,
    ) -> &IndexMap<String, RandomisedResourceDetails> {
        match self {
            Self::Override(wire) => &wire.loot_item_resource_randomization,
            Self::Resident { configs, .. } => &bot_config(configs).loot_item_resource_randomization,
        }
    }

    /// `PmcConfig`, narrowed to what bot generation reads.
    pub(crate) fn pmc_config(&self) -> &PmcConfigWire {
        match self {
            Self::Override(wire) => &wire.pmc_config,
            Self::Resident { configs, .. } => pmc_config_stem(configs),
        }
    }

    /// `RepairConfig.RepairKit.Weapon`.
    pub(crate) fn repair_kit_weapon(&self) -> &BonusSettings {
        match self {
            Self::Override(wire) => &wire.repair_kit_weapon,
            Self::Resident { configs, .. } => &repair_config(configs).repair_kit.weapon,
        }
    }

    /// `ItemFilterService.GetBlacklistedItems()` = `ItemConfig.Blacklist`.
    pub(crate) fn config_blacklist(&self) -> &HashSet<String> {
        match self {
            Self::Override(wire) => &wire.config_blacklist,
            Self::Resident { configs, .. } => &item_config(configs).blacklist,
        }
    }
}

/// The override arm resolves without consulting the process-global store; the resident arm needs
/// the named epoch resident with the bot views derived *and* the configs root carrying all four
/// stems this family reads.
///
/// A missing root is a stale epoch — the publish never carried it, so a republish is the fix. A
/// configs root that *is* resident but is missing a stem is a different failure and gets a
/// different answer: an error naming that stem, per call, rather than a silent default.
///
/// # Errors
///
/// [`LootEpochError::StaleEpoch`] as above, or [`LootEpochError::Loot`] naming the absent stem.
pub fn resolve_bot_views(
    epoch: u64,
    views_override: Option<Box<BotViewsWire>>,
) -> Result<BotViews, LootEpochError> {
    if let Some(views) = views_override {
        return Ok(BotViews::Override(views)); // never touches the store
    }

    let db = crate::db::current().ok_or(LootEpochError::StaleEpoch)?;
    if db.epoch != epoch {
        return Err(LootEpochError::StaleEpoch);
    }

    let views = db.bot_views.clone().ok_or(LootEpochError::StaleEpoch)?;
    let configs = db.configs.clone().ok_or(LootEpochError::StaleEpoch)?;

    for (present, stem) in [
        (configs.bot.is_some(), "spt-bot"),
        (configs.pmc.is_some(), "spt-pmc"),
        (configs.repair.is_some(), "spt-repair"),
        (configs.item.is_some(), "spt-item"),
    ] {
        if !present {
            return Err(LootError::new(format!("configs root has no {stem} stem")).into());
        }
    }

    Ok(BotViews::Resident { views, configs })
}

/// Resident (or override) equipment with the live `EquipmentMods` overlay applied.
///
/// Each overlay band replaces the `equipment_mods` of the first NOT-YET-WRITTEN band that has an
/// equal `levelRange` **and** `Some` `equipment_mods` — each such band is written at most once, so
/// duplicate ranges pair positionally. The `is_some` half of the predicate is what makes that
/// pairing sound: the sender skips the bands whose live `EquipmentMods` is null
/// (`BotPayloadProjection.cs:101`), so the overlay is a *subsequence* of the role's `randomisation`
/// list and both ends only line up if the merge skips the same bands. A resident band can only
/// carry `None` here if its live counterpart is null too — `EquipmentMods` flips null↔`Some` only
/// through a barriered property setter, which republishes — so no skipped band is ever owed mods.
/// Unmatched overlay entries drop: their existence implies a post-publish band-structure container
/// edit — or a runtime write to a band's `LevelRange`, whose setters Ceciler never barriers
/// (`MinMax<T>` is an open generic) — which is the Broken ledger's booked stale window either way.
/// And if the sender/merge invariant above ever broke, a duplicate-range group would not merely
/// drop: the positional pairing could hand one band's live mods to its range-twin neighbour, so a
/// fresh band would inherit another band's clamps. That invariant is enforced only by the write
/// barriers' denylist, a different repo tree.
///
/// The resident arm skips the `None` roles, which is the filter the C# projection applies on the
/// override arm (`BotPayloadProjection.cs:149`).
pub fn resolve_equipment(
    views: &BotViews,
    live: &IndexMap<String, Vec<LiveEquipmentModsBandWire>>,
) -> IndexMap<String, EquipmentFilters> {
    let mut merged: IndexMap<String, EquipmentFilters> = match views {
        BotViews::Override(wire) => wire.equipment.clone(),
        BotViews::Resident { configs, .. } => bot_config(configs)
            .equipment
            .iter()
            .filter_map(|(role, filters)| Some((role.clone(), filters.clone()?)))
            .collect(),
    };

    for (role, bands) in live {
        if let Some(filters) = merged.get_mut(role)
            && let Some(randomisation) = filters.randomisation.as_mut()
        {
            let mut written = vec![false; randomisation.len()];
            for band in bands {
                if let Some((index, target)) =
                    randomisation
                        .iter_mut()
                        .enumerate()
                        .find(|(index, resident)| {
                            !written[*index]
                                && resident.level_range == band.level_range
                                && resident.equipment_mods.is_some()
                        })
                {
                    target.equipment_mods = Some(band.equipment_mods.clone());
                    written[index] = true;
                }
            }
        }
    }

    merged
}

/// `BotEquipmentFilterService.GetBotEquipmentBlacklist` (`BotEquipmentFilterService.cs:137-144`),
/// bug for bug:
///
/// ```csharp
/// var blacklistDetailsForBot = BotEquipmentConfig.GetValueOrDefault(botRole, null);
///
/// return (blacklistDetailsForBot?.Blacklist ?? []).FirstOrDefault(equipmentFilter =>
///     playerLevel >= equipmentFilter.LevelRange.Min && playerLevel <= equipmentFilter.LevelRange.Max
/// );
/// ```
///
/// So: the first band whose inclusive `levelRange` contains the level, `None` for an unknown role,
/// a role with no `blacklist`, or a level no band covers. Not a draw — a lookup — so moving it
/// native adds, removes and reorders nothing on the RNG stream.
///
/// Called **twice** per bot with two different levels: the equipment path defaults an absent player
/// level to 1 and the weapon-mod path to **0** (`BotPayloads.cs:184-191`,
/// `BotEquipmentModGenerator.cs:546`). Level 0 matches no stock `levelRange`, so on a session with
/// no PMC profile the two answers deliberately differ. Legacy is internally inconsistent there and
/// this port has to be too.
pub(crate) fn select_equipment_blacklist<'a>(
    equipment: &'a IndexMap<String, EquipmentFilters>,
    role: &str,
    player_level: i32,
) -> Option<&'a EquipmentFilterDetails> {
    equipment
        .get(role)?
        .blacklist
        .as_ref()?
        .iter()
        .find(|band| player_level >= band.level_range.min && player_level <= band.level_range.max)
}

/// Both blacklists one bot needs, as `(equipment path, weapon-mod path)`: the same
/// [`select_equipment_blacklist`] call twice, differing only in how an absent `player_level`
/// defaults — `1` for the equipment path (written at `BotInventoryGenerator.cs:614` and its six
/// siblings, effective as `GetValueOrDefault(1)` at the call, `:937-939`) and **0** for the
/// weapon-mod path (`BotEquipmentModGenerator.cs:546`). The single place that pair lives, so the
/// two cannot drift apart.
///
/// No band covering the level is [`NO_EQUIP_BLACKLIST`], the `?? new EquipmentFilterDetails()`
/// the C# projection applied before the lift (`BotPayloadProjection.cs:135-138`).
pub(crate) fn select_equipment_blacklists<'a>(
    equipment: &'a IndexMap<String, EquipmentFilters>,
    role: &str,
    player_level: Option<i32>,
) -> (&'a EquipmentFilterDetails, &'a EquipmentFilterDetails) {
    let select =
        |level| select_equipment_blacklist(equipment, role, level).unwrap_or(&NO_EQUIP_BLACKLIST);

    (
        select(player_level.unwrap_or(1)),
        select(player_level.unwrap_or(0)),
    )
}

/// `?? new EquipmentFilterDetails()` — what both blacklist resolutions land on when
/// [`select_equipment_blacklist`] finds no band (`BotPayloadProjection.cs:135-138` before the
/// lift). Also the empty stand-in for fixtures that exercise no blacklist.
pub(crate) static NO_EQUIP_BLACKLIST: std::sync::LazyLock<EquipmentFilterDetails> =
    std::sync::LazyLock::new(EquipmentFilterDetails::default);

/// The read-only views one bot generation run consults, plus the [`DiagSink`] its diagnostics
/// emit through — the bot family's analog of [`crate::loot::item_helper::LootContext`].
///
/// Every view is borrowed for `'a`, so copying one out (`let items = ctx.items;`) releases the
/// `&mut ctx` and leaves the diagnostics writable.
pub struct BotContext<'a> {
    /// The `TemplateItem` slice, borrowed from [`BotViews::items`] (resident or override). There
    /// is no `ItemsView` type; the map itself is the view, matching `loot::item_helper`'s helpers.
    pub items: &'a IndexMap<String, ItemView>,
    /// `BotConfig.Bosses` — `BotHelper.IsBotBoss` scans it, so
    /// `durability_limits_helper::get_durability_role` needs it on every durability roll.
    pub bosses: &'a [String],
    /// `BotConfig.Durability`.
    pub durability: &'a BotDurability,
    /// `BotConfig.Equipment` as [`resolve_equipment`] merged it, keyed by *equipment* role —
    /// `pmcBEAR`/`pmcUSEC` collapse to `pmc` through
    /// `bot_generator_helper::get_bot_equipment_role` before the lookup. The whole map, not one
    /// resolved entry: `GenerateExtraPropertiesForItem` takes a per-item `botRole` and
    /// `PlayerScavGenerator.cs:177` passes a literal `"assault"` that need not be the bot's own.
    pub equipment: &'a IndexMap<String, EquipmentFilters>,
    /// `BotConfig.LootItemResourceRandomization`, keyed by the raw bot role (no equipment-role
    /// mapping — `BotGeneratorHelper.cs:63` looks it up verbatim).
    pub loot_item_resource_randomization: &'a IndexMap<String, RandomisedResourceDetails>,
    /// `RaidConfiguration.IsNightRaid`, hoisted by the C# caller. C# reads it off
    /// `profileActivityService.GetFirstProfileActivityRaidData()?.RaidConfiguration`, whose absence
    /// (no raid) defaults to day — the caller folds that into this `false`.
    pub is_night_time: bool,
    /// `ItemFilterService.GetBlacklistedItems()` — one half of the union
    /// `BotEquipmentModGenerator.FilterModsByBlacklist` builds.
    pub item_blacklist: &'a HashSet<String>,
    /// `PresetHelper.GetDefaultPresetByTpl()`, keyed by the tpl the preset is the default for and
    /// valued by the preset's own id — resolve it through [`Self::item_presets`], which is what
    /// `PresetHelper` resolves every default out of. `GetDefaultPresetArmorSlot` reads it.
    pub default_presets_by_tpl: &'a IndexMap<String, String>,
    /// `GlobalTable.ItemPresets`, keyed by preset `_id` — scanned **in order** by
    /// `BotWeaponGenerator.GetPresetWeaponMods` (`:337`), which takes the first preset whose root
    /// item matches the weapon tpl, so the map has to stay ordered.
    pub item_presets: &'a IndexMap<String, PresetView>,
    /// `GetBotEquipmentBlacklist(equipmentRole, playerLevel)`, resolved by the C# caller. The
    /// equipment path takes its blacklist as a parameter because the C# does.
    pub equipment_blacklist: &'a EquipmentFilterDetails,
    /// The weapon path's own blacklist — it resolves one internally (`:546`) with the player level
    /// defaulted to 0 rather than 1, which matches no `levelRange`, so it is a different object
    /// from [`Self::equipment_blacklist`] and cannot be shared with the equipment path.
    pub weapon_mod_equipment_blacklist: &'a EquipmentFilterDetails,
    /// `BotConfig.LowProfileGasBlockTpls` — membership tests only (`:1063`, `:1072`).
    pub low_profile_gas_block_tpls: &'a HashSet<String>,
    /// `PmcConfig.WeaponHasEnhancementChancePercent` — the gate on `RepairService.AddBuff`
    /// (`BotWeaponGenerator.cs:154`).
    pub weapon_has_enhancement_chance_percent: f64,
    /// `RepairConfig.RepairKit.Weapon` — the only `BonusSettings` bot generation passes to
    /// [`crate::bot::repair_service::add_buff`].
    pub repair_kit_weapon: &'a BonusSettings,
    /// `BotConfig.SecureContainerAmmoStackCount` (`BotConfig.cs:85`).
    pub secure_container_ammo_stack_count: i32,
    pub diagnostics: DiagSink,
}

/// Empty stand-ins for the views a fixture that exercises none of them still has to supply.
#[cfg(test)]
pub(crate) static NO_BLACKLIST: std::sync::LazyLock<HashSet<String>> =
    std::sync::LazyLock::new(HashSet::new);
#[cfg(test)]
pub(crate) static NO_PRESETS: std::sync::LazyLock<IndexMap<String, PresetView>> =
    std::sync::LazyLock::new(IndexMap::new);
#[cfg(test)]
pub(crate) static NO_DEFAULT_PRESETS: std::sync::LazyLock<IndexMap<String, String>> =
    std::sync::LazyLock::new(IndexMap::new);
#[cfg(test)]
pub(crate) static NO_BUFFS: std::sync::LazyLock<BonusSettings> = std::sync::LazyLock::new(|| {
    serde_json::from_value(serde_json::json!({
        "rarityWeight": {}, "bonusTypeWeight": {}, "Common": {}, "Rare": {}
    }))
    .expect("empty bonus settings parse")
});

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::db::tests::DB_TEST_LOCK;

    fn empty_override() -> Box<BotViewsWire> {
        override_with_equipment(json!({}))
    }

    /// [`empty_override`] with `BotConfig.Equipment` filled in — the strict member the override arm
    /// always carries.
    fn override_with_equipment(equipment: serde_json::Value) -> Box<BotViewsWire> {
        Box::new(
            serde_json::from_value(json!({
                "items": {}, "itemPresets": {}, "defaultPresetsByTpl": {},
                "bosses": [], "durability": durability(),
                "itemSpawnLimits": {}, "walletLoot": {}, "currencyStackSize": {},
                "secureContainerAmmoStackCount": 0, "disableLootOnBotTypes": [],
                "lowProfileGasBlockTpls": [], "lootItemResourceRandomization": {},
                "equipment": equipment,
                "pmcConfig": {}, "repairKitWeapon": bonus_settings(), "configBlacklist": []
            }))
            .expect("views override parses"),
        )
    }

    /// `BotDurability`'s three members are all strict, so even an "empty" fixture has to spell the
    /// shape out.
    fn durability() -> serde_json::Value {
        let armor = json!({"maxDelta": 0, "minDelta": 0, "minLimitPercent": 0});
        let weapon = json!({"lowestMax": 0, "highestMax": 0, "maxDelta": 0, "minDelta": 0,
                            "minLimitPercent": 0});

        json!({
            "default": {"armor": armor, "weapon": weapon},
            "botDurabilities": {},
            "pmc": {"armor": {"lowestMaxPercent": 0, "highestMaxPercent": 0, "maxDelta": 0,
                              "minDelta": 0, "minLimitPercent": 0},
                    "weapon": weapon},
        })
    }

    fn bonus_settings() -> serde_json::Value {
        json!({"rarityWeight": {}, "bonusTypeWeight": {}, "Common": {}, "Rare": {}})
    }

    /// The smallest publish that derives the bot views (templates + traders + globals) plus
    /// whatever `configs` root the caller wants to test the stem guard with.
    fn publish_with_configs(configs: serde_json::Value) -> u64 {
        crate::db::publish(
            serde_json::from_value(json!({"schema": 1, "roots": {
                "templates": {}, "traders": {},
                "globals": {"config": {"exp": {"level": {"exp_table": [{"exp": 10}, {"exp": 20}]}}}},
                "configs": configs,
            }}))
            .expect("publish request parses"),
        )
        .expect("publish succeeds")
    }

    /// Every stem the bot family reads, with sentinel values the accessors are read back through.
    fn bot_stems() -> serde_json::Value {
        json!({
            "spt-bot": {
                "kind": "spt-bot",
                "bosses": ["bossknight"],
                "durability": durability(),
                "itemSpawnLimits": {"assault": {"limited_tpl": 3}},
                "walletLoot": {"chancePercent": 11},
                "currencyStackSize": {"default": {"RUB": {"1000": 1}}},
                "secureContainerAmmoStackCount": 7,
                "disableLootOnBotTypes": ["bosstest"],
                "lowProfileGasBlockTpls": ["gas_block_low"],
                "lootItemResourceRandomization": {"assault": {"food": {"resourcePercent": 44}}},
                "equipment": {"assault": {}},
            },
            "spt-pmc": {"kind": "spt-pmc", "weaponHasEnhancementChancePercent": 33},
            "spt-repair": {"kind": "spt-repair", "repairKit": {
                "weapon": {"rarityWeight": {"Common": 1}, "bonusTypeWeight": {},
                           "Common": {}, "Rare": {}},
            }},
            "spt-item": {"kind": "spt-item", "blacklist": ["config_blacklisted"]},
        })
    }

    #[test]
    fn an_empty_store_is_a_stale_epoch() {
        let _guard = DB_TEST_LOCK.lock().unwrap();
        crate::db::clear();

        assert!(matches!(
            resolve_bot_views(1, None),
            Err(LootEpochError::StaleEpoch)
        ));
    }

    #[test]
    fn a_published_store_resolves_only_its_own_epoch() {
        let _guard = DB_TEST_LOCK.lock().unwrap();
        crate::db::clear();
        let epoch = publish_with_configs(bot_stems());

        assert!(matches!(
            resolve_bot_views(epoch + 1, None),
            Err(LootEpochError::StaleEpoch)
        ));

        // The matching epoch resolves the resident arm, accessors dispatching to the derived views.
        let views = resolve_bot_views(epoch, None).expect("matching epoch resolves");
        assert!(matches!(views, BotViews::Resident { .. }));
        assert_eq!(views.exp_table(), [10, 20]);
    }

    /// The three arms of the resident config resolve: a stem the publish did not carry is a loud,
    /// *named* failure and never a stale epoch; the stems it did carry are read back through the
    /// accessors; and a store that never arrived at all is stale.
    #[test]
    fn a_resident_resolve_reads_the_bot_stems_and_names_a_missing_one() {
        let _guard = DB_TEST_LOCK.lock().unwrap();

        for stem in ["spt-bot", "spt-pmc", "spt-repair", "spt-item"] {
            crate::db::clear();
            // The filler keeps the root non-empty. `spt-inventory` is all-defaulted, so it parses
            // from a bare `kind` where `spt-ragfair`/`spt-quest` would fail the publish outright.
            let mut configs = bot_stems();
            configs
                .as_object_mut()
                .unwrap()
                .remove(stem)
                .expect("the fixture carries every stem");
            configs["spt-inventory"] = json!({"kind": "spt-inventory"});

            let epoch = publish_with_configs(configs);
            let Err(LootEpochError::Loot(error)) = resolve_bot_views(epoch, None) else {
                panic!("expected a failure naming the absent {stem} stem");
            };
            assert!(error.message.contains(stem), "{error:?}");
        }

        // Present: every accessor reads its own stem's sentinel.
        crate::db::clear();
        let epoch = publish_with_configs(bot_stems());
        let views = resolve_bot_views(epoch, None).expect("every stem present resolves");

        assert_eq!(views.bosses(), ["bossknight"]);
        assert_eq!(views.durability().bot_durabilities.len(), 0);
        assert_eq!(views.item_spawn_limits()["assault"]["limited_tpl"], 3.0);
        assert_eq!(views.wallet_loot().chance_percent, 11.0);
        assert_eq!(views.currency_stack_size()["default"]["RUB"]["1000"], 1.0);
        assert_eq!(views.secure_container_ammo_stack_count(), 7);
        assert!(views.disable_loot_on_bot_types().contains("bosstest"));
        assert!(views.low_profile_gas_block_tpls().contains("gas_block_low"));
        assert_eq!(
            views.loot_item_resource_randomization()["assault"]
                .food
                .as_ref()
                .unwrap()
                .resource_percent,
            44.0
        );
        assert_eq!(
            views.pmc_config().weapon_has_enhancement_chance_percent,
            33.0
        );
        assert_eq!(views.repair_kit_weapon().rarity_weight["Common"], 1.0);
        assert!(views.config_blacklist().contains("config_blacklisted"));

        // A store that never arrived is stale, not a stem failure.
        crate::db::clear();
        assert!(matches!(
            resolve_bot_views(epoch, None),
            Err(LootEpochError::StaleEpoch)
        ));
    }

    #[test]
    fn an_override_resolves_without_any_publish() {
        let _guard = DB_TEST_LOCK.lock().unwrap();
        crate::db::clear();

        let views = resolve_bot_views(0, Some(empty_override())).expect("override resolves");

        assert!(matches!(views, BotViews::Override(_)));
        assert!(views.items().is_empty());
        assert!(views.exp_table().is_empty());
        assert!(views.bosses().is_empty());
        assert!(views.config_blacklist().is_empty());
    }

    /// `BotEquipmentFilterService.cs:137-144`, and the `level ?? 0` divergence
    /// `BotPayloads.cs:184-191` documents: one role, one band list, two levels. The band covers
    /// `[0..0]`, so the weapon-mod resolution (level defaulted to 0) selects it and the equipment
    /// resolution (level defaulted to 1) selects nothing at all.
    #[test]
    fn the_two_blacklist_resolutions_diverge_on_a_request_without_a_level() {
        let equipment: IndexMap<String, EquipmentFilters> = serde_json::from_value(json!({
            "assault": {"blacklist": [
                {"levelRange": {"min": 0, "max": 0},
                 "equipment": {"Headwear": ["banned_at_level_zero"]}},
                {"levelRange": {"min": 5, "max": 99},
                 "equipment": {"Headwear": ["banned_at_five_plus"]}},
            ]},
        }))
        .expect("equipment fixture parses");

        // No level on the request: `?? 0` for the weapon-mod path, `?? 1` for the equipment path.
        let (equip_path, weapon_mod_path) =
            select_equipment_blacklists(&equipment, "assault", None);
        assert!(
            weapon_mod_path.equipment.as_ref().unwrap()["Headwear"]
                .contains("banned_at_level_zero"),
            "the weapon-mod default of 0 must select the level-0 band"
        );
        assert!(
            equip_path.equipment.is_none(),
            "no band covers level 1, so the equipment path takes its empty default"
        );

        // With a level both resolutions agree, and `FirstOrDefault` takes the first covering band.
        let (equip_path, weapon_mod_path) =
            select_equipment_blacklists(&equipment, "assault", Some(7));
        for selected in [equip_path, weapon_mod_path] {
            assert!(
                selected.equipment.as_ref().unwrap()["Headwear"].contains("banned_at_five_plus")
            );
        }

        // An unknown role and a role with no `blacklist` are both `None`, never a panic.
        assert!(select_equipment_blacklist(&equipment, "bossknight", 1).is_none());
        let no_blacklist: IndexMap<String, EquipmentFilters> =
            serde_json::from_value(json!({"assault": {}})).expect("fixture parses");
        assert!(select_equipment_blacklist(&no_blacklist, "assault", 1).is_none());
    }

    // -- `resolve_equipment`: the resident (or override) equipment plus the live overlay.
    //
    // Every fixture below is parsed from a JSON literal, so the tests pin the *wire* names the C#
    // sender writes rather than this crate's struct internals.

    /// One role's filters as a single `randomisation` band covering `[min..max]`.
    fn filters_with_band(min: i32, max: i32, mods: serde_json::Value) -> EquipmentFilters {
        serde_json::from_value(json!({
            "randomisation": [{"levelRange": {"min": min, "max": max}, "equipmentMods": mods}],
        }))
        .expect("band fixture parses")
    }

    fn live_bands(bands: serde_json::Value) -> IndexMap<String, Vec<LiveEquipmentModsBandWire>> {
        serde_json::from_value(bands).expect("live equipment mods parse")
    }

    /// A resident-arm [`BotViews`] whose `spt-bot` lift carries `equipment`. The caller holds
    /// [`DB_TEST_LOCK`].
    fn resident_views_with_equipment(equipment: serde_json::Value) -> BotViews {
        crate::db::clear();
        let mut configs = bot_stems();
        configs["spt-bot"]["equipment"] = equipment;

        resolve_bot_views(publish_with_configs(configs), None).expect("the publish resolves")
    }

    #[test]
    fn the_overlay_replaces_the_matching_band_wholesale() {
        let _guard = DB_TEST_LOCK.lock().unwrap();
        let views = resident_views_with_equipment(json!({
            "pmc": filters_with_band(1, 14, json!({"mod_nvg": 95.0, "front_plate": 40.0})),
        }));
        let live = live_bands(json!({
            "pmc": [{"levelRange": {"min": 1, "max": 14}, "equipmentMods": {"mod_nvg": 100.0}}],
        }));

        let merged = resolve_equipment(&views, &live);

        let mods = merged["pmc"].randomisation.as_ref().unwrap()[0]
            .equipment_mods
            .as_ref()
            .unwrap();
        // Replacement, not patch: `front_plate` is gone, `mod_nvg` took the overlay value.
        assert_eq!(mods.len(), 1);
        assert_eq!(mods["mod_nvg"], 100.0);
    }

    /// An overlay entry that matches no resident band — and one naming a role the resident lift
    /// does not hold — drops. Their existence implies a post-publish band-structure edit, which is
    /// a stale window either way; inserting would be a guess at the missing filters.
    #[test]
    fn an_unmatched_overlay_band_or_role_is_dropped() {
        let _guard = DB_TEST_LOCK.lock().unwrap();
        let views = resident_views_with_equipment(json!({
            "pmc": filters_with_band(1, 14, json!({"mod_nvg": 95.0})),
        }));
        let live = live_bands(json!({
            "pmc": [{"levelRange": {"min": 15, "max": 22}, "equipmentMods": {"mod_nvg": 100.0}}],
            "unpublished": [{"levelRange": {"min": 1, "max": 14},
                             "equipmentMods": {"mod_nvg": 100.0}}],
        }));

        let merged = resolve_equipment(&views, &live);

        assert_eq!(merged.keys().collect::<Vec<_>>(), ["pmc"]);
        let bands = merged["pmc"].randomisation.as_ref().unwrap();
        // Dropped means *dropped*: an implementation that appended the unmatched band as a new
        // element would keep the mods assert below green.
        assert_eq!(bands.len(), 1);
        let mods = bands[0].equipment_mods.as_ref().unwrap();
        assert_eq!(mods["mod_nvg"], 95.0);
    }

    /// The null roles the C# per-call projection used to filter (`BotPayloadProjection.cs:149`)
    /// reach the resident root as `None`, and the merge applies that same filter — an overlay
    /// naming one cannot resurrect it.
    #[test]
    fn a_null_resident_role_stays_absent() {
        let _guard = DB_TEST_LOCK.lock().unwrap();
        let views = resident_views_with_equipment(json!({"pmc": null}));
        let live = live_bands(json!({
            "pmc": [{"levelRange": {"min": 1, "max": 14}, "equipmentMods": {"mod_nvg": 100.0}}],
        }));

        let merged = resolve_equipment(&views, &live);

        assert!(merged.is_empty(), "{merged:?}");
    }

    /// Two bands with the same `levelRange` are indistinguishable by range alone, so the pairing is
    /// positional: each resident band is written at most once, in order.
    #[test]
    fn duplicate_level_ranges_pair_positionally_and_write_each_band_once() {
        let _guard = DB_TEST_LOCK.lock().unwrap();
        let views = resident_views_with_equipment(json!({
            "pmc": {"randomisation": [
                {"levelRange": {"min": 1, "max": 14}, "equipmentMods": {"mod_nvg": 10.0}},
                {"levelRange": {"min": 1, "max": 14}, "equipmentMods": {"mod_nvg": 20.0}},
            ]},
        }));

        let live = live_bands(json!({"pmc": [
            {"levelRange": {"min": 1, "max": 14}, "equipmentMods": {"mod_nvg": 30.0}},
            {"levelRange": {"min": 1, "max": 14}, "equipmentMods": {"mod_nvg": 40.0}},
        ]}));
        let merged = resolve_equipment(&views, &live);
        let bands = merged["pmc"].randomisation.as_ref().unwrap();
        // Never both entries into band 0 — that is the clobber bug this test exists for.
        assert_eq!(bands[0].equipment_mods.as_ref().unwrap()["mod_nvg"], 30.0);
        assert_eq!(bands[1].equipment_mods.as_ref().unwrap()["mod_nvg"], 40.0);

        // One overlay entry writes band 0 only; band 1 keeps its published mods.
        let live = live_bands(json!({"pmc": [
            {"levelRange": {"min": 1, "max": 14}, "equipmentMods": {"mod_nvg": 30.0}},
        ]}));
        let merged = resolve_equipment(&views, &live);
        let bands = merged["pmc"].randomisation.as_ref().unwrap();
        assert_eq!(bands[0].equipment_mods.as_ref().unwrap()["mod_nvg"], 30.0);
        assert_eq!(bands[1].equipment_mods.as_ref().unwrap()["mod_nvg"], 20.0);
    }

    /// The overlay is a *subsequence* of the role's bands — the sender drops the ones whose live
    /// `EquipmentMods` is null (`BotPayloadProjection.cs:101`) — so the merge has to skip the very
    /// same bands or the two ends stop lining up. Same `levelRange` twice, the first band without
    /// `equipmentMods`: the single overlay entry is band 2's, and writing it onto band 1 would both
    /// invent mods for a band that has none and leave band 2 stale.
    #[test]
    fn an_overlay_band_skips_a_resident_band_that_carries_no_mods() {
        let _guard = DB_TEST_LOCK.lock().unwrap();
        let views = resident_views_with_equipment(json!({
            "pmc": {"randomisation": [
                {"levelRange": {"min": 1, "max": 14}},
                {"levelRange": {"min": 1, "max": 14}, "equipmentMods": {"mod_nvg": 20.0}},
            ]},
        }));
        let live = live_bands(json!({"pmc": [
            {"levelRange": {"min": 1, "max": 14}, "equipmentMods": {"mod_nvg": 30.0}},
        ]}));

        let merged = resolve_equipment(&views, &live);

        let bands = merged["pmc"].randomisation.as_ref().unwrap();
        assert!(
            bands[0].equipment_mods.is_none(),
            "a band the sender skipped must stay mod-less: {:?}",
            bands[0].equipment_mods
        );
        assert_eq!(bands[1].equipment_mods.as_ref().unwrap()["mod_nvg"], 30.0);
    }

    /// The override arm's equipment and its overlay are built call-time-fresh from the same live
    /// C# object, so the overlay always carries what the bands already hold: the merge has to be a
    /// no-op there, or the two arms would stop generating the same bytes. The second half proves
    /// that no-op is the overlay *agreeing* rather than the loop skipping this arm — a differing
    /// overlay value has to win.
    #[test]
    fn the_override_arm_merge_is_idempotent() {
        let equipment = json!({
            "pmc": filters_with_band(1, 14, json!({"mod_nvg": 95.0, "front_plate": 40.0})),
        });
        let views = resolve_bot_views(0, Some(override_with_equipment(equipment)))
            .expect("override resolves");
        let live = live_bands(json!({"pmc": [
            {"levelRange": {"min": 1, "max": 14},
             "equipmentMods": {"mod_nvg": 95.0, "front_plate": 40.0}},
        ]}));

        let merged = resolve_equipment(&views, &live);

        let BotViews::Override(wire) = &views else {
            panic!("the override arm resolved resident");
        };
        assert_eq!(
            serde_json::to_value(&merged).expect("merged serializes"),
            serde_json::to_value(&wire.equipment).expect("the override's equipment serializes"),
        );

        // The overlay loop runs on this arm too: a differing value wins over the override's own
        // band, so the equality above is agreement rather than a skipped merge.
        let live = live_bands(json!({"pmc": [
            {"levelRange": {"min": 1, "max": 14}, "equipmentMods": {"mod_nvg": 3.0}},
        ]}));
        let merged = resolve_equipment(&views, &live);
        let mods = merged["pmc"].randomisation.as_ref().unwrap()[0]
            .equipment_mods
            .as_ref()
            .unwrap();
        assert_eq!(mods["mod_nvg"], 3.0);
        assert_eq!(mods.len(), 1, "replacement, not patch: {mods:?}");
    }
}
