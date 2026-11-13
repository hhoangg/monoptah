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
