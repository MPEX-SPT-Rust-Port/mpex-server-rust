//! `Generators/Loot/LocationLootGenerator.cs:94-1266` — the loot generator, ported method for
//! method: static containers first, then the dynamic (loose) loot half.
//!
//! The C# logs through `ISptLogger` and localises through `ServerLocalisationService`; both come out
//! of here as [`Diagnostic`]s for the caller to replay. Where the C# throws (or dereferences a null
//! and crashes), the port returns a [`LootError`] rather than panicking behind the FFI boundary —
//! each such site names the C# line it stands in for.

use std::collections::{HashMap, HashSet};

use indexmap::IndexMap;
use serde_json::json;

use super::container_extensions::{
    FindSlotResult, find_slot_for_item, try_fill_container_map_with_item,
};
use super::item_helper::{self, LootContext, LootError};
use super::models::{
    CounterState, DEBUG, Diagnostic, DynamicLootRequest, DynamicLootResult, ERROR, Item,
    ItemLocation, ItemRotation, LootCommon, LootConfigView, SUCCESS, Spawnpoint,
    SpawnpointTemplate, SptLootItem, StaticContainer, StaticContainerData, StaticContainersRequest,
    StaticContainersResult, StaticForced, StaticLootDetails, Upd, WARNING,
};
use super::probability_object_array::{ProbabilityObject, ProbabilityObjectArray};
use super::{mongo_id, random_util};
use crate::diag::DiagSink;

/// The `typeof(T).FullName` this file's diagnostics log under.
const CATEGORY: &str = "SPTarkov.Server.Core.Generators.Loot.LocationLootGenerator";

/// `LocationLootGenerator.cs:1269-1276`. C# types `ChosenCount` as `double?`; the empty group is
/// seeded with -1 and every other value comes out of `GetInt`.
#[derive(Debug, Clone, Default)]
struct ContainerGroupCount {
    /// `IndexMap`, not `HashMap` or `BTreeMap`: the iteration order decides both the order
    /// containers are rolled in and the order they enter the probability array, and the C#
    /// `Dictionary` it stands in for walks in insertion order — so the containers must be kept in
    /// the order the map was filled in, not sorted by id.
    container_ids_with_probability: IndexMap<String, f64>,
    chosen_count: f64,
}

/// `LocationLootGenerator.cs:1278-1290`.
#[derive(Debug, Clone)]
struct ContainerItem {
    items: Vec<Item>,
    width: Option<i32>,
    height: Option<i32>,
}

/// A plain interpolated log line.
fn diagnostic(level: &str, message: String) -> Diagnostic {
    Diagnostic {
        category: CATEGORY,
        level: level.to_owned(),
        locale_key: None,
        args: None,
        message: Some(message),
    }
}

/// A `ServerLocalisationService.GetText` line: the key plus the arguments the C# passes with it
/// (a bare value for the `%s` keys, an object whose members match the C# anonymous type otherwise).
fn localised(level: &str, locale_key: &str, args: serde_json::Value) -> Diagnostic {
    Diagnostic {
        category: CATEGORY,
        level: level.to_owned(),
        locale_key: Some(locale_key.to_owned()),
        args: Some(args),
        message: None,
    }
}

/// The container spawn point's id. C# dereferences `Template.Id` unguarded (`:130,241,392,461`).
fn template_id(container: &StaticContainerData) -> &str {
    container
        .template
        .as_ref()
        .and_then(|template| template.id.as_deref())
        .unwrap_or_default()
}

/// The tpl of the container item itself. C# dereferences `Items.FirstOrDefault()` unguarded
/// (`:290,306`).
fn first_item_tpl(container: &StaticContainerData) -> Option<&str> {
    let items = container.template.as_ref()?.items.as_ref()?;

    items.first().map(|item| item.item.template.as_str())
}

/// `SpawnpointTemplate.Items.Count()` (`:153,176,258`).
fn item_count(template: &SpawnpointTemplate) -> i32 {
    template.items.as_ref().map_or(0, Vec::len) as i32
}

/// The read-only half of a request, lent to the run; `counter` moves in so the run can mutate it
/// and the totals can be handed back to C#.
fn loot_context(common: &LootCommon, counter: CounterState) -> LootContext<'_> {
    LootContext {
        items_view: &common.items_view,
        static_ammo_dist: &common.static_ammo_dist,
        default_presets: &common.default_presets,
        money_tpls: &common.money_tpls,
        lootable_item_blacklist: &common.lootable_item_blacklist,
        config: &common.config,
        seasonal: &common.seasonal,
        counter,
        diagnostics: DiagSink::Pipeline,
    }
}

/// Consumes the run, handing the caller its spawn points, counters and log lines.
fn into_result(
    ctx: LootContext,
    spawnpoints: Vec<SpawnpointTemplate>,
    static_loot_item_count: i32,
    static_container_count: i32,
) -> StaticContainersResult {
    StaticContainersResult {
        spawnpoints,
        tracked_counts: ctx.counter.tracked_counts,
        static_loot_item_count,
        static_container_count,
    }
}

/// `LocationLootGenerator.GenerateStaticContainers` (`:94-266`) — mounted weapons, then every
/// guaranteed container, then a weighted pick per container group.
pub fn generate_static_containers(
    mut request: StaticContainersRequest,
) -> Result<StaticContainersResult, LootError> {
    let _seed_guard = request
        .common
        .test_seed
        .map(random_util::TestSeedGuard::install);

    // Everything the run mutates is moved out before the rest of the request is lent to the context.
    let counter = std::mem::take(&mut request.common.counter);
    let static_weapons = request.static_weapons.take();
    let static_containers = request.static_containers.take();
    let static_forced = request.static_forced.take();
    let statics = request.statics.take();

    let mut ctx = loot_context(&request.common, counter);
    let location_id = request.common.location_id.as_str();
    let config = ctx.config;
    let seasonal = ctx.seasonal;

    let mut static_loot_item_count = 0;
    let mut result: Vec<SpawnpointTemplate> = Vec::new();

    let Some(static_weapons) = static_weapons else {
        ctx.diagnostics.push(localised(
            ERROR,
            "location-unable_to_find_static_weapon_for_map",
            json!(location_id),
        ));

        // `result.AddRange(staticWeaponsOnMap)` (`:111`) throws on the null it just logged about.
        return Err(LootError::new(format!(
            "Unable to find static weapon data for map: {location_id}"
        )));
    };

    // Add mounted weapons to output loot
    result.extend(static_weapons);

    if static_containers.is_none() {
        ctx.diagnostics.push(localised(
            ERROR,
            "location-unable_to_find_static_container_for_map",
            json!(location_id),
        ));
    }

    // Containers that MUST be added to map (e.g. quest containers)
    if static_forced.is_none() {
        ctx.diagnostics.push(localised(
            ERROR,
            "location-unable_to_find_forced_static_data_for_map",
            json!(location_id),
        ));
    }

    let Some(all_static_containers_on_map) = static_containers else {
        // Both errors above are logged first, then the christmas filter (`:129`) or
        // `GetRandomisableContainersOnMap` (`:134`) enumerates the null list and throws.
        return Err(LootError::new(format!(
            "Unable to find static container data for map: {location_id}"
        )));
    };

    // Remove christmas items from loot data
    let all_static_containers_on_map: Vec<StaticContainerData> = if seasonal.christmas_event_enabled
    {
        all_static_containers_on_map
    } else {
        all_static_containers_on_map
            .into_iter()
            .filter(|container| {
                !seasonal
                    .christmas_container_ids
                    .contains(template_id(container))
            })
            .collect()
    };

    let static_randomisable_containers_on_map =
        get_randomisable_containers_on_map(config, &all_static_containers_on_map);

    // Find all 100% spawn containers
    let guaranteed_containers = get_guaranteed_containers(config, &all_static_containers_on_map);

    // Keep track of static loot count
    let mut static_container_count = guaranteed_containers.len() as i32;

    // Add loot to guaranteed containers and add to result
    for container in &guaranteed_containers {
        let container_with_loot = add_loot_to_container(
            &mut ctx,
            container,
            static_forced.as_deref(),
            &request.static_loot_dist,
            location_id,
        )?;

        static_loot_item_count += item_count(&container_with_loot);
        result.push(container_with_loot);
    }

    ctx.diagnostics.push(diagnostic(
        DEBUG,
        format!(
            "Added {} guaranteed containers",
            guaranteed_containers.len()
        ),
    ));

    // Randomisation is turned off for location / globally
    if !location_randomisation_enabled(config) {
        ctx.diagnostics.push(diagnostic(
            DEBUG,
            format!(
                "Container randomisation disabled, Adding: {} containers to: {location_id}",
                static_randomisable_containers_on_map.len()
            ),
        ));

        for container in &static_randomisable_containers_on_map {
            let container_with_loot = add_loot_to_container(
                &mut ctx,
                container,
                static_forced.as_deref(),
                &request.static_loot_dist,
                location_id,
            )?;

            static_loot_item_count += item_count(&container_with_loot);
            result.push(container_with_loot);
        }

        ctx.diagnostics.push(diagnostic(
            SUCCESS,
            format!("A total of {static_loot_item_count} static items spawned"),
        ));

        return Ok(into_result(
            ctx,
            result,
            static_loot_item_count,
            static_container_count,
        ));
    }

    // Group containers by their groupId
    let Some(statics) = statics else {
        ctx.diagnostics.push(localised(
            WARNING,
            "location-unable_to_generate_static_loot",
            json!(location_id),
        ));

        return Ok(into_result(
            ctx,
            result,
            static_loot_item_count,
            static_container_count,
        ));
    };

    // For each of the container groups, choose from the pool of containers, hydrate container with
    // loot and add to result array
    let mappings = get_group_id_to_container_mappings(
        &mut ctx,
        &statics,
        &static_randomisable_containers_on_map,
    );
    for (group_id, mut container_group_count) in mappings {
        // Count chosen was 0, skip
        if container_group_count.chosen_count == 0.0 {
            continue;
        }

        if container_group_count
            .container_ids_with_probability
            .is_empty()
        {
            ctx.diagnostics.push(diagnostic(
                DEBUG,
                format!(
                    "Group: {group_id} has no containers with < 100 % spawn chance to choose from, skipping"
                ),
            ));

            continue;
        }

        // EDGE CASE: These are containers without a group and have a probability < 100%
        if group_id.is_empty() {
            let container_ids_copy =
                std::mem::take(&mut container_group_count.container_ids_with_probability);

            // Roll each containers probability, if it passes, it gets added
            for (container_id, probability) in container_ids_copy {
                if random_util::get_chance_100(probability * 100.0) {
                    container_group_count
                        .container_ids_with_probability
                        .insert(container_id, probability);
                }
            }

            // Set desired count to size of array (we want all containers chosen)
            container_group_count.chosen_count =
                container_group_count.container_ids_with_probability.len() as f64;

            // EDGE CASE: chosen container count could be 0
            if container_group_count.chosen_count == 0.0 {
                continue;
            }
        }

        // Pass possible containers into function to choose some
        let chosen_container_ids =
            get_containers_by_probability(&mut ctx, &group_id, &container_group_count);
        for chosen_container_id in chosen_container_ids {
            // Look up container object from full list of containers on map
            let Some(container_object) = static_randomisable_containers_on_map
                .iter()
                .find(|container| template_id(container) == chosen_container_id)
            else {
                ctx.diagnostics.push(diagnostic(
                    DEBUG,
                    format!(
                        "Container: {chosen_container_id} not found in staticRandomisableContainersOnMap, this is bad"
                    ),
                ));

                continue;
            };

            // Add loot to container and push into result object
            let container_with_loot = add_loot_to_container(
                &mut ctx,
                container_object,
                static_forced.as_deref(),
                &request.static_loot_dist,
                location_id,
            )?;
            static_container_count += 1;

            static_loot_item_count += item_count(&container_with_loot);
            result.push(container_with_loot);
        }
    }

    ctx.diagnostics.push(diagnostic(
        SUCCESS,
        format!("A total of: {static_loot_item_count} static items spawned"),
    ));
    ctx.diagnostics.push(localised(
        SUCCESS,
        "location-containers_generated_success",
        json!(static_container_count),
    ));

    Ok(into_result(
        ctx,
        result,
        static_loot_item_count,
        static_container_count,
    ))
}

/// `LocationLootGenerator.LocationRandomisationEnabled` (`:273-277`) — the map lookup is resolved by
/// the C# caller into `location_in_randomisation_maps`.
fn location_randomisation_enabled(config: &LootConfigView) -> bool {
    config.container_randomisation_enabled && config.location_in_randomisation_maps
}

