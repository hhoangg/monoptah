//! The web link's side of the shared serve: a request id on a Noise session is
//! one [`Duplex`] stream, and [`Requests`] decides how the streams of a session
//! run (architecture 6.1).
//!
//! Per session, control-class requests (and any request whose method is not in
//! the policy table) run strictly in arrival order on one queue, so commands
//! keep their order. Read-class requests and watches run concurrently. A
//! `{"rid":N,"cancel":true}` envelope resolves the `closed` future of that
//! stream, which ends a watch.
use super::session::SessionKey;
use crate::mobile::host::MobileHost;
use crate::mobile::policy::{self, Access};
use monocode_mobile_link::{serve_stream, Duplex, ServeOptions};
use serde_json::{json, Map, Value};
use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
};
use tokio::{
    sync::{mpsc, Notify},
    task::JoinHandle,
};

/// Requests and watches in flight on one relay connection.
pub(super) const MAX_IN_FLIGHT: usize = 64;

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
}

impl Channel {
    fn wrap(&self, value: &Value) -> Value {
        let mut wrapped = Map::new();
        wrapped.insert("rid".into(), self.rid.clone());
        if let Some(fields) = value.as_object() {
            for (name, field) in fields {
                wrapped.insert(name.clone(), field.clone());
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
        self.out
            .send(Out {
                key: self.key,
                value,
            })
            .await
            .map_err(|_| "Web link closed".to_string())
    }
    fn finish(&mut self) -> Result<(), String> {
        // The final frame ends the request on the web side.
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
    concurrent: HashMap<Key, JoinHandle<()>>,
    queues: HashMap<SessionKey, Queue>,
}

impl Drop for Requests {
    fn drop(&mut self) {
        for task in self.concurrent.values() {
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
        }
    }

    /// The requests and watches of a session end with it.
    pub(super) fn closed(&mut self, key: SessionKey) {
        self.concurrent.retain(|(owner, _), task| {
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
        self.concurrent.retain(|_, task| !task.is_finished());
        let notify = Arc::new(Notify::new());
        {
            let Ok(mut live) = self.live.lock() else {
                return;
            };
            // A request id already in flight is the same request again.
            if live.contains_key(&(key, id.clone())) {
                return;
            }
            if live.len() >= MAX_IN_FLIGHT {
                drop(live);
                let busy = json!({"rid": rid, "ok": false, "error": "Desktop is busy; try again"});
                let _ = self.out.try_send(Out { key, value: busy });
                return;
            }
            live.insert((key, id.clone()), notify.clone());
        }
        let in_flight = InFlight {
            live: self.live.clone(),
            key: (key, id.clone()),
        };
        // A watch only reads. Anything the policy table does not know counts
        // as control, so it keeps its place in the queue.
        let concurrent = envelope["watch"] == true
            || matches!(
                policy::classify(
                    envelope["method"].as_str().unwrap_or(""),
                    &envelope["params"]
                ),
                Some(Access::Read)
            );
        let channel = Channel {
            key,
            rid,
            request: Some(envelope),
            out: self.out.clone(),
            cancel: notify,
        };
        let host = self.host.clone();
        let job: Job = Box::pin(async move {
            let _in_flight = in_flight;
            serve_request(host, peer, channel).await;
        });
        if concurrent {
            self.concurrent.insert((key, id), tokio::spawn(job));
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
