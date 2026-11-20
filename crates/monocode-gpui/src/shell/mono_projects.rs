//! `projects.add` and `projects.list`: a Mono adds a folder to the app's projects, or lists them.
//!
//! The add goes through `Shell::add_project_path`, the same function the user's Open project
//! uses, so the rail, the saved workspace and everything read from them (git state, the
//! worktrees folder) behave the same. What differs is only what a Mono must not do: the folder
//! is validated first (`monocode_core::project_add`), the project is not selected, nothing is
//! shared with a web device, and the user is told who added it.
use super::*;
use monocode_core::{files::path_key, mobile::audit, project_add};
use serde_json::{Value, json};
use std::path::Path;

/// What the user reads when a Mono adds a project.
pub(crate) fn added_text(project: &str, mono: &str) -> String {
    format!("Project {project} added by Mono {mono}")
}

/// Whether the folder is in a git work tree (a `.git` folder or file in it or above it).
fn is_git(path: &str) -> bool {
    !path.starts_with("remote://")
        && Path::new(path)
            .ancestors()
            .any(|dir| dir.join(".git").exists())
}

impl Shell {
    /// Adds `path` to the rail the way Open project does and saves the workspace. `name`, when
    /// given, becomes the project's display name. Does not select the project.
    pub(super) fn add_project_path(
        &mut self,
        path: &str,
        name: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        self.workspace.add_project(path);
        match name {
            Some(name) => {
                let mut appearance = self.appearance(path);
                appearance.name = name.to_owned();
                self.save_project_appearance(path.to_owned(), appearance, cx);
            }
            None => {
                self.invalidate_chrome(cx);
                self.persist(cx);
                cx.notify();
            }
        }
    }

    fn project_row(&self, path: &str) -> Value {
        json!({"id": path, "name": self.project_label(path), "path": path, "isGit": is_git(path)})
    }

    pub(crate) fn mono_projects_call(
        &mut self,
        mono: &str,
        action: &str,
        input: &Value,
        cx: &mut Context<Self>,
    ) -> Result<Value, String> {
        match action {
            "projects.list" => Ok(Value::Array(
                self.workspace
                    .projects
                    .iter()
                    .map(|p| self.project_row(p))
                    .collect(),
            )),
            "projects.add" => {
                let raw = input["path"].as_str().unwrap_or_default();
                let result = self.mono_add_project(mono, raw, input, cx);
                let outcome = match &result {
                    Ok(row) if row["alreadyExisted"] == true => "ok:alreadyExisted".to_owned(),
                    Ok(_) => "ok".to_owned(),
                    Err(error) => format!("refused: {error}"),
                };
                let path = result
                    .as_ref()
                    .ok()
                    .and_then(|row| row["path"].as_str())
                    .unwrap_or(raw);
                audit::record_app_action("projects.add", mono, path, &outcome);
                result
            }
            _ => Err("Unknown action. Run app help to see supported actions".into()),
        }
    }

