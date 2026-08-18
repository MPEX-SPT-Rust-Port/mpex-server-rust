//! `Services/Ragfair/RagfairLinkedItemService.cs` — `BuildLinkedItemTable` in bulk: every
//! template's slot/chamber/cartridge filters unioned into an id ↔ id-set table, plus the
//! revolver camora-ammo edge case.

use std::collections::HashSet;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::loot::models::{ItemView, SlotView};
use crate::loot::mongo_id;

/// `templateTable.Items` projected to the four members the walk reads (`parent`, `slots`,
/// `chambers`, `cartridges`).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkedItemsRequest {
    pub items_view: IndexMap<String, ItemView>,
}

/// The table `BuildLinkedItemTable` fills, copied into `linkedItemsCache` C#-side.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LinkedItemsResponse {
    pub linked_items: IndexMap<String, HashSet<String>>,
}

/// `BaseClasses.REVOLVER` (`BaseClasses.cs:99`) — the parent tpl the revolver special case tests.
const REVOLVER: &str = "617f1ef5e8b54b0998387733";

/// `RagfairLinkedItemService.BuildLinkedItemTable` (`RagfairLinkedItemService.cs:62-109`).
pub fn build(req: &LinkedItemsRequest) -> LinkedItemsResponse {
    let mut linked_items: IndexMap<String, HashSet<String>> =
        IndexMap::with_capacity(req.items_view.len());

    for (tpl, item) in &req.items_view {
        // `linkedItems.TryAdd(item.Id, [])` (RagfairLinkedItemService.cs:67) — every template
        // seeds an entry, so an unlinked tpl answers with an empty set, not a miss. Quirk 7:
        // the map is keyed here by the view key, which equals `item.Id` by table construction.
        linked_items.entry(tpl.clone()).or_default();

        // The three bidirectional loops (:71-95): slots, then chambers, then cartridges — each
        // linked id lands in the item's set and the item in the id's set.
        let linked_ids: Vec<&str> = slot_filter_ids(item.slots.as_deref())
            .chain(slot_filter_ids(item.chambers.as_deref()))
            .chain(slot_filter_ids(item.cartridges.as_deref()))
            .collect();
        for linked_id in linked_ids {
            linked_items
                .get_mut(tpl)
                .expect("seeded above")
                .insert(linked_id.to_owned());
            linked_items
                .entry(linked_id.to_owned())
                .or_default()
                .insert(tpl.clone());
        }

        // Edge case, ensure ammo for revolvers is included (:98-102).
        if item.parent.as_deref() == Some(REVOLVER) {
            for ammo_tpl in revolver_cylinder_ammo(item, &req.items_view) {
                // Quirk 2, ported verbatim: forward-only — the ammo's set does not gain the
                // revolver, unlike the three loops above (:136 unions into itemLinkedSet only).
                linked_items
                    .get_mut(tpl)
                    .expect("seeded above")
                    .insert(ammo_tpl.to_owned());
            }
        }
    }

    LinkedItemsResponse { linked_items }
}

/// `GetSlotFilters`/`GetChamberFilters`/`GetCartridgeFilters` (:144-222) — every `Filters`
/// group's ids, flattened in order. The projection already unioned the groups into each
/// `SlotView.filter`, so here a slot contributes its one flattened list.
fn slot_filter_ids(slots: Option<&[SlotView]>) -> impl Iterator<Item = &str> {
    slots
        .unwrap_or_default()
        .iter()
        .flat_map(|slot| slot.filter.as_deref().unwrap_or_default())
        .map(String::as_str)
}

