//! Fetches ClickUp tasks (every status) for the Contributors ticket scoring.
//!
//! Everything goes through the `ClickUpHttp` seam so tests feed fixtures
//! instead of the network. The live implementation reuses the Inbox's
//! `clickup_get`, so rate-limit backoff is shared with background polling.

use std::collections::HashMap;

use serde::Deserialize;
use serde_json::Value;

use super::ticket_store::{StoredTicket, TicketAssignee};

/// ClickUp returns at most this many tasks per page.
const PAGE_SIZE: usize = 100;
/// `bulk_time_in_status` accepts at most this many task ids per call.
pub(super) const BULK_IDS: usize = 100;

pub(crate) trait ClickUpHttp: Sync {
    fn get(&self, path: &str) -> Result<Value, String>;
}

pub(crate) struct LiveClickUp {
    pub token: String,
}

impl ClickUpHttp for LiveClickUp {
    fn get(&self, path: &str) -> Result<Value, String> {
        crate::clickup::clickup_get(&self.token, path)
    }
}

/// The ClickUp workspace (and optionally the spaces within it) the app-level
/// Ticket Contributors view fetches tickets from.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TicketLink {
    pub(crate) workspace_id: String,
    pub(crate) space_ids: Option<Vec<String>>,
}

/// Every task updated since `from_ms`, whatever its status. `date_updated_gt`
/// compares strictly, so it is one millisecond before `from_ms`; there is no
/// upper bound.
pub(crate) fn tasks_path(link: &TicketLink, from_ms: i64, page: u32) -> String {
    let mut path = format!(
        "/team/{}/task?include_closed=true&subtasks=true&order_by=updated\
         &date_updated_gt={}&page={page}",
        link.workspace_id,
        from_ms.saturating_sub(1)
    );
    for id in link.space_ids.iter().flatten() {
        path.push_str(&format!("&space_ids[]={id}"));
    }
    path
}

/// Reads an id or string field; ClickUp documents ids as strings but user ids
/// come back as JSON numbers.
fn text(node: &Value, key: &str) -> Option<String> {
    let value = match node.get(key)? {
        Value::String(text) => text.trim().to_string(),
        Value::Number(number) => number.to_string(),
        _ => return None,
    };
    Some(value).filter(|value| !value.is_empty())
}

fn parse_assignee(node: &Value) -> Option<TicketAssignee> {
    Some(TicketAssignee {
        id: text(node, "id")?,
        name: text(node, "username").unwrap_or_default(),
        email: text(node, "email").unwrap_or_default(),
    })
}

/// A status of one of these types means the work is done.
pub(crate) fn is_done_type(status_type: &str) -> bool {
    status_type == "done" || status_type == "closed"
}

/// Maps one task from "Get Filtered Team Tasks" or "Get Task". Every status is
/// kept; only a task without a usable id or update date is skipped. `done_at`
/// is set only for a done or closed status: custom statuses have no done date.
pub(crate) fn parse_task(node: &Value, workspace_id: &str) -> Option<StoredTicket> {
    let id = text(node, "id")?;
    let updated_ms = text(node, "date_updated")?;
    let updated_epoch = updated_ms.parse::<i64>().ok()?.div_euclid(1000);
    let status_type = node
        .pointer("/status/type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let done_at = if is_done_type(&status_type) {
        text(node, "date_done")
            .or_else(|| text(node, "date_closed"))
            .map(|ms| crate::clickup::epoch_ms_to_iso(&ms))
            .filter(|iso| !iso.is_empty())
    } else {
        None
    };
    let assignees = node
        .get("assignees")
        .and_then(Value::as_array)
        .map(|people| people.iter().filter_map(parse_assignee).collect())
        .unwrap_or_default();
    let tags = node
        .get("tags")
        .and_then(Value::as_array)
        .map(|tags| tags.iter().filter_map(|tag| text(tag, "name")).collect())
        .unwrap_or_default();
    Some(StoredTicket {
        source: "clickup".into(),
        custom_id: text(node, "custom_id"),
        title: text(node, "name").unwrap_or_default(),
        url: text(node, "url").unwrap_or_else(|| crate::clickup::task_url(&id)),
        workspace_id: workspace_id.to_string(),
        space_id: node
            .get("space")
            .and_then(|space| text(space, "id"))
            .unwrap_or_default(),
        status: node
            .pointer("/status/status")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string(),
        status_type,
        updated_at: crate::clickup::epoch_ms_to_iso(&updated_ms),
        updated_epoch,
        done_at,
        status_since: None,
        parent_id: text(node, "parent"),
        assignees,
        tags,
        description: text(node, "text_content")
            .or_else(|| text(node, "description"))
            .unwrap_or_default(),
        id,
    })
}

