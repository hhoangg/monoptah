//! Session folders for web devices: how the desktop's per-project folders
//! (`session_groups`) appear in the catalog, how they are cut down to what a
//! device may see, and the four control methods (`folder.create`,
//! `folder.rename`, `folder.move`, `folder.delete`) that edit them.
//!
//! Everything here is pure. The app builds the catalog cards, resolves the
//! wire ids of sessions and projects, and applies an [`Op`] to the project's
//! [`Groups`] through the same path the sidebar uses.
//!
//! Folders are web-only: `cloud_policy::scope_catalog` removes them from phone
//! replies, so a phone's catalog is exactly what it always was.
use crate::session_groups::{Folder, Groups};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

/// The most characters of a folder name (the Operator's `folders.move` limit).
pub const NAME_MAX: usize = 100;
/// The most characters of a folder, session or conversation id in a request.
const ID_MAX: usize = 256;
/// The most characters of a project id: it percent-encodes the project's path
/// (3 characters per non-ASCII byte), so a deep or non-Latin path is long.
const PROJECT_ID_MAX: usize = 8192;
/// The most sessions one `folder.create` takes.
pub const CREATE_MAX: usize = 50;

/// The methods this module serves, all control class.
pub const METHODS: [&str; 4] = [
    "folder.create",
    "folder.rename",
    "folder.move",
    "folder.delete",
];

/// A custom folder color the catalog passes on: `#rrggbb`.
fn is_hex_color(color: &str) -> bool {
    color.len() == 7 && color.starts_with('#') && color[1..].chars().all(|c| c.is_ascii_hexdigit())
}

/// The catalog card of one folder. `wire` turns a session id of the store
/// into the id devices know it by; a session without one is left out.
pub fn folder_card(
    project_id: &str,
    folder: &Folder,
    wire: impl Fn(i64) -> Option<String>,
) -> Value {
    let mut card = json!({
        "id": folder.id,
        "projectId": project_id,
        "name": folder.name,
        "sessionIds": folder.session_ids.iter().filter_map(|id| wire(*id)).collect::<Vec<_>>(),
        "collapsed": folder.collapsed,
    });
    if let Some(index) = folder.color_index.filter(|index| *index > 0) {
        card["colorIndex"] = json!(index);
    }
    if let Some(color) = folder.custom_color.as_deref().filter(|c| is_hex_color(c)) {
        card["customColor"] = json!(color);
    }
    card
}

/// The ids of the sessions the catalog's folders hold. The workspace poll
/// keeps these sessions however old they are, so a folder is never shown
/// with members the device has not got.
pub fn foldered_session_ids(catalog: &Value) -> HashSet<String> {
    catalog["folders"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|folder| folder["sessionIds"].as_array().into_iter().flatten())
        .filter_map(|id| id.as_str().map(str::to_owned))
        .collect()
}

/// Cuts the reply's folders to what the device sees: a folder keeps only the
/// sessions of the reply that belong to the folder's own project, and a
/// folder left with none goes (the desktop drops empty folders too). Run
/// after the reply's sessions were narrowed to the device's selection.
pub fn scope(reply: &mut Value) {
    let project_of: HashMap<String, String> = reply["sessions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|session| {
            Some((
                session["id"].as_str()?.to_owned(),
                session["projectId"].as_str()?.to_owned(),
            ))
        })
        .collect();
    let Some(folders) = reply.get_mut("folders").and_then(Value::as_array_mut) else {
        return;
    };
    folders.retain_mut(|folder| {
        let project = folder["projectId"].as_str().unwrap_or("").to_owned();
        let Some(ids) = folder.get_mut("sessionIds").and_then(Value::as_array_mut) else {
            return false;
        };
        ids.retain(|id| {
            id.as_str()
                .and_then(|id| project_of.get(id))
                .is_some_and(|owner| *owner == project)
        });
        !ids.is_empty()
    });
}

/// What a folder request asks for, with the wire ids of its sessions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    Create {
        conversation_ids: Vec<String>,
        name: Option<String>,
    },
    Rename {
        folder_id: String,
        name: String,
    },
    /// `folder_id: None` takes the session out of its folder.
    Move {
        conversation_id: String,
        folder_id: Option<String>,
    },
    Delete {
        folder_id: String,
    },
}

