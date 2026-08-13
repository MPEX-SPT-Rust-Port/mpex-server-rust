//! `Generators/Loot/LootGenerator.cs` — the reward-loot generator, ported method for method:
//! randomised loot (airdrops, reward containers) and forced loot.
//!
//! Same conventions as `location_loot_generator`: the C# logs through `ISptLogger` and localises
//! through `ServerLocalisationService`, both of which come out of here as [`Diagnostic`]s for the
//! caller to replay, and where the C# throws (or dereferences a null and crashes) the port returns a
//! [`LootError`] rather than panicking behind the FFI boundary, naming the C# line it stands in for.

use std::collections::HashSet;

use indexmap::IndexMap;
use serde_json::json;

use super::item_helper::{self, LootError};
use super::models::{
    CreateForcedLootRequest, CreateRandomLootRequest, DEBUG, Diagnostic, ERROR, Item, ItemView,
    LootRequestView, MinMaxI32, PresetView, RandomLootContainerRequest, RewardDetailsView,
    RewardLootDb, RewardLootResult, SealedContainerSettingsView, SealedWeaponCaseRequest, Upd,
    WARNING,
};
use super::{mongo_id, random_util};

/// `LootGenerator.cs:715-724` — a spawn limit and how much of it has been used.
#[derive(Debug, Clone, Copy)]
struct ItemLimit {
    current: i32,
    max: i32,
}

/// `LootGenerator.ItemRewardPoolResults` (`:707-712`). The pool is `(tpl, view)` pairs in items-view
/// order — `get_array_value` indexes into it, so the order is observable.
struct ItemRewardPoolResults<'a> {
    item_pool: Vec<(&'a str, &'a ItemView)>,
    blacklist: HashSet<String>,
}

/// A plain interpolated log line.
fn diagnostic(level: &str, message: String) -> Diagnostic {
    Diagnostic {
        level: level.to_owned(),
        locale_key: None,
        args: None,
        message: Some(message),
    }
}

/// A `ServerLocalisationService.GetText` line: the key plus the arguments the C# passes with it (a
/// bare value for the `%s` keys, `None` for the keys C# calls the single-argument overload with).
fn localised(level: &str, locale_key: &str, args: Option<serde_json::Value>) -> Diagnostic {
    Diagnostic {
        level: level.to_owned(),
        locale_key: Some(locale_key.to_owned()),
        args,
        message: None,
    }
}

/// `randomUtil.GetInt(x.Min, x.Max)` where `x` is one of `LootRequest`'s nullable `MinMax<int>`
/// properties: a null one is C#'s `NullReferenceException`, and an absent member inside a present
/// object is C#'s non-nullable `int` default of 0.
fn get_int_of(min_max: Option<&MinMaxI32>, member: &str) -> Result<i32, LootError> {
    let min_max = min_max.ok_or_else(|| {
        LootError::new(format!("LootRequest.{member} is null when reading a count"))
    })?;

    Ok(random_util::get_int(
        min_max.min.unwrap_or(0),
        min_max.max.unwrap_or(0),
    ))
}

