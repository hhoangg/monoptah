//! A small file logger for `log` records at warn level and above. GPUI reports
//! frame stalls, lost display links and drawable failures only through `log`,
//! and a Finder-launched app has no stderr, so without this they are lost.
//!
//! The file is bounded: it rotates to `native.log.1` past `MAX_BYTES` (one old
//! file is kept) and at most `MAX_RECORDS_PER_MINUTE` records are written per
//! minute, so the logger holds at most about 2 MiB on disk.
use std::{
    fs::File,
    io::Write,
    path::{Path, PathBuf},
    sync::Mutex,
    time::{Duration, Instant},
};

const MAX_BYTES: u64 = 1024 * 1024;
const MAX_RECORDS_PER_MINUTE: u32 = 200;
const MAX_MESSAGE_CHARS: usize = 2000;
const WINDOW: Duration = Duration::from_secs(60);

/// Starts a window on the first record and admits `MAX_RECORDS_PER_MINUTE`
/// per window. A new window reports how many records the last one dropped.
#[derive(Default)]
struct RateLimit {
    window_start: Option<Instant>,
    admitted: u32,
    dropped: u64,
}
enum Verdict {
    Write { dropped_before: u64 },
    Drop,
}
impl RateLimit {
    fn check(&mut self, now: Instant) -> Verdict {
        let mut dropped_before = 0;
        if self
            .window_start
            .is_none_or(|start| now.saturating_duration_since(start) >= WINDOW)
        {
            self.window_start = Some(now);
            self.admitted = 0;
            dropped_before = std::mem::take(&mut self.dropped);
        }
        if self.admitted >= MAX_RECORDS_PER_MINUTE {
            self.dropped += 1;
            return Verdict::Drop;
        }
        self.admitted += 1;
        Verdict::Write { dropped_before }
    }
}

struct Sink {
    path: PathBuf,
    file: Option<File>,
    size: u64,
    limit: RateLimit,
}
impl Sink {
    fn new(directory: &Path) -> Self {
        let _ = std::fs::create_dir_all(directory);
        let path = directory.join("native.log");
        let mut sink = Self {
            path,
            file: None,
            size: 0,
            limit: RateLimit::default(),
        };
        sink.open();
        sink
    }

    fn open(&mut self) {
        use std::os::unix::fs::OpenOptionsExt;
        self.file = std::fs::OpenOptions::new()
            .append(true)
            .create(true)
            .mode(0o600)
            .open(&self.path)
            .ok();
        self.size = self
            .file
            .as_ref()
            .and_then(|file| file.metadata().ok())
            .map_or(0, |meta| meta.len());
    }

    fn write_line(&mut self, line: &str) {
        let bytes = line.len() as u64 + 1;
        if self.size + bytes > MAX_BYTES {
            self.file = None;
            let mut old = self.path.clone().into_os_string();
            old.push(".1");
            if std::fs::rename(&self.path, old).is_err() {
                // Without a rotation the file would grow forever; start it over.
                let _ = std::fs::OpenOptions::new()
                    .write(true)
                    .truncate(true)
                    .open(&self.path);
            }
            self.open();
        }
        if let Some(file) = self.file.as_mut()
            && writeln!(file, "{line}").is_ok()
        {
            self.size += bytes;
        }
    }

    fn record(&mut self, now: Instant, line: impl FnOnce() -> String) {
        match self.limit.check(now) {
            Verdict::Drop => {}
            Verdict::Write { dropped_before } => {
                if dropped_before > 0 {
                    let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f");
                    self.write_line(&format!(
                        "{stamp} WARN native_log {dropped_before} records dropped"
                    ));
                }
                self.write_line(&line());
            }
        }
    }
}

struct NativeLog(Mutex<Sink>);
impl log::Log for NativeLog {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        wanted(metadata.level(), metadata.target())
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        // Format before locking so the lock covers only the check and the write.
        let line = format_line(record);
        let Ok(mut sink) = self.0.lock() else {
            return;
        };
        sink.record(Instant::now(), move || line);
    }

    fn flush(&self) {}
}

/// Only the UI runtime and the watchdog are recorded. Other crates (HTTP,
/// TLS, providers) can put URLs or request details in their warnings, and the
/// file is meant to be safe to send along with a freeze report.
const RECORDED_CRATES: [&str; 4] = ["gpui", "gpui_macos", "gpui_apple", "ui_watchdog"];

fn recorded_target(target: &str) -> bool {
    let krate = target.split("::").next().unwrap_or_default();
    RECORDED_CRATES.contains(&krate)
}

/// Warnings from every recorded crate, plus the macOS window's info lines
/// (occlusion, resize, file drop and its frame probe), which describe the
/// state around a freeze and are otherwise lost.
fn wanted(level: log::Level, target: &str) -> bool {
    recorded_target(target)
        && (level <= log::Level::Warn
            || (level == log::Level::Info && target.split("::").next() == Some("gpui_macos")))
}

