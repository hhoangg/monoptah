//! The web link's side of the shared serve: a request id on a Noise session is
//! one [`Duplex`] stream, and [`Requests`] decides how the streams of a session
//! run (architecture 6.1).
//!
//! Per session, control-class requests (and any request whose method is not in
//! the policy table) run strictly in arrival order on one queue, so commands
//! keep their order. Read-class requests and watches run concurrently. A
//! `{"rid":N,"cancel":true}` envelope resolves the `closed` future of that
//! stream, which ends a watch.
use super::pacer::{Msg, Pacer, QUEUE, WATCH_INTERVAL};
use super::session::SessionKey;
use crate::mobile::host::MobileHost;
use crate::mobile::policy::{self, Access};
use monocode_mobile_link::{serve_stream, Duplex, ServeOptions};
use serde_json::{json, Map, Value};
use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};
use tokio::{
    sync::{mpsc, Notify},
    task::JoinHandle,
    time::Duration,
};

/// Requests and watches in flight on one relay connection.
pub(super) const MAX_IN_FLIGHT: usize = 64;
/// Requests and watches in flight on one session, so one browser cannot use
/// up the connection's budget.
pub(super) const MAX_IN_FLIGHT_PER_SESSION: usize = 16;
/// Read requests running at once on one session (watches are limited by the
/// host, per device).
pub(super) const MAX_READS_PER_SESSION: usize = 4;

const BUSY: &str = "Desktop is busy; try again";
const UNSUPPORTED: &str = "Unsupported mobile operation";

/// The stable `code` of an error the host words in plain text. The host's
/// error path carries text only, so the web channel adds the code the browser
/// keys on (additive: the text stays, and QUIC is unchanged).
fn code_of(error: &str) -> Option<&'static str> {
    match error {
        BUSY => Some("busy"),
        UNSUPPORTED => Some("unsupported"),
        _ => None,
    }
}

/// The answer that refuses a request because the desktop or the session is busy.
fn busy(rid: Value) -> Value {
    json!({"rid": rid, "ok": false, "error": BUSY, "code": "busy"})
}

/// A reply to send on a session, from a request task.
pub(super) struct Out {
    pub(super) key: SessionKey,
    pub(super) value: Value,
}

/// One request id of one session. Every frame it writes carries the `rid`.
pub(crate) struct Channel {
    key: SessionKey,
    rid: Value,
    request: Option<Value>,
    out: mpsc::Sender<Out>,
    cancel: Arc<Notify>,
    /// Set for a watch: its frames go through the pacer (R-11), not straight out.
    pacer: Option<mpsc::Sender<Msg>>,
    finished: Arc<AtomicBool>,
}

impl Channel {
    #[cfg(test)]
    pub(super) fn for_test(
        key: SessionKey,
        rid: Value,
        request: Value,
        out: mpsc::Sender<Out>,
        cancel: Arc<Notify>,
    ) -> Self {
        Self {
            key,
            rid,
            request: Some(request),
            out,
            cancel,
            pacer: None,
            finished: Arc::default(),
        }
    }

    /// A channel for a watch, and the pacer that must run next to its serve.
    pub(super) fn paced(mut self, interval: Duration, heartbeat: Duration) -> (Self, Pacer) {
        let (pace, input) = mpsc::channel(QUEUE);
        let pacer = Pacer {
            key: self.key,
            rid: self.rid.clone(),
            admit: Arc::new(|| true),
            input,
            finished: self.finished.clone(),
            out: self.out.clone(),
            interval,
            heartbeat,
        };
        self.pacer = Some(pace);
        (self, pacer)
    }

    fn wrap(&self, value: &Value) -> Value {
        let mut wrapped = Map::new();
        wrapped.insert("rid".into(), self.rid.clone());
        if let Some(fields) = value.as_object() {
            for (name, field) in fields {
                wrapped.insert(name.clone(), field.clone());
            }
        }
        if wrapped.get("ok") == Some(&Value::Bool(false)) && !wrapped.contains_key("code") {
            if let Some(code) = wrapped
                .get("error")
                .and_then(Value::as_str)
                .and_then(code_of)
            {
                wrapped.insert("code".into(), code.into());
            }
        }
        Value::Object(wrapped)
    }
}

