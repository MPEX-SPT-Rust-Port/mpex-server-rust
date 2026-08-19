//! The one slice of `Services/Commerce/RepairService.cs` bot generation reaches: `AddBuff`.
//!
//! `BotWeaponGenerator.cs:152-157` calls it on the weapon root once, behind an `IsPmc` +
//! `GetChance100(pmcConfig.WeaponHasEnhancementChancePercent)` gate that lives in the caller (and
//! so is ported with it, not here), always with `repairConfig.RepairKit.Weapon`. The profile-side
//! `AddBuffToItem` entry point, its armor/vest/headwear branches and the `ShouldBuffItem` skill
//! checks are not reachable from bot generation and are not ported.
//!
//! # RNG calls, in C# source order — the parity contract
//!
//! `RepairService.AddBuff` (`:504-523`) draws exactly, in this order:
//!
//! 1. `weightedRandomHelper.GetWeightedValue(itemConfig.RarityWeight)` (`:506`) → rarity name.
//! 2. `weightedRandomHelper.GetWeightedValue(itemConfig.BonusTypeWeight)` (`:507`) → bonus name.
//! 3. `randomUtil.GetDouble(valuesMinMax.Min, valuesMinMax.Max)` (`:511`) → buff value.
//! 4. `randomUtil.GetDouble(activeDurabilityPercentMinMax.Min, .Max)` (`:514`) → threshold percent.
//!
//! `randomUtil.GetPercentOfValue(thresholdPercent, durability, 0)` (`:521`) is pure arithmetic and
//! consumes nothing. Each `GetWeightedValue` is itself one draw, or none when its map holds a
//! single entry — that shortcut lives in `random_util` and applies here unchanged.
use indexmap::IndexMap;
use serde::Deserialize;

use crate::loot::item_helper::LootError;
use crate::loot::models::{Item, RepairBuffType, UpdBuff};
use crate::loot::random_util::{get_double, get_percent_of_value, get_weighted_value};

/// `Models/Spt/Config/RepairConfig.cs` — `RepairConfig.RepairKit.Weapon` is the only instance bot
/// generation passes. The weight maps are [`IndexMap`]s because insertion order is the draw order.
#[derive(Debug, Clone, Deserialize)]
pub struct BonusSettings {
    #[serde(rename = "rarityWeight")]
    pub rarity_weight: IndexMap<String, f64>,
    #[serde(rename = "bonusTypeWeight")]
    pub bonus_type_weight: IndexMap<String, f64>,
    #[serde(rename = "Common")]
    pub common: IndexMap<String, BonusValues>,
    #[serde(rename = "Rare")]
    pub rare: IndexMap<String, BonusValues>,
}

/// `Models/Spt/Config/RepairConfig.cs`.
#[derive(Debug, Clone, Deserialize)]
pub struct BonusValues {
    #[serde(rename = "valuesMinMax")]
    pub values_min_max: MinMax<f64>,
    /// What durability the buff is active between, as a percent of current max.
    #[serde(rename = "activeDurabilityPercentMinMax")]
    pub active_durability_percent_min_max: MinMax<i32>,
}

/// `Models/Common/MinMax.cs` with its members as declared — non-nullable, so a key the JSON omits
/// lands on the C# default rather than failing the parse. Distinct from
/// [`crate::loot::models::MinMaxI32`], whose members are nullable because the loot request
/// declares them that way. `MinMax.Type` is never read and is not mirrored.
#[derive(Debug, Clone, Copy, Default, Deserialize)]
pub struct MinMax<T> {
    #[serde(default)]
    pub min: T,
    #[serde(default)]
    pub max: T,
}

