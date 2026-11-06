//! Sample the process while a UI stall is happening, before a restart erases
//! the blocked stacks. A dedicated thread works even if GPUI's workers stall.
//!
//! Two stalls are told apart. A hang: the main thread stops running, so its
//! heartbeat goes silent (`ui-hang-*.txt`). A render stall: the main thread is
//! alive and input arrives, but no frame is presented (`ui-stall-*.txt`). Both
//! reports start with the last 30 one-second snapshots of the frame counters,
//! so the next freeze leaves evidence of which stage stopped.
use crate::native_log;
use gpui::App;
use std::{
    collections::VecDeque,
    fmt::Write as _,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

const PERIOD: Duration = Duration::from_secs(1);
const STALL: Duration = Duration::from_secs(5);
const COOLDOWN: Duration = Duration::from_secs(60);
const MAX_REPORTS: usize = 5;
const SNAPSHOTS: usize = 30;
/// Input events within `STALL` that make a flat frame counter suspicious.
/// Lone events (a click on a dead area) legitimately draw nothing.
const MIN_INPUT: u64 = 3;
/// The main thread beats once a second; a gap this long is a visible stutter.
const SLOW_BEAT: Duration = Duration::from_millis(2500);

pub(crate) fn start(directory: PathBuf, cx: &mut App) {
    if cx.is_test() {
        return;
    }
    let epoch = Instant::now();
    let heartbeat = Arc::new(AtomicU64::new(0));
    let max_gap = Arc::new(AtomicU64::new(0));
    let beat = heartbeat.clone();
    let gap = max_gap.clone();
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(PERIOD).await;
            cx.update(|_| {
                let now = epoch.elapsed().as_millis() as u64;
                let previous = beat.swap(now, Ordering::Relaxed);
                let elapsed = now.saturating_sub(previous);
                gap.fetch_max(elapsed, Ordering::Relaxed);
                // The first beat measures from startup, not a stutter.
                if previous != 0 && Duration::from_millis(elapsed) >= SLOW_BEAT {
                    native_log::warn(&format!("main thread beat after {elapsed}ms"));
                }
            });
        }
    })
    .detach();
    let (stop, stopped) = mpsc::channel();
    cx.on_app_quit(move |_| {
        let _ = stop.send(());
        async {}
    })
    .detach();
    if let Err(error) = std::thread::Builder::new()
        .name("ui-watchdog".into())
        .spawn(move || {
            let mut watch = Watch::default();
            let mut render = RenderWatch::default();
            let mut ring = Ring::default();
            let mut previous = read_counters();
            while let Err(mpsc::RecvTimeoutError::Timeout) = stopped.recv_timeout(PERIOD) {
                let now = epoch.elapsed();
                let beat = Duration::from_millis(heartbeat.load(Ordering::Relaxed));
                let current = read_counters();
                ring.push(
                    &previous,
                    &current,
                    now,
                    now.saturating_sub(beat),
                    max_gap.swap(0, Ordering::Relaxed),
                );
                previous = current;
                if watch.poll(now, beat) {
                    let age = now.saturating_sub(beat).as_millis();
                    capture(
                        &directory,
                        "ui-hang",
                        &format!("Main thread heartbeat silent for {age}ms (threshold {}s).", STALL.as_secs()),
                        &ring.render(),
                    );
                }
                if render.poll(now, beat, &current) {
                    let header = format!(
                        "Render stall: main thread alive, input advancing, no frame presented for at least {}s.",
                        STALL.as_secs()
                    );
                    native_log::warn(&format!("render stall: {header}"));
                    capture(&directory, "ui-stall", &header, &ring.render());
                }
            }
        })
    {
        eprintln!("Could not start UI watchdog: {error}");
    }
}

/// Process-wide counters from the vendored GPUI crates. Every field except
/// `next_drawable_max_ms` (a gauge, reset by each read) only grows.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Counters {
    vsync: u64,
    input: u64,
    presented: u64,
    drawable_failures: u64,
    next_drawable_max_ms: u64,
    restarts: u64,
    no_screen: u64,
}