/// `LocationLootGenerator.GetRandomisableContainersOnMap` (`:284-293`).
fn get_randomisable_containers_on_map<'a>(
    config: &LootConfigView,
    static_containers: &'a [StaticContainerData],
) -> Vec<&'a StaticContainerData> {
    static_containers
        .iter()
        .filter(|static_container| {
            static_container.probability != Some(1.0)
                && !static_container
                    .template
                    .as_ref()
                    .and_then(|template| template.is_always_spawn)
                    .unwrap_or(false)
                && !first_item_tpl(static_container)
                    .is_some_and(|tpl| config.container_types_to_not_randomise.contains(tpl))
        })
        .collect()
}

/// `LocationLootGenerator.GetGuaranteedContainers` (`:300-309`) — the exact complement of
/// [`get_randomisable_containers_on_map`], so a container with no items lands in neither list twice.
fn get_guaranteed_containers<'a>(
    config: &LootConfigView,
    static_containers_on_map: &'a [StaticContainerData],
) -> Vec<&'a StaticContainerData> {
    static_containers_on_map
        .iter()
        .filter(|static_container| {
            static_container.probability == Some(1.0)
                || static_container
                    .template
                    .as_ref()
                    .and_then(|template| template.is_always_spawn)
                    .unwrap_or(false)
                || first_item_tpl(static_container)
                    .is_some_and(|tpl| config.container_types_to_not_randomise.contains(tpl))
        })
        .collect()
}

/// `LocationLootGenerator.GetContainersByProbability` (`:318-346`). `Draw` picks with replacement,
/// so a group can be handed the same container twice — that is the C# behaviour.
fn get_containers_by_probability(
    ctx: &mut LootContext,
    group_id: &str,
    container_data: &ContainerGroupCount,
) -> Vec<String> {
    let container_ids = &container_data.container_ids_with_probability;
    if container_data.chosen_count > container_ids.len() as f64 {
        ctx.diagnostics.push(diagnostic(
            DEBUG,
            format!(
                "Group: {group_id} wants: {} containers but pool only has: {}, adding what's available",
                container_data.chosen_count,
                container_ids.len()
            ),
        ));

        return container_ids.keys().cloned().collect();
    }

    // Create probability array with all possible container ids in this group and their relative
    // probability of spawning. C# also stores the probability as the object's data; nothing reads it.
    let mut container_distribution: ProbabilityObjectArray<String, ()> =
        ProbabilityObjectArray::new();
    for (container_id, value) in container_ids {
        container_distribution.add(ProbabilityObject {
            key: container_id.clone(),
            relative_probability: *value,
            data: None,
        });
    }

    container_distribution.draw(container_data.chosen_count.max(0.0) as usize)
}

/// `LocationLootGenerator.GetGroupIdToContainerMappings` (`:354-419`).
fn get_group_id_to_container_mappings(
    ctx: &mut LootContext,
    static_container_group_data: &StaticContainer,
    static_containers_on_map: &[&StaticContainerData],
) -> IndexMap<String, ContainerGroupCount> {
    let config = ctx.config;

    // Create dictionary of all group ids and choose a count of containers the map will spawn of
    // that group. Insertion-ordered, as the C# `Dictionary` is: the groups are walked again below
    // in this order, so the empty group must stay where it was added - after every group from
    // `containersGroups` and before any group only the containers mention.
    let mut mapping: IndexMap<String, ContainerGroupCount> = IndexMap::new();
    for (container_group_id, container_min_max) in static_container_group_data
        .containers_groups
        .iter()
        .flatten()
    {
        // C# reads `MinContainers!.Value` and throws on absent bounds; 0 stands in for that.
        let min = f64::from(container_min_max.min_containers.unwrap_or(0));
        let max = f64::from(container_min_max.max_containers.unwrap_or(0));

        mapping.insert(
            container_group_id.clone(),
            ContainerGroupCount {
                container_ids_with_probability: IndexMap::new(),
                chosen_count: f64::from(random_util::get_int(
                    random_util::round_half_even(min * config.container_group_min_size_multiplier)
                        as i32,
                    random_util::round_half_even(max * config.container_group_max_size_multiplier)
                        as i32,
                )),
            },
        );
    }

    // Add an empty group for containers without a group id but still have a < 100% chance to spawn.
    // Likely bad BSG data, will be fixed...eventually.
    mapping.insert(
        String::new(),
        ContainerGroupCount {
            container_ids_with_probability: IndexMap::new(),
            chosen_count: -1.0,
        },
    );

    // Iterate over all containers and add to group keyed by groupId
    // Containers without a group go into a group with the empty key: ""
    for container in static_containers_on_map {
        let container_id = template_id(container);
        let Some(group_data) = static_container_group_data
            .containers
            .as_ref()
            .and_then(|containers| containers.get(container_id))
        else {
            ctx.diagnostics.push(localised(
                ERROR,
                "location-unable_to_find_container_in_statics_json",
                json!(container_id),
            ));

            continue;
        };
        let group_id = group_data.group_id.clone().unwrap_or_default();

        if container
            .probability
            .is_some_and(|probability| probability >= 1.0)
        {
            ctx.diagnostics.push(diagnostic(
                DEBUG,
                format!(
                    "Container {container_id} with group: {group_id} had 100 % chance to spawn was picked as random container, skipping"
                ),
            ));

            continue;
        }

        mapping
            .entry(group_id)
            .or_default()
            .container_ids_with_probability
            .entry(container_id.to_owned())
            // C# reads `Probability!.Value` and throws on a null probability; 0 stands in for that.
            .or_insert(container.probability.unwrap_or(0.0));
    }

    mapping
}

/// `LocationLootGenerator.AddLootToContainer` (`:431-539`).
///
/// C# returns the cloned `StaticContainerData`; only its template is ever read back out, so that is
/// what comes back here.
fn add_loot_to_container(
    ctx: &mut LootContext,
    static_container: &StaticContainerData,
    static_forced: Option<&[StaticForced]>,
    static_loot_dist: &HashMap<String, StaticLootDetails>,
    location_name: &str,
) -> Result<SpawnpointTemplate, LootError> {
    let items_view = ctx.items_view;
    let config = ctx.config;

    let mut container_clone = static_container
        .template
        .clone()
        .ok_or_else(|| LootError::new("Static container has no template, unable to add loot"))?;
    let container_id = container_clone.id.clone().unwrap_or_default();

    // Create new unique parent id to prevent any collisions
    let parent_id = mongo_id::generate();
    let Some(container_item) = container_clone
        .items
        .as_mut()
        .and_then(|items| items.first_mut())
    else {
        // `Items.FirstOrDefault().Template` (`:440`) throws on an item-less container.
        return Err(LootError::new(format!(
            "Static container: {container_id} holds no items, unable to add loot"
        )));
    };
    let container_tpl = container_item.item.template.clone();
    container_item.item.id = parent_id.clone();
    container_clone.root = Some(parent_id.clone());

    let mut container_map =
        item_helper::get_container_mapping(items_view, &container_tpl).map_err(LootError::new)?;

    // Choose count of items to add to container
    let item_count_to_add = get_weighted_count_of_container_items(
        ctx,
        &container_tpl,
        static_loot_dist,
        location_name,
    )?;
    if item_count_to_add == 0 {
        return Ok(container_clone);
    }

    // Get all possible loot items for container
    let container_loot_pool =
        get_possible_loot_items_for_container(ctx, &container_tpl, static_loot_dist);

    // Some containers need to have items forced into it (quest keys etc.)
    let Some(static_forced) = static_forced else {
        // `staticForced.Where(...)` (`:460`) throws on the null list the caller only logged about.
        return Err(LootError::new(
            "Unable to find forced static data for map, unable to add loot to container",
        ));
    };
    let mut tpls_forced: Vec<String> = Vec::new();
    let mut forced_lookup: HashSet<String> = HashSet::new();
    for forced_static_prop in static_forced {
        if forced_static_prop.container_id == container_id
            && forced_lookup.insert(forced_static_prop.item_tpl.clone())
        {
            tpls_forced.push(forced_static_prop.item_tpl.clone());
        }
    }

    // Draw random loot
    // Allow money to spawn more than once in container
    let mut failed_to_fit_attempt_count = 0;
    let draw_count = item_count_to_add.max(0) as usize;

    // Choose items to add to container, factor in weighting + lock money down
    let drawn_tpls = if config.allow_duplicate_items_in_static_containers {
        container_loot_pool.draw(draw_count)
    } else {
        container_loot_pool.draw_and_remove(draw_count, Some(ctx.money_tpls))
    };

    // Filter out items picked that are already in the above `tplsForced` array, and count every
    // drawn tpl against its spawn limit exactly once. C# defers this filter into the loop below,
    // where an early `break` skips the remaining increments and `Any()` double-counts the first.
    let mut tpls_to_add_to_container = tpls_forced;
    for tpl in drawn_tpls {
        if forced_lookup.contains(&tpl) || increment_count(&mut ctx.counter, &tpl) {
            continue;
        }

        tpls_to_add_to_container.push(tpl);
    }

    if tpls_to_add_to_container.is_empty() {
        ctx.diagnostics.push(diagnostic(
            WARNING,
            format!("Added no items to container: {container_tpl}"),
        ));
    }

    for tpl_to_add in &tpls_to_add_to_container {
        let Some(chosen_item_with_children) =
            create_static_loot_item(ctx, tpl_to_add, Some(&parent_id))?
        else {
            continue;
        };

        // Check if item should have children removed
        let mut items = if config.tpls_to_strip_child_items_from.contains(tpl_to_add) {
            // Strip children from parent
            chosen_item_with_children
                .items
                .into_iter()
                .take(1)
                .collect()
        } else {
            chosen_item_with_children.items
        };
        if items.is_empty() {
            // `items.First()` (`:525`) throws; only an empty armor preset can get here.
            continue;
        }

        let size = match (
            chosen_item_with_children.width,
            chosen_item_with_children.height,
        ) {
            (Some(width), Some(height)) if width > 0 && height > 0 => Some((width, height)),
            _ => None,
        };

        // look for open slot to put chosen item into
        let result = match size {
            Some((width, height)) => find_slot_for_item(&container_map, width, height),
            // C# hands the nullable size straight to `FindSlotForItem` (`:499`), where every loop
            // bound becomes null, the scan never runs and a miss comes back. A 0 would walk off the
            // grid here, so it joins the same path.
            None => FindSlotResult::default(),
        };
        if !result.success {
            if failed_to_fit_attempt_count > config.fit_loot_into_container_attempts {
                // x attempts to fit an item, container is probably full, stop trying to add more
                break;
            }

            // Can't fit item, skip
            failed_to_fit_attempt_count += 1;

            continue;
        }

        // Find somewhere for item inside container. The C# discards both the result and the partial
        // marks a collision would leave behind (`out _`), and so does this.
        if let Some((width, height)) = size {
            try_fill_container_map_with_item(
                &mut container_map,
                result.x,
                result.y,
                width,
                height,
                result.rotation,
            );
        }

        // Update root item properties with result of position finder
        items[0].slot_id = Some("main".to_owned());
        items[0].location = serde_json::to_value(ItemLocation {
            x: Some(result.x),
            y: Some(result.y),
            r: if result.rotation {
                ItemRotation::Vertical
            } else {
                ItemRotation::Horizontal
            },
            ..Default::default()
        })
        .ok();

        // Add loot to container before returning. C# `Union`s, which cannot drop anything here —
        // every item carries a fresh id.
        container_clone
            .items
            .get_or_insert_default()
            .extend(items.iter().map(item_helper::to_loot_item));
    }

    Ok(container_clone)
}

