//! `Helpers/Quest/RepeatableQuestHelper.cs` — the template clone/placeholder pass every repeatable
//! quest generator opens with, plus the level-band config lookups they select their bounds from.

use indexmap::IndexMap;

use crate::loot::models::{Diagnostic, ERROR};
use crate::loot::mongo_id;
use crate::quest::QuestContext;
use crate::quest::models::{
    CompletionConfig, EliminationConfig, ExplorationConfig, RepeatableQuest, RepeatableQuestConfig,
    RepeatableQuestType,
};

/// The `typeof(T).FullName` this file's diagnostics log under.
const CATEGORY: &str = "SPTarkov.Server.Core.Helpers.Quest.RepeatableQuestHelper";

/// `Models/Enums/Traders.cs:7`.
pub(crate) const PRAPOR: &str = "54cb50c76803fa8b248b4571";
/// `Models/Enums/Traders.cs:17`.
pub(crate) const REF: &str = "6617beeaa9cfa777ca915b7c";

/// A `ServerLocalisationService.GetText` line the C# caller replays through its logger — the
/// localised text stays C#-side, so only the key and its arguments cross.
fn localised(locale_key: &str, args: serde_json::Value) -> Diagnostic {
    Diagnostic {
        category: CATEGORY,
        level: ERROR.to_owned(),
        locale_key: Some(locale_key.to_owned()),
        args: Some(args),
        message: None,
    }
}

/// `Enum.GetName(type)` (`:116`). Never null here, so the C#'s null branch (`:117-121`) is
/// unreachable in Rust.
fn template_name(quest_type: RepeatableQuestType) -> &'static str {
    match quest_type {
        RepeatableQuestType::Elimination => "Elimination",
        RepeatableQuestType::Completion => "Completion",
        RepeatableQuestType::Exploration => "Exploration",
        RepeatableQuestType::Pickup => "Pickup",
    }
}

/// `GetEliminationConfigByPmcLevel` (`:28-31`) — the first band whose `LevelRange` contains
/// `pmcLevel`, both bounds inclusive.
pub fn get_elimination_config_by_pmc_level(
    pmc_level: i32,
    repeatable_config: &RepeatableQuestConfig,
) -> Option<&EliminationConfig> {
    repeatable_config
        .quest_config
        .elimination
        .iter()
        .find(|config| pmc_level >= config.level_range.min && pmc_level <= config.level_range.max)
}

/// `GetExplorationConfigByPmcLevel` (`:39-44`).
pub fn get_exploration_config_by_pmc_level(
    pmc_level: i32,
    repeatable_config: &RepeatableQuestConfig,
) -> Option<&ExplorationConfig> {
    repeatable_config
        .quest_config
        .exploration
        .iter()
        .find(|config| pmc_level >= config.level_range.min && pmc_level <= config.level_range.max)
}

/// `GetCompletionConfigByPmcLevel` (`:52-57`).
pub fn get_completion_config_by_pmc_level(
    pmc_level: i32,
    repeatable_config: &RepeatableQuestConfig,
) -> Option<&CompletionConfig> {
    repeatable_config
        .quest_config
        .completion
        .iter()
        .find(|config| pmc_level >= config.level_range.min && pmc_level <= config.level_range.max)
}

/// `GetRepeatableQuestTemplatesByGroup` (`:187-197`). The C# throws `ArgumentOutOfRangeException`
/// for a group that is neither `Pmc` nor `Scav` (`:195`), which this ports as a `Diagnostic` + `None`
/// rather than the panic a sanctioned throw normally gets: that arm is the compiler-required default
/// of a switch over `PlayerGroup`, a closed two-member enum whose only source is
/// `RepeatableQuestConfig.Side` — string-converted at config load, so an out-of-domain value throws
/// there and never reaches this switch. The deviation is unobservable; the Rust signature only widens
/// the domain because the wire carries the group as a string.
pub fn get_repeatable_quest_templates_by_group<'a>(
    ctx: &mut QuestContext<'a>,
    player_group: &str,
) -> Option<&'a IndexMap<String, String>> {
    let templates = ctx.repeatable_quest_template_ids;

    match player_group {
        "Pmc" => Some(&templates.pmc),
        "Scav" => Some(&templates.scav),
        _ => {
            ctx.diagnostics.push(localised(
                "repeatable-quest_helper_unknown_player_group",
                serde_json::json!(player_group),
            ));

            None
        }
    }
}