fn format_line(record: &log::Record) -> String {
    let mut message = record.args().to_string().replace(['\n', '\r'], " ");
    if let Some((index, _)) = message.char_indices().nth(MAX_MESSAGE_CHARS) {
        message.truncate(index);
    }
    format!(
        "{} {} {} {}",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S%.3f"),
        record.level(),
        record.target(),
        message
    )
}

/// Installs the logger. Does nothing if another logger is already set or the
/// directory cannot be used.
pub(crate) fn install(directory: &Path) {
    let logger = Box::leak(Box::new(NativeLog(Mutex::new(Sink::new(directory)))));
    if log::set_logger(logger).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
}

/// Writes one warn line from the watchdog thread.
pub(crate) fn warn(message: &str) {
    log::warn!(target: "ui_watchdog", "{message}");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let directory = std::env::temp_dir().join(format!(
            "monocode-native-log-{name}-{}",
            monocode_core::mono::new_id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        directory
    }

    #[test]
    fn records_info_only_from_the_macos_window() {
        assert!(wanted(log::Level::Info, "gpui_macos::window"));
        assert!(wanted(log::Level::Warn, "gpui::window"));
        assert!(!wanted(log::Level::Info, "gpui::window"));
        assert!(!wanted(log::Level::Info, "gpui_apple"));
        assert!(!wanted(log::Level::Debug, "gpui_macos::window"));
        assert!(!wanted(log::Level::Warn, "reqwest::connect"));
    }

    #[test]
    fn records_only_the_ui_runtime_and_the_watchdog() {
        for target in [
            "gpui",
            "gpui::window",
            "gpui_macos::window",
            "gpui_apple",
            "ui_watchdog",
        ] {
            assert!(recorded_target(target), "{target}");
        }
        for target in [
            "gpuix",
            "reqwest::connect",
            "monocode_core::providers",
            "hyper",
            "",
        ] {
            assert!(!recorded_target(target), "{target}");
        }
    }

    #[test]
    fn rate_limit_drops_past_the_cap_and_reports_the_count_next_window() {
        let mut limit = RateLimit::default();
        let start = Instant::now();
        for _ in 0..MAX_RECORDS_PER_MINUTE {
            assert!(matches!(
                limit.check(start),
                Verdict::Write { dropped_before: 0 }
            ));
        }
        for _ in 0..7 {
            assert!(matches!(limit.check(start), Verdict::Drop));
        }
        assert!(matches!(
            limit.check(start + WINDOW),
            Verdict::Write { dropped_before: 7 }
        ));
    }

    #[test]
    fn rotates_past_the_size_cap_keeping_one_old_file_with_private_mode() {
        let directory = temp_dir("rotate");
        let mut sink = Sink::new(&directory);
        let line = "x".repeat(1000);
        for _ in 0..1100 {
            sink.write_line(&line);
        }
        let current = std::fs::metadata(directory.join("native.log")).unwrap();
        let old = std::fs::metadata(directory.join("native.log.1")).unwrap();
        assert!(current.len() <= MAX_BYTES && current.len() > 0);
        assert!(old.len() <= MAX_BYTES && old.len() > MAX_BYTES / 2);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(current.permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::read_dir(&directory).unwrap().count(), 2);
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn a_failed_rotation_truncates_instead_of_growing_without_bound() {
        let directory = temp_dir("norotate");
        // A non-empty directory in the way makes the rename fail.
        let blocker = directory.join("native.log.1");
        std::fs::create_dir_all(blocker.join("inner")).unwrap();
        let mut sink = Sink::new(&directory);
        let line = "x".repeat(1000);
        for _ in 0..3000 {
            sink.write_line(&line);
        }
        let size = std::fs::metadata(directory.join("native.log"))
            .unwrap()
            .len();
        assert!(size <= MAX_BYTES, "{size}");
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn writes_a_drop_notice_and_survives_an_unusable_directory() {
        let directory = temp_dir("drop");
        let mut sink = Sink::new(&directory);
        let start = Instant::now();
        for i in 0..MAX_RECORDS_PER_MINUTE + 3 {
            sink.record(start, || format!("line {i}"));
        }
        sink.record(start + WINDOW, || "after".into());
        let text = std::fs::read_to_string(directory.join("native.log")).unwrap();
        assert!(text.contains("3 records dropped"), "{text}");
        assert!(!text.contains(&format!("line {}", MAX_RECORDS_PER_MINUTE + 1)));
        assert!(text.trim_end().ends_with("after"));
        std::fs::remove_dir_all(&directory).unwrap();

        std::fs::create_dir_all(&directory).unwrap();
        let file = directory.join("blocker");
        std::fs::write(&file, "x").unwrap();
        let mut sink = Sink::new(&file.join("sub"));
        sink.record(Instant::now(), || "ignored".into());
        std::fs::remove_dir_all(&directory).unwrap();
    }
}