    fn mono_add_project(
        &mut self,
        mono: &str,
        raw: &str,
        input: &Value,
        cx: &mut Context<Self>,
    ) -> Result<Value, String> {
        let name = match input.get("name") {
            Some(name) => Some(project_add::check_name(
                name.as_str().ok_or("name must be text")?,
            )?),
            None => None,
        };
        let home = project_add::home_dir();
        let canonical = project_add::resolve(raw, home.as_deref())?;
        let canonical = canonical.to_string_lossy().into_owned();
        // A stored project may have been added under a symlinked or otherwise non-canonical
        // path: match it by its own spelling, its resolved path and the raw input.
        let mut keys = vec![path_key(&canonical)];
        if let Ok(expanded) = project_add::expand_home(raw, home.as_deref()) {
            keys.push(path_key(&expanded.to_string_lossy()));
        }
        let known = self
            .workspace
            .projects
            .iter()
            .find(|p| {
                keys.contains(&path_key(p))
                    || (!p.starts_with("remote://")
                        && Path::new(p.as_str())
                            .canonicalize()
                            .is_ok_and(|real| keys[0] == path_key(&real.to_string_lossy())))
            })
            .cloned();
        if let Some(known) = known {
            let mut row = self.project_row(&known);
            row["alreadyExisted"] = true.into();
            return Ok(row);
        }
        self.add_project_path(&canonical, name.as_deref(), cx);
        let mut row = self.project_row(&canonical);
        row["alreadyExisted"] = false.into();
        let text = added_text(row["name"].as_str().unwrap_or_default(), mono);
        if !cx.is_test() && self.workspace.preferences.notifications {
            let _ = crate::notifications::platform::show(
                crate::notifications::platform::Notification {
                    tag: format!("monocode-project-added:{}", monocode_core::time::now_ms()),
                    subtitle: "Project added".into(),
                    body: crate::notifications::delivery::clip_body(&text),
                    sound: self.workspace.preferences.sounds,
                },
            );
        }
        Ok(row)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shell::tests::window;
    use gpui::{TestAppContext, WindowHandle};

    fn scratch() -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("native-projects-{}", monocode_core::mono::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn call(
        window: WindowHandle<Shell>,
        cx: &mut TestAppContext,
        action: &str,
        input: Value,
    ) -> Result<Value, String> {
        window
            .update(cx, |shell, _, cx| {
                shell.mono_projects_call("Ada", action, &input, cx)
            })
            .unwrap()
    }

    #[test]
    fn the_notice_names_the_project_and_the_mono() {
        assert_eq!(added_text("api", "Ada"), "Project api added by Mono Ada");
    }

    #[test]
    fn git_is_found_in_the_folder_or_above_it() {
        let dir = scratch();
        let inner = dir.join("a/b");
        std::fs::create_dir_all(&inner).unwrap();
        std::fs::create_dir(dir.join(".git")).unwrap();
        assert!(is_git(inner.to_str().unwrap()));
        assert!(!is_git("remote://host/work"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[gpui::test]
    #[cfg_attr(
        windows,
        ignore = "temp dir lives under AppData, which projects.add refuses"
    )]
    fn add_registers_the_project_once_and_does_not_select_it(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        let dir = scratch();
        let project = dir.join("fresh");
        std::fs::create_dir_all(project.join(".git")).unwrap();
        let canonical = project
            .canonicalize()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let before = window
            .update(cx, |shell, _, _| shell.workspace.project.clone())
            .unwrap();
        let first = call(
            window,
            cx,
            "projects.add",
            json!({"path": project.to_str().unwrap(), "name": "Fresh one"}),
        )
        .unwrap();
        assert_eq!(
            first,
            json!({"id": canonical, "name": "Fresh one", "path": canonical, "isGit": true, "alreadyExisted": false})
        );
        let second = call(
            window,
            cx,
            "projects.add",
            json!({"path": project.to_str().unwrap()}),
        )
        .unwrap();
        assert_eq!(second["alreadyExisted"], true);
        assert_eq!(second["name"], "Fresh one");
        let (count, selected) = window
            .update(cx, |shell, _, _| {
                (
                    shell
                        .workspace
                        .projects
                        .iter()
                        .filter(|p| **p == canonical)
                        .count(),
                    shell.workspace.project.clone(),
                )
            })
            .unwrap();
        assert_eq!(count, 1);
        assert_eq!(selected, before);
        let listed = call(window, cx, "projects.list", json!({})).unwrap();
        assert!(
            listed
                .as_array()
                .unwrap()
                .iter()
                .any(|row| row["path"] == canonical && row["isGit"] == true)
        );
    }

    #[cfg(unix)]
    #[gpui::test]
    #[cfg_attr(
        windows,
        ignore = "temp dir lives under AppData, which projects.add refuses"
    )]
    fn a_project_stored_under_a_symlink_is_found_by_its_real_path(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        let dir = scratch();
        let real = dir.join("real");
        std::fs::create_dir_all(&real).unwrap();
        let link = dir.join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let link_text = link.to_string_lossy().into_owned();
        window
            .update(cx, |shell, _, cx| {
                shell.add_project_path(&link_text, None, cx)
            })
            .unwrap();
        let before = window
            .update(cx, |shell, _, _| shell.workspace.projects.len())
            .unwrap();
        let row = call(
            window,
            cx,
            "projects.add",
            json!({"path": real.to_str().unwrap()}),
        )
        .unwrap();
        assert_eq!(row["alreadyExisted"], true);
        assert_eq!(row["path"], link_text);
        let after = window
            .update(cx, |shell, _, _| shell.workspace.projects.len())
            .unwrap();
        assert_eq!(before, after);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[gpui::test]
    fn add_refuses_bad_input_without_changing_the_rail(cx: &mut TestAppContext) {
        let (window, _, _) = window(cx);
        let count = |window: WindowHandle<Shell>, cx: &mut TestAppContext| {
            window
                .update(cx, |shell, _, _| shell.workspace.projects.len())
                .unwrap()
        };
        let before = count(window, cx);
        for input in [
            json!({"path": "relative/dir"}),
            json!({"path": "/definitely/not/here"}),
            json!({"path": "/"}),
            json!({"path": std::env::temp_dir().to_str().unwrap(), "name": ""}),
        ] {
            assert!(
                call(window, cx, "projects.add", input.clone()).is_err(),
                "{input}"
            );
        }
        assert_eq!(count(window, cx), before);
    }
}
