use super::*;
use crate::cloud_sessions::{origin_key, unsaved_key, StepKind};
use crate::store::{pending_cloud_project, RemoteProject};

#[test]
fn codespace_refs_keep_the_names_stored_before_backends_existed() {
    let cloud = CloudRef::codespace("octo-abc");
    assert_eq!(cloud.key(), "octo-abc");
    assert_eq!(cloud.pending_env(), "pending-octo-abc");
    assert_eq!((cloud.noun(), cloud.title()), ("codespace", "Codespace"));
    assert_eq!(unsaved_key(&cloud), "codespace-unsaved:octo-abc");
    assert_eq!(
        origin_key(CloudBackendKind::Codespaces, 7),
        "codespace-origin:7"
    );
}

#[test]
fn sandbox_refs_live_in_their_own_namespace() {
    let cloud = CloudRef::sandbox("octo-abc");
    assert_eq!(cloud.key(), "sbx:octo-abc");
    assert_eq!(cloud.pending_env(), "pending-sbx-octo-abc");
    assert!(crate::store::is_pending_cloud_environment(
        &cloud.pending_env()
    ));
    assert_eq!((cloud.noun(), cloud.title()), ("sandbox", "Sandbox"));
    assert_ne!(cloud, CloudRef::codespace("octo-abc"));
    assert_ne!(
        unsaved_key(&cloud),
        unsaved_key(&CloudRef::codespace("octo-abc"))
    );
}

#[test]
fn the_backend_kind_defaults_to_codespaces_and_serialises_camel_case() {
    assert_eq!(CloudBackendKind::default(), CloudBackendKind::Codespaces);
    assert_eq!(
        serde_json::to_string(&CloudBackendKind::DockerSandbox).unwrap(),
        "\"dockerSandbox\""
    );
    assert_eq!(
        serde_json::from_str::<CloudBackendKind>("\"codespaces\"").unwrap(),
        CloudBackendKind::Codespaces
    );
}

#[test]
fn codespace_step_labels_equal_the_original_ones() {
    for kind in StepKind::ALL {
        assert_eq!(kind.label_for(CloudBackendKind::Codespaces), kind.label());
        assert_eq!(
            kind.running_label_for(CloudBackendKind::Codespaces),
            kind.running_label()
        );
    }
}

#[test]
fn sandbox_step_labels_drop_the_word_codespace() {
    let docker = CloudBackendKind::DockerSandbox;
    assert_eq!(StepKind::Create.label_for(docker), "Create");
    assert_eq!(
        StepKind::Create.running_label_for(docker),
        "Creating sandbox"
    );
    assert_eq!(
        StepKind::ConfigureCodespace.label_for(docker),
        "Configure environment"
    );
    assert_eq!(
        StepKind::ConfigureCodespace.running_label_for(docker),
        "Configuring environment"
    );
    for kind in StepKind::ALL {
        assert!(!kind.label_for(docker).contains("odespace"), "{kind:?}");
        assert!(
            !kind.running_label_for(docker).contains("odespace"),
            "{kind:?}"
        );
    }
}

const OLD_PROJECTS: &str = r#"{"remote://env-a/workspaces/w":{"key":"remote://env-a/workspaces/w","environmentId":"env-a","projectId":"p1","cwd":"/workspaces/w","localProject":"/Users/me/w","codespace":"octo-abc"},"remote://env-b/srv":{"key":"remote://env-b/srv","environmentId":"env-b","projectId":"p2","cwd":"/srv"}}"#;

#[test]
fn old_remote_projects_read_and_write_back_unchanged() {
    let projects: std::collections::BTreeMap<String, RemoteProject> =
        serde_json::from_str(OLD_PROJECTS).unwrap();
    assert_eq!(projects.len(), 2);
    assert_eq!(
        projects["remote://env-a/workspaces/w"].cloud(),
        Some(CloudRef::codespace("octo-abc"))
    );
    assert_eq!(projects["remote://env-b/srv"].cloud(), None);
    assert_eq!(serde_json::to_string(&projects).unwrap(), OLD_PROJECTS);
}

#[test]
fn a_sandbox_project_round_trips_and_is_never_a_codespace() {
    let project =
        pending_cloud_project("/Users/me/w", &CloudRef::sandbox("sbx-1"), "/workspaces/w");
    let json = serde_json::to_string(&project).unwrap();
    assert!(json.contains("\"sandbox\":\"sbx-1\""), "{json}");
    assert!(!json.contains("codespace"), "{json}");
    let back: RemoteProject = serde_json::from_str(&json).unwrap();
    assert_eq!(back.cloud(), Some(CloudRef::sandbox("sbx-1")));
    assert_eq!(back.codespace, None);
    assert_eq!(serde_json::to_string(&back).unwrap(), json);
}

/// A `remote-sessions.v1` value as origin/main writes it, written by hand.
const OLD_SESSIONS: &str = r#"{"3":{"summary":{"id":3,"title":"New claude session","provider":"claude","cwd":"remote://env-a/workspaces/w","provider_id":null,"has_messages":false,"updated_at":1700000000000,"archived":false,"pinned":false,"work_item":null,"surface":"chat"},"project":{"key":"remote://env-a/workspaces/w","environmentId":"env-a","projectId":"p1","cwd":"/workspaces/w","localProject":"/Users/me/w","codespace":"octo-abc"},"session_id":"host-1","outbox":[],"uploads":[],"rejected":null,"configuration":null,"running":false,"needs_input":false}}"#;

#[test]
fn old_remote_sessions_read_and_write_back_byte_identical() {
    let sessions: std::collections::BTreeMap<i64, crate::store::RemoteSession> =
        serde_json::from_str(OLD_SESSIONS).unwrap();
    assert_eq!(
        sessions[&3].project.cloud(),
        Some(CloudRef::codespace("octo-abc"))
    );
    assert_eq!(serde_json::to_string(&sessions).unwrap(), OLD_SESSIONS);
}