/// A validated folder request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub project_id: String,
    pub kind: Kind,
}

fn id_field(params: &Value, key: &str) -> Result<String, String> {
    let max = if key == "projectId" {
        PROJECT_ID_MAX
    } else {
        ID_MAX
    };
    let value = params[key]
        .as_str()
        .filter(|value| !value.is_empty() && value.chars().count() <= max)
        .ok_or_else(|| format!("Invalid {key}"))?;
    Ok(value.to_owned())
}

/// A folder name: trimmed, 1 to [`NAME_MAX`] characters.
fn name_field(value: &Value) -> Result<String, String> {
    let name = value.as_str().ok_or("Enter a folder name")?.trim();
    if name.is_empty() || name.chars().count() > NAME_MAX {
        return Err(format!("Enter a folder name under {NAME_MAX} characters"));
    }
    Ok(name.to_owned())
}

/// Validates the params of a folder method, before anything is looked up.
pub fn parse(method: &str, params: &Value) -> Result<Request, String> {
    let project_id = id_field(params, "projectId")?;
    let kind = match method {
        "folder.create" => {
            let ids = params["conversationIds"]
                .as_array()
                .filter(|ids| !ids.is_empty() && ids.len() <= CREATE_MAX)
                .ok_or("Choose between 1 and 50 sessions")?;
            let mut conversation_ids: Vec<String> = Vec::new();
            for id in ids {
                let id = id
                    .as_str()
                    .filter(|id| !id.is_empty() && id.chars().count() <= ID_MAX)
                    .ok_or("Invalid conversationIds")?;
                if !conversation_ids.iter().any(|seen| seen == id) {
                    conversation_ids.push(id.to_owned());
                }
            }
            let name = match &params["name"] {
                Value::Null => None,
                value => Some(name_field(value)?),
            };
            Kind::Create {
                conversation_ids,
                name,
            }
        }
        "folder.rename" => Kind::Rename {
            folder_id: id_field(params, "folderId")?,
            name: name_field(&params["name"])?,
        },
        "folder.move" => Kind::Move {
            conversation_id: id_field(params, "conversationId")?,
            folder_id: match &params["folderId"] {
                Value::Null => None,
                _ => Some(id_field(params, "folderId")?),
            },
        },
        "folder.delete" => Kind::Delete {
            folder_id: id_field(params, "folderId")?,
        },
        _ => return Err("Unsupported folder method".into()),
    };
    Ok(Request { project_id, kind })
}

/// A request with its sessions resolved to the store's ids.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    Create {
        sessions: Vec<i64>,
        name: Option<String>,
        /// The id the new folder gets.
        id: String,
    },
    Rename {
        folder: String,
        name: String,
    },
    Move {
        session: i64,
        folder: Option<String>,
    },
    Delete {
        folder: String,
    },
}