fn read_counters() -> Counters {
    let frames = gpui_macos::frame_diagnostics();
    let draws = gpui_apple::metal_renderer::draw_diagnostics();
    Counters {
        vsync: frames.vsync_ticks,
        input: frames.input_events,
        presented: draws.presented,
        drawable_failures: draws.drawable_failures,
        next_drawable_max_ms: draws.next_drawable_max_ms,
        restarts: frames.stalled_restarts,
        no_screen: frames.no_screen,
    }
}

struct Snapshot {
    at: Duration,
    heartbeat_age: Duration,
    max_gap_ms: u64,
    /// Counter deltas over the second; `next_drawable_max_ms` is the gauge itself.
    delta: Counters,
}

#[derive(Default)]
struct Ring(VecDeque<Snapshot>);
impl Ring {
    fn push(
        &mut self,
        previous: &Counters,
        current: &Counters,
        at: Duration,
        heartbeat_age: Duration,
        max_gap_ms: u64,
    ) {
        if self.0.len() == SNAPSHOTS {
            self.0.pop_front();
        }
        self.0.push_back(Snapshot {
            at,
            heartbeat_age,
            max_gap_ms,
            delta: Counters {
                vsync: current.vsync.saturating_sub(previous.vsync),
                input: current.input.saturating_sub(previous.input),
                presented: current.presented.saturating_sub(previous.presented),
                drawable_failures: current
                    .drawable_failures
                    .saturating_sub(previous.drawable_failures),
                next_drawable_max_ms: current.next_drawable_max_ms,
                restarts: current.restarts.saturating_sub(previous.restarts),
                no_screen: current.no_screen.saturating_sub(previous.no_screen),
            },
        });
    }

    fn render(&self) -> String {
        let mut text = String::new();
        for snapshot in &self.0 {
            let delta = &snapshot.delta;
            let _ = writeln!(
                text,
                "t={}s hb_age={}ms gap={}ms vsync=+{} input=+{} presented=+{} drawable_fail=+{} \
                 next_drawable_max={}ms restarts=+{} no_screen=+{}",
                snapshot.at.as_secs(),
                snapshot.heartbeat_age.as_millis(),
                snapshot.max_gap_ms,
                delta.vsync,
                delta.input,
                delta.presented,
                delta.drawable_failures,
                delta.next_drawable_max_ms,
                delta.restarts,
                delta.no_screen,
            );
        }
        text
    }
}

/// Detects frames that stopped while the main thread and input still work.
#[derive(Default)]
struct RenderWatch {
    polled: Duration,
    captured: Option<Duration>,
    presented: Option<u64>,
    presented_changed: Duration,
    /// `(time, input counter)` for the last `STALL` seconds.
    input: VecDeque<(Duration, u64)>,
    /// One report per stall; presented frames arm the watch again.
    reported: bool,
}
impl RenderWatch {
    fn poll(&mut self, now: Duration, heartbeat: Duration, counters: &Counters) -> bool {
        let paused = now.saturating_sub(self.polled) > PERIOD * 3;
        self.polled = now;
        if paused {
            // After system sleep, give the app a chance to draw again.
            self.input.clear();
            // Frames that never resume after wake must still be reported, so
            // the grace period is a fresh stall clock rather than a suppression.
            self.presented = None;
        }
        if self.presented != Some(counters.presented) {
            self.presented = Some(counters.presented);
            self.presented_changed = now;
            self.reported = false;
        }
        self.input.push_back((now, counters.input));
        while self
            .input
            .front()
            .is_some_and(|(at, _)| now.saturating_sub(*at) > STALL)
        {
            self.input.pop_front();
        }
        let recent_input = self
            .input
            .front()
            .map_or(0, |(_, first)| counters.input.saturating_sub(*first));
        if !self.reported
            && now.saturating_sub(heartbeat) < STALL
            && now.saturating_sub(self.presented_changed) >= STALL
            && recent_input >= MIN_INPUT
            && self
                .captured
                .is_none_or(|at| now.saturating_sub(at) >= COOLDOWN)
        {
            self.captured = Some(now);
            self.reported = true;
            return true;
        }
        false
    }
}

