//! H-09: what the catalog, reads and watch frames say about cloud and remote
//! sessions, for web devices and for phones, from cached state only.
use super::*;
use crate::shell::cloud::CloudMark;
use gpui::TestAppContext;
use monocode_core::{
    cloud_sessions::{CloudBackendKind, CloudSessions, CloudState, Host, StepKind, StepStatus},
    codespaces::{GhOutput, GhRunner},
    mobile::{DeviceKind, Permission},
    remote::Machine,
};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// The rail project the window fixture opens.
const PROJECT: &str = "/work/monocode";
const REMOTE_CWD: &str = "remote://box/home/me/app";

fn device(kind: DeviceKind, projects: Option<Vec<String>>) -> DeviceContext {
    DeviceContext {
        id: format!("{kind:?}"),
        kind,
        permission: Permission::Control,
        projects,
        monos: None,
    }
}

/// Counts every call that would reach `gh` or a remote host.
#[derive(Clone, Default)]
struct Calls(Arc<AtomicUsize>);
impl Calls {
    fn count(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }
}
struct CountingGh(Calls);
impl GhRunner for CountingGh {
    fn run(&self, _: &[String], _: Option<&str>) -> Result<GhOutput, String> {
        self.0.0.fetch_add(1, Ordering::SeqCst);
        Err("a read must not call gh".into())
    }
}
struct CountingHost(Calls);
impl CountingHost {
    fn hit<T>(&self) -> Result<T, String> {
        self.0.0.fetch_add(1, Ordering::SeqCst);
        Err("a read must not reach a host".into())
    }
}
impl Host for CountingHost {
    fn machines(&self) -> Result<Vec<Machine>, String> {
        self.hit()
    }
    fn connect(
        &self,
        _: &str,
        _: &str,
        _: &AtomicBool,
        _: &mut dyn FnMut(&str),
    ) -> Result<Machine, String> {
        self.hit()
    }
    fn ensure(&self, _: &str, _: &AtomicBool, _: &mut dyn FnMut(&str)) -> Result<(), String> {
        self.hit()
    }
    fn request(&self, _: &str, _: &str, _: Value) -> Result<Value, String> {
        self.hit()
    }
    fn disconnect(&self, _: &str) -> Result<(), String> {
        self.hit()
    }
}

struct World {
    codespace: i64,
    sandbox: i64,
    stopped: i64,
    remote: i64,
    moved: i64,
    local: i64,
    other: i64,
    calls: Calls,
}

/// A session in `cwd`: a local folder, or a `remote://env/path` project the
/// store must know first (it refuses a remote cwd it has no project for).
fn add(shell: &mut Shell, cwd: &str, cloud: Option<(CloudBackendKind, &str, CloudState)>) -> i64 {
    use monocode_core::cloud_sessions::CloudRef;
    let mut summary = {
        let store = locked(&shell.store).unwrap();
        let key = match monocode_core::remote::parse_remote_path(cwd) {
            None => cwd.to_owned(),
            Some(path) => {
                let project = json!({"id": "p", "cwd": path.host_path});
                match cloud {
                    Some((kind, name, _)) => {
                        let cloud_ref = CloudRef {
                            kind,
                            name: name.into(),
                        };
                        store
                            .remember_cloud_ref_project(
                                &path.environment_id,
                                &project,
                                PROJECT,
                                &cloud_ref,
                            )
                            .unwrap()
                            .key
                    }
                    None => {
                        store
                            .remember_remote_project(&path.environment_id, &project)
                            .unwrap()
                            .key
                    }
                }
            }
        };
        assert_eq!(key, cwd);
        store.create("claude", &key).unwrap()
    };
    summary.has_messages = true;
    let id = summary.id;
    shell.session_summaries.insert(id, summary);
    shell.order.push(id);
    if let Some((kind, name, state)) = cloud {
        let mark = CloudMark {
            local_project: PROJECT.into(),
            codespace: name.into(),
            kind,
        };
        shell.cloud_states.insert(mark.key(), state);
        shell.cloud_marks.insert(id, mark);
    }
    id
}

