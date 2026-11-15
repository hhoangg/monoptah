//! The cloud workspaces overview with gh and sbx faked: rows for both backends, orphans, cost
//! numbers, the stop and delete flows (a sandbox that hibernates included) and degradation.
use super::backend::docker::{SandboxShell, ShellOutput};
use super::docker_start::StoreSecrets;
use super::stopping::{StopReport, STOP_EXPECTED};
use super::workspaces::*;
use super::{CloudRef, CloudSessions, DeleteOutcome, Host};
use crate::codespaces::{GhOutput, GhRunner};
use crate::mono::cloud_api::{CloudApi, DeleteOptions};
use crate::remote::Machine;
use crate::sandboxes::{Prerequisites, SbxOutput, SbxRunner};
use crate::store::Store;
use crate::store::{CloudLocation, SessionLocation};
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
    /// What `gh codespace view --json gitStatus` answers per codespace; clean when absent.
    git: Mutex<std::collections::HashMap<String, Value>>,
    /// `view --json gitStatus` fails with this.
    git_error: Mutex<Option<String>>,
}

impl Gh {
    fn new(list: Value) -> Arc<Self> {
        Arc::new(Self {
            auth: Mutex::new(Ok(out(true, AUTH_OK))),
            list: Mutex::new(list.to_string()),
            calls: Mutex::new(vec![]),
            git: Mutex::new(Default::default()),
            git_error: Mutex::new(None),
        })
    }
    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
    fn count(&self, word: &str) -> usize {
        self.calls().iter().filter(|c| c.contains(word)).count()
    }
    /// The codespace has these git flags from now on.
    fn set_git(&self, name: &str, uncommitted: bool, unpushed: bool) {
        self.git.lock().unwrap().insert(
            name.into(),
            json!({"gitStatus": {"ahead": i32::from(unpushed), "behind": 0,
                "hasUncommittedChanges": uncommitted, "hasUnpushedChanges": unpushed,
                "ref": "fix/login"}}),
        );
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
        if line.starts_with("codespace view") && line.contains("gitStatus") {
            if let Some(error) = self.git_error.lock().unwrap().clone() {
                return Err(error);
            }
            let name = &args[3];
            let reply = self
                .git
                .lock()
                .unwrap()
                .get(name)
                .cloned()
                .unwrap_or_else(|| {
                    json!({"gitStatus": {"ahead": 0, "behind": 0, "hasUncommittedChanges": false,
                    "hasUnpushedChanges": false, "ref": "fix/login"}})
                });
            return Ok(out(true, &reply.to_string()));
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
    /// The Docker account it lives in (`None`: the machine login).
    account: Option<&'static str>,
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
        account: None,
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
    /// `(verb, account)` of every call, in order.
    accounts: Mutex<Vec<(String, Option<String>)>>,
    /// The accounts whose `ls` answers this instead of their boxes.
    ls_fail: Mutex<std::collections::HashMap<Option<String>, Result<SbxOutput, String>>>,
    /// Once a box has read `stopped` it reads `running` again (something woke it).
    wake_after_stopped: Mutex<bool>,
    /// `stop` fails with this.
    stop_error: Mutex<Option<String>>,
}

impl Sbx {
    fn new(boxes: Vec<Bx>) -> Arc<Self> {
        Arc::new(Self {
            boxes: Mutex::new(boxes),
            calls: Mutex::new(vec![]),
            ls_reply: Mutex::new(None),
            settle_after: Mutex::new(2),
            rm_refusals: Mutex::new(0),
            accounts: Mutex::new(vec![]),
            ls_fail: Mutex::new(Default::default()),
            wake_after_stopped: Mutex::new(false),
            stop_error: Mutex::new(None),
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
    /// The accounts the calls of `verb` ran as.
    fn accounts_of(&self, verb: &str) -> Vec<Option<String>> {
        self.accounts
            .lock()
            .unwrap()
            .iter()
            .filter(|(v, _)| v == verb)
            .map(|(_, account)| account.clone())
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
    fn run(&self, args: &[String], stdin: Option<&str>) -> Result<SbxOutput, String> {
        self.run_as(None, args, stdin)
    }
    fn run_as(
        &self,
        account: Option<&str>,
        args: &[String],
        _: Option<&str>,
    ) -> Result<SbxOutput, String> {
        // The base flags every call carries are not part of what a test cares about.
        let args: Vec<String> = args
            .iter()
            .filter(|a| !matches!(a.as_str(), "--cloud" | "--no-input"))
            .cloned()
            .collect();
        self.calls.lock().unwrap().push(args.join(" "));
        let verb = args.first().map(String::as_str).unwrap_or_default();
        self.accounts
            .lock()
            .unwrap()
            .push((verb.to_owned(), account.map(str::to_owned)));
        let mut boxes = self.boxes.lock().unwrap();
        // A sandbox lives in one account: asked as another, sbx does not know it.
        let mine = |b: &Bx| b.account == account;
        match verb {
            "ls" => {
                if let Some(reply) = self.ls_reply.lock().unwrap().clone() {
                    return reply;
                }
                if let Some(reply) = self
                    .ls_fail
                    .lock()
                    .unwrap()
                    .get(&account.map(str::to_owned))
                    .cloned()
                {
                    return reply;
                }
                sbx_ok(
                    json!(boxes
                        .iter()
                        .filter(|b| mine(b))
                        .map(json_of)
                        .collect::<Vec<_>>())
                    .to_string(),
                )
            }
            "inspect" => {
                let Some(b) = boxes.iter_mut().find(|b| b.id == args[1] && mine(b)) else {
                    return sbx_fail(&format!("error: sandbox '{}' not found", args[1]));
                };
                if matches!(b.state.as_str(), "stopping" | "hibernating") {
                    let mut left = self.settle_after.lock().unwrap();
                    *left = left.saturating_sub(1);
                    if *left == 0 {
                        b.state = "stopped".into();
                    }
                } else if b.state == "stopped" && *self.wake_after_stopped.lock().unwrap() {
                    // The read that ended the wait said stopped; the next one finds it up again.
                    *self.wake_after_stopped.lock().unwrap() = false;
                    b.state = "running".into();
                }
                sbx_ok(json_of(b).to_string())
            }
            "stop" => {
                let Some(b) = boxes.iter_mut().find(|b| b.id == args[1] && mine(b)) else {
                    return sbx_fail(&format!("error: sandbox '{}' not found", args[1]));
                };
                if let Some(error) = self.stop_error.lock().unwrap().clone() {
                    return sbx_fail(&error);
                }
                b.state = "stopping".into();
                sbx_ok(String::new())
            }
            "rm" => {
                let id = args.last().unwrap();
                if !boxes.iter().any(|b| &b.id == id && mine(b)) {
                    return sbx_fail(&format!("error: sandbox '{id}' not found"));
                }
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

/// The reasons sandboxes could not be listed, whatever account they belong to.
fn problems(view: &WorkspaceOverview) -> Vec<Prerequisites> {
    view.sandbox_problems
        .iter()
        .map(|p| p.problem.clone())
        .collect()
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
    // The two orphans were this app's: an archived session still names each, so the store knows
    // them although no live session uses them.
    for cloud in [
        CloudRef::codespace("octo-lost"),
        CloudRef::sandbox("monoptah-main-d4e5f6"),
    ] {
        let id = add_session(&fx.store, PROJECT, &cloud);
        fx.store.lock().unwrap().set_archived(id, true).unwrap();
    }
    fx
}

// ------------------------------------------------------------------------- tests

#[test]
fn lists_a_row_per_workspace_of_both_backends() {
    let fx = full();
    let view = fx.svc.workspaces_overview(PROJECT, false);
    assert_eq!(view.repo.as_deref(), Some("octo/widgets"));
    assert!(view.codespaces_error.is_none() && view.sandbox_problems.is_empty());
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
fn the_footer_asks_about_a_project_only_when_it_uses_cloud_and_never_calls_a_cli() {
    use super::{docker_config, project_cloud};
    let fx = fx(Gh::new(json!([])), Sbx::new(vec![]));
    assert!(!fx.svc.project_uses_cloud(PROJECT), "a plain project");
    assert!(!fx.svc.project_uses_cloud(""));
    assert!(!fx
        .svc
        .project_uses_cloud("remote://env-fake/workspaces/widgets"));

    // Saved settings of either backend.
    {
        let store = fx.store.lock().unwrap();
        project_cloud::save(&store, "/work/codespace", &Default::default()).unwrap();
        let config = docker_config::DockerProjectConfig {
            repo_url: "https://github.com/octo/widgets.git".into(),
            ..Default::default()
        };
        docker_config::save(&store, "/work/sandbox", &config).unwrap();
    }
    assert!(fx.svc.project_uses_cloud("/work/codespace"));
    assert!(fx.svc.project_uses_cloud("/work/sandbox"));
    assert!(!fx.svc.project_uses_cloud(PROJECT));

    // A cloud session of the project, with nothing saved.
    add_session(&fx.store, PROJECT, &CloudRef::codespace("octo-used"));
    assert!(fx.svc.project_uses_cloud(PROJECT));
    assert!(!fx.svc.project_uses_cloud("/work/other"));

    assert!(fx.gh.calls().is_empty(), "{:?}", fx.gh.calls());
    assert!(fx.sbx.calls().is_empty(), "{:?}", fx.sbx.calls());
}

#[test]
fn without_sbx_the_codespaces_still_list_and_the_fix_is_named() {
    let fx = full();
    *fx.sbx.ls_reply.lock().unwrap() =
        Some(Err("Docker Sandboxes CLI (sbx) is not installed".into()));
    let view = fx.svc.workspaces_overview(PROJECT, false);
    assert_eq!(problems(&view), [Prerequisites::SbxMissing]);
    assert_eq!(view.rows.len(), 2);
    assert!(view.rows.iter().all(|r| r.cloud.name.starts_with("octo-")));

    *fx.sbx.ls_reply.lock().unwrap() = Some(sbx_fail("not logged in: run `sbx login`"));
    let view = fx.svc.workspaces_overview(PROJECT, true);
    let problem = problems(&view).remove(0);
    assert!(
        matches!(problem, Prerequisites::NotLoggedIn(_)),
        "{problem:?}"
    );
    assert_eq!(problem.fix_command(), Some("sbx login"));

    *fx.sbx.ls_reply.lock().unwrap() = Some(sbx_fail("connection refused"));
    let view = fx.svc.workspaces_overview(PROJECT, true);
    assert!(matches!(
        problems(&view)[..],
        [Prerequisites::Unreachable(_)]
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
    api.stop_instance(&target_of("octo-used"), &|_: &CloudRef| false)
        .unwrap();
    assert_eq!(fx.gh.count("codespace stop -c octo-used"), 1);
    assert!(
        api.cloud().cached_workspaces(PROJECT).is_none(),
        "the view is stale now"
    );
}

/// A codespace, or a sandbox of the machine login, as a stop or delete names it.
fn target_of(name: &str) -> WorkspaceTarget {
    WorkspaceTarget {
        cloud: if name.starts_with("octo-") {
            CloudRef::codespace(name)
        } else {
            CloudRef::sandbox(name)
        },
        account: None,
    }
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
        fx.svc.delete_settled(&cloud, &|| Ok(())).unwrap(),
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
    fx.svc.delete_settled(&cloud, &|| Ok(())).unwrap();
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
    fx.svc.delete_settled(&cloud, &|| Ok(())).unwrap();
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
    let error = fx.svc.delete_settled(&cloud, &|| Ok(())).unwrap_err();
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
    fx.svc.delete_settled(&cloud, &|| Ok(())).unwrap();
    assert_eq!(fx.gh.count("codespace delete -c octo-used --force"), 1);
    assert_eq!(fx.sleeps.load(Ordering::SeqCst), 0);
    assert!(fx.svc.session_ids_on(&cloud).is_empty());
    assert!(fx.sbx.calls().is_empty(), "no sbx call for a codespace");
}

// ------------------------------------------------------- what is lost, and whose it is

/// The app's live view of what is busy: a session, and from which `working_on` call on it says
/// it is archiving (the calls are counted).
struct AppView {
    session: Option<i64>,
    working_from: usize,
    working_calls: AtomicUsize,
}
impl AppView {
    fn session(id: i64) -> Self {
        Self {
            session: Some(id),
            working_from: usize::MAX,
            working_calls: AtomicUsize::new(0),
        }
    }
    fn working_from(call: usize) -> Self {
        Self {
            session: None,
            working_from: call,
            working_calls: AtomicUsize::new(0),
        }
    }
}
impl crate::mono::cloud_api::AppBusy for AppView {
    fn working_on(&self, _: &CloudRef) -> bool {
        self.working_calls.fetch_add(1, Ordering::SeqCst) >= self.working_from
    }
    fn session_busy(&self, id: i64) -> bool {
        self.session == Some(id)
    }
}

/// What the dialog sends once the user confirmed `lines` and ticked the acknowledgement.
fn confirmed(lines: &[&str]) -> DeleteOptions {
    DeleteOptions {
        confirmed: Some(lines.iter().map(|l| (*l).to_owned()).collect()),
        acknowledged: true,
        settle: true,
    }
}

/// The same confirmation of the lines, without the acknowledgement of a foreign workspace.
fn confirmed_unacknowledged(lines: &[&str]) -> DeleteOptions {
    DeleteOptions {
        acknowledged: false,
        ..confirmed(lines)
    }
}

#[test]
fn the_delete_refuses_busy_sessions_and_unsaved_files_unless_the_user_confirmed_them() {
    let fx = full();
    let api = api_over(&fx);
    let target = target_of("octo-used");
    let cloud = target.cloud.clone();
    let session = fx.svc.session_ids_on(&cloud)[0];
    let none = |_: &CloudRef| false;

    // A session the live view says is busy blocks the delete.
    let error = api
        .delete_instance(&target, &AppView::session(session), confirmed(&[]))
        .unwrap_err();
    assert!(error.contains("sessions are working on it"), "{error}");
    assert_eq!(fx.gh.count("codespace delete"), 0);

    // Files that exist only in the workspace: the plain `cloud.delete` refuses outright ...
    api.cloud()
        .record_unsaved_cloud(&cloud, &["3 files were not copied.".into()])
        .unwrap();
    let error = api
        .delete_instance(&target, &none, DeleteOptions::default())
        .unwrap_err();
    assert!(error.contains("would lose files"), "{error}");
    // ... and the overview refuses what the user was not shown.
    let error = api
        .delete_instance(&target, &none, confirmed(&[]))
        .unwrap_err();
    assert!(
        error.contains("not shown") && error.contains("3 files were not copied."),
        "{error}"
    );
    assert_eq!(fx.gh.count("codespace delete"), 0);
    let done = api
        .delete_instance(&target, &none, confirmed(&["3 files were not copied."]))
        .unwrap();
    assert_eq!(done["deleted"], true);
    // The window drops these sessions, like after a `cloud.delete`.
    assert_eq!(done["removedSessionIds"], json!([session]));
    assert_eq!(fx.gh.count("codespace delete -c octo-used --force"), 1);
}

#[test]
fn the_preview_reads_the_git_state_of_a_codespace_and_asks_for_an_acknowledgement() {
    let fx = full();
    let target = target_of("octo-used");
    // Clean and this app's own: a plain confirmation is enough.
    let clean = fx.svc.delete_preview(&target).unwrap();
    assert!(clean.lines.is_empty() && clean.notes.is_empty() && clean.blockers.is_empty());
    assert!(clean.ours && !clean.contents_unknown && !clean.needs_acknowledgement());
    // Uncommitted and unpushed work is named, and needs the user's say-so.
    fx.gh.set_git("octo-used", true, true);
    let dirty = fx.svc.delete_preview(&target).unwrap();
    assert_eq!(dirty.lines, [LINE_UNCOMMITTED, LINE_UNPUSHED]);
    assert!(dirty.needs_acknowledgement());
    // What an earlier archive recorded is part of it.
    fx.svc
        .record_unsaved_cloud(&target.cloud, &["A file was not copied.".into()])
        .unwrap();
    assert_eq!(
        fx.svc.delete_preview(&target).unwrap().lines,
        ["A file was not copied.", LINE_UNCOMMITTED, LINE_UNPUSHED]
    );
    // GitHub saying nothing is not "nothing to lose".
    fx.gh
        .git
        .lock()
        .unwrap()
        .insert("octo-used".into(), json!({}));
    assert!(fx
        .svc
        .delete_preview(&target)
        .unwrap()
        .lines
        .contains(&LINE_GIT_UNKNOWN.to_owned()));
    // A read that fails is an error: no Delete is offered without knowing.
    *fx.gh.git_error.lock().unwrap() = Some("HTTP 502".into());
    assert!(fx.svc.delete_preview(&target).unwrap_err().contains("502"));
    // Reading never starts anything.
    assert_eq!(
        fx.gh.count("codespace stop") + fx.gh.count("codespace delete"),
        0
    );
}

#[test]
fn a_sandbox_is_not_started_to_read_it_and_always_needs_an_acknowledgement() {
    let fx = full();
    let preview = fx
        .svc
        .delete_preview(&target_of("monoptah-fix-login-a1b2c3"))
        .unwrap();
    assert!(preview.ours && preview.contents_unknown && preview.lines.is_empty());
    assert!(preview.notes.contains(&NOTE_SANDBOX_CONTENTS.to_owned()));
    assert!(preview.needs_acknowledgement());
    assert!(fx.sbx.calls().is_empty(), "{:?}", fx.sbx.calls());
}

#[test]
fn a_delete_refuses_what_appeared_after_the_user_confirmed() {
    let fx = full();
    let api = api_over(&fx);
    let target = target_of("octo-used");
    let none = |_: &CloudRef| false;
    let seen = fx.svc.delete_preview(&target).unwrap().lines;
    assert!(seen.is_empty());
    // Work was left in the codespace after the confirmation opened.
    fx.gh.set_git("octo-used", true, false);
    let error = api
        .delete_instance(&target, &none, confirmed(&[]))
        .unwrap_err();
    assert!(
        error.contains(LINE_UNCOMMITTED) && error.contains("not shown"),
        "{error}"
    );
    assert_eq!(fx.gh.count("codespace delete"), 0);
    // Once it is shown and confirmed, the delete goes through.
    api.delete_instance(&target, &none, confirmed(&[LINE_UNCOMMITTED]))
        .unwrap();
    assert_eq!(fx.gh.count("codespace delete -c octo-used --force"), 1);
    // A git state that cannot be read now refuses too.
    let fx = full();
    let api = api_over(&fx);
    *fx.gh.git_error.lock().unwrap() = Some("HTTP 502".into());
    assert!(api
        .delete_instance(&target, &none, confirmed(&[]))
        .unwrap_err()
        .contains("502"));
    assert_eq!(fx.gh.count("codespace delete"), 0);
}

#[test]
fn a_workspace_this_store_does_not_know_is_not_an_orphan_and_needs_a_strong_confirmation() {
    let fx = full();
    // Made by hand or by another app: a codespace of the repository and a Monoptah-named sandbox
    // that no session, start, move or pin of this store names.
    let mut list: Vec<Value> = serde_json::from_str(&fx.gh.list.lock().unwrap()).unwrap();
    list.push(codespace(
        "octo-by-hand",
        "basicLinux32gb",
        "Available",
        hours_ago(5.0),
    ));
    *fx.gh.list.lock().unwrap() = Value::Array(list).to_string();
    fx.sbx.boxes.lock().unwrap().push(bx(
        "sbx_other",
        "monoptah-elsewhere-999999",
        "running",
        2,
        4096,
        hours_ago(1.0),
    ));
    let view = fx.svc.workspaces_overview(PROJECT, false);
    for name in ["octo-by-hand", "monoptah-elsewhere-999999"] {
        let foreign = row(&view, name);
        assert!(!foreign.ours, "{name}");
        assert!(!foreign.orphan(), "{name} must not be called an orphan");
        assert!(foreign.sessions.is_empty());
    }
    // This store's own are unchanged.
    for name in [
        "octo-used",
        "octo-lost",
        "monoptah-fix-login-a1b2c3",
        "monoptah-main-d4e5f6",
    ] {
        assert!(row(&view, name).ours, "{name}");
    }
    let orphans: Vec<_> = view
        .rows
        .iter()
        .filter(|r| r.orphan())
        .map(|r| r.cloud.name.as_str())
        .collect();
    assert_eq!(orphans, ["octo-lost", "monoptah-main-d4e5f6"]);

    // Deleting one says whose it is and needs the acknowledgement, whatever it holds.
    let preview = fx.svc.delete_preview(&target_of("octo-by-hand")).unwrap();
    assert!(!preview.ours && preview.lines.is_empty());
    assert!(preview.notes.contains(&NOTE_NOT_OURS.to_owned()));
    assert!(preview.needs_acknowledgement());
}

#[test]
fn the_api_refuses_a_workspace_this_store_does_not_know_without_the_acknowledgement() {
    let fx = full();
    // A codespace made by hand and a Monoptah-named sandbox made elsewhere.
    let mut list: Vec<Value> = serde_json::from_str(&fx.gh.list.lock().unwrap()).unwrap();
    list.push(codespace(
        "octo-by-hand",
        "basicLinux32gb",
        "Available",
        hours_ago(5.0),
    ));
    *fx.gh.list.lock().unwrap() = Value::Array(list).to_string();
    fx.sbx.boxes.lock().unwrap().push(bx(
        "sbx_other",
        "monoptah-elsewhere-999999",
        "stopped",
        2,
        4096,
        hours_ago(1.0),
    ));
    let api = api_over(&fx);
    let none = |_: &CloudRef| false;
    for name in ["octo-by-hand", "monoptah-elsewhere-999999"] {
        let target = target_of(name);
        // Whatever lines were confirmed, `acknowledged: false` is not enough for a foreign one.
        for options in [
            confirmed_unacknowledged(&[]),
            DeleteOptions {
                confirmed: Some(vec![]),
                ..Default::default()
            },
        ] {
            let error = api.delete_instance(&target, &none, options).unwrap_err();
            assert!(error.contains("did not create"), "{name}: {error}");
        }
        assert_eq!(fx.gh.count("codespace delete"), 0, "{name}");
        assert!(!fx.sbx.verbs().contains(&"rm".to_owned()), "{name}");
    }
    assert!(fx.sbx.state_of("sbx_other").is_some());
    // Acknowledged, it goes through.
    api.delete_instance(&target_of("octo-by-hand"), &none, confirmed(&[]))
        .unwrap();
    assert_eq!(fx.gh.count("codespace delete -c octo-by-hand --force"), 1);
    // A workspace this store knows needs no acknowledgement to be deleted (the dialog still
    // asks for one when something would be lost).
    api.delete_instance(
        &target_of("octo-lost"),
        &none,
        confirmed_unacknowledged(&[]),
    )
    .unwrap();
    assert_eq!(fx.gh.count("codespace delete -c octo-lost --force"), 1);
}

#[test]
fn a_sandbox_in_an_unrecognised_state_is_neither_stopped_nor_deleted() {
    let fx = full();
    let api = api_over(&fx);
    let target = target_of("monoptah-fix-login-a1b2c3");
    // A state this app has no word for may well be a running sandbox.
    fx.sbx
        .boxes
        .lock()
        .unwrap()
        .iter_mut()
        .find(|b| b.id == "sbx_001a")
        .unwrap()
        .state = "quantum".into();
    let error = api
        .delete_instance(&target, &|_: &CloudRef| false, confirmed(&[]))
        .unwrap_err();
    assert!(
        error.contains("does not recognise") && error.contains("quantum"),
        "{error}"
    );
    let verbs = fx.sbx.verbs();
    assert!(
        !verbs.contains(&"rm".to_owned()) && !verbs.contains(&"stop".to_owned()),
        "{:?}",
        fx.sbx.calls()
    );
    assert!(fx.sbx.state_of("sbx_001a").is_some());
    assert_eq!(fx.svc.session_ids_on(&target.cloud).len(), 1);
    // A recognised one still settles: stopped goes straight to the delete.
    fx.sbx
        .boxes
        .lock()
        .unwrap()
        .iter_mut()
        .find(|b| b.id == "sbx_001a")
        .unwrap()
        .state = "stopped".into();
    api.delete_instance(&target, &|_: &CloudRef| false, confirmed(&[]))
        .unwrap();
    assert!(fx.sbx.state_of("sbx_001a").is_none());
}

#[test]
fn what_makes_a_workspace_known_to_this_store() {
    let fx = full();
    let svc = &fx.svc;
    let unknown = CloudRef::codespace("octo-nobody");
    assert!(!svc.known_to_this_app(&unknown));
    // A live session, an archived one, a saved list of what a delete loses.
    assert!(svc.known_to_this_app(&CloudRef::codespace("octo-used")));
    assert!(svc.known_to_this_app(&CloudRef::codespace("octo-lost")));
    svc.record_unsaved_cloud(&unknown, &["A file.".into()])
        .unwrap();
    assert!(svc.known_to_this_app(&unknown));
    // A local session moved into it.
    let moved = CloudRef::codespace("octo-moved");
    assert!(!svc.known_to_this_app(&moved));
    move_local_session(&fx, &moved);
    assert!(svc.known_to_this_app(&moved));
    // A Docker account pin names a sandbox this store created.
    let sandbox = CloudRef::sandbox("monoptah-pinned-123456");
    assert!(!svc.known_to_this_app(&sandbox));
    crate::sandboxes::account::pin(&fx.store.lock().unwrap(), &sandbox.name, Some("acctone"))
        .unwrap();
    assert!(svc.known_to_this_app(&sandbox));
    // A store that cannot be read knows nothing.
    let poisoned = fx.store.clone();
    let _ = std::thread::spawn(move || {
        let _guard = poisoned.lock().unwrap();
        panic!("poison the store");
    })
    .join();
    assert!(!svc.known_to_this_app(&CloudRef::codespace("octo-used")));
}

/// A LOCAL session of [`PROJECT`] whose location is `cloud` (moved there); returns its id.
fn move_local_session(fx: &Fx, cloud: &CloudRef) -> i64 {
    let store = fx.store.lock().unwrap();
    let local = store.create("claude", PROJECT).unwrap();
    store
        .set_cloud_location(
            local.id,
            &SessionLocation::Cloud(CloudLocation {
                cloud: cloud.clone(),
                environment_id: "env-moved".into(),
                project_id: "p".into(),
                host_session_id: "host-1".into(),
                remote_cwd: "/workspaces/widgets".into(),
                local_cwd: PROJECT.into(),
                moved_at_row: 0,
                pushed_branch: None,
            }),
        )
        .unwrap();
    local.id
}

#[test]
fn a_local_session_moved_into_a_workspace_is_counted_and_blocks_its_delete() {
    let fx = full();
    let cloud = CloudRef::codespace("octo-lost");
    let moved = move_local_session(&fx, &cloud);
    let view = fx.svc.workspaces_overview(PROJECT, false);
    let lost = row(&view, "octo-lost");
    // It uses the workspace, so the workspace is no orphan, and the row says it was moved there.
    assert!(!lost.orphan());
    assert!(lost.sessions.iter().any(|s| s.id == moved && s.moved));
    // Deleting it is refused whatever the user confirms: bring the conversation back first.
    let preview = fx.svc.delete_preview(&target_of("octo-lost")).unwrap();
    assert_eq!(preview.blockers.len(), 1, "{preview:?}");
    assert!(preview.blockers[0].contains("moved into it"));
    let api = api_over(&fx);
    let error = api
        .delete_instance(
            &target_of("octo-lost"),
            &|_: &CloudRef| false,
            confirmed(&[]),
        )
        .unwrap_err();
    assert!(error.contains("moved into it"), "{error}");
    assert_eq!(fx.gh.count("codespace delete"), 0);
    // An unreadable location could be this workspace's too, and blocks as well.
    fx.store
        .lock()
        .unwrap()
        .save_setting("cloud-location:9999", "{not json")
        .unwrap();
    assert!(fx
        .svc
        .delete_blockers(&CloudRef::codespace("octo-used"))
        .iter()
        .any(|b| b.contains("cannot be read")));
}

// ---------------------------------------------------------------- Docker accounts

fn use_account(fx: &Fx, account: &str) {
    let store = fx.store.lock().unwrap();
    super::docker_config::save(
        &store,
        PROJECT,
        &super::docker_config::DockerProjectConfig {
            repo_url: "https://github.com/octo/widgets".into(),
            docker_account: Some(account.into()),
            ..Default::default()
        },
    )
    .unwrap();
}

fn bx_in(account: &'static str, id: &str, name: &str, state: &str) -> Bx {
    Bx {
        account: Some(account),
        ..bx(id, name, state, 4, 8192, hours_ago(2.0))
    }
}

#[test]
fn sandboxes_are_listed_as_the_projects_docker_account_not_the_machine_login() {
    let fx = full();
    use_account(&fx, "acctone");
    // The two sandboxes the fixture's sessions are bound to belong to that account too.
    for name in ["monoptah-fix-login-a1b2c3", "monoptah-main-d4e5f6"] {
        crate::sandboxes::account::pin(&fx.store.lock().unwrap(), name, Some("acctone")).unwrap();
    }
    {
        let mut boxes = fx.sbx.boxes.lock().unwrap();
        boxes.clear();
        boxes.push(bx_in("acctone", "sbx_a", "monoptah-acct-a11111", "running"));
        // The machine login has its own sandbox, and no working login at all.
        boxes.push(bx(
            "sbx_m",
            "monoptah-login-m22222",
            "running",
            2,
            4096,
            hours_ago(1.0),
        ));
    }
    fx.sbx
        .ls_fail
        .lock()
        .unwrap()
        .insert(None, sbx_fail("not logged in: run `sbx login`"));
    let view = fx.svc.workspaces_overview(PROJECT, false);
    // Only the project's account was asked: no `sbx login` is needed and none is reported.
    assert_eq!(fx.sbx.accounts_of("ls"), [Some("acctone".to_owned())]);
    assert!(
        view.sandbox_problems.is_empty(),
        "{:?}",
        view.sandbox_problems
    );
    let listed = row(&view, "monoptah-acct-a11111");
    assert_eq!(listed.account.as_deref(), Some("acctone"));
    assert!(view
        .rows
        .iter()
        .all(|r| r.cloud.name != "monoptah-login-m22222"));
    // Rows of the same name in two accounts stay apart.
    assert_ne!(
        target_key(&listed.cloud, Some("acctone")),
        target_key(&listed.cloud, None)
    );
    assert_eq!(listed.key(), target_key(&listed.cloud, Some("acctone")));
}

#[test]
fn a_sandbox_bound_to_another_account_adds_that_account_to_the_listing() {
    let fx = full();
    use_account(&fx, "acctone");
    let name = "monoptah-fix-login-a1b2c3";
    // The session's sandbox was created by another account; the pin remembers which.
    crate::sandboxes::account::pin(&fx.store.lock().unwrap(), name, Some("acctwo")).unwrap();
    crate::sandboxes::account::pin(
        &fx.store.lock().unwrap(),
        "monoptah-main-d4e5f6",
        Some("acctone"),
    )
    .unwrap();
    {
        let mut boxes = fx.sbx.boxes.lock().unwrap();
        boxes.clear();
        boxes.push(bx_in("acctone", "sbx_a", "monoptah-acct-a11111", "stopped"));
        boxes.push(bx_in("acctwo", "sbx_b", name, "running"));
    }
    let view = fx.svc.workspaces_overview(PROJECT, false);
    assert_eq!(
        fx.sbx.accounts_of("ls"),
        [Some("acctone".to_owned()), Some("acctwo".to_owned())]
    );
    assert_eq!(row(&view, name).account.as_deref(), Some("acctwo"));
    assert_eq!(
        row(&view, "monoptah-acct-a11111").account.as_deref(),
        Some("acctone")
    );
}

#[test]
fn an_account_that_cannot_be_listed_is_named_and_the_others_still_list() {
    let fx = full();
    use_account(&fx, "acctone");
    let name = "monoptah-fix-login-a1b2c3";
    crate::sandboxes::account::pin(&fx.store.lock().unwrap(), name, Some("acctwo")).unwrap();
    crate::sandboxes::account::pin(
        &fx.store.lock().unwrap(),
        "monoptah-main-d4e5f6",
        Some("acctone"),
    )
    .unwrap();
    {
        let mut boxes = fx.sbx.boxes.lock().unwrap();
        boxes.clear();
        boxes.push(bx_in("acctone", "sbx_a", "monoptah-acct-a11111", "running"));
        boxes.push(bx_in("acctwo", "sbx_b", name, "running"));
    }
    fx.sbx.ls_fail.lock().unwrap().insert(
        Some("acctwo".into()),
        Err(
            "Docker rejected the access token of this account. Choose another in Cloud settings."
                .into(),
        ),
    );
    let view = fx.svc.workspaces_overview(PROJECT, false);
    assert_eq!(view.sandbox_problems.len(), 1);
    assert!(
        matches!(
            view.sandbox_problems[0].problem,
            Prerequisites::DockerAccount(_)
        ),
        "{:?}",
        view.sandbox_problems
    );
    // Not the machine login's fix.
    assert_eq!(view.sandbox_problems[0].problem.fix_command(), None);
    row(&view, "monoptah-acct-a11111");
    assert!(view.rows.iter().all(|r| r.cloud.name != name));
}

/// The overview row of `name` and a stop or delete API over the same fakes.
fn listed(fx: &Fx, name: &str) -> (WorkspaceRow, CloudApi) {
    let view = fx.svc.workspaces_overview(PROJECT, true);
    (row(&view, name).clone(), api_over(fx))
}

#[test]
fn stop_and_delete_run_as_the_account_the_row_was_listed_under() {
    let fx = full();
    use_account(&fx, "acctone");
    let name = "monoptah-acct-a11111";
    // No pin for it (another install created it): only the listing knows the account.
    {
        let mut boxes = fx.sbx.boxes.lock().unwrap();
        boxes.clear();
        boxes.push(bx_in("acctone", "sbx_a", name, "running"));
    }
    let (found, api) = listed(&fx, name);
    assert_eq!(found.account.as_deref(), Some("acctone"));
    assert!(!found.ours, "nothing in this store names it");
    let none = |_: &CloudRef| false;
    api.stop_instance(&found.target(), &none).unwrap();
    assert_eq!(fx.sbx.accounts_of("stop"), [Some("acctone".to_owned())]);

    let (found, api) = listed(&fx, name);
    api.delete_instance(&found.target(), &none, confirmed(&[]))
        .unwrap();
    // Every call of the delete, the state reads and the removal included, ran as the account.
    for verb in ["inspect", "rm"] {
        let accounts = fx.sbx.accounts_of(verb);
        assert!(!accounts.is_empty(), "{verb}");
        assert!(
            accounts.iter().all(|a| a.as_deref() == Some("acctone")),
            "{verb}: {accounts:?}"
        );
    }
    assert!(fx.sbx.state_of("sbx_a").is_none(), "it was really removed");
}

#[test]
fn a_pin_that_disagrees_with_the_listing_is_left_alone_not_overridden() {
    let fx = full();
    use_account(&fx, "acctone");
    let name = "monoptah-acct-a11111";
    fx.sbx.boxes.lock().unwrap().clear();
    fx.sbx
        .boxes
        .lock()
        .unwrap()
        .push(bx_in("acctone", "sbx_a", name, "running"));
    let (found, api) = listed(&fx, name);
    crate::sandboxes::account::pin(&fx.store.lock().unwrap(), name, Some("acctthree")).unwrap();
    let before = fx.sbx.calls().len();
    let none = |_: &CloudRef| false;
    for error in [
        api.stop_instance(&found.target(), &none).unwrap_err(),
        api.delete_instance(&found.target(), &none, confirmed(&[]))
            .unwrap_err(),
        fx.svc.delete_preview(&found.target()).unwrap_err(),
    ] {
        assert!(error.contains("different Docker account"), "{error}");
    }
    assert_eq!(fx.sbx.calls().len(), before, "nothing was sent to sbx");
    assert!(fx.sbx.state_of("sbx_a").is_some());
}

// ---------------------------------------------------- checks before the destructive call

#[test]
fn the_busy_check_is_made_again_right_before_the_delete() {
    let fx = full();
    let api = api_over(&fx);
    let target = target_of("monoptah-fix-login-a1b2c3");
    // The app is idle when the delete starts (call 0) and archiving by the time the wait ended.
    let view = AppView::working_from(1);
    let error = api
        .delete_instance(&target, &view, confirmed(&[]))
        .unwrap_err();
    assert!(error.contains("archiving or converting"), "{error}");
    assert!(
        !fx.sbx.verbs().contains(&"rm".to_owned()),
        "{:?}",
        fx.sbx.calls()
    );
    assert!(fx.sbx.state_of("sbx_001a").is_some());
    // Its sessions are still there.
    assert_eq!(fx.svc.session_ids_on(&target.cloud).len(), 1);
}

#[test]
fn a_sandbox_that_woke_up_during_the_wait_is_not_deleted() {
    let fx = full();
    let api = api_over(&fx);
    let target = target_of("monoptah-fix-login-a1b2c3");
    // Once it reads `stopped`, a send to a session on it starts it again.
    *fx.sbx.wake_after_stopped.lock().unwrap() = true;
    let error = api
        .delete_instance(&target, &|_: &CloudRef| false, confirmed(&[]))
        .unwrap_err();
    assert!(error.contains("started again"), "{error}");
    assert!(
        !fx.sbx.verbs().contains(&"rm".to_owned()),
        "{:?}",
        fx.sbx.calls()
    );
    assert_eq!(fx.sbx.state_of("sbx_001a").as_deref(), Some("running"));
    assert_eq!(fx.svc.session_ids_on(&target.cloud).len(), 1);
}

#[test]
fn the_guard_runs_before_every_delete_call_and_can_stop_it() {
    let fx = full();
    let cloud = CloudRef::sandbox("monoptah-main-d4e5f6");
    let asked = AtomicUsize::new(0);
    *fx.sbx.rm_refusals.lock().unwrap() = 1;
    // The first removal is refused as too early and retried: the guard ran for both.
    fx.svc
        .delete_settled(&cloud, &|| {
            asked.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .unwrap();
    assert_eq!(asked.load(Ordering::SeqCst), 2);
    // A guard that says no ends the delete before any `rm`.
    let fx = full();
    let error = fx
        .svc
        .delete_settled(&cloud, &|| Err("something changed".into()))
        .unwrap_err();
    assert_eq!(error, "something changed");
    assert!(!fx.sbx.verbs().contains(&"rm".to_owned()));
}

// ------------------------------------------------------------- the overview cache

#[test]
fn every_stop_and_delete_clears_the_cached_overview_not_just_the_api_paths() {
    let fx = full();
    let cached = |fx: &Fx| fx.svc.cached_workspaces(PROJECT).is_some();
    fx.svc.workspaces_overview(PROJECT, false);
    assert!(cached(&fx));
    // The archive dialog and the session Stop call these directly.
    fx.svc
        .stop_cloud(&CloudRef::codespace("octo-used"))
        .unwrap();
    assert!(!cached(&fx), "stop_cloud");
    fx.svc.workspaces_overview(PROJECT, false);
    fx.svc
        .delete_cloud_outcome(&CloudRef::codespace("octo-lost"))
        .unwrap();
    assert!(!cached(&fx), "delete_cloud_outcome");
    // A stop that fails leaves the view stale just as well.
    fx.svc.workspaces_overview(PROJECT, false);
    fx.sbx.boxes.lock().unwrap().clear();
    let _ = fx
        .svc
        .stop_cloud(&CloudRef::sandbox("monoptah-fix-login-a1b2c3"));
    assert!(!cached(&fx), "a failed stop");
    // Forgetting a workspace that is gone.
    fx.svc.workspaces_overview(PROJECT, false);
    let _ = fx
        .svc
        .forget_missing(&CloudRef::sandbox("monoptah-main-d4e5f6"));
    assert!(!cached(&fx), "forget_missing");
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

// ------------------------------------------------- a Stop is closed first and then verified

/// A host that has live connections to some sandboxes and notes, for each connection it closes,
/// which sbx calls had run by then (so a test can tell the close came before the stop).
struct LiveHost {
    machines: Vec<Machine>,
    sbx: Arc<Sbx>,
    /// `(machine id, sbx calls made before it was closed)`.
    closed: Mutex<Vec<(String, Vec<String>)>>,
    /// Closing fails with this.
    close_error: Mutex<Option<String>>,
    disconnected: Mutex<Vec<String>>,
    /// The `listed_at_ms` of every close asked.
    asked: Mutex<Vec<Option<u64>>>,
    /// The connection is busy: closing is deferred.
    busy: Mutex<bool>,
    /// When the open connection was started (ms), for a listing-based close to compare with.
    tunnel_started_ms: Mutex<Option<u64>>,
}

impl Host for LiveHost {
    fn machines(&self) -> Result<Vec<Machine>, String> {
        Ok(self.machines.clone())
    }
    fn connect(
        &self,
        _: &str,
        _: &str,
        _: &AtomicBool,
        _: &mut dyn FnMut(&str),
    ) -> Result<Machine, String> {
        panic!("a stop must not connect")
    }
    fn ensure(&self, id: &str, _: &AtomicBool, _: &mut dyn FnMut(&str)) -> Result<(), String> {
        panic!("a stop must not wake {id}")
    }
    fn request(&self, id: &str, method: &str, _: Value) -> Result<Value, String> {
        panic!("unexpected host request {id} {method}")
    }
    fn disconnect(&self, id: &str) -> Result<(), String> {
        self.disconnected.lock().unwrap().push(id.into());
        Ok(())
    }
    fn close_connection(
        &self,
        id: &str,
        listed_at_ms: Option<u64>,
    ) -> Result<crate::remote::TunnelClose, String> {
        use crate::remote::TunnelClose;
        if let Some(error) = self.close_error.lock().unwrap().clone() {
            return Err(error);
        }
        self.asked.lock().unwrap().push(listed_at_ms);
        if *self.busy.lock().unwrap() {
            return Ok(TunnelClose::Deferred);
        }
        // A connection opened after the listing was read is not that listing's to close.
        if let (Some(listed), Some(started)) =
            (listed_at_ms, *self.tunnel_started_ms.lock().unwrap())
        {
            if started >= listed {
                return Ok(TunnelClose::NoneOpen);
            }
        }
        self.closed
            .lock()
            .unwrap()
            .push((id.into(), self.sbx.calls()));
        Ok(TunnelClose::Closed)
    }
}

fn machine_of(id: &str, sandbox_id: &str, name: &str) -> Machine {
    Machine {
        id: id.into(),
        name: name.into(),
        endpoint: "http://127.0.0.1:1".into(),
        environment_id: format!("env-{id}"),
        ssh: None,
        codespace: None,
        sandbox: Some(crate::remote::SandboxTarget {
            id: sandbox_id.into(),
            name: name.into(),
            workspace: "/home/agent/ws".into(),
            relay_port: 47200,
            config: None,
            mode: Default::default(),
            docker_account: None,
        }),
    }
}

const WORKING: &str = "monoptah-fix-login-a1b2c3";

/// A fixture whose clock the test moves, and whose host holds a live connection to `WORKING`
/// (machine `m-1`) and to another sandbox (machine `m-2`).
struct StopFx {
    fx: Fx,
    host: Arc<LiveHost>,
    clock: Arc<std::sync::atomic::AtomicU64>,
}

fn stop_fx() -> StopFx {
    let gh = Gh::new(json!([]));
    let sbx = Sbx::new(vec![
        bx("sbx_001a", WORKING, "running", 4, 8192, hours_ago(2.0)),
        bx(
            "sbx_001c",
            "monoptah-other-c3d4e5",
            "running",
            2,
            4096,
            hours_ago(1.0),
        ),
    ]);
    let host = Arc::new(LiveHost {
        machines: vec![
            machine_of("m-1", "sbx_001a", WORKING),
            machine_of("m-2", "sbx_001c", "monoptah-other-c3d4e5"),
        ],
        sbx: sbx.clone(),
        closed: Mutex::new(vec![]),
        close_error: Mutex::new(None),
        disconnected: Mutex::new(vec![]),
        asked: Mutex::new(vec![]),
        busy: Mutex::new(false),
        tunnel_started_ms: Mutex::new(None),
    });
    let store = Arc::new(Mutex::new(Store::open(Path::new(":memory:")).unwrap()));
    let clock = Arc::new(std::sync::atomic::AtomicU64::new(now_ms()));
    let ticking = clock.clone();
    let svc = CloudSessions::new(gh.clone(), host.clone(), store.clone())
        .with_timing(
            Duration::from_secs(5),
            |_| {},
            move || ticking.load(Ordering::SeqCst),
        )
        .with_origin(|_| Ok("git@github.com:octo/widgets.git".into()))
        .with_sandboxes(
            sbx.clone(),
            Arc::new(NoSsh),
            Arc::new(StoreSecrets::new(store.clone())),
        );
    add_session(&store, PROJECT, &CloudRef::sandbox(WORKING));
    StopFx {
        fx: Fx {
            svc,
            gh,
            sbx,
            store,
            sleeps: Arc::new(AtomicUsize::new(0)),
        },
        host,
        clock,
    }
}

impl StopFx {
    fn cloud(&self) -> CloudRef {
        CloudRef::sandbox(WORKING)
    }
    fn advance(&self, secs: u64) {
        self.clock.fetch_add(secs * 1000, Ordering::SeqCst);
    }
    /// A forced read of the listing, as the dialog and the chip do it.
    fn read(&self) -> WorkspaceRow {
        let view = self.fx.svc.workspaces_overview(PROJECT, true);
        row(&view, WORKING).clone()
    }
    fn set_state(&self, state: &str) {
        self.fx.sbx.boxes.lock().unwrap()[0].state = state.into();
    }
}

#[test]
fn a_stop_closes_the_sessions_connection_to_that_sandbox_before_it_sends_the_stop() {
    let t = stop_fx();
    t.fx.svc.stop_cloud(&t.cloud()).unwrap();
    let closed = t.host.closed.lock().unwrap().clone();
    assert_eq!(
        closed.len(),
        1,
        "only the machine of this sandbox: {closed:?}"
    );
    assert_eq!(closed[0].0, "m-1");
    assert!(
        !closed[0].1.iter().any(|call| call.starts_with("stop")),
        "the connection was still open when the stop went out: {:?}",
        closed[0].1
    );
    assert!(t
        .fx
        .sbx
        .calls()
        .iter()
        .any(|call| call.starts_with("stop sbx_001a")));
    // It only stops: the machine is kept for Reconnect and nothing is deleted.
    assert!(t.host.disconnected.lock().unwrap().is_empty());
    assert!(!t.fx.sbx.verbs().contains(&"rm".to_owned()));
    assert_eq!(t.fx.sbx.state_of("sbx_001a").as_deref(), Some("stopping"));
    assert_eq!(
        t.fx.sbx.state_of("sbx_001c").as_deref(),
        Some("running"),
        "another sandbox is not touched"
    );
}

#[test]
fn a_stop_that_cannot_close_the_connection_still_stops_and_says_so_when_it_comes_back() {
    let t = stop_fx();
    *t.host.close_error.lock().unwrap() = Some("tunnel lock poisoned".into());
    t.fx.svc.stop_cloud(&t.cloud()).unwrap();
    assert_eq!(t.fx.sbx.state_of("sbx_001a").as_deref(), Some("stopping"));
    // Something wakes it again.
    t.set_state("running");
    t.advance(120);
    let row = t.read();
    match row.stop {
        Some(StopReport::StillRunning(reason)) => assert!(
            reason.contains("could not close") && reason.contains("tunnel lock poisoned"),
            "{reason}"
        ),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_stop_is_followed_from_stopping_to_stopped_by_the_listing() {
    let t = stop_fx();
    t.fx.svc.stop_cloud(&t.cloud()).unwrap();
    let requested = t.clock.load(Ordering::SeqCst);

    t.advance(20);
    let row = t.read();
    assert_eq!(row.state, WorkspaceState::Stopping);
    assert_eq!(row.stop, Some(StopReport::Stopping));
    assert_eq!(
        row.stopping_since_ms,
        Some(requested),
        "counted from the request"
    );

    t.set_state("stopped");
    t.advance(20);
    let row = t.read();
    assert_eq!(row.state, WorkspaceState::Stopped);
    assert_eq!(row.stop, Some(StopReport::Stopped));
    assert_eq!(row.stopping_since_ms, None);
    // Nothing woke it to find out: only listings, and the one stop.
    assert_eq!(
        t.fx.sbx.verbs().iter().filter(|v| *v == "stop").count(),
        1,
        "{:?}",
        t.fx.sbx.calls()
    );
}

#[test]
fn a_stop_that_takes_longer_than_expected_is_said_so_and_keeps_counting() {
    let t = stop_fx();
    t.fx.svc.stop_cloud(&t.cloud()).unwrap();
    t.advance(STOP_EXPECTED.as_secs() - 1);
    assert_eq!(t.read().stop, Some(StopReport::Stopping));
    t.advance(1);
    let row = t.read();
    assert_eq!(row.state, WorkspaceState::Stopping);
    assert_eq!(row.stop, Some(StopReport::TakingLong));
    t.advance(20 * 60);
    let row = t.read();
    assert_eq!(row.stop, Some(StopReport::TakingLong));
    assert_eq!(
        row.stopping_since_ms,
        Some(now_ms()),
        "still counted from the request, not from the last read"
    );
}

#[test]
fn a_sandbox_that_is_running_again_after_a_stop_is_told_never_silently_running() {
    let t = stop_fx();
    t.fx.svc.stop_cloud(&t.cloud()).unwrap();
    // Within the grace period a listing that has not caught up is not yet a verdict.
    t.set_state("running");
    t.advance(30);
    let row = t.read();
    assert_eq!(row.state, WorkspaceState::Running);
    assert_eq!(row.stop, Some(StopReport::Stopping));

    t.advance(60);
    let row = t.read();
    let Some(StopReport::StillRunning(reason)) = row.stop.clone() else {
        panic!("{:?}", row.stop)
    };
    assert!(
        reason.contains("Auto-resume") && reason.contains("closed its own connection"),
        "{reason}"
    );
    // It stays said until the user acts, however long after.
    t.advance(3600);
    assert_eq!(t.read().stop, row.stop);
    // Another Stop starts over.
    t.fx.svc.stop_cloud(&t.cloud()).unwrap();
    assert_eq!(t.read().stop, Some(StopReport::Stopping));
}

#[test]
fn a_hibernating_sandbox_counts_as_stopping_and_one_nobody_stopped_is_timed_from_first_sight() {
    let t = stop_fx();
    t.set_state("hibernating");
    let first_seen = t.clock.load(Ordering::SeqCst);
    t.advance(5);
    let row = t.read();
    assert_eq!(row.state, WorkspaceState::Hibernating);
    // Not this app's stop: counted from the first listing that showed it going down.
    assert_eq!(row.stopping_since_ms, Some(first_seen + 5000));
    assert_eq!(
        row.stop, None,
        "nobody asked this app to stop it: no report, no polling"
    );
    t.advance(300);
    assert_eq!(t.read().stopping_since_ms, Some(first_seen + 5000));
    t.set_state("stopped");
    let row = t.read();
    assert_eq!(row.stopping_since_ms, None);
    assert_eq!(row.stop, None, "no Stop of this app to report on");
}

#[test]
fn a_stop_the_cli_refuses_is_an_error_and_nothing_is_followed() {
    let t = stop_fx();
    *t.fx.sbx.stop_error.lock().unwrap() = Some("error: cloud API returned 503".into());
    let error = t.fx.svc.stop_cloud(&t.cloud()).unwrap_err();
    assert!(error.contains("503"), "{error}");
    assert!(
        error.contains("Reconnect"),
        "it says the sessions lost their connection: {error}"
    );
    assert_eq!(t.read().stop, None);
}

#[test]
fn a_codespace_stop_is_not_followed_reported_or_polled() {
    let fx = fx(
        Gh::new(json!([codespace(
            "octo-used",
            "basicLinux32gb",
            "Available",
            hours_ago(3.0)
        )])),
        Sbx::new(vec![]),
    );
    let cloud = CloudRef::codespace("octo-used");
    add_session(&fx.store, PROJECT, &cloud);
    fx.svc.stop_cloud(&cloud).unwrap();
    assert_eq!(fx.svc.stops.lock().unwrap().followed(), 0, "nothing noted");
    // Even if the listing lags past the grace period and says running, or says stopping, no
    // sandbox text and no report appear: the row is the listing's state alone.
    let row = fx.svc.workspaces_overview(PROJECT, true).rows[0].clone();
    assert_eq!(row.stop, None);
    *fx.gh.list.lock().unwrap() = json!([codespace(
        "octo-used",
        "basicLinux32gb",
        "ShuttingDown",
        hours_ago(3.0)
    )])
    .to_string();
    let row = fx.svc.workspaces_overview(PROJECT, true).rows[0].clone();
    assert_eq!(row.state, WorkspaceState::Stopping);
    assert_eq!(
        row.stop, None,
        "no report, so the dialog never polls for it"
    );
    assert!(row.stopping_since_ms.is_some(), "the chip still times it");
}

#[test]
fn a_stopped_codespace_closes_no_connection() {
    let gh = Gh::new(json!([codespace(
        "octo-used",
        "basicLinux32gb",
        "Available",
        hours_ago(3.0)
    )]));
    let t = stop_fx();
    let svc = CloudSessions::new(gh, t.host.clone(), t.fx.store.clone());
    svc.stop_cloud(&CloudRef::codespace("octo-used")).unwrap();
    assert!(t.host.closed.lock().unwrap().is_empty());
}

#[test]
fn deleting_a_stopped_sandbox_forgets_the_stop_that_was_followed() {
    let t = stop_fx();
    t.fx.svc.stop_cloud(&t.cloud()).unwrap();
    t.set_state("stopped");
    t.fx.svc.delete_cloud_outcome(&t.cloud()).unwrap();
    assert!(t.fx.sbx.state_of("sbx_001a").is_none());
    assert_eq!(t.fx.svc.stops.lock().unwrap().followed(), 0);
    let view = t.fx.svc.workspaces_overview(PROJECT, true);
    assert!(view.rows.iter().all(|r| r.cloud.name != WORKING));
}

#[test]
fn a_listing_that_shows_a_sandbox_down_closes_the_tunnel_that_would_wake_it_again() {
    let t = stop_fx();
    // Someone else stopped it (the TTL, another computer): this app's tunnel is still open.
    t.set_state("stopped");
    t.read();
    assert!(
        t.host.closed.lock().unwrap().is_empty(),
        "one reading may be stale: a connection somebody just opened is not closed on it"
    );
    t.read();
    let closed = t.host.closed.lock().unwrap().clone();
    assert_eq!(
        closed.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(),
        ["m-1"],
        "only the machine of the stopped sandbox: {closed:?}"
    );
    assert!(
        t.host.disconnected.lock().unwrap().is_empty(),
        "the machine is kept"
    );
    // Nothing was woken or deleted to find out.
    assert_eq!(t.fx.sbx.verbs().iter().filter(|v| *v == "ls").count(), 2);
    assert!(!t
        .fx
        .sbx
        .verbs()
        .iter()
        .any(|v| matches!(v.as_str(), "stop" | "rm")));
}

#[test]
fn a_running_sandbox_keeps_its_tunnel_and_so_does_one_a_reconnect_is_bringing_back() {
    let t = stop_fx();
    t.read();
    assert!(t.host.closed.lock().unwrap().is_empty());
    t.set_state("stopped");
    t.fx.svc.reconnecting.lock().unwrap().insert(WORKING.into());
    t.read();
    t.read();
    t.read();
    assert!(
        t.host.closed.lock().unwrap().is_empty(),
        "the user is resuming it right now"
    );
}

#[test]
fn a_sandbox_that_wakes_after_it_was_seen_stopped_is_never_quietly_running() {
    use super::footer_status::{Concern, FooterStatus};
    let t = stop_fx();
    t.fx.svc.stop_cloud(&t.cloud()).unwrap();
    t.set_state("stopped");
    t.advance(20);
    assert_eq!(t.read().stop, Some(StopReport::Stopped));

    // Another computer's tunnel, the TTL or Auto-resume traffic wakes it: it must say so.
    t.set_state("running");
    t.advance(300);
    let row = t.read();
    assert_eq!(row.state, WorkspaceState::Running);
    let Some(StopReport::StillRunning(reason)) = row.stop.clone() else {
        panic!("quietly running again: {:?}", row.stop)
    };
    assert!(
        reason.contains("after it had stopped") && reason.contains("Auto-resume"),
        "{reason}"
    );
    // The chip warns too.
    let view = t.fx.svc.workspaces_overview(PROJECT, true);
    let status = FooterStatus::of(&view, t.clock.load(Ordering::SeqCst));
    let entry = status.entries.iter().find(|e| e.name == WORKING).unwrap();
    assert_eq!(entry.concern, Some(Concern::StopUndone));
    // It stays said however long, until it is seen stopped again...
    t.advance(7200);
    assert_eq!(t.read().stop, row.stop);
    t.set_state("stopped");
    assert_eq!(t.read().stop, Some(StopReport::Stopped));
    // ...or the user brings it back on purpose.
    t.set_state("running");
    t.advance(60);
    assert!(matches!(t.read().stop, Some(StopReport::StillRunning(_))));
    t.fx.svc.forget_stop(&t.cloud());
    assert_eq!(t.read().stop, None);
}

#[test]
fn a_second_stop_of_a_sandbox_already_stopping_keeps_the_first_request_time() {
    let t = stop_fx();
    t.fx.svc.stop_cloud(&t.cloud()).unwrap();
    let first = t.clock.load(Ordering::SeqCst);
    t.advance(30);
    assert_eq!(t.read().stopping_since_ms, Some(first));
    t.advance(30);
    t.fx.svc.stop_cloud(&t.cloud()).unwrap();
    t.advance(30);
    assert_eq!(
        t.read().stopping_since_ms,
        Some(first),
        "the timers do not restart"
    );
}

#[test]
fn a_deleting_sandbox_is_neither_stopping_nor_followed() {
    use super::footer_status::FooterStatus;
    let t = stop_fx();
    t.set_state("deleting");
    let view = t.fx.svc.workspaces_overview(PROJECT, true);
    let row = row(&view, WORKING);
    assert!(row.is_deleting() && !row.going_down());
    assert_eq!((row.stop.clone(), row.stopping_since_ms), (None, None));
    let status = FooterStatus::of(&view, t.clock.load(Ordering::SeqCst));
    assert_eq!(status.stopping, 0, "{status:?}");
    t.advance(3600);
    let view = t.fx.svc.workspaces_overview(PROJECT, true);
    assert!(!FooterStatus::of(&view, t.clock.load(Ordering::SeqCst)).warns());
}

#[test]
fn a_stop_that_fails_before_it_is_sent_leaves_the_connections_open() {
    let t = stop_fx();
    // The sandbox's id cannot be read: nothing was stopped, so nothing may be disconnected.
    *t.fx.sbx.ls_reply.lock().unwrap() = Some(Err("sbx is unreachable".into()));
    let error = t.fx.svc.stop_cloud(&t.cloud()).unwrap_err();
    assert!(error.contains("unreachable"), "{error}");
    assert!(t.host.closed.lock().unwrap().is_empty(), "the tunnel stays");
    assert!(!t.fx.sbx.verbs().contains(&"stop".to_owned()));
    assert_eq!(t.fx.svc.stops.lock().unwrap().followed(), 0);
}

#[test]
fn a_connection_that_could_not_be_closed_is_told_when_the_stop_is_accepted() {
    let t = stop_fx();
    *t.host.close_error.lock().unwrap() = Some("tunnel lock poisoned".into());
    let note =
        t.fx.svc
            .stop_cloud_noted(&t.cloud())
            .unwrap()
            .expect("a note");
    assert!(
        note.contains("could not close") && note.contains("tunnel lock poisoned"),
        "{note}"
    );
    *t.host.close_error.lock().unwrap() = None;
    let t = stop_fx();
    assert_eq!(t.fx.svc.stop_cloud_noted(&t.cloud()).unwrap(), None);
}

#[test]
fn a_followed_stop_whose_listing_cannot_be_read_says_so_and_comes_back() {
    let t = stop_fx();
    t.fx.svc.stop_cloud(&t.cloud()).unwrap();
    t.advance(10);
    *t.fx.sbx.ls_reply.lock().unwrap() = Some(Err("sbx is unreachable".into()));
    let view = t.fx.svc.workspaces_overview(PROJECT, true);
    assert!(view.rows.iter().all(|r| r.cloud.name != WORKING));
    assert_eq!(view.stop_unread.len(), 1, "{:?}", view.stop_unread);
    let text = &view.stop_unread[0].text;
    assert!(
        text.starts_with(&format!("Could not read the state of {WORKING}:")),
        "{text}"
    );
    // Unreadable for a long time: the stop is followed no longer than a limit.
    t.advance(10);
    assert_eq!(
        t.fx.svc
            .workspaces_overview(PROJECT, true)
            .stop_unread
            .len(),
        1
    );
    // The listing comes back: the row, and no more notes.
    *t.fx.sbx.ls_reply.lock().unwrap() = None;
    t.set_state("stopped");
    let view = t.fx.svc.workspaces_overview(PROJECT, true);
    assert!(view.stop_unread.is_empty());
    assert_eq!(row(&view, WORKING).stop, Some(StopReport::Stopped));
}

#[test]
fn a_followed_stop_with_a_listing_that_stays_unreadable_is_given_up_after_a_limit() {
    let t = stop_fx();
    t.fx.svc.stop_cloud(&t.cloud()).unwrap();
    *t.fx.sbx.ls_reply.lock().unwrap() = Some(Err("sbx is unreachable".into()));
    assert_eq!(
        t.fx.svc
            .workspaces_overview(PROJECT, true)
            .stop_unread
            .len(),
        1
    );
    t.advance(31 * 60);
    assert!(t
        .fx
        .svc
        .workspaces_overview(PROJECT, true)
        .stop_unread
        .is_empty());
    assert_eq!(t.fx.svc.stops.lock().unwrap().followed(), 0);
}

#[test]
fn a_followed_stop_that_never_reappears_in_a_readable_listing_is_dropped() {
    let t = stop_fx();
    t.fx.svc.stop_cloud(&t.cloud()).unwrap();
    // Deleted by hand: the listing is fine and no longer has it.
    t.fx.sbx.boxes.lock().unwrap().remove(0);
    for _ in 0..2 {
        let view = t.fx.svc.workspaces_overview(PROJECT, true);
        assert_eq!(view.stop_unread.len(), 1);
        assert!(view.stop_unread[0].text.contains("not in the listing"));
    }
    let view = t.fx.svc.workspaces_overview(PROJECT, true);
    assert!(view.stop_unread.is_empty(), "given up after three listings");
    assert_eq!(t.fx.svc.stops.lock().unwrap().followed(), 0);
}

#[test]
fn the_follow_tick_reads_only_the_sandbox_listing() {
    let t = stop_fx();
    t.read();
    let gh_calls = t.fx.gh.calls().len();
    let ls_calls = || t.fx.sbx.verbs().iter().filter(|v| *v == "ls").count();
    t.fx.svc.stop_cloud(&t.cloud()).unwrap();
    t.read(); // the full forced read that follows a stop fills the cache again
    let gh_after_full = t.fx.gh.calls().len();
    assert!(gh_after_full > gh_calls);
    t.advance(20);
    t.set_state("stopped");
    let before = ls_calls();
    let view = t.fx.svc.refresh_sandbox_rows(PROJECT);
    assert_eq!(
        t.fx.gh.calls().len(),
        gh_after_full,
        "no gh call on the tick"
    );
    assert_eq!(ls_calls(), before + 1, "one more sbx ls");
    assert_eq!(row(&view, WORKING).stop, Some(StopReport::Stopped));
    assert_eq!(view.rows.len(), 2, "the other sandbox row is still there");
}

#[test]
fn another_projects_load_does_not_count_a_followed_stop_as_missing() {
    let t = stop_fx();
    t.fx.svc.stop_cloud(&t.cloud()).unwrap();
    // The user switches project: the chip reads the other project about once a minute. Sessions of
    // the first project use the sandbox, so the second project's listing does not show it.
    for _ in 0..6 {
        t.advance(60);
        let other = t.fx.svc.workspaces_overview("/work/other", true);
        assert!(other.rows.iter().all(|r| r.cloud.name != WORKING));
        assert!(
            other.stop_unread.is_empty(),
            "no line about a sandbox this project does not list: {:?}",
            other.stop_unread
        );
    }
    // The same when that project's sandbox listing fails.
    *t.fx.sbx.ls_reply.lock().unwrap() = Some(Err("sbx is unreachable".into()));
    assert!(t
        .fx
        .svc
        .workspaces_overview("/work/other", true)
        .stop_unread
        .is_empty());
    *t.fx.sbx.ls_reply.lock().unwrap() = None;
    assert_eq!(
        t.fx.svc.stops.lock().unwrap().followed(),
        1,
        "the Stop is still followed"
    );
    // Back in the first project it is read as before.
    t.set_state("stopped");
    assert_eq!(t.read().stop, Some(StopReport::Stopped));
}

#[test]
fn a_sandbox_no_session_uses_is_missed_by_every_project() {
    let t = stop_fx();
    // The other sandbox has no session: every project lists it, so one that vanishes is missed
    // wherever the load is.
    let other = CloudRef::sandbox("monoptah-other-c3d4e5");
    t.fx.svc.stop_cloud(&other).unwrap();
    t.fx.sbx.boxes.lock().unwrap().remove(1);
    let view = t.fx.svc.workspaces_overview("/work/elsewhere", true);
    assert_eq!(view.stop_unread.len(), 1);
}

#[test]
fn a_listing_read_before_a_connection_was_opened_never_closes_it() {
    let t = stop_fx();
    let read_at = t.clock.load(Ordering::SeqCst);
    // A Reconnect finished and opened a tunnel just after this listing was read.
    *t.host.tunnel_started_ms.lock().unwrap() = Some(read_at + 1_000);
    t.set_state("stopped");
    t.read();
    t.read();
    t.read();
    assert!(
        t.host.closed.lock().unwrap().is_empty(),
        "closed a connection that is newer than the listing"
    );
    assert!(t
        .host
        .asked
        .lock()
        .unwrap()
        .iter()
        .all(|at| *at == Some(read_at)));
    // A listing read after the tunnel started may close it.
    t.advance(120);
    t.read();
    assert_eq!(t.host.closed.lock().unwrap().len(), 1);
    assert_eq!(
        t.host.asked.lock().unwrap().last().copied(),
        Some(Some(read_at + 120_000)),
        "the time the listing was read is what is passed"
    );
}

#[test]
fn a_connection_that_is_busy_is_told_when_the_stop_is_accepted() {
    let t = stop_fx();
    *t.host.busy.lock().unwrap() = true;
    let note =
        t.fx.svc
            .stop_cloud_noted(&t.cloud())
            .unwrap()
            .expect("a note");
    assert!(
        note.contains("busy") && note.contains("may wake the sandbox again"),
        "{note}"
    );
    assert_eq!(t.fx.sbx.state_of("sbx_001a").as_deref(), Some("stopping"));
    // And if it does come back, the reason says what happened to the connection.
    t.set_state("running");
    t.advance(120);
    let Some(StopReport::StillRunning(reason)) = t.read().stop else {
        panic!("not told")
    };
    assert!(reason.contains("busy"), "{reason}");
}

#[test]
fn a_still_running_line_can_be_dismissed_but_a_stop_on_its_way_cannot() {
    use super::footer_status::{Concern, FooterStatus};
    let t = stop_fx();
    t.fx.svc.stop_cloud(&t.cloud()).unwrap();
    assert!(
        !t.fx.svc.dismiss_stop(&t.cloud()),
        "still stopping: nothing to dismiss"
    );
    t.set_state("stopped");
    t.advance(20);
    t.read();
    // Resumed by other means (sbx run, another computer).
    t.set_state("running");
    t.advance(60);
    assert!(matches!(t.read().stop, Some(StopReport::StillRunning(_))));
    assert!(t.fx.svc.dismiss_stop(&t.cloud()));
    let view = t.fx.svc.workspaces_overview(PROJECT, true);
    assert_eq!(row(&view, WORKING).stop, None);
    let status = FooterStatus::of(&view, t.clock.load(Ordering::SeqCst));
    assert!(status
        .entries
        .iter()
        .all(|e| e.concern != Some(Concern::StopUndone)));
    assert!(!t.fx.svc.dismiss_stop(&t.cloud()), "nothing left");
    assert_eq!(t.fx.svc.stops.lock().unwrap().followed(), 0);
}

#[test]
fn what_is_remembered_of_a_sandbox_that_is_gone_does_not_pile_up() {
    let t = stop_fx();
    t.fx.svc.stop_cloud(&t.cloud()).unwrap();
    t.set_state("stopped");
    t.read();
    t.read();
    assert_eq!(
        t.fx.svc.stops.lock().unwrap().followed(),
        1,
        "a Stopped verdict is kept"
    );
    // Deleted by hand: a settled verdict has nobody to wait for, so it is dropped quietly.
    t.fx.sbx.boxes.lock().unwrap().remove(0);
    for _ in 0..3 {
        let view = t.fx.svc.workspaces_overview(PROJECT, true);
        assert!(view.stop_unread.is_empty());
    }
    let watches = t.fx.svc.stops.lock().unwrap();
    assert_eq!(watches.followed(), 0);
    assert_eq!(
        watches.down_counted(),
        0,
        "the down counts follow the listing"
    );
}