/// `GetClonedQuestTemplateForType` (`:66-87`) — clone the type's template, give it a fresh id and
/// point it at the trader.
pub fn get_cloned_quest_template_for_type(
    ctx: &QuestContext<'_>,
    quest_type: RepeatableQuestType,
    trader_id: &str,
) -> Option<RepeatableQuest> {
    let repeatable_quest_templates = ctx.repeatable_quest_templates;
    let quest = match quest_type {
        RepeatableQuestType::Elimination => repeatable_quest_templates.elimination.as_ref(),
        RepeatableQuestType::Completion => repeatable_quest_templates.completion.as_ref(),
        RepeatableQuestType::Exploration => repeatable_quest_templates.exploration.as_ref(),
        RepeatableQuestType::Pickup => repeatable_quest_templates.pickup.as_ref(),
    };

    let mut quest = quest.cloned()?;

    quest.quest.id = mongo_id::generate();
    quest.quest.trader_id = trader_id.to_owned();

    Some(quest)
}

/// `GenerateRepeatableTemplate` (`:102-179`) — the base quest object the caller then fills with
/// conditions and rewards.
pub fn generate_repeatable_template(
    ctx: &mut QuestContext<'_>,
    quest_type: RepeatableQuestType,
    trader_id: &str,
    player_group: &str,
    session_id: &str,
) -> Option<RepeatableQuest> {
    let mut quest_data = match get_cloned_quest_template_for_type(ctx, quest_type, trader_id) {
        Some(quest_data) => quest_data,
        None => {
            ctx.diagnostics.push(localised(
                "repeatable-quest_helper_template_not_found",
                serde_json::json!(template_name(quest_type)),
            ));

            return None;
        }
    };

    let template_name = template_name(quest_type);

    // Get template id from config based on side and type of quest
    let type_ids = get_repeatable_quest_templates_by_group(ctx, player_group)?;
    // `GetValueOrDefault` on a `Dictionary<string, MongoId>` (`:125`): a missing type yields
    // `default(MongoId)`, whose `ToString` is `string.Empty` (`MongoId.cs:179-183`), not null.
    let template_id = type_ids.get(template_name).cloned().unwrap_or_default();
    quest_data.quest.template_id = Some(template_id.clone());

    // Force REF templates to use prapors ID - solves missing text issue
    let desired_trader_id = if trader_id == REF { PRAPOR } else { trader_id };

    //  In locale, these id correspond to the text of quests
    //  template ids -pmc  : Elimination = 616052ea3054fc0e2c24ce6e / Completion = 61604635c725987e815b1a46 / Exploration = 616041eb031af660100c9967
    //  template ids -scav : Elimination = 62825ef60e88d037dc1eb428 / Completion = 628f588ebb558574b2260fe5 / Exploration = 62825ef60e88d037dc1eb42c

    // Ported quirk, not a typo: `Name` substitutes the raw `traderId` (`:134`) while every other
    // text member takes the REF→PRAPOR substitution (`:136-166`).
    let substitute = |text: &str, trader: &str| {
        text.replace("{traderId}", trader)
            .replace("{templateId}", &template_id)
    };

    quest_data.quest.name = substitute(&quest_data.quest.name, trader_id);
    quest_data.quest.note = quest_data
        .quest
        .note
        .as_deref()
        .map(|text| substitute(text, desired_trader_id));
    quest_data.quest.description = substitute(&quest_data.quest.description, desired_trader_id);
    quest_data.quest.success_message_text = quest_data
        .quest
        .success_message_text
        .as_deref()
        .map(|text| substitute(text, desired_trader_id));
    quest_data.quest.fail_message_text = quest_data
        .quest
        .fail_message_text
        .as_deref()
        .map(|text| substitute(text, desired_trader_id));
    quest_data.quest.started_message_text = quest_data
        .quest
        .started_message_text
        .as_deref()
        .map(|text| substitute(text, desired_trader_id));
    quest_data.quest.change_quest_message_text = quest_data
        .quest
        .change_quest_message_text
        .as_deref()
        .map(|text| substitute(text, desired_trader_id));
    quest_data.quest.accept_player_message = quest_data
        .quest
        .accept_player_message
        .as_deref()
        .map(|text| substitute(text, desired_trader_id));
    quest_data.quest.decline_player_message = quest_data
        .quest
        .decline_player_message
        .as_deref()
        .map(|text| substitute(text, desired_trader_id));
    quest_data.quest.complete_player_message = quest_data
        .quest
        .complete_player_message
        .as_deref()
        .map(|text| substitute(text, desired_trader_id));

    let Some(quest_status) = quest_data.quest_status.as_mut() else {
        ctx.diagnostics.push(localised(
            "repeatable-quest_helper_no_status",
            serde_json::json!(template_name),
        ));

        return None;
    };

    quest_status.id = mongo_id::generate();
    quest_status.uid = Some(session_id.to_owned()); // Needs to match user id
    quest_status.qid = quest_data.quest.id.clone(); // Needs to match quest id

    Some(quest_data)
}