/// `LocationLootGenerator.GetWeightedCountOfContainerItems` (`:548-578`).
fn get_weighted_count_of_container_items(
    ctx: &mut LootContext,
    container_type_id: &str,
    static_loot_dist: &HashMap<String, StaticLootDetails>,
    location_name: &str,
) -> Result<i32, LootError> {
    let Some(container_loot) = static_loot_dist.get(container_type_id) else {
        // `staticLootDist[containerTypeId]` (`:556`) throws KeyNotFoundException.
        return Err(LootError::new(format!(
            "Container: {container_type_id} is missing from staticLoot.json"
        )));
    };

    let Some(count_distribution) = container_loot.item_count_distribution.as_deref() else {
        ctx.diagnostics.push(localised(
            WARNING,
            "location-unable_to_find_count_distribution_for_container",
            json!({ "containerId": container_type_id, "locationName": location_name }),
        ));

        return Ok(0);
    };

    // Create probability array to calculate the total count of lootable items inside container
    let mut item_count_array: ProbabilityObjectArray<i32, ()> = ProbabilityObjectArray::new();
    for item_count_distribution in count_distribution {
        // Add each count of items into array. C# reads both `.Value`s and throws when either is
        // absent; 0 stands in for that.
        item_count_array.add(ProbabilityObject {
            key: item_count_distribution.count.unwrap_or(0),
            relative_probability: item_count_distribution.relative_probability.unwrap_or(0.0),
            data: None,
        });
    }

    let Some(drawn_count) = item_count_array.draw(1).into_iter().next() else {
        // `itemCountArray.Draw()[0]` (`:577`) index-crashes on the empty draw an all-zero (or
        // empty) pool produces.
        return Err(LootError::new(format!(
            "Unable to draw an item count for container: {container_type_id}"
        )));
    };

    Ok(
        random_util::round_half_even(ctx.config.static_loot_multiplier * f64::from(drawn_count))
            as i32,
    )
}

/// `LocationLootGenerator.GetPossibleLootItemsForContainer` (`:587-623`).
fn get_possible_loot_items_for_container(
    ctx: &mut LootContext,
    container_type_id: &str,
    static_loot_dist: &HashMap<String, StaticLootDetails>,
) -> ProbabilityObjectArray<String, ()> {
    let mut item_distribution: ProbabilityObjectArray<String, ()> = ProbabilityObjectArray::new();

    let Some(static_loot) = static_loot_dist
        .get(container_type_id)
        .and_then(|details| details.item_distribution.as_deref())
    else {
        ctx.diagnostics.push(localised(
            WARNING,
            "location-missing_item_distribution_data",
            json!(container_type_id),
        ));

        return item_distribution;
    };

    let seasonal_event_active = ctx.seasonal.seasonal_event_active;
    for item_with_probability in static_loot {
        if !seasonal_event_active
            && ctx
                .seasonal
                .inactive_seasonal_items
                .contains(&item_with_probability.tpl)
        {
            // Prevent seasonal loot when not inside season
            continue;
        }

        if ctx
            .lootable_item_blacklist
            .contains(&item_with_probability.tpl)
        {
            // Prevent non-loot items getting into pool
            continue;
        }

        // C# reads `RelativeProbability!.Value` and throws when absent; 0 stands in for that.
        item_distribution.add(ProbabilityObject {
            key: item_with_probability.tpl.clone(),
            relative_probability: item_with_probability.relative_probability.unwrap_or(0.0),
            data: None,
        });
    }

    item_distribution
}

/// The spawn point's template id. C# dereferences `Template.Id` unguarded (`:669-672`); an absent
/// one is simply not a christmas point and not blacklisted here, which keeps the null-template
/// warning at `:771` reachable.
fn spawn_point_template_id(spawn_point: &Spawnpoint) -> &str {
    spawn_point
        .template
        .as_ref()
        .and_then(|template| template.id.as_deref())
        .unwrap_or_default()
}

/// `Template.Id.StartsWith("christmas", OrdinalIgnoreCase)` (`:668-673`).
fn is_christmas_spawn_point(spawn_point: &Spawnpoint) -> bool {
    spawn_point_template_id(spawn_point)
        .get(..9)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("christmas"))
}

/// `Template.IsAlwaysSpawn.GetValueOrDefault()` (`:678,711`).
fn is_always_spawn(spawn_point: &Spawnpoint) -> bool {
    spawn_point
        .template
        .as_ref()
        .and_then(|template| template.is_always_spawn)
        .unwrap_or(false)
}

/// `LocationLootGenerator.GenerateDynamicLoot` (`:656-854`) — forced loot, then a weighted draw of
/// loose spawn points, one item generated per chosen point.
///
/// C# mutates `dynamicLootDist` as it goes (the christmas filter, and each chosen point's `Items` /
/// `Root`). This owns a deserialized copy, so the caller's `LooseLoot` comes back untouched — the
/// one documented behaviour change of the port.
pub fn generate_dynamic_loot(
    mut request: DynamicLootRequest,
) -> Result<DynamicLootResult, LootError> {
    // `resume`, not `install`: this is the second half of one `GenerateLocationLoot`, and the C#
    // draws both halves from the single `SeededRandomSource` the caller installed, so the stream
    // carries on from where the static-container run ended.
    let _seed_guard = request
        .common
        .test_seed
        .map(random_util::TestSeedGuard::resume);

    // Everything the run mutates is moved out before the rest of the request is lent to the context.
    let counter = std::mem::take(&mut request.common.counter);
    let loose_loot = std::mem::take(&mut request.loose_loot);

    let mut ctx = loot_context(&request.common, counter);
    let location_name = request.common.location_id.as_str();
    let config = ctx.config;
    let seasonal = ctx.seasonal;

    // C# enumerates all three unguarded (`:668-685`) and throws on a null without logging first.
    let (Some(mut spawnpoints), Some(mut spawnpoints_forced), Some(spawnpoint_count)) = (
        loose_loot.spawnpoints,
        loose_loot.spawnpoints_forced,
        loose_loot.spawnpoint_count,
    ) else {
        return Err(LootError::new(format!(
            "Loose loot data for map: {location_name} is incomplete"
        )));
    };

    // Remove christmas items from loot data
    if !seasonal.christmas_event_enabled {
        spawnpoints.retain(|spawn_point| !is_christmas_spawn_point(spawn_point));
        spawnpoints_forced.retain(|spawn_point| !is_christmas_spawn_point(spawn_point));
    }

    // Build the list of forced loot from both `SpawnpointsForced` and any point marked
    // `IsAlwaysSpawn`. C# shares the point objects between the two lists; the copies here never
    // diverge, since the main loop skips always-spawn points (`:711`).
    let mut dynamic_forced_spawn_points = spawnpoints_forced;
    dynamic_forced_spawn_points.extend(
        spawnpoints
            .iter()
            .filter(|spawn_point| is_always_spawn(spawn_point))
            .cloned(),
    );

    let mut loot = get_forced_dynamic_loot(&mut ctx, dynamic_forced_spawn_points, location_name)?;

    // Draw from random distribution
    let desired_spawn_point_count = random_util::round_half_even(
        config.loose_loot_multiplier
            * random_util::get_normally_distributed_random_number(
                spawnpoint_count.mean,
                spawnpoint_count.std,
            ),
    );

    // Init empty array to hold spawn points, letting us pick them pseudo-randomly
    let mut spawn_point_array: ProbabilityObjectArray<String, Spawnpoint> =
        ProbabilityObjectArray::new();

    // Positions not in forced but have 100% chance to spawn
    let mut guaranteed_loose_points: Vec<Spawnpoint> = Vec::new();

    for spawn_point in spawnpoints {
        let template_id = spawn_point_template_id(&spawn_point).to_owned();

        // Point is blacklisted, skip
        if config.loose_loot_blacklist.contains(&template_id) {
            ctx.diagnostics.push(diagnostic(
                DEBUG,
                format!("Ignoring loose loot location: {template_id}"),
            ));

            continue;
        }

        // We've handled IsAlwaysSpawn above, so skip them
        if is_always_spawn(&spawn_point) {
            continue;
        }

        // 100%, add it to guaranteed
        if spawn_point.probability == Some(1.0) {
            guaranteed_loose_points.push(spawn_point);

            continue;
        }

        spawn_point_array.add(ProbabilityObject {
            key: template_id,
            // C# reads `Probability ?? 0`.
            relative_probability: spawn_point.probability.unwrap_or(0.0),
            data: Some(spawn_point),
        });
    }

    // Select a number of spawn points to add loot to
    // Add ALL loose loot with 100% chance to pool
    let guaranteed_loose_point_count = guaranteed_loose_points.len();
    let mut chosen_spawn_points = guaranteed_loose_points;

    let random_spawn_point_count = desired_spawn_point_count - chosen_spawn_points.len() as f64;
    // Only draw random spawn points if needed
    if random_spawn_point_count > 0.0 && !spawn_point_array.is_empty() {
        // Add randomly chosen spawn points. `DrawAndRemove` only removes from its own working copy,
        // which is why `Data` can still find every key it just drew (`:736-738`).
        for key in spawn_point_array.draw_and_remove(random_spawn_point_count as usize, None) {
            if let Some(spawn_point) = spawn_point_array.data(&key) {
                chosen_spawn_points.push(spawn_point.clone());
            }
        }
    }

    // Filter out duplicate locationIds // prob can be done better
    let mut seen_location_ids: HashSet<Option<String>> = HashSet::new();
    chosen_spawn_points
        .retain(|spawn_point| seen_location_ids.insert(spawn_point.location_id.clone()));

    // Do we have enough items in pool to fulfill requirement
    if desired_spawn_point_count - chosen_spawn_points.len() as f64 > 0.0 {
        ctx.diagnostics.push(localised(
            DEBUG,
            "location-spawn_point_count_requested_vs_found",
            json!({
                "requested": desired_spawn_point_count + guaranteed_loose_point_count as f64,
                "found": chosen_spawn_points.len(),
                "mapName": location_name,
            }),
        ));
    }

    // Iterate over spawnPoints
    let seasonal_event_active = seasonal.seasonal_event_active;
    for mut spawn_point in chosen_spawn_points {
        // SpawnPoint is invalid, skip it
        let Some(mut spawn_point_template) = spawn_point.template.take() else {
            ctx.diagnostics.push(localised(
                WARNING,
                "location-missing_dynamic_template",
                json!(spawn_point.location_id),
            ));

            continue;
        };

        // Ensure no blacklisted lootable items are in pool. C# enumerates a null `Items` and throws
        // (`:779`); an absent pool takes the empty-pool path below instead.
        let mut items: Vec<SptLootItem> = spawn_point_template
            .items
            .take()
            .unwrap_or_default()
            .into_iter()
            .filter(|item| !ctx.lootable_item_blacklist.contains(&item.item.template))
            .collect();

        // Ensure no seasonal items are in pool if not in-season
        if !seasonal_event_active {
            items.retain(|item| {
                !seasonal
                    .inactive_seasonal_items
                    .contains(&item.item.template)
            });
        }

        // Spawn point has no items after filtering, skip
        if items.is_empty() {
            ctx.diagnostics.push(localised(
                DEBUG,
                "location-spawnpoint_missing_items",
                json!(spawn_point_template.id),
            ));

            continue;
        }

        // Get an array of allowed IDs after above filtering has occured. C# throws on an item
        // distribution entry with no `composedKey` object at all (`:807`); it reads as the null key
        // here, which the pool below treats the same as a missing one.
        let valid_composed_keys: HashSet<&str> = items
            .iter()
            .map(|item| item.composed_key.as_deref().unwrap_or_default())
            .collect();

        // Construct container to hold above filtered items, letting us pick an item for the spot
        let mut item_array: ProbabilityObjectArray<String, ()> = ProbabilityObjectArray::new();
        for item_distribution in spawn_point.item_distribution.iter().flatten() {
            let composed_key = item_distribution
                .composed_key
                .as_ref()
                .and_then(|composed_key| composed_key.key.as_deref())
                .unwrap_or_default();
            if !valid_composed_keys.contains(composed_key) {
                continue;
            }

            item_array.add(ProbabilityObject {
                key: composed_key.to_owned(),
                relative_probability: item_distribution.relative_probability.unwrap_or(0.0),
                data: None,
            });
        }

        if item_array.is_empty() {
            ctx.diagnostics.push(localised(
                WARNING,
                "location-loot_pool_is_empty_skipping",
                json!(spawn_point_template.id),
            ));

            continue;
        }

        // Draw a random item from the spawn points possible items. An all-zero pool draws nothing,
        // where C# would fall back to `FirstOrDefault`'s null and match an item with a null
        // composed key; both end up on the warning below for every pool that has one.
        let chosen_composed_key = item_array.draw(1).into_iter().next();
        let chosen_item = chosen_composed_key.as_deref().and_then(|composed_key| {
            items
                .iter()
                .find(|item| item.composed_key.as_deref().unwrap_or_default() == composed_key)
        });
        let Some(chosen_item) = chosen_item else {
            ctx.diagnostics.push(diagnostic(
                WARNING,
                format!(
                    "Unable to find item with composed key: {}, skipping spawn point: {} ",
                    chosen_composed_key.unwrap_or_default(),
                    spawn_point.location_id.unwrap_or_default()
                ),
            ));

            continue;
        };

        let create_item_result = create_dynamic_loot_item(&mut ctx, chosen_item, &items)?;

        // `Items.FirstOrDefault().Template` (`:836`) throws on the empty list an item with no
        // children in the pool leaves behind; the point is skipped instead.
        let Some(root_item) = create_item_result.items.first() else {
            continue;
        };

        // If count reaches max, skip adding item to loot
        if increment_count(&mut ctx.counter, &root_item.template) {
            continue;
        }

        // Root id can change when generating a weapon, ensure ids match
        spawn_point_template.root = Some(root_item.id.clone());

        // Convert the processed items into the correct output type, overwriting the entire pool
        // with the chosen item
        spawn_point_template.items = Some(
            create_item_result
                .items
                .iter()
                .map(item_helper::to_loot_item)
                .collect(),
        );

        loot.push(spawn_point_template);
    }

    Ok(DynamicLootResult {
        spawnpoints: loot,
        tracked_counts: ctx.counter.tracked_counts,
    })
}