#[derive(Default)]
struct Watch {
    polled: Duration,
    captured: Option<Duration>,
    heartbeat: Option<Duration>,
    /// One report per stall; a resumed heartbeat arms the watchdog again.
    reported: bool,
}
impl Watch {
    fn poll(&mut self, now: Duration, heartbeat: Duration) -> bool {
        let paused = now.saturating_sub(self.polled) > PERIOD * 3;
        self.polled = now;
        if self.heartbeat != Some(heartbeat) {
            self.heartbeat = Some(heartbeat);
            self.reported = false;
        }
        if paused {
            // After system sleep, give the main thread a chance to resume.
            self.reported = true;
        }
        if !self.reported
            && now.saturating_sub(heartbeat) >= STALL
            && self
                .captured
                .is_none_or(|at| now.saturating_sub(at) >= COOLDOWN)
        {
            self.captured = Some(now);
            self.reported = true;
            return true;
        }
        false
    }
}

/// Report files to delete so that `keep` remain, oldest first. Hang and stall
/// reports share the budget; the timestamp after the prefix orders them.
fn prune_victims(names: Vec<String>, keep: usize) -> Vec<String> {
    let mut reports = names
        .into_iter()
        .filter(|name| {
            (name.starts_with("ui-hang-") || name.starts_with("ui-stall-"))
                && name.ends_with(".txt")
        })
        .collect::<Vec<_>>();
    reports.sort_by_key(|name| {
        let stamp = name
            .strip_prefix("ui-hang-")
            .or_else(|| name.strip_prefix("ui-stall-"))
            .unwrap_or(name)
            .to_string();
        (stamp, name.clone())
    });
    let excess = reports.len().saturating_sub(keep);
    reports.truncate(excess);
    reports
}

/// Stack samples written aside by an earlier process that ended before it
/// could append and remove them (for example, quit while a stall was sampled).
fn stale_samples(names: Vec<String>, pid: u32) -> Vec<String> {
    let current = format!(".sample-{pid}.tmp");
    names
        .into_iter()
        .filter(|name| name.starts_with(".sample-") && name.ends_with(".tmp") && *name != current)
        .collect()
}