/// Applies `op` to the project's folders, the way the sidebar's menus do
/// (`create_folder`, `rename_folder`, `add_to_folder`, `remove_sessions`,
/// `dissolve`), and answers what the device is told. `Err` leaves `groups`
/// as it was.
pub fn apply(groups: &mut Groups, op: Op) -> Result<Value, String> {
    let missing = || "Folder not found".to_owned();
    match op {
        Op::Create { sessions, name, id } => {
            let folder = groups
                .create_folder(&sessions, id)
                .ok_or("Choose a session")?;
            if let Some(name) = name {
                groups.rename_folder(&folder, &name);
            }
            Ok(json!({"folderId": folder}))
        }
        Op::Rename { folder, name } => {
            groups.folder(&folder).ok_or_else(missing)?;
            groups.rename_folder(&folder, &name);
            Ok(json!({"folderId": folder}))
        }
        Op::Move {
            session,
            folder: Some(folder),
        } => {
            groups.folder(&folder).ok_or_else(missing)?;
            groups.add_to_folder(&folder, session);
            Ok(json!({"folderId": folder}))
        }
        Op::Move {
            session,
            folder: None,
        } => {
            groups.remove_sessions(&[session]);
            Ok(json!({"folderId": null}))
        }
        Op::Delete { folder } => {
            groups.folder(&folder).ok_or_else(missing)?;
            groups.dissolve(&folder);
            Ok(json!({"folderId": folder}))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(id: &str, name: &str, sessions: &[i64]) -> Folder {
        Folder {
            id: id.into(),
            name: name.into(),
            session_ids: sessions.to_vec(),
            ..Folder::default()
        }
    }

    fn card(id: &str, project: &str) -> Value {
        json!({"id": id, "projectId": project})
    }

    #[test]
    fn a_folder_card_carries_name_order_members_collapse_and_color() {
        let mut f = folder("f1", "Docs", &[1, 2, 3]);
        f.collapsed = true;
        f.color_index = Some(4);
        let wire = |id: i64| (id != 2).then(|| format!("s{id}"));
        let value = folder_card("p", &f, wire);
        assert_eq!(
            value,
            json!({"id":"f1","projectId":"p","name":"Docs","sessionIds":["s1","s3"],"collapsed":true,"colorIndex":4})
        );
        // The default wash and a bad custom color are not passed on.
        f.color_index = Some(0);
        f.custom_color = Some("red; background:url(x)".into());
        let value = folder_card("p", &f, |id| Some(id.to_string()));
        assert!(value.get("colorIndex").is_none() && value.get("customColor").is_none());
        f.custom_color = Some("#a1B2c3".into());
        assert_eq!(
            folder_card("p", &f, |id| Some(id.to_string()))["customColor"],
            "#a1B2c3"
        );
    }

    #[test]
    fn scope_keeps_only_visible_sessions_of_the_folders_own_project() {
        let mut reply = json!({
            "sessions": [card("a", "p"), card("b", "p"), card("c", "q")],
            "folders": [
                {"id":"f1","projectId":"p","name":"One","sessionIds":["a","hidden","c"]},
                {"id":"f2","projectId":"p","name":"Gone","sessionIds":["hidden"]},
                {"id":"f3","projectId":"q","name":"Other","sessionIds":["c"]},
                {"id":"f4","projectId":"p","name":"Order","sessionIds":["b","a"]},
            ],
        });
        scope(&mut reply);
        let folders = reply["folders"].as_array().unwrap();
        let ids: Vec<_> = folders.iter().map(|f| f["id"].as_str().unwrap()).collect();
        assert_eq!(ids, ["f1", "f3", "f4"], "empty folders go, order is kept");
        assert_eq!(folders[0]["sessionIds"], json!(["a"]));
        assert_eq!(folders[2]["sessionIds"], json!(["b", "a"]));
        // A reply with no folders is left alone.
        let mut plain = json!({"sessions": []});
        scope(&mut plain);
        assert!(plain.get("folders").is_none());
    }

    #[test]
    fn foldered_session_ids_lists_every_member() {
        let catalog = json!({"folders":[
            {"sessionIds":["a","b"]}, {"sessionIds":["b","c"]}
        ]});
        let ids = foldered_session_ids(&catalog);
        assert_eq!(ids.len(), 3);
        assert!(foldered_session_ids(&json!({})).is_empty());
    }

    #[test]
    fn parse_validates_every_method() {
        let p = |method: &str, params: Value| parse(method, &params);
        assert_eq!(
            p(
                "folder.create",
                json!({"projectId":"p","conversationIds":["a","a","b"],"name":"  Docs "})
            ),
            Ok(Request {
                project_id: "p".into(),
                kind: Kind::Create {
                    conversation_ids: vec!["a".into(), "b".into()],
                    name: Some("Docs".into())
                }
            })
        );
        assert_eq!(
            p(
                "folder.move",
                json!({"projectId":"p","conversationId":"a","folderId":null})
            )
            .unwrap()
            .kind,
            Kind::Move {
                conversation_id: "a".into(),
                folder_id: None
            }
        );
        let long = "x".repeat(NAME_MAX + 1);
        let many: Vec<String> = (0..=CREATE_MAX).map(|i| i.to_string()).collect();
        for (method, params) in [
            (
                "folder.create",
                json!({"projectId":"p","conversationIds":[]}),
            ),
            (
                "folder.create",
                json!({"projectId":"p","conversationIds":many}),
            ),
            (
                "folder.create",
                json!({"projectId":"p","conversationIds":[""]}),
            ),
            (
                "folder.create",
                json!({"projectId":"p","conversationIds":[1]}),
            ),
            (
                "folder.create",
                json!({"projectId":"p","conversationIds":["a"],"name":"  "}),
            ),
            ("folder.create", json!({"conversationIds":["a"]})),
            (
                "folder.rename",
                json!({"projectId":"p","folderId":"f","name":long}),
            ),
            (
                "folder.rename",
                json!({"projectId":"p","folderId":"f","name":" "}),
            ),
            ("folder.rename", json!({"projectId":"p","name":"A"})),
            ("folder.move", json!({"projectId":"p","folderId":"f"})),
            (
                "folder.move",
                json!({"projectId":"p","conversationId":"a","folderId":7}),
            ),
            (
                "folder.delete",
                json!({"projectId":"p","folderId":"x".repeat(ID_MAX + 1)}),
            ),
            ("folder.delete", json!({"projectId":"","folderId":"f"})),
            ("folder.other", json!({"projectId":"p"})),
        ] {
            assert!(p(method, params.clone()).is_err(), "{method} {params}");
        }
        // A project id percent-encodes its path: a deep Vietnamese path is long.
        let long = "%E1%BB%87".repeat(100);
        assert!(long.len() > ID_MAX);
        assert!(p("folder.delete", json!({"projectId": long, "folderId": "f"})).is_ok());
        let huge = "x".repeat(PROJECT_ID_MAX + 1);
        assert!(p("folder.delete", json!({"projectId": huge, "folderId": "f"})).is_err());
        for method in METHODS {
            assert!(super::super::policy::entry(method).is_some(), "{method}");
        }
    }

    #[test]
    fn apply_follows_the_sidebar_semantics() {
        let mut groups = Groups::default();
        let create = |sessions: Vec<i64>, name: Option<&str>, id: &str| Op::Create {
            sessions,
            name: name.map(Into::into),
            id: id.into(),
        };
        apply(&mut groups, create(vec![1, 2], Some("Docs"), "f1")).unwrap();
        // A new folder goes first and takes sessions from their old folders.
        apply(&mut groups, create(vec![2, 3], None, "f2")).unwrap();
        assert_eq!(groups.folders[0].id, "f2");
        assert_eq!(groups.folders[0].name, "New folder");
        assert_eq!(groups.folder("f1").unwrap().session_ids, [1]);
        assert!(apply(&mut groups, create(vec![], None, "f3")).is_err());

        apply(
            &mut groups,
            Op::Rename {
                folder: "f1".into(),
                name: "Specs".into(),
            },
        )
        .unwrap();
        assert_eq!(groups.folder("f1").unwrap().name, "Specs");

        // Moving into a folder opens it and empties-and-drops the old one.
        groups.toggle_folder("f2");
        apply(
            &mut groups,
            Op::Move {
                session: 1,
                folder: Some("f2".into()),
            },
        )
        .unwrap();
        assert!(groups.folder("f1").is_none());
        assert_eq!(groups.folder("f2").unwrap().session_ids, [2, 3, 1]);
        assert!(!groups.folder("f2").unwrap().collapsed);

        apply(
            &mut groups,
            Op::Move {
                session: 3,
                folder: None,
            },
        )
        .unwrap();
        assert_eq!(groups.folder("f2").unwrap().session_ids, [2, 1]);

        // Unknown folders are refused and change nothing.
        let before = groups.clone();
        for op in [
            Op::Rename {
                folder: "nope".into(),
                name: "A".into(),
            },
            Op::Move {
                session: 1,
                folder: Some("nope".into()),
            },
            Op::Delete {
                folder: "nope".into(),
            },
        ] {
            assert_eq!(apply(&mut groups, op), Err("Folder not found".into()));
        }
        assert_eq!(groups, before);

        apply(
            &mut groups,
            Op::Delete {
                folder: "f2".into(),
            },
        )
        .unwrap();
        assert!(groups.folders.is_empty());
    }
}