/// `LocationLootGenerator.GetForcedDynamicLoot` (`:863-919`) — force items into loot spawn points,
/// primarily quest items.
fn get_forced_dynamic_loot(
    ctx: &mut LootContext,
    forced_spawn_points: Vec<Spawnpoint>,
    location_name: &str,
) -> Result<Vec<SpawnpointTemplate>, LootError> {
    let seasonal = ctx.seasonal;
    let seasonal_event_active = seasonal.seasonal_event_active;

    let mut result: Vec<SpawnpointTemplate> = Vec::new();

    for forced_loot_location in forced_spawn_points {
        // C# dereferences the template (`:877`) and its first item (`:879`) unguarded; a point
        // missing either is skipped here.
        let Some(mut location_template_to_add) = forced_loot_location.template else {
            continue;
        };
        let items = location_template_to_add.items.take().unwrap_or_default();
        // `Items.FirstOrDefault(item => item.Id == rootItem.Id)` (`:890`) can only ever find the
        // root item itself, so the two are one and the same here.
        let Some(chosen_item) = items.first() else {
            continue;
        };

        // Counted before the seasonal check below, so a skipped seasonal point still spends a slot.
        if increment_count(&mut ctx.counter, &chosen_item.item.template) {
            continue;
        }

        // Skip adding seasonal items when seasonal event is not active
        if !seasonal_event_active
            && seasonal
                .inactive_seasonal_items
                .contains(&chosen_item.item.template)
        {
            continue;
        }

        let create_item_result = create_dynamic_loot_item(ctx, chosen_item, &items)?;

        // `Items.FirstOrDefault().Id` (`:894`) throws on an empty list; the point is skipped instead.
        let Some(root_item) = create_item_result.items.first() else {
            continue;
        };

        // Update root ID with the above dynamically generated ID
        location_template_to_add.root = Some(root_item.id.clone());

        // Convert the processed items into the correct output type
        location_template_to_add.items = Some(
            create_item_result
                .items
                .iter()
                .map(item_helper::to_loot_item)
                .collect(),
        );

        // Push forced location into array as long as it doesn't exist already
        if result
            .iter()
            .any(|spawn_point| spawn_point.id == location_template_to_add.id)
        {
            ctx.diagnostics.push(diagnostic(
                DEBUG,
                format!(
                    "Attempted to add a forced loot location with Id: {} to map {location_name} that already has that id in use, skipping",
                    location_template_to_add.id.as_deref().unwrap_or_default()
                ),
            ));

            continue;
        }

        result.push(location_template_to_add);
    }

    Ok(result)
}

/// `LocationLootGenerator.CreateDynamicLootItem` (`:928-1015`) — the item that lands in a loose loot
/// position, with its children.
fn create_dynamic_loot_item(
    ctx: &mut LootContext,
    chosen_item: &SptLootItem,
    loot_items: &[SptLootItem],
) -> Result<ContainerItem, LootError> {
    let items_view = ctx.items_view;
    let config = ctx.config;
    let chosen_tpl = chosen_item.item.template.as_str();

    let Some(item_db_template) = item_helper::get_item(items_view, chosen_tpl) else {
        ctx.diagnostics.push(diagnostic(
            ERROR,
            format!("Item tpl: {chosen_tpl} cannot be found in database"),
        ));

        // C# logs the line above (`:939`) and carries on, but it cannot get out of the method: the
        // base-class gates below all answer false for a tpl the database has never heard of
        // (`ItemBaseClassService.cs:97-102`), so it reaches `GetItemSize`, which returns null for an
        // unknown root template (`ItemHelper.cs:1187-1190`), and `size.Width` (`:1012`) throws.
        return Err(LootError::new(format!(
            "Item tpl: {chosen_tpl} cannot be found in database"
        )));
    };

    // Item array to return
    let mut item_with_mods: Vec<Item> = Vec::new();

    // Money/Ammo - don't rely on items in spawnPoint.template.Items so we can randomise it ourselves
    if item_helper::is_of_baseclasses(
        items_view,
        chosen_tpl,
        &[item_helper::MONEY, item_helper::AMMO],
    ) {
        // C# reads both `.Value`s and throws when either is absent; 0 stands in for that.
        let stack_count = if item_db_template.stack_max_size == Some(1) {
            1
        } else {
            random_util::get_int(
                item_db_template.stack_min_random.unwrap_or(0),
                item_db_template.stack_max_random.unwrap_or(0),
            )
        };

        item_with_mods.push(Item {
            id: mongo_id::generate(),
            template: chosen_tpl.to_owned(),
            upd: Some(Upd {
                stack_objects_count: Some(f64::from(stack_count)),
                ..Default::default()
            }),
            ..Default::default()
        });
    } else if item_helper::is_of_baseclass(items_view, chosen_tpl, item_helper::AMMO_BOX) {
        // Fill with cartridges
        let mut ammo_box_item = vec![Item {
            id: mongo_id::generate(),
            template: chosen_tpl.to_owned(),
            ..Default::default()
        }];

        // Both failures are C# crashes inside `AddCartridgesToAmmoBox`; unlike the static path,
        // there is no null to return here, so the run stops with them.
        item_helper::add_cartridges_to_ammo_box(ctx.items_view, &mut ammo_box_item, chosen_tpl)
            .map_err(|failure| LootError::new(failure.message.unwrap_or_default()))?;

        item_with_mods.extend(ammo_box_item);
    } else if item_helper::is_of_baseclass(items_view, chosen_tpl, item_helper::MAGAZINE) {
        // Create array with just magazine
        let mut magazine_item = vec![Item {
            id: mongo_id::generate(),
            template: chosen_tpl.to_owned(),
            ..Default::default()
        }];

        // Yes: the loose path gates on the *static* chance and then fills to the *loose* minimum
        // (`:974-983`).
        if random_util::get_chance_100(config.static_magazine_loot_has_ammo_chance_percent) {
            // Add randomised amount of cartridges
            item_helper::fill_magazine_with_random_cartridge(
                ctx,
                &mut magazine_item,
                chosen_tpl,
                None,
                config.min_fill_loose_magazine_percent / 100.0,
                None,
                None,
            )?;
        }

        item_with_mods.extend(magazine_item);
    } else {
        // Also used by armors to get child mods
        // Get item + children and add into array we return. `GetItemWithChildren` reads the base
        // `Item` of each loot item, and clones as it walks, which is the C# `cloner.Clone` too.
        let pool: Vec<Item> = loot_items.iter().map(|item| item.item.clone()).collect();
        let mut item_with_children =
            item_helper::get_item_with_children(&pool, &chosen_item.item.id);

        // Ensure all IDs are unique
        item_helper::replace_ids(&mut item_with_children);

        if config.tpls_to_strip_child_items_from.contains(chosen_tpl) {
            // Strip children from parent before adding
            item_with_children.truncate(1);
        }

        item_with_mods.extend(item_with_children);
    }

    // Get inventory size of item. `itemWithMods.FirstOrDefault().Id` (`:1007`) throws on the empty
    // list an unknown root leaves behind; the size comes out unset here and both callers skip the
    // spawn point on the same empty list.
    let size = item_with_mods.first().and_then(|root_item| {
        item_helper::get_item_size(items_view, &item_with_mods, &root_item.id)
    });

    Ok(ContainerItem {
        items: item_with_mods,
        width: size.map(|(width, _)| width),
        height: size.map(|(_, height)| height),
    })
}

/// `LocationLootGenerator.CreateStaticLootItem` (`:1025-1094`) — HIGHLY BRITTLE, LEGACY CODE.
fn create_static_loot_item(
    ctx: &mut LootContext,
    chosen_tpl: &str,
    parent_id: Option<&str>,
) -> Result<Option<ContainerItem>, LootError> {
    let items_view = ctx.items_view;
    let config = ctx.config;

    let Some(item_template) = item_helper::get_item(items_view, chosen_tpl) else {
        ctx.diagnostics.push(diagnostic(
            ERROR,
            format!("Unable to process item: {chosen_tpl}. it lacks _props"),
        ));

        return Ok(None);
    };

    let mut width = item_template.width;
    let mut height = item_template.height;
    let mut items = vec![Item {
        id: mongo_id::generate(),
        template: chosen_tpl.to_owned(),
        // Use passed in parentId as override for new item
        parent_id: parent_id
            .filter(|parent_id| !parent_id.is_empty())
            .map(str::to_owned),
        ..Default::default()
    }];

    if item_helper::is_of_baseclass(items_view, chosen_tpl, item_helper::MONEY)
        || item_helper::is_of_baseclass(items_view, chosen_tpl, item_helper::AMMO)
    {
        // Money needs its stack size randomised.
        // Edge case - some ammos e.g. flares or M406 grenades shouldn't be stacked
        let stack_count = if item_template.stack_max_size == Some(1) {
            1
        } else {
            random_util::get_int(
                item_template.stack_min_random.unwrap_or(0),
                item_template.stack_max_random.unwrap_or(0),
            )
        };

        items[0].upd = Some(Upd {
            stack_objects_count: Some(f64::from(stack_count)),
            ..Default::default()
        });
    } else if item_helper::is_of_baseclass(items_view, chosen_tpl, item_helper::WEAPON) {
        // No spawn point, use default template
        let root_item = create_weapon_root_and_children(ctx, chosen_tpl, parent_id, &mut items)?;

        let size = item_helper::get_item_size(items_view, &items, &root_item.id);
        width = size.map(|(width, _)| width);
        height = size.map(|(_, height)| height);
    } else if item_helper::is_of_baseclass(items_view, chosen_tpl, item_helper::AMMO_BOX) {
        // No spawnPoint to fall back on, generate manually
        if let Err(failure) =
            item_helper::add_cartridges_to_ammo_box(ctx.items_view, &mut items, chosen_tpl)
        {
            // The C# equivalents are crashes; reported and the box skipped instead.
            ctx.diagnostics.push(failure);

            return Ok(None);
        }
    } else if item_helper::is_of_baseclass(items_view, chosen_tpl, item_helper::MAGAZINE) {
        if random_util::get_chance_100(config.magazine_loot_has_ammo_chance_percent) {
            // Create array with just magazine
            generate_static_magazine_item(ctx, &mut items, chosen_tpl)?;
        }
    } else if item_helper::armor_item_can_hold_mods(items_view, chosen_tpl) {
        items = get_armor_items(ctx, chosen_tpl, items);
    }

    Ok(Some(ContainerItem {
        items,
        width,
        height,
    }))
}

/// `LocationLootGenerator.GetArmorItems` (`:1104-1126`). C# also takes the root item and the armor's
/// db template; both are derivable here — the root is always `items[0]`.
fn get_armor_items(ctx: &mut LootContext, chosen_tpl: &str, items: Vec<Item>) -> Vec<Item> {
    let items_view = ctx.items_view;
    let default_presets = ctx.default_presets;
    let config = ctx.config;

    if let Some(default_preset) = default_presets.get(chosen_tpl) {
        let mut preset_and_mods_clone = default_preset.items.clone();
        item_helper::replace_ids(&mut preset_and_mods_clone);
        item_helper::remap_root_item_id(&mut preset_and_mods_clone);

        // Use original items parentId otherwise item doesn't get added to container correctly
        let root_parent_id = items.first().and_then(|item| item.parent_id.clone());
        if let Some(preset_root) = preset_and_mods_clone.first_mut() {
            preset_root.parent_id = root_parent_id;
        }

        return preset_and_mods_clone;
    }

    // We make base item in calling method, no need to do it here
    let has_slots = item_helper::get_item(items_view, chosen_tpl)
        .and_then(|armor_db_template| armor_db_template.slots.as_ref())
        .is_some_and(|slots| !slots.is_empty());
    if has_slots {
        return item_helper::add_child_slot_items(
            items_view,
            &mut ctx.diagnostics,
            items,
            chosen_tpl,
            Some(&config.mod_spawn_chance_percent),
        );
    }

    items
}

