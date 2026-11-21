//! The policy of `file.prepare`: which file a remote device may open through an encrypted
//! object (see [`crate::mobile::file_transfer`]), and what that file turns into.
//!
//! Unlike `chat.mentionedFile` this needs no mention and no tool evidence:
//!
//! - a **Control** device may open any file this desktop user can read, inside or outside the
//!   shared projects (`/tmp`, `~/Documents`), because it can already make the agent read any
//!   such file; an open outside the shared projects is audited (path, never content);
//! - a **Read** device stays inside the projects it was given;
//! - for every device the same refusals hold: the deny-list (the written path, the canonical
//!   target and every component of the absolute path, case-insensitively), the app's data
//!   folders and the credential folders under the home folder, a hard link to a credential
//!   file, symlinks resolved before every check, no directory or device, and the kernel's
//!   virtual folders (`/proc`, `/sys`, `/dev`), whose "files" can leak the environment.
//!
//! An asset (an image an html or markdown page shows) is reached only through its page, by the
//! rules of `chat.mentionedFile` (rule 6): listed by the page, inside its root, an image.
use super::{
    authorize_parts, file_name, html, kind_by_extension, looks_like_text, lossy, mime_of,
    not_regular_or_found, open_regular, read_at, references, resolve_asset, rule, Context, Env,
    Guard, Kind, Refusal, MAX_PATH_CHARS, SNIFF_BYTES,
};
use crate::mobile::{
    file_transfer::{crypto::MAX_PLAINTEXT_BYTES, Source},
    policy::{Code, Failure, Permission},
};
use serde_json::Value;
use std::path::Path;

/// What the device asked for: a file, or an asset of an html or markdown file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Params {
    pub path: String,
    pub asset: Option<String>,
    /// For an html or markdown file: its raw text instead of the sanitized document or the render, for a jump to
    /// a line.
    pub source: bool,
}

impl Params {
    pub fn from_json(params: &Value) -> Result<Self, Failure> {
        let invalid = || Failure::from("Invalid file request");
        let text = |value: &Value| -> Result<Option<String>, Failure> {
            match value {
                Value::Null => Ok(None),
                Value::String(s)
                    if !s.is_empty()
                        && s.chars().count() <= MAX_PATH_CHARS
                        && !s.contains('\0') =>
                {
                    Ok(Some(s.clone()))
                }
                _ => Err(invalid()),
            }
        };
        let source = match &params["source"] {
            Value::Null => false,
            Value::Bool(flag) => *flag,
            _ => return Err(invalid()),
        };
        Ok(Self {
            path: text(&params["path"])?.ok_or_else(invalid)?,
            asset: text(&params["asset"])?,
            source,
        })
    }
}

/// A file ready to publish.
pub struct Prepared {
    pub path: String,
    pub name: String,
    pub kind: Kind,
    pub mime: &'static str,
    /// The images an html or markdown page lists, as keys for an asset request.
    pub assets: Option<Vec<String>>,
    pub source: Source,
    /// A Control device opened a file outside the shared projects (an audited event).
    pub outside_projects: bool,
    /// The selected project holding the file and the path inside it.
    pub project: Option<(String, String)>,
}

/// Whether `path` lies in a virtual folder of the kernel: never a file to hand out.
fn kernel_virtual(path: &Path) -> bool {
    cfg!(unix)
        && ["/proc", "/sys", "/dev"]
            .iter()
            .any(|dir| path.starts_with(dir))
}

/// Whether `canonical` is inside what a Read device was given: a selected project, or the
/// session's own worktree of one.
fn within_shared(ctx: &Context, canonical: &Path) -> bool {
    super::in_selected_project(ctx, canonical)
        || ctx
            .worktree_owner()
            .is_some_and(|(tree, owner)| owner.selected && canonical.starts_with(&tree))
}