/// One session of each kind on top of the window fixture, a service whose
/// every outside call is counted and refused.
fn world(cx: &mut TestAppContext) -> (gpui::WindowHandle<Shell>, World) {
    let (window, other, local) = crate::shell::tests::window(cx);
    let world = window
        .update(cx, |shell, _, cx| {
            let calls = Calls::default();
            shell.cloud = Arc::new(CloudSessions::new(
                Arc::new(CountingGh(calls.clone())),
                Arc::new(CountingHost(calls.clone())),
                shell.store.clone(),
            ));
            use CloudBackendKind::{Codespaces, DockerSandbox};
            let world = World {
                codespace: add(
                    shell,
                    "remote://env-cs1/workspaces/widgets",
                    Some((Codespaces, "cs1", CloudState::Available)),
                ),
                sandbox: add(
                    shell,
                    "remote://env-sb1/workspaces/widgets",
                    Some((DockerSandbox, "sb1", CloudState::Available)),
                ),
                stopped: add(
                    shell,
                    "remote://env-cs2/workspaces/widgets",
                    Some((Codespaces, "cs2", CloudState::Stopped)),
                ),
                remote: add(shell, REMOTE_CWD, None),
                // Moved: the local row and cwd are kept.
                moved: add(
                    shell,
                    PROJECT,
                    Some((Codespaces, "cs3", CloudState::Available)),
                ),
                local,
                other,
                calls,
            };
            shell.sessions[&local].update(cx, |chat, _| chat.summary.has_messages = true);
            world
        })
        .unwrap();
    (window, world)
}

fn entry_of(shell: &Shell, id: i64, cx: &App) -> CatalogEntry {
    let paths = shell.mobile_projects(cx);
    shell
        .mobile_catalog_sessions(&paths, cx)
        .into_iter()
        .find(|entry| entry.id == id)
        .unwrap_or_else(|| panic!("session {id} is not in the catalog"))
}

/// The catalog JSON the app would publish, built from the same pieces as
/// `publish_mobile_catalog`: session cards, rail projects and the extra
/// remote projects. `today` leaves out what this change adds: it lists only
/// the sessions whose own cwd is a rail project and adds no field.
fn catalog(shell: &Shell, today: bool, cx: &App) -> Value {
    let paths = shell.mobile_projects(cx);
    let entries = shell.mobile_catalog_sessions(&paths, cx);
    let extra = cloud::web_only_projects(
        entries
            .iter()
            .filter_map(|entry| Some((entry.cloud.as_ref()?, entry.summary.cwd.as_str()))),
    );
    let on_rail = |cwd: &str| {
        paths
            .iter()
            .any(|p| monocode_core::files::path_key(p) == monocode_core::files::path_key(cwd))
    };
    let extra = if today { Vec::new() } else { extra };
    let projects: Vec<Value> = paths
        .iter()
        .chain(&extra)
        .map(|path| {
            let mut project = json!({"id": project_id(path), "path": path, "name": path});
            if extra.contains(path) {
                cloud::mark_web_only(&mut project);
            }
            project
        })
        .collect();
    let sessions: Vec<Value> = entries
        .iter()
        .filter(|entry| !today || on_rail(&entry.summary.cwd))
        .map(|entry| {
            let mut card = mobile_catalog::session(
                &entry.summary,
                &format!("c{}", entry.id),
                &entry.project,
                &Default::default(),
                None,
                "",
            );
            if !today {
                cloud::annotate_card(&mut card, entry.cloud.as_ref());
            }
            card
        })
        .collect();
    json!({"projects": projects, "monos": [], "sessions": sessions})
}

fn card(catalog: &Value, id: i64) -> Option<&Value> {
    let want = format!("c{id}");
    catalog["sessions"]
        .as_array()?
        .iter()
        .find(|card| card["id"] == want)
}

