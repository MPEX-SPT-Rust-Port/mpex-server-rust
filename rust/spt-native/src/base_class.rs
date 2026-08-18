//! `Services/Items/ItemBaseClassService.cs` — `HydrateItemBaseClassCache` in bulk: every
//! template's parent chain walked once, then split into the `_itemBaseClassesCache` entries
//! (`_type == "Item"`) and the `_rootNodeIds` set (everything else).

use std::collections::HashSet;

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::db::models::TemplateItem;
use crate::loot::item_helper::ItemBaseClassCache;
use crate::loot::models::ItemView;

/// `{epoch, viewsOverride?}` — the epoch-protocol envelope (spec § Exports). No varying block:
/// the whole pre-flip payload was invariant.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BaseClassRequest {
    pub epoch: u64,
    pub views_override: Option<BaseClassViewsWire>,
}

/// The distrust fallback: the C#-built projection, used for this call only and never made
/// resident. Present iff the caller is ineligible for residency.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BaseClassViewsWire {
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

/// Resolve the walk input and build: the override when the request carries one, the resident
/// templates root otherwise. `Err(StaleEpoch)` when an override-less request names an epoch this
/// process does not hold, or before the templates root is resident.
pub fn run(request: BaseClassRequest) -> Result<BaseClassResponse, crate::db::StaleEpoch> {
    match request.views_override {
        Some(views) => Ok(build(&views.items_view)),
        None => {
            let db = crate::db::current().ok_or(crate::db::StaleEpoch)?;
            if db.epoch != request.epoch {
                return Err(crate::db::StaleEpoch);
            }
            let templates = db.templates.clone().ok_or(crate::db::StaleEpoch)?;
            Ok(build(&items_view_for_base_class(&templates.items)))
        }
    }
}

/// `ItemBaseClassNativeRequestBuilder.Build` mirrored off the resident templates root: every
/// template crosses, props-less included — deliberately NOT `build_items_view`, which drops them
/// and would cut every chain at its first node — reduced to the two members the walk reads.
/// `pub` for the flip-3 equivalence harness (`tests/flip3_oneshot_views.rs`).
pub fn items_view_for_base_class(
    items: &IndexMap<String, TemplateItem>,
) -> IndexMap<String, ItemView> {
    items
        .iter()
        .map(|(tpl, template)| {
            (
                tpl.clone(),
                ItemView {
                    // C# sends the non-nullable MongoId as-is; the walk breaks on empty either way
                    parent: Some(template.parent.clone()),
                    item_type: template.item_type.clone(),
                    ..ItemView::default()
                },
            )
        })
        .collect()
}