/// The `new Item { Upd = new Upd { StackObjectsCount = n, SpawnedInSession = true } }` literal the
/// add-sites share (`:62-70,177-182,325-330`). `SpawnedInSession` is untyped on [`Upd`], so it rides
/// in the passthrough map the same way `item_helper::set_found_in_raid` writes it.
fn new_loot_item(tpl: &str, stack_objects_count: f64) -> Item {
    Item {
        id: mongo_id::generate(),
        template: tpl.to_owned(),
        upd: Some(Upd {
            stack_objects_count: Some(stack_objects_count),
            extra: [("SpawnedInSession".to_owned(), serde_json::Value::Bool(true))]
                .into_iter()
                .collect(),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// `LootGenerator.CreateRandomLoot` (`:45-142`) — sealed weapon crates, then plain items, then
/// weapon presets, then armor presets.
///
/// **Bug-for-bug: the three add loops retry forever.** C# decrements the loop index on a failed add
/// (`:92-94,113-115,133-135`), so a pool that can only ever be rejected — every item an armor, say —
/// spins the loop until the process dies. Ported as written; the retry consumes draws each pass, and
/// the draw count is what the parity test compares.
pub fn create_random_loot(request: CreateRandomLootRequest) -> Result<RewardLootResult, LootError> {
    let _seed_guard = request
        .db
        .test_seed
        .map(random_util::TestSeedGuard::install);

    let db = &request.db;
    let options = &request.loot_request;
    let mut result: Vec<Vec<Item>> = Vec::new();
    let mut diagnostics: Vec<Diagnostic> = Vec::new();

    // `InitItemLimitCounter(options.ItemLimits)` (`:49`) enumerates the dictionary, so a null one
    // throws before anything else happens.
    let item_limits = options
        .item_limits
        .as_ref()
        .ok_or_else(|| LootError::new("LootRequest.ItemLimits is null"))?;
    let mut item_type_counts = init_item_limit_counter(item_limits);

    // Handle sealed weapon containers
    let sealed_weapon_crate_count =
        get_int_of(options.weapon_crate_count.as_ref(), "WeaponCrateCount")?;
    if sealed_weapon_crate_count > 0 {
        // Get list of all sealed containers from db - they're all the same, just for flavor.
        let mut sealed_weapon_container_pool: Vec<&str> = Vec::new();
        for (tpl, item) in &db.items_view {
            // `item.Name.Contains(...)` (`:57`) dereferences a null `_name` and throws.
            let name = item.name.as_deref().ok_or_else(|| {
                LootError::new(format!(
                    "Item: {tpl} has no name when filtering sealed crates"
                ))
            })?;

            if name.contains("event_container_airdrop") {
                sealed_weapon_container_pool.push(tpl.as_str());
            }
        }

        if sealed_weapon_container_pool.is_empty() {
            // The pool is a lazy `Where`, so `GetRandomElement` copies it with `ToList()`
            // (`RandomUtil.cs:172-173`), draws `GetInt(0, -1)` — which short-circuits to 0 without
            // consuming any randomness — and indexes the empty list:
            // `ArgumentOutOfRangeException` (`:62`).
            return Err(LootError::new(
                "No sealed weapon containers found in the item db",
            ));
        }

        for _ in 0..sealed_weapon_crate_count {
            // Choose one at random + add to results array
            let chosen_sealed_container =
                *random_util::get_array_value(&sealed_weapon_container_pool);
            result.push(vec![new_loot_item(chosen_sealed_container, 1.0)]);
        }
    }

    // Get items from items.json that have a type of item + not in global blacklist + base type is in
    // whitelist
    let reward_pool_results = get_item_reward_pool(
        db,
        // Both are spread into a collection expression / probed with `Contains` the moment the pool
        // is built, so a null one is the C# throw path (`:217,247`).
        options
            .item_blacklist
            .as_ref()
            .ok_or_else(|| LootError::new("LootRequest.ItemBlacklist is null"))?,
        options
            .item_type_whitelist
            .as_ref()
            .ok_or_else(|| LootError::new("LootRequest.ItemTypeWhitelist is null"))?,
        options.use_reward_item_blacklist.unwrap_or(false),
        options.allow_boss_items.unwrap_or(false),
        options.block_seasonal_items_out_of_season.unwrap_or(false),
    );

    // Pool has items we could add as loot, proceed
    if !reward_pool_results.item_pool.is_empty() {
        let randomised_item_count = get_int_of(options.item_count.as_ref(), "ItemCount")?;
        let mut index = 0;
        while index < randomised_item_count {
            // Failed to add, reduce index so we get another attempt
            if find_and_add_random_item_to_loot(
                db,
                &reward_pool_results.item_pool,
                &mut item_type_counts,
                options,
                &mut result,
            )? {
                index += 1;
            }
        }
    }

    let global_default_presets = &db.default_presets;

    // Filter default presets to just weapons. The count is drawn before the pool is built (`:100`),
    // so it is consumed even when no preset survives the filter.
    let randomised_weapon_preset_count =
        get_int_of(options.weapon_preset_count.as_ref(), "WeaponPresetCount")?;
    if randomised_weapon_preset_count > 0 {
        let weapon_default_presets = filter_presets(global_default_presets, |encyclopedia| {
            Ok(item_helper::is_of_baseclass(
                &db.items_view,
                encyclopedia,
                item_helper::WEAPON,
            ))
        })?;

        if !weapon_default_presets.is_empty() {
            let mut index = 0;
            while index < randomised_weapon_preset_count {
                // Failed to add, reduce index so we get another attempt
                if find_and_add_random_preset_to_loot(
                    db,
                    &weapon_default_presets,
                    &mut item_type_counts,
                    &reward_pool_results.blacklist,
                    &mut result,
                    &mut diagnostics,
                )? {
                    index += 1;
                }
            }
        }
    }

    // Filter default presets to just armors and then filter again by protection level
    let randomised_armor_preset_count =
        get_int_of(options.armor_preset_count.as_ref(), "ArmorPresetCount")?;
    if randomised_armor_preset_count > 0 {
        let armor_default_presets = filter_presets(global_default_presets, |encyclopedia| {
            Ok(item_helper::armor_item_can_hold_mods(
                &db.items_view,
                encyclopedia,
            ))
        })?;

        // Both C# filters are lazy, but `GetRandomElement` copies the sequence with `ToList()`
        // (`RandomUtil.cs:173`) on the first draw that follows the `Any()` check, so every preset is
        // run through the predicate either way — filtering eagerly here changes nothing observable.
        // Neither predicate draws, so the RNG stream is identical; the one difference is which error
        // surfaces first when both stages would throw (a preset without an `_encyclopedia` *after*
        // one with a null armor class reports the encyclopedia here, the armor class in C#).
        let mut level_filtered_armor_presets = Vec::new();
        for preset in armor_default_presets {
            if is_armor_of_desired_protection_level(db, preset, options)? {
                level_filtered_armor_presets.push(preset);
            }
        }

        // Add some armors to rewards
        if !level_filtered_armor_presets.is_empty() {
            let mut index = 0;
            while index < randomised_armor_preset_count {
                // Failed to add, reduce index so we get another attempt
                if find_and_add_random_preset_to_loot(
                    db,
                    &level_filtered_armor_presets,
                    &mut item_type_counts,
                    &reward_pool_results.blacklist,
                    &mut result,
                    &mut diagnostics,
                )? {
                    index += 1;
                }
            }
        }
    }

    Ok(RewardLootResult {
        items: result,
        diagnostics,
    })
}

/// The `globalDefaultPresets.Where(preset => ...(preset.Encyclopedia.Value...))` filters
/// (`:102-104,124`).
///
/// **`Err` is the C# throw path**: `Encyclopedia` is a `MongoId?` and `.Value` on a null one throws
/// `InvalidOperationException`, so a default preset without an `_encyclopedia` takes down the whole
/// call rather than reaching the null-encyclopedia branch inside
/// [`find_and_add_random_preset_to_loot`] — which is why that branch is unreachable from here.
fn filter_presets(
    presets: &[PresetView],
    mut predicate: impl FnMut(&str) -> Result<bool, LootError>,
) -> Result<Vec<&PresetView>, LootError> {
    let mut filtered = Vec::new();

    for preset in presets {
        let encyclopedia = preset.encyclopedia.as_deref().ok_or_else(|| {
            LootError::new(format!(
                "Default preset: {} lacks an encyclopedia value",
                preset.id.as_deref().unwrap_or_default()
            ))
        })?;

        if predicate(encyclopedia)? {
            filtered.push(preset);
        }
    }

    Ok(filtered)
}

/// `LootGenerator.CreateForcedLoot` (`:150-196`) — a count per entry, presets cloned whole and
/// everything else split into stacks.
pub fn create_forced_loot(request: CreateForcedLootRequest) -> Result<RewardLootResult, LootError> {
    let _seed_guard = request
        .db
        .test_seed
        .map(random_util::TestSeedGuard::install);

    let db = &request.db;
    let mut result: Vec<Vec<Item>> = Vec::new();

    for (item_tpl, details) in &request.forced_loot {
        // How many of this item we want. The dictionary value is a non-nullable `MinMax<int>`, so
        // only its members can be absent, and those default to 0.
        let randomised_item_count =
            random_util::get_int(details.min.unwrap_or(0), details.max.unwrap_or(0));

        // Check if item being added has a preset and use that instead. C# probes the dictionary
        // twice (`ContainsKey` then `TryGetValue`, `:160-163`); harmless, and neither draws.
        if let Some(preset) = db.default_presets_by_tpl.get(item_tpl) {
            // Add the chosen preset as many times as randomisedItemCount states
            for _ in 0..randomised_item_count {
                // Clone preset and alter Ids to be unique
                let mut preset_with_unique_ids_clone = preset.items.clone();
                item_helper::replace_ids(&mut preset_with_unique_ids_clone);

                // Add to results
                result.push(preset_with_unique_ids_clone);
            }

            continue;
        }

        // Non-preset item to be added
        let new_loot_item = new_loot_item(item_tpl, f64::from(randomised_item_count));
        for split_item in item_helper::split_stack(&db.items_view, &new_loot_item)? {
            // Add as separate lists
            result.push(vec![split_item]);
        }
    }

    Ok(RewardLootResult {
        items: result,
        diagnostics: Vec::new(),
    })
}

/// `LootGenerator.GetItemRewardPool` (`:207-251`) — the blacklist union, then the pool that survives
/// it.
fn get_item_reward_pool<'a>(
    db: &'a RewardLootDb,
    item_tpl_blacklist: &HashSet<String>,
    item_type_whitelist: &HashSet<String>,
    use_reward_item_blacklist: bool,
    allow_boss_items: bool,
    block_seasonal_items_out_of_season: bool,
) -> ItemRewardPoolResults<'a> {
    // `GetBlacklistedItems()` (`:425`) — the config list, not the cache the sealed filters read
    let mut item_blacklist: HashSet<String> = db.config_blacklist.clone();
    item_blacklist.extend(item_tpl_blacklist.iter().cloned());

    if use_reward_item_blacklist {
        let item_type_blacklist: Vec<&str> = db
            .reward_base_type_blacklist
            .iter()
            .map(String::as_str)
            .collect();

        // Get all items that match the blacklisted types and fold into item blacklist. Note the walk
        // starts at the item's *parent*, as the C# does, so an item whose own parent is a listed base
        // class is only caught through its grandparent.
        let items_matching_type_blacklist: Vec<String> = db
            .items_view
            .iter()
            .filter_map(|(tpl, template_item)| {
                // Ignore items without parents
                let parent = template_item.parent.as_deref().filter(|p| !p.is_empty())?;

                item_helper::is_of_baseclasses(&db.items_view, parent, &item_type_blacklist)
                    .then(|| tpl.clone())
            })
            .collect();

        item_blacklist.extend(db.reward_item_blacklist.iter().cloned());
        item_blacklist.extend(items_matching_type_blacklist);
    }

    if !allow_boss_items {
        item_blacklist.extend(db.boss_items.iter().cloned());
    }

    if block_seasonal_items_out_of_season {
        item_blacklist.extend(db.inactive_seasonal_items.iter().cloned());
    }

    let item_pool = db
        .items_view
        .iter()
        .filter(|(tpl, item)| {
            !item_blacklist.contains(tpl.as_str())
                // `string.Equals(item.Type, "item", OrdinalIgnoreCase)`; every `_type` in the db is
                // ASCII, so an ASCII-insensitive compare answers identically.
                && item
                    .item_type
                    .as_deref()
                    .is_some_and(|item_type| item_type.eq_ignore_ascii_case("item"))
                && !item.quest_item.unwrap_or(false)
                // `TemplateItem.Parent` is a non-nullable `MongoId`, so a missing one is the empty
                // id rather than a null the whitelist would throw on.
                && item_type_whitelist.contains(item.parent.as_deref().unwrap_or_default())
        })
        .map(|(tpl, item)| (tpl.as_str(), item))
        .collect();

    ItemRewardPoolResults {
        item_pool,
        blacklist: item_blacklist,
    }
}

/// `LootGenerator.IsArmorOfDesiredProtectionLevel` (`:259-277`) — the **first** slot of the three
/// that the preset has decides the answer; the rest are never looked at.
///
/// **`Err` is the C# throw path**: `GetItem(...).Value.Properties` on an item missing from the db,
/// and `ArmorClass.Value` on a null armor class, both crash there.
fn is_armor_of_desired_protection_level(
    db: &RewardLootDb,
    armor: &PresetView,
    options: &LootRequestView,
) -> Result<bool, LootError> {
    const RELEVANT_SLOTS: [&str; 3] = ["front_plate", "helmet_top", "soft_armor_front"];

    for slot_id in RELEVANT_SLOTS {
        let Some(armor_item) = armor
            .items
            .iter()
            .find(|item| item.slot_id.as_deref() == Some(slot_id))
        else {
            continue;
        };

        let armor_details = item_helper::get_item(&db.items_view, &armor_item.template)
            .ok_or_else(|| {
                LootError::new(format!(
                    "Preset armor item: {} is missing from the item db",
                    armor_item.template
                ))
            })?;
        let armor_class = armor_details.armor_class.ok_or_else(|| {
            LootError::new(format!(
                "Preset armor item: {} has no armor class",
                armor_item.template
            ))
        })?;
        let armor_level_whitelist = options
            .armor_level_whitelist
            .as_ref()
            .ok_or_else(|| LootError::new("LootRequest.ArmorLevelWhitelist is null"))?;

        return Ok(armor_level_whitelist.contains(&armor_class));
    }

    Ok(false)
}

/// `LootGenerator.InitItemLimitCounter` (`:284-293`).
fn init_item_limit_counter(limits: &IndexMap<String, i32>) -> IndexMap<String, ItemLimit> {
    limits
        .iter()
        .map(|(item_type_id, max)| {
            (
                item_type_id.clone(),
                ItemLimit {
                    current: 0,
                    max: *max,
                },
            )
        })
        .collect()
}

/// `LootGenerator.FindAndAddRandomItemToLoot` (`:303-348`).
fn find_and_add_random_item_to_loot(
    db: &RewardLootDb,
    items: &[(&str, &ItemView)],
    item_type_counts: &mut IndexMap<String, ItemLimit>,
    options: &LootRequestView,
    result: &mut Vec<Vec<Item>>,
) -> Result<bool, LootError> {
    if items.is_empty() {
        // The pool is a lazy `Where`, so `GetRandomElement` copies it with `ToList()`
        // (`RandomUtil.cs:172-173`), draws `GetInt(0, -1)` — no randomness consumed, it
        // short-circuits to 0 — and indexes the empty list: `ArgumentOutOfRangeException` (`:310`).
        // The only C# caller checks `Any()` first, so this is unreachable from `create_random_loot`.
        return Err(LootError::new("Item reward pool is empty"));
    }

    let (random_item_tpl, random_item) = *random_util::get_array_value(items);
    let random_item_parent = random_item.parent.as_deref().unwrap_or_default();

    // Dead guard, replicated as written (`:312-316`): `itemLimitCount` is false exactly when
    // `randomItemLimitCount` is null, and the lifted `null > null` comparison is false, so the two
    // halves can never both hold and this never rejects anything.
    let random_item_limit_count = item_type_counts.get(random_item_parent).copied();
    let item_limit_count = random_item_limit_count.is_some();
    if !item_limit_count && random_item_limit_count.is_some_and(|limit| limit.current > limit.max) {
        return Ok(false);
    }

    // Skip armors as they need to come from presets
    if item_helper::armor_item_can_hold_mods(&db.items_view, random_item_tpl) {
        return Ok(false);
    }

    // Special case - handle items that need a stackcount > 1. C# mints the item's `MongoId` before
    // drawing the stack count (`:324-335`) and this draws first; both orders consume the same
    // randomness, since ids are not drawn from the seeded stream.
    let stack_objects_count = if random_item.stack_max_size.is_some_and(|size| size > 1) {
        f64::from(get_randomised_stack_count(
            random_item,
            random_item_tpl,
            options,
        )?)
    } else {
        1.0
    };

    result.push(vec![new_loot_item(random_item_tpl, stack_objects_count)]);

    // Increment item count as it's in limit array
    if let Some(limit) = item_type_counts.get_mut(random_item_parent) {
        limit.current += 1;
    }

    // Item added okay
    Ok(true)
}

/// `LootGenerator.GetRandomisedStackCount` (`:356-368`).
fn get_randomised_stack_count(
    item: &ItemView,
    tpl: &str,
    options: &LootRequestView,
) -> Result<i32, LootError> {
    let mut min = item.stack_min_random;
    let mut max = item.stack_max_size;

    // `options.ItemStackLimits.TryGetValue` on a null dictionary is the C# throw path.
    let item_stack_limits = options
        .item_stack_limits
        .as_ref()
        .ok_or_else(|| LootError::new("LootRequest.ItemStackLimits is null"))?;

    if let Some(item_limits) = item_stack_limits.get(tpl) {
        min = item_limits.min;
        max = item_limits.max;
    }

    Ok(random_util::get_int(min.unwrap_or(1), max.unwrap_or(1)))
}

/// `LootGenerator.FindAndAddRandomPresetToLoot` (`:378-455`).
fn find_and_add_random_preset_to_loot(
    db: &RewardLootDb,
    preset_pool: &[&PresetView],
    item_type_counts: &mut IndexMap<String, ItemLimit>,
    item_blacklist: &HashSet<String>,
    result: &mut Vec<Vec<Item>>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<bool, LootError> {
    if preset_pool.is_empty() {
        diagnostics.push(localised(WARNING, "loot-preset_pool_is_empty", None));

        return Ok(false);
    }

    // Choose random preset and get details from item db using encyclopedia value (encyclopedia ===
    // tplId)
    let chosen_preset = *random_util::get_array_value(preset_pool);

    // No `_encyclopedia` property, not possible to reliably get root item tpl
    let Some(encyclopedia) = chosen_preset.encyclopedia.as_deref() else {
        // Deviation: C# gates this `logger.Warning` behind `IsLogEnabled(Debug)` (`:398-401`). The
        // `Diagnostic` schema has no "gated" concept, so it goes out at level `debug` — replay keeps
        // the visibility condition at the cost of the level tag. Item output is unaffected.
        diagnostics.push(localised(
            DEBUG,
            "loot-chosen_preset_missing_encyclopedia_value",
            Some(json!(chosen_preset.id)),
        ));

        return Ok(false);
    };

    // Get preset root item db details via its `_encyclopedia` property
    let Some(item_db_details) = item_helper::get_item(&db.items_view, encyclopedia) else {
        // Deviation: debug-gated in C# (`:412-415`), see above. The stray `$` is C#'s, not a typo.
        diagnostics.push(diagnostic(
            DEBUG,
            format!("$Unable to find preset with tpl: {encyclopedia}, skipping"),
        ));

        return Ok(false);
    };

    // Skip preset if root item is blacklisted. `FirstOrDefault().Template` (`:419`) dereferences a
    // null on a preset with no items and throws.
    let root_tpl = chosen_preset
        .items
        .first()
        .map(|item| item.template.as_str())
        .ok_or_else(|| {
            LootError::new(format!(
                "Preset: {} has no items",
                chosen_preset.id.as_deref().unwrap_or_default()
            ))
        })?;
    if item_blacklist.contains(root_tpl) {
        return Ok(false);
    }

    // Some custom mod items lack a parent property.
    //
    // Note: the C# guard (`:425-430`) is dead — `TemplateItem.Parent` is a non-nullable `MongoId`
    // and the item was just found in the db, so `Value?.Parent is null` never holds. An **empty**
    // `_parent` is therefore not the error branch there: it proceeds and keys the limit lookup under
    // the empty id, which is what this does. Only an entirely absent `parent` — impossible from a C#
    // `MongoId`, but expressible on `ItemView` — takes the error branch.
    let Some(parent) = item_db_details.parent.as_deref() else {
        diagnostics.push(localised(
            ERROR,
            "loot-item_missing_parentid",
            Some(json!(item_db_details.name)),
        ));

        return Ok(false);
    };

    // Check chosen preset hasn't exceeded spawn limit. Second dead guard, replicated as written
    // (`:433-437`) — see `find_and_add_random_item_to_loot`.
    let item_limit_count = item_type_counts.get(parent).copied();
    let has_item_limit_count = item_limit_count.is_some();
    if !has_item_limit_count && item_limit_count.is_some_and(|limit| limit.current > limit.max) {
        return Ok(false);
    }

    let mut preset_and_mods_clone = chosen_preset.items.clone();
    item_helper::replace_ids(&mut preset_and_mods_clone);
    item_helper::remap_root_item_id(&mut preset_and_mods_clone);

    item_helper::set_found_in_raid(&db.items_view, &mut preset_and_mods_clone);

    // Add chosen preset tpl to result array
    result.push(preset_and_mods_clone);

    // Increment item count as item has been chosen and its inside itemLimitCount dictionary
    if let Some(limit) = item_type_counts.get_mut(parent) {
        limit.current += 1;
    }

    // Item added okay
    Ok(true)
}

/// `randomUtil.GetArrayValue` over a pool the C# builds inline. An empty one is the
/// `ArgumentOutOfRangeException` path described at the top of [`create_random_loot`]: `ToList()`,
/// a `GetInt(0, -1)` that short-circuits to 0 without consuming randomness, then an index into an
/// empty list.
fn get_array_value_of<'a, T>(pool: &'a [T], description: &str) -> Result<&'a T, LootError> {
    if pool.is_empty() {
        return Err(LootError::new(format!("{description} is empty")));
    }

    Ok(random_util::get_array_value(pool))
}

/// `randomUtil.GetInt(settings.Min, settings.Max)` over a non-nullable `MinMax<int>` dictionary
/// value (`:522,617`): only the members can be absent, and those default to 0.
fn get_int_of_minmax(min_max: &MinMaxI32) -> i32 {
    random_util::get_int(min_max.min.unwrap_or(0), min_max.max.unwrap_or(0))
}

/// The `new List<Item> { new() { Id = new MongoId(), Template = tpl } }` literal the sealed-container
/// and reward-container add-sites share (`:554-557,588-591,643-646,683`) — no `Upd` on any of them.
fn new_reward_item(tpl: &str) -> Item {
    Item {
        id: mongo_id::generate(),
        template: tpl.to_owned(),
        ..Default::default()
    }
}

/// `LootGenerator.GetSealedWeaponCaseLoot` (`:462-505`) — a weapon preset, the mods that fit it,
/// then the non-weapon-mod reward types.
///
/// Nothing here is marked found-in-raid: `SealedAirdropContainerSettings.FoundInRaid` is applied by
/// the C# caller after the call, not by this method.
pub fn get_sealed_weapon_case_loot(
    request: SealedWeaponCaseRequest,
) -> Result<RewardLootResult, LootError> {
    let _seed_guard = request
        .db
        .test_seed
        .map(random_util::TestSeedGuard::install);

    let db = &request.db;
    let settings = &request.container_settings;
    let mut items_to_return: Vec<Vec<Item>> = Vec::new();
    let mut diagnostics: Vec<Diagnostic> = Vec::new();

    // Choose a weapon to give to the player (weighted)
    let chosen_weapon_tpl = random_util::get_weighted_value(&settings.weapon_reward_weight)?;

    // Get itemDb details of weapon
    let Some(weapon_details_db) = item_helper::get_item(&db.items_view, &chosen_weapon_tpl) else {
        diagnostics.push(localised(
            ERROR,
            "loot-non_item_picked_as_sealed_weapon_crate_reward",
            Some(json!(chosen_weapon_tpl)),
        ));

        return Ok(RewardLootResult {
            items: items_to_return,
            diagnostics,
        });
    };

    // `presetHelper.GetPresets(tpl)` returns an empty list for a tpl with no presets
    // (`PresetHelper.cs:182-186`), which is what an absent key is here.
    let presets_for_weapon: &[PresetView] = request
        .presets_by_tpl
        .get(&chosen_weapon_tpl)
        .map_or(&[], Vec::as_slice);

    // Get weapon preset - default or choose a random one from globals.json preset pool
    let chosen_weapon_preset = if settings.default_presets_only {
        db.default_presets_by_tpl.get(&chosen_weapon_tpl)
    } else {
        Some(get_array_value_of(
            presets_for_weapon,
            &format!("Preset pool for weapon: {chosen_weapon_tpl}"),
        )?)
    };

    // No default preset found for weapon, choose a random one. Only reachable from the branch
    // above: `GetArrayValue` never hands back a null.
    let chosen_weapon_preset = match chosen_weapon_preset {
        Some(preset) => preset,
        None => {
            diagnostics.push(localised(
                WARNING,
                "loot-default_preset_not_found_using_random",
                Some(json!(chosen_weapon_tpl)),
            ));

            get_array_value_of(
                presets_for_weapon,
                &format!("Preset pool for weapon: {chosen_weapon_tpl}"),
            )?
        }
    };

    // Clean up Ids to ensure they're all unique and prevent collisions
    let mut preset_and_mods_clone = chosen_weapon_preset.items.clone();
    item_helper::replace_ids(&mut preset_and_mods_clone);
    item_helper::remap_root_item_id(&mut preset_and_mods_clone);

    // Add preset to return object
    items_to_return.push(preset_and_mods_clone);

    // Get a random collection of weapon mods related to chosen weapon and add them to result array.
    // An absent key is the empty list `GetLinkedDbItems` returns for an unlinked tpl, which lands on
    // the same skip branch the C# null check does.
    let linked_items_to_weapon: &[String] = request
        .linked_items
        .get(&chosen_weapon_tpl)
        .map_or(&[], Vec::as_slice);
    items_to_return.extend(get_sealed_container_weapon_mod_rewards(
        db,
        settings,
        linked_items_to_weapon,
        chosen_weapon_preset,
        &mut diagnostics,
    ));

    // Handle non-weapon mod reward types
    items_to_return.extend(get_sealed_container_non_weapon_mod_rewards(
        db,
        settings,
        weapon_details_db,
        &mut diagnostics,
    )?);

    Ok(RewardLootResult {
        items: items_to_return,
        diagnostics,
    })
}

/// `LootGenerator.GetSealedContainerNonWeaponModRewards` (`:513-598`).
fn get_sealed_container_non_weapon_mod_rewards(
    db: &RewardLootDb,
    settings: &SealedContainerSettingsView,
    weapon_details_db: &ItemView,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<Vec<Vec<Item>>, LootError> {
    let mut rewards: Vec<Vec<Item>> = Vec::new();

    for (reward_key, limits) in &settings.reward_type_limits {
        let reward_count = get_int_of_minmax(limits);
        if reward_count == 0 {
            continue;
        }

        // Edge case - ammo boxes
        if reward_key == item_helper::AMMO_BOX {
            // Need to find boxes that matches weapons caliber
            let weapon_caliber = weapon_details_db.ammo_caliber.as_deref();
            let mut ammo_boxes_matching_caliber: Vec<&str> = Vec::new();
            for tpl in &settings.ammo_box_whitelist {
                // A whitelisted tpl the db has no item for leaves a null in the C# list, and the
                // caliber filter dereferences `.Properties` on it (`:540`). Deviation: C# only
                // reaches that null when the lazy chain is enumerated past it, so a match earlier in
                // the whitelist can defer the crash to the `GetArrayValue` that follows — and a
                // negative reward count would skip it entirely. Filtering eagerly here reports it up
                // front; no draw separates the two points, so nothing else shifts.
                let details = item_helper::get_item(&db.items_view, tpl).ok_or_else(|| {
                    LootError::new(format!(
                        "Whitelisted ammo box: {tpl} is missing from the item db"
                    ))
                })?;

                if details.ammo_caliber.as_deref() == weapon_caliber {
                    ammo_boxes_matching_caliber.push(tpl.as_str());
                }
            }

            if ammo_boxes_matching_caliber.is_empty() {
                diagnostics.push(diagnostic(
                    DEBUG,
                    format!(
                        "No ammo box with caliber {} found, skipping",
                        weapon_caliber.unwrap_or_default()
                    ),
                ));

                continue;
            }

            for _ in 0..reward_count {
                let chosen_ammo_box = *random_util::get_array_value(&ammo_boxes_matching_caliber);
                let mut ammo_box_reward = vec![new_reward_item(chosen_ammo_box)];
                // The three failure cases are all C# crash paths (a box naming no cartridge
                // dereferences `cartridgeTpl!`, an empty box list indexes `ammoBox[0]`, a cartridge
                // without a stack size loops forever), so they end the call rather than log.
                item_helper::add_cartridges_to_ammo_box(
                    &db.items_view,
                    &mut ammo_box_reward,
                    chosen_ammo_box,
                )
                .map_err(|failure| {
                    LootError::new(failure.message.unwrap_or_else(|| {
                        format!("Unable to add cartridges to ammo box: {chosen_ammo_box}")
                    }))
                })?;

                rewards.push(ammo_box_reward);
            }

            continue;
        }

        // Get all items of the desired type + not quest items + not globally blacklisted.
        //
        // **Bug-for-bug, three of them.** The comment is the C#'s; the code is not what it says.
        // `IsItemBlacklisted` is not negated (`:569`), so the pool is the blacklisted items and
        // nothing else. `!(AllowBossItems || IsBossItem(id))` (`:570`) is false for every item once
        // `AllowBossItems` is set, emptying the pool outright rather than admitting boss items. And
        // `QuestItem is null` (`:571`) rejects an explicit `false` as well as a `true`. With real
        // configs the three together leave this pool empty and the branch just logs the skip.
        //
        // The blacklist is [`RewardLootDb::global_blacklist`] - the cache `IsItemBlacklisted` reads,
        // not the config list [`RewardLootDb::config_blacklist`] the reward pool unions in.
        let reward_item_pool: Vec<&str> = db
            .items_view
            .iter()
            .filter(|(tpl, item)| {
                // `TemplateItem.Parent` is a non-nullable `MongoId`, so a missing one is the empty
                // id and only matches an empty reward key.
                item.parent.as_deref().unwrap_or_default() == reward_key
                    && item
                        .item_type
                        .as_deref()
                        .is_some_and(|item_type| item_type.eq_ignore_ascii_case("item"))
                    && db.global_blacklist.contains(tpl.as_str())
                    && !(settings.allow_boss_items || db.boss_items.contains(tpl.as_str()))
                    && item.quest_item.is_none()
            })
            .map(|(tpl, _)| tpl.as_str())
            .collect();

        if reward_item_pool.is_empty() {
            diagnostics.push(diagnostic(
                DEBUG,
                format!("No items with base type of {reward_key} found, skipping"),
            ));

            continue;
        }

        for _ in 0..reward_count {
            // Choose a random item from pool
            let chosen_reward_item = *random_util::get_array_value(&reward_item_pool);

            rewards.push(vec![new_reward_item(chosen_reward_item)]);
        }
    }

    Ok(rewards)
}

/// `LootGenerator.GetSealedContainerWeaponModRewards` (`:607-653`).
fn get_sealed_container_weapon_mod_rewards(
    db: &RewardLootDb,
    settings: &SealedContainerSettingsView,
    linked_items_to_weapon: &[String],
    chosen_weapon_preset: &PresetView,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<Vec<Item>> {
    let mut mod_rewards: Vec<Vec<Item>> = Vec::new();

    for (reward_key, limits) in &settings.weapon_mod_reward_limits {
        // Drawn before the pool is built, so it is consumed even when the type is then skipped.
        let reward_count = get_int_of_minmax(limits);

        // Nothing to add, skip reward type
        if reward_count == 0 {
            continue;
        }

        // Get items that fulfil reward type criteria from items that fit on gun. A linked tpl the db
        // has no item for stands in for the null element C#'s `item?.Parent` filters out.
        let related_items: Vec<&str> = linked_items_to_weapon
            .iter()
            .filter(|tpl| {
                item_helper::get_item(&db.items_view, tpl)
                    .is_some_and(|item| item.parent.as_deref().unwrap_or_default() == reward_key)
                    && !db.global_blacklist.contains(tpl.as_str())
            })
            .map(String::as_str)
            .collect();

        if related_items.is_empty() {
            diagnostics.push(diagnostic(
                DEBUG,
                format!(
                    "No items found to fulfil reward type: {reward_key} for weapon: {}, skipping type",
                    chosen_weapon_preset.name.as_deref().unwrap_or_default()
                ),
            ));

            continue;
        }

        // Find a random item of the desired type and add as reward
        for _ in 0..reward_count {
            let chosen_item = *random_util::get_array_value(&related_items);

            mod_rewards.push(vec![new_reward_item(chosen_item)]);
        }
    }

    mod_rewards
}

/// `LootGenerator.GetRandomLootContainerLoot` (`:660-688`) — the halloween pumpkins and friends.
pub fn get_random_loot_container_loot(
    request: RandomLootContainerRequest,
) -> Result<RewardLootResult, LootError> {
    let _seed_guard = request
        .db
        .test_seed
        .map(random_util::TestSeedGuard::install);

    let db = &request.db;
    let reward_container_details = &request.reward_details;
    let mut items_to_return: Vec<Vec<Item>> = Vec::new();

    // Get random items and add to newItemRequest
    for _ in 0..reward_container_details.reward_count {
        // Pick random reward from pool, add to request object
        let chosen_reward_item_tpl = pick_reward_item(db, reward_container_details)?;

        if request.preset_tpls.contains(&chosen_reward_item_tpl) {
            // `HasPreset` is true for every tpl with a preset, `GetDefaultPreset` only returns one
            // for those with a default, and `:675` dereferences the difference.
            let preset = db
                .default_presets_by_tpl
                .get(&chosen_reward_item_tpl)
                .ok_or_else(|| {
                    LootError::new(format!(
                        "Tpl: {chosen_reward_item_tpl} has presets but no default preset"
                    ))
                })?;

            // Ensure preset has unique ids and is cloned so we don't alter the preset data stored in
            // memory. C# does not mutate the cache either: `GetDefaultPreset` hands back
            // `cloner.Clone(...)` (`PresetHelper.cs:226,230`), so `:675`'s `ReplaceIDs()` runs on a
            // clone. Both sides operate on their own copy, so the items handed back are identical.
            let mut preset_and_mods = preset.items.clone();
            item_helper::replace_ids(&mut preset_and_mods);
            item_helper::remap_root_item_id(&mut preset_and_mods);
            items_to_return.push(preset_and_mods);

            continue;
        }

        items_to_return.push(vec![new_reward_item(&chosen_reward_item_tpl)]);
    }

    Ok(RewardLootResult {
        items: items_to_return,
        diagnostics: Vec::new(),
    })
}

/// `LootGenerator.PickRewardItem` (`:695-705`).
fn pick_reward_item(
    db: &RewardLootDb,
    reward_container_details: &RewardDetailsView,
) -> Result<String, LootError> {
    if let Some(reward_tpl_pool) = &reward_container_details.reward_tpl_pool
        && !reward_tpl_pool.is_empty()
    {
        return random_util::get_weighted_value(reward_tpl_pool);
    }

    let reward_type_pool = reward_container_details
        .reward_type_pool
        .as_ref()
        // `itemTypeWhitelist.Contains(...)` inside the pool filter is the C# throw path (`:247`).
        .ok_or_else(|| LootError::new("RewardDetails.RewardTypePool is null"))?;

    let reward_pool =
        get_item_reward_pool(db, &HashSet::new(), reward_type_pool, true, true, false);
    let reward_pool_tpls: Vec<&str> = reward_pool.item_pool.iter().map(|(tpl, _)| *tpl).collect();

    get_array_value_of(&reward_pool_tpls, "Reward item pool").map(|tpl| (*tpl).to_owned())
}

#[cfg(test)]
mod tests {
    use serde_json::Value;

    use super::*;
    use crate::loot::item_helper::{AMMO, AMMO_BOX, ARMOR, WEAPON};
    use crate::loot::mongo_id;

    /// Parentless root, as `_parent: ""` items are in the db.
    const ITEM_NODE: &str = "000000000000000000000000";
    /// A non-armor, non-ammo base class.
    const MISC_NODE: &str = "111111111111111111111111";

    const CRATE_A_TPL: &str = "aaaaaaaaaaaaaaaaaaaaaaaa";
    const CRATE_B_TPL: &str = "aaaaaaaaaaaaaaaaaaaaaaab";
    /// Stackable: `StackMaxSize` 3, `StackMinRandom` 1.
    const AMMO_ITEM_TPL: &str = "bbbbbbbbbbbbbbbbbbbbbbbb";
    /// `_type` is `"Item"` — the C# compare is `OrdinalIgnoreCase`, so it belongs in the pool.
    const PLAIN_ITEM_TPL: &str = "cccccccccccccccccccccccc";
    /// Passes `armor_item_can_hold_mods`, so it can only ever be rejected.
    const ARMOR_ITEM_TPL: &str = "dddddddddddddddddddddddd";
    const QUEST_ITEM_TPL: &str = "eeeeeeeeeeeeeeeeeeeeeeee";
    const BOSS_ITEM_TPL: &str = "ffffffffffffffffffffffff";
    /// `StackMaxSize` 50, for the forced-loot split.
    const STACK_ITEM_TPL: &str = "f5f5f5f5f5f5f5f5f5f5f5f5";

    const WEAPON_TPL: &str = "a1a1a1a1a1a1a1a1a1a1a1a1";
    const WEAPON_MOD_TPL: &str = "b2b2b2b2b2b2b2b2b2b2b2b2";
    /// Second linked mod, same parent as [`WEAPON_MOD_TPL`]. Like it, `_type` is absent, so neither
    /// mod joins the plain-item reward pool and the `create_random_loot` fixtures are unaffected.
    const WEAPON_MOD_B_TPL: &str = "b2b2b2b2b2b2b2b2b2b2b2b3";
    const VEST_TPL: &str = "c3c3c3c3c3c3c3c3c3c3c3c3";
    /// `front_plate` of the vest preset, armor class 4.
    const PLATE_TPL: &str = "d4d4d4d4d4d4d4d4d4d4d4d4";

    /// Same `_ammoCaliber` as [`WEAPON_TPL`]; holds 5 rounds of [`AMMO_ITEM_TPL`] (stack max 3).
    const AMMO_BOX_MATCH_TPL: &str = "e5e5e5e5e5e5e5e5e5e5e5e5";
    /// A whitelisted box of the wrong caliber.
    const AMMO_BOX_OTHER_TPL: &str = "e5e5e5e5e5e5e5e5e5e5e5e6";
    const WEAPON_CALIBER: &str = "Caliber545x39";

    const WEAPON_PRESET_ROOT_ID: &str = "1a1a1a1a1a1a1a1a1a1a1a1a";
    const WEAPON_PRESET_MOD_ID: &str = "2b2b2b2b2b2b2b2b2b2b2b2b";
    const WEAPON_PRESET_B_ROOT_ID: &str = "1a1a1a1a1a1a1a1a1a1a1a1b";
    const VEST_PRESET_ROOT_ID: &str = "3c3c3c3c3c3c3c3c3c3c3c3c";
    const VEST_PRESET_PLATE_ID: &str = "4d4d4d4d4d4d4d4d4d4d4d4d";

    /// Every view is built through serde so the tests exercise the wire shape the C# caller sends.
    fn db_json() -> Value {
        json!({
            "itemsView": {
                // Every entry carries a `_name`, as every real db template does — a null one is
                // the C# throw path in the sealed-crate filter.
                ITEM_NODE: { "name": "Item" },
                MISC_NODE: { "parent": ITEM_NODE, "name": "misc" },
                WEAPON: { "parent": ITEM_NODE, "name": "Weapon" },
                ARMOR: { "parent": ITEM_NODE, "name": "Armor" },
                AMMO: { "parent": ITEM_NODE, "name": "Ammo" },
                // Parented outside the whitelist so the crates cannot also be drawn as plain items.
                CRATE_A_TPL: { "parent": ITEM_NODE, "type": "item", "name": "event_container_airdrop_a" },
                CRATE_B_TPL: { "parent": ITEM_NODE, "type": "item", "name": "event_container_airdrop_b" },
                AMMO_ITEM_TPL: {
                    "parent": AMMO, "type": "item", "name": "patron_545",
                    "stackMaxSize": 3, "stackMinRandom": 1
                },
                PLAIN_ITEM_TPL: { "parent": MISC_NODE, "type": "Item", "name": "bandage" },
                ARMOR_ITEM_TPL: { "parent": ARMOR, "type": "item", "name": "plain_armor" },
                QUEST_ITEM_TPL: { "parent": MISC_NODE, "type": "item", "name": "quest_thing", "questItem": true },
                BOSS_ITEM_TPL: { "parent": MISC_NODE, "type": "item", "name": "boss_thing" },
                STACK_ITEM_TPL: { "parent": MISC_NODE, "type": "item", "name": "screws", "stackMaxSize": 50 },
                WEAPON_TPL: {
                    "parent": WEAPON, "type": "item", "name": "weapon",
                    "ammoCaliber": WEAPON_CALIBER
                },
                WEAPON_MOD_TPL: { "parent": MISC_NODE, "name": "weapon_mod" },
                WEAPON_MOD_B_TPL: { "parent": MISC_NODE, "name": "weapon_mod_b" },
                VEST_TPL: { "parent": ARMOR, "type": "item", "name": "vest" },
                PLATE_TPL: { "parent": MISC_NODE, "name": "plate", "armorClass": 4 },
                // Parented under the ammo box base class, which is in no whitelist, so the boxes
                // stay out of every `create_random_loot` pool.
                AMMO_BOX: { "parent": ITEM_NODE, "name": "AmmoBox" },
                AMMO_BOX_MATCH_TPL: {
                    "parent": AMMO_BOX, "type": "item", "name": "ammo_box_545",
                    "ammoCaliber": WEAPON_CALIBER,
                    "stackSlotMaxCount": 5, "stackSlotFirstFilterFirst": AMMO_ITEM_TPL
                },
                AMMO_BOX_OTHER_TPL: {
                    "parent": AMMO_BOX, "type": "item", "name": "ammo_box_762",
                    "ammoCaliber": "Caliber762x39",
                    "stackSlotMaxCount": 5, "stackSlotFirstFilterFirst": AMMO_ITEM_TPL
                }
            },
            "defaultPresets": [weapon_preset_json(), vest_preset_json()],
            "defaultPresetsByTpl": { WEAPON_TPL: weapon_preset_json() },
            "globalBlacklist": [],
            "configBlacklist": [],
            "rewardItemBlacklist": [],
            "rewardBaseTypeBlacklist": [],
            "bossItems": [BOSS_ITEM_TPL],
            "inactiveSeasonalItems": []
        })
    }

    fn weapon_preset_json() -> Value {
        json!({
            "id": "5f5f5f5f5f5f5f5f5f5f5f5f",
            "name": "weapon_default",
            "encyclopedia": WEAPON_TPL,
            "items": [
                { "_id": WEAPON_PRESET_ROOT_ID, "_tpl": WEAPON_TPL },
                {
                    "_id": WEAPON_PRESET_MOD_ID, "_tpl": WEAPON_MOD_TPL,
                    "parentId": WEAPON_PRESET_ROOT_ID, "slotId": "mod_stock"
                }
            ]
        })
    }

    /// The weapon's non-default preset: one item, so a group's length says which preset was drawn.
    fn weapon_preset_b_json() -> Value {
        json!({
            "id": "7b7b7b7b7b7b7b7b7b7b7b7b",
            "name": "weapon_alt",
            "encyclopedia": WEAPON_TPL,
            "items": [{ "_id": WEAPON_PRESET_B_ROOT_ID, "_tpl": WEAPON_TPL }]
        })
    }

    fn vest_preset_json() -> Value {
        json!({
            "id": "6a6a6a6a6a6a6a6a6a6a6a6a",
            "name": "vest_default",
            "encyclopedia": VEST_TPL,
            "items": [
                { "_id": VEST_PRESET_ROOT_ID, "_tpl": VEST_TPL },
                {
                    "_id": VEST_PRESET_PLATE_ID, "_tpl": PLATE_TPL,
                    "parentId": VEST_PRESET_ROOT_ID, "slotId": "front_plate"
                }
            ]
        })
    }

    /// Every count at one, so each phase of `create_random_loot` contributes exactly one group.
    fn options_json() -> Value {
        json!({
            "weaponPresetCount": { "min": 1, "max": 1 },
            "armorPresetCount": { "min": 1, "max": 1 },
            "itemCount": { "min": 1, "max": 1 },
            "weaponCrateCount": { "min": 1, "max": 1 },
            "itemBlacklist": [],
            "itemTypeWhitelist": [MISC_NODE, AMMO, ARMOR],
            "itemLimits": {},
            "itemStackLimits": {},
            "armorLevelWhitelist": [4],
            "allowBossItems": false,
            "useRewardItemBlacklist": false,
            "blockSeasonalItemsOutOfSeason": false
        })
    }

    fn random_request(seed: u64, options: Value) -> CreateRandomLootRequest {
        request_from(db_json(), seed, options)
    }

    /// `RewardLootDb` is flattened into the request, so its members are spliced in one level up
    /// rather than nested.
    fn request_from(mut envelope: Value, seed: u64, options: Value) -> CreateRandomLootRequest {
        envelope["testSeed"] = json!(seed);
        envelope
            .as_object_mut()
            .unwrap()
            .insert("lootRequest".to_owned(), options);

        serde_json::from_value(envelope).unwrap()
    }

    /// Only the sealed-crate phase runs.
    fn crates_only_options() -> Value {
        let mut options = options_json();
        options["itemCount"] = json!({ "min": 0, "max": 0 });
        options["weaponPresetCount"] = json!({ "min": 0, "max": 0 });
        options["armorPresetCount"] = json!({ "min": 0, "max": 0 });

        options
    }

    fn forced_request(seed: u64, forced_loot: Value) -> CreateForcedLootRequest {
        let mut envelope = db_json();
        envelope["testSeed"] = json!(seed);
        envelope
            .as_object_mut()
            .unwrap()
            .insert("forcedLoot".to_owned(), forced_loot);

        serde_json::from_value(envelope).unwrap()
    }

    /// The generated ids vary run to run (they are not seeded), so parity is compared on the shape:
    /// each group's tpls and stack counts.
    fn shape(result: &RewardLootResult) -> Vec<Vec<(String, Option<f64>)>> {
        result
            .items
            .iter()
            .map(|group| {
                group
                    .iter()
                    .map(|item| {
                        (
                            item.template.clone(),
                            item.upd.as_ref().and_then(|upd| upd.stack_objects_count),
                        )
                    })
                    .collect()
            })
            .collect()
    }

    fn tpls(result: &RewardLootResult) -> Vec<&str> {
        result
            .items
            .iter()
            .flatten()
            .map(|item| item.template.as_str())
            .collect()
    }

    #[test]
    fn a_test_seed_makes_random_loot_deterministic() {
        // Swept over seeds rather than repeated on one: a single seed replays one set of draw
        // values, so an order hazard whose draws happen to coincide under it would stay invisible.
        for seed in 0..25 {
            let a = create_random_loot(random_request(seed, options_json())).unwrap();
            let b = create_random_loot(random_request(seed, options_json())).unwrap();

            assert_eq!(shape(&a), shape(&b), "seed {seed}");
        }
    }

    /// Golden: the exact draw sequence at one pinned seed — crate pick, three item picks with their
    /// stack counts, then the two preset picks. Comparing two runs of the same code (as
    /// `a_test_seed_makes_random_loot_deterministic` does) shifts both sides together and would miss
    /// a draw-order or draw-count regression; this literal does not.
    #[test]
    fn a_pinned_seed_draws_a_known_sequence() {
        let mut options = options_json();
        options["itemCount"] = json!({ "min": 3, "max": 3 });

        let result = create_random_loot(random_request(12345, options)).unwrap();

        // Any draw the retry loop rejected (the pool holds two armor entries) is invisible in the
        // output but still shifts everything after it, which is what makes this a draw-order check.
        // Crate B, then ammo (stack 1 of 1-3), a bandage, screws (stack 50 of 1-50), the weapon
        // preset and the vest preset — with the armor item and the vest tpl rejected in between,
        // each rejection having consumed its own draw.
        assert_eq!(
            shape(&result),
            vec![
                vec![(CRATE_B_TPL.to_owned(), Some(1.0))],
                vec![(AMMO_ITEM_TPL.to_owned(), Some(1.0))],
                vec![(PLAIN_ITEM_TPL.to_owned(), Some(1.0))],
                vec![(STACK_ITEM_TPL.to_owned(), Some(50.0))],
                vec![
                    (WEAPON_TPL.to_owned(), None),
                    (WEAPON_MOD_TPL.to_owned(), None)
                ],
                vec![(VEST_TPL.to_owned(), None), (PLATE_TPL.to_owned(), None)],
            ]
        );
    }

    #[test]
    fn random_loot_adds_a_crate_an_item_and_both_presets() {
        for seed in 0..25 {
            let result = create_random_loot(random_request(seed, options_json())).unwrap();
            let all_tpls = tpls(&result);

            // One sealed crate, one item, one weapon preset, one armor preset.
            assert_eq!(result.items.len(), 4, "seed {seed}: {all_tpls:?}");
            assert_eq!(
                all_tpls
                    .iter()
                    .filter(|tpl| **tpl == CRATE_A_TPL || **tpl == CRATE_B_TPL)
                    .count(),
                1,
                "seed {seed}"
            );
            assert!(all_tpls.contains(&WEAPON_TPL), "seed {seed}");
            assert!(all_tpls.contains(&VEST_TPL), "seed {seed}");

            // Armors come from presets, so the plain armor item is never drawn as loot, and quest
            // and boss items are filtered out of the pool entirely.
            assert!(!all_tpls.contains(&ARMOR_ITEM_TPL), "seed {seed}");
            assert!(!all_tpls.contains(&QUEST_ITEM_TPL), "seed {seed}");
            assert!(!all_tpls.contains(&BOSS_ITEM_TPL), "seed {seed}");

            // Every id is one C# `new MongoId(...)` accepts, and the whole result serializes.
            for item in result.items.iter().flatten() {
                assert!(mongo_id::is_valid(&item.id), "{} is not a MongoId", item.id);
            }
            serde_json::to_value(&result).unwrap();
        }
    }

    /// The reward pool unions in `GetBlacklistedItems()` (`config_blacklist`), while the sealed
    /// filters test `IsItemBlacklisted`'s cache (`global_blacklist`). Two different C# sources, so a
    /// tpl blacklisted only in the cache still reaches the pool.
    #[test]
    fn the_reward_pool_reads_the_config_blacklist_and_not_the_cache() {
        let mut options = options_json();
        options["itemCount"] = json!({ "min": 3, "max": 3 });
        options["weaponPresetCount"] = json!({ "min": 0, "max": 0 });
        options["armorPresetCount"] = json!({ "min": 0, "max": 0 });
        options["weaponCrateCount"] = json!({ "min": 0, "max": 0 });

        let mut cache_only = db_json();
        cache_only["globalBlacklist"] = json!([PLAIN_ITEM_TPL]);
        let unfiltered =
            create_random_loot(request_from(cache_only, 12345, options.clone())).unwrap();
        assert!(tpls(&unfiltered).contains(&PLAIN_ITEM_TPL));

        let mut config = db_json();
        config["configBlacklist"] = json!([PLAIN_ITEM_TPL]);
        let filtered = create_random_loot(request_from(config, 12345, options)).unwrap();
        assert!(!tpls(&filtered).contains(&PLAIN_ITEM_TPL));
    }

    #[test]
    fn preset_items_are_found_in_raid_under_fresh_ids() {
        let result = create_random_loot(random_request(7, options_json())).unwrap();
        let preset_group = result
            .items
            .iter()
            .find(|group| group.len() == 2 && group[0].template == WEAPON_TPL)
            .expect("weapon preset group");

        assert!(
            preset_group
                .iter()
                .all(|item| item.id != WEAPON_PRESET_ROOT_ID && item.id != WEAPON_PRESET_MOD_ID)
        );
        // The mod still hangs off the (remapped) root.
        assert_eq!(
            preset_group[1].parent_id.as_deref(),
            Some(preset_group[0].id.as_str())
        );
        assert!(
            preset_group
                .iter()
                .all(|item| item.upd.as_ref().unwrap().extra["SpawnedInSession"] == json!(true))
        );
    }

    #[test]
    fn armor_level_whitelist_admits_only_matching_presets() {
        let mut options = options_json();
        options["armorLevelWhitelist"] = json!([4]);
        let admitted = create_random_loot(random_request(3, options)).unwrap();
        assert!(tpls(&admitted).contains(&VEST_TPL));

        let mut options = options_json();
        options["armorLevelWhitelist"] = json!([2]);
        let rejected = create_random_loot(random_request(3, options)).unwrap();
        assert!(!tpls(&rejected).contains(&VEST_TPL));
        assert!(rejected.diagnostics.is_empty());
    }

    /// The pool filter compares `_type` with `OrdinalIgnoreCase`, so `"Item"` belongs in it.
    #[test]
    fn item_type_is_matched_ignoring_case() {
        let mut options = options_json();
        options["weaponCrateCount"] = json!({ "min": 0, "max": 0 });
        options["weaponPresetCount"] = json!({ "min": 0, "max": 0 });
        options["armorPresetCount"] = json!({ "min": 0, "max": 0 });
        options["itemTypeWhitelist"] = json!([MISC_NODE]);
        options["itemBlacklist"] = json!([STACK_ITEM_TPL]);

        let result = create_random_loot(random_request(11, options)).unwrap();

        assert_eq!(tpls(&result), vec![PLAIN_ITEM_TPL]);
    }

    /// The C# limit check is dead code (`:312-316`): a tracked parent takes the `!found` half out,
    /// an untracked one leaves both sides of the comparison null. A max of 0 must not reject.
    #[test]
    fn the_item_limit_guard_never_rejects() {
        let mut options = options_json();
        options["weaponCrateCount"] = json!({ "min": 0, "max": 0 });
        options["weaponPresetCount"] = json!({ "min": 0, "max": 0 });
        options["armorPresetCount"] = json!({ "min": 0, "max": 0 });
        options["itemCount"] = json!({ "min": 3, "max": 3 });
        options["itemTypeWhitelist"] = json!([MISC_NODE]);
        options["itemBlacklist"] = json!([STACK_ITEM_TPL]);
        options["itemLimits"] = json!({ MISC_NODE: 0 });

        let result = create_random_loot(random_request(5, options)).unwrap();

        // A working limit check would have rejected the second and third draws forever.
        assert_eq!(tpls(&result), vec![PLAIN_ITEM_TPL; 3]);
    }

    #[test]
    fn stackable_items_get_a_randomised_stack_count() {
        let mut options = options_json();
        options["weaponCrateCount"] = json!({ "min": 0, "max": 0 });
        options["weaponPresetCount"] = json!({ "min": 0, "max": 0 });
        options["armorPresetCount"] = json!({ "min": 0, "max": 0 });
        options["itemTypeWhitelist"] = json!([AMMO]);

        for seed in 0..25 {
            let result = create_random_loot(random_request(seed, options.clone())).unwrap();
            let count = result.items[0][0].upd.as_ref().unwrap().stack_objects_count;

            assert_eq!(result.items[0][0].template, AMMO_ITEM_TPL);
            assert!(
                matches!(count, Some(count) if (1.0..=3.0).contains(&count)),
                "seed {seed}: {count:?}"
            );
        }
    }

    /// A per-item override in `itemStackLimits` replaces both ends of the range.
    #[test]
    fn item_stack_limits_override_the_template_range() {
        let mut options = options_json();
        options["weaponCrateCount"] = json!({ "min": 0, "max": 0 });
        options["weaponPresetCount"] = json!({ "min": 0, "max": 0 });
        options["armorPresetCount"] = json!({ "min": 0, "max": 0 });
        options["itemTypeWhitelist"] = json!([AMMO]);
        options["itemStackLimits"] = json!({ AMMO_ITEM_TPL: { "min": 30, "max": 30 } });

        let result = create_random_loot(random_request(2, options)).unwrap();

        assert_eq!(
            result.items[0][0].upd.as_ref().unwrap().stack_objects_count,
            Some(30.0)
        );
    }

    #[test]
    fn a_null_minmax_is_the_c_sharp_null_dereference() {
        let mut options = options_json();
        options["weaponCrateCount"] = Value::Null;

        let error = create_random_loot(random_request(1, options)).unwrap_err();

        assert!(error.message.contains("WeaponCrateCount"), "{error:?}");
    }

    /// `item.Name.Contains(...)` (`:57`) dereferences a null `_name`.
    #[test]
    fn a_template_without_a_name_is_the_c_sharp_null_dereference() {
        let mut envelope = db_json();
        envelope["itemsView"][PLAIN_ITEM_TPL]["name"] = Value::Null;

        let error =
            create_random_loot(request_from(envelope, 1, crates_only_options())).unwrap_err();

        assert!(error.message.contains("has no name"), "{error:?}");
    }

    /// `chosenPreset.Items.FirstOrDefault().Template` (`:419`) dereferences a null on a preset with
    /// no items.
    #[test]
    fn a_preset_without_items_is_the_c_sharp_null_dereference() {
        let mut envelope = db_json();
        envelope["defaultPresets"] = json!([{
            "id": "7b7b7b7b7b7b7b7b7b7b7b7b",
            "encyclopedia": WEAPON_TPL,
            "items": []
        }]);
        let mut options = crates_only_options();
        options["weaponCrateCount"] = json!({ "min": 0, "max": 0 });
        options["weaponPresetCount"] = json!({ "min": 1, "max": 1 });

        let error = create_random_loot(request_from(envelope, 1, options)).unwrap_err();

        assert!(error.message.contains("has no items"), "{error:?}");
    }

    #[test]
    fn forced_loot_clones_presets_and_splits_stacks() {
        let request = forced_request(
            9,
            json!({
                WEAPON_TPL: { "min": 2, "max": 2 },
                STACK_ITEM_TPL: { "min": 250, "max": 250 }
            }),
        );

        let result = create_forced_loot(request).unwrap();

        // Two preset clones, then ceil(250 / 50) split stacks.
        assert_eq!(result.items.len(), 7);
        assert_eq!(shape(&result)[0], shape(&result)[1]);
        assert_eq!(
            shape(&result)[0],
            vec![
                (WEAPON_TPL.to_owned(), None),
                (WEAPON_MOD_TPL.to_owned(), None)
            ]
        );

        for group in &result.items[2..] {
            assert_eq!(group.len(), 1);
            assert_eq!(group[0].template, STACK_ITEM_TPL);
            assert_eq!(
                group[0].upd.as_ref().unwrap().stack_objects_count,
                Some(50.0)
            );
            assert_eq!(
                group[0].upd.as_ref().unwrap().extra["SpawnedInSession"],
                json!(true)
            );
        }

        // Preset ids are replaced, and every id is still a MongoId.
        for item in result.items.iter().flatten() {
            assert!(mongo_id::is_valid(&item.id), "{} is not a MongoId", item.id);
            assert_ne!(item.id, WEAPON_PRESET_ROOT_ID);
            assert_ne!(item.id, WEAPON_PRESET_MOD_ID);
        }

        serde_json::to_value(&result).unwrap();
    }

    /// `SplitStack` cannot end its loop without a positive `StackMaxSize`; C# hangs there, the port
    /// returns the error instead.
    #[test]
    fn forced_loot_propagates_a_split_that_cannot_terminate() {
        let request = forced_request(1, json!({ PLAIN_ITEM_TPL: { "min": 5, "max": 5 } }));

        let error = create_forced_loot(request).unwrap_err();

        assert!(error.message.contains("StackMaxSize"), "{error:?}");
    }

    // -----------------------------------------------------------------------
    // Sealed weapon case
    // -----------------------------------------------------------------------

    /// A weapon (single weight entry, so the weighted pick returns without drawing), two mod
    /// rewards off the linked list, and one caliber-matched ammo box.
    fn sealed_settings_json() -> Value {
        json!({
            "weaponRewardWeight": { WEAPON_TPL: 1 },
            "defaultPresetsOnly": false,
            "weaponModRewardLimits": { MISC_NODE: { "min": 2, "max": 2 } },
            "rewardTypeLimits": { AMMO_BOX: { "min": 1, "max": 1 } },
            "ammoBoxWhitelist": [AMMO_BOX_MATCH_TPL, AMMO_BOX_OTHER_TPL],
            "allowBossItems": false
        })
    }

    fn sealed_request(seed: u64, settings: Value) -> SealedWeaponCaseRequest {
        sealed_request_from(db_json(), seed, settings)
    }

    fn sealed_request_from(
        mut envelope: Value,
        seed: u64,
        settings: Value,
    ) -> SealedWeaponCaseRequest {
        envelope["testSeed"] = json!(seed);
        let object = envelope.as_object_mut().unwrap();
        object.insert("containerSettings".to_owned(), settings);
        object.insert(
            "presetsByTpl".to_owned(),
            json!({ WEAPON_TPL: [weapon_preset_json(), weapon_preset_b_json()] }),
        );
        object.insert(
            "linkedItems".to_owned(),
            // Two mods parented under the reward key, plus one item that is not.
            json!({ WEAPON_TPL: [WEAPON_MOD_TPL, WEAPON_MOD_B_TPL, AMMO_ITEM_TPL] }),
        );

        serde_json::from_value(envelope).unwrap()
    }

    /// Only the mod-reward phase runs.
    fn mods_only_settings() -> Value {
        let mut settings = sealed_settings_json();
        settings["rewardTypeLimits"] = json!({});

        settings
    }

    #[test]
    fn a_test_seed_makes_the_sealed_case_deterministic() {
        for seed in 0..25 {
            let a =
                get_sealed_weapon_case_loot(sealed_request(seed, sealed_settings_json())).unwrap();
            let b =
                get_sealed_weapon_case_loot(sealed_request(seed, sealed_settings_json())).unwrap();

            assert_eq!(shape(&a), shape(&b), "seed {seed}");
        }
    }

    /// Golden: preset first, then the mod rewards, then the ammo box with its cartridges — the
    /// order `:495-502` builds them in, at a pinned seed so a draw-count regression shows up.
    #[test]
    fn a_pinned_seed_draws_a_known_sealed_case() {
        let result =
            get_sealed_weapon_case_loot(sealed_request(12345, sealed_settings_json())).unwrap();

        assert_eq!(
            shape(&result),
            vec![
                // Preset draw: the alt preset (one item) out of the tpl's two.
                vec![(WEAPON_TPL.to_owned(), None)],
                // The mod count is a fixed 2 (no draw), then one draw per reward.
                vec![(WEAPON_MOD_TPL.to_owned(), None)],
                vec![(WEAPON_MOD_TPL.to_owned(), None)],
                // Ammo box: 5 rounds of a stack-3 cartridge is 3 + 2.
                vec![
                    (AMMO_BOX_MATCH_TPL.to_owned(), None),
                    (AMMO_ITEM_TPL.to_owned(), Some(3.0)),
                    (AMMO_ITEM_TPL.to_owned(), Some(2.0)),
                ],
            ]
        );
        assert!(result.diagnostics.is_empty(), "{:?}", result.diagnostics);

        // The cartridge that lands on location 0 goes out without one, as live does it.
        let ammo_box = result.items.last().unwrap();
        assert_eq!(ammo_box[1].location, Some(json!(1)));
        assert!(ammo_box[2].location.is_none());
        for item in result.items.iter().flatten() {
            assert!(mongo_id::is_valid(&item.id), "{} is not a MongoId", item.id);
        }
        serde_json::to_value(&result).unwrap();
    }

    /// `defaultPresetsOnly` takes `GetDefaultPreset` instead of drawing from the tpl's presets.
    #[test]
    fn default_presets_only_takes_the_default_preset() {
        let mut settings = mods_only_settings();
        settings["defaultPresetsOnly"] = json!(true);
        settings["weaponModRewardLimits"] = json!({});

        for seed in 0..10 {
            let result =
                get_sealed_weapon_case_loot(sealed_request(seed, settings.clone())).unwrap();

            assert_eq!(
                shape(&result),
                vec![vec![
                    (WEAPON_TPL.to_owned(), None),
                    (WEAPON_MOD_TPL.to_owned(), None)
                ]],
                "seed {seed}"
            );
        }
    }

    /// Ids are replaced and the mod still hangs off the remapped root (`:491-492`), and — unlike
    /// `create_random_loot`'s presets — nothing is marked found-in-raid here; the C# caller does it.
    #[test]
    fn sealed_case_presets_get_fresh_ids_and_no_found_in_raid() {
        let mut settings = mods_only_settings();
        settings["defaultPresetsOnly"] = json!(true);
        settings["weaponModRewardLimits"] = json!({});

        let result = get_sealed_weapon_case_loot(sealed_request(4, settings)).unwrap();
        let preset = &result.items[0];

        assert!(
            preset
                .iter()
                .all(|item| item.id != WEAPON_PRESET_ROOT_ID && item.id != WEAPON_PRESET_MOD_ID)
        );
        assert_eq!(preset[1].parent_id.as_deref(), Some(preset[0].id.as_str()));
        assert!(preset.iter().all(|item| item.upd.is_none()));
    }

    /// No default for the tpl (`:484-488`) — warn, then draw from its presets anyway.
    #[test]
    fn a_missing_default_preset_warns_and_draws_a_random_one() {
        let mut envelope = db_json();
        envelope["defaultPresetsByTpl"] = json!({});
        let mut settings = mods_only_settings();
        settings["defaultPresetsOnly"] = json!(true);
        settings["weaponModRewardLimits"] = json!({});

        let result =
            get_sealed_weapon_case_loot(sealed_request_from(envelope, 6, settings)).unwrap();

        assert_eq!(result.items.len(), 1);
        assert_eq!(
            result.diagnostics[0].locale_key.as_deref(),
            Some("loot-default_preset_not_found_using_random")
        );
        assert_eq!(result.diagnostics[0].args, Some(json!(WEAPON_TPL)));
    }

    /// The weighted pick landed on a tpl the db has no item for (`:470-476`).
    #[test]
    fn a_weapon_tpl_missing_from_the_db_ends_the_sealed_case() {
        let mut settings = sealed_settings_json();
        settings["weaponRewardWeight"] = json!({ "aaaaaaaaaaaaaaaaaaaaaaa9": 1 });

        let result = get_sealed_weapon_case_loot(sealed_request(1, settings)).unwrap();

        assert!(result.items.is_empty());
        assert_eq!(
            result.diagnostics[0].locale_key.as_deref(),
            Some("loot-non_item_picked_as_sealed_weapon_crate_reward")
        );
        assert_eq!(result.diagnostics[0].level, ERROR);
    }

    /// A tpl with no presets at all: `GetArrayValue` on the empty list is C#'s
    /// `ArgumentOutOfRangeException`.
    #[test]
    fn a_weapon_without_presets_is_the_c_sharp_empty_draw() {
        let mut envelope = db_json();
        envelope["defaultPresetsByTpl"] = json!({});
        let mut settings = mods_only_settings();
        settings["weaponRewardWeight"] = json!({ VEST_TPL: 1 });

        let error =
            get_sealed_weapon_case_loot(sealed_request_from(envelope, 1, settings)).unwrap_err();

        assert!(error.message.contains("Preset pool"), "{error:?}");
    }

    /// The mod pool is the linked list filtered to the reward key and past the blacklist
    /// (`:626-628`) — order preserved, so the draw indexes the same slots the C# does.
    #[test]
    fn weapon_mod_rewards_skip_blacklisted_and_unrelated_items() {
        let mut envelope = db_json();
        envelope["globalBlacklist"] = json!([WEAPON_MOD_B_TPL]);
        let mut settings = mods_only_settings();
        settings["weaponModRewardLimits"] = json!({ MISC_NODE: { "min": 3, "max": 3 } });

        let result =
            get_sealed_weapon_case_loot(sealed_request_from(envelope, 8, settings)).unwrap();

        // The preset group plus three mod rewards, all of the one mod that survived the filter —
        // the blacklisted mod and the one parented outside the reward key are both gone.
        assert_eq!(result.items.len(), 4);
        for reward in &result.items[1..] {
            assert_eq!(reward.len(), 1);
            assert_eq!(reward[0].template, WEAPON_MOD_TPL);
        }
    }

    /// Nothing linked to the weapon fulfils the reward type (`:629-637`).
    #[test]
    fn an_empty_mod_pool_logs_a_debug_line_naming_the_preset() {
        let mut settings = mods_only_settings();
        settings["defaultPresetsOnly"] = json!(true);
        settings["weaponModRewardLimits"] = json!({ ARMOR: { "min": 1, "max": 1 } });

        let result = get_sealed_weapon_case_loot(sealed_request(2, settings)).unwrap();

        assert_eq!(result.items.len(), 1);
        assert_eq!(result.diagnostics[0].level, DEBUG);
        assert_eq!(
            result.diagnostics[0].message.as_deref(),
            Some(
                format!(
                    "No items found to fulfil reward type: {ARMOR} for weapon: weapon_default, skipping type"
                )
                .as_str()
            )
        );
    }

    /// The count is drawn before the pool is built (`:617` ahead of `:626`), so a reward type that
    /// is then skipped for want of items has still moved the stream on.
    ///
    /// (A `min == max` count draws nothing at all — `RandomUtil.GetInt` short-circuits when
    /// `max <= min` — so the skipped type here needs a real range to be observable.)
    #[test]
    fn a_skipped_reward_type_has_still_drawn_its_count() {
        let mut with_skip_settings = mods_only_settings();
        with_skip_settings["defaultPresetsOnly"] = json!(true);
        with_skip_settings["weaponModRewardLimits"] = json!({
            // No linked item is parented under ARMOR, so this type only ever logs the skip.
            ARMOR: { "min": 1, "max": 2 },
            MISC_NODE: { "min": 1, "max": 1 }
        });

        let mut without_settings = mods_only_settings();
        without_settings["defaultPresetsOnly"] = json!(true);
        without_settings["weaponModRewardLimits"] = json!({ MISC_NODE: { "min": 1, "max": 1 } });

        let mut seeds_that_disagree = 0;
        for seed in 0..25 {
            let with_skip =
                get_sealed_weapon_case_loot(sealed_request(seed, with_skip_settings.clone()))
                    .unwrap();
            let without =
                get_sealed_weapon_case_loot(sealed_request(seed, without_settings.clone()))
                    .unwrap();

            // Both hand back the preset and one mod; only which mod can differ.
            assert_eq!(with_skip.items.len(), 2, "seed {seed}");
            assert_eq!(without.items.len(), 2, "seed {seed}");
            if with_skip.items[1][0].template != without.items[1][0].template {
                seeds_that_disagree += 1;
            }
        }

        // Had the skipped type not drawn, the two streams would line up at every seed.
        assert!(seeds_that_disagree > 0);
    }

    /// Only boxes whose caliber matches the weapon's are eligible (`:539-540`).
    #[test]
    fn only_caliber_matching_ammo_boxes_are_drawn() {
        let mut settings = mods_only_settings();
        settings["defaultPresetsOnly"] = json!(true);
        settings["weaponModRewardLimits"] = json!({});
        settings["rewardTypeLimits"] = json!({ AMMO_BOX: { "min": 4, "max": 4 } });

        for seed in 0..15 {
            let result =
                get_sealed_weapon_case_loot(sealed_request(seed, settings.clone())).unwrap();

            assert_eq!(result.items.len(), 5, "seed {seed}");
            for box_group in &result.items[1..] {
                assert_eq!(box_group[0].template, AMMO_BOX_MATCH_TPL, "seed {seed}");
                assert_eq!(box_group.len(), 3, "seed {seed}");
            }
        }
    }

    #[test]
    fn no_matching_ammo_box_logs_a_debug_line() {
        let mut envelope = db_json();
        envelope["itemsView"][AMMO_BOX_MATCH_TPL]["ammoCaliber"] = json!("Caliber762x39");
        let mut settings = mods_only_settings();
        settings["defaultPresetsOnly"] = json!(true);
        settings["weaponModRewardLimits"] = json!({});
        settings["rewardTypeLimits"] = json!({ AMMO_BOX: { "min": 1, "max": 1 } });

        let result =
            get_sealed_weapon_case_loot(sealed_request_from(envelope, 1, settings)).unwrap();

        assert_eq!(result.items.len(), 1);
        assert_eq!(
            result.diagnostics[0].message.as_deref(),
            Some(format!("No ammo box with caliber {WEAPON_CALIBER} found, skipping").as_str())
        );
    }

    /// A whitelisted box the db has no item for: C# puts a null in the list and dereferences
    /// `.Properties` on it (`:540`).
    #[test]
    fn an_unknown_ammo_box_tpl_is_the_c_sharp_null_dereference() {
        let mut settings = mods_only_settings();
        settings["defaultPresetsOnly"] = json!(true);
        settings["weaponModRewardLimits"] = json!({});
        settings["rewardTypeLimits"] = json!({ AMMO_BOX: { "min": 1, "max": 1 } });
        settings["ammoBoxWhitelist"] = json!(["aaaaaaaaaaaaaaaaaaaaaaa9"]);

        let error = get_sealed_weapon_case_loot(sealed_request(1, settings)).unwrap_err();

        assert!(
            error.message.contains("aaaaaaaaaaaaaaaaaaaaaaa9"),
            "{error:?}"
        );
    }

    /// The two quirks at `:569-570`, asserted deliberately: the pool keeps only *blacklisted*
    /// items, and `allowBossItems` being true empties it outright.
    #[test]
    fn the_non_weapon_mod_pool_is_blacklist_only_and_boss_items_empty_it() {
        let mut envelope = db_json();
        envelope["globalBlacklist"] = json!([PLAIN_ITEM_TPL]);
        let mut settings = mods_only_settings();
        settings["defaultPresetsOnly"] = json!(true);
        settings["weaponModRewardLimits"] = json!({});
        settings["rewardTypeLimits"] = json!({ MISC_NODE: { "min": 2, "max": 2 } });

        let blacklist_only =
            get_sealed_weapon_case_loot(sealed_request_from(envelope.clone(), 1, settings.clone()))
                .unwrap();

        // Only the blacklisted item is eligible — every other MISC item is filtered out.
        assert_eq!(blacklist_only.items.len(), 3);
        for reward in &blacklist_only.items[1..] {
            assert_eq!(reward.len(), 1);
            assert_eq!(reward[0].template, PLAIN_ITEM_TPL);
        }

        settings["allowBossItems"] = json!(true);
        let boss_items_allowed =
            get_sealed_weapon_case_loot(sealed_request_from(envelope, 1, settings)).unwrap();

        // `!(true || IsBossItem(id))` is false for every item, so the pool is empty and the branch
        // only logs.
        assert_eq!(boss_items_allowed.items.len(), 1);
        assert_eq!(
            boss_items_allowed.diagnostics[0].message.as_deref(),
            Some(format!("No items with base type of {MISC_NODE} found, skipping").as_str())
        );
    }

    /// `QuestItem is null` (`:571`) — an explicit `false` is not the same as an absent value.
    #[test]
    fn an_explicit_quest_item_false_is_excluded_from_the_reward_pool() {
        let mut envelope = db_json();
        envelope["globalBlacklist"] = json!([PLAIN_ITEM_TPL]);
        envelope["itemsView"][PLAIN_ITEM_TPL]["questItem"] = json!(false);
        let mut settings = mods_only_settings();
        settings["defaultPresetsOnly"] = json!(true);
        settings["weaponModRewardLimits"] = json!({});
        settings["rewardTypeLimits"] = json!({ MISC_NODE: { "min": 1, "max": 1 } });

        let result =
            get_sealed_weapon_case_loot(sealed_request_from(envelope, 1, settings)).unwrap();

        assert_eq!(result.items.len(), 1);
        assert_eq!(result.diagnostics[0].level, DEBUG);
    }

    // -----------------------------------------------------------------------
    // Random loot container
    // -----------------------------------------------------------------------

    fn container_request(
        seed: u64,
        reward_details: Value,
        preset_tpls: Value,
    ) -> RandomLootContainerRequest {
        let mut envelope = db_json();
        envelope["testSeed"] = json!(seed);
        let object = envelope.as_object_mut().unwrap();
        object.insert("rewardDetails".to_owned(), reward_details);
        object.insert("presetTpls".to_owned(), preset_tpls);

        serde_json::from_value(envelope).unwrap()
    }

    /// The weighted pool path (`:697-700`), and the preset expansion at `:670-681`.
    #[test]
    fn a_container_draws_its_reward_tpl_pool_and_expands_presets() {
        let details = json!({
            "rewardCount": 6,
            "rewardTplPool": { PLAIN_ITEM_TPL: 3, WEAPON_TPL: 1 }
        });

        let result =
            get_random_loot_container_loot(container_request(9, details, json!([WEAPON_TPL])))
                .unwrap();

        assert_eq!(result.items.len(), 6);
        for group in &result.items {
            match group.len() {
                // Non-preset reward: one item, no `Upd` (`:683`).
                1 => {
                    assert_eq!(group[0].template, PLAIN_ITEM_TPL);
                    assert!(group[0].upd.is_none());
                }
                // Preset reward: the default preset's items under fresh ids.
                2 => {
                    assert_eq!(group[0].template, WEAPON_TPL);
                    assert_eq!(group[1].template, WEAPON_MOD_TPL);
                    assert_ne!(group[0].id, WEAPON_PRESET_ROOT_ID);
                    assert_eq!(group[1].parent_id.as_deref(), Some(group[0].id.as_str()));
                }
                other => panic!("unexpected group size {other}"),
            }
        }
        // Both branches were taken at this seed.
        assert!(result.items.iter().any(|group| group.len() == 2));
        assert!(result.items.iter().any(|group| group.len() == 1));
        serde_json::to_value(&result).unwrap();
    }

    #[test]
    fn a_test_seed_makes_the_container_deterministic() {
        let details = json!({
            "rewardCount": 4,
            "rewardTplPool": { PLAIN_ITEM_TPL: 3, STACK_ITEM_TPL: 1 }
        });

        for seed in 0..25 {
            let a =
                get_random_loot_container_loot(container_request(seed, details.clone(), json!([])))
                    .unwrap();
            let b =
                get_random_loot_container_loot(container_request(seed, details.clone(), json!([])))
                    .unwrap();

            assert_eq!(shape(&a), shape(&b), "seed {seed}");
        }
    }

    /// No tpl pool: fall back to a reward pool built over the type pool (`:702-704`), with the
    /// reward-item blacklist on and boss items allowed.
    #[test]
    fn an_empty_tpl_pool_falls_back_to_the_reward_type_pool() {
        let details = json!({
            "rewardCount": 8,
            "rewardTplPool": {},
            "rewardTypePool": [MISC_NODE]
        });

        let result =
            get_random_loot_container_loot(container_request(5, details, json!([]))).unwrap();

        assert_eq!(result.items.len(), 8);
        for tpl in tpls(&result) {
            // Quest items are filtered out of the pool; boss items are not, as `:703` passes
            // `allowBossItems: true`.
            assert!(
                [PLAIN_ITEM_TPL, STACK_ITEM_TPL, BOSS_ITEM_TPL].contains(&tpl),
                "{tpl} is not in the reward pool"
            );
        }
        assert!(tpls(&result).contains(&BOSS_ITEM_TPL));
    }

    /// `HasPreset` is `PresetCache.ContainsKey` — true for tpls with presets but no *default*, and
    /// `:675` then dereferences the null `GetDefaultPreset` returned.
    #[test]
    fn a_preset_tpl_without_a_default_is_the_c_sharp_null_dereference() {
        let details = json!({ "rewardCount": 1, "rewardTplPool": { PLAIN_ITEM_TPL: 1 } });

        let error =
            get_random_loot_container_loot(container_request(1, details, json!([PLAIN_ITEM_TPL])))
                .unwrap_err();

        assert!(error.message.contains(PLAIN_ITEM_TPL), "{error:?}");
    }

    /// Neither pool set: the type pool is null and `GetItemRewardPool` probes it with `Contains`.
    #[test]
    fn a_container_without_any_pool_is_the_c_sharp_null_dereference() {
        let error = get_random_loot_container_loot(container_request(
            1,
            json!({ "rewardCount": 1 }),
            json!([]),
        ))
        .unwrap_err();

        assert!(error.message.contains("RewardTypePool"), "{error:?}");
    }
}
