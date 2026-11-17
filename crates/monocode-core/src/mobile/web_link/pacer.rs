//! Paces the frames of one web watch (R-11): at most one flush of watch updates
//! per [`WATCH_INTERVAL`], with the updates held back in between merged where
//! that loses nothing, and heartbeats skipped while data flows.
//!
//! This is the web path only. QUIC writes every update as it comes.
//!
//! Frames are never dropped or reordered. Two held updates merge only when the
//! merge is exact (see [`merge`]); otherwise both go out in the same flush, one
//! TRANSPORT message each (contract gap: the wire protocol has no batch shape,
//! see [`pack`]).
use super::{channel::Out, session::SessionKey};
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::{
    sync::mpsc,
    time::{sleep_until, Instant},
};

/// The shortest time between two flushes of watch updates for one watch.
pub(super) const WATCH_INTERVAL: Duration = Duration::from_millis(500);

/// How many frames the pacer takes from its watch ahead of the relay socket.
/// Beyond it the watch's `write` waits, so its sink fills and the host reports
/// busy, exactly as a slow socket slows a QUIC watch.
pub(super) const QUEUE: usize = 16;
/// The most update envelopes held at once for a slot; past it the pacer stops
/// reading until a flush.
const MAX_HELD: usize = 64;

/// What a [`super::channel::Channel`] hands to its pacer.
pub(super) enum Msg {
    /// A watch update (`{"rid","ok":true,"result":frame}`): held and merged.
    Update(Value),
    /// A heartbeat: skipped when a data frame went out within the heartbeat period.
    Heartbeat(Value),
    /// Any other frame (the accept, an error): held updates go out first, then it.
    Control(Value),
}

/// Merges `later` into `earlier`, two watch frames (`watch.rs` `update`) in the
/// order they were produced. Returns false, leaving `earlier` as it was, when
/// the merge would not be exact.
///
/// `question` is not state but a stream of transitions (`watch.rs` sends it
/// with the first frame, again whenever it changes, and null once answered),
/// and the client applies each one in order. Two frames that both carry it
/// never merge, so an answered-then-new question reaches the client as both.
///
/// A frame holds only what changed since the one before it, so a key in the
/// later frame replaces the same key in the earlier one, and a key it leaves
/// out is unchanged. `turns` are whole snapshots keyed by id, so a later
/// snapshot replaces an earlier one. `patches` are edits relative to the state
/// the frame before it produced, so they are only concatenated, in order, per
/// turn id, and only when neither frame carries snapshot turns (their order
/// against patches inside one frame is not defined).
pub(super) fn merge(earlier: &mut Value, later: &Value) -> bool {
    let (Some(a), Some(b)) = (earlier.as_object(), later.as_object()) else {
        return false;
    };
    let nonempty = |frame: &serde_json::Map<String, Value>, key: &str| {
        frame
            .get(key)
            .and_then(Value::as_array)
            .is_some_and(|list| !list.is_empty())
    };
    let patches = nonempty(a, "patches") || nonempty(b, "patches");
    let turns = nonempty(a, "turns") || nonempty(b, "turns");
    if patches && turns {
        return false;
    }
    if a.contains_key("question") && b.contains_key("question") {
        return false;
    }
    let id_of = |turn: &Value| turn["id"].as_str().map(str::to_owned);
    let mut merged = a.clone();
    if turns {
        let mut list = merged
            .get("turns")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let incoming = b.get("turns").and_then(Value::as_array);
        for turn in incoming.into_iter().flatten() {
            let Some(id) = id_of(turn) else { return false };
            match list.iter().position(|t| id_of(t).as_deref() == Some(&id)) {
                Some(at) => list[at] = turn.clone(),
                None => list.push(turn.clone()),
            }
        }
        if list.iter().any(|t| id_of(t).is_none()) {
            return false;
        }
        merged.insert("turns".into(), Value::Array(list));
    }
    if patches {
        let mut list = merged
            .get("patches")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for patch in b
            .get("patches")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let (Some(id), Some(edits)) = (id_of(patch), patch["edits"].as_array()) else {
                return false;
            };
            let same = list.iter_mut().find(|p| id_of(p).as_deref() == Some(&id));
            match same.and_then(|p| p["edits"].as_array_mut()) {
                Some(existing) => existing.extend(edits.iter().cloned()),
                None => list.push(patch.clone()),
            }
        }
        merged.insert("patches".into(), Value::Array(list));
    }
    for (key, value) in b {
        if key != "turns" && key != "patches" {
            merged.insert(key.clone(), value.clone());
        }
    }
    *earlier = Value::Object(merged);
    true
}