/// `LocationLootGenerator.CreateWeaponRootAndChildren` (`:1137-1238`). Every C# failure inside it is
/// logged and rethrown, so each one is a [`LootError`] here.
fn create_weapon_root_and_children(
    ctx: &mut LootContext,
    chosen_tpl: &str,
    parent_id: Option<&str>,
    items: &mut Vec<Item>,
) -> Result<Item, LootError> {
    let items_view = ctx.items_view;
    let default_presets = ctx.default_presets;
    let mut children: Vec<Item> = Vec::new();

    // Look up a default preset for desired weapon tpl
    if let Some(default_preset) = default_presets.get(chosen_tpl) {
        let mut preset_items = default_preset.items.clone();
        let Some(preset_root) = preset_items.first().cloned() else {
            // `ReparentItemAndChildren` indexes `[0]`; C# logs this and rethrows (`:1152-1173`).
            // The preset's id and name are not part of the view the caller sends.
            ctx.diagnostics.push(localised(
                ERROR,
                "location-preset_not_found",
                json!({
                    "tpl": chosen_tpl,
                    "defaultId": null,
                    "defaultName": null,
                    "parentId": parent_id,
                }),
            ));

            return Err(LootError::new(format!(
                "preset not found for {chosen_tpl}, parentId: {}",
                parent_id.unwrap_or_default()
            )));
        };

        children = item_helper::reparent_item_and_children(&preset_root, &mut preset_items);
    } else {
        // RSP30 doesn't have any default presets and kills the code below as it has no children to
        // re-parent
        ctx.diagnostics.push(diagnostic(
            DEBUG,
            format!("createStaticLootItem() No preset found for weapon: {chosen_tpl}"),
        ));
    }

    let Some(root_item) = items.first().cloned() else {
        ctx.diagnostics.push(localised(
            ERROR,
            "location-missing_root_item",
            json!({ "tpl": chosen_tpl, "parentId": parent_id }),
        ));

        return Err(LootError::new(crate::diag::localise(
            "location-critical_error_see_log",
            None,
        )));
    };

    if !children.is_empty() {
        *items = item_helper::reparent_item_and_children(&root_item, &mut children);
    }

    // Here we should use generalized BotGenerators functions e.g. fillExistingMagazines in the
    // future since it can handle revolver ammo (it's not restructured to be used here yet.)
    // some weapon presets come without magazine; only fill the mag if it exists
    let Some(magazine_index) = items
        .iter()
        .position(|item| item.slot_id.as_deref() == Some("mod_magazine"))
    else {
        return Ok(root_item);
    };

    // Create array with just magazine, then replace the existing magazine with it. C# removes it
    // after filling, which lands it in the same place: the end of the list.
    let magazine = items.remove(magazine_index);
    let mag_tpl = magazine.template.clone();
    let mut magazine_with_cartridges = vec![magazine];

    // C# dereferences both templates' `Properties`; an unknown tpl leaves the caliber and the
    // fallback ammo unset here instead, which the fill handles.
    let caliber = item_helper::get_item(items_view, chosen_tpl)
        .and_then(|weapon_template| weapon_template.ammo_caliber.clone());
    let default_ammo = item_helper::get_item(items_view, &root_item.template)
        .and_then(|default_weapon| default_weapon.def_ammo.clone());

    item_helper::fill_magazine_with_random_cartridge(
        ctx,
        &mut magazine_with_cartridges,
        &mag_tpl,
        caliber.as_deref(),
        0.25,
        default_ammo.as_deref(),
        Some(&root_item.template),
    )?;

    items.extend(magazine_with_cartridges);

    Ok(root_item)
}

/// `LocationLootGenerator.GenerateStaticMagazineItem` (`:1247-1266`). C# passes the root item in
/// separately; the only call site's root is `items[0]`.
fn generate_static_magazine_item(
    ctx: &mut LootContext,
    items: &mut Vec<Item>,
    item_tpl: &str,
) -> Result<(), LootError> {
    let min_fill_percent = ctx.config.min_fill_static_magazine_percent;

    let Some(root_item) = items.first().cloned() else {
        return Ok(());
    };
    let mut magazine_with_cartridges = vec![root_item];

    item_helper::fill_magazine_with_random_cartridge(
        ctx,
        &mut magazine_with_cartridges,
        item_tpl,
        None,
        min_fill_percent / 100.0,
        None,
        None,
    )?;

    // Replace existing magazine with above array
    items.remove(0);
    items.extend(magazine_with_cartridges);

    Ok(())
}

/// `Helpers/InRaid/CounterTrackerHelper.IncrementCount` (`:27-39`) — true once the key is over its
/// max. The only call site increments by the default of 1.
fn increment_count(counter: &mut CounterState, key: &str) -> bool {
    // Not tracked, skip
    let Some(max_count) = counter.max_counts.get(key) else {
        return false;
    };

    let tracked_count = counter.tracked_counts.entry(key.to_owned()).or_insert(0);
    *tracked_count += 1;

    *tracked_count > *max_count
}

#[cfg(test)]
mod tests {
    use super::*;

    use serde_json::json;

    use crate::loot::item_helper::{AMMO, AMMO_BOX, ARMOR, MAGAZINE, MONEY, WEAPON};
    use crate::loot::models::ContainerData;
    use crate::loot::mongo_id;

    /// `BaseClasses.ITEM` — the root node the base classes below hang off.
    const ITEM_NODE: &str = "54009119af1c881c07000029";
    const CONTAINER_TPL: &str = "111111111111111111111111";
    const FORCED_TPL: &str = "222222222222222222222222";
    const MONEY_TPL: &str = "333333333333333333333333";
    const AMMO_BOX_TPL: &str = "444444444444444444444444";
    const MAGAZINE_TPL: &str = "555555555555555555555555";
    const CARTRIDGE_TPL: &str = "666666666666666666666666";
    const WEAPON_TPL: &str = "777777777777777777777777";
    const WEAPON_MOD_TPL: &str = "888888888888888888888888";
    const ARMOR_TPL: &str = "999999999999999999999999";
    const ARMOR_MOD_TPL: &str = "aaaaaaaaaaaaaaaaaaaaaaaa";
    const PLAIN_TPL: &str = "bbbbbbbbbbbbbbbbbbbbbbbb";
    const SEASONAL_TPL: &str = "cccccccccccccccccccccccc";
    const CALIBER: &str = "Caliber762x39";

    /// A container spawn point: probability, spawn-point id, and the container item itself.
    fn container(id: &str, probability: f64, always_spawn: bool) -> serde_json::Value {
        json!({
            "probability": probability,
            "template": {
                "Id": id,
                "IsContainer": true,
                "IsAlwaysSpawn": always_spawn,
                "Root": mongo_id::generate(),
                "Items": [{ "_id": mongo_id::generate(), "_tpl": CONTAINER_TPL }],
            },
        })
    }

    /// Two guaranteed containers (`c1` at 100%, `c2` flagged always-spawn), three randomisable ones
    /// in a single group of exactly two, a forced item in `c1`, and a 5x3 container grid.
    fn fixture_request() -> StaticContainersRequest {
        serde_json::from_value(json!({
            "locationId": "bigmap",
            "itemsView": {
                ITEM_NODE: {},
                MONEY: { "parent": ITEM_NODE },
                AMMO: { "parent": ITEM_NODE },
                AMMO_BOX: { "parent": ITEM_NODE },
                MAGAZINE: { "parent": ITEM_NODE },
                WEAPON: { "parent": ITEM_NODE },
                CONTAINER_TPL: {
                    "parent": ITEM_NODE, "width": 1, "height": 1,
                    "gridCellsH": 5, "gridCellsV": 3
                },
                FORCED_TPL: { "parent": ITEM_NODE, "width": 1, "height": 1 },
                MONEY_TPL: {
                    "parent": MONEY, "width": 1, "height": 1,
                    "stackMaxSize": 500000, "stackMinRandom": 100, "stackMaxRandom": 200
                },
                AMMO_BOX_TPL: {
                    "parent": AMMO_BOX, "width": 2, "height": 1,
                    "stackSlotMaxCount": 60, "stackSlotFirstFilterFirst": CARTRIDGE_TPL
                },
                MAGAZINE_TPL: {
                    "parent": MAGAZINE, "width": 1, "height": 2,
                    "cartridgesMaxCount": 30, "cartridgesFirstFilter": [CARTRIDGE_TPL]
                },
                CARTRIDGE_TPL: { "parent": AMMO, "width": 1, "height": 1,
                    "stackMaxSize": 30, "caliber": CALIBER },
                WEAPON_TPL: {
                    "parent": WEAPON, "width": 2, "height": 1,
                    "ammoCaliber": CALIBER, "defAmmo": CARTRIDGE_TPL,
                    "chambersFirstFilter": [CARTRIDGE_TPL]
                },
                WEAPON_MOD_TPL: { "parent": ITEM_NODE, "width": 1, "height": 1 },
                ARMOR: { "parent": ITEM_NODE },
                ARMOR_TPL: {
                    "parent": ARMOR, "width": 3, "height": 4,
                    "slots": [{ "name": "mod_plate", "required": true, "filter": [ARMOR_MOD_TPL] }]
                },
                ARMOR_MOD_TPL: { "parent": ITEM_NODE, "width": 1, "height": 1 },
            },
            "defaultPresets": {},
            "moneyTpls": [MONEY_TPL],
            "staticAmmoDist": {
                CALIBER: [{ "tpl": CARTRIDGE_TPL, "relativeProbability": 1 }]
            },
            "config": {
                "containerRandomisationEnabled": true, "locationInRandomisationMaps": true,
                "containerTypesToNotRandomise": [], "containerGroupMinSizeMultiplier": 1,
                "containerGroupMaxSizeMultiplier": 1, "allowDuplicateItemsInStaticContainers": true,
                "tplsToStripChildItemsFrom": [], "fitLootIntoContainerAttempts": 3,
                "magazineLootHasAmmoChancePercent": 100,
                "staticMagazineLootHasAmmoChancePercent": 100,
                "minFillLooseMagazinePercent": 30, "minFillStaticMagazinePercent": 30,
                "staticLootMultiplier": 1, "looseLootMultiplier": 1,
                "modSpawnChancePercent": {}, "looseLootBlacklist": []
            },
            "seasonal": {
                "seasonalEventActive": false, "christmasEventEnabled": false,
                "inactiveSeasonalItems": [], "christmasContainerIds": []
            },
            "lootableItemBlacklist": [],
            "counter": { "maxCounts": {}, "trackedCounts": {} },
            "staticWeapons": [{
                "Id": "w1", "Root": mongo_id::generate(),
                "Items": [{ "_id": mongo_id::generate(), "_tpl": WEAPON_TPL }]
            }],
            "staticContainers": [
                container("c1", 1.0, false),
                container("c2", 0.5, true),
                container("r1", 0.5, false),
                container("r2", 0.5, false),
                container("r3", 0.5, false),
            ],
            "staticForced": [{ "containerId": "c1", "itemTpl": FORCED_TPL }],
            "staticLootDist": {
                CONTAINER_TPL: {
                    "itemcountDistribution": [{ "count": 4, "relativeProbability": 1 }],
                    // The forced tpl is deliberately absent: it may only reach a container through
                    // `staticForced`.
                    "itemDistribution": [
                        { "tpl": MONEY_TPL, "relativeProbability": 1 },
                        { "tpl": AMMO_BOX_TPL, "relativeProbability": 1 },
                        { "tpl": MAGAZINE_TPL, "relativeProbability": 1 },
                    ]
                }
            },
            "statics": {
                "containersGroups": { "g1": { "minContainers": 2, "maxContainers": 2 } },
                "containers": {
                    "r1": { "groupId": "g1" },
                    "r2": { "groupId": "g1" },
                    "r3": { "groupId": "g1" },
                }
            }
        }))
        .unwrap()
    }

    fn spawnpoint_ids(result: &StaticContainersResult) -> Vec<&str> {
        result
            .spawnpoints
            .iter()
            .filter_map(|spawnpoint| spawnpoint.id.as_deref())
            .collect()
    }

