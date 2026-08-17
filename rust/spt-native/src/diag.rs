//! Native diagnostic emission: the locale rendering `PayloadProjection.LocaliseDiagnostic` did on
//! the C# side of the boundary, and (from Task 2) the sink generator diagnostics flow through.

use std::collections::HashMap;
use std::sync::Mutex;

use serde_json::Value;

/// The resolved server-locale table, pushed once by C# over `spt_locales_set` after the database
/// import. Overwritten wholesale on every set — the prepatch host's second push is harmless.
static LOCALES: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

pub fn set_locales(table: HashMap<String, String>) {
    let mut guard = LOCALES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *guard = Some(table);
}

/// Renders a locale-keyed line against the process-global table.
pub fn localise(key: &str, args: Option<&Value>) -> String {
    let guard = LOCALES
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    localise_with(guard.as_ref(), key, args)
}

/// `PayloadProjection.LocaliseDiagnostic` + `AbstractLocalisationService.GetLocalisedValue`: a
/// missing key (or a table that never arrived) makes the key itself the template; a scalar
/// argument replaces every `%s`; object arguments are walked member-by-member, so a placeholder no
/// member names stays literal in the output.
pub fn localise_with(
    table: Option<&HashMap<String, String>>,
    key: &str,
    args: Option<&Value>,
) -> String {
    let template = table
        .and_then(|entries| entries.get(key))
        .map(String::as_str)
        .unwrap_or(key);
    let Some(args) = args else {
        return template.to_owned();
    };
    match args {
        Value::Object(members) => {
            let mut text = template.to_owned();
            for (name, value) in members {
                text = text.replace(&format!("{{{{{name}}}}}"), &element_to_string(value));
            }
            text
        }
        scalar => template.replace("%s", &element_to_string(scalar)),
    }
}

/// `JsonElement.ToString()`: Null is empty, booleans are `bool.ToString()`, everything else is the
/// JSON text (strings unquoted).
fn element_to_string(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::Bool(true) => "True".to_owned(),
        Value::Bool(false) => "False".to_owned(),
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn table() -> HashMap<String, String> {
        HashMap::from([
            ("item-invalid_tpl_item".to_owned(), "Unable to find an item with tpl of: %s in Db".to_owned()),
            ("both-sides".to_owned(), "%s and %s".to_owned()),
            (
                "bot-mod_slot_missing_from_item".to_owned(),
                "Slot '{{modSlot}}' does not exist for item: {{parentId}} {{parentName}} on {{botRole}}".to_owned(),
            ),
            (
                "location-unable_to_find_count_distribution_for_container".to_owned(),
                "Unable to acquire count distribution data for container: {{containerTypeId}} on: {{locationName}}. defaulting to 0".to_owned(),
            ),
            ("loot-preset_pool_is_empty".to_owned(), "Unable to find random preset in the given pool as it is empty, skipping".to_owned()),
            ("typed-values".to_owned(), "n={{n}} b={{b}} s={{s}} nil={{nil}}".to_owned()),
        ])
    }

    #[test]
    fn scalar_args_replace_every_percent_s() {
        let table = table();
        assert_eq!(
            localise_with(
                Some(&table),
                "item-invalid_tpl_item",
                Some(&json!("54009119af1c881c07000029"))
            ),
            "Unable to find an item with tpl of: 54009119af1c881c07000029 in Db"
        );
        assert_eq!(
            localise_with(Some(&table), "both-sides", Some(&json!(7))),
            "7 and 7"
        );
    }

    #[test]
    fn object_args_substitute_named_placeholders() {
        let table = table();
        let args = json!({ "modSlot": "mod_stock", "parentId": "5644bd2b4bdc2d3b4c8b4572", "parentName": "AK-74N", "botRole": "assault" });
        assert_eq!(
            localise_with(Some(&table), "bot-mod_slot_missing_from_item", Some(&args)),
            "Slot 'mod_stock' does not exist for item: 5644bd2b4bdc2d3b4c8b4572 AK-74N on assault"
        );
    }

    #[test]
    fn unmatched_placeholder_stays_literal() {
        // The en.json template names {{containerTypeId}}; C# and Rust both send containerId. The
        // C# loop walks the args' members, so the stray placeholder survives — so must ours.
        let table = table();
        let args = json!({ "containerId": "578f87b7245977356274f2cd", "locationName": "bigmap" });
        assert_eq!(
            localise_with(
                Some(&table),
                "location-unable_to_find_count_distribution_for_container",
                Some(&args)
            ),
            "Unable to acquire count distribution data for container: {{containerTypeId}} on: bigmap. defaulting to 0"
        );
    }

    #[test]
    fn value_kinds_render_like_json_element_to_string() {
        // JsonElement.ToString(): Null -> "", True/False -> "True"/"False", numbers raw, strings unquoted.
        let table = table();
        let args = json!({ "n": 42, "b": true, "s": "text", "nil": null });
        assert_eq!(
            localise_with(Some(&table), "typed-values", Some(&args)),
            "n=42 b=True s=text nil="
        );
    }

    #[test]
    fn no_args_and_empty_object_args_return_the_template() {
        let table = table();
        assert_eq!(
            localise_with(Some(&table), "loot-preset_pool_is_empty", None),
            "Unable to find random preset in the given pool as it is empty, skipping"
        );
        assert_eq!(
            localise_with(Some(&table), "loot-preset_pool_is_empty", Some(&json!({}))),
            "Unable to find random preset in the given pool as it is empty, skipping"
        );
    }

    #[test]
    fn missing_key_or_missing_table_falls_back_to_the_key_as_template() {
        let table = table();
        // GetLocalisedValue returns the key; the %s pass then finds nothing to replace.
        assert_eq!(
            localise_with(
                Some(&table),
                "repeatable-quest_helper_unknown_player_group",
                Some(&json!("Pmc"))
            ),
            "repeatable-quest_helper_unknown_player_group"
        );
        assert_eq!(
            localise_with(None, "item-invalid_tpl_item", Some(&json!("x"))),
            "item-invalid_tpl_item"
        );
    }
}
