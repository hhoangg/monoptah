//! Native ClickUp Inbox source. Port of the Tauri client: same requests, parsers and
//! `clickup-config.json` format. Every public fn takes the data dir and blocks.
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::secret_file::write_secret_file;

const API_BASE: &str = "https://api.clickup.com/api/v2";
const HTTP_TIMEOUT: Duration = Duration::from_secs(20);
const DEFAULT_LIMIT: u32 = 40;
const DEFAULT_LIMIT_WITH_CLOSED: u32 = 100;
const MAX_LIMIT: u32 = 100;
// ClickUp returns at most 100 tasks per page of the filtered team endpoint.
const TASK_PAGE_SIZE: usize = 100;
const MAX_TASK_PAGES: u32 = 5;
const COMMENT_LIMIT: usize = 50;
// ClickUp returns the 25 newest comments per request.
const COMMENT_PAGE_SIZE: usize = 25;
const MAX_COMMENT_PAGES: usize = 5;
const DEFAULT_BACKOFF: Duration = Duration::from_secs(60);
const MAX_BACKOFF: Duration = Duration::from_secs(120);
/// The error every ClickUp call returns while a rate-limit backoff is running.
/// Recognise it with `is_rate_limited`, not by matching text.
pub(crate) const RATE_LIMITED: &str = "ClickUp rate limited, retry shortly";