impl Duplex for Channel {
    async fn read_request(&mut self, _limit: usize) -> Result<Value, String> {
        // The session already decrypted and bounded the envelope.
        self.request
            .take()
            .ok_or_else(|| "Request already read".into())
    }
    async fn write(&mut self, value: &Value, _limit: usize) -> Result<(), String> {
        // The session table enforces the frame size and answers an oversized
        // reply with "Response too large".
        let value = self.wrap(value);
        if let Some(pacer) = &self.pacer {
            let message = if value["heartbeat"] == true {
                Msg::Heartbeat(value)
            } else if value["ok"] == true && value.get("result").is_some() {
                Msg::Update(value)
            } else {
                Msg::Control(value)
            };
            return pacer
                .send(message)
                .await
                .map_err(|_| "Web link closed".to_string());
        }
        self.out
            .send(Out {
                key: self.key,
                value,
            })
            .await
            .map_err(|_| "Web link closed".to_string())
    }
    fn finish(&mut self) -> Result<(), String> {
        // The final frame ends the request on the web side. A watch lets the
        // pacer send what it still holds.
        self.finished.store(true, Ordering::SeqCst);
        Ok(())
    }
    fn closed(&mut self) -> Pin<Box<dyn Future<Output = ()> + Send>> {
        let cancel = self.cancel.clone();
        Box::pin(async move { cancel.notified().await })
    }
}

type Job = Pin<Box<dyn Future<Output = ()> + Send>>;
type Key = (SessionKey, String);
type Live = Arc<Mutex<HashMap<Key, Arc<Notify>>>>;

/// Removes a request from the in-flight table once it ends, however it ends.
struct InFlight {
    live: Live,
    key: Key,
}
impl Drop for InFlight {
    fn drop(&mut self) {
        if let Ok(mut live) = self.live.lock() {
            live.remove(&self.key);
        }
    }
}

struct Queue {
    jobs: mpsc::UnboundedSender<Job>,
    task: JoinHandle<()>,
}
impl Queue {
    fn new() -> Self {
        let (jobs, mut waiting) = mpsc::unbounded_channel::<Job>();
        let task = tokio::spawn(async move {
            while let Some(job) = waiting.recv().await {
                job.await;
            }
        });
        Self { jobs, task }
    }
}

/// Serves the envelopes the sessions of one relay connection deliver.
pub(super) struct Requests {
    host: MobileHost,
    out: mpsc::Sender<Out>,
    live: Live,
    /// Running concurrent jobs; the flag marks a plain read (not a watch).
    concurrent: HashMap<Key, (JoinHandle<()>, bool)>,
    queues: HashMap<SessionKey, Queue>,
    /// Requests ended by `cancel` from the relay side: frames still queued for
    /// them are dropped.
    ended: std::collections::HashSet<Key>,
}

impl Drop for Requests {
    fn drop(&mut self) {
        for (task, _) in self.concurrent.values() {
            task.abort();
        }
        for queue in self.queues.values() {
            queue.task.abort();
        }
    }
}

impl Requests {
    pub(super) fn new(host: MobileHost, out: mpsc::Sender<Out>) -> Self {
        Self {
            host,
            out,
            live: Live::default(),
            concurrent: HashMap::new(),
            queues: HashMap::new(),
            ended: std::collections::HashSet::new(),
        }
    }

    /// Whether `rid` of a session was ended by [`Requests::cancel`].
    pub(super) fn is_ended(&self, key: SessionKey, rid: &Value) -> bool {
        self.ended.contains(&(key, rid.to_string()))
    }

    /// The requests and watches of a session end with it.
    pub(super) fn closed(&mut self, key: SessionKey) {
        self.concurrent.retain(|(owner, _), (task, _)| {
            let keep = *owner != key;
            if !keep {
                task.abort();
            }
            keep
        });
        if let Some(queue) = self.queues.remove(&key) {
            queue.task.abort();
        }
        if let Ok(mut live) = self.live.lock() {
            live.retain(|(owner, _), _| *owner != key);
        }
        self.ended.retain(|(owner, _)| *owner != key);
    }

    /// Ends the request `rid` of a session, as a browser's cancel does. A
    /// watch stops and drops its sink; for a request that ended already it
    /// does nothing.
    pub(super) fn cancel(&mut self, key: SessionKey, rid: &Value) {
        self.ended.insert((key, rid.to_string()));
        let notify = self
            .live
            .lock()
            .ok()
            .and_then(|live| live.get(&(key, rid.to_string())).cloned());
        if let Some(notify) = notify {
            notify.notify_one();
        }
    }

