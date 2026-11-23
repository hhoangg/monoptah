use super::{Download, Installation, Installed, PackageKind, Update};
#[cfg(any(target_os = "macos", test))]
use std::path::Component;
#[cfg(any(target_os = "macos", target_os = "linux", test))]
use std::path::Path;
use std::{fs, process::Command};

pub(super) fn install(
    update: &Update,
    download: Download,
    installation: &Installation,
) -> Result<Installed, String> {
    match installation.kind {
        #[cfg(target_os = "macos")]
        PackageKind::App => install_mac(update, &download.path, &installation.path),
        #[cfg(target_os = "linux")]
        PackageKind::Appimage => install_appimage(&download.path, &installation.path),
        #[cfg(target_os = "linux")]
        PackageKind::Deb | PackageKind::Rpm => {
            let extension = installation.kind.suffix();
            let package = download
                ._directory
                .path()
                .join(format!("update.{extension}"));
            fs::rename(&download.path, &package).map_err(|e| e.to_string())?;
            let (command, argument) = if installation.kind == PackageKind::Deb {
                ("/usr/bin/dpkg", "-i")
            } else {
                ("/usr/bin/rpm", "-U")
            };
            success(
                Command::new("pkexec")
                    .arg(command)
                    .arg(argument)
                    .arg("--")
                    .arg(package),
                "The package update was cancelled or failed",
            )?;
            Ok(Installed::Relaunch(installation.path.clone()))
        }
        #[cfg(windows)]
        PackageKind::Nsis => {
            use std::os::windows::process::CommandExt;
            let installer = download._directory.path().join("update.exe");
            fs::rename(&download.path, &installer).map_err(|e| e.to_string())?;
            let directory = installation
                .path
                .parent()
                .ok_or("Cannot locate the installation")?;
            // NSIS requires /D to be last and unquoted, including paths with spaces.
            Command::new(&installer)
                .arg("/S")
                .arg(format!("/UPDATEPID={}", std::process::id()))
                .raw_arg(format!("/D={}", directory.display()))
                .spawn()
                .map_err(|e| e.to_string())?;
            // The installer waits for our normal quit hooks, replaces the app and
            // launches it. Keep its executable alive after the old app exits.
            let _ = download._directory.keep();
            Ok(Installed::InstallerStarted)
        }
        _ => {
            let _ = (update, download);
            Err("This installation cannot install native updates".into())
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn success(command: &mut Command, message: &str) -> Result<(), String> {
    let output = command.output().map_err(|e| format!("{message}: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{message}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

/// Prepare first, then swap on the same filesystem, retaining the original if
/// the second rename fails. A failed rollback leaves its backup for recovery.
#[cfg(any(target_os = "macos", target_os = "linux", test))]
fn replace(prepared: &Path, target: &Path, backup: &Path) -> Result<(), String> {
    fs::rename(target, backup).map_err(|e| e.to_string())?;
    if let Err(error) = fs::rename(prepared, target) {
        return match fs::rename(backup, target) {
            Ok(()) => Err(format!(
                "Could not install update; the previous version was restored: {error}"
            )),
            Err(rollback) => Err(format!(
                "Could not install update: {error}. The previous version is at {}: {rollback}",
                backup.display()
            )),
        };
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn install_appimage(package: &Path, target: &Path) -> Result<Installed, String> {
    use std::io::Read;
    let mut header = [0u8; 11];
    fs::File::open(package)
        .and_then(|mut f| f.read_exact(&mut header))
        .map_err(|e| e.to_string())?;
    if &header[..4] != b"\x7fELF" || &header[8..11] != b"AI\x02" {
        return Err("The update is not a type-2 AppImage".into());
    }
    let parent = target
        .parent()
        .ok_or("Cannot locate the AppImage directory")?;
    let stage = tempfile::Builder::new()
        .prefix(".monocode-update-")
        .tempdir_in(parent)
        .map_err(|e| e.to_string())?;
    let prepared = stage.path().join("new.AppImage");
    fs::copy(package, &prepared).map_err(|e| e.to_string())?;
    fs::set_permissions(
        &prepared,
        fs::metadata(target)
            .map_err(|e| e.to_string())?
            .permissions(),
    )
    .map_err(|e| e.to_string())?;
    let backup = stage.path().join("previous.AppImage");
    let result = replace(&prepared, target, &backup);
    if result.is_err() && backup.exists() {
        let _ = stage.keep();
    }
    result?;
    Ok(Installed::Relaunch(target.to_path_buf()))
}

#[cfg(any(target_os = "macos", test))]
fn extract_app(package: &Path, directory: &Path) -> Result<(), String> {
    let file = fs::File::open(package).map_err(|e| e.to_string())?;
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(file));
    let mut expanded = 0u64;
    for entry in archive.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path().map_err(|e| e.to_string())?.into_owned();
        let mut components = path.components();
        if components.next() != Some(Component::Normal("Monoptah Native.app".as_ref()))
            || components.any(|c| !matches!(c, Component::Normal(_)))
        {
            return Err("The update archive contains an invalid app path".into());
        }
        let kind = entry.header().entry_type();
        // Native bundles currently contain only files and directories. Refuse
        // archive links/devices rather than allowing writes outside the stage.
        if !kind.is_file() && !kind.is_dir() {
            return Err("The update archive contains an unsupported entry".into());
        }
        expanded = expanded.saturating_add(entry.size());
        if expanded > 2 * 1024 * 1024 * 1024 {
            return Err("Expanded update exceeds its size limit".into());
        }
        if !entry.unpack_in(directory).map_err(|e| e.to_string())? {
            return Err("The update archive escaped its staging directory".into());
        }
    }
    Ok(())
}

/// Fork-local. SHA-1 of the stable self-signed certificate that signs Monoptah
/// Native releases. Compiled in so an update must be signed by the same identity.
#[cfg(any(target_os = "macos", test))]
const PINNED_LEAF_CERTIFICATE_SHA1: &str = "0a44567bf676438c0dad7e5b3ec5ef80708bebfe";

/// Fork-local. The code requirement every release must satisfy.
#[cfg(any(target_os = "macos", test))]
fn pinned_requirement() -> String {
    format!(
        "identifier \"{}\" and certificate leaf = H\"{PINNED_LEAF_CERTIFICATE_SHA1}\"",
        super::APP_ID
    )
}

/// Fork-local. How an app bundle is signed, read from `codesign -dv` output.
#[cfg(any(target_os = "macos", test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SignatureKind {
    AdHoc,
    /// Signed with a certificate (self-signed or Developer ID).
    Signed,
    Unknown,
}

#[cfg(any(target_os = "macos", test))]
#[derive(Debug, PartialEq, Eq)]
enum Acceptance {
    Accept,
    Reject(&'static str),
    /// No identity rule applies; fall back to the Gatekeeper assessment.
    Gatekeeper,
}

/// Fork-local. Reads `codesign -dv` output (written to stderr). Anything that is
/// not clearly ad-hoc or certificate-signed is `Unknown`.
#[cfg(any(target_os = "macos", test))]
fn signature_kind(output: &str) -> SignatureKind {
    if output.lines().any(|line| line.trim() == "Signature=adhoc") {
        SignatureKind::AdHoc
    } else if output
        .lines()
        .any(|line| line.starts_with("Authority=") || line.starts_with("Signature size="))
    {
        SignatureKind::Signed
    } else {
        SignatureKind::Unknown
    }
}

/// Fork-local. Reads the `designated => ...` line of `codesign -d -r-` output.
#[cfg(any(target_os = "macos", test))]
fn designated_requirement(output: &str) -> Option<String> {
    output
        .lines()
        .find_map(|line| line.strip_prefix("designated => "))
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .map(str::to_owned)
}

/// Fork-local. The acceptance policy. The feed and package signatures, bundle
/// identity and `codesign --verify` are checked before this runs.
/// - A download that satisfies the pinned requirement is accepted (this covers the
///   move from an ad-hoc build to the self-signed one).
/// - A signed running app also accepts a download that satisfies its own designated
///   requirement. This only matters for a non-pinned signed running app (for example
///   Developer ID). Rotating the pinned certificate needs a build signed by the old
///   certificate that ships the new pinned hash first.
/// - Ad-hoc replacing ad-hoc is accepted (legacy transition).
/// - A signed app never accepts an ad-hoc download.
/// - Anything else must pass Gatekeeper.
#[cfg(any(target_os = "macos", test))]
fn decide_acceptance(
    running: SignatureKind,
    downloaded: SignatureKind,
    downloaded_matches_pinned: bool,
    downloaded_matches_running_requirement: bool,
) -> Acceptance {
    if downloaded_matches_pinned {
        return Acceptance::Accept;
    }
    if running == SignatureKind::Signed && downloaded_matches_running_requirement {
        return Acceptance::Accept;
    }
    match (running, downloaded) {
        (SignatureKind::AdHoc, SignatureKind::AdHoc) => Acceptance::Accept,
        (SignatureKind::Signed, SignatureKind::AdHoc) => {
            Acceptance::Reject("The downloaded app is ad-hoc signed but the installed app is not")
        }
        _ => Acceptance::Gatekeeper,
    }
}

#[cfg(target_os = "macos")]
fn signature_kind_of(app: &Path) -> SignatureKind {
    Command::new("/usr/bin/codesign")
        .arg("-dv")
        .arg(app)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| signature_kind(&String::from_utf8_lossy(&o.stderr)))
        .unwrap_or(SignatureKind::Unknown)
}

#[cfg(target_os = "macos")]
fn satisfies(app: &Path, requirement: &str) -> bool {
    Command::new("/usr/bin/codesign")
        .args(["--verify", "--deep", "--strict"])
        .arg(format!("-R={requirement}"))
        .arg(app)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(target_os = "macos")]
fn running_requirement(app: &Path) -> Option<String> {
    let output = Command::new("/usr/bin/codesign")
        .args(["-d", "-r-"])
        .arg(app)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| designated_requirement(&String::from_utf8_lossy(&output.stdout)))?
}

#[cfg(target_os = "macos")]
fn install_mac(update: &Update, package: &Path, target: &Path) -> Result<Installed, String> {
    let parent = target.parent().ok_or("Cannot locate the app directory")?;
    let (stage, authorize) = match tempfile::Builder::new()
        .prefix(".monocode-update-")
        .tempdir_in(parent)
    {
        Ok(stage) => (stage, false),
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => (
            tempfile::Builder::new()
                .prefix("monocode-update-")
                .tempdir()
                .map_err(|e| e.to_string())?,
            true,
        ),
        Err(e) => return Err(e.to_string()),
    };
    extract_app(package, stage.path())?;
    let prepared = stage.path().join("Monoptah Native.app");
    let plist =
        fs::read_to_string(prepared.join("Contents/Info.plist")).map_err(|e| e.to_string())?;
    for expected in [
        format!("<string>{}</string>", super::APP_ID),
        format!("<string>{}</string>", update.version),
    ] {
        if !plist.contains(&expected) {
            return Err("The downloaded app has a different identity or version".into());
        }
    }
    success(
        Command::new("/usr/bin/codesign")
            .args(["--verify", "--deep", "--strict"])
            .arg(&prepared),
        "The downloaded app signature is invalid",
    )?;
    // Fork-local: Monoptah Native releases are signed with a stable self-signed
    // certificate and not notarized, so Gatekeeper can never approve them. Identity is
    // pinned instead (see `decide_acceptance`); the feed and package signatures, bundle
    // identity and `codesign --verify` above still apply.
    let running = signature_kind_of(target);
    let downloaded = signature_kind_of(&prepared);
    let pinned = satisfies(&prepared, &pinned_requirement());
    let matches_running = running == SignatureKind::Signed
        && running_requirement(target).is_some_and(|r| satisfies(&prepared, &r));
    match decide_acceptance(running, downloaded, pinned, matches_running) {
        Acceptance::Accept => {}
        Acceptance::Reject(reason) => return Err(reason.into()),
        Acceptance::Gatekeeper => success(
            Command::new("/usr/sbin/spctl")
                .args(["--assess", "--type", "execute"])
                .arg(&prepared),
            "macOS did not approve the downloaded app",
        )?,
    }
    if authorize {
        // Pass paths as quoted shell data, then quote the complete command as
        // an AppleScript string. Never interpolate an unescaped app path.
        let suffix = stage
            .path()
            .file_name()
            .ok_or("Invalid update stage")?
            .to_string_lossy();
        let next = parent.join(format!(".{suffix}.new.app"));
        let previous = parent.join(format!(".{suffix}.previous.app"));
        let quote = |p: &Path| format!("'{}'", p.to_string_lossy().replace('\'', "'\\''"));
        let shell = format!("set -e; /usr/bin/ditto {} {}; /bin/mv {} {}; if /bin/mv {} {}; then /bin/rm -rf {}; else /bin/mv {} {}; exit 1; fi", quote(&prepared), quote(&next), quote(target), quote(&previous), quote(&next), quote(target), quote(&previous), quote(&previous), quote(target));
        let script = format!(
            "do shell script {} with administrator privileges",
            serde_json::to_string(&shell).map_err(|e| e.to_string())?
        );
        success(
            Command::new("/usr/bin/osascript").arg("-e").arg(script),
            "The app update was cancelled or failed",
        )?;
    } else {
        let backup = stage.path().join("previous.app");
        let result = replace(&prepared, target, &backup);
        if result.is_err() && backup.exists() {
            let _ = stage.keep();
        }
        result?;
    }
    Ok(Installed::Relaunch(
        target.join("Contents/MacOS/monocode-gpui"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn app_archives_preserve_files_and_refuse_other_roots_and_links() {
        fn archive(path: &Path, name: &str, symlink: bool) {
            let file = fs::File::create(path).unwrap();
            let gzip = flate2::write::GzEncoder::new(file, flate2::Compression::default());
            let mut archive = tar::Builder::new(gzip);
            let mut header = tar::Header::new_gnu();
            header.set_mode(0o755);
            if symlink {
                header.set_entry_type(tar::EntryType::Symlink);
                header.set_link_name("../../outside").unwrap();
                header.set_size(0);
                header.set_cksum();
                archive.append_data(&mut header, name, &b""[..]).unwrap();
            } else {
                header.set_size(5);
                header.set_cksum();
                archive
                    .append_data(&mut header, name, &b"hello"[..])
                    .unwrap();
            }
            archive.into_inner().unwrap().finish().unwrap();
        }
        let temp = tempfile::tempdir().unwrap();
        let package = temp.path().join("package.tar.gz");
        let stage = temp.path().join("stage");
        fs::create_dir(&stage).unwrap();
        archive(
            &package,
            "Monoptah Native.app/Contents/MacOS/monocode-gpui",
            false,
        );
        extract_app(&package, &stage).unwrap();
        assert_eq!(
            fs::read(stage.join("Monoptah Native.app/Contents/MacOS/monocode-gpui")).unwrap(),
            b"hello"
        );
        archive(&package, "Another.app/Contents/file", false);
        assert!(extract_app(&package, &stage).is_err());
        archive(&package, "Monoptah Native.app/escape", true);
        assert!(extract_app(&package, &stage).is_err());
        assert!(!temp.path().join("outside").exists());
    }
    const ADHOC: &str = "Executable=/Applications/Monoptah Native.app/Contents/MacOS/monocode-gpui\nIdentifier=com.monoptah.native\nCodeDirectory v=20400 size=1 flags=0x2(adhoc) hashes=1+3 location=embedded\nSignature=adhoc\nTeamIdentifier=not set\n";
    const SELF_SIGNED: &str = "Identifier=com.monoptah.native\nCodeDirectory v=20400 size=1 flags=0x0(none) hashes=1+3 location=embedded\nSignature size=1700\nAuthority=Monoptah Native Release Signing\nTeamIdentifier=not set\n";
    #[test]
    fn signature_kinds_are_read_from_codesign_output() {
        assert_eq!(signature_kind(ADHOC), SignatureKind::AdHoc);
        assert_eq!(signature_kind(SELF_SIGNED), SignatureKind::Signed);
        let developer_id = "Identifier=com.monoptah.native\nCodeDirectory v=20500 flags=0x10000(runtime)\nAuthority=Developer ID Application: Example (TEAM)\nTeamIdentifier=TEAM\n";
        assert_eq!(signature_kind(developer_id), SignatureKind::Signed);
        assert_eq!(signature_kind(""), SignatureKind::Unknown);
        assert_eq!(
            signature_kind("code object is not signed at all\n"),
            SignatureKind::Unknown
        );
        assert_eq!(
            signature_kind("Foo=Signature=adhoc\nXAuthority=a\n"),
            SignatureKind::Unknown
        );
    }
    #[test]
    fn designated_requirement_is_read_from_its_line() {
        let output = "# comment\ndesignated => identifier \"com.monoptah.native\" and certificate leaf = H\"0a44\"\n";
        assert_eq!(
            designated_requirement(output).as_deref(),
            Some("identifier \"com.monoptah.native\" and certificate leaf = H\"0a44\"")
        );
        assert_eq!(designated_requirement(""), None);
        assert_eq!(designated_requirement("designated => \n"), None);
        assert_eq!(designated_requirement("x designated => y\n"), None);
    }
    #[test]
    fn pinned_requirement_names_the_app_and_the_certificate() {
        assert_eq!(
            pinned_requirement(),
            format!(
                "identifier \"{}\" and certificate leaf = H\"0a44567bf676438c0dad7e5b3ec5ef80708bebfe\"",
                super::super::APP_ID
            )
        );
    }
    #[test]
    fn acceptance_matrix() {
        use Acceptance::*;
        use SignatureKind::*;
        // adhoc -> pinned, pinned -> pinned: accepted without Gatekeeper.
        assert_eq!(decide_acceptance(AdHoc, Signed, true, false), Accept);
        assert_eq!(decide_acceptance(Signed, Signed, true, false), Accept);
        assert_eq!(decide_acceptance(Unknown, Signed, true, false), Accept);
        // adhoc -> adhoc keeps the legacy transition.
        assert_eq!(decide_acceptance(AdHoc, AdHoc, false, false), Accept);
        // No downgrade of the signing identity.
        assert!(matches!(
            decide_acceptance(Signed, AdHoc, false, false),
            Reject(_)
        ));
        // A signed running app also accepts its own designated requirement.
        assert_eq!(decide_acceptance(Signed, Signed, false, true), Accept);
        // The running requirement only counts for a signed running app.
        assert_eq!(decide_acceptance(AdHoc, Signed, false, true), Gatekeeper);
        // Requirement mismatch or unreadable signatures go to Gatekeeper.
        assert_eq!(decide_acceptance(Signed, Signed, false, false), Gatekeeper);
        assert_eq!(decide_acceptance(Signed, Unknown, false, false), Gatekeeper);
        assert_eq!(decide_acceptance(Unknown, AdHoc, false, false), Gatekeeper);
        assert_eq!(decide_acceptance(AdHoc, Signed, false, false), Gatekeeper);
    }
    #[test]
    fn failed_swap_restores_the_previous_executable() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("app");
        let backup = temp.path().join("backup");
        fs::write(&target, "old version").unwrap();
        assert!(replace(&temp.path().join("missing"), &target, &backup).is_err());
        assert_eq!(fs::read_to_string(target).unwrap(), "old version");
        assert!(!backup.exists());
    }
    #[test]
    fn successful_swap_preserves_a_backup_until_the_caller_commits() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("app");
        let prepared = temp.path().join("new");
        let backup = temp.path().join("backup");
        fs::write(&target, "old").unwrap();
        fs::write(&prepared, "new").unwrap();
        replace(&prepared, &target, &backup).unwrap();
        assert_eq!(fs::read_to_string(target).unwrap(), "new");
        assert_eq!(fs::read_to_string(backup).unwrap(), "old");
    }
}
