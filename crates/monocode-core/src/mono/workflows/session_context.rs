//! Where a workflow node's session came from, so a question forwarded to Telegram can say so
//! (`workflowSessionContext.ts` `workflowItemInfo` and the runner's `rememberSessionContext`).
//! The map from session to context lives with the runner; this is the pure part.
use super::model::{parse_branch, WorkflowRun};
use super::ordered::{js_number_text, Json};
use super::template::js_trim;
use crate::telegram::forward::Workflow as SessionContext;

const LABEL_FIELDS: [&str; 4] = ["title", "name", "summary", "subject"];
const ID_FIELDS: [&str; 4] = ["identifier", "key", "id", "number"];

/// A readable label and id for a For each item.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ItemInfo {
    pub label: Option<String>,
    pub id: Option<String>,
}

fn first_text(item: &Json, fields: &[&str]) -> Option<String> {
    fields.iter().find_map(|field| match item.get(field)? {
        Json::String(text) if !js_trim(text).is_empty() => Some(js_trim(text).to_string()),
        Json::Number(n) if n.is_finite() => Some(js_number_text(*n)),
        _ => None,
    })
}

/// The label and id of a For each item, when it has the usual fields (`title`/`name`/`summary`/
/// `subject` and `identifier`/`key`/`id`/`number`); a plain string is its own label.
pub fn workflow_item_info(item: Option<&Json>) -> ItemInfo {
    match item {
        Some(Json::String(text)) => ItemInfo {
            label: Some(js_trim(text).to_string()).filter(|t| !t.is_empty()),
            id: None,
        },
        Some(item @ Json::Object(_)) => ItemInfo {
            label: first_text(item, &LABEL_FIELDS),
            id: first_text(item, &ID_FIELDS),
        },
        _ => ItemInfo::default(),
    }
}

/// The context of the session `node_id` runs in on `branch`: the workflow, the node, the For each
/// item of the branch, and the loop round when the workflow loops.
pub fn session_context(run: &WorkflowRun, node_id: &str, branch: &str) -> SessionContext {
    let title = run
        .definition
        .node(node_id)
        .and_then(|node| node.title.as_deref())
        .map(js_trim)
        .filter(|title| !title.is_empty())
        .unwrap_or(node_id);
    let item =
        parse_branch(branch).and_then(|split| run.items.get(&split.group_id)?.get(split.index));
    let info = workflow_item_info(item);
    let looped = run
        .definition
        .edges
        .iter()
        .any(|edge| edge.loop_spec.is_some());
    let round = run
        .nodes
        .iter()
        .find(|n| n.node_id == node_id && n.branch == branch)
        .filter(|_| looped)
        .map(|n| u64::from(n.round));
    SessionContext {
        workflow_name: run.definition.name.clone(),
        node_title: title.to_string(),
        item_label: info.label,
        item_id: info.id,
        round,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mono::workflows::engine::start_run;
    use crate::mono::workflows::fixtures::{example, json};
    use crate::mono::workflows::model::{NodeRun, RunTrigger};

    fn info(text: &str) -> ItemInfo {
        workflow_item_info(Some(&json(text)))
    }

    #[test]
    fn reads_an_item_label_and_id_from_a_loop_item() {
        assert_eq!(
            info(r#"{"key": "ABC-1", "title": "Crash on start"}"#),
            ItemInfo {
                id: Some("ABC-1".into()),
                label: Some("Crash on start".into())
            }
        );
        assert_eq!(
            info(r#"{"number": 12}"#),
            ItemInfo {
                id: Some("12".into()),
                label: None
            }
        );
        assert_eq!(
            info(r#""plain""#),
            ItemInfo {
                label: Some("plain".into()),
                id: None
            }
        );
        assert_eq!(workflow_item_info(None), ItemInfo::default());
    }

    #[test]
    fn takes_the_first_usable_field_in_order() {
        // `title` is blank, so `name` is the label; `identifier` beats `key`.
        assert_eq!(
            info(
                r#"{"title": "  ", "name": " Fix it ", "summary": "s", "key": "K", "identifier": "ID-9"}"#
            ),
            ItemInfo {
                label: Some("Fix it".into()),
                id: Some("ID-9".into())
            }
        );
        // `title` beats `name`, and `identifier` beats `key`, whatever order the item lists them.
        assert_eq!(
            info(r#"{"name": "N", "key": "K", "title": "T", "identifier": "I"}"#),
            ItemInfo {
                label: Some("T".into()),
                id: Some("I".into())
            }
        );
        // A number is a usable id or label only when finite; a blank string is nothing.
        assert_eq!(
            info(r#"{"id": 1.5, "title": 7}"#).id.as_deref(),
            Some("1.5")
        );
        assert_eq!(info(r#"{"title": 7}"#).label.as_deref(), Some("7"));
        assert_eq!(info(r#""   ""#), ItemInfo::default());
    }

    #[test]
    fn things_that_are_not_text_or_an_object_have_nothing_to_show() {
        for item in [
            "null",
            "true",
            "3",
            "[1, 2]",
            r#"{"title": true, "key": null}"#,
        ] {
            assert_eq!(info(item), ItemInfo::default(), "{item}");
        }
    }

    /// The ClickUp example: `dev` and `review` sit in the For each `each` and loop up to 3 rounds.
    fn looping() -> WorkflowRun {
        let mut def = example();
        def.nodes.iter_mut().find(|n| n.id == "dev").unwrap().title = Some(" Fix ".into());
        let mut run = start_run(&def, RunTrigger::Manual, 1, "run-1");
        run.items.insert(
            "each".into(),
            vec![
                json(r#"{"key":"A-1","title":"One"}"#),
                json(r#"{"key":"B-2","title":"Two"}"#),
            ],
        );
        run
    }

    #[test]
    fn a_branch_session_carries_its_item_and_round() {
        let mut run = looping();
        let mut node = NodeRun::new("dev", "each#1");
        node.round = 2;
        run.nodes.push(node);
        let ctx = session_context(&run, "dev", "each#1");
        assert_eq!(ctx.workflow_name, "ClickUp -> Dev -> Review");
        assert_eq!(ctx.node_title, "Fix");
        assert_eq!(ctx.item_id.as_deref(), Some("B-2"));
        assert_eq!(ctx.item_label.as_deref(), Some("Two"));
        assert_eq!(ctx.round, Some(2));
    }

    #[test]
    fn a_top_level_node_without_a_title_is_named_by_its_id_and_has_no_item() {
        let mut run = looping();
        run.nodes.push(NodeRun::new("fetch", ""));
        let ctx = session_context(&run, "fetch", "");
        assert_eq!(ctx.node_title, "fetch");
        assert_eq!((ctx.item_id, ctx.item_label), (None, None));
        assert_eq!(ctx.round, Some(1));
    }

    #[test]
    fn the_round_is_left_out_when_the_workflow_does_not_loop() {
        let mut run = looping();
        run.definition.edges.retain(|e| e.loop_spec.is_none());
        run.nodes.push(NodeRun::new("dev", "each#0"));
        assert_eq!(session_context(&run, "dev", "each#0").round, None);
    }

    #[test]
    fn an_item_that_is_not_there_gives_no_item_info() {
        let run = looping();
        let ctx = session_context(&run, "dev", "each#5");
        assert_eq!((ctx.item_id, ctx.item_label), (None, None));
        // No node entry either, so no round.
        assert_eq!(ctx.round, None);
    }
}