/// The most plaintext one batch of envelopes may carry.
// needs contract (a) batch shape
#[cfg(test)]
pub(super) const PACK_LIMIT: usize = 16 * 1024;

/// Groups consecutive envelopes so that each group, as a JSON array, stays
/// within [`PACK_LIMIT`] bytes; an envelope over the limit is a group of its
/// own. Ready for the day the protocol defines a batch shape inside one MSG.
// needs contract (a) batch shape: `web/protocol` has no batch envelope, so the
// desktop sends one MSG per envelope today and this is exercised by tests only.
#[cfg(test)]
pub(super) fn pack(envelopes: &[Value]) -> Vec<Vec<Value>> {
    let mut groups: Vec<Vec<Value>> = Vec::new();
    let mut size = 0usize;
    for envelope in envelopes {
        let len = envelope.to_string().len();
        // Two brackets, plus a comma between neighbours.
        match groups.last_mut() {
            Some(group) if size + len + 2 <= PACK_LIMIT => {
                group.push(envelope.clone());
                size += 1 + len;
            }
            _ => {
                groups.push(vec![envelope.clone()]);
                size = len + 2;
            }
        }
    }
    groups
}

/// Wraps the held update `frame` back into its envelope.
fn envelope(template: &Value, frame: Value) -> Value {
    let mut wrapped = template.clone();
    wrapped["result"] = frame;
    wrapped
}

/// Paces one watch. Runs next to the watch's `serve_stream`, in the same task,
/// so ending that task (session closed) ends this too.
pub(super) struct Pacer {
    pub(super) key: SessionKey,
    /// The request id of the watch, for the frame that tells the browser the
    /// desktop ended it.
    pub(super) rid: Value,
    /// Whether the device may still receive; asked before every flush.
    pub(super) admit: Arc<dyn Fn() -> bool + Send + Sync>,
    pub(super) input: mpsc::Receiver<Msg>,
    /// Set by the channel when the watch ends cleanly: held updates still go
    /// out at the next slot. A channel dropped without it was cancelled or
    /// closed, and what it held is discarded.
    pub(super) finished: Arc<AtomicBool>,
    pub(super) out: mpsc::Sender<Out>,
    pub(super) interval: Duration,
    pub(super) heartbeat: Duration,
}

/// What the browser reads when the desktop ends a watch with no final frame of
/// its own. The wire protocol has no end-of-watch shape yet (contract gap for
/// WS-K), so it is the existing error envelope: the browser reopens with `from`.
pub(super) const WATCH_ENDED: &str = "Watch ended";

impl Pacer {
    /// Replaces the admission check (the default admits everything).
    pub(super) fn admitting(mut self, admit: impl Fn() -> bool + Send + Sync + 'static) -> Self {
        self.admit = Arc::new(admit);
        self
    }