/// The canonical path `requested` names, when `permission` may open it. Nothing about a file
/// outside the device's reach is told apart: a Read device gets `forbidden` for it, existing
/// or not.
fn authorize(
    ctx: &Context,
    guard: &Guard,
    permission: Permission,
    requested: &str,
) -> Result<(std::path::PathBuf, bool), Refusal> {
    let written = super::literal(ctx, requested)
        .ok_or_else(|| Refusal::new(super::forbidden(), rule::BAD_REQUEST))?;
    if kernel_virtual(&written) {
        return Err(Refusal::new(super::denied(), rule::PROTECTED_DIR));
    }
    if let Some(by) = guard.denial(&written) {
        return Err(Refusal::new(super::denied(), by));
    }
    let Ok(canonical) = written.canonicalize() else {
        return Err(if permission == Permission::Control {
            Refusal::new(super::not_found(), rule::NOT_FOUND)
        } else {
            Refusal::new(super::forbidden(), rule::NOT_IN_PROJECT)
        });
    };
    if kernel_virtual(&canonical) {
        return Err(Refusal::new(super::denied(), rule::PROTECTED_DIR));
    }
    let shared = within_shared(ctx, &canonical);
    if permission == Permission::Read && !shared {
        return Err(Refusal::new(super::forbidden(), rule::NOT_IN_PROJECT));
    }
    authorize_parts(ctx, guard, &canonical)?;
    Ok((canonical, shared))
}

/// Judges the request and opens what it names.
pub fn resolve(
    device: &str,
    permission: Permission,
    params: &Params,
    ctx: &Context,
) -> Result<Prepared, Refusal> {
    let guard = Guard::new(ctx);
    let (canonical, shared) = authorize(ctx, &guard, permission, &params.path)?;
    let outside_projects = permission == Permission::Control && !shared;
    let env = Env {
        device,
        ctx,
        guard: &guard,
        page: ctx.page_bytes,
    };
    let (mut file, metadata) = open_regular(&canonical).map_err(not_regular_or_found)?;
    // The same scope as the permission check: a file of the session's worktree is inside the shared projects too.
    if guard.hard_link_denied(&metadata, shared) {
        return Err(Refusal::new(super::denied(), rule::HARD_LINK));
    }
    let project = super::project_of(ctx, &canonical);
    if let Some(asset) = &params.asset {
        if !matches!(
            kind_by_extension(&file_name(&canonical)),
            Some(Kind::Html | Kind::Markdown)
        ) {
            return Err(Refusal::new(super::forbidden(), rule::ASSET));
        }
        let (image, image_meta, image_path, mime) =
            resolve_asset(&env, &mut file, &canonical, &metadata, asset)?;
        if image_meta.len() > MAX_PLAINTEXT_BYTES {
            return Err(Code::TooLarge.failure().into());
        }
        return Ok(Prepared {
            path: lossy(&image_path),
            name: file_name(&image_path),
            kind: Kind::Image,
            mime,
            assets: None,
            source: Source::File(image),
            outside_projects,
            project: super::project_of(ctx, &image_path),
        });
    }
    let size = metadata.len();
    let name = file_name(&canonical);
    let kind = match kind_by_extension(&name) {
        Some(kind) => kind,
        None => {
            let head = read_at(&mut file, 0, SNIFF_BYTES)?;
            if looks_like_text(&head, (head.len() as u64) >= size) {
                Kind::Text
            } else {
                Kind::Binary
            }
        }
    };
    if size > MAX_PLAINTEXT_BYTES {
        return Err(Code::TooLarge.failure().into());
    }
    // The raw text of a page that would be rendered, for a jump to a line.
    let kind = match kind {
        Kind::Html | Kind::Markdown if params.source => Kind::Text,
        other => other,
    };
    let mime = match kind {
        Kind::Image => html::extension(&name)
            .and_then(|e| mime_of(&e))
            .unwrap_or("application/octet-stream"),
        Kind::Html => "text/html",
        Kind::Markdown => "text/markdown",
        Kind::Text => "text/plain",
        Kind::Binary => "application/octet-stream",
    };
    let (assets, source) = match kind {
        Kind::Html => {
            let doc = super::html_document(&env, &mut file, &canonical, &metadata)?;
            (
                Some(doc.assets.clone()),
                Source::Bytes(doc.html.clone().into_bytes()),
            )
        }
        Kind::Markdown => {
            let doc = references(&env, &mut file, &canonical, &metadata)?;
            (Some(doc.assets.clone()), Source::File(file))
        }
        _ => (None, Source::File(file)),
    };
    Ok(Prepared {
        path: lossy(&canonical),
        name,
        kind,
        mime,
        assets,
        source,
        outside_projects,
        project,
    })
}