#[gpui::test]
fn the_catalog_files_each_kind_under_its_project_with_its_fields_for_web(cx: &mut TestAppContext) {
    let (window, w) = world(cx);
    window
        .update(cx, |shell, _, cx| {
            let project = project_id(PROJECT);
            let mut web = catalog(shell, false, cx);
            policy::filter_reply(&device(DeviceKind::Web, None), "workspace.list", &mut web);
            let target = |id: i64| card(&web, id).unwrap_or_else(|| panic!("{id} missing"));

            // Codespace, sandbox and a stopped one sit under their local twin.
            let cs = target(w.codespace);
            assert_eq!(cs["projectId"], project);
            assert_eq!(
                cs["target"],
                json!({"kind": "codespace", "instance": "cs1", "state": "available"})
            );
            assert_eq!(cs["cloudState"], "available");
            let sb = target(w.sandbox);
            assert_eq!(sb["projectId"], project);
            assert_eq!(sb["target"]["kind"], "sandbox");
            assert_eq!(sb["target"]["instance"], "sb1");
            let stopped = target(w.stopped);
            assert_eq!(stopped["cloudState"], "stopped");
            assert_eq!(stopped["target"]["state"], "stopped");

            // A remote-machine session has its own remote project.
            let remote = target(w.remote);
            assert_eq!(remote["projectId"], project_id(REMOTE_CWD));
            assert_eq!(
                remote["target"],
                json!({"kind": "remote", "instance": "box"})
            );
            assert!(remote.get("cloudState").is_none());
            let own = web["projects"]
                .as_array()
                .unwrap()
                .iter()
                .find(|p| p["id"] == project_id(REMOTE_CWD))
                .expect("the remote project is listed for web devices");
            assert_eq!(own["path"], REMOTE_CWD);

            // A moved session keeps its local row and gains its cloud target.
            let moved = target(w.moved);
            assert_eq!(moved["projectId"], project);
            assert_eq!(moved["target"]["kind"], "codespace");
            assert_eq!(moved["target"]["instance"], "cs3");

            // Local sessions say so.
            assert_eq!(target(w.local)["target"], json!({"kind": "local"}));
            assert_eq!(target(w.other)["target"], json!({"kind": "local"}));
            assert!(!web.to_string().contains("webOnly"));
        })
        .unwrap();
}

#[gpui::test]
fn a_phone_gets_exactly_todays_lists_and_a_moved_session_stays_local(cx: &mut TestAppContext) {
    let (window, w) = world(cx);
    window
        .update(cx, |shell, _, cx| {
            // Today's catalog: no hook, no new field.
            let today = catalog(shell, true, cx);
            let shared = catalog(shell, false, cx);
            assert!(
                shared.to_string().contains("webOnly"),
                "the shared catalog is marked"
            );
            let ids = |catalog: &Value| -> Vec<String> {
                catalog["sessions"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|card| card["id"].as_str().unwrap().to_owned())
                    .collect()
            };
            for method in ["workspace.list", "sessions.list"] {
                let mut reply = shared.clone();
                policy::filter_reply(&device(DeviceKind::Phone, None), method, &mut reply);
                // Exactly today's: same sessions, same cards, same projects.
                assert_eq!(ids(&reply), ids(&today), "{method}");
                assert_eq!(reply, today, "{method}");
                // The cloud-born sessions and the remote machine are not there.
                let listed = ids(&reply);
                for hidden in [w.codespace, w.sandbox, w.stopped, w.remote] {
                    assert!(!listed.contains(&format!("c{hidden}")), "{hidden}");
                }
                // The moved session is listed as local, with no cloud field.
                let moved = card(&reply, w.moved).expect("a moved session stays listed");
                assert!(moved.get("target").is_none() && moved.get("cloudState").is_none());
                assert_eq!(listed.len(), 3, "{method}");
            }
            assert!(
                entry_of(shell, w.moved, cx)
                    .cloud
                    .is_some_and(|c| !c.web_only),
                "a moved session is not web only"
            );
        })
        .unwrap();
}