    pub(super) fn deliver(&mut self, key: SessionKey, peer: String, mut envelope: Value) {
        let Some(object) = envelope.as_object_mut() else {
            return;
        };
        let Some(rid) = object.remove("rid") else {
            return;
        };
        let id = rid.to_string();
        let cancel = object.remove("cancel").is_some_and(|v| v == true);
        if cancel {
            let notify = self
                .live
                .lock()
                .ok()
                .and_then(|live| live.get(&(key, id)).cloned());
            if let Some(notify) = notify {
                notify.notify_one();
            }
            return;
        }
        // A new request under an id that was ended starts clean.
        self.ended.remove(&(key, id.clone()));
        self.concurrent.retain(|_, (task, _)| !task.is_finished());
        // A watch only reads. Anything the policy table does not know counts
        // as control, so it keeps its place in the queue.
        let watch = envelope["watch"] == true;
        let read = !watch
            && matches!(
                policy::classify(
                    envelope["method"].as_str().unwrap_or(""),
                    &envelope["params"]
                ),
                Some(Access::Read)
            );
        let concurrent = watch || read;
        let notify = Arc::new(Notify::new());
        {
            let Ok(mut live) = self.live.lock() else {
                return;
            };
            // A request id already in flight is the same request again.
            if live.contains_key(&(key, id.clone())) {
                return;
            }
            let on_session = live.keys().filter(|(owner, _)| *owner == key).count();
            let reads = self
                .concurrent
                .iter()
                .filter(|(k, (_, is_read))| k.0 == key && *is_read && live.contains_key(*k))
                .count();
            if live.len() >= MAX_IN_FLIGHT
                || on_session >= MAX_IN_FLIGHT_PER_SESSION
                || (read && reads >= MAX_READS_PER_SESSION)
            {
                drop(live);
                let _ = self.out.try_send(Out {
                    key,
                    value: busy(rid),
                });
                return;
            }
            live.insert((key, id.clone()), notify.clone());
        }
        let in_flight = InFlight {
            live: self.live.clone(),
            key: (key, id.clone()),
        };
        let channel = Channel {
            key,
            rid,
            request: Some(envelope),
            out: self.out.clone(),
            cancel: notify,
            pacer: None,
            finished: Arc::default(),
        };
        let host = self.host.clone();
        let job: Job = if watch {
            let (channel, pacer) = channel.paced(WATCH_INTERVAL, ServeOptions::WEB.heartbeat);
            let admits = self.host.clone();
            let owner = peer.clone();
            let pacer = pacer.admitting(move || admits.web_admits(&owner));
            Box::pin(async move {
                let _in_flight = in_flight;
                // One task: ending it (session closed) ends the pacer with the
                // watch, and the pacer ends when the watch drops its channel.
                tokio::join!(serve_request(host, peer, channel), pacer.run());
            })
        } else {
            Box::pin(async move {
                let _in_flight = in_flight;
                serve_request(host, peer, channel).await;
            })
        };
        if concurrent {
            self.concurrent.insert((key, id), (tokio::spawn(job), read));
        } else {
            self.enqueue(key, job);
        }
    }

    fn enqueue(&mut self, key: SessionKey, job: Job) {
        let queue = self.queues.entry(key).or_insert_with(Queue::new);
        if let Err(failed) = queue.jobs.send(job) {
            // The worker is gone (a panic in a handler): start a new one.
            let fresh = Queue::new();
            let _ = fresh.jobs.send(failed.0);
            self.queues.insert(key, fresh);
        }
    }
}

async fn serve_request(host: MobileHost, peer: String, channel: Channel) {
    let (admits, handler, watcher) = (host.clone(), host.clone(), host);
    serve_stream(
        channel,
        &peer,
        move |peer| admits.web_admits(peer),
        move |peer, request| {
            let host = handler.clone();
            async move { host.handle_web(&peer, request).await }
        },
        move |peer, request, sink| {
            let host = watcher.clone();
            async move { host.watch_web(&peer, request, sink).await }
        },
        ServeOptions::WEB,
    )
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::time::Instant;

    /// A web watch beats every 25 s, not the QUIC 10 s, and its frames carry the rid.
    #[test]
    fn a_web_watch_heartbeats_every_25_seconds_with_its_rid() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .start_paused(true)
            .build()
            .unwrap();
        rt.block_on(async {
            let (out, mut frames) = mpsc::channel::<Out>(8);
            let key = SessionKey {
                device_id: [7; 16],
                channel: 1,
            };
            let channel = Channel {
                key,
                rid: json!(5),
                request: Some(json!({"watch": true})),
                out,
                cancel: Arc::new(Notify::new()),
                pacer: None,
                finished: Arc::default(),
            };
            let task = tokio::spawn(serve_stream(
                channel,
                "peer",
                |_| true,
                |_, _| async { Ok(Value::Null) },
                |_, _, sink| async move {
                    // Held open and idle: nothing is ever sent.
                    tokio::spawn(async move {
                        sink.closed().await;
                    });
                    Ok(())
                },
                ServeOptions::WEB,
            ));
            let first = frames.recv().await.unwrap();
            assert_eq!(first.value, json!({"rid":5,"ok":true,"watching":true}));
            let started = Instant::now();
            let beat = frames.recv().await.unwrap();
            assert_eq!(beat.value, json!({"rid":5,"ok":true,"heartbeat":true}));
            assert_eq!(started.elapsed(), Duration::from_secs(25));
            task.abort();
        });
    }
}
