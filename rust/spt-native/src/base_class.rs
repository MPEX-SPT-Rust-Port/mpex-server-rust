//! `Services/Items/ItemBaseClassService.cs` — `HydrateItemBaseClassCache` in bulk: every
//! template's parent chain walked once, then split into the `_itemBaseClassesCache` entries
//! (`_type == "Item"`) and the `_rootNodeIds` set (everything else).

use std::collections::HashSet;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::loot::item_helper::ItemBaseClassCache;
use crate::loot::models::ItemView;

/// `templateTable.Items` projected to the two members the walk reads (`parent`, `type`).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BaseClassRequest {
    pub items_view: IndexMap<String, ItemView>,
}

/// The two fields `HydrateItemBaseClassCache` fills.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BaseClassResponse {
    /// `_itemBaseClassesCache` — key = item tpl, values = ids of its parents.
    pub item_base_classes: IndexMap<String, HashSet<String>>,
    /// `_rootNodeIds` — every tpl that failed the `_type == "Item"` test.
    pub root_node_ids: Vec<String>,
}

/// `ItemBaseClassService.HydrateItemBaseClassCache` (`ItemBaseClassService.cs:31-40`), which is
/// `AddItemToCache` (`:42-64`) per template.
pub fn build(req: &BaseClassRequest) -> BaseClassResponse {
    // The walk runs over the *whole* view, not just the Items: a chain climbs through `Node`-type
    // parents, so filtering them out first would cut every chain at its first node. The chains
    // computed for non-Item tpls are simply dropped below, exactly as C# never computes them.
    let mut ancestors = ItemBaseClassCache::build(&req.items_view).into_ancestors();

    let mut item_base_classes = IndexMap::with_capacity(ancestors.len());
    let mut root_node_ids = Vec::new();

    for (tpl, item) in &req.items_view {
        // Quirk 5, ported verbatim: the type test is `string.Equals(item.Type, "Item",
        // StringComparison.OrdinalIgnoreCase)`, so `"item"` and `"ITEM"` are Items too
        // (ItemBaseClassService.cs:54). A template carrying no `_type` fails it and joins the
        // `Node`s in `_rootNodeIds`.
        if item
            .item_type
            .as_deref()
            .is_some_and(|item_type| item_type.eq_ignore_ascii_case("Item"))
        {
            // Quirk 2, ported verbatim: `AddBaseItems` stores a parent id before looking it up, so
            // a chain keeps a final parent that is missing from the view
            // (ItemBaseClassService.cs:73-74) — the reused walk stores in the same order. The
            // `unwrap_or_default` stands in for C#'s empty-set seed (`:56`), which is what a
            // parentless Item is left with.
            item_base_classes.insert(tpl.clone(), ancestors.remove(tpl).unwrap_or_default());
        } else {
            root_node_ids.push(tpl.clone());
        }
    }

    BaseClassResponse {
        item_base_classes,
        root_node_ids,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(json: &str) -> BaseClassRequest {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn minimal_projection_deserializes_into_item_view() {
        let r = req(r#"{"itemsView":{"aaa":{"parent":"bbb","type":"Item"}}}"#);
        let v = &r.items_view["aaa"];
        assert_eq!(v.parent.as_deref(), Some("bbb"));
        assert_eq!(v.item_type.as_deref(), Some("Item"));
        assert!(v.stack_max_size.is_none()); // absent members land as None
    }

    #[test]
    fn item_type_test_is_case_insensitive_and_none_is_root() {
        // "item" (lowercase) is an Item (quirk 5); missing _type is a root node.
        let r = req(r#"{"itemsView":{
                "child":{"parent":"node","type":"item"},
                "node":{"parent":"root","type":"Node"},
                "untyped":{"parent":"root"}
            }}"#);
        let resp = build(&r);
        assert!(resp.item_base_classes.contains_key("child"));
        assert_eq!(
            resp.root_node_ids,
            vec!["node".to_string(), "untyped".to_string()]
        );
    }

    #[test]
    fn chain_walks_through_node_parents_and_stores_the_final_missing_parent() {
        // child -> node -> root(absent from view): chain holds both, the walk
        // stores a parent before looking it up (quirk 2, ItemBaseClassService.cs:73-74).
        let r = req(r#"{"itemsView":{
                "child":{"parent":"node","type":"Item"},
                "node":{"parent":"root","type":"Node"}
            }}"#);
        let resp = build(&r);
        let chain = &resp.item_base_classes["child"];
        assert!(chain.contains("node"));
        assert!(chain.contains("root"));
        assert_eq!(chain.len(), 2);
    }

    #[test]
    fn into_ancestors_hands_back_the_walked_map() {
        let mut view = IndexMap::new();
        view.insert(
            "a".to_string(),
            serde_json::from_str::<ItemView>(r#"{"parent":"b"}"#).unwrap(),
        );
        let cache = ItemBaseClassCache::build(&view);
        let map = cache.into_ancestors();
        assert!(map["a"].contains("b"));
    }
}