/// `ItemBaseClassService.HydrateItemBaseClassCache` (`ItemBaseClassService.cs:31-40`), which is
/// `AddItemToCache` (`:42-64`) per template.
pub fn build(items_view: &IndexMap<String, ItemView>) -> BaseClassResponse {
    // The walk runs over the *whole* view, not just the Items: a chain climbs through `Node`-type
    // parents, so filtering them out first would cut every chain at its first node. The chains
    // computed for non-Item tpls are simply dropped below, exactly as C# never computes them.
    let mut ancestors = ItemBaseClassCache::build(items_view).into_ancestors();

    let mut item_base_classes = IndexMap::with_capacity(ancestors.len());
    let mut root_node_ids = Vec::new();

    for (tpl, item) in items_view {
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
            // (ItemBaseClassService.cs:73-74) — the reused walk stores in the same order.
            //
            // Quirk 3, sanctioned divergence — `unwrap_or_default` is *not* C#'s empty-set seed
            // (`:56`) surviving: that seed is always overwritten, because `AddBaseItems` runs
            // unconditionally (`:57`) and opens with an unguarded `Add(item.Parent)` (`:73`). So an
            // Item-type template with an empty `_parent` keeps `{ MongoId.Empty }` there, where the
            // frozen walk breaks before storing (`loot/item_helper.rs:179-183`) and leaves `{}`.
            // No shipped Item-type template is parentless; the parity test over the real table
            // guards that.
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

    fn view(json: &str) -> IndexMap<String, ItemView> {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn minimal_projection_deserializes_into_item_view() {
        let v = view(r#"{"aaa":{"parent":"bbb","type":"Item"}}"#);
        let item = &v["aaa"];
        assert_eq!(item.parent.as_deref(), Some("bbb"));
        assert_eq!(item.item_type.as_deref(), Some("Item"));
        assert!(item.stack_max_size.is_none()); // absent members land as None
    }

    #[test]
    fn item_type_test_is_case_insensitive_and_none_is_root() {
        // "item" (lowercase) is an Item (quirk 5); missing _type is a root node.
        let v = view(
            r#"{
                "child":{"parent":"node","type":"item"},
                "node":{"parent":"root","type":"Node"},
                "untyped":{"parent":"root"}
            }"#,
        );
        let resp = build(&v);
        assert!(resp.item_base_classes.contains_key("child"));
        assert_eq!(resp.item_base_classes.len(), 1);
        assert_eq!(
            resp.root_node_ids,
            vec!["node".to_string(), "untyped".to_string()]
        );
    }

    #[test]
    fn chain_walks_through_node_parents_and_stores_the_final_missing_parent() {
        // child -> node -> root(absent from view): chain holds both, the walk
        // stores a parent before looking it up (quirk 2, ItemBaseClassService.cs:73-74).
        let v = view(
            r#"{
                "child":{"parent":"node","type":"Item"},
                "node":{"parent":"root","type":"Node"}
            }"#,
        );
        let resp = build(&v);
        let chain = &resp.item_base_classes["child"];
        assert!(chain.contains("node"));
        assert!(chain.contains("root"));
        assert_eq!(chain.len(), 2);
    }

    #[test]
    fn resident_items_view_reduces_to_parent_and_type_only() {
        use crate::db::models::TemplateItem;
        let templates: IndexMap<String, TemplateItem> = serde_json::from_str(
            r#"{"child":{"_parent":"node","_type":"Item","_props":{"Width":2}},
                "node":{"_type":"Node"}}"#,
        )
        .unwrap();
        let view = items_view_for_base_class(&templates);
        assert_eq!(view["child"].parent.as_deref(), Some("node"));
        assert_eq!(view["child"].item_type.as_deref(), Some("Item"));
        assert!(view["child"].width.is_none()); // reduced: props never cross
        // Absent _parent is the empty id (C# non-nullable MongoId); the walk breaks on it
        assert_eq!(view["node"].parent.as_deref(), Some(""));
    }

    #[test]
    fn run_resolves_override_then_resident_then_stale() {
        let _guard = crate::db::tests::DB_TEST_LOCK.lock().unwrap();
        crate::db::clear();

        // Override at epoch 0: never touches the resident DB
        let request: BaseClassRequest = serde_json::from_str(
            r#"{"epoch":0,"viewsOverride":{"itemsView":{"a":{"parent":"b","type":"Item"}}}}"#,
        )
        .unwrap();
        assert!(run(request).unwrap().item_base_classes.contains_key("a"));

        // Override-less before any publish: stale
        let request: BaseClassRequest = serde_json::from_str(r#"{"epoch":1}"#).unwrap();
        assert!(run(request).is_err());

        // Publish a templates-only mini root; the named epoch generates
        let epoch = crate::db::publish(
            serde_json::from_str(
                r#"{"schema":1,"roots":{"templates":{"items":{
                    "child":{"_parent":"node","_type":"Item"},
                    "node":{"_parent":"root","_type":"Node"}}}}}"#,
            )
            .unwrap(),
        )
        .unwrap();
        let request: BaseClassRequest =
            serde_json::from_str(&format!(r#"{{"epoch":{epoch}}}"#)).unwrap();
        let resp = run(request).unwrap();
        assert_eq!(resp.item_base_classes["child"].len(), 2);
        assert_eq!(resp.root_node_ids, vec!["node".to_string()]);

        // A mismatched epoch is stale
        let request: BaseClassRequest =
            serde_json::from_str(&format!(r#"{{"epoch":{}}}"#, epoch + 1)).unwrap();
        assert!(run(request).is_err());
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