#[gpui::test]
fn a_cloud_session_of_an_unselected_project_is_not_listed_or_reachable(cx: &mut TestAppContext) {
    let (window, w) = world(cx);
    window
        .update(cx, |shell, _, cx| {
            let project = project_id(PROJECT);
            let elsewhere = project_id("/work/another-project");
            let only_elsewhere = device(DeviceKind::Web, Some(vec![elsewhere.clone()]));
            let mut reply = catalog(shell, false, cx);
            policy::filter_reply(&only_elsewhere, "workspace.list", &mut reply);
            for hidden in [
                w.codespace,
                w.sandbox,
                w.stopped,
                w.remote,
                w.moved,
                w.local,
            ] {
                assert!(card(&reply, hidden).is_none(), "{hidden} must be filtered");
            }
            assert!(card(&reply, w.other).is_some());
            assert!(
                reply["projects"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .all(|p| p["id"] == elsewhere),
                "the remote project is filtered too"
            );

            let with_project = device(DeviceKind::Web, Some(vec![project.clone()]));
            let mut reply = catalog(shell, false, cx);
            policy::filter_reply(&with_project, "sessions.list", &mut reply);
            for shown in [w.codespace, w.sandbox, w.stopped, w.moved, w.local] {
                assert!(card(&reply, shown).is_some(), "{shown}");
            }
            assert!(
                card(&reply, w.remote).is_none(),
                "its own project is not selected"
            );

            // The same selection guards the app side (H-02).
            let cs = shell.mobile_session_id(w.codespace);
            assert_eq!(
                shell.mobile_target(&cs, &only_elsewhere, cx).err(),
                Some(not_selected())
            );
            assert!(shell.mobile_target(&cs, &with_project, cx).is_ok());
            let remote = shell.mobile_session_id(w.remote);
            assert_eq!(
                shell.mobile_target(&remote, &with_project, cx).err(),
                Some(not_selected())
            );
            let own = device(DeviceKind::Web, Some(vec![project_id(REMOTE_CWD)]));
            assert!(shell.mobile_target(&remote, &own, cx).is_ok());
            // A narrowed phone keeps today's answer: it never reaches a cloud session.
            let narrowed_phone = device(DeviceKind::Phone, Some(vec![project]));
            assert_eq!(
                shell.mobile_target(&cs, &narrowed_phone, cx).err(),
                Some(not_selected())
            );
            let moved = shell.mobile_session_id(w.moved);
            assert!(shell.mobile_target(&moved, &narrowed_phone, cx).is_ok());
        })
        .unwrap();
}

#[gpui::test]
fn reads_and_frames_carry_the_cloud_field_for_web_only_and_never_call_out(cx: &mut TestAppContext) {
    let (window, w) = world(cx);
    window
        .update(cx, |shell, _, cx| {
            let web = device(DeviceKind::Web, None);
            let phone = device(DeviceKind::Phone, None);
            for (id, kind, instance, state) in [
                (w.codespace, "codespace", "cs1", "available"),
                (w.sandbox, "sandbox", "sb1", "available"),
                (w.stopped, "codespace", "cs2", "stopped"),
                (w.moved, "codespace", "cs3", "available"),
                (w.remote, "remote", "box", "unknown"),
            ] {
                for fields in [
                    cloud::read_fields(shell, id, &web, cx),
                    cloud::frame_fields(shell, id, &web, cx),
                ] {
                    let field = &fields["cloud"];
                    assert_eq!(field["kind"], kind);
                    assert_eq!(field["instance"], instance);
                    assert_eq!(field["state"], state);
                    assert!(field.get("startup").is_none());
                }
                assert!(cloud::read_fields(shell, id, &phone, cx).is_empty());
                assert!(cloud::frame_fields(shell, id, &phone, cx).is_empty());
            }
            for local in [w.local, w.other] {
                assert!(cloud::read_fields(shell, local, &web, cx).is_empty());
            }
            // The merged read and frame fields of the host.
            let read = shell.mobile_read_fields(&Target::Session(w.stopped), &web, cx);
            assert_eq!(read["cloud"]["state"], "stopped");
            assert!(
                shell
                    .mobile_read_fields(&Target::Session(w.stopped), &phone, cx)
                    .is_empty()
            );
            // Reading a stopped session, in every way, called nothing.
            let _ = catalog(shell, false, cx);
            assert_eq!(
                w.calls.count(),
                0,
                "a read must never call gh or wake a host"
            );
        })
        .unwrap();
}

#[gpui::test]
fn a_start_in_progress_shows_its_steps_in_milliseconds_and_a_failure_its_clean_error(
    cx: &mut TestAppContext,
) {
    let (window, w) = world(cx);
    window
        .update(cx, |shell, _, cx| {
            let web = device(DeviceKind::Web, None);
            let mut record = crate::cloud_start_view::fixtures::record(2);
            record.session_id = w.codespace;
            record.codespace = "cs1".into();
            monocode_core::cloud_sessions::startup::save_start_record(
                &locked(&shell.store).unwrap(),
                &record,
            )
            .unwrap();
            let read = cloud::read_fields(shell, w.codespace, &web, cx);
            let field = &read["cloud"];
            assert_eq!(field["state"], "starting");
            assert_eq!(field["startup"]["current"], "buildContainer");
            assert_eq!(field["startup"]["steps"][0]["startedAt"], 1000);
            assert_eq!(field["startup"]["steps"][0]["finishedAt"], 6000);
            assert!(field["startup"]["steps"][2]["finishedAt"].is_null());
            let cards = catalog(shell, false, cx);
            let cs = card(&cards, w.codespace).unwrap();
            assert_eq!(cs["cloudState"], "starting");
            assert_eq!(cs["startup"], json!({"step": "buildContainer"}));
            assert_eq!(w.calls.count(), 0);

            record.steps[2].status = StepStatus::Failed;
            record.failed_at = Some(StepKind::BuildContainer);
            record.error = Some(format!(
                "Container build failed in /workspaces/widgets, see https://example.test/log {}",
                "x".repeat(400)
            ));
            monocode_core::cloud_sessions::startup::save_start_record(
                &locked(&shell.store).unwrap(),
                &record,
            )
            .unwrap();
            let cards = catalog(shell, false, cx);
            let cs = card(&cards, w.codespace).unwrap();
            // A failed start shows the cached state again, plus the error.
            assert_eq!(cs["cloudState"], "available");
            let error = cs["startup"]["error"].as_str().unwrap();
            assert!(error.starts_with("Container build failed in see"));
            assert!(error.chars().count() <= 200);
            assert!(!error.contains("/workspaces") && !error.contains("https"));
            let read = cloud::read_fields(shell, w.codespace, &web, cx);
            assert_eq!(read["cloud"]["startup"]["error"], error);
        })
        .unwrap();
}

#[gpui::test]
fn the_watch_sends_the_cloud_field_on_change_only(cx: &mut TestAppContext) {
    let (window, w) = world(cx);
    window
        .update(cx, |shell, _, cx| {
            let web = device(DeviceKind::Web, None);
            let target = Target::Session(w.stopped);
            let none = Value::Null;
            let turns = [json!({"id": "1", "items": []})];
            let extra = shell.mobile_frame_fields(&target, &web, cx);
            let (first, baseline) =
                watch::update(None, false, false, &none, &turns, false, &none, &extra).unwrap();
            assert_eq!(first["cloud"]["state"], "stopped");
            assert!(
                watch::update(
                    Some(&baseline),
                    false,
                    false,
                    &none,
                    &turns,
                    false,
                    &none,
                    &extra
                )
                .is_none(),
                "an unchanged cloud field is not sent again"
            );
            let key = shell.cloud_marks[&w.stopped].key();
            shell.cloud_states.insert(key, CloudState::Starting);
            let extra = shell.mobile_frame_fields(&target, &web, cx);
            let (next, _) = watch::update(
                Some(&baseline),
                false,
                false,
                &none,
                &turns,
                false,
                &none,
                &extra,
            )
            .unwrap();
            assert_eq!(next["cloud"]["state"], "starting");
            // A phone's frame is what it always was.
            let phone_extra =
                shell.mobile_frame_fields(&target, &device(DeviceKind::Phone, None), cx);
            assert_eq!(phone_extra, json!({}));
        })
        .unwrap();
}

#[gpui::test]
fn remote_projects_are_refused_for_web_and_unchanged_for_phones(cx: &mut TestAppContext) {
    let (window, _w) = world(cx);
    window
        .update(cx, |shell, _, cx| {
            let id = project_id(REMOTE_CWD);
            let web = device(DeviceKind::Web, None);
            assert_eq!(
                shell.mobile_project_path(&id, &web, cx),
                Err(policy::Code::UnsupportedTarget.failure())
            );
            // Not on the rail and not this phone's business: today's answer.
            assert_eq!(
                shell.mobile_project_path(&id, &device(DeviceKind::Phone, None), cx),
                Err("Project is no longer available".into())
            );
            assert!(
                shell
                    .mobile_project_path(&project_id(PROJECT), &web, cx)
                    .is_ok()
            );
            // A selection check still comes first.
            let narrowed = device(DeviceKind::Web, Some(vec!["x".into()]));
            assert_eq!(
                shell.mobile_project_path(&id, &narrowed, cx),
                Err(not_selected())
            );
        })
        .unwrap();
}

#[gpui::test]
fn cloud_methods_answer_coded_failures_and_destructive_ones_are_forbidden(cx: &mut TestAppContext) {
    let (window, _w) = world(cx);
    window
        .update(cx, |shell, _, cx| {
            let web = device(DeviceKind::Web, None);
            for method in [
                // Answered through the host (it needs the window).
                "cloud.moveToLocal",
                "cloud.something",
            ] {
                assert_eq!(
                    cloud::dispatch(shell, &web, method, &json!({}), cx),
                    Err(policy::Code::Unsupported.failure()),
                    "{method}"
                );
            }
            for method in [
                "cloud.delete",
                "cloud.create",
                "cloud.rebuild",
                "cloud.pullEvidence",
                "cloud.openInBrowser",
                "cloud.deleteCodespace",
            ] {
                for kind in [DeviceKind::Web, DeviceKind::Phone] {
                    assert_eq!(
                        cloud::dispatch(shell, &device(kind, None), method, &json!({}), cx),
                        Err(policy::Code::Forbidden.failure()),
                        "{method}"
                    );
                }
            }
        })
        .unwrap();
}