/// `AddRevolverCylinderAmmoToLinkedItems` (:117-137): resolve the cylinder behind the
/// `mod_magazine` slot, then hand back the cylinder's own slot filter ids (the camora ammo).
fn revolver_cylinder_ammo<'a>(
    revolver: &'a ItemView,
    items_view: &'a IndexMap<String, ItemView>,
) -> Vec<&'a str> {
    // `Slots?.FirstOrDefault(x => x.Name == "mod_magazine")` (:119) — quirk 8: exact,
    // case-sensitive name match. Quirk 4: a null-`Properties` revolver NREs in C# here;
    // native sees absent `slots` and skips.
    let Some(cylinder_mod) = revolver.slots.as_deref().and_then(|slots| {
        slots
            .iter()
            .find(|slot| slot.name.as_deref() == Some("mod_magazine"))
    }) else {
        return Vec::new();
    };

    // `Filters?.First().Filter?.FirstOrDefault() ?? MongoId.Empty()` (:126) on the flattened
    // per-slot filter — quirk 3: the first id of the first non-empty group; the
    // empty-first-group and empty-`Filters`-list shapes are sanctioned divergences.
    let Some(cylinder_tpl) = cylinder_mod
        .filter
        .as_deref()
        .and_then(|filter| filter.first())
    else {
        return Vec::new();
    };

    // `IsValidMongoId` gate (:128).
    if !mongo_id::is_valid(cylinder_tpl) {
        return Vec::new();
    }

    // Quirk 4, sanctioned divergence: a valid-format cylinder tpl missing from the table NREs
    // in C# (:135-136); native skips.
    let Some(cylinder) = items_view.get(cylinder_tpl.as_str()) else {
        return Vec::new();
    };

    // `itemLinkedSet.UnionWith(GetSlotFilters(cylinderTemplate))` (:136).
    slot_filter_ids(cylinder.slots.as_deref()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(json: &str) -> LinkedItemsRequest {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn minimal_projection_deserializes_into_item_view() {
        let r = req(r#"{"itemsView":{"aaa":{"parent":"bbb",
                "slots":[{"name":"mod_stock","filter":["ccc"]}],
                "chambers":[{"filter":["ddd"]}],
                "cartridges":[{"filter":["eee"]}]}}}"#);
        let v = &r.items_view["aaa"];
        assert_eq!(v.parent.as_deref(), Some("bbb"));
        assert_eq!(
            v.slots.as_deref().unwrap()[0].name.as_deref(),
            Some("mod_stock")
        );
        assert_eq!(
            v.chambers.as_deref().unwrap()[0].filter.as_deref().unwrap(),
            ["ddd"]
        );
        assert!(v.item_type.is_none()); // absent members land as None
    }

    #[test]
    fn slots_chambers_and_cartridges_link_bidirectionally() {
        let r = req(r#"{"itemsView":{
                "weapon":{"parent":"p",
                    "slots":[{"name":"mod_stock","filter":["stockA"]}],
                    "chambers":[{"filter":["ammoA"]}],
                    "cartridges":[{"filter":["ammoB"]}]},
                "stockA":{"parent":"p"}
            }}"#);
        let resp = build(&r);
        let weapon = &resp.linked_items["weapon"];
        assert!(weapon.contains("stockA"));
        assert!(weapon.contains("ammoA"));
        assert!(weapon.contains("ammoB"));
        assert_eq!(weapon.len(), 3);
        // Reverse edges — including onto ids the view itself never holds (ammoA/ammoB).
        assert!(resp.linked_items["stockA"].contains("weapon"));
        assert!(resp.linked_items["ammoA"].contains("weapon"));
        assert!(resp.linked_items["ammoB"].contains("weapon"));
    }

    #[test]
    fn every_template_gets_an_entry_even_unlinked() {
        let r = req(r#"{"itemsView":{"lonely":{"parent":"p"}}}"#);
        let resp = build(&r);
        assert_eq!(resp.linked_items.len(), 1);
        assert!(resp.linked_items["lonely"].is_empty());
    }

    #[test]
    fn revolver_gains_cylinder_ammo_one_way() {
        // Quirk 2: the revolver's set gains the camora ammo; the ammo's set gains only the
        // cylinder (via the cylinder's own row), never the revolver.
        let r = req(r#"{"itemsView":{
                "revolver":{"parent":"617f1ef5e8b54b0998387733",
                    "slots":[{"name":"mod_magazine","filter":["cccccccccccccccccccccccc"]}]},
                "cccccccccccccccccccccccc":{"parent":"magParent",
                    "slots":[{"name":"camora_000","filter":["ammoA"]}]}
            }}"#);
        let resp = build(&r);
        let revolver = &resp.linked_items["revolver"];
        assert!(revolver.contains("cccccccccccccccccccccccc")); // its own slots loop
        assert!(revolver.contains("ammoA")); // the special case
        assert!(!resp.linked_items["ammoA"].contains("revolver"));
        assert!(resp.linked_items["ammoA"].contains("cccccccccccccccccccccccc"));
    }

    #[test]
    fn revolver_without_mod_magazine_slot_adds_no_ammo() {
        let r = req(r#"{"itemsView":{
                "revolver":{"parent":"617f1ef5e8b54b0998387733",
                    "slots":[{"name":"mod_barrel","filter":["barrelA"]}]}
            }}"#);
        let resp = build(&r);
        assert_eq!(resp.linked_items["revolver"].len(), 1); // barrelA only
    }

    #[test]
    fn invalid_or_missing_cylinder_tpl_is_skipped() {
        // "not-a-mongo-id" fails the IsValidMongoId gate (:128); the valid-format
        // "dddddddddddddddddddddddd" is absent from the view (quirk 4). Both still land in
        // the revolver's set through the ordinary slots loop.
        let r = req(r#"{"itemsView":{
                "revolverA":{"parent":"617f1ef5e8b54b0998387733",
                    "slots":[{"name":"mod_magazine","filter":["not-a-mongo-id"]}]},
                "revolverB":{"parent":"617f1ef5e8b54b0998387733",
                    "slots":[{"name":"mod_magazine","filter":["dddddddddddddddddddddddd"]}]}
            }}"#);
        let resp = build(&r);
        assert_eq!(resp.linked_items["revolverA"].len(), 1);
        assert!(resp.linked_items["revolverA"].contains("not-a-mongo-id"));
        assert_eq!(resp.linked_items["revolverB"].len(), 1);
        assert!(resp.linked_items["revolverB"].contains("dddddddddddddddddddddddd"));
    }
}
