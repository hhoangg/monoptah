//! Writes vectors/monoptah-ik.snow.json. Run from web/spikes/s-04-noise/rust: `cargo run --bin gen`.
use std::path::PathBuf;

fn main() {
    let vectors = s04_noise_interop::build_vectors().expect("build vectors");
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../vectors/monoptah-ik.snow.json");
    let mut text = serde_json::to_string_pretty(&vectors).expect("serialize");
    text.push('\n');
    std::fs::write(&path, text).expect("write vectors");
    println!("wrote {}", path.display());
}