/// Pages through every task updated since `from_ms`. A short page ends the
/// walk (the page count is not capped), and `cancelled` is checked before each
/// page. A cancelled fetch is an error: its partial list must never be stored
/// as the full window.
pub(crate) fn fetch_tasks(
    http: &dyn ClickUpHttp,
    link: &TicketLink,
    from_ms: i64,
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<StoredTicket>, String> {
    let mut tickets = Vec::new();
    for page in 0u32.. {
        if cancelled() {
            return Err("Ticket fetch cancelled".into());
        }
        let data = http.get(&tasks_path(link, from_ms, page))?;
        let nodes = data
            .get("tasks")
            .and_then(Value::as_array)
            .ok_or_else(|| "ClickUp did not return tasks".to_string())?;
        tickets.extend(
            nodes
                .iter()
                .filter_map(|node| parse_task(node, &link.workspace_id)),
        );
        if nodes.len() < PAGE_SIZE {
            break;
        }
    }
    Ok(tickets)
}

/// When each task entered its current status, as ISO text, from the
/// time-in-status history (`current_status.total_time.since`, epoch ms). Only
/// tasks with a date appear in the map: with the "Total time in Status"
/// ClickApp off ClickUp answers `current_status` with nulls, and those tasks
/// are left out.
pub(crate) fn fetch_status_since(
    http: &dyn ClickUpHttp,
    task_ids: &[String],
) -> Result<HashMap<String, String>, String> {
    let mut since = HashMap::new();
    for chunk in task_ids.chunks(BULK_IDS) {
        let query = chunk
            .iter()
            .map(|id| format!("task_ids={id}"))
            .collect::<Vec<_>>()
            .join("&");
        let data = http.get(&format!("/task/bulk_time_in_status/task_ids?{query}"))?;
        let by_task = data
            .as_object()
            .ok_or_else(|| "ClickUp did not return time in status".to_string())?;
        for (id, entry) in by_task {
            let Some(ms) = entry
                .pointer("/current_status/total_time")
                .and_then(|total| text(total, "since"))
            else {
                continue;
            };
            let iso = crate::clickup::epoch_ms_to_iso(&ms);
            if !iso.is_empty() {
                since.insert(id.clone(), iso);
            }
        }
    }
    Ok(since)
}

/// Plain text of the first page of comments (newest first). Authors are left
/// out on purpose: the rubric reads what was said, not who said it.
pub(crate) fn fetch_comments(http: &dyn ClickUpHttp, task_id: &str) -> Result<Vec<String>, String> {
    let data = http.get(&format!("/task/{task_id}/comment"))?;
    let comments = data
        .get("comments")
        .and_then(Value::as_array)
        .ok_or_else(|| "ClickUp did not return comments".to_string())?;
    Ok(comments
        .iter()
        .filter_map(|comment| text(comment, "comment_text"))
        .collect())
}

/// Hours each task spent in statuses of type `custom` (the in-progress kind;
/// open and done statuses have their own types). Only tasks ClickUp answered
/// for appear in the map.
pub(crate) fn fetch_hours_in_progress(
    http: &dyn ClickUpHttp,
    task_ids: &[String],
) -> Result<HashMap<String, f64>, String> {
    let mut hours = HashMap::new();
    for chunk in task_ids.chunks(BULK_IDS) {
        let query = chunk
            .iter()
            .map(|id| format!("task_ids={id}"))
            .collect::<Vec<_>>()
            .join("&");
        let data = http.get(&format!("/task/bulk_time_in_status/task_ids?{query}"))?;
        let by_task = data
            .as_object()
            .ok_or_else(|| "ClickUp did not return time in status".to_string())?;
        for (id, entry) in by_task {
            let minutes: f64 = entry
                .get("status_history")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|status| status.get("type").and_then(Value::as_str) == Some("custom"))
                .filter_map(|status| status.pointer("/total_time/by_minute")?.as_f64())
                .sum();
            hours.insert(id.clone(), minutes / 60.0);
        }
    }
    Ok(hours)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::cell::Cell;
    use std::sync::Mutex;

    struct FakeHttp {
        routes: HashMap<String, Value>,
        seen: Mutex<Vec<String>>,
    }

    impl FakeHttp {
        fn new(routes: Vec<(String, Value)>) -> Self {
            Self {
                routes: routes.into_iter().collect(),
                seen: Mutex::new(Vec::new()),
            }
        }
    }

    impl ClickUpHttp for FakeHttp {
        fn get(&self, path: &str) -> Result<Value, String> {
            self.seen.lock().unwrap().push(path.to_string());
            self.routes
                .get(path)
                .cloned()
                .ok_or_else(|| format!("no fixture for {path}"))
        }
    }

    fn link(spaces: Option<Vec<&str>>) -> TicketLink {
        TicketLink {
            workspace_id: "w1".into(),
            space_ids: spaces.map(|ids| ids.into_iter().map(String::from).collect()),
        }
    }

    fn done_task(id: &str) -> Value {
        json!({
            "id": id,
            "custom_id": "ENG-7",
            "name": "Fix login",
            "url": format!("https://app.clickup.com/t/{id}"),
            "space": { "id": "s1" },
            "status": { "status": "complete", "type": "closed" },
            "date_updated": "1700020000000",
            "date_done": "1700000123000",
            "date_closed": "1700009999000",
            "parent": null,
            "assignees": [
                { "id": 42, "username": "Ada", "email": "ada@example.com" },
                { "id": 43, "username": "Bob", "email": "bob@example.com" }
            ],
            "tags": [{ "name": "bug", "tag_fg": "#fff", "tag_bg": "#f00" }],
            "text_content": "plain text",
            "description": "markdown text"
        })
    }

    fn page_of(count: usize, prefix: &str) -> Value {
        let tasks: Vec<Value> = (0..count)
            .map(|n| done_task(&format!("{prefix}{n}")))
            .collect();
        json!({ "tasks": tasks })
    }

    #[test]
    fn tasks_path_filters_by_date_updated_without_upper_bound() {
        assert_eq!(
            tasks_path(&link(None), 1000, 3),
            "/team/w1/task?include_closed=true&subtasks=true&order_by=updated\
             &date_updated_gt=999&page=3"
        );
        assert_eq!(
            tasks_path(&link(Some(vec!["a", "b"])), 1000, 0),
            "/team/w1/task?include_closed=true&subtasks=true&order_by=updated\
             &date_updated_gt=999&page=0&space_ids[]=a&space_ids[]=b"
        );
        assert!(!tasks_path(&link(None), 1000, 0).contains("date_done"));
        assert!(!tasks_path(&link(None), 1000, 0).contains("date_updated_lt"));
    }

    #[test]
    fn parse_task_reads_assignee_ids_and_emails() {
        let ticket = parse_task(&done_task("t1"), "w1").unwrap();
        assert_eq!(ticket.id, "t1");
        assert_eq!(ticket.custom_id.as_deref(), Some("ENG-7"));
        assert_eq!(ticket.title, "Fix login");
        assert_eq!(ticket.workspace_id, "w1");
        assert_eq!(ticket.space_id, "s1");
        assert_eq!(ticket.status, "complete");
        assert_eq!(ticket.status_type, "closed");
        assert_eq!(ticket.parent_id, None);
        assert_eq!(ticket.status_since, None);
        assert_eq!(ticket.tags, vec!["bug".to_string()]);
        assert_eq!(ticket.description, "plain text");
        assert_eq!(
            ticket.assignees,
            vec![
                TicketAssignee {
                    id: "42".into(),
                    name: "Ada".into(),
                    email: "ada@example.com".into()
                },
                TicketAssignee {
                    id: "43".into(),
                    name: "Bob".into(),
                    email: "bob@example.com".into()
                },
            ]
        );
    }

    /// A task in a working status: ClickUp leaves both done dates null.
    fn custom_task(id: &str, status: &str, status_type: &str) -> Value {
        let mut node = done_task(id);
        node["status"] = json!({ "status": status, "type": status_type });
        node["date_done"] = Value::Null;
        node["date_closed"] = Value::Null;
        node
    }

    #[test]
    fn parse_task_keeps_custom_and_open_statuses() {
        for (status, status_type) in [
            ("ready for casium testing", "custom"),
            ("code review pr", "custom"),
            ("in progress", "custom"),
            ("planned", "unstarted"),
            ("backlog", "open"),
            ("shipped", "closed"),
        ] {
            let ticket = parse_task(&custom_task("t1", status, status_type), "w1")
                .unwrap_or_else(|| panic!("{status} was dropped"));
            assert_eq!(ticket.status, status);
            assert_eq!(ticket.status_type, status_type);
            assert_eq!(ticket.updated_epoch, 1_700_020_000);
            assert_eq!(ticket.updated_at, "2023-11-15T03:46:40.000Z");
        }
    }

    #[test]
    fn parse_task_needs_an_id_and_an_update_date() {
        let mut node = done_task("t1");
        node["date_updated"] = Value::Null;
        assert!(parse_task(&node, "w1").is_none());
        let mut node = done_task("t1");
        node["id"] = Value::Null;
        assert!(parse_task(&node, "w1").is_none());
    }

    #[test]
    fn parse_task_sets_done_at_only_for_done_or_closed() {
        for status_type in ["done", "closed"] {
            let mut node = done_task("t1");
            node["status"]["type"] = json!(status_type);
            let ticket = parse_task(&node, "w1").unwrap();
            assert_eq!(ticket.done_at.as_deref(), Some("2023-11-14T22:15:23.000Z"));
        }
        // A stale done date on a task that is back in a working status is ignored.
        for status_type in ["open", "custom", "unstarted"] {
            let mut node = done_task("t1");
            node["status"]["type"] = json!(status_type);
            assert_eq!(parse_task(&node, "w1").unwrap().done_at, None);
        }
        assert_eq!(
            parse_task(&custom_task("t1", "in progress", "custom"), "w1")
                .unwrap()
                .done_at,
            None
        );
    }

    #[test]
    fn parse_task_prefers_date_done_over_date_closed() {
        let ticket = parse_task(&done_task("t1"), "w1").unwrap();
        assert_eq!(ticket.done_at.as_deref(), Some("2023-11-14T22:15:23.000Z"));

        let mut node = done_task("t1");
        node["date_done"] = Value::Null;
        let ticket = parse_task(&node, "w1").unwrap();
        assert_eq!(ticket.done_at.as_deref(), Some("2023-11-15T00:59:59.000Z"));
    }

    #[test]
    fn parse_task_falls_back_for_url_description_and_parent() {
        let mut node = done_task("t1");
        node["url"] = Value::Null;
        node["text_content"] = json!("");
        node["parent"] = json!("p9");
        let ticket = parse_task(&node, "w1").unwrap();
        assert_eq!(ticket.url, "https://app.clickup.com/t/t1");
        assert_eq!(ticket.description, "markdown text");
        assert_eq!(ticket.parent_id.as_deref(), Some("p9"));
    }

    #[test]
    fn fetch_tasks_pages_until_a_short_page() {
        let l = link(None);
        let http = FakeHttp::new(vec![
            (tasks_path(&l, 1, 0), page_of(100, "a")),
            (tasks_path(&l, 1, 1), page_of(100, "b")),
            (tasks_path(&l, 1, 2), page_of(5, "c")),
        ]);
        let tickets = fetch_tasks(&http, &l, 1, &|| false).unwrap();
        assert_eq!(tickets.len(), 205);
        assert_eq!(http.seen.lock().unwrap().len(), 3);
    }

    #[test]
    fn fetch_tasks_stops_when_cancelled() {
        let l = link(None);
        let http = FakeHttp::new(vec![
            (tasks_path(&l, 1, 0), page_of(100, "a")),
            (tasks_path(&l, 1, 1), page_of(100, "b")),
        ]);
        let checks = Cell::new(0);
        let cancelled = || {
            checks.set(checks.get() + 1);
            checks.get() > 1
        };
        let result = fetch_tasks(&http, &l, 1, &cancelled);
        assert!(result.is_err());
        assert_eq!(http.seen.lock().unwrap().len(), 1);
    }

    #[test]
    fn fetch_comments_returns_plain_text_only() {
        let http = FakeHttp::new(vec![(
            "/task/t1/comment".into(),
            json!({ "comments": [
                { "id": "c1", "comment_text": "looks good", "user": { "username": "Ada" } },
                { "id": "c2", "comment_text": "  " }
            ] }),
        )]);
        assert_eq!(
            fetch_comments(&http, "t1").unwrap(),
            vec!["looks good".to_string()]
        );
    }

    #[test]
    fn fetch_hours_in_progress_sums_custom_status_minutes() {
        let http = FakeHttp::new(vec![(
            "/task/bulk_time_in_status/task_ids?task_ids=t1&task_ids=t2".into(),
            json!({
                "t1": {
                    "current_status": { "status": "complete", "total_time": { "by_minute": 9999 } },
                    "status_history": [
                        { "status": "open", "type": "open", "total_time": { "by_minute": 600 } },
                        { "status": "in progress", "type": "custom", "total_time": { "by_minute": 90 } },
                        { "status": "review", "type": "custom", "total_time": { "by_minute": 30 } },
                        { "status": "complete", "type": "closed", "total_time": { "by_minute": 5 } }
                    ]
                },
                "t2": { "status_history": [] }
            }),
        )]);
        let ids = vec!["t1".to_string(), "t2".to_string()];
        let hours = fetch_hours_in_progress(&http, &ids).unwrap();
        assert_eq!(hours["t1"], 2.0);
        assert_eq!(hours["t2"], 0.0);
    }

    #[test]
    fn fetch_hours_in_progress_batches_at_one_hundred_ids() {
        let ids: Vec<String> = (0..150).map(|n| format!("t{n}")).collect();
        let query = |range: std::ops::Range<usize>| {
            let joined = range
                .map(|n| format!("task_ids=t{n}"))
                .collect::<Vec<_>>()
                .join("&");
            format!("/task/bulk_time_in_status/task_ids?{joined}")
        };
        let http = FakeHttp::new(vec![
            (query(0..100), json!({})),
            (query(100..150), json!({})),
        ]);
        assert!(fetch_hours_in_progress(&http, &ids).unwrap().is_empty());
        assert_eq!(http.seen.lock().unwrap().len(), 2);
    }

    #[test]
    fn fetch_status_since_reads_current_status_since() {
        let http = FakeHttp::new(vec![(
            "/task/bulk_time_in_status/task_ids?task_ids=t1&task_ids=t2&task_ids=t3".into(),
            json!({
                "t1": {
                    "current_status": {
                        "status": "ready for casium testing",
                        "total_time": { "by_minute": 120, "since": "1700000123000" }
                    },
                    "status_history": []
                },
                "t2": {
                    "current_status": {
                        "status": "code review pr",
                        "total_time": { "by_minute": 5, "since": 1700009999000i64 }
                    }
                },
                "t3": {
                    "current_status": { "status": "in progress", "total_time": { "by_minute": 5, "since": null } }
                }
            }),
        )]);
        let ids = vec!["t1".to_string(), "t2".to_string(), "t3".to_string()];
        let since = fetch_status_since(&http, &ids).unwrap();
        assert_eq!(since["t1"], "2023-11-14T22:15:23.000Z");
        assert_eq!(since["t2"], "2023-11-15T00:59:59.000Z");
        assert!(!since.contains_key("t3"));
    }

    #[test]
    fn fetch_status_since_returns_empty_when_history_is_off() {
        // The shape measured on a workspace with the ClickApp off.
        let http = FakeHttp::new(vec![(
            "/task/bulk_time_in_status/task_ids?task_ids=t1&task_ids=t2&task_ids=t3".into(),
            json!({
                "t1": {
                    "current_status": { "status": null, "total_time": null },
                    "status_history": []
                },
                "t2": { "status_history": [] },
                "t3": null
            }),
        )]);
        let ids = vec!["t1".to_string(), "t2".to_string(), "t3".to_string()];
        assert!(fetch_status_since(&http, &ids).unwrap().is_empty());
    }

    #[test]
    fn fetch_status_since_batches_at_one_hundred_ids() {
        let ids: Vec<String> = (0..150).map(|n| format!("t{n}")).collect();
        let query = |range: std::ops::Range<usize>| {
            let joined = range
                .map(|n| format!("task_ids=t{n}"))
                .collect::<Vec<_>>()
                .join("&");
            format!("/task/bulk_time_in_status/task_ids?{joined}")
        };
        let http = FakeHttp::new(vec![
            (query(0..100), json!({})),
            (query(100..150), json!({})),
        ]);
        assert!(fetch_status_since(&http, &ids).unwrap().is_empty());
        assert_eq!(http.seen.lock().unwrap().len(), 2);
    }
}