    pub(super) async fn run(mut self) {
        let mut last_flush: Option<Instant> = None;
        // The last time anything went out; silence is measured from it.
        let mut last_data = Instant::now();
        // A heartbeat the watch asked for while data had just gone out: it is
        // sent once the silence reaches the heartbeat period, so a watch is
        // never quiet for longer than that.
        let mut deferred: Option<Value> = None;
        let mut errored = false;
        // Held envelopes: the first carries the rid and ok fields for the rest.
        let mut held: Vec<Value> = Vec::new();
        let mut open = true;
        loop {
            if !open && held.is_empty() {
                // The watch ended cleanly with no final frame of its own.
                if !errored {
                    let ended = json!({"rid": self.rid, "ok": false, "error": WATCH_ENDED});
                    self.send(ended).await;
                }
                return;
            }
            let slot = last_flush.map_or_else(Instant::now, |at| at + self.interval);
            let waiting = !held.is_empty();
            let beat_at = last_data + self.heartbeat;
            tokio::select! {
                message = self.input.recv(), if open && held.len() < MAX_HELD => match message {
                    // Dropped without a clean finish: cancelled or closed.
                    None if !self.finished.load(Ordering::SeqCst) => return,
                    None => open = false,
                    Some(Msg::Update(value)) => {
                        hold(&mut held, value);
                        if Instant::now() >= slot
                            && !self.flush(&mut held, &mut last_flush, &mut last_data).await
                        {
                            return;
                        }
                    }
                    Some(Msg::Heartbeat(value)) => {
                        if last_data.elapsed() >= self.heartbeat {
                            if !self.send(value).await {
                                return;
                            }
                            last_data = Instant::now();
                            deferred = None;
                        } else {
                            deferred = Some(value);
                        }
                    }
                    Some(Msg::Control(value)) => {
                        if value["ok"] == false {
                            // An error ends the watch: what it still held is
                            // stale (the device may have been revoked).
                            held.clear();
                            errored = true;
                        } else if !self
                            .flush(&mut held, &mut last_flush, &mut last_data)
                            .await
                        {
                            return;
                        }
                        if !self.send(value).await {
                            return;
                        }
                        last_data = Instant::now();
                        deferred = None;
                    }
                },
                _ = sleep_until(beat_at), if deferred.is_some() => {
                    if let Some(value) = deferred.take() {
                        if !self.send(value).await {
                            return;
                        }
                        last_data = Instant::now();
                    }
                }
                _ = sleep_until(slot), if waiting => {
                    if !self.flush(&mut held, &mut last_flush, &mut last_data).await {
                        return;
                    }
                }
            }
        }
    }

    async fn send(&self, value: Value) -> bool {
        self.out
            .send(Out {
                key: self.key,
                value,
            })
            .await
            .is_ok()
    }

    /// Sends every held update, one message each, in order.
    async fn flush(
        &self,
        held: &mut Vec<Value>,
        last_flush: &mut Option<Instant>,
        last_data: &mut Instant,
    ) -> bool {
        if held.is_empty() {
            return true;
        }
        if !(self.admit)() {
            // The device is no longer admitted: nothing held goes out.
            held.clear();
            return true;
        }
        for value in held.drain(..) {
            if !self.send(value).await {
                return false;
            }
        }
        let now = Instant::now();
        *last_flush = Some(now);
        *last_data = now;
        true
    }
}

