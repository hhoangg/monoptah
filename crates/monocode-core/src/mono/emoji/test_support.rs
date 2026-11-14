//! Tiny images and zip files for tests of the emoji importer and the Emoji settings page.

use image::{ExtendedColorType, ImageEncoder, RgbaImage};
use std::io::Write;
use std::path::Path;

fn rgba(w: u32, h: u32, seed: u8) -> RgbaImage {
    RgbaImage::from_fn(w, h, |x, y| {
        image::Rgba([seed, (x * 40) as u8, (y * 40) as u8, 255])
    })
}

/// A small PNG; different `seed` values give different bytes (so different hashes).
pub fn png(w: u32, h: u32, seed: u8) -> Vec<u8> {
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out)
        .write_image(rgba(w, h, seed).as_raw(), w, h, ExtendedColorType::Rgba8)
        .expect("encode a PNG");
    out
}

/// A small animated GIF with `frames` frames.
pub fn gif(frames: usize) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut encoder = image::codecs::gif::GifEncoder::new(&mut out);
        let list = (0..frames).map(|i| image::Frame::new(rgba(2, 2, i as u8 * 50)));
        encoder.encode_frames(list).expect("encode a GIF");
    }
    out
}

/// Write a zip file with the given (entry name, bytes) pairs, stored without compression.
pub fn zip(path: &Path, entries: &[(&str, &[u8])]) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create the zip folder");
    }
    let options =
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    let mut writer = zip::ZipWriter::new(std::fs::File::create(path).expect("create the zip"));
    for (name, data) in entries {
        writer
            .start_file(*name, options)
            .expect("start a zip entry");
        writer.write_all(data).expect("write a zip entry");
    }
    writer.finish().expect("finish the zip");
}
