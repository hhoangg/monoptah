//! Fork-local (Monoptah). Offline publication check for the Monoptah Native
//! release, run with the actual client verifier and embedded public key.
//! Upstream's verify_native_update example expects all six upstream packages;
//! this fork publishes five: macOS arm64, Linux x86_64 (AppImage, deb, rpm) and
//! Windows x86_64 (NSIS).
use monocode_core::app_update::{verify_feed, Config};
use std::{collections::BTreeSet, env, fs, path::PathBuf};

const TARGETS: [&str; 5] = [
    "darwin-aarch64-app",
    "linux-x86_64-appimage",
    "linux-x86_64-deb",
    "linux-x86_64-rpm",
    "windows-x86_64-nsis",
];

fn verify() -> Result<(), String> {
    let directory = PathBuf::from(
        env::args_os()
            .nth(1)
            .ok_or("Expected an artifact directory")?,
    );
    let key =
        env::var("MONOCODE_NATIVE_UPDATER_PUBKEY").map_err(|_| "Missing update public key")?;
    let expected =
        env::var("MONOCODE_NATIVE_VERSION").map_err(|_| "Missing native release version")?;
    let config = Config::new(
        "https://offline.example.test/native-latest/latest.json",
        &key,
        "0.0.0-0",
    )?;
    let feed = fs::read(directory.join("latest.json")).map_err(|e| e.to_string())?;
    let mut verified = BTreeSet::new();
    let mut metadata = Vec::new();
    for entry in fs::read_dir(&directory).map_err(|e| e.to_string())? {
        let name = entry
            .map_err(|e| e.to_string())?
            .file_name()
            .to_string_lossy()
            .to_string();
        if name.starts_with("artifacts-") && name.ends_with(".json") {
            metadata.push(name);
        }
    }
    metadata.sort();
    for name in metadata {
        let records: Vec<serde_json::Value> =
            serde_json::from_slice(&fs::read(directory.join(&name)).map_err(|e| e.to_string())?)
                .map_err(|e| e.to_string())?;
        for record in records {
            let Some(target) = record["target"].as_str() else {
                continue;
            };
            if !TARGETS.contains(&target) {
                return Err(format!("Unexpected native target: {target}"));
            }
            let filename = record["filename"]
                .as_str()
                .ok_or("Missing package filename")?;
            if std::path::Path::new(filename)
                .file_name()
                .and_then(|n| n.to_str())
                != Some(filename)
                || filename.contains('\\')
            {
                return Err("Invalid package filename".into());
            }
            let update = verify_feed(&feed, &config, target)?
                .ok_or("No native update in the signed feed")?;
            if update.version.to_string() != expected {
                return Err("Signed feed version does not match the native release".into());
            }
            update.verify_file(&directory.join(filename))?;
            if !verified.insert(target.to_string()) {
                return Err(format!("Duplicate native target: {target}"));
            }
        }
    }
    for target in TARGETS {
        if !verified.contains(target) {
            return Err(format!("Missing verified native update package: {target}"));
        }
    }
    println!(
        "Verified the native feed and all {} updater packages with the client verifier.",
        TARGETS.len()
    );
    Ok(())
}
fn main() {
    if let Err(error) = verify() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