/// Adds an update to what is held, merged into the last one when exact.
fn hold(held: &mut Vec<Value>, value: Value) {
    if let Some(last) = held.last_mut() {
        let mut frame = last["result"].clone();
        if merge(&mut frame, &value["result"]) {
            *last = envelope(last, frame);
            return;
        }
    }
    held.push(value);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mobile::web_link::{channel::Channel, relay::Timing};
    use monocode_mobile_link::{serve_stream, ServeOptions};
    use serde_json::json;
    use std::sync::Arc;
    use tokio::sync::Notify;

    fn frame(turn: &str, text: &str) -> Value {
        json!({"usageLimit":null,"busy":true,"older":false,"order":[turn],"turns":[{"id":turn,"text":text}]})
    }
    fn patch(id: &str, from: usize, append: &str) -> Value {
        json!({"usageLimit":null,"busy":true,"older":false,"order":[id],"turns":[],
            "patches":[{"id":id,"edits":[{"path":["text"],"from":from,"append":append}]}]})
    }

    #[test]
    fn a_later_snapshot_replaces_an_earlier_one_and_state_keys_take_the_latest() {
        let mut held = frame("t1", "he");
        let mut later = frame("t1", "hello");
        later["busy"] = json!(false);
        later["question"] = json!({"q": 1});
        assert!(merge(&mut held, &later));
        assert_eq!(held["turns"], json!([{"id":"t1","text":"hello"}]));
        assert_eq!(held["busy"], json!(false));
        assert_eq!(held["question"], json!({"q": 1}));
        // A key the later frame leaves out is unchanged, so the earlier one stays.
        let mut quiet = frame("t2", "x");
        quiet["order"] = json!(["t1", "t2"]);
        assert!(merge(&mut held, &quiet));
        assert_eq!(held["question"], json!({"q": 1}));
        assert_eq!(held["order"], json!(["t1", "t2"]));
        assert_eq!(held["turns"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn patches_for_one_turn_concatenate_in_order() {
        let mut held = patch("t1", 0, "ab");
        assert!(merge(&mut held, &patch("t1", 2, "cd")));
        assert!(merge(&mut held, &patch("t2", 0, "z")));
        assert_eq!(
            held["patches"],
            json!([
                {"id":"t1","edits":[
                    {"path":["text"],"from":0,"append":"ab"},
                    {"path":["text"],"from":2,"append":"cd"}]},
                {"id":"t2","edits":[{"path":["text"],"from":0,"append":"z"}]}
            ])
        );
    }

    #[test]
    fn snapshots_and_patches_are_not_merged() {
        let (mut a, b) = (frame("t1", "a"), patch("t1", 1, "b"));
        let before = a.clone();
        assert!(!merge(&mut a, &b));
        assert_eq!(a, before);
        let (mut p, s) = (patch("t1", 1, "b"), frame("t1", "ab"));
        assert!(!merge(&mut p, &s));
        let mut nameless = json!({"turns":[{"text":"x"}]});
        assert!(!merge(&mut nameless, &json!({"turns":[{"id":"a"}]})));
    }

    #[test]
    fn packing_groups_envelopes_up_to_16_kib_as_one_array() {
        let small = json!({"rid":1,"ok":true,"result":{"pad":"x".repeat(1000)}});
        let list = vec![small; 40];
        let groups = pack(&list);
        assert!(groups.len() > 1);
        assert_eq!(groups.iter().map(Vec::len).sum::<usize>(), 40);
        for group in &groups {
            assert!(Value::Array(group.clone()).to_string().len() <= PACK_LIMIT);
        }
        let huge = json!({"pad": "y".repeat(PACK_LIMIT + 1)});
        let groups = pack(&[json!({"a":1}), huge.clone(), json!({"b":2})]);
        assert_eq!(
            groups,
            vec![vec![json!({"a":1})], vec![huge], vec![json!({"b":2})]]
        );
    }

    #[test]
    fn the_keepalive_and_heartbeat_periods_are_30_and_25_seconds() {
        assert_eq!(Timing::default().ping, Duration::from_secs(30));
        assert_eq!(ServeOptions::WEB.heartbeat, Duration::from_secs(25));
        assert_eq!(WATCH_INTERVAL, Duration::from_millis(500));
    }

    struct Rig {
        frames: mpsc::Receiver<Out>,
        sinks: mpsc::Receiver<mpsc::Sender<Value>>,
        cancel: Arc<Notify>,
        job: tokio::task::JoinHandle<()>,
    }

    /// A watch on a paced channel whose watcher hands its sink to the test.
    fn start(rid: u32) -> Rig {
        start_gated(
            rid,
            Arc::new(AtomicBool::new(true)),
            Arc::new(AtomicBool::new(true)),
        )
    }

    /// `start`, with the admission of the device switchable apart for the
    /// serve (checked before each frame) and for the pacer (before a flush).
    fn start_gated(rid: u32, serve_admits: Arc<AtomicBool>, pacer_admits: Arc<AtomicBool>) -> Rig {
        let (out, frames) = mpsc::channel::<Out>(1024);
        let cancel = Arc::new(Notify::new());
        let key = SessionKey {
            device_id: [7; 16],
            channel: 1,
        };
        let channel =
            Channel::for_test(key, json!(rid), json!({"watch": true}), out, cancel.clone());
        let (channel, pacer) = channel.paced(WATCH_INTERVAL, ServeOptions::WEB.heartbeat);
        let pacer = pacer.admitting(move || pacer_admits.load(Ordering::SeqCst));
        let (sink_tx, sinks) = mpsc::channel(1);
        let job = tokio::spawn(async move {
            let serve = serve_stream(
                channel,
                "peer",
                move |_| serve_admits.load(Ordering::SeqCst),
                |_, _| async { Ok(Value::Null) },
                move |_, _, sink| {
                    let sink_tx = sink_tx.clone();
                    async move {
                        sink_tx.send(sink).await.unwrap();
                        Ok(())
                    }
                },
                ServeOptions::WEB,
            );
            tokio::join!(serve, pacer.run());
        });
        Rig {
            frames,
            sinks,
            cancel,
            job,
        }
    }

    fn paused() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .start_paused(true)
            .build()
            .unwrap()
    }

    /// S-02 measurement: one watch, 20 updates a second for 10 simulated seconds.
    #[test]
    fn one_watch_at_20_updates_a_second_sends_at_most_2_frames_a_second() {
        paused().block_on(async {
            let mut rig = start(3);
            let accept = rig.frames.recv().await.unwrap();
            assert_eq!(accept.value, json!({"rid":3,"ok":true,"watching":true}));
            let sink = rig.sinks.recv().await.unwrap();
            let started = Instant::now();
            let collector = tokio::spawn(async move {
                let mut seen = Vec::new();
                while let Some(out) = rig.frames.recv().await {
                    seen.push((started.elapsed(), out.value));
                }
                seen
            });
            let mut produced_bytes = 0usize;
            let mut last_text = String::new();
            for n in 1..=200u32 {
                tokio::time::sleep(Duration::from_millis(50)).await;
                last_text = format!("{}word{n} ", last_text);
                let update = frame("t1", &last_text);
                produced_bytes += json!({"rid":3,"ok":true,"result":update}).to_string().len();
                sink.send(update).await.unwrap();
            }
            drop(sink);
            rig.job.await.unwrap();
            let seen = collector.await.unwrap();
            let updates: Vec<_> = seen
                .iter()
                .filter(|(_, v)| v.get("result").is_some())
                .collect();
            let sent_bytes: usize = updates.iter().map(|(_, v)| v.to_string().len()).sum();
            // Frames are one per slot, and no window of one second holds three.
            for pair in updates.windows(3) {
                assert!(pair[2].0 - pair[0].0 >= Duration::from_secs(1), "{pair:?}");
            }
            assert!(updates.len() <= 21, "{} frames", updates.len());
            assert!(updates.len() >= 19, "{} frames", updates.len());
            // Nothing is lost: the last frame is the last state.
            assert_eq!(
                updates.last().unwrap().1["result"]["turns"][0]["text"],
                json!(last_text)
            );
            println!("S-02 measurement (web path, 1 watch, 20 updates/s for 10 s, simulated time)");
            println!("updates produced: 200, bytes produced: {produced_bytes}");
            println!(
                "update frames sent: {} ({:.1}/s), bytes sent: {sent_bytes}",
                updates.len(),
                updates.len() as f64 / 10.0
            );
            println!(
                "heartbeats sent: {}",
                seen.iter().filter(|(_, v)| v["heartbeat"] == true).count()
            );
            println!(
                "transport frames in total, accept included: {}",
                seen.len() + 1
            );
            println!(
                "reduction vs one frame per update: {:.1}x frames, {:.1}x bytes",
                200.0 / updates.len() as f64,
                produced_bytes as f64 / sent_bytes as f64
            );
        });
    }

    #[test]
    fn frames_that_cannot_merge_are_all_delivered_in_order_in_one_flush() {
        paused().block_on(async {
            let mut rig = start(4);
            rig.frames.recv().await.unwrap();
            let sink = rig.sinks.recv().await.unwrap();
            // The first goes out at once; the next three are held, two cannot merge.
            sink.send(frame("t1", "a")).await.unwrap();
            let first = rig.frames.recv().await.unwrap();
            assert_eq!(first.value["result"]["turns"][0]["text"], "a");
            let t0 = Instant::now();
            for update in [frame("t1", "ab"), patch("t1", 2, "c"), patch("t1", 3, "d")] {
                sink.send(update).await.unwrap();
            }
            let second = rig.frames.recv().await.unwrap();
            let third = rig.frames.recv().await.unwrap();
            assert_eq!(t0.elapsed(), WATCH_INTERVAL);
            assert_eq!(second.value["result"]["turns"][0]["text"], "ab");
            assert_eq!(
                third.value["result"]["patches"][0]["edits"]
                    .as_array()
                    .unwrap()
                    .len(),
                2
            );
            drop(sink);
            rig.job.await.unwrap();
            // The desktop ended the watch: the browser is told (see
            // `a_watch_the_desktop_ends_cleanly_tells_the_browser`).
            assert_eq!(
                rig.frames.try_recv().unwrap().value,
                json!({"rid":4,"ok":false,"error":WATCH_ENDED})
            );
            assert!(rig.frames.try_recv().is_err());
        });
    }

    #[test]
    fn zero_frames_once_the_last_watch_is_cancelled() {
        paused().block_on(async {
            let mut rig = start(5);
            rig.frames.recv().await.unwrap();
            let sink = rig.sinks.recv().await.unwrap();
            sink.send(frame("t1", "a")).await.unwrap();
            rig.frames.recv().await.unwrap();
            // An update is held when the device cancels: it is discarded.
            sink.send(frame("t1", "ab")).await.unwrap();
            rig.cancel.notify_one();
            rig.job.await.unwrap();
            // The host's push side is gone, which the app reports as Gone.
            assert!(sink.is_closed());
            assert!(sink.send(frame("t1", "abc")).await.is_err());
            tokio::time::sleep(Duration::from_secs(60)).await;
            assert!(rig.frames.try_recv().is_err());
            assert!(rig.frames.recv().await.is_none());
        });
    }

    #[test]
    fn aborting_the_task_of_a_closed_session_drops_the_sink_and_sends_nothing() {
        paused().block_on(async {
            let mut rig = start(6);
            rig.frames.recv().await.unwrap();
            let sink = rig.sinks.recv().await.unwrap();
            sink.send(frame("t1", "a")).await.unwrap();
            rig.frames.recv().await.unwrap();
            sink.send(frame("t1", "ab")).await.unwrap();
            tokio::task::yield_now().await;
            rig.job.abort();
            let _ = rig.job.await;
            assert!(sink.is_closed());
            tokio::time::sleep(Duration::from_secs(60)).await;
            assert!(rig.frames.recv().await.is_none());
        });
    }

    #[test]
    fn a_watch_with_no_updates_sends_no_data_frames_only_heartbeats() {
        paused().block_on(async {
            let mut rig = start(7);
            rig.frames.recv().await.unwrap();
            let _sink = rig.sinks.recv().await.unwrap();
            let started = Instant::now();
            let mut beats = 0;
            while started.elapsed() < Duration::from_secs(100) {
                let out = tokio::time::timeout(Duration::from_secs(101), rig.frames.recv())
                    .await
                    .unwrap()
                    .unwrap();
                assert_eq!(out.value["heartbeat"], true);
                beats += 1;
            }
            assert_eq!(beats, 4);
            rig.job.abort();
        });
    }

    #[test]
    fn a_heartbeat_is_skipped_while_data_went_out_within_the_period() {
        paused().block_on(async {
            let mut rig = start(8);
            rig.frames.recv().await.unwrap();
            let sink = rig.sinks.recv().await.unwrap();
            // A frame every 10 s for 100 s: data inside every 25 s window.
            let started = Instant::now();
            let mut seen = Vec::new();
            for n in 0..10 {
                sink.send(frame("t1", &n.to_string())).await.unwrap();
                tokio::time::sleep(Duration::from_secs(10)).await;
                while let Ok(out) = rig.frames.try_recv() {
                    seen.push(out.value);
                }
            }
            assert_eq!(seen.len(), 10);
            assert!(
                seen.iter().all(|v| v.get("heartbeat").is_none()),
                "{:?}",
                started.elapsed()
            );
            rig.job.abort();
        });
    }

    #[test]
    fn a_question_transition_is_never_merged_away() {
        // Answered (null), then a new question with the same request id.
        let q = |value: Value| {
            let mut f = json!({"usageLimit":null,"busy":true,"older":false,"order":[],"turns":[]});
            f["question"] = value;
            f
        };
        let mut held = q(Value::Null);
        let before = held.clone();
        assert!(!merge(&mut held, &q(json!({"requestId":"r1"}))));
        assert_eq!(held, before);
        let mut held = q(json!({"requestId":"r1"}));
        assert!(!merge(&mut held, &q(json!({"requestId":"r2"}))));
        // A frame with no question still merges into one that has it.
        let mut quiet = frame("t1", "a");
        quiet["question"] = json!({"requestId":"r1"});
        assert!(merge(&mut quiet, &frame("t1", "ab")));
        assert_eq!(quiet["question"], json!({"requestId":"r1"}));
    }

    #[test]
    fn an_answered_then_new_question_within_a_slot_arrives_as_two_frames_in_order() {
        paused().block_on(async {
            let mut rig = start(10);
            rig.frames.recv().await.unwrap();
            let sink = rig.sinks.recv().await.unwrap();
            sink.send(frame("t1", "a")).await.unwrap();
            rig.frames.recv().await.unwrap();
            let t0 = Instant::now();
            let mut answered = frame("t1", "a");
            answered["question"] = Value::Null;
            let mut asked = frame("t1", "a");
            asked["question"] = json!({"requestId":"r1"});
            sink.send(answered).await.unwrap();
            sink.send(asked).await.unwrap();
            let first = rig.frames.recv().await.unwrap();
            let second = rig.frames.recv().await.unwrap();
            assert_eq!(t0.elapsed(), WATCH_INTERVAL);
            assert_eq!(first.value["result"]["question"], Value::Null);
            assert_eq!(
                second.value["result"]["question"],
                json!({"requestId":"r1"})
            );
            rig.job.abort();
        });
    }

    #[test]
    fn a_watch_the_desktop_ends_cleanly_tells_the_browser() {
        paused().block_on(async {
            let mut rig = start(11);
            rig.frames.recv().await.unwrap();
            let sink = rig.sinks.recv().await.unwrap();
            sink.send(frame("t1", "a")).await.unwrap();
            rig.frames.recv().await.unwrap();
            // An update is still held when the host ends the watch: it goes
            // out first, then the end.
            sink.send(frame("t1", "ab")).await.unwrap();
            drop(sink);
            rig.job.await.unwrap();
            let held = rig.frames.recv().await.unwrap();
            assert_eq!(held.value["result"]["turns"][0]["text"], "ab");
            let ended = rig.frames.recv().await.unwrap();
            assert_eq!(
                ended.value,
                json!({"rid":11,"ok":false,"error":"Watch ended"})
            );
            assert!(rig.frames.recv().await.is_none());
        });
    }

    #[test]
    fn a_watch_that_ended_with_an_error_sends_no_second_ending() {
        paused().block_on(async {
            let serve = Arc::new(AtomicBool::new(true));
            let mut rig = start_gated(12, serve.clone(), Arc::new(AtomicBool::new(true)));
            rig.frames.recv().await.unwrap();
            let sink = rig.sinks.recv().await.unwrap();
            sink.send(frame("t1", "a")).await.unwrap();
            rig.frames.recv().await.unwrap();
            // Revoked: the next update is answered with the error and the watch ends.
            serve.store(false, Ordering::SeqCst);
            sink.send(frame("t1", "ab")).await.unwrap();
            rig.job.await.unwrap();
            assert_eq!(
                rig.frames.recv().await.unwrap().value,
                json!({"rid":12,"ok":false,"error":"Device revoked"})
            );
            assert!(rig.frames.recv().await.is_none());
        });
    }

    #[test]
    fn updates_held_before_a_revoke_are_dropped_not_flushed_after_it() {
        paused().block_on(async {
            let serve = Arc::new(AtomicBool::new(true));
            let mut rig = start_gated(13, serve.clone(), Arc::new(AtomicBool::new(true)));
            rig.frames.recv().await.unwrap();
            let sink = rig.sinks.recv().await.unwrap();
            sink.send(frame("t1", "a")).await.unwrap();
            rig.frames.recv().await.unwrap();
            // Held (not yet flushed), then the device is revoked.
            sink.send(frame("t1", "ab")).await.unwrap();
            tokio::task::yield_now().await;
            serve.store(false, Ordering::SeqCst);
            sink.send(frame("t1", "abc")).await.unwrap();
            let error = rig.frames.recv().await.unwrap();
            assert_eq!(error.value["error"], "Device revoked");
            rig.job.await.unwrap();
            // Nothing held was flushed behind the error.
            assert!(rig.frames.recv().await.is_none());
        });
    }

    #[test]
    fn a_flush_for_a_device_that_is_no_longer_admitted_sends_nothing() {
        paused().block_on(async {
            let pacer = Arc::new(AtomicBool::new(true));
            let mut rig = start_gated(14, Arc::new(AtomicBool::new(true)), pacer.clone());
            rig.frames.recv().await.unwrap();
            let sink = rig.sinks.recv().await.unwrap();
            sink.send(frame("t1", "a")).await.unwrap();
            rig.frames.recv().await.unwrap();
            sink.send(frame("t1", "ab")).await.unwrap();
            tokio::task::yield_now().await;
            pacer.store(false, Ordering::SeqCst);
            tokio::time::sleep(WATCH_INTERVAL * 2).await;
            assert!(rig.frames.try_recv().is_err(), "a held update was flushed");
            rig.job.abort();
        });
    }

    #[test]
    fn silence_on_a_watch_never_exceeds_the_heartbeat_period() {
        paused().block_on(async {
            let mut rig = start(15);
            rig.frames.recv().await.unwrap();
            let sink = rig.sinks.recv().await.unwrap();
            let started = Instant::now();
            tokio::time::sleep(Duration::from_secs(1)).await;
            sink.send(frame("t1", "a")).await.unwrap();
            let data = rig.frames.recv().await.unwrap();
            assert_eq!(data.value["result"]["turns"][0]["text"], "a");
            // The heartbeat tick at 25 s sees data from 1 s ago and holds back,
            // so the beat goes out 25 s after the data, not at 50 s.
            let beat = rig.frames.recv().await.unwrap();
            assert_eq!(beat.value["heartbeat"], true);
            assert_eq!(started.elapsed(), Duration::from_secs(26));
            // And it goes on: the next beat is 25 s after this one at most.
            let next = rig.frames.recv().await.unwrap();
            assert_eq!(next.value["heartbeat"], true);
            assert!(started.elapsed() <= Duration::from_secs(51));
            rig.job.abort();
        });
    }
}