/// `GetQuestLocationByMapId` (`:204-213`) — e.g. `factory4_day` into
/// `55f2d3fd4bdc2d5f408b4567`.
pub fn get_quest_location_by_map_id<'a>(
    ctx: &mut QuestContext<'a>,
    location_key: &str,
) -> Option<&'a str> {
    let location_id_map = ctx.location_id_map;

    match location_id_map.get(location_key) {
        Some(location_id) => Some(location_id.as_str()),
        None => {
            ctx.diagnostics.push(localised(
                "repeatable-quest_helper_no_loc_id",
                serde_json::json!(location_key),
            ));

            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diag::DiagSink;
    use crate::quest::models::{
        RepeatableQuestType,
        tests::{varying, views_override},
    };
    use crate::quest::{QuestContext, QuestViews};

    /// The shipped Elimination template carries `{templateId} name {traderId}` in every text
    /// member, so the fixture views need no patching to exercise the substitution.
    #[test]
    fn generate_repeatable_template_substitutes_prapor_everywhere_but_the_name() {
        let views = QuestViews::Override(Box::new(views_override()));
        let varying = varying();
        let mut ctx = QuestContext::new(&views, &varying);
        ctx.diagnostics = DiagSink::capture();

        let quest = generate_repeatable_template(
            &mut ctx,
            RepeatableQuestType::Elimination,
            REF,
            "Pmc",
            "6193a720f8ee7e52e4290000",
        )
        .expect("template generated");

        let template_id = "616052ea3054fc0e2c24ce6e";
        assert_eq!(quest.quest.template_id.as_deref(), Some(template_id));
        assert_eq!(quest.quest.name, format!("{template_id} name {REF}"));
        assert_eq!(
            quest.quest.description,
            format!("{template_id} description {PRAPOR} 0")
        );

        let status = quest.quest_status.as_ref().expect("quest status");
        assert_eq!(status.qid, quest.quest.id);
        assert_eq!(status.uid.as_deref(), Some("6193a720f8ee7e52e4290000"));
        assert!(ctx.diagnostics.captured().is_empty());
    }

    /// The three error paths the C# logs and bails on: a type the database has no template for
    /// (`:110-114`), a group that is neither `Pmc` nor `Scav` (`:187-197`) and an unmapped
    /// location key (`:206-210`).
    #[test]
    fn the_error_paths_report_their_locale_key_and_give_up() {
        let views = QuestViews::Override(Box::new(views_override()));
        let varying = varying();
        let mut ctx = QuestContext::new(&views, &varying);
        ctx.diagnostics = DiagSink::capture();

        // The fixture only carries the Elimination template
        assert!(
            generate_repeatable_template(
                &mut ctx,
                RepeatableQuestType::Completion,
                REF,
                "Pmc",
                "6193a720f8ee7e52e4290000",
            )
            .is_none()
        );
        assert!(
            generate_repeatable_template(
                &mut ctx,
                RepeatableQuestType::Elimination,
                REF,
                "Random",
                "6193a720f8ee7e52e4290000",
            )
            .is_none()
        );
        assert_eq!(
            get_quest_location_by_map_id(&mut ctx, "bigmap"),
            Some("55f2d3fd4bdc2d5f408b4567")
        );
        assert_eq!(get_quest_location_by_map_id(&mut ctx, "rezervbase"), None);

        let keys: Vec<_> = ctx
            .diagnostics
            .captured()
            .iter()
            .map(|diagnostic| diagnostic.locale_key.as_deref().unwrap())
            .collect();
        assert_eq!(
            keys,
            [
                "repeatable-quest_helper_template_not_found",
                "repeatable-quest_helper_unknown_player_group",
                "repeatable-quest_helper_no_loc_id",
            ]
        );
    }

    /// Both `LevelRange` bounds are inclusive (`:30`), so a level sitting on a band edge picks the
    /// earlier band. `quest.json`'s first Daily exploration band is 1-15.
    #[test]
    fn the_level_bands_include_both_bounds() {
        const QUEST_CONFIG_PATH: &str = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../Libraries/SPTarkov.Server.Assets/SPT_Data/configs/quest.json"
        );

        let config: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(QUEST_CONFIG_PATH).expect("readable"))
                .expect("JSON");
        let daily: RepeatableQuestConfig =
            serde_json::from_value(config["repeatableQuests"][0].clone()).expect("parses");

        assert_eq!(
            get_exploration_config_by_pmc_level(15, &daily)
                .expect("band")
                .level_range
                .max,
            15
        );
        assert_eq!(
            get_exploration_config_by_pmc_level(16, &daily)
                .expect("band")
                .level_range
                .min,
            16
        );
        assert!(get_exploration_config_by_pmc_level(0, &daily).is_none());
        assert_eq!(
            get_elimination_config_by_pmc_level(1, &daily)
                .expect("band")
                .level_range
                .min,
            1
        );
        assert_eq!(
            get_completion_config_by_pmc_level(1, &daily)
                .expect("band")
                .level_range
                .min,
            1
        );
    }
}
