//! The cloud workspaces overview with gh and sbx faked: rows for both backends, orphans, cost
//! numbers, the stop and delete flows (a sandbox that hibernates included) and degradation.
use super::backend::docker::{SandboxShell, ShellOutput};
use super::docker_start::StoreSecrets;
use super::workspaces::*;
use super::{CloudRef, CloudSessions, DeleteOutcome, Host};
use crate::codespaces::{GhOutput, GhRunner};
use crate::mono::cloud_api::{CloudApi, DeleteOptions};
use crate::remote::Machine;
use crate::sandboxes::{Prerequisites, SbxOutput, SbxRunner};
use crate::store::Store;
use serde_json::{json, Value};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const AUTH_OK: &str = "github.com\n  \u{2713} Logged in to github.com account me (keyring)\n  - Token scopes: 'codespace', 'repo'\n";
const AUTH_NO_SCOPE: &str =
    "github.com\n  \u{2713} Logged in to github.com account me (keyring)\n  - Token scopes: 'repo'\n";
const NOW: &str = "2026-11-12T10:00:00Z";
const PROJECT: &str = "/work/widgets";

fn now_ms() -> u64 {
    parse_time_ms(NOW).unwrap()
}

/// A time `hours` before [`NOW`], as gh and sbx write it.
fn hours_ago(hours: f64) -> String {
    let ms = now_ms() - (hours * 3_600_000.0) as u64;
    chrono::DateTime::from_timestamp_millis(ms as i64)
        .unwrap()
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

// ----------------------------------------------------------------------- fake gh

struct Gh {
    auth: Mutex<Result<GhOutput, String>>,
    list: Mutex<String>,
    calls: Mutex<Vec<String>>,
}

impl Gh {
    fn new(list: Value) -> Arc<Self> {
        Arc::new(Self {
            auth: Mutex::new(Ok(out(true, AUTH_OK))),
            list: Mutex::new(list.to_string()),
            calls: Mutex::new(vec![]),
        })
    }
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
    fn count(&self, word: &str) -> usize {
        self.calls().iter().filter(|c| c.contains(word)).count()
    }
}

fn out(success: bool, stdout: &str) -> GhOutput {
    GhOutput {
        success,
        stdout: stdout.into(),
        stderr: String::new(),
    }
}

impl GhRunner for Gh {
    fn run(&self, args: &[String], _: Option<&str>) -> Result<GhOutput, String> {
        let line = args.join(" ");
        self.calls.lock().unwrap().push(line.clone());
        if line.starts_with("auth status") {
            return self.auth.lock().unwrap().clone();
        }
        if line.starts_with("codespace list") {
            return Ok(out(true, &self.list.lock().unwrap()));
        }
        if line.starts_with("codespace stop") || line.starts_with("codespace delete") {
            return Ok(out(true, ""));
        }
        if line.starts_with("codespace view") {
            return Ok(out(true, r#"{"state":"Available"}"#));
        }
        if line.starts_with("api user/codespaces/") {
            return Ok(out(true, "https://octo-abc.github.dev\n"));
        }
        panic!("unexpected gh call: {line}");
    }
}

fn codespace(name: &str, machine: &str, state: &str, created: String) -> Value {
    json!({
        "name": name, "displayName": format!("monoptah-{name}"), "state": state,
        "gitStatus": {"ref": "fix/login"}, "machineName": machine,
        "repository": "octo/widgets", "lastUsedAt": hours_ago(0.5), "createdAt": created,
    })
}

// ---------------------------------------------------------------------- fake sbx

#[derive(Clone)]
struct Bx {
    id: String,
    name: String,
    agent: &'static str,
    state: String,
    cpus: u32,
    memory_mib: u64,
    created: String,
}

fn bx(id: &str, name: &str, state: &str, cpus: u32, memory_mib: u64, created: String) -> Bx {
    Bx {
        id: id.into(),
        name: name.into(),
        agent: "shell",
        state: state.into(),
        cpus,
        memory_mib,
        created,
    }
}

struct Sbx {
    boxes: Mutex<Vec<Bx>>,
    calls: Mutex<Vec<String>>,
    /// `ls` answers this instead of the list.
    ls_reply: Mutex<Option<Result<SbxOutput, String>>>,
    /// A stopping or hibernating box becomes `stopped` after this many `inspect` calls.
    settle_after: Mutex<usize>,
    /// The next this-many `rm` calls are refused as too early.
    rm_refusals: Mutex<usize>,
}

impl Sbx {
    fn new(boxes: Vec<Bx>) -> Arc<Self> {
        Arc::new(Self {
            boxes: Mutex::new(boxes),
            calls: Mutex::new(vec![]),
            ls_reply: Mutex::new(None),
            settle_after: Mutex::new(2),
            rm_refusals: Mutex::new(0),
        })
    }
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
    fn verbs(&self) -> Vec<String> {
        self.calls()
            .iter()
            .map(|c| c.split_whitespace().next().unwrap().to_owned())
            .collect()
    }
    fn state_of(&self, id: &str) -> Option<String> {
        self.boxes
            .lock()
            .unwrap()
            .iter()
            .find(|b| b.id == id)
            .map(|b| b.state.clone())
    }
}

fn json_of(b: &Bx) -> Value {
    json!({"id": b.id, "name": b.name, "agent": b.agent, "state": b.state, "cpus": b.cpus,
        "memory_mib": b.memory_mib, "created_at": b.created})
}

fn sbx_ok(stdout: String) -> Result<SbxOutput, String> {
    Ok(SbxOutput {
        success: true,
        stdout,
        stderr: String::new(),
    })
}

fn sbx_fail(stderr: &str) -> Result<SbxOutput, String> {
    Ok(SbxOutput {
        success: false,
        stdout: String::new(),
        stderr: stderr.into(),
    })
}

impl SbxRunner for Sbx {
    fn run(&self, args: &[String], _: Option<&str>) -> Result<SbxOutput, String> {
        // The base flags every call carries are not part of what a test cares about.
        let args: Vec<String> = args
            .iter()
            .filter(|a| !matches!(a.as_str(), "--cloud" | "--no-input"))
            .cloned()
            .collect();
        self.calls.lock().unwrap().push(args.join(" "));
        let verb = args.first().map(String::as_str).unwrap_or_default();
        let mut boxes = self.boxes.lock().unwrap();
        match verb {
            "ls" => {
                if let Some(reply) = self.ls_reply.lock().unwrap().clone() {
                    return reply;
                }
                sbx_ok(json!(boxes.iter().map(json_of).collect::<Vec<_>>()).to_string())
            }
            "inspect" => {
                let Some(b) = boxes.iter_mut().find(|b| b.id == args[1]) else {
                    return sbx_fail(&format!("error: sandbox '{}' not found", args[1]));
                };
                if matches!(b.state.as_str(), "stopping" | "hibernating") {
                    let mut left = self.settle_after.lock().unwrap();
                    *left = left.saturating_sub(1);
                    if *left == 0 {
                        b.state = "stopped".into();
                    }
                }
                sbx_ok(json_of(b).to_string())
            }
            "stop" => {
                if let Some(b) = boxes.iter_mut().find(|b| b.id == args[1]) {
                    b.state = "stopping".into();
                }
                sbx_ok(String::new())
            }
            "rm" => {
                let id = args.last().unwrap();
                let mut refusals = self.rm_refusals.lock().unwrap();
                if *refusals > 0 {
                    *refusals -= 1;
                    return sbx_fail("error: sandbox is hibernating; wait until it is stopped");
                }
                boxes.retain(|b| &b.id != id);
                sbx_ok(String::new())
            }
            other => panic!("unexpected sbx call: {other} {args:?}"),
        }
    }
}

struct NoSsh;
impl SandboxShell for NoSsh {
    fn run(
        &self,
        _: &str,
        remote: &str,
        _: &str,
        _: &mut dyn FnMut(&str),
        _: &dyn Fn() -> bool,
        _: Duration,
    ) -> Result<ShellOutput, String> {
        panic!("ssh must not run: {remote}");
    }
}

#[derive(Default)]
struct FakeHost {
    log: Mutex<Vec<String>>,
}
impl Host for FakeHost {
    fn machines(&self) -> Result<Vec<Machine>, String> {
        Ok(vec![])
    }
    fn connect(
        &self,
        _: &str,
        _: &str,
        _: &AtomicBool,
        _: &mut dyn FnMut(&str),
    ) -> Result<Machine, String> {
        panic!("a workspaces call must not connect")
    }
    fn ensure(&self, id: &str, _: &AtomicBool, _: &mut dyn FnMut(&str)) -> Result<(), String> {
        panic!("a workspaces call must not wake {id}")
    }
    fn request(&self, id: &str, method: &str, _: Value) -> Result<Value, String> {
        panic!("unexpected host request {id} {method}")
    }
    fn disconnect(&self, id: &str) -> Result<(), String> {
        self.log.lock().unwrap().push(format!("disconnect {id}"));
        Ok(())
    }
}

// --------------------------------------------------------------------- fixtures

struct Fx {
    svc: CloudSessions,
    gh: Arc<Gh>,
    sbx: Arc<Sbx>,
    store: Arc<Mutex<Store>>,
    sleeps: Arc<AtomicUsize>,
}

fn fx(gh: Arc<Gh>, sbx: Arc<Sbx>) -> Fx {
    let store = Arc::new(Mutex::new(Store::open(Path::new(":memory:")).unwrap()));
    let sleeps = Arc::new(AtomicUsize::new(0));
    let counter = sleeps.clone();
    let svc = CloudSessions::new(gh.clone(), Arc::new(FakeHost::default()), store.clone())
        .with_timing(
            Duration::from_secs(5),
            move |_| {
                counter.fetch_add(1, Ordering::SeqCst);
            },
            now_ms,
        )
        .with_origin(|_| Ok("git@github.com:octo/widgets.git".into()))
        .with_sandboxes(
            sbx.clone(),
            Arc::new(NoSsh),
            Arc::new(StoreSecrets::new(store.clone())),
        );
    Fx {
        svc,
        gh,
        sbx,
        store,
        sleeps,
    }
}

/// A live session of `local` on `cloud`; returns its id.
fn add_session(store: &Mutex<Store>, local: &str, cloud: &CloudRef) -> i64 {
    let store = store.lock().unwrap();
    let project = store
        .remember_cloud_ref_project(
            &format!("env-{}", cloud.name),
            &json!({"id":"p","cwd":"/workspaces/widgets"}),
            local,
            cloud,
        )
        .unwrap();
    store.create("claude", &project.key).unwrap().id
}

fn row<'a>(view: &'a WorkspaceOverview, name: &str) -> &'a WorkspaceRow {
    view.rows
        .iter()
        .find(|r| r.cloud.name == name)
        .unwrap_or_else(|| panic!("no row {name}: {:?}", view.rows))
}

/// One codespace in use, one orphan codespace, one sandbox in use and one orphan sandbox.
fn full() -> Fx {
    let gh = Gh::new(json!([
        codespace("octo-used", "basicLinux32gb", "Available", hours_ago(3.0)),
        codespace(
            "octo-lost",
            "standardLinux32gb",
            "Shutdown",
            hours_ago(100.0)
        ),
    ]));
    let sbx = Sbx::new(vec![
        bx(
            "sbx_001a",
            "monoptah-fix-login-a1b2c3",
            "running",
            4,
            8192,
            hours_ago(2.0),
        ),
        bx(
            "sbx_001b",
            "monoptah-main-d4e5f6",
            "stopped",
            2,
            4096,
            hours_ago(30.0),
        ),
        bx(
            "sbx_001c",
            "someone-elses",
            "running",
            2,
            4096,
            hours_ago(1.0),
        ),
    ]);
    let fx = fx(gh, sbx);
    add_session(&fx.store, PROJECT, &CloudRef::codespace("octo-used"));
    add_session(
        &fx.store,
        PROJECT,
        &CloudRef::sandbox("monoptah-fix-login-a1b2c3"),
    );
    fx
}

// ------------------------------------------------------------------------- tests

#[test]
fn lists_a_row_per_workspace_of_both_backends() {
    let fx = full();
    let view = fx.svc.workspaces_overview(PROJECT, false);
    assert_eq!(view.repo.as_deref(), Some("octo/widgets"));
    assert!(view.codespaces_error.is_none() && view.sandbox_problem.is_none());
    // The sandbox that is not Monoptah's is not listed.
    assert_eq!(view.rows.len(), 4, "{:?}", view.rows);

    let used = row(&view, "octo-used");
    assert_eq!(used.cloud, CloudRef::codespace("octo-used"));
    assert_eq!(used.display_name, "monoptah-octo-used");
    assert_eq!(used.branch.as_deref(), Some("fix/login"));
    assert_eq!(used.size, "basicLinux32gb \u{b7} 2 cores");
    assert_eq!(used.state, WorkspaceState::Running);
    assert_eq!(used.age_secs(now_ms()), Some(3 * 3600));
    assert!(used.has_web && used.id.is_none());
    assert_eq!(used.sessions.len(), 1);

    let sandbox = row(&view, "monoptah-fix-login-a1b2c3");
    assert_eq!(
        sandbox.cloud,
        CloudRef::sandbox("monoptah-fix-login-a1b2c3")
    );
    assert_eq!(sandbox.id.as_deref(), Some("sbx_001a"));
    assert_eq!(sandbox.size, "Medium \u{b7} 4 vCPU / 8g");
    assert_eq!(sandbox.state, WorkspaceState::Running);
    assert!(!sandbox.has_web);
    assert_eq!(sandbox.sessions.len(), 1);
}

#[test]
fn a_workspace_no_session_uses_is_an_orphan_and_rows_list_the_costly_first() {
    let fx = full();
    let view = fx.svc.workspaces_overview(PROJECT, false);
    let orphans: Vec<_> = view
        .rows
        .iter()
        .filter(|r| r.orphan())
        .map(|r| r.cloud.name.as_str())
        .collect();
    assert_eq!(orphans, ["octo-lost", "monoptah-main-d4e5f6"]);
    assert!(!row(&view, "octo-used").orphan());
    // Running first (used or not), then the stopped ones, orphans before used ones.
    let order: Vec<_> = view.rows.iter().map(|r| r.cloud.name.as_str()).collect();
    assert_eq!(
        order,
        [
            "octo-used",
            "monoptah-fix-login-a1b2c3",
            "octo-lost",
            "monoptah-main-d4e5f6"
        ]
    );
}

#[test]
fn a_sandbox_used_only_by_another_project_is_left_out() {
    let fx = full();
    add_session(
        &fx.store,
        "/work/other",
        &CloudRef::sandbox("monoptah-main-d4e5f6"),
    );
    let view = fx.svc.workspaces_overview(PROJECT, false);
    assert!(view
        .rows
        .iter()
        .all(|r| r.cloud.name != "monoptah-main-d4e5f6"));
}

#[test]
fn a_session_on_a_workspace_names_itself_and_takes_it_off_the_orphans() {
    let fx = full();
    let id = add_session(&fx.store, PROJECT, &CloudRef::codespace("octo-lost"));
    let view = fx.svc.workspaces_overview(PROJECT, false);
    let lost = row(&view, "octo-lost");
    assert_eq!(lost.sessions.len(), 1);
    assert_eq!(lost.sessions[0].id, id);
    assert!(!lost.orphan());
}

#[test]
fn the_cost_numbers_follow_the_published_rates() {
    // The tier table is Docker's usage page.
    let tiers: Vec<_> = DOCKER_TIERS
        .iter()
        .map(|t| (t.name, t.cpus, t.memory_gib, t.rate))
        .collect();
    assert_eq!(
        tiers,
        [
            ("Micro", 1, 2, 0.07),
            ("Small", 2, 4, 0.14),
            ("Medium", 4, 8, 0.28),
            ("Large", 8, 16, 0.56),
            ("XL", 16, 32, 1.12),
        ]
    );
    assert_eq!(docker_tier(4, 8192).unwrap().name, "Medium");
    assert_eq!(docker_tier(4, 0).unwrap().name, "Medium");
    assert!(docker_tier(4, 4096).is_none(), "not a tier shape");
    assert!(docker_tier(3, 6144).is_none());
    assert_eq!(codespace_cores("basicLinux32gb"), Some(2));
    assert_eq!(codespace_cores("premiumLinux"), Some(8));
    assert_eq!(codespace_cores("mysteryLinux"), None);

    let fx = full();
    let view = fx.svc.workspaces_overview(PROJECT, false);
    // Medium sandbox, running for 2 h: 2 x $0.28.
    let sandbox = row(&view, "monoptah-fix-login-a1b2c3").cost;
    assert!(sandbox.accruing);
    assert_eq!(sandbox.rate_per_hour, Some(0.28));
    assert_eq!(sandbox.runtime_secs, Some(2 * 3600));
    assert!((sandbox.total.unwrap() - 0.56).abs() < 1e-9);
    // 2-core codespace, running for 3 h: 3 h x 2 cores x $0.09.
    let codespace = row(&view, "octo-used").cost;
    assert!((codespace.rate_per_hour.unwrap() - 0.18).abs() < 1e-9);
    assert!((codespace.total.unwrap() - 0.54).abs() < 1e-9);
    // A stopped workspace is not drawing compute: a rate, no running time, no total.
    let stopped = row(&view, "octo-lost").cost;
    assert!(!stopped.accruing);
    assert!((stopped.rate_per_hour.unwrap() - 0.36).abs() < 1e-9);
    assert_eq!((stopped.runtime_secs, stopped.total), (None, None));
    let stopped_sandbox = row(&view, "monoptah-main-d4e5f6").cost;
    assert_eq!(stopped_sandbox.rate_per_hour, Some(0.14));
    assert_eq!(stopped_sandbox.total, None);
}

#[test]
fn an_unknown_size_has_a_state_but_no_price() {
    let gh = Gh::new(json!([codespace(
        "octo-odd",
        "mysteryLinux",
        "Available",
        hours_ago(1.0)
    )]));
    let sbx = Sbx::new(vec![bx(
        "sbx_001z",
        "monoptah-odd-000000",
        "running",
        3,
        6144,
        hours_ago(1.0),
    )]);
    let view = fx(gh, sbx).svc.workspaces_overview(PROJECT, false);
    for r in &view.rows {
        assert!(r.cost.accruing, "{r:?}");
        assert_eq!((r.cost.rate_per_hour, r.cost.total), (None, None), "{r:?}");
        assert!(r.cost.runtime_secs.is_some());
    }
    assert_eq!(row(&view, "octo-odd").size, "mysteryLinux");
    assert_eq!(row(&view, "monoptah-odd-000000").size, "3 vCPU / 6g");
}

#[test]
fn states_read_as_the_overview_words() {
    use WorkspaceState::*;
    for (raw, want) in [
        ("Available", Running),
        ("Shutdown", Stopped),
        ("ShuttingDown", Stopping),
        ("Provisioning", Starting),
        ("Failed", Failed),
        ("Unavailable", Failed),
        ("Surprise", Unknown),
    ] {
        assert_eq!(WorkspaceState::of_codespace(raw), want, "{raw}");
    }
    for (raw, want) in [
        ("running", Running),
        ("creating", Starting),
        ("stopping", Stopping),
        ("hibernating", Hibernating),
        ("stopped", Stopped),
        ("failed", Failed),
        ("Surprise", Unknown),
    ] {
        assert_eq!(WorkspaceState::of_sandbox(raw), want, "{raw}");
    }
    assert!(Hibernating.settling() && Hibernating.accruing() && !Hibernating.can_stop());
    assert!(Running.can_stop() && !Stopped.accruing() && !Failed.accruing());
}

#[test]
fn loading_only_reads_and_the_result_is_cached() {
    let fx = full();
    fx.svc.workspaces_overview(PROJECT, false);
    // Nothing that could start a workspace or connect to it.
    assert_eq!(fx.sbx.verbs(), ["ls"], "{:?}", fx.sbx.calls());
    assert_eq!(
        fx.gh.calls(),
        [
            "auth status --active",
            "codespace list --repo octo/widgets --json name,displayName,state,gitStatus,machineName,repository,lastUsedAt,createdAt"
        ]
    );
    // Reopening within the TTL asks nobody; an explicit refresh does.
    fx.svc.workspaces_overview(PROJECT, false);
    assert_eq!((fx.sbx.calls().len(), fx.gh.calls().len()), (1, 2));
    assert!(fx.svc.cached_workspaces(PROJECT).is_some());
    fx.svc.workspaces_overview(PROJECT, true);
    assert_eq!((fx.sbx.calls().len(), fx.gh.calls().len()), (2, 4));
    fx.svc.forget_workspace_views();
    assert!(fx.svc.cached_workspaces(PROJECT).is_none());
}

#[test]
fn without_sbx_the_codespaces_still_list_and_the_fix_is_named() {
    let fx = full();
    *fx.sbx.ls_reply.lock().unwrap() =
        Some(Err("Docker Sandboxes CLI (sbx) is not installed".into()));
    let view = fx.svc.workspaces_overview(PROJECT, false);
    assert_eq!(view.sandbox_problem, Some(Prerequisites::SbxMissing));
    assert_eq!(view.rows.len(), 2);
    assert!(view.rows.iter().all(|r| r.cloud.name.starts_with("octo-")));

    *fx.sbx.ls_reply.lock().unwrap() = Some(sbx_fail("not logged in: run `sbx login`"));
    let view = fx.svc.workspaces_overview(PROJECT, true);
    let problem = view.sandbox_problem.unwrap();
    assert!(
        matches!(problem, Prerequisites::NotLoggedIn(_)),
        "{problem:?}"
    );
    assert_eq!(problem.fix_command(), Some("sbx login"));

    *fx.sbx.ls_reply.lock().unwrap() = Some(sbx_fail("connection refused"));
    let view = fx.svc.workspaces_overview(PROJECT, true);
    assert!(matches!(
        view.sandbox_problem,
        Some(Prerequisites::Unreachable(_))
    ));
    assert_eq!(view.rows.len(), 2);
}

#[test]
fn without_gh_scope_or_login_no_codespace_is_asked_for_and_sandboxes_still_list() {
    let fx = full();
    *fx.gh.auth.lock().unwrap() = Ok(out(true, AUTH_NO_SCOPE));
    let view = fx.svc.workspaces_overview(PROJECT, false);
    let status = view.gh.clone().unwrap().unwrap();
    assert!(status.logged_in && !status.has_codespace_scope);
    assert_eq!(
        fx.gh.count("codespace list"),
        0,
        "never list without the scope"
    );
    assert_eq!(view.rows.len(), 2);
    assert!(view
        .rows
        .iter()
        .all(|r| r.cloud.name.starts_with("monoptah-")));

    *fx.gh.auth.lock().unwrap() = Err("GitHub CLI (gh) is not installed".into());
    let view = fx.svc.workspaces_overview(PROJECT, true);
    assert!(view.gh.unwrap().unwrap_err().contains("not installed"));
    assert_eq!(view.rows.len(), 2);

    *fx.gh.auth.lock().unwrap() = Ok(out(false, "You are not logged into any GitHub hosts."));
    let view = fx.svc.workspaces_overview(PROJECT, true);
    assert!(!view.gh.unwrap().unwrap().logged_in);
}

#[test]
fn a_project_not_on_github_asks_gh_nothing() {
    let fx = full();
    let svc = fx.svc.with_origin(|_| Err("no origin".into()));
    let view = svc.workspaces_overview(PROJECT, false);
    assert!(view.repo.is_none() && view.gh.is_none());
    assert_eq!(fx.gh.calls().len(), 0);
    assert_eq!(view.rows.len(), 2, "the sandboxes still list");
}

#[test]
fn stopping_a_codespace_goes_through_the_existing_stop_and_forgets_the_view() {
    let fx = full();
    let api = api_over(&fx);
    api.cloud().workspaces_overview(PROJECT, false);
    assert!(api.cloud().cached_workspaces(PROJECT).is_some());
    api.stop_instance(&CloudRef::codespace("octo-used"), &|_: &CloudRef| false)
        .unwrap();
    assert_eq!(fx.gh.count("codespace stop -c octo-used"), 1);
    assert!(
        api.cloud().cached_workspaces(PROJECT).is_none(),
        "the view is stale now"
    );
}

/// A `CloudApi` over the same fakes (its own service, like the app's shared one).
fn api_over(fx: &Fx) -> CloudApi {
    CloudApi::with_sandboxes(
        fx.gh.clone(),
        Arc::new(FakeHost::default()),
        fx.store.clone(),
        fx.sbx.clone(),
        Arc::new(NoSsh),
        Arc::new(StoreSecrets::new(fx.store.clone())),
    )
}

#[test]
fn deleting_a_running_sandbox_stops_it_waits_for_stopped_then_removes_it() {
    let fx = full();
    let cloud = CloudRef::sandbox("monoptah-fix-login-a1b2c3");
    let ids = fx.svc.session_ids_on(&cloud);
    assert_eq!(ids.len(), 1);
    assert_eq!(
        fx.svc.delete_settled(&cloud).unwrap(),
        DeleteOutcome::Deleted
    );
    let verbs = fx.sbx.verbs();
    let stop = verbs
        .iter()
        .position(|v| v == "stop")
        .expect("stopped first");
    let rm = verbs.iter().position(|v| v == "rm").expect("removed");
    assert!(stop < rm, "{verbs:?}");
    assert!(fx.sbx.state_of("sbx_001a").is_none());
    // The Monoptah side is cleaned like any other delete: sessions forgotten, cloud gone.
    assert!(fx.svc.session_ids_on(&cloud).is_empty());
    assert!(fx.svc.cloud_gone(&cloud));
    assert!(
        fx.sleeps.load(Ordering::SeqCst) >= 1,
        "it waited for stopped"
    );
}

#[test]
fn deleting_a_hibernating_sandbox_waits_without_stopping_it_again() {
    let fx = full();
    fx.sbx.boxes.lock().unwrap()[0].state = "hibernating".into();
    let cloud = CloudRef::sandbox("monoptah-fix-login-a1b2c3");
    fx.svc.delete_settled(&cloud).unwrap();
    assert!(
        !fx.sbx.verbs().contains(&"stop".to_owned()),
        "{:?}",
        fx.sbx.calls()
    );
    assert!(fx.sbx.state_of("sbx_001a").is_none());
    assert!(fx.sleeps.load(Ordering::SeqCst) >= 1);
}

#[test]
fn a_delete_the_sandbox_refuses_as_too_early_is_retried() {
    let fx = full();
    // The sandbox already reads `stopped`, but the first two removals are refused.
    fx.sbx.boxes.lock().unwrap()[1].state = "stopped".into();
    *fx.sbx.rm_refusals.lock().unwrap() = 2;
    let cloud = CloudRef::sandbox("monoptah-main-d4e5f6");
    fx.svc.delete_settled(&cloud).unwrap();
    let rms = fx.sbx.verbs().iter().filter(|v| *v == "rm").count();
    assert_eq!(rms, 3);
    assert!(fx.sbx.state_of("sbx_001b").is_none());
}

#[test]
fn a_sandbox_that_never_settles_is_not_deleted() {
    let fx = full();
    fx.sbx.boxes.lock().unwrap()[0].state = "hibernating".into();
    *fx.sbx.settle_after.lock().unwrap() = usize::MAX;
    let cloud = CloudRef::sandbox("monoptah-fix-login-a1b2c3");
    let error = fx.svc.delete_settled(&cloud).unwrap_err();
    assert!(
        error.contains("hibernating") && error.contains("not deleted"),
        "{error}"
    );
    assert!(!fx.sbx.verbs().contains(&"rm".to_owned()));
    assert_eq!(fx.sbx.state_of("sbx_001a").as_deref(), Some("hibernating"));
    // The sessions stay: nothing was deleted.
    assert_eq!(fx.svc.session_ids_on(&cloud).len(), 1);
}

#[test]
fn deleting_a_codespace_needs_no_wait_and_forgets_its_sessions() {
    let fx = full();
    let cloud = CloudRef::codespace("octo-used");
    fx.svc.delete_settled(&cloud).unwrap();
    assert_eq!(fx.gh.count("codespace delete -c octo-used --force"), 1);
    assert_eq!(fx.sleeps.load(Ordering::SeqCst), 0);
    assert!(fx.svc.session_ids_on(&cloud).is_empty());
    assert!(fx.sbx.calls().is_empty(), "no sbx call for a codespace");
}

#[test]
fn the_delete_refuses_busy_sessions_and_unsaved_files_until_the_user_accepted() {
    let fx = full();
    let api = api_over(&fx);
    let cloud = CloudRef::codespace("octo-used");
    let session = fx.svc.session_ids_on(&cloud)[0];
    let none = |_: &CloudRef| false;

    // A session the live view says is busy blocks the delete.
    struct Busy(i64);
    impl crate::mono::cloud_api::AppBusy for Busy {
        fn working_on(&self, _: &CloudRef) -> bool {
            false
        }
        fn session_busy(&self, id: i64) -> bool {
            id == self.0
        }
    }
    let error = api
        .delete_instance(&cloud, &Busy(session), DeleteOptions::default())
        .unwrap_err();
    assert!(error.contains("sessions are working on it"), "{error}");
    assert_eq!(fx.gh.count("codespace delete"), 0);

    // Files that exist only in the workspace need the user's say-so.
    api.cloud()
        .record_unsaved_cloud(&cloud, &["3 files were not copied.".into()])
        .unwrap();
    let error = api
        .delete_instance(&cloud, &none, DeleteOptions::default())
        .unwrap_err();
    assert!(error.contains("would lose files"), "{error}");
    assert_eq!(fx.gh.count("codespace delete"), 0);
    let done = api
        .delete_instance(
            &cloud,
            &none,
            DeleteOptions {
                accept_unsaved: true,
                settle: true,
            },
        )
        .unwrap();
    assert_eq!(done["deleted"], true);
    // The window drops these sessions, like after a `cloud.delete`.
    assert_eq!(done["removedSessionIds"], json!([session]));
    assert_eq!(fx.gh.count("codespace delete -c octo-used --force"), 1);
}

#[test]
fn the_web_address_is_read_without_waking_the_workspace() {
    let fx = full();
    let url = fx
        .svc
        .workspace_web_url(&CloudRef::codespace("octo-abc"))
        .unwrap();
    assert_eq!(url, "https://octo-abc.github.dev");
    assert!(fx
        .svc
        .workspace_web_url(&CloudRef::sandbox("monoptah-main-d4e5f6"))
        .is_err());
}

#[test]
fn time_stamps_parse_from_rfc3339_and_bare_numbers() {
    assert_eq!(
        parse_time_ms("2026-11-12T10:00:00Z"),
        Some(1_794_477_600_000)
    );
    assert_eq!(
        parse_time_ms("2026-11-12T17:00:00+07:00"),
        Some(1_794_477_600_000)
    );
    assert_eq!(parse_time_ms("1794477600"), Some(1_794_477_600_000));
    assert_eq!(parse_time_ms("1794477600000"), Some(1_794_477_600_000));
    assert_eq!(parse_time_ms(""), None);
    assert_eq!(parse_time_ms("yesterday"), None);
}