    fn items_of<'a>(result: &'a StaticContainersResult, spawnpoint_id: &str) -> Vec<&'a str> {
        result
            .spawnpoints
            .iter()
            .filter(|spawnpoint| spawnpoint.id.as_deref() == Some(spawnpoint_id))
            .flat_map(|spawnpoint| spawnpoint.items.iter().flatten())
            .map(|item| item.item.template.as_str())
            .collect()
    }

    /// Item count across every spawn point but the mounted weapon the request came in with.
    fn container_item_count(result: &StaticContainersResult) -> i32 {
        result
            .spawnpoints
            .iter()
            .skip(1)
            .map(|spawnpoint| spawnpoint.items.as_ref().map_or(0, Vec::len) as i32)
            .sum()
    }

    /// Strips MongoIds (24 hex chars) — the ids are minted from the process-wide MongoId counter,
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

    /// `fixture_request()` with two container groups whose bounds differ. The base fixture's single
    /// `minContainers == maxContainers` group makes `get_int` short-circuit without drawing, which
    /// hides the per-group draw in `get_group_id_to_container_mappings` — the order-sensitive path.
    fn multi_group_fixture() -> StaticContainersRequest {
        let mut request = fixture_request();
        let statics = request.statics.as_mut().expect("fixture has statics");

        statics.containers_groups = Some(
            serde_json::from_value(json!({
                "g1": { "minContainers": 1, "maxContainers": 2 },
                "g2": { "minContainers": 1, "maxContainers": 2 },
            }))
            .unwrap(),
        );
        statics.containers = Some(
            serde_json::from_value(json!({
                "r1": { "groupId": "g1" },
                "r2": { "groupId": "g1" },
                "r3": { "groupId": "g2" },
            }))
            .unwrap(),
        );

        // Ceilings high enough never to bite, purely so several tpls land in `trackedCounts` — it
        // stays empty under the base fixture, which would hide its ordering from the comparison.
        request.common.counter.max_counts = [MONEY_TPL, AMMO_BOX_TPL, MAGAZINE_TPL]
            .into_iter()
            .map(|tpl| (tpl.to_owned(), 9999))
            .collect();

        request
    }

    #[test]
    fn a_test_seed_makes_static_generation_deterministic() {
        // Swept over seeds, not repeated on one: a fixed seed replays the same draw values every
        // iteration, so an order hazard whose two draws happen to coincide under that one seed
        // would stay invisible no matter how often it ran. Varying the seed varies the values, so
        // a regression to `HashMap` on the container-group map or `trackedCounts` surfaces.
        for seed in 0..25 {
            let mut request_a = multi_group_fixture();
            request_a.common.test_seed = Some(seed);
            let mut request_b = multi_group_fixture();
            request_b.common.test_seed = Some(seed);

            let result_a = generate_static_containers(request_a).unwrap();
            let result_b = generate_static_containers(request_b).unwrap();

            assert_eq!(
                strip_mongo_ids(&serde_json::to_string(&result_a).unwrap()),
                strip_mongo_ids(&serde_json::to_string(&result_b).unwrap())
            );
        }
    }

    #[test]
    fn guaranteed_containers_are_always_spawned() {
        for _ in 0..25 {
            let result = generate_static_containers(fixture_request()).unwrap();
            let ids = spawnpoint_ids(&result);

            assert!(ids.contains(&"c1"), "c1 missing from {ids:?}");
            assert!(ids.contains(&"c2"), "c2 missing from {ids:?}");
        }
    }

    #[test]
    fn forced_items_always_land_in_their_own_container() {
        for _ in 0..25 {
            let result = generate_static_containers(fixture_request()).unwrap();

            assert!(
                items_of(&result, "c1").contains(&FORCED_TPL),
                "forced tpl missing from c1"
            );
            // The forced entry names c1 only, so no other container may be handed it.
            assert!(!items_of(&result, "c2").contains(&FORCED_TPL));
        }
    }

    #[test]
    fn spawn_limits_cap_a_tpl_across_every_container() {
        let mut request = fixture_request();
        // Money is the only thing left in the pool, so every draw of every container is money.
        request
            .static_loot_dist
            .get_mut(CONTAINER_TPL)
            .unwrap()
            .item_distribution = Some(
            serde_json::from_value(json!([{ "tpl": MONEY_TPL, "relativeProbability": 1 }]))
                .unwrap(),
        );
        request
            .common
            .counter
            .max_counts
            .insert(MONEY_TPL.to_owned(), 1);

        let result = generate_static_containers(request).unwrap();

        let money_spawned = result
            .spawnpoints
            .iter()
            .flat_map(|spawnpoint| spawnpoint.items.iter().flatten())
            .filter(|item| item.item.template == MONEY_TPL)
            .count();
        assert_eq!(money_spawned, 1, "the spawn limit of 1 was not enforced");
        // 4 containers x 4 draws, each incremented once during filtering even after the cap hits.
        assert_eq!(result.tracked_counts[MONEY_TPL], 16);
    }

    #[test]
    fn disabled_randomisation_adds_every_container() {
        let mut request = fixture_request();
        request.common.config.container_randomisation_enabled = false;

        let result = generate_static_containers(request).unwrap();
        let ids = spawnpoint_ids(&result);

        assert_eq!(ids, vec!["w1", "c1", "c2", "r1", "r2", "r3"]);
        // Only the guaranteed containers are counted on this path (`:142` runs, `:256` does not).
        assert_eq!(result.static_container_count, 2);
    }

    #[test]
    fn reported_counts_match_the_spawn_points() {
        let result = generate_static_containers(fixture_request()).unwrap();

        // 1 mounted weapon + 2 guaranteed + a group of exactly 2.
        assert_eq!(result.spawnpoints.len(), 5);
        assert_eq!(result.static_container_count, 4);
        assert_eq!(result.static_loot_item_count, container_item_count(&result));
    }

    #[test]
    fn every_emitted_item_id_is_a_mongo_id() {
        let result = generate_static_containers(fixture_request()).unwrap();

        for spawnpoint in &result.spawnpoints {
            for item in spawnpoint.items.iter().flatten() {
                assert!(mongo_id::is_valid(&item.item.id), "{}", item.item.id);
            }
        }
    }

    #[test]
    fn an_item_location_keeps_the_field_order_the_c_sharp_writes() {
        // `Item::location` is an untyped `Value`, so the object lands in a `serde_json::Map`. Without
        // `preserve_order` that map sorts, and every location comes out `{"r","x","y"}` where the C#
        // `ItemLocation` writes `{"x","y","r"}` - a byte difference in output that is otherwise
        // invisible to a structural comparison.
        let json = serde_json::to_string(&generate_static_containers(fixture_request()).unwrap())
            .expect("result serializes");

        assert!(
            json.contains(r#""location":{"x":"#),
            "no x-first location in {json}"
        );
        assert!(!json.contains(r#""location":{"r":"#));
    }

    #[test]
    fn adding_loot_leaves_the_request_container_untouched() {
        let request = fixture_request();
        let mut ctx = loot_context(&request.common, CounterState::default());
        ctx.diagnostics = DiagSink::capture();
        let container = &request.static_containers.as_ref().unwrap()[0];
        let before = serde_json::to_value(container).unwrap();

        let filled = add_loot_to_container(
            &mut ctx,
            container,
            request.static_forced.as_deref(),
            &request.static_loot_dist,
            "bigmap",
        )
        .unwrap();

        assert!(filled.items.unwrap().len() > 1);
        assert_eq!(serde_json::to_value(container).unwrap(), before);
    }

    #[test]
    fn increment_count_ignores_untracked_keys_and_caps_tracked_ones() {
        let mut counter = CounterState::default();

        assert!(!increment_count(&mut counter, MONEY_TPL));
        assert!(counter.tracked_counts.is_empty());

        counter.max_counts.insert(MONEY_TPL.to_owned(), 1);
        assert!(!increment_count(&mut counter, MONEY_TPL));
        assert!(increment_count(&mut counter, MONEY_TPL));
        assert_eq!(counter.tracked_counts[MONEY_TPL], 2);
    }

    #[test]
    fn weighted_count_errors_when_the_container_is_absent_from_the_loot_dist() {
        let request = fixture_request();
        let mut ctx = loot_context(&request.common, CounterState::default());
        ctx.diagnostics = DiagSink::capture();

        assert!(
            get_weighted_count_of_container_items(
                &mut ctx,
                FORCED_TPL,
                &request.static_loot_dist,
                "bigmap"
            )
            .is_err()
        );
    }

    #[test]
    fn containers_by_probability_returns_the_whole_pool_when_it_is_too_small() {
        let request = fixture_request();
        let mut ctx = loot_context(&request.common, CounterState::default());
        ctx.diagnostics = DiagSink::capture();
        let container_data = ContainerGroupCount {
            container_ids_with_probability: IndexMap::from([("r1".to_owned(), 0.5)]),
            chosen_count: 3.0,
        };

        let mut chosen = get_containers_by_probability(&mut ctx, "g1", &container_data);
        chosen.sort();

        assert_eq!(chosen, vec!["r1"]);
    }

    #[test]
    fn containers_by_probability_hands_back_the_pool_in_insertion_order() {
        // The whole-pool fallback returns `ContainerIdsWithProbability.Keys`, and the C#
        // `Dictionary` enumerates those in insertion order. Ids deliberately in reverse-alphabetical
        // order, so a sorted map would answer `[r1, r2, r3]` instead. The same ordering decides
        // which container each `get_chance_100` roll belongs to in the ungrouped edge case, and the
        // index each one takes in the probability array on the drawing path.
        let request = fixture_request();
        let mut ctx = loot_context(&request.common, CounterState::default());
        ctx.diagnostics = DiagSink::capture();
        let container_data = ContainerGroupCount {
            container_ids_with_probability: IndexMap::from([
                ("r3".to_owned(), 0.5),
                ("r2".to_owned(), 0.5),
                ("r1".to_owned(), 0.5),
            ]),
            chosen_count: 99.0,
        };

        let chosen = get_containers_by_probability(&mut ctx, "g1", &container_data);

        assert_eq!(chosen, vec!["r3", "r2", "r1"]);
    }

    #[test]
    fn container_groups_are_walked_in_json_order() {
        // `containersGroups` is a C# `Dictionary` deserialized from JSON, so it is walked in file
        // order - both for the `get_int` that sizes each group and for the draws that fill them.
        // The group names are reverse-alphabetical and the empty group sorts before all of them, so
        // a sorted map emits `z_group`'s container after `a_group`'s, and rolls the ungrouped
        // containers before either.
        let mut request = fixture_request();
        request.common.test_seed = Some(42);
        let statics = request.statics.as_mut().expect("fixture has statics");
        // Rides on serde_json's `preserve_order`, as the FFI entry points do: without it the `json!`
        // below - and the request the C# caller sends - would arrive alphabetised.
        statics.containers_groups = Some(
            serde_json::from_value(json!({
                "z_group": { "minContainers": 1, "maxContainers": 1 },
                "a_group": { "minContainers": 1, "maxContainers": 1 },
                "m_group": { "minContainers": 0, "maxContainers": 0 },
            }))
            .unwrap(),
        );
        statics.containers = Some(
            serde_json::from_value(json!({
                "r1": { "groupId": "z_group" },
                "r2": { "groupId": "a_group" },
                // Sized 0, so it is skipped without drawing and cannot mask the order above.
                "r3": { "groupId": "m_group" },
            }))
            .unwrap(),
        );

        let result = generate_static_containers(request).unwrap();

        assert_eq!(spawnpoint_ids(&result), vec!["w1", "c1", "c2", "r1", "r2"]);
    }

    #[test]
    fn christmas_containers_are_dropped_outside_the_event() {
        let mut request = fixture_request();
        request
            .common
            .seasonal
            .christmas_container_ids
            .insert("c1".to_owned());

        let result = generate_static_containers(request).unwrap();
        assert!(!spawnpoint_ids(&result).contains(&"c1"));

        let mut request = fixture_request();
        request
            .common
            .seasonal
            .christmas_container_ids
            .insert("c1".to_owned());
        request.common.seasonal.christmas_event_enabled = true;

        let result = generate_static_containers(request).unwrap();
        assert!(spawnpoint_ids(&result).contains(&"c1"));
    }

    #[test]
    fn missing_statics_stops_after_the_guaranteed_containers() {
        let mut request = fixture_request();
        request.statics = None;

        let result = generate_static_containers(request).unwrap();

        assert_eq!(spawnpoint_ids(&result), vec!["w1", "c1", "c2"]);
        assert_eq!(result.static_container_count, 2);
    }

    #[test]
    fn absent_static_data_is_fatal() {
        let mut request = fixture_request();
        request.static_weapons = None;
        assert!(generate_static_containers(request).is_err());

        let mut request = fixture_request();
        request.static_containers = None;
        assert!(generate_static_containers(request).is_err());

        // The forced list is only reached once a container is filled, exactly as in C#.
        let mut request = fixture_request();
        request.static_forced = None;
        assert!(generate_static_containers(request).is_err());
    }

    /// The `groupId == ""` edge case: every container rolls its own probability and the survivors
    /// are all taken. `GetChance100` rolls an integer 1-99, so 99% is a certainty and 0.5% is
    /// impossible — both ends are deterministic.
    #[test]
    fn ungrouped_containers_roll_their_own_probability() {
        for (probability, expected_containers) in [(0.99, 5), (0.005, 2)] {
            let mut request = fixture_request();
            request.statics.as_mut().unwrap().containers = Some(HashMap::from([
                ("r1".to_owned(), ContainerData::default()),
                ("r2".to_owned(), ContainerData::default()),
                ("r3".to_owned(), ContainerData::default()),
            ]));
            for container in request.static_containers.as_mut().unwrap() {
                if template_id(container).starts_with('r') {
                    container.probability = Some(probability);
                }
            }

            let result = generate_static_containers(request).unwrap();

            assert_eq!(result.static_container_count, expected_containers);
            assert_eq!(result.spawnpoints.len() as i32, expected_containers + 1);
        }
    }

    #[test]
    fn weapons_are_built_from_their_default_preset() {
        let mut request = fixture_request();
        request.common.default_presets = serde_json::from_value(json!({
            WEAPON_TPL: { "items": [
                { "_id": "p1", "_tpl": WEAPON_TPL },
                { "_id": "p2", "_tpl": WEAPON_MOD_TPL, "parentId": "p1", "slotId": "mod_handguard" },
                { "_id": "p3", "_tpl": MAGAZINE_TPL, "parentId": "p1", "slotId": "mod_magazine" },
            ]}
        }))
        .unwrap();
        let mut ctx = loot_context(&request.common, CounterState::default());
        ctx.diagnostics = DiagSink::capture();

        let weapon = create_static_loot_item(&mut ctx, WEAPON_TPL, Some("container"))
            .unwrap()
            .unwrap();

        // Root keeps the caller's parent, the preset mods hang off it under fresh ids.
        assert_eq!(weapon.items[0].template, WEAPON_TPL);
        assert_eq!(weapon.items[0].parent_id.as_deref(), Some("container"));
        assert!(weapon.items.iter().all(|item| mongo_id::is_valid(&item.id)));
        // Size is recomputed from the assembled tree rather than the weapon's own template.
        assert_eq!((weapon.width, weapon.height), (Some(2), Some(1)));
        // The magazine is re-added at the end, filled with cartridges of the weapon's caliber.
        let magazine = weapon.items.iter().rev().nth(1).unwrap();
        assert_eq!(magazine.slot_id.as_deref(), Some("mod_magazine"));
        assert_eq!(
            magazine.parent_id.as_deref(),
            Some(weapon.items[0].id.as_str())
        );
        let last = weapon.items.last().unwrap();
        assert_eq!(last.template, CARTRIDGE_TPL);
        assert_eq!(last.parent_id.as_deref(), Some(magazine.id.as_str()));
    }

    #[test]
    fn an_empty_weapon_preset_is_fatal() {
        let mut request = fixture_request();
        request.common.default_presets =
            serde_json::from_value(json!({ WEAPON_TPL: { "items": [] } })).unwrap();
        let mut ctx = loot_context(&request.common, CounterState::default());
        ctx.diagnostics = DiagSink::capture();

        assert!(create_static_loot_item(&mut ctx, WEAPON_TPL, None).is_err());
        assert!(ctx.diagnostics.captured().iter().any(|entry| {
            entry.level == ERROR && entry.locale_key.as_deref() == Some("location-preset_not_found")
        }));
    }

    #[test]
    fn a_missing_weapon_root_carries_the_critical_error_locale_key() {
        let request = fixture_request();
        let mut ctx = loot_context(&request.common, CounterState::default());
        ctx.diagnostics = DiagSink::capture();

        // The arm guards an empty item list, which `create_static_loot_item` never produces —
        // reach it directly, as C#'s guard would be reached.
        let error =
            match create_weapon_root_and_children(&mut ctx, WEAPON_TPL, None, &mut Vec::new()) {
                Err(error) => error,
                Ok(_) => panic!("expected the missing-root failure"),
            };

        // `localise` falls back to the key when the process-global table lacks it, and no test
        // installs this key — deterministic under any interleaving with `logger_exports_roundtrip`.
        assert_eq!(error.message, "location-critical_error_see_log");
        assert!(ctx.diagnostics.captured().iter().any(|entry| {
            entry.level == ERROR
                && entry.locale_key.as_deref() == Some("location-missing_root_item")
        }));
    }

    #[test]
    fn an_absent_count_distribution_warns_and_counts_nothing() {
        let mut request = fixture_request();
        request
            .static_loot_dist
            .get_mut(CONTAINER_TPL)
            .unwrap()
            .item_count_distribution = None;
        let mut ctx = loot_context(&request.common, CounterState::default());
        ctx.diagnostics = DiagSink::capture();

        let count = get_weighted_count_of_container_items(
            &mut ctx,
            CONTAINER_TPL,
            &request.static_loot_dist,
            "bigmap",
        )
        .unwrap();

        assert_eq!(count, 0);
        let warning = &ctx.diagnostics.captured()[0];
        assert_eq!(warning.level, WARNING);
        assert_eq!(
            warning.locale_key.as_deref(),
            Some("location-unable_to_find_count_distribution_for_container")
        );
        assert_eq!(
            warning.args,
            Some(json!({ "containerId": CONTAINER_TPL, "locationName": "bigmap" }))
        );
    }

    #[test]
    fn static_loot_items_are_hydrated_by_base_class() {
        let request = fixture_request();
        let mut ctx = loot_context(&request.common, CounterState::default());
        ctx.diagnostics = DiagSink::capture();

        let money = create_static_loot_item(&mut ctx, MONEY_TPL, Some("parent"))
            .unwrap()
            .unwrap();
        let stack = money.items[0].upd.as_ref().unwrap().stack_objects_count;
        assert!((100.0..=200.0).contains(&stack.unwrap()));
        assert_eq!(money.items[0].parent_id.as_deref(), Some("parent"));

        let ammo_box = create_static_loot_item(&mut ctx, AMMO_BOX_TPL, None)
            .unwrap()
            .unwrap();
        assert_eq!(ammo_box.items.len(), 3);

        let magazine = create_static_loot_item(&mut ctx, MAGAZINE_TPL, None)
            .unwrap()
            .unwrap();
        assert!(magazine.items.len() > 1);

        // A weapon with no preset keeps just its root, and says so.
        let weapon = create_static_loot_item(&mut ctx, WEAPON_TPL, None)
            .unwrap()
            .unwrap();
        assert_eq!(weapon.items.len(), 1);
        assert_eq!((weapon.width, weapon.height), (Some(2), Some(1)));
        assert!(ctx.diagnostics.captured().iter().any(|entry| {
            entry.level == DEBUG
                && entry.message.as_deref()
                    == Some(&format!(
                        "createStaticLootItem() No preset found for weapon: {WEAPON_TPL}"
                    ) as &str)
        }));
    }

    #[test]
    fn armor_is_hydrated_from_a_preset_or_its_slots() {
        // No preset: the base item made by the caller keeps its parent and gains its slot mods.
        let request = fixture_request();
        let mut ctx = loot_context(&request.common, CounterState::default());
        ctx.diagnostics = DiagSink::capture();

        let armor = create_static_loot_item(&mut ctx, ARMOR_TPL, Some("container"))
            .unwrap()
            .unwrap();

        assert_eq!(armor.items.len(), 2);
        assert_eq!(armor.items[0].parent_id.as_deref(), Some("container"));
        assert_eq!(armor.items[1].template, ARMOR_MOD_TPL);
        assert_eq!(armor.items[1].slot_id.as_deref(), Some("mod_plate"));

        // Preset: its items replace the base one wholesale, re-ided, under the same parent.
        let mut request = fixture_request();
        request.common.default_presets = serde_json::from_value(json!({
            ARMOR_TPL: { "items": [
                { "_id": "p1", "_tpl": ARMOR_TPL },
                { "_id": "p2", "_tpl": ARMOR_MOD_TPL, "parentId": "p1", "slotId": "mod_plate" },
            ]}
        }))
        .unwrap();
        let mut ctx = loot_context(&request.common, CounterState::default());
        ctx.diagnostics = DiagSink::capture();

        let armor = create_static_loot_item(&mut ctx, ARMOR_TPL, Some("container"))
            .unwrap()
            .unwrap();

        assert_eq!(armor.items.len(), 2);
        assert!(armor.items.iter().all(|item| mongo_id::is_valid(&item.id)));
        assert_eq!(armor.items[0].parent_id.as_deref(), Some("container"));
        assert_eq!(
            armor.items[1].parent_id.as_deref(),
            Some(armor.items[0].id.as_str())
        );
        // Armor size comes from the template, not the assembled tree (the weapon branch does that).
        assert_eq!((armor.width, armor.height), (Some(3), Some(4)));
    }

    // -----------------------------------------------------------------------
    // Dynamic (loose) loot
    // -----------------------------------------------------------------------

    /// One item in a loose loot spawn point; its composed key is derived from its id.
    fn loose_item(id: &str, tpl: &str) -> serde_json::Value {
        json!({ "_id": id, "_tpl": tpl, "composedKey": format!("ck_{id}") })
    }

    /// A child of `parent_id` — no composed key, so it is never an option in its own right.
    fn loose_child(id: &str, tpl: &str, parent_id: &str) -> serde_json::Value {
        json!({ "_id": id, "_tpl": tpl, "parentId": parent_id, "slotId": "mod_handguard" })
    }

    /// A loose loot spawn point holding `items`; every root item is an equally weighted option, and
    /// the template carries a mod-added field to prove passthrough survives generation.
    fn loose_point(
        location_id: &str,
        probability: f64,
        template_id: &str,
        items: Vec<serde_json::Value>,
    ) -> serde_json::Value {
        let item_distribution: Vec<serde_json::Value> = items
            .iter()
            .filter(|item| item.get("parentId").is_none())
            .map(|item| {
                json!({ "composedKey": { "key": item["composedKey"] }, "relativeProbability": 1 })
            })
            .collect();

        json!({
            "locationId": location_id,
            "probability": probability,
            "template": {
                "Id": template_id,
                "Root": items[0]["_id"],
                "Items": items,
                "modAddedField": location_id,
            },
            "itemDistribution": item_distribution,
        })
    }

    /// Forced loot (two points sharing a template id, one seasonal), a point flagged always-spawn,
    /// five guaranteed points (money, a weapon with a child mod, a magazine and two sharing one
    /// `locationId`), a christmas point, a blacklisted one, and two weighted points. `mean` 6 with
    /// `std` 0 fixes the desired count at 6, and the count is taken before the dedupe, so exactly
    /// one of the two weighted points is drawn.
    fn fixture_dynamic_request() -> DynamicLootRequest {
        let mut always_spawn_point = loose_point(
            "always_1",
            0.5,
            "always_1",
            vec![loose_item("ai1", PLAIN_TPL)],
        );
        always_spawn_point["template"]["IsAlwaysSpawn"] = json!(true);

        serde_json::from_value(json!({
            "locationId": "bigmap",
            "itemsView": {
                ITEM_NODE: {},
                MONEY: { "parent": ITEM_NODE },
                AMMO: { "parent": ITEM_NODE },
                AMMO_BOX: { "parent": ITEM_NODE },
                MAGAZINE: { "parent": ITEM_NODE },
                WEAPON: { "parent": ITEM_NODE },
                MONEY_TPL: {
                    "parent": MONEY, "width": 1, "height": 1,
                    "stackMaxSize": 500000, "stackMinRandom": 100, "stackMaxRandom": 200
                },
                WEAPON_TPL: { "parent": WEAPON, "width": 2, "height": 1 },
                WEAPON_MOD_TPL: { "parent": ITEM_NODE, "width": 1, "height": 1 },
                MAGAZINE_TPL: {
                    "parent": MAGAZINE, "width": 1, "height": 2,
                    "cartridgesMaxCount": 30, "cartridgesFirstFilter": [CARTRIDGE_TPL]
                },
                CARTRIDGE_TPL: { "parent": AMMO, "width": 1, "height": 1,
                    "stackMaxSize": 30, "caliber": CALIBER },
                FORCED_TPL: { "parent": ITEM_NODE, "width": 1, "height": 1 },
                PLAIN_TPL: { "parent": ITEM_NODE, "width": 1, "height": 1 },
                SEASONAL_TPL: { "parent": ITEM_NODE, "width": 1, "height": 1 },
            },
            "defaultPresets": {},
            "moneyTpls": [MONEY_TPL],
            "staticAmmoDist": {
                CALIBER: [{ "tpl": CARTRIDGE_TPL, "relativeProbability": 1 }]
            },
            "config": {
                "containerRandomisationEnabled": true, "locationInRandomisationMaps": true,
                "containerTypesToNotRandomise": [], "containerGroupMinSizeMultiplier": 1,
                "containerGroupMaxSizeMultiplier": 1, "allowDuplicateItemsInStaticContainers": true,
                "tplsToStripChildItemsFrom": [], "fitLootIntoContainerAttempts": 3,
                // The two magazine settings the loose path must NOT use are set to fail loudly:
                // a 0% chance never fills, a 10% fill leaves a third of the stack.
                "magazineLootHasAmmoChancePercent": 0,
                "staticMagazineLootHasAmmoChancePercent": 100,
                "minFillLooseMagazinePercent": 90, "minFillStaticMagazinePercent": 10,
                "staticLootMultiplier": 1, "looseLootMultiplier": 1,
                "modSpawnChancePercent": {}, "looseLootBlacklist": ["blacklisted_1"]
            },
            "seasonal": {
                "seasonalEventActive": false, "christmasEventEnabled": false,
                "inactiveSeasonalItems": [SEASONAL_TPL], "christmasContainerIds": []
            },
            "lootableItemBlacklist": [],
            "counter": { "maxCounts": { SEASONAL_TPL: 5 }, "trackedCounts": {} },
            "looseLoot": {
                "spawnpointCount": { "mean": 6, "std": 0 },
                "spawnpointsForced": [
                    loose_point("f1", 1.0, "forced_1", vec![loose_item("fi1", FORCED_TPL)]),
                    // Same template id as the point above, so it is logged and dropped.
                    loose_point("f2", 1.0, "forced_1", vec![loose_item("fi2", FORCED_TPL)]),
                    loose_point("f3", 1.0, "forced_seasonal", vec![loose_item("fi3", SEASONAL_TPL)]),
                ],
                "spawnpoints": [
                    loose_point("money_1", 1.0, "money_1", vec![loose_item("mi1", MONEY_TPL)]),
                    loose_point("weapon_1", 1.0, "weapon_1", vec![
                        loose_item("wi1", WEAPON_TPL),
                        loose_child("wi2", WEAPON_MOD_TPL, "wi1"),
                    ]),
                    loose_point("magazine_1", 1.0, "magazine_1", vec![loose_item("gi1", MAGAZINE_TPL)]),
                    // Two guaranteed points on one position: only the first may survive the dedupe.
                    loose_point("shared_location", 1.0, "dupe_first", vec![loose_item("di1", PLAIN_TPL)]),
                    loose_point("shared_location", 1.0, "dupe_second", vec![loose_item("di2", PLAIN_TPL)]),
                    loose_point("christmas_1", 1.0, "Christmas_1", vec![loose_item("ci1", PLAIN_TPL)]),
                    loose_point("blacklisted_1", 1.0, "blacklisted_1", vec![loose_item("bi1", PLAIN_TPL)]),
                    loose_point("weighted_1", 0.5, "weighted_1", vec![loose_item("wi3", PLAIN_TPL)]),
                    loose_point("weighted_2", 0.5, "weighted_2", vec![loose_item("wi4", PLAIN_TPL)]),
                    always_spawn_point,
                ]
            }
        }))
        .unwrap()
    }

    fn dynamic_ids(result: &DynamicLootResult) -> Vec<&str> {
        result
            .spawnpoints
            .iter()
            .filter_map(|spawnpoint| spawnpoint.id.as_deref())
            .collect()
    }

    fn dynamic_tpls(result: &DynamicLootResult) -> Vec<&str> {
        result
            .spawnpoints
            .iter()
            .flat_map(|spawnpoint| spawnpoint.items.iter().flatten())
            .map(|item| item.item.template.as_str())
            .collect()
    }

    #[test]
    fn forced_loot_lands_once_per_template_id() {
        for _ in 0..25 {
            let result = generate_dynamic_loot(fixture_dynamic_request()).unwrap();
            let ids = dynamic_ids(&result);

            assert_eq!(
                ids.iter().filter(|id| **id == "forced_1").count(),
                1,
                "forced point missing or duplicated in {ids:?}"
            );
            // The always-spawn point is forced too, and the main loop must not add it a second time.
            assert_eq!(ids.iter().filter(|id| **id == "always_1").count(), 1);
            // 2 forced + 4 guaranteed (5 less the deduped one) + 1 of the 2 weighted points.
            assert_eq!(result.spawnpoints.len(), 7, "{ids:?}");
        }
    }

    #[test]
    fn seasonal_forced_loot_is_counted_before_it_is_skipped() {
        let result = generate_dynamic_loot(fixture_dynamic_request()).unwrap();

        assert!(!dynamic_ids(&result).contains(&"forced_seasonal"));
        // `IncrementCount` runs before the seasonal check (`:879-888`), so the skipped point still
        // counts against the tpl's spawn limit.
        assert_eq!(result.tracked_counts[SEASONAL_TPL], 1);

        let mut request = fixture_dynamic_request();
        request.common.seasonal.seasonal_event_active = true;

        let result = generate_dynamic_loot(request).unwrap();
        assert!(dynamic_ids(&result).contains(&"forced_seasonal"));
    }

    #[test]
    fn christmas_and_blacklisted_spawn_points_are_dropped() {
        for _ in 0..25 {
            let result = generate_dynamic_loot(fixture_dynamic_request()).unwrap();
            let ids = dynamic_ids(&result);

            // Both are 100% spawn points, so only the filters can keep them out.
            assert!(!ids.contains(&"Christmas_1"), "{ids:?}");
            assert!(!ids.contains(&"blacklisted_1"), "{ids:?}");
        }

        // The christmas point comes back for the event, the blacklisted one never does.
        let mut request = fixture_dynamic_request();
        request.common.seasonal.christmas_event_enabled = true;

        let result = generate_dynamic_loot(request).unwrap();
        assert!(dynamic_ids(&result).contains(&"Christmas_1"));
    }

    #[test]
    fn spawn_limits_gate_dynamic_spawn_points() {
        let mut request = fixture_dynamic_request();
        request
            .common
            .counter
            .max_counts
            .insert(MONEY_TPL.to_owned(), 0);

        let result = generate_dynamic_loot(request).unwrap();

        assert!(!dynamic_tpls(&result).contains(&MONEY_TPL));
        assert!(!dynamic_ids(&result).contains(&"money_1"));
        assert_eq!(result.tracked_counts[MONEY_TPL], 1);
        assert_eq!(result.spawnpoints.len(), 6);
    }

    #[test]
    fn duplicate_location_ids_keep_the_first_spawn_point() {
        for _ in 0..25 {
            let result = generate_dynamic_loot(fixture_dynamic_request()).unwrap();
            let ids = dynamic_ids(&result);

            // Both points are guaranteed and sit on one position, so only the dedupe can drop one.
            assert!(ids.contains(&"dupe_first"), "{ids:?}");
            assert!(!ids.contains(&"dupe_second"), "{ids:?}");
        }
    }

    /// The loose path gates on the *static* chance percent and fills to the *loose* minimum
    /// (`:974-983`); the fixture sets the other two values of the four so that either half of the
    /// wrong pairing shows up here.
    #[test]
    fn magazines_use_the_static_chance_with_the_loose_fill() {
        for _ in 0..20 {
            let result = generate_dynamic_loot(fixture_dynamic_request()).unwrap();

            let magazine = result
                .spawnpoints
                .iter()
                .find(|spawnpoint| spawnpoint.id.as_deref() == Some("magazine_1"))
                .expect("the magazine point spawns at 100%");
            let items = magazine.items.as_ref().unwrap();

            assert_eq!(items[0].item.template, MAGAZINE_TPL);
            // The static chance of 100% always fills; the loose chance of 0% never would.
            assert_eq!(items.len(), 2, "the magazine was not filled");
            assert_eq!(items[1].item.template, CARTRIDGE_TPL);

            // 90% of the magazine's 30 rounds; the static fill of 10% allows as few as 3.
            let stack = items[1]
                .item
                .upd
                .as_ref()
                .unwrap()
                .stack_objects_count
                .unwrap();
            assert!(
                (27.0..=30.0).contains(&stack),
                "filled to {stack} rounds, below the loose minimum of 27"
            );
        }
    }

    #[test]
    fn every_spawn_point_roots_its_first_item() {
        for _ in 0..25 {
            let result = generate_dynamic_loot(fixture_dynamic_request()).unwrap();

            for spawnpoint in &result.spawnpoints {
                let items = spawnpoint.items.as_ref().unwrap();
                assert_eq!(
                    spawnpoint.root.as_deref(),
                    Some(items[0].item.id.as_str()),
                    "root mismatch on {:?}",
                    spawnpoint.id
                );
                assert!(items.iter().all(|item| mongo_id::is_valid(&item.item.id)));
                // The pool is overwritten with the chosen item, never appended to.
                assert!(items.len() <= 2);
            }
        }
    }

    #[test]
    fn weapon_points_keep_their_children() {
        let result = generate_dynamic_loot(fixture_dynamic_request()).unwrap();

        let weapon = result
            .spawnpoints
            .iter()
            .find(|spawnpoint| spawnpoint.id.as_deref() == Some("weapon_1"))
            .expect("the weapon point spawns at 100%");
        let items = weapon.items.as_ref().unwrap();

        assert_eq!(items.len(), 2);
        assert_eq!(items[0].item.template, WEAPON_TPL);
        assert_eq!(items[1].item.template, WEAPON_MOD_TPL);
        assert_eq!(
            items[1].item.parent_id.as_deref(),
            Some(items[0].item.id.as_str())
        );
        // Ids are replaced, so nothing collides with the next point built from the same pool.
        assert_ne!(items[0].item.id, "wi1");
        // `ToLootItem` drops the composed key.
        assert!(items.iter().all(|item| item.composed_key.is_none()));
    }

    #[test]
    fn results_round_trip_through_serde() {
        let result = generate_dynamic_loot(fixture_dynamic_request()).unwrap();

        let serialized = serde_json::to_value(&result.spawnpoints).unwrap();
        let reparsed: Vec<SpawnpointTemplate> = serde_json::from_value(serialized.clone()).unwrap();

        assert_eq!(serde_json::to_value(&reparsed).unwrap(), serialized);
        // Mod-added fields ride through generation untouched.
        for spawnpoint in serialized.as_array().unwrap() {
            assert!(spawnpoint["modAddedField"].is_string(), "{spawnpoint}");
        }
    }

    #[test]
    fn missing_loose_loot_data_is_fatal() {
        let mut request = fixture_dynamic_request();
        request.loose_loot.spawnpoints = None;
        assert!(generate_dynamic_loot(request).is_err());

        let mut request = fixture_dynamic_request();
        request.loose_loot.spawnpoint_count = None;
        assert!(generate_dynamic_loot(request).is_err());
    }

    #[test]
    fn the_loot_pool_drops_out_of_season_and_blacklisted_items() {
        let mut request = fixture_request();
        request
            .common
            .seasonal
            .inactive_seasonal_items
            .insert(MONEY_TPL.to_owned());
        request
            .common
            .lootable_item_blacklist
            .insert(AMMO_BOX_TPL.to_owned());
        let mut ctx = loot_context(&request.common, CounterState::default());
        ctx.diagnostics = DiagSink::capture();

        let pool = get_possible_loot_items_for_container(
            &mut ctx,
            CONTAINER_TPL,
            &request.static_loot_dist,
        );

        assert_eq!(pool.len(), 1);
        assert!(pool.draw(20).iter().all(|tpl| tpl == MAGAZINE_TPL));
    }
}