/// `RepairService.AddBuff` (`Services/Commerce/RepairService.cs:504-523`) — roll a random buff
/// onto `item`'s `Upd`.
///
/// # Errors
///
/// Where the C# throws: an unusable weight map (`GetWeightedValue`), a bonus type absent from the
/// chosen rarity's table (`KeyNotFoundException`), a bonus type that is not a `RepairBuffType`
/// (`Enum.Parse`), and — as `NullReferenceException` — an item with no `Upd`, no `Upd.Repairable`
/// or no `Upd.Repairable.Durability`.
pub fn add_buff(item_config: &BonusSettings, item: &mut Item) -> Result<(), LootError> {
    // 1. :506
    let bonus_rarity_name = get_weighted_value(&item_config.rarity_weight)?;
    // 2. :507
    let bonus_type_name = get_weighted_value(&item_config.bonus_type_weight)?;

    let bonus_rarity = if bonus_rarity_name == "Rare" {
        &item_config.rare
    } else {
        &item_config.common
    };
    let bonus = bonus_rarity.get(&bonus_type_name).ok_or_else(|| {
        LootError::new(format!(
            "The given key '{bonus_type_name}' was not present in the dictionary."
        ))
    })?;

    // 3. :511
    let bonus_value = get_double(bonus.values_min_max.min, bonus.values_min_max.max);
    // 4. :514 — a `MinMax<int>` widened to double by the C# overload resolution.
    let bonus_threshold_percent = get_double(
        f64::from(bonus.active_durability_percent_min_max.min),
        f64::from(bonus.active_durability_percent_min_max.max),
    );

    // `item.Upd.Buff = …` dereferences `item.Upd` before it evaluates the initializer, so this
    // check precedes the `Enum.Parse` below exactly as the C# ordering does.
    let upd = item
        .upd
        .as_mut()
        .ok_or_else(|| LootError::new("Item has no Upd to write a buff onto"))?;
    let buff_type = RepairBuffType::from_name(&bonus_type_name).ok_or_else(|| {
        LootError::new(format!(
            "Requested value '{bonus_type_name}' was not found in RepairBuffType."
        ))
    })?;
    let durability = upd
        .repairable
        .as_ref()
        .and_then(|repairable| repairable.durability)
        .ok_or_else(|| {
            LootError::new("Item has no Upd.Repairable.Durability to threshold against")
        })?;

    upd.buff = Some(UpdBuff {
        rarity: Some(bonus_rarity_name),
        buff_type: Some(buff_type),
        value: Some(bonus_value),
        threshold_durability: Some(get_percent_of_value(bonus_threshold_percent, durability, 0)),
        extra: Default::default(),
    });

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loot::models::{Upd, UpdRepairable};
    use crate::loot::random_util::TestSeedGuard;

    const SEED: u64 = 42;

    fn bonus(value_min: f64, value_max: f64) -> BonusValues {
        BonusValues {
            values_min_max: MinMax {
                min: value_min,
                max: value_max,
            },
            active_durability_percent_min_max: MinMax { min: 75, max: 90 },
        }
    }

    /// `SPT_Data/configs/repair.json` → `repairKit.weapon`, with the third `Rare` entry
    /// (`WeaponDamage`) kept so the rarity branch picks different tables.
    fn weapon_config() -> BonusSettings {
        BonusSettings {
            rarity_weight: [("Common".to_owned(), 5.0), ("Rare".to_owned(), 1.0)]
                .into_iter()
                .collect(),
            bonus_type_weight: [
                ("WeaponSpread".to_owned(), 1.0),
                ("MalfunctionProtections".to_owned(), 1.0),
            ]
            .into_iter()
            .collect(),
            common: [
                ("WeaponSpread".to_owned(), bonus(0.9, 0.99)),
                ("MalfunctionProtections".to_owned(), bonus(0.94, 0.96)),
            ]
            .into_iter()
            .collect(),
            rare: [
                ("WeaponSpread".to_owned(), bonus(0.8, 0.9)),
                ("MalfunctionProtections".to_owned(), bonus(0.75, 0.9)),
                ("WeaponDamage".to_owned(), bonus(0.3, 0.6)),
            ]
            .into_iter()
            .collect(),
        }
    }

    fn weapon_root() -> Item {
        Item {
            id: "weapon-root".to_owned(),
            template: "5447a9cd4bdc2dbd208b4567".to_owned(),
            upd: Some(Upd {
                repairable: Some(UpdRepairable {
                    durability: Some(87.0),
                    max_durability: Some(100.0),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    #[test]
    fn add_buff_writes_the_pinned_buff_for_three_consecutive_rolls() {
        let config = weapon_config();
        let _guard = TestSeedGuard::install(SEED);

        let rolled: Vec<(String, RepairBuffType, f64, f64)> = (0..3)
            .map(|_| {
                let mut item = weapon_root();
                add_buff(&config, &mut item).unwrap();
                let buff = item.upd.unwrap().buff.unwrap();

                (
                    buff.rarity.unwrap(),
                    buff.buff_type.unwrap(),
                    buff.value.unwrap(),
                    buff.threshold_durability.unwrap(),
                )
            })
            .collect();

        assert_eq!(
            rolled,
            vec![
                (
                    "Common".to_owned(),
                    RepairBuffType::WeaponSpread,
                    0.929_248_662_612_869,
                    74.0
                ),
                (
                    "Rare".to_owned(),
                    RepairBuffType::WeaponSpread,
                    0.833_015_980_708_086_8,
                    67.0
                ),
                (
                    "Common".to_owned(),
                    RepairBuffType::MalfunctionProtections,
                    0.944_625_258_521_290_3,
                    76.0
                ),
            ]
        );
    }

    #[test]
    fn a_single_entry_weight_map_consumes_no_draw() {
        let mut config = weapon_config();
        config.rarity_weight = [("Rare".to_owned(), 1.0)].into_iter().collect();
        config.bonus_type_weight = [("WeaponDamage".to_owned(), 1.0)].into_iter().collect();

        // Both weight maps shortcut, so the value roll must be the *first* draw of the stream.
        let first_draw_of_the_stream = {
            let _guard = TestSeedGuard::install(SEED);

            get_double(0.3, 0.6)
        };

        let _guard = TestSeedGuard::install(SEED);
        let mut item = weapon_root();
        add_buff(&config, &mut item).unwrap();
        let buff = item.upd.unwrap().buff.unwrap();

        assert_eq!(buff.rarity.as_deref(), Some("Rare"));
        assert_eq!(buff.buff_type, Some(RepairBuffType::WeaponDamage));
        assert_eq!(buff.value, Some(first_draw_of_the_stream));
        assert_eq!(buff.threshold_durability, Some(76.0));
    }

    #[test]
    fn a_bonus_type_missing_from_the_chosen_rarity_is_an_error() {
        let mut config = weapon_config();
        config.rarity_weight = [("Common".to_owned(), 1.0)].into_iter().collect();
        config.bonus_type_weight = [("WeaponDamage".to_owned(), 1.0)].into_iter().collect();
        let _guard = TestSeedGuard::install(SEED);

        let mut item = weapon_root();
        let error = add_buff(&config, &mut item).unwrap_err();

        assert_eq!(
            error.message,
            "The given key 'WeaponDamage' was not present in the dictionary."
        );
    }

    #[test]
    fn an_item_without_a_repairable_upd_is_an_error() {
        let config = weapon_config();
        let _guard = TestSeedGuard::install(SEED);

        let mut item = weapon_root();
        item.upd = Some(Upd::default());
        let error = add_buff(&config, &mut item).unwrap_err();

        assert_eq!(
            error.message,
            "Item has no Upd.Repairable.Durability to threshold against"
        );
    }

    #[test]
    fn the_buff_serializes_under_the_c_sharp_wire_names() {
        let config = weapon_config();
        let _guard = TestSeedGuard::install(SEED);

        let mut item = weapon_root();
        add_buff(&config, &mut item).unwrap();
        let upd = serde_json::to_value(item.upd.unwrap()).unwrap();

        assert_eq!(upd["Buff"]["Rarity"], "Common");
        assert_eq!(upd["Buff"]["BuffType"], "WeaponSpread");
        assert_eq!(upd["Buff"]["ThresholdDurability"], 74.0);
        assert_eq!(upd["Repairable"]["Durability"], 87.0);
    }
}
