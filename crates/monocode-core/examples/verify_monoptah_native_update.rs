//! Fork-local (Monoptah). Offline publication check for the macOS arm64 native
//! release, run with the actual client verifier and embedded public key. Upstream's
//! verify_native_update example expects all six platform packages; this fork
//! publishes only `darwin-aarch64-app`.
use monocode_core::app_update::{verify_feed, Config};
use std::{env, fs, path::PathBuf};

const TARGET: &str = "darwin-aarch64-app";

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
    let records: Vec<serde_json::Value> = serde_json::from_slice(
        &fs::read(directory.join("artifacts-macos-aarch64.json")).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let mut verified = 0;
    for record in records {
        let Some(target) = record["target"].as_str() else {
            continue;
        };
        if target != TARGET {
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
        let update =
            verify_feed(&feed, &config, target)?.ok_or("No native update in the signed feed")?;
        if update.version.to_string() != expected {
            return Err("Signed feed version does not match the native release".into());
        }
        update.verify_file(&directory.join(filename))?;
        verified += 1;
    }
    if verified != 1 {
        return Err(format!(
            "Expected one signed native update package, verified {verified}"
        ));
    }
    println!("Verified the native feed and the {TARGET} updater package with the client verifier.");
    Ok(())
}
fn main() {
    if let Err(error) = verify() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