/// One `file.prepare` call, answered: the result and what its audit line needs.
pub struct TransferCall {
    pub result: Result<Value, Failure>,
    /// The rule that refused the call, when it was refused.
    pub rule: Option<&'static str>,
    /// The canonical path opened, or the path as requested when refused.
    pub path: String,
    pub asset: Option<String>,
    /// Size of the file (not of what was uploaded).
    pub bytes: u64,
    pub outside_projects: bool,
    /// The server already held the object, nothing was uploaded.
    pub reused: bool,
}

impl TransferCall {
    pub fn outcome(&self) -> crate::mobile::audit::Outcome {
        match &self.result {
            Ok(_) => crate::mobile::audit::Outcome::Ok,
            Err(failure) => crate::mobile::policy::code_of_failure(failure).map_or(
                crate::mobile::audit::Outcome::Error,
                crate::mobile::audit::Outcome::Denied,
            ),
        }
    }

    fn refused(params: &Value, failure: Failure, rule: &'static str) -> Self {
        Self {
            result: Err(failure),
            rule: Some(rule),
            path: params["path"].as_str().unwrap_or_default().to_owned(),
            asset: params["asset"].as_str().map(str::to_owned),
            bytes: 0,
            outside_projects: false,
            reused: false,
        }
    }
}

/// The whole method for `device`: the budget, the parameters, the policy, the content hash and the
/// upload when the server lacks the object. The app calls it on the background executor, with
/// the context it built from the conversation (folder, roots, projects); the headless E2E
/// desktop calls it too.
pub fn handle(
    device: &crate::mobile::policy::DeviceContext,
    params: &Value,
    ctx: &Context,
    store: &dyn crate::mobile::file_transfer::ObjectStore,
    keys: &crate::mobile::file_transfer::keystore::KeyStore,
) -> TransferCall {
    if super::budget::admit(&device.id).is_err() {
        return TransferCall::refused(params, super::budget::busy(&device.id, 0), rule::BUDGET);
    }
    let parsed = match Params::from_json(params) {
        Ok(parsed) => parsed,
        Err(failure) => return TransferCall::refused(params, failure, rule::BAD_REQUEST),
    };
    let mut prepared = match resolve(&device.id, device.permission, &parsed, ctx) {
        Ok(prepared) => prepared,
        Err(refusal) => return TransferCall::refused(params, refusal.failure, refusal.rule),
    };
    // The bytes this call may upload count against the device's window before it starts, so parallel calls cannot
    // together pass it; a reuse or a failure gives them back.
    let reserved = prepared.source.len_hint();
    if super::budget::reserve(&device.id, reserved).is_err() {
        return TransferCall::refused(
            params,
            super::budget::busy(&device.id, reserved),
            rule::BUDGET,
        );
    }
    let published = crate::mobile::file_transfer::publish(&mut prepared.source, store, keys);
    if !matches!(&published, Ok(done) if !done.reused) {
        super::budget::refund(&device.id, reserved);
    }
    let call = |result, bytes, reused| TransferCall {
        result,
        rule: None,
        path: prepared.path.clone(),
        asset: parsed.asset.clone(),
        bytes,
        outside_projects: prepared.outside_projects,
        reused,
    };
    match published {
        Ok(published) => {
            let mut reply = published.to_json();
            reply["path"] = Value::String(prepared.path.clone());
            reply["name"] = Value::String(prepared.name.clone());
            reply["kind"] = Value::String(prepared.kind.as_str().to_owned());
            reply["mime"] = Value::String(prepared.mime.to_owned());
            if let Some(assets) = &prepared.assets {
                reply["assets"] = serde_json::json!(assets);
            }
            if let Some((project_id, path)) = &prepared.project {
                reply["project"] = serde_json::json!({"projectId": project_id, "path": path});
            }
            call(Ok(reply), published.size, published.reused)
        }
        Err(failure) => {
            let mut failed = call(Err(failure), 0, false);
            failed.rule = Some(rule::FAILED);
            failed
        }
    }
}

#[cfg(test)]
mod tests;