/// Writes `<prefix>-<time>-<pid>.txt` holding the header, the snapshot ring and
/// a stack sample.
fn capture(directory: &Path, prefix: &str, header: &str, ring: &str) {
    let result = (|| -> std::io::Result<()> {
        use std::io::Write as _;
        use std::os::unix::fs::OpenOptionsExt;
        std::fs::create_dir_all(directory)?;
        let names = std::fs::read_dir(directory)?
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        let pid = std::process::id();
        for name in stale_samples(names.clone(), pid) {
            let _ = std::fs::remove_file(directory.join(name));
        }
        for name in prune_victims(names, MAX_REPORTS - 1) {
            let _ = std::fs::remove_file(directory.join(name));
        }
        let path = directory.join(format!(
            "{prefix}-{}-{pid}.txt",
            chrono::Local::now().format("%Y%m%d-%H%M%S"),
        ));
        let mut report = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        write!(
            report,
            "{header}\nOne line per second, oldest first (gap is the longest main thread heartbeat \
             gap; about 1000ms is normal; counters are per-second deltas):\n{ring}\n"
        )?;
        // `sample -file` may overwrite its target, so it writes aside and the
        // result is appended below the ring.
        let aside = directory.join(format!(".sample-{pid}.tmp"));
        let _ = std::fs::remove_file(&aside);
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&aside)?;
        let status = std::process::Command::new("/usr/bin/sample")
            .args([pid.to_string(), "2".into(), "10".into(), "-file".into()])
            .arg(&aside)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        let sample = std::fs::read(&aside);
        let _ = std::fs::remove_file(&aside);
        match (&status, sample) {
            (Ok(status), Ok(sample)) if status.success() => {
                report.write_all(&sample)?;
            }
            _ => {
                writeln!(report, "Stack sample failed: {status:?}")?;
                return Err(std::io::Error::other(format!(
                    "sample failed: {status:?}; kept the counters in {}",
                    path.display()
                )));
            }
        }
        eprintln!("UI stall saved {}", path.display());
        Ok(())
    })();
    if let Err(error) = result {
        eprintln!("Could not sample UI stall: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn seconds(value: u64) -> Duration {
        Duration::from_secs(value)
    }

    #[test]
    fn healthy_heartbeats_do_not_capture_and_a_stall_is_sampled_once() {
        let mut watch = Watch::default();
        for time in 1..10 {
            assert!(!watch.poll(seconds(time), seconds(time)));
        }
        for time in 10..14 {
            assert!(!watch.poll(seconds(time), seconds(9)));
        }
        assert!(watch.poll(seconds(14), seconds(9)));
        for time in 15..100 {
            assert!(!watch.poll(seconds(time), seconds(9)));
        }
        assert!(!watch.poll(seconds(100), seconds(100)));
        for time in 101..105 {
            assert!(!watch.poll(seconds(time), seconds(100)));
        }
        assert!(watch.poll(seconds(105), seconds(100)));
    }

    #[test]
    fn sleep_and_repeated_short_stalls_do_not_flood_reports() {
        let mut watch = Watch::default();
        assert!(!watch.poll(seconds(1), seconds(1)));
        assert!(!watch.poll(seconds(600), seconds(1)));
        assert!(!watch.poll(seconds(601), seconds(601)));
        for time in 602..606 {
            assert!(!watch.poll(seconds(time), seconds(601)));
        }
        assert!(watch.poll(seconds(606), seconds(601)));
        assert!(!watch.poll(seconds(607), seconds(607)));
        for time in 608..666 {
            assert!(!watch.poll(seconds(time), seconds(607)));
        }
        assert!(watch.poll(seconds(666), seconds(607)));
    }

    fn counters(input: u64, presented: u64) -> Counters {
        Counters {
            input,
            presented,
            ..Counters::default()
        }
    }

    /// Frames present until second 10, then stop while 2 inputs arrive each second.
    fn run_stall(watch: &mut RenderWatch, from: u64, to: u64) -> Vec<u64> {
        let mut fired = Vec::new();
        for time in from..to {
            let presented = time.min(10);
            if watch.poll(seconds(time), seconds(time), &counters(time * 2, presented)) {
                fired.push(time);
            }
        }
        fired
    }

    #[test]
    fn render_stall_fires_once_when_input_moves_but_frames_do_not() {
        let mut watch = RenderWatch::default();
        assert_eq!(run_stall(&mut watch, 1, 100), vec![15]);
    }

    #[test]
    fn render_stall_needs_fresh_heartbeat_real_input_and_rearms_when_frames_resume() {
        // Main thread silent: that is a hang, not a render stall.
        let mut watch = RenderWatch::default();
        for time in 1..40 {
            assert!(!watch.poll(seconds(time), seconds(0), &counters(time * 2, 2)));
        }
        // An idle window: frames flat, no input.
        let mut watch = RenderWatch::default();
        for time in 1..40 {
            assert!(!watch.poll(seconds(time), seconds(time), &counters(0, 2)));
        }
        // A lone input event is not enough.
        let mut watch = RenderWatch::default();
        for time in 1..40 {
            let input = u64::from(time == 20);
            assert!(!watch.poll(seconds(time), seconds(time), &counters(input, 2)));
        }
        // Frames resume, then stall again after the cooldown.
        let mut watch = RenderWatch::default();
        assert_eq!(run_stall(&mut watch, 1, 30), vec![15]);
        assert!(!watch.poll(seconds(30), seconds(30), &counters(60, 30)));
        let mut fired = Vec::new();
        for time in 31..120 {
            if watch.poll(seconds(time), seconds(time), &counters(time * 2, 30)) {
                fired.push(time);
            }
        }
        assert_eq!(fired, vec![75]);
    }

    #[test]
    fn render_stall_after_sleep_waits_a_full_period_then_reports_if_frames_never_resume() {
        let mut watch = RenderWatch::default();
        assert!(!watch.poll(seconds(1), seconds(1), &counters(0, 1)));
        let mut fired = Vec::new();
        for time in 600..620 {
            if watch.poll(seconds(time), seconds(time), &counters((time - 599) * 2, 1)) {
                fired.push(time);
            }
        }
        assert_eq!(fired, vec![605]);
    }

    #[test]
    fn ring_keeps_thirty_snapshots_of_deltas_and_renders_one_line_each() {
        let mut ring = Ring::default();
        let mut prev = Counters::default();
        for time in 1..=40u64 {
            let cur = Counters {
                vsync: time * 60,
                input: time,
                presented: time * 59,
                drawable_failures: 0,
                next_drawable_max_ms: 1000,
                restarts: u64::from(time >= 35),
                no_screen: 0,
            };
            ring.push(&prev, &cur, seconds(time), Duration::from_millis(12), 1003);
            prev = cur;
        }
        let text = ring.render();
        assert_eq!(text.lines().count(), SNAPSHOTS);
        let last = text.lines().last().unwrap();
        for part in [
            "t=40s",
            "hb_age=12ms",
            "gap=1003ms",
            "vsync=+60",
            "input=+1",
            "presented=+59",
            "drawable_fail=+0",
            "next_drawable_max=1000ms",
            "restarts=+0",
            "no_screen=+0",
        ] {
            assert!(last.contains(part), "{part} missing from {last}");
        }
        assert!(text.lines().next().unwrap().contains("t=11s"));
        assert!(text.contains("restarts=+1"));
    }

    #[test]
    fn pruning_counts_hang_and_stall_reports_together_oldest_first() {
        let names = [
            "ui-hang-20260101-000001-1.txt",
            "ui-stall-20260101-000002-1.txt",
            "ui-hang-20260101-000003-1.txt",
            "ui-stall-20260101-000004-1.txt",
            "ui-hang-20260101-000005-1.txt",
            "ui-stall-20260101-000006-1.txt",
            "unrelated.txt",
            "native.log",
        ]
        .map(String::from)
        .to_vec();
        assert_eq!(
            prune_victims(names, 4),
            vec![
                "ui-hang-20260101-000001-1.txt".to_string(),
                "ui-stall-20260101-000002-1.txt".to_string(),
            ]
        );
    }

    #[test]
    fn sample_files_left_by_other_processes_are_removed() {
        let names = vec![
            ".sample-11.tmp".to_string(),
            ".sample-22.tmp".to_string(),
            ".sample-x.txt".to_string(),
            "ui-hang-20260101-000001-11.txt".to_string(),
        ];
        assert_eq!(stale_samples(names, 22), vec![".sample-11.tmp".to_string()]);
    }

    #[test]
    fn captures_private_stack_reports_and_keeps_only_five() {
        let directory = std::env::temp_dir().join(format!(
            "monocode-watchdog-test-{}",
            monocode_core::mono::new_id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        for i in 0..MAX_REPORTS {
            std::fs::write(
                directory.join(format!("ui-hang-20000101-00000{i}.txt")),
                "old report",
            )
            .unwrap();
        }
        let other = directory.join("unrelated.txt");
        std::fs::write(&other, "keep").unwrap();
        capture(&directory, "ui-hang", "test header", "t=1s ring line\n");
        let reports = std::fs::read_dir(&directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("ui-hang-")
            })
            .collect::<Vec<_>>();
        assert_eq!(reports.len(), MAX_REPORTS);
        let report = reports
            .iter()
            .find(|path| {
                !path
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with("ui-hang-2000")
            })
            .expect("watchdog did not save a stack sample");
        let sample = std::fs::read_to_string(report).unwrap();
        assert!(
            sample.contains("Call graph:"),
            "missing stack sample: {sample}"
        );
        assert!(sample.starts_with("test header\n"), "{sample}");
        assert!(sample.find("t=1s ring line") < sample.find("Call graph:"));
        assert!(
            !directory
                .join(format!(".sample-{}.tmp", std::process::id()))
                .exists()
        );
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(report).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(std::fs::read_to_string(&other).unwrap(), "keep");
        std::fs::remove_dir_all(&directory).unwrap();
    }
}