// Shared by every call so a 429 pauses background Inbox polling too.
static RATE_LIMIT_BACKOFF: Mutex<Option<Instant>> = Mutex::new(None);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClickUpStatus {
    pub connected: bool,
    pub team_id: String,
    pub team_name: String,
    pub username: String,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ClickUpConfig {
    pub(crate) token: String,
    pub(crate) team_id: String,
    team_name: String,
    // Cached from `GET /user` so the status check stays offline.
    #[serde(default)]
    username: String,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ClickUpWorkspace {
    pub id: String,
    pub name: String,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ClickUpSpace {
    pub id: String,
    pub name: String,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ClickUpLabel {
    pub name: String,
    pub color: String,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ClickUpAssignee {
    pub login: String,
    pub avatar_url: String,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ClickUpIssue {
    pub provider: String,
    pub kind: String,
    pub id: String,
    pub identifier: String,
    pub number: i64,
    pub title: String,
    pub url: String,
    pub state: String,
    pub state_type: String,
    pub updated_at: String,
    pub labels: Vec<ClickUpLabel>,
    pub assignees: Vec<ClickUpAssignee>,
    pub draft: bool,
    pub repo: String,
    pub team_id: String,
    pub team_name: String,
    pub list_name: String,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ClickUpIssueDetails {
    pub body: String,
    pub author: String,
    pub author_avatar_url: String,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ClickUpIssueComment {
    pub id: String,
    pub kind: String,
    pub author: String,
    pub author_avatar_url: String,
    pub body: String,
    pub created_at: String,
    pub url: String,
    pub state: String,
    pub path: String,
    pub line: Option<i64>,
    pub resolved: bool,
    pub thread_id: String,
    pub replies: Vec<ClickUpIssueComment>,
}

#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ClickUpIssueThread {
    pub comments: Vec<ClickUpIssueComment>,
    pub truncated: bool,
    pub review_decision: String,
    pub base_ref_name: String,
    pub head_ref_name: String,
}

pub fn clickup_status(app: PathBuf) -> Result<ClickUpStatus, String> {
    let config = read_config(&app)?;
    Ok(status_for(config.as_ref()))
}

pub fn clickup_set_config(
    app: PathBuf,
    token: String,
    team_id: Option<String>,
) -> Result<ClickUpStatus, String> {
    let token = token.trim().to_string();
    if token.is_empty() {
        delete_config(&app)?;
        return Ok(status_for(None));
    }
    let user = clickup_get(&token, "/user")?;
    let username = user
        .pointer("/user/username")
        .and_then(Value::as_str)
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .ok_or_else(|| "ClickUp did not return the current user".to_string())?;
    let workspaces = parse_clickup_workspaces(&clickup_get(&token, "/team")?)?;
    let workspace = select_workspace(&workspaces, team_id.as_deref())?;
    let config = ClickUpConfig {
        token,
        team_id: workspace.id,
        team_name: workspace.name,
        username,
    };
    write_config(&app, &config)?;
    Ok(status_for(Some(&config)))
}

pub fn clickup_list_workspaces(
    app: PathBuf,
    token: Option<String>,
) -> Result<Vec<ClickUpWorkspace>, String> {
    let token = workspace_lookup_token(token, || Ok(require_config(&app)?.token))?;
    parse_clickup_workspaces(&clickup_get(&token, "/team")?)
}

pub fn clickup_list_spaces(app: PathBuf) -> Result<Vec<ClickUpSpace>, String> {
    let config = require_config(&app)?;
    fetch_spaces(&config)
}

/// `space_ids` of `None` or empty means every space.
pub fn clickup_list_issues(
    app: PathBuf,
    space_ids: Option<Vec<String>>,
    include_closed: bool,
    limit: Option<usize>,
) -> Result<Vec<ClickUpIssue>, String> {
    let Some(config) = read_config(&app)? else {
        return Ok(Vec::new());
    };
    let space_ids = space_ids.unwrap_or_default();
    let limit = effective_limit(
        limit.map(|limit| limit.min(MAX_LIMIT as usize) as u32),
        include_closed,
    ) as usize;
    // Validate before any request so a bad id fails instead of widening the query.
    let first_path = tasks_path(&config.team_id, &space_ids, include_closed, 0)?;
    // Tasks only carry the space id, so names come from the space list.
    // Names are cosmetic: a failure here must not hide the tasks.
    let spaces = fetch_spaces(&config).unwrap_or_default();
    let mut issues: Vec<ClickUpIssue> = Vec::new();
    for page in 0..MAX_TASK_PAGES {
        let path = if page == 0 {
            first_path.clone()
        } else {
            tasks_path(&config.team_id, &space_ids, include_closed, page)?
        };
        let data = clickup_get(&config.token, &path)?;
        let raw_count = data["tasks"].as_array().map_or(0, Vec::len);
        issues.extend(parse_clickup_issues(&data, &spaces)?);
        // The API's default sort direction is undocumented, so read every
        // page (not just enough for the cap) and sort locally below.
        if raw_count < TASK_PAGE_SIZE {
            break;
        }
    }
    Ok(newest_first(issues, limit))
}

pub fn clickup_issue_details(app: PathBuf, task_id: String) -> Result<ClickUpIssueDetails, String> {
    let config = require_config(&app)?;
    let task_id = require_task_id(&task_id)?;
    let data = clickup_get(
        &config.token,
        &format!("/task/{task_id}?include_markdown_description=true"),
    )?;
    parse_clickup_issue_details(&data)
}

pub fn clickup_issue_thread(app: PathBuf, task_id: String) -> Result<ClickUpIssueThread, String> {
    let config = require_config(&app)?;
    let task_id = require_task_id(&task_id)?;
    fetch_clickup_thread(task_id, |cursor| {
        let path = match cursor {
            Some((date, id)) => format!("/task/{task_id}/comment?start={date}&start_id={id}"),
            None => format!("/task/{task_id}/comment"),
        };
        clickup_get(&config.token, &path)
    })
}

pub fn clickup_issue_comment(
    app: PathBuf,
    task_id: String,
    body: String,
) -> Result<String, String> {
    let config = require_config(&app)?;
    let task_id = require_task_id(&task_id)?;
    let body = body.trim();
    if body.is_empty() {
        return Err("Comment cannot be empty".into());
    }
    // `notify_all` is required by the API; false skips pinging the author.
    let data = clickup_post(
        &config.token,
        &format!("/task/{task_id}/comment"),
        &json!({ "comment_text": body, "notify_all": false }),
    )?;
    created_comment_id(&data)?;
    Ok(task_url(task_id))
}

/// The id of the comment ClickUp just created. A missing id means the post did
/// not take, so it must be an error rather than a silent success.
fn created_comment_id(data: &Value) -> Result<String, String> {
    value_id(data, "id").ok_or_else(|| "Could not post ClickUp comment".to_string())
}

fn status_for(config: Option<&ClickUpConfig>) -> ClickUpStatus {
    ClickUpStatus {
        connected: config.is_some(),
        team_id: config
            .map(|config| config.team_id.clone())
            .unwrap_or_default(),
        team_name: config
            .map(|config| config.team_name.clone())
            .unwrap_or_default(),
        username: config
            .map(|config| config.username.clone())
            .unwrap_or_default(),
    }
}

fn effective_limit(limit: Option<u32>, include_closed: bool) -> u32 {
    let default = if include_closed {
        DEFAULT_LIMIT_WITH_CLOSED
    } else {
        DEFAULT_LIMIT
    };
    limit.unwrap_or(default).clamp(1, MAX_LIMIT)
}

/// Sorts by `updated_at` descending, so the cap keeps the most recently updated
/// tasks whichever direction the API returned them in. ISO strings sort
/// lexicographically; tasks without a timestamp go last.
fn newest_first(mut issues: Vec<ClickUpIssue>, limit: usize) -> Vec<ClickUpIssue> {
    issues.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
    issues.truncate(limit);
    issues
}

fn tasks_path(
    team_id: &str,
    space_ids: &[String],
    include_closed: bool,
    page: u32,
) -> Result<String, String> {
    let mut path = format!(
        "/team/{team_id}/task?order_by=updated&subtasks=true&include_closed={include_closed}"
    );
    for id in space_ids.iter().map(|id| id.trim()) {
        // Dropping an unusable id would silently widen the query to every space.
        if id.is_empty() || !id.chars().all(|ch| ch.is_ascii_digit()) {
            return Err(format!("Invalid ClickUp space id \"{id}\""));
        }
        path.push_str(&format!("&space_ids[]={id}"));
    }
    path.push_str(&format!("&page={page}"));
    Ok(path)
}

fn fetch_spaces(config: &ClickUpConfig) -> Result<Vec<ClickUpSpace>, String> {
    fetch_spaces_for(&config.token, &config.team_id)
}

pub(crate) fn fetch_spaces_for(token: &str, team_id: &str) -> Result<Vec<ClickUpSpace>, String> {
    parse_clickup_spaces(&clickup_get(
        token,
        &format!("/team/{team_id}/space?archived=false"),
    )?)
}

pub(crate) fn clickup_get(token: &str, path: &str) -> Result<Value, String> {
    wait_for_backoff()?;
    let agent = ureq::AgentBuilder::new().timeout(HTTP_TIMEOUT).build();
    let result = agent
        .get(&format!("{API_BASE}{path}"))
        // Personal tokens (`pk_...`) go in the header as-is, without `Bearer`.
        .set("Authorization", token)
        .set("Accept", "application/json")
        .call();
    read_clickup_response(result)
}

fn clickup_post(token: &str, path: &str, body: &Value) -> Result<Value, String> {
    wait_for_backoff()?;
    let agent = ureq::AgentBuilder::new().timeout(HTTP_TIMEOUT).build();
    let payload = serde_json::to_string(body).map_err(|error| error.to_string())?;
    let result = agent
        .post(&format!("{API_BASE}{path}"))
        .set("Authorization", token)
        .set("Accept", "application/json")
        .set("Content-Type", "application/json")
        .send_string(&payload);
    read_clickup_response(result)
}

/// Whether `error` came from a ClickUp rate limit (a 429, or a call made while
/// the backoff after one is running). Such an error is transient.
// Reused by the ticket contributors jobs; nothing calls it before that lands.
#[allow(dead_code)]
pub(crate) fn is_rate_limited(error: &str) -> bool {
    error == RATE_LIMITED
}

fn wait_for_backoff() -> Result<(), String> {
    match RATE_LIMIT_BACKOFF.lock() {
        Ok(mut slot) => backoff_error(&mut slot, Instant::now()),
        Err(_) => Ok(()),
    }
}

fn backoff_error(slot: &mut Option<Instant>, now: Instant) -> Result<(), String> {
    if slot.is_some_and(|until| now < until) {
        return Err(RATE_LIMITED.into());
    }
    *slot = None;
    Ok(())
}

/// ClickUp sends `X-RateLimit-Reset` as epoch seconds; without it, wait a minute.
fn backoff_duration(reset_header: Option<&str>, now_epoch_secs: u64) -> Duration {
    reset_header
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(|reset| Duration::from_secs(reset.saturating_sub(now_epoch_secs).max(1)))
        .map_or(DEFAULT_BACKOFF, |wait| wait.min(MAX_BACKOFF))
}

fn start_backoff(reset_header: Option<&str>) {
    let now_epoch_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let until = Instant::now() + backoff_duration(reset_header, now_epoch_secs);
    if let Ok(mut slot) = RATE_LIMIT_BACKOFF.lock() {
        *slot = Some(until);
    }
}

fn read_clickup_response(result: Result<ureq::Response, ureq::Error>) -> Result<Value, String> {
    let response = match result {
        Ok(response) => response,
        Err(ureq::Error::Status(401, _)) => {
            return Err("ClickUp API token is invalid or expired".into());
        }
        Err(ureq::Error::Status(429, response)) => {
            start_backoff(response.header("X-RateLimit-Reset"));
            return Err(RATE_LIMITED.into());
        }
        Err(ureq::Error::Status(status, response)) => {
            let body = response.into_string().unwrap_or_default();
            return Err(clickup_http_error(status, &body));
        }
        Err(_) => return Err("Could not reach ClickUp".into()),
    };
    let status = response.status();
    let body = response
        .into_string()
        .map_err(|_| "ClickUp returned an unreadable response".to_string())?;
    if !(200..300).contains(&status) {
        return Err(clickup_http_error(status, &body));
    }
    serde_json::from_str(&body).map_err(|_| "ClickUp returned invalid JSON".to_string())
}

fn clickup_http_error(status: u16, body: &str) -> String {
    if let Some(message) = clickup_error_message(body) {
        return message;
    }
    match status {
        403 => "ClickUp denied access. Check the API token's permissions".into(),
        404 => "ClickUp could not find that item".into(),
        _ => format!("ClickUp request failed ({status})"),
    }
}

fn clickup_error_message(body: &str) -> Option<String> {
    let parsed: Value = serde_json::from_str(body).ok()?;
    parsed
        .get("err")
        .and_then(Value::as_str)
        .map(|message| message.trim().to_string())
        .filter(|message| !message.is_empty())
}

fn parse_clickup_workspaces(data: &Value) -> Result<Vec<ClickUpWorkspace>, String> {
    let teams = data
        .get("teams")
        .and_then(Value::as_array)
        .ok_or_else(|| "ClickUp did not return workspaces".to_string())?;
    Ok(teams
        .iter()
        .filter_map(|team| {
            let id = value_string(team, "id").filter(|id| !id.is_empty())?;
            let name = value_string(team, "name")
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| id.clone());
            Some(ClickUpWorkspace { id, name })
        })
        .collect())
}

/// A token that has several workspaces cannot be saved until one is chosen, so
/// Settings lists them with the typed token before any config exists.
fn workspace_lookup_token(
    typed: Option<String>,
    stored: impl FnOnce() -> Result<String, String>,
) -> Result<String, String> {
    match typed.map(|token| token.trim().to_string()) {
        Some(token) if !token.is_empty() => Ok(token),
        _ => stored(),
    }
}

fn select_workspace(
    workspaces: &[ClickUpWorkspace],
    requested: Option<&str>,
) -> Result<ClickUpWorkspace, String> {
    match workspaces {
        [] => Err("This ClickUp token has no workspaces".into()),
        [only] => Ok(only.clone()),
        _ => {
            let requested = requested.map(str::trim).filter(|id| !id.is_empty());
            if let Some(found) = requested.and_then(|id| workspaces.iter().find(|w| w.id == id)) {
                return Ok(found.clone());
            }
            let available = workspaces
                .iter()
                .map(|workspace| format!("{} ({})", workspace.name, workspace.id))
                .collect::<Vec<_>>()
                .join(", ");
            Err(match requested {
                Some(id) => format!("ClickUp workspace {id} was not found. Available: {available}"),
                None => format!("Choose a ClickUp workspace. Available: {available}"),
            })
        }
    }
}

pub(crate) fn parse_clickup_spaces(data: &Value) -> Result<Vec<ClickUpSpace>, String> {
    let spaces = data
        .get("spaces")
        .and_then(Value::as_array)
        .ok_or_else(|| "ClickUp did not return spaces".to_string())?;
    Ok(spaces
        .iter()
        .filter_map(|space| {
            let id = value_string(space, "id").filter(|id| !id.is_empty())?;
            let name = value_string(space, "name")
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| id.clone());
            Some(ClickUpSpace { id, name })
        })
        .collect())
}

fn parse_clickup_issues(
    data: &Value,
    spaces: &[ClickUpSpace],
) -> Result<Vec<ClickUpIssue>, String> {
    let tasks = data
        .get("tasks")
        .and_then(Value::as_array)
        .ok_or_else(|| "ClickUp did not return tasks".to_string())?;
    Ok(tasks
        .iter()
        .filter_map(|task| parse_clickup_issue(task, spaces))
        .collect())
}

fn parse_clickup_issue(node: &Value, spaces: &[ClickUpSpace]) -> Option<ClickUpIssue> {
    let id = value_string(node, "id").filter(|id| !id.is_empty())?;
    let custom_id = value_string(node, "custom_id").filter(|custom| !custom.is_empty());
    let status = node.get("status");
    let space_id = node
        .pointer("/space/id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let space_name = spaces
        .iter()
        .find(|space| space.id == space_id)
        .map(|space| space.name.clone())
        .unwrap_or_default();
    let labels = node
        .get("tags")
        .and_then(Value::as_array)
        .map(|tags| {
            tags.iter()
                .filter_map(|tag| {
                    let name = value_string(tag, "name").filter(|name| !name.is_empty())?;
                    Some(ClickUpLabel {
                        name,
                        color: value_string(tag, "tag_bg").unwrap_or_default(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let assignees = node
        .get("assignees")
        .and_then(Value::as_array)
        .map(|people| {
            people
                .iter()
                .filter_map(|person| {
                    let (login, avatar_url) = person_fields(Some(person));
                    Some(ClickUpAssignee {
                        login: login?,
                        avatar_url,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Some(ClickUpIssue {
        provider: "clickup".into(),
        kind: "clickup".into(),
        number: custom_id.as_deref().map_or(0, issue_number),
        identifier: issue_identifier(custom_id.as_deref(), &id),
        title: value_string(node, "name").unwrap_or_default(),
        url: value_string(node, "url")
            .filter(|url| !url.is_empty())
            .unwrap_or_else(|| task_url(&id)),
        state: status
            .and_then(|value| value_string(value, "status"))
            .filter(|state| !state.is_empty())
            .unwrap_or_else(|| "Open".into()),
        state_type: state_type(
            status
                .and_then(|value| value.get("type"))
                .and_then(Value::as_str)
                .unwrap_or_default(),
        )
        .into(),
        updated_at: epoch_ms_to_iso(&value_string(node, "date_updated").unwrap_or_default()),
        labels,
        assignees,
        draft: false,
        repo: space_name.clone(),
        team_id: space_id,
        team_name: space_name,
        list_name: node
            .pointer("/list/name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string(),
        id,
    })
}

fn parse_clickup_issue_details(data: &Value) -> Result<ClickUpIssueDetails, String> {
    if !data.is_object() || value_string(data, "id").is_none() {
        return Err("ClickUp did not return that task".into());
    }
    let body = ["markdown_description", "description"]
        .into_iter()
        .filter_map(|key| value_string(data, key))
        .find(|text| !text.is_empty())
        .unwrap_or_default();
    let assignee = data
        .get("assignees")
        .and_then(Value::as_array)
        .and_then(|people| people.first());
    let (author, author_avatar_url) = [data.get("creator"), assignee]
        .into_iter()
        .map(person_fields)
        .find(|(name, _)| name.is_some())
        .unwrap_or((None, String::new()));
    Ok(ClickUpIssueDetails {
        body,
        author: author.unwrap_or_default(),
        author_avatar_url,
    })
}

// Comments arrive newest first in pages. Fetch until one more than the limit
// is seen so `truncated` is exact, then keep the latest ones oldest first.
fn fetch_clickup_thread(
    task_id: &str,
    mut fetch: impl FnMut(Option<(String, String)>) -> Result<Value, String>,
) -> Result<ClickUpIssueThread, String> {
    let mut parsed: Vec<ClickUpIssueComment> = Vec::new();
    let mut cursor = None;
    for _ in 0..MAX_COMMENT_PAGES {
        let data = fetch(cursor.take())?;
        let comments = data
            .get("comments")
            .and_then(Value::as_array)
            .ok_or_else(|| "ClickUp did not return comments".to_string())?;
        let last = comments.last().and_then(|comment| {
            Some((value_string(comment, "date")?, value_string(comment, "id")?))
        });
        parsed.extend(
            comments
                .iter()
                .filter_map(|comment| parse_clickup_comment(comment, task_id)),
        );
        if parsed.len() > COMMENT_LIMIT || comments.len() < COMMENT_PAGE_SIZE {
            break;
        }
        match last {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    let truncated = parsed.len() > COMMENT_LIMIT;
    parsed.truncate(COMMENT_LIMIT);
    parsed.sort_by(|left, right| left.created_at.cmp(&right.created_at));
    Ok(ClickUpIssueThread {
        comments: parsed,
        truncated,
        review_decision: String::new(),
        base_ref_name: String::new(),
        head_ref_name: String::new(),
    })
}

fn parse_clickup_comment(node: &Value, task_id: &str) -> Option<ClickUpIssueComment> {
    let id = value_string(node, "id").filter(|id| !id.is_empty())?;
    let (author, author_avatar_url) = person_fields(node.get("user"));
    Some(ClickUpIssueComment {
        // No documented per-comment deep link, so comments point at the task.
        url: task_url(task_id),
        kind: "comment".into(),
        author: author.unwrap_or_default(),
        author_avatar_url,
        body: comment_markdown(node),
        created_at: epoch_ms_to_iso(&value_string(node, "date").unwrap_or_default()),
        state: String::new(),
        path: String::new(),
        line: None,
        resolved: node["resolved"].as_bool().unwrap_or(false),
        thread_id: String::new(),
        replies: Vec::new(),
        id,
    })
}

/// ClickUp stores comment formatting as runs in the `comment` array and only
/// keeps a plain-text flattening in `comment_text`. Rebuild markdown from the
/// runs, falling back to the flattening when the runs are missing or empty.
fn comment_markdown(node: &Value) -> String {
    let from_runs = node
        .get("comment")
        .and_then(Value::as_array)
        .map(|runs| runs.iter().filter_map(run_markdown).collect::<String>())
        .map(|body| body.trim().to_string())
        .filter(|body| !body.is_empty());
    with_hard_breaks(
        &from_runs.unwrap_or_else(|| value_string(node, "comment_text").unwrap_or_default()),
    )
}

/// The comment renderer folds a single newline into a space, but a ClickUp
/// comment uses single newlines as real line breaks. Mark them as markdown hard
/// breaks (two trailing spaces). Blank lines already separate paragraphs.
fn with_hard_breaks(body: &str) -> String {
    let lines: Vec<&str> = body.split('\n').collect();
    let mut out = String::with_capacity(body.len() + 8);
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        let next_has_text = lines
            .get(index + 1)
            .is_some_and(|next| !next.trim().is_empty());
        if next_has_text && !line.trim().is_empty() {
            out.push_str(line.trim_end());
            out.push_str("  ");
        } else {
            out.push_str(line);
        }
    }
    out
}

/// Renders one run. Only `bold`, `italic`, `code` and `link` are understood;
/// any other attribute leaves the run as plain text rather than dropping it.
fn run_markdown(run: &Value) -> Option<String> {
    let text = run.get("text").and_then(Value::as_str)?;
    let attributes = run.get("attributes");
    let flag = |key: &str| {
        attributes
            .and_then(|attributes| attributes.get(key))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    };
    let (bold, italic, code) = (flag("bold"), flag("italic"), flag("code"));
    let link = attributes
        .and_then(|attributes| attributes.get("link"))
        .and_then(link_destination);
    // Markers must hug the text, and emphasis cannot span a line break, so
    // each line is wrapped on its own with its edge whitespace kept outside.
    let lines: Vec<String> = text
        .split('\n')
        .map(|line| {
            let core = line.trim();
            if core.is_empty() {
                return line.to_string();
            }
            let start = line.len() - line.trim_start().len();
            let end = start + core.len();
            let mut out = if code {
                code_span(core)
            } else {
                escape_markdown(core)
            };
            out = match (bold, italic) {
                (true, true) => format!("***{out}***"),
                (true, false) => format!("**{out}**"),
                (false, true) => format!("*{out}*"),
                (false, false) => out,
            };
            if let Some(url) = &link {
                out = format!("[{out}]({url})");
            }
            format!("{}{out}{}", &line[..start], &line[end..])
        })
        .collect();
    Some(lines.join("\n"))
}

/// The documented link attribute is a plain URL string. An object with a `url`
/// is accepted too in case a workspace returns that. Anything that is not a
/// web or mail address is rejected so the run degrades to plain text.
fn link_destination(value: &Value) -> Option<String> {
    let raw = match value {
        Value::String(url) => url.as_str(),
        Value::Object(map) => map.get("url")?.as_str()?,
        _ => return None,
    }
    .trim();
    let lower = raw.to_ascii_lowercase();
    let allowed = ["http://", "https://", "mailto:"]
        .iter()
        .any(|scheme| lower.starts_with(scheme));
    if !allowed {
        return None;
    }
    let mut url = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            ' ' | '(' | ')' | '<' | '>' | '\\' => url.push_str(&format!("%{:02X}", ch as u32)),
            ch if ch.is_control() => {}
            ch => url.push(ch),
        }
    }
    Some(url)
}

/// Escapes markdown metacharacters in a single line so what the user typed,
/// such as a literal `**`, is shown as typed. Backslash-escaping any ASCII
/// punctuation is always valid markdown, so over-escaping is harmless, except
/// inside a bare URL: ClickUp sends links as plain text, the renderer
/// autolinks them, and a backslash would stay in the destination. Those spans
/// are copied through untouched.
fn escape_markdown(line: &str) -> String {
    let mut out = String::with_capacity(line.len() + 4);
    // Block markers only matter at the start of a line.
    let mut rest = line;
    match line.chars().next() {
        Some('#' | '+' | '-' | '=') => out.push('\\'),
        Some(first) if first.is_ascii_digit() => {
            let digits = line.len()
                - line
                    .trim_start_matches(|ch: char| ch.is_ascii_digit())
                    .len();
            out.push_str(&line[..digits]);
            rest = &line[digits..];
            if rest.starts_with(['.', ')']) {
                out.push('\\');
            }
        }
        _ => {}
    }
    let mut previous = None;
    while let Some(ch) = rest.chars().next() {
        let span = bare_url_len(rest).filter(|_| !previous.is_some_and(char::is_alphanumeric));
        if let Some(len) = span {
            out.push_str(&rest[..len]);
            previous = rest[..len].chars().last();
            rest = &rest[len..];
            continue;
        }
        if matches!(
            ch,
            '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '>' | '~' | '|' | '&' | '$'
        ) {
            out.push('\\');
        }
        out.push(ch);
        previous = Some(ch);
        rest = &rest[ch.len_utf8()..];
    }
    out
}

/// Length of an `http://` or `https://` span at the start of `text`, up to
/// whitespace. It also stops at `<` and a backtick, which would otherwise open
/// HTML or code. Returns `None` when `text` does not start with a URL.
fn bare_url_len(text: &str) -> Option<usize> {
    let lower = text.get(..8).unwrap_or(text).to_ascii_lowercase();
    let scheme = ["https://", "http://"]
        .into_iter()
        .find(|scheme| lower.starts_with(scheme))?;
    let end = text
        .find(|ch: char| ch.is_whitespace() || ch == '<' || ch == '`')
        .unwrap_or(text.len());
    (end > scheme.len()).then_some(end)
}

/// Inline code cannot be escaped, so use a fence longer than any backtick run
/// inside the text.
fn code_span(text: &str) -> String {
    let (mut longest, mut current) = (0, 0);
    for ch in text.chars() {
        current = if ch == '`' { current + 1 } else { 0 };
        longest = longest.max(current);
    }
    let fence = "`".repeat(longest + 1);
    let pad = if text.starts_with('`') || text.ends_with('`') {
        " "
    } else {
        ""
    };
    format!("{fence}{pad}{text}{pad}{fence}")
}

pub(crate) fn task_url(task_id: &str) -> String {
    format!("https://app.clickup.com/t/{task_id}")
}

fn issue_identifier(custom_id: Option<&str>, id: &str) -> String {
    match custom_id.map(str::trim).filter(|custom| !custom.is_empty()) {
        Some(custom) => custom.to_string(),
        None => format!("#{id}"),
    }
}

/// Maps ClickUp's status types onto the Jira-style categories the Inbox understands.
fn state_type(clickup_type: &str) -> &'static str {
    match clickup_type.trim().to_ascii_lowercase().as_str() {
        // `unstarted` is a status in the not-started group, like `open`.
        "open" | "unstarted" => "new",
        "custom" => "indeterminate",
        "closed" | "done" => "done",
        // A type ClickUp adds later stays open work. Only an explicit done
        // value may mark a task finished, because the Inbox hides those.
        _ => "indeterminate",
    }
}

fn issue_number(custom_id: &str) -> i64 {
    custom_id
        .rsplit('-')
        .next()
        .and_then(|number| number.parse().ok())
        .unwrap_or(0)
}

/// ClickUp timestamps are epoch milliseconds in a string; the app uses ISO-8601 UTC.
pub(crate) fn epoch_ms_to_iso(value: &str) -> String {
    let Ok(ms) = value.trim().parse::<i64>() else {
        return String::new();
    };
    let secs = ms.div_euclid(1000);
    let millis = ms.rem_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rest = secs.rem_euclid(86_400);
    // Civil-from-days (Howard Hinnant), valid for the whole proleptic calendar.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// Drops hidden ids whose space no longer exists. The API rejects any space id
/// it cannot use, so a stale preference must never reach a request.
pub fn reconcile_hidden_spaces(spaces: &[ClickUpSpace], hidden: &[String]) -> Vec<String> {
    hidden
        .iter()
        .filter(|id| spaces.iter().any(|space| &space.id == *id))
        .cloned()
        .collect()
}

/// `None` means do not filter by space; `Some(vec![])` means every known space
/// is hidden. Every returned id comes from `spaces`, never from the preference.
pub fn space_ids_for_fetch(spaces: &[ClickUpSpace], hidden: &[String]) -> Option<Vec<String>> {
    let hidden = reconcile_hidden_spaces(spaces, hidden);
    if hidden.is_empty() {
        return None;
    }
    Some(
        spaces
            .iter()
            .map(|space| space.id.clone())
            .filter(|id| !hidden.contains(id))
            .collect(),
    )
}

/// The API has no assignee filter, so "assigned to me" is matched locally on the
/// username cached with the token. Without one nothing matches: showing the whole
/// workspace would mislead.
pub fn assigned_to(issues: Vec<ClickUpIssue>, username: &str) -> Vec<ClickUpIssue> {
    let me = username.trim().to_lowercase();
    if me.is_empty() {
        return Vec::new();
    }
    issues
        .into_iter()
        .filter(|issue| {
            issue
                .assignees
                .iter()
                .any(|person| person.login.trim().to_lowercase() == me)
        })
        .collect()
}

/// The widest window the API allows. "Assigned to me" is filtered locally, so it
/// needs this or older assigned tasks would silently drop out.
const INBOX_ALL_LIMIT: usize = 100;

/// Inbox fetch rules over an injected backend `list(space_ids, include_closed, limit)`.
/// `spaces` is the live space list; `hidden` the user's hidden space ids.
pub fn inbox_issues(
    spaces: &[ClickUpSpace],
    hidden: &[String],
    username: &str,
    assigned_to_me: bool,
    include_closed: bool,
    list: impl FnOnce(Option<Vec<String>>, bool, Option<usize>) -> Result<Vec<ClickUpIssue>, String>,
) -> Result<Vec<ClickUpIssue>, String> {
    let space_ids = if hidden.is_empty() {
        None
    } else {
        space_ids_for_fetch(spaces, hidden)
    };
    if space_ids.as_ref().is_some_and(Vec::is_empty) {
        return Ok(Vec::new());
    }
    let limit = (include_closed || assigned_to_me).then_some(INBOX_ALL_LIMIT);
    let issues = list(space_ids, include_closed, limit)?;
    let visible: Vec<ClickUpIssue> = issues
        .into_iter()
        .filter(|issue| !hidden.contains(&issue.team_id))
        .collect();
    Ok(if assigned_to_me {
        assigned_to(visible, username)
    } else {
        visible
    })
}

fn require_task_id(task_id: &str) -> Result<&str, String> {
    let task_id = task_id.trim();
    let valid = !task_id.is_empty()
        && task_id.len() < 64
        && task_id
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'));
    if valid {
        Ok(task_id)
    } else {
        Err("Missing ClickUp task".into())
    }
}

fn person_fields(node: Option<&Value>) -> (Option<String>, String) {
    let Some(node) = node.filter(|node| node.is_object()) else {
        return (None, String::new());
    };
    (
        value_string(node, "username").filter(|name| !name.is_empty()),
        value_string(node, "profilePicture").unwrap_or_default(),
    )
}

fn value_string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(|text| text.trim().to_string())
}

/// Reads an id that ClickUp documents as a string but may return as a JSON number.
/// Kept apart from `value_string`: every other field there really is a string.
fn value_id(value: &Value, key: &str) -> Option<String> {
    let id = match value.get(key)? {
        Value::String(text) => text.trim().to_string(),
        Value::Number(number) => number.to_string(),
        _ => return None,
    };
    Some(id).filter(|id| !id.is_empty())
}

// ---- Config storage ----

fn config_path(app: &Path) -> PathBuf {
    app.join("clickup-config.json")
}

fn read_config(app: &Path) -> Result<Option<ClickUpConfig>, String> {
    let path = config_path(app);
    match fs::read_to_string(path) {
        Ok(raw) => {
            let mut config: ClickUpConfig = serde_json::from_str(&raw)
                .map_err(|_| "ClickUp settings are invalid".to_string())?;
            config.token = config.token.trim().to_string();
            config.team_id = config.team_id.trim().to_string();
            config.team_name = config.team_name.trim().to_string();
            config.username = config.username.trim().to_string();
            if config.token.is_empty() || config.team_id.is_empty() {
                Ok(None)
            } else {
                Ok(Some(config))
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

pub(crate) fn require_config(app: &Path) -> Result<ClickUpConfig, String> {
    read_config(app)?.ok_or_else(|| "Connect ClickUp in Settings".to_string())
}

fn write_config(app: &Path, config: &ClickUpConfig) -> Result<(), String> {
    let path = config_path(app);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let value = serde_json::to_string(config).map_err(|error| error.to_string())?;
    write_secret_file(&path, &value)
}

fn delete_config(app: &Path) -> Result<(), String> {
    let path = config_path(app);
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spaces() -> Vec<ClickUpSpace> {
        vec![ClickUpSpace {
            id: "90100".into(),
            name: "Engineering".into(),
        }]
    }

    fn task() -> Value {
        json!({
            "id": "86abc12",
            "custom_id": null,
            "name": "Fix login",
            "status": { "status": "in progress", "type": "custom" },
            "date_updated": "1700000000123",
            "tags": [{ "name": "bug", "tag_fg": "#fff", "tag_bg": "#ff0000" }],
            "assignees": [{ "id": 1, "username": "ada", "profilePicture": "https://x/ada.png" }],
            "url": "https://app.clickup.com/t/86abc12",
            "list": { "id": "1", "name": "Sprint 4" },
            "space": { "id": "90100" }
        })
    }

    #[test]
    fn maps_status_types() {
        assert_eq!(state_type("open"), "new");
        assert_eq!(state_type("unstarted"), "new");
        assert_eq!(state_type(" Unstarted "), "new");
        assert_eq!(state_type("custom"), "indeterminate");
        assert_eq!(state_type("closed"), "done");
        assert_eq!(state_type("done"), "done");
        assert_eq!(state_type("Closed"), "done");
        assert_eq!(state_type(""), "indeterminate");
        // An unrecognised future type is open work, never done.
        assert_eq!(state_type("paused"), "indeterminate");
    }

    #[test]
    fn converts_epoch_ms_strings_to_iso() {
        assert_eq!(epoch_ms_to_iso("0"), "1970-01-01T00:00:00.000Z");
        assert_eq!(epoch_ms_to_iso("1700000000123"), "2023-11-14T22:13:20.123Z");
        // Leap day, and a pre-epoch value.
        assert_eq!(
            epoch_ms_to_iso(" 1709164800000 "),
            "2024-02-29T00:00:00.000Z"
        );
        assert_eq!(epoch_ms_to_iso("-1"), "1969-12-31T23:59:59.999Z");
        assert_eq!(epoch_ms_to_iso(""), "");
        assert_eq!(epoch_ms_to_iso("soon"), "");
    }

    #[test]
    fn prefers_custom_id_over_hash_id() {
        assert_eq!(issue_identifier(Some("ENG-42"), "86abc12"), "ENG-42");
        assert_eq!(issue_identifier(None, "86abc12"), "#86abc12");
        assert_eq!(issue_identifier(Some("  "), "86abc12"), "#86abc12");
        assert_eq!(issue_number("ENG-42"), 42);
        assert_eq!(issue_number("nonumber"), 0);
    }

    #[test]
    fn parses_a_task_into_the_jira_shape() {
        let issue = parse_clickup_issue(&task(), &spaces()).unwrap();
        assert_eq!(issue.provider, "clickup");
        assert_eq!(issue.kind, "clickup");
        assert_eq!(issue.id, "86abc12");
        assert_eq!(issue.identifier, "#86abc12");
        assert_eq!(issue.title, "Fix login");
        assert_eq!(issue.state, "in progress");
        assert_eq!(issue.state_type, "indeterminate");
        assert_eq!(issue.updated_at, "2023-11-14T22:13:20.123Z");
        assert_eq!(issue.labels[0].name, "bug");
        assert_eq!(issue.labels[0].color, "#ff0000");
        assert_eq!(issue.assignees[0].login, "ada");
        assert_eq!(issue.assignees[0].avatar_url, "https://x/ada.png");
        assert_eq!(issue.team_id, "90100");
        assert_eq!(issue.team_name, "Engineering");
        assert_eq!(issue.list_name, "Sprint 4");

        let mut with_custom = task();
        with_custom["custom_id"] = json!("ENG-7");
        assert_eq!(
            parse_clickup_issue(&with_custom, &spaces())
                .unwrap()
                .identifier,
            "ENG-7"
        );
        // An unknown space keeps the task and just drops the name.
        assert_eq!(parse_clickup_issue(&task(), &[]).unwrap().team_name, "");
        assert!(parse_clickup_issue(&json!({ "name": "no id" }), &[]).is_none());
    }

    #[test]
    fn serializes_issues_with_camel_case_keys() {
        let value = serde_json::to_value(parse_clickup_issue(&task(), &spaces()).unwrap()).unwrap();
        for key in ["stateType", "updatedAt", "teamId", "teamName", "listName"] {
            assert!(value.get(key).is_some(), "missing {key}");
        }
        assert!(value["assignees"][0].get("avatarUrl").is_some());
    }

    #[test]
    fn details_prefer_markdown_description() {
        let data = json!({
            "id": "1",
            "markdown_description": "**hi**",
            "description": "hi",
            "creator": { "username": "ada", "profilePicture": "https://x/a.png" }
        });
        let details = parse_clickup_issue_details(&data).unwrap();
        assert_eq!(details.body, "**hi**");
        assert_eq!(details.author, "ada");
        let plain = json!({ "id": "1", "markdown_description": "", "description": "plain" });
        assert_eq!(parse_clickup_issue_details(&plain).unwrap().body, "plain");
        assert!(parse_clickup_issue_details(&json!({})).is_err());
    }

    #[test]
    fn builds_task_paths_with_numeric_space_filters() {
        let path = tasks_path("123", &["90100".into(), " 90101 ".into()], true, 2).unwrap();
        assert_eq!(
            path,
            "/team/123/task?order_by=updated&subtasks=true&include_closed=true\
             &space_ids[]=90100&space_ids[]=90101&page=2"
        );
        assert!(tasks_path("123", &[], false, 0)
            .unwrap()
            .ends_with("include_closed=false&page=0"));
    }

    #[test]
    fn rejects_unusable_space_ids_instead_of_widening_the_query() {
        let error = tasks_path("123", &["90100".into(), "bad id".into()], false, 0).unwrap_err();
        assert_eq!(error, "Invalid ClickUp space id \"bad id\"");
        assert!(tasks_path("123", &["".into()], false, 0).is_err());
        assert!(tasks_path("123", &["12/../3".into()], false, 0).is_err());
    }

    #[test]
    fn cap_keeps_the_newest_tasks_whatever_order_the_api_returns() {
        let updated = |id: &str, ms: &str| {
            let mut node = task();
            node["id"] = json!(id);
            node["date_updated"] = json!(ms);
            node
        };
        // Oldest first, with an undated task in the middle.
        let mut undated = updated("undated", "0");
        undated["date_updated"] = json!("");
        let payload = json!({ "tasks": [
            updated("old", "1600000000000"),
            undated,
            updated("newest", "1800000000000"),
            updated("mid", "1700000000000"),
        ] });
        let issues = parse_clickup_issues(&payload, &spaces()).unwrap();
        let kept = newest_first(issues.clone(), 2);
        assert_eq!(
            kept.iter()
                .map(|issue| issue.id.as_str())
                .collect::<Vec<_>>(),
            vec!["newest", "mid"]
        );
        let all = newest_first(issues, 10);
        assert_eq!(all.last().unwrap().id, "undated");
    }

    fn response(raw: &str) -> ureq::Response {
        raw.parse().unwrap()
    }

    #[test]
    fn maps_401_to_an_invalid_token_message() {
        let error = read_clickup_response(Err(ureq::Error::Status(
            401,
            response("HTTP/1.1 401 Unauthorized\r\n\r\n{\"err\":\"Token invalid\"}"),
        )))
        .unwrap_err();
        assert_eq!(error, "ClickUp API token is invalid or expired");
    }

    #[test]
    fn maps_429_to_a_rate_limit_message_and_starts_the_backoff() {
        let error = read_clickup_response(Err(ureq::Error::Status(
            429,
            response("HTTP/1.1 429 Too Many Requests\r\nX-RateLimit-Reset: 1\r\n\r\n{}"),
        )))
        .unwrap_err();
        assert_eq!(error, RATE_LIMITED);
        assert_ne!(error, "ClickUp API token is invalid or expired");
        // The shared slot is set, so the next request fails fast.
        assert_eq!(wait_for_backoff().unwrap_err(), RATE_LIMITED);
        *RATE_LIMIT_BACKOFF.lock().unwrap() = None;
        assert!(wait_for_backoff().is_ok());
    }

    #[test]
    fn maps_other_statuses_and_successes() {
        let error = read_clickup_response(Err(ureq::Error::Status(
            500,
            response("HTTP/1.1 500 Internal Server Error\r\n\r\n"),
        )))
        .unwrap_err();
        assert_eq!(error, "ClickUp request failed (500)");
        let ok =
            read_clickup_response(Ok(response("HTTP/1.1 200 OK\r\n\r\n{\"teams\":[]}"))).unwrap();
        assert_eq!(ok, json!({ "teams": [] }));
        assert_eq!(
            read_clickup_response(Ok(response("HTTP/1.1 200 OK\r\n\r\nnot json"))).unwrap_err(),
            "ClickUp returned invalid JSON"
        );
    }

    #[test]
    fn caps_the_task_limit() {
        assert_eq!(effective_limit(None, false), 40);
        assert_eq!(effective_limit(None, true), 100);
        assert_eq!(effective_limit(Some(500), false), 100);
        assert_eq!(effective_limit(Some(0), false), 1);
    }

    #[test]
    fn selects_the_only_workspace_or_names_the_choices() {
        let one = vec![ClickUpWorkspace {
            id: "1".into(),
            name: "Acme".into(),
        }];
        assert_eq!(select_workspace(&one, None).unwrap().id, "1");
        let two = vec![
            one[0].clone(),
            ClickUpWorkspace {
                id: "2".into(),
                name: "Globex".into(),
            },
        ];
        assert_eq!(select_workspace(&two, Some(" 2 ")).unwrap().name, "Globex");
        let missing = select_workspace(&two, None).unwrap_err();
        assert!(missing.contains("Acme (1)") && missing.contains("Globex (2)"));
        assert!(select_workspace(&two, Some("9")).unwrap_err().contains("9"));
        assert!(select_workspace(&[], None).is_err());
    }

    #[test]
    fn workspace_lookup_prefers_the_typed_token() {
        let typed = workspace_lookup_token(Some(" pk_typed ".into()), || panic!("not needed"));
        assert_eq!(typed.unwrap(), "pk_typed");
        let stored = workspace_lookup_token(None, || Ok("pk_stored".into()));
        assert_eq!(stored.unwrap(), "pk_stored");
        let blank = workspace_lookup_token(Some("  ".into()), || Ok("pk_stored".into()));
        assert_eq!(blank.unwrap(), "pk_stored");
        let missing = workspace_lookup_token(None, || Err("Connect ClickUp in Settings".into()));
        assert_eq!(missing.unwrap_err(), "Connect ClickUp in Settings");
    }

    #[test]
    fn maps_http_errors() {
        assert_eq!(
            clickup_http_error(400, r#"{"err":"Team not authorized","ECODE":"OAUTH_023"}"#),
            "Team not authorized"
        );
        assert!(clickup_http_error(403, "").contains("denied access"));
        assert_eq!(clickup_http_error(500, ""), "ClickUp request failed (500)");
    }

    #[test]
    fn rate_limit_errors_are_recognised_by_their_constant() {
        assert!(is_rate_limited(RATE_LIMITED));
        assert!(!is_rate_limited("ClickUp could not find that item"));
        assert!(!is_rate_limited("Could not reach ClickUp"));
    }

    #[test]
    fn rate_limit_backoff_blocks_until_reset() {
        let now = Instant::now();
        let mut slot = Some(now + Duration::from_secs(30));
        assert_eq!(backoff_error(&mut slot, now).unwrap_err(), RATE_LIMITED);
        assert!(slot.is_some());
        assert!(backoff_error(&mut slot, now + Duration::from_secs(31)).is_ok());
        assert!(slot.is_none());

        assert_eq!(
            backoff_duration(Some("1030"), 1000),
            Duration::from_secs(30)
        );
        assert_eq!(backoff_duration(Some("900"), 1000), Duration::from_secs(1));
        assert_eq!(backoff_duration(Some("99999"), 1000), MAX_BACKOFF);
        assert_eq!(backoff_duration(Some("soon"), 1000), DEFAULT_BACKOFF);
        assert_eq!(backoff_duration(None, 1000), DEFAULT_BACKOFF);
    }

    #[test]
    fn thread_keeps_latest_fifty_oldest_first() {
        let comment = |n: u64| {
            json!({
                "id": n.to_string(),
                "comment_text": format!("c{n}"),
                "date": (1_700_000_000_000u64 + n * 1000).to_string(),
                "user": { "username": "ada" }
            })
        };
        // 60 comments, newest first, served 25 at a time.
        let all: Vec<Value> = (1..=60).rev().map(comment).collect();
        let mut cursors = Vec::new();
        let thread = fetch_clickup_thread("86abc12", |cursor| {
            cursors.push(cursor.clone());
            let start = (cursors.len() - 1) * COMMENT_PAGE_SIZE;
            let end = (start + COMMENT_PAGE_SIZE).min(all.len());
            Ok(json!({ "comments": all[start..end] }))
        })
        .unwrap();
        assert!(thread.truncated);
        assert_eq!(thread.comments.len(), COMMENT_LIMIT);
        assert_eq!(thread.comments.first().unwrap().body, "c11");
        assert_eq!(thread.comments.last().unwrap().body, "c60");
        assert_eq!(cursors.len(), 3);
        // The cursor is the date and id of the oldest comment on the last page.
        assert_eq!(
            cursors[1],
            Some(("1700000036000".to_string(), "36".to_string()))
        );
        assert_eq!(thread.comments[0].author, "ada");
    }

    #[test]
    fn short_thread_is_not_truncated() {
        let thread = fetch_clickup_thread("t", |_| {
            Ok(json!({ "comments": [
                { "id": "2", "comment_text": "b", "date": "2000" },
                { "id": "1", "comment_text": "a", "date": "1000" }
            ] }))
        })
        .unwrap();
        assert!(!thread.truncated);
        assert_eq!(thread.comments[0].body, "a");
        assert_eq!(thread.comments[0].url, "https://app.clickup.com/t/t");
    }

    #[test]
    fn created_comment_needs_an_id_in_either_shape() {
        let live = json!({ "id": 1100290000049524u64, "hist_id": "5294670272230211025" });
        assert_eq!(created_comment_id(&live).unwrap(), "1100290000049524");
        assert_eq!(
            created_comment_id(&json!({ "id": "1100290000049524" })).unwrap(),
            "1100290000049524"
        );
        for data in [json!({}), json!({ "id": "" }), json!({ "id": null })] {
            assert_eq!(
                created_comment_id(&data).unwrap_err(),
                "Could not post ClickUp comment"
            );
        }
    }

    #[test]
    fn reads_ids_as_number_or_string() {
        // The live comment POST returns a number, the API reference a string.
        let live = json!({ "id": 1100290000049524u64, "hist_id": "5294670272230211025" });
        assert_eq!(value_id(&live, "id").as_deref(), Some("1100290000049524"));
        let documented = json!({ "id": " 1100290000049524 " });
        assert_eq!(
            value_id(&documented, "id").as_deref(),
            Some("1100290000049524")
        );
        assert_eq!(value_id(&json!({ "id": "" }), "id"), None);
        assert_eq!(value_id(&json!({ "id": null }), "id"), None);
        assert_eq!(value_id(&json!({ "id": true }), "id"), None);
        assert_eq!(value_id(&json!({}), "id"), None);
    }

    fn comment_body(runs: Value) -> String {
        comment_markdown(&json!({ "comment_text": "flat", "comment": runs }))
    }

    #[test]
    fn comment_runs_become_markdown() {
        // Shape observed on the live API.
        let body = comment_body(json!([
            { "text": "a", "attributes": { "bold": true } },
            { "text": "\n\n", "attributes": {} },
            { "text": "b", "attributes": { "bold": true, "italic": true } },
            { "text": " ", "attributes": {} },
            { "text": "c", "attributes": { "italic": true } },
            { "text": " ", "attributes": {} },
            { "text": "d", "attributes": { "code": true } }
        ]));
        assert_eq!(body, "**a**\n\n***b*** *c* `d`");
    }

    #[test]
    fn literal_asterisks_stay_literal_next_to_real_bold() {
        let body = comment_body(json!([
            { "attributes": {}, "text": "**asd adas \n**" },
            { "attributes": { "italic": true, "bold": true }, "text": "12312313" }
        ]));
        assert_eq!(body, "\\*\\*asd adas  \n\\*\\****12312313***");
    }

    #[test]
    fn comment_text_is_escaped_and_code_is_not() {
        assert_eq!(
            comment_body(json!([{ "text": "a_b [x] <i> `c` 1. \\", "attributes": {} }])),
            "a\\_b \\[x\\] \\<i\\> \\`c\\` 1. \\\\"
        );
        assert_eq!(
            comment_body(json!([{ "text": "# - 2) x", "attributes": {} }])),
            "\\# - 2) x"
        );
        assert_eq!(
            comment_body(json!([{ "text": "2) x", "attributes": {} }])),
            "2\\) x"
        );
        assert_eq!(
            comment_body(json!([{ "text": "a*b", "attributes": { "code": true } }])),
            "`a*b`"
        );
        assert_eq!(
            comment_body(json!([{ "text": "a`b", "attributes": { "code": true } }])),
            "``a`b``"
        );
        assert_eq!(
            comment_body(json!([{ "text": "`x`", "attributes": { "code": true } }])),
            "`` `x` ``"
        );
    }

    #[test]
    fn emphasis_hugs_text_and_does_not_span_lines() {
        let body = comment_body(json!([
            { "text": "x ", "attributes": {} },
            { "text": " hi \nthere ", "attributes": { "bold": true } },
            { "text": "y", "attributes": {} }
        ]));
        assert_eq!(body, "x  **hi**  \n**there** y");
    }

    #[test]
    fn bare_urls_pass_through_unescaped() {
        // Observed on the live API: a URL is plain text in an ordinary run.
        let body = comment_body(json!([
            { "text": "probe see https://example.com/a?b=1&c=2 end" }
        ]));
        assert_eq!(body, "probe see https://example.com/a?b=1&c=2 end");
        assert_eq!(
            comment_body(json!([
                { "text": "HTTP://a.com/x_y*z]. and_b ", "attributes": { "bold": true } }
            ])),
            "**HTTP://a.com/x_y*z]. and\\_b**"
        );
        // Not at a word boundary, no host, or a stop character: escaped as text.
        assert_eq!(
            comment_body(json!([{ "text": "xhttp://a_b http:// a_b" }])),
            "xhttp://a\\_b http:// a\\_b"
        );
        assert_eq!(
            comment_body(json!([{ "text": "http://a_b<i>" }])),
            "http://a_b\\<i\\>"
        );
    }

    #[test]
    fn single_newlines_become_hard_breaks() {
        assert_eq!(
            comment_body(json!([{ "text": "one\ntwo\n\nthree" }])),
            "one  \ntwo\n\nthree"
        );
        // A newline in its own run still joins the lines around it.
        assert_eq!(
            comment_body(json!([
                { "text": "a", "attributes": { "bold": true } },
                { "text": "\n" },
                { "text": "b" }
            ])),
            "**a**  \nb"
        );
        // The flat fallback gets the same treatment.
        assert_eq!(
            comment_markdown(&json!({ "comment_text": "x\ny" })),
            "x  \ny"
        );
    }

    #[test]
    fn comment_links_use_the_link_attribute() {
        assert_eq!(
            comment_body(json!([{
                "text": "docs",
                "attributes": { "link": "https://clickup.com/a b(1)" }
            }])),
            "[docs](https://clickup.com/a%20b%281%29)"
        );
        assert_eq!(
            comment_body(json!([{
                "text": "docs",
                "attributes": { "link": "https://x.test", "bold": true }
            }])),
            "[**docs**](https://x.test)"
        );
        assert_eq!(
            comment_body(json!([{
                "text": "docs",
                "attributes": { "link": { "url": "https://x.test" } }
            }])),
            "[docs](https://x.test)"
        );
        // Unknown or unsafe shapes degrade to the run's plain text.
        for link in [
            json!("javascript:alert(1)"),
            json!({ "href": "https://x.test" }),
            json!(42),
            json!(null),
        ] {
            assert_eq!(
                comment_body(json!([{ "text": "docs", "attributes": { "link": link } }])),
                "docs"
            );
        }
    }

    #[test]
    fn unknown_attributes_keep_the_run_as_plain_text() {
        let body = comment_body(json!([
            { "text": "a", "attributes": { "underline": true, "list": { "list": "bullet" } } },
            { "text": "b" },
            { "text": "c", "attributes": { "bold": "yes" } },
            { "type": "image" }
        ]));
        assert_eq!(body, "abc");
    }

    #[test]
    fn comment_falls_back_to_the_flat_text() {
        for node in [
            json!({ "comment_text": " flat ", "comment": [] }),
            json!({ "comment_text": " flat " }),
            json!({ "comment_text": " flat ", "comment": "nope" }),
            json!({ "comment_text": " flat ", "comment": [{ "attributes": {} }] }),
        ] {
            assert_eq!(comment_markdown(&node), "flat");
        }
        assert_eq!(comment_markdown(&json!({})), "");
    }

    #[test]
    fn parsed_comments_carry_the_markdown_body() {
        let node = json!({
            "id": "9",
            "comment_text": "bold",
            "comment": [{ "text": "bold", "attributes": { "bold": true } }],
            "date": "1000"
        });
        assert_eq!(parse_clickup_comment(&node, "t").unwrap().body, "**bold**");
    }

    #[test]
    fn validates_task_ids() {
        assert_eq!(require_task_id(" 86abc12 ").unwrap(), "86abc12");
        assert!(require_task_id("").is_err());
        assert!(require_task_id("../team").is_err());
    }

    // ---- Native storage (no network) ----

    #[test]
    fn reads_a_config_written_by_the_tauri_app() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("clickup-config.json"),
            r#"{"token":" pk_1 ","teamId":"9001","teamName":"Acme"}"#,
        )
        .unwrap();
        let status = clickup_status(dir.path().to_path_buf()).unwrap();
        assert!(status.connected);
        assert_eq!(
            (status.team_id.as_str(), status.team_name.as_str()),
            ("9001", "Acme")
        );
        assert_eq!(status.username, "");
        assert_eq!(require_config(dir.path()).unwrap().token, "pk_1");
    }

    #[test]
    fn config_round_trips_with_camel_case_keys() {
        let dir = tempfile::tempdir().unwrap();
        let config = ClickUpConfig {
            token: "pk_1".into(),
            team_id: "9001".into(),
            team_name: "Acme".into(),
            username: "ada".into(),
        };
        write_config(dir.path(), &config).unwrap();
        let raw = fs::read_to_string(dir.path().join("clickup-config.json")).unwrap();
        let value: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(
            value,
            json!({"token":"pk_1","teamId":"9001","teamName":"Acme","username":"ada"})
        );
        assert_eq!(read_config(dir.path()).unwrap().unwrap().username, "ada");
    }

    #[cfg(unix)]
    #[test]
    fn config_file_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let config = ClickUpConfig {
            token: "pk_1".into(),
            team_id: "9001".into(),
            team_name: "Acme".into(),
            username: String::new(),
        };
        write_config(dir.path(), &config).unwrap();
        let mode = fs::metadata(dir.path().join("clickup-config.json"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn empty_token_deletes_the_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("clickup-config.json");
        fs::write(
            &path,
            r#"{"token":"pk_1","teamId":"9001","teamName":"Acme"}"#,
        )
        .unwrap();
        let status = clickup_set_config(dir.path().to_path_buf(), "  ".into(), None).unwrap();
        assert!(!status.connected);
        assert!(!path.exists());
        // Deleting again is not an error.
        clickup_set_config(dir.path().to_path_buf(), String::new(), None).unwrap();
    }

    #[test]
    fn disconnected_reads_and_calls_do_not_touch_the_network() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().to_path_buf();
        assert!(!clickup_status(app.clone()).unwrap().connected);
        assert!(clickup_list_issues(app.clone(), None, false, None)
            .unwrap()
            .is_empty());
        assert_eq!(
            clickup_list_spaces(app.clone()).unwrap_err(),
            "Connect ClickUp in Settings"
        );
        assert_eq!(
            clickup_issue_comment(app, "t".into(), "hi".into()).unwrap_err(),
            "Connect ClickUp in Settings"
        );
    }

    #[test]
    fn rejects_incomplete_or_corrupt_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("clickup-config.json");
        fs::write(&path, r#"{"token":"","teamId":"1","teamName":"A"}"#).unwrap();
        assert!(read_config(dir.path()).unwrap().is_none());
        fs::write(&path, "nope").unwrap();
        assert_eq!(
            read_config(dir.path()).err().as_deref(),
            Some("ClickUp settings are invalid")
        );
    }
}

#[cfg(test)]
mod inbox_tests {
    use super::*;
    use std::cell::RefCell;

    fn space(id: &str, name: &str) -> ClickUpSpace {
        ClickUpSpace {
            id: id.into(),
            name: name.into(),
        }
    }
    fn spaces() -> Vec<ClickUpSpace> {
        vec![space("1001", "Engineering"), space("1002", "Operations")]
    }
    fn ids(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| v.to_string()).collect()
    }
    fn issue(id: &str, team_id: &str, assignees: &[&str]) -> ClickUpIssue {
        ClickUpIssue {
            provider: "clickup".into(),
            kind: "clickup".into(),
            id: id.into(),
            identifier: "ENG-42".into(),
            number: 42,
            title: "Fix auth".into(),
            url: format!("https://app.clickup.com/t/{id}"),
            state: "in progress".into(),
            state_type: "indeterminate".into(),
            updated_at: "2026-09-23T10:00:00Z".into(),
            labels: vec![],
            assignees: assignees
                .iter()
                .map(|login| ClickUpAssignee {
                    login: (*login).into(),
                    avatar_url: String::new(),
                })
                .collect(),
            draft: false,
            repo: "Engineering".into(),
            team_id: team_id.into(),
            team_name: "Engineering".into(),
            list_name: "Sprint 12".into(),
        }
    }
    type Call = (Option<Vec<String>>, bool, Option<usize>);
    /// Runs `inbox_issues` with a fake backend and returns the result plus the
    /// arguments the backend saw (None when it was never called).
    fn run(
        hidden: &[&str],
        username: &str,
        assigned_to_me: bool,
        include_closed: bool,
        returned: Vec<ClickUpIssue>,
    ) -> (Vec<ClickUpIssue>, Option<Call>) {
        let call = RefCell::new(None);
        let out = inbox_issues(
            &spaces(),
            &ids(hidden),
            username,
            assigned_to_me,
            include_closed,
            |space_ids, closed, limit| {
                *call.borrow_mut() = Some((space_ids, closed, limit));
                Ok(returned)
            },
        )
        .unwrap();
        (out, call.into_inner())
    }

    #[test]
    fn reconciles_hidden_ids_against_the_live_space_list() {
        assert_eq!(
            reconcile_hidden_spaces(&spaces(), &ids(&["1002", "gone"])),
            ids(&["1002"])
        );
        assert!(reconcile_hidden_spaces(&[], &ids(&["1001"])).is_empty());
    }

    #[test]
    fn builds_the_space_filter_only_from_live_ids() {
        assert_eq!(space_ids_for_fetch(&spaces(), &[]), None);
        assert_eq!(
            space_ids_for_fetch(&spaces(), &ids(&["1002"])),
            Some(ids(&["1001"]))
        );
        assert_eq!(
            space_ids_for_fetch(&spaces(), &ids(&["1001", "1002"])),
            Some(vec![])
        );
        // A stale id alone hides nothing, so the request is not narrowed.
        assert_eq!(space_ids_for_fetch(&spaces(), &ids(&["deleted"])), None);
        assert_eq!(
            space_ids_for_fetch(&spaces(), &ids(&["1002", "deleted"])),
            Some(ids(&["1001"]))
        );
    }

    #[test]
    fn assignee_filter_matches_the_cached_username_case_insensitively() {
        let issues = vec![
            issue("86abc12", "1001", &["ada"]),
            issue("2", "1001", &["bob"]),
            issue("3", "1001", &[]),
        ];
        let mine = assigned_to(issues, " ADA ");
        assert_eq!(
            mine.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
            vec!["86abc12"]
        );
    }

    #[test]
    fn assignee_filter_fails_closed_when_the_username_is_unknown() {
        assert!(assigned_to(vec![issue("1", "1001", &["ada"])], "").is_empty());
        assert!(assigned_to(vec![issue("1", "1001", &["ada"])], "   ").is_empty());
    }

    #[test]
    fn loads_workspace_wide_tasks_with_the_backend_default_window() {
        let (out, call) = run(
            &[],
            "ada",
            false,
            false,
            vec![issue("86abc12", "1001", &[])],
        );
        assert_eq!(out.len(), 1);
        assert_eq!(call, Some((None, false, None)));
    }

    #[test]
    fn includes_closed_tasks_with_the_widest_window_when_every_state_is_requested() {
        let (_, call) = run(&[], "ada", false, true, vec![]);
        assert_eq!(call, Some((None, true, Some(100))));
    }

    #[test]
    fn sends_only_live_space_ids_and_drops_hidden_ids_for_deleted_spaces() {
        let (_, call) = run(&["1002"], "ada", false, false, vec![]);
        assert_eq!(call.unwrap().0, Some(ids(&["1001"])));
        // 1002 still exists, "gone" does not. The stale id must not be forwarded.
        let (_, call) = run(&["1002", "gone"], "ada", false, false, vec![]);
        assert_eq!(call.unwrap().0, Some(ids(&["1001"])));
        // Only a stale id is stored: nothing real is hidden, so no narrowing.
        let (_, call) = run(&["gone"], "ada", false, false, vec![]);
        assert_eq!(call.unwrap().0, None);
    }

    #[test]
    fn skips_the_fetch_and_returns_nothing_when_every_space_is_hidden() {
        let (out, call) = run(
            &["1001", "1002"],
            "ada",
            false,
            false,
            vec![issue("1", "1001", &[])],
        );
        assert!(out.is_empty());
        assert_eq!(call, None);
    }

    #[test]
    fn filters_hidden_spaces_out_of_the_result_as_well() {
        let (out, _) = run(
            &["1002"],
            "",
            false,
            false,
            vec![issue("86abc12", "1001", &[]), issue("2", "1002", &[])],
        );
        assert_eq!(
            out.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
            vec!["86abc12"]
        );
    }

    #[test]
    fn assigned_to_me_narrows_to_the_connected_users_tasks() {
        let (out, _) = run(
            &[],
            "ada",
            true,
            false,
            vec![
                issue("86abc12", "1001", &["ada"]),
                issue("2", "1001", &["bob"]),
            ],
        );
        assert_eq!(
            out.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
            vec!["86abc12"]
        );
    }

    #[test]
    fn assigned_to_me_shows_nothing_when_the_cached_username_is_empty() {
        let tasks = vec![issue("1", "1001", &["ada"])];
        assert!(run(&[], "", true, false, tasks.clone()).0.is_empty());
        assert_eq!(run(&[], "", false, false, tasks).0.len(), 1);
    }

    #[test]
    fn assigned_to_me_asks_the_backend_for_its_maximum_window() {
        let (_, call) = run(&[], "ada", true, false, vec![]);
        assert_eq!(call, Some((None, false, Some(100))));
        // Without the filter the backend default is kept.
        let (_, call) = run(&[], "ada", false, false, vec![]);
        assert_eq!(call.unwrap().2, None);
    }

    #[test]
    fn backend_errors_propagate() {
        let err = inbox_issues(&spaces(), &[], "ada", false, false, |_, _, _| {
            Err("ClickUp rate limited, retry shortly".to_string())
        })
        .unwrap_err();
        assert_eq!(err, RATE_LIMITED);
    }
}
