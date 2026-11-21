//! What a request stream does after the handshake, over any transport that can
//! carry one request and its answers: a QUIC bidirectional stream, or one
//! request id on an encrypted relay channel. The wire format of each lives in
//! its [`Duplex`] impl; the rules (response wrapping, the watch accept frame,
//! heartbeats, the gate recheck before the request and before every frame, the
//! handler timeout) live here once.
use crate::transport::{WatchSink, WATCH_HEARTBEAT};
use crate::{Failure, LinkResult, REQUEST_LIMIT, RESPONSE_LIMIT};
use serde_json::{json, Value};
use std::{future::Future, pin::Pin, time::Duration};
use tokio::{sync::mpsc, time::timeout};

/// One request stream: the request in, the answers out.
pub trait Duplex: Send + 'static {
    /// Reads the request, at most `limit` bytes.
    fn read_request(&mut self, limit: usize) -> impl Future<Output = LinkResult<Value>> + Send;
    /// Writes a frame on a stream that stays open for more.
    fn write(&mut self, value: &Value, limit: usize)
        -> impl Future<Output = LinkResult<()>> + Send;
    /// Ends the answers.
    fn finish(&mut self) -> LinkResult<()>;
    /// A future that resolves once the peer closed its side, which stops a watch.
    /// Taken once, after the request was read.
    fn closed(&mut self) -> Pin<Box<dyn Future<Output = ()> + Send>>;
    /// Writes the last frame and ends the answers.
    fn write_final(
        &mut self,
        value: &Value,
        limit: usize,
    ) -> impl Future<Output = LinkResult<()>> + Send {
        async move {
            self.write(value, limit).await?;
            self.finish()
        }
    }
}

/// The time rules of one transport.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServeOptions {
    /// How often an idle watch proves the desktop is still there.
    pub heartbeat: Duration,
    /// How long the handler (or the watcher's first answer) may take.
    pub handler_timeout: Duration,
    /// How long one frame may take to be written.
    pub write_timeout: Duration,
}
impl ServeOptions {
    /// Direct QUIC: the iOS app's rules.
    pub const QUIC: Self = Self {
        heartbeat: WATCH_HEARTBEAT,
        handler_timeout: Duration::from_secs(30),
        write_timeout: Duration::from_secs(10),
    };
    /// The web link: a slower heartbeat keeps the relay's message count low.
    pub const WEB: Self = Self {
        heartbeat: Duration::from_secs(25),
        handler_timeout: Duration::from_secs(30),
        write_timeout: Duration::from_secs(10),
    };
}

fn frame(result: Result<Value, Failure>) -> Value {
    match result {
        Ok(value) => json!({"ok":true,"result":value}),
        Err(Failure {
            error, code: None, ..
        }) => json!({"ok":false,"error":error}),
        Err(Failure {
            error,
            code: Some(code),
            retry_after: None,
        }) => json!({"ok":false,"error":error,"code":code}),
        Err(Failure {
            error,
            code: Some(code),
            retry_after: Some(seconds),
        }) => json!({"ok":false,"error":error,"code":code,"retryAfter":seconds}),
    }
}

/// Serves one request stream to its end: a request is answered once, a watch
/// keeps the stream open until its sink is dropped, the peer closes, or the
/// device is no longer admitted. `admits` is asked about `peer` before the
/// request is handled and before every frame of a watch.
pub async fn serve_stream<S, A, F, Fut, E, W, WFut, WE>(
    stream: S,
    peer: &str,
    admits: A,
    handler: F,
    watcher: W,
    options: ServeOptions,
) where
    S: Duplex,
    A: Fn(&str) -> bool + Send + 'static,
    F: Fn(String, Value) -> Fut + Send + Sync,
    Fut: Future<Output = Result<Value, E>> + Send,
    E: Into<Failure>,
    W: Fn(String, Value, WatchSink) -> WFut + Send + Sync,
    WFut: Future<Output = Result<(), WE>> + Send,
    WE: Into<Failure>,
{
    if let Accepted::Watch(run) = accept(stream, peer, admits, &handler, &watcher, options).await {
        run.run().await;
    }
}

/// What is left of a stream once its request was read.
pub(crate) enum Accepted<S, A, WFut> {
    /// A request, answered.
    Finished,
    /// A watch to run, apart from the caller's other requests.
    Watch(WatchRun<S, A, WFut>),
}

/// Reads the request and, unless it is a watch, answers it before returning:
/// a caller that awaits this serves its requests in order.
pub(crate) async fn accept<S, A, F, Fut, E, W, WFut, WE>(
    mut stream: S,
    peer: &str,
    admits: A,
    handler: &F,
    watcher: &W,
    options: ServeOptions,
) -> Accepted<S, A, WFut>
where
    S: Duplex,
    A: Fn(&str) -> bool + Send + 'static,
    F: Fn(String, Value) -> Fut + Send + Sync,
    Fut: Future<Output = Result<Value, E>> + Send,
    E: Into<Failure>,
    W: Fn(String, Value, WatchSink) -> WFut + Send + Sync,
    WFut: Future<Output = Result<(), WE>> + Send,
    WE: Into<Failure>,
{
    let request = stream
        .read_request(REQUEST_LIMIT)
        .await
        .and_then(|request| {
            if admits(peer) {
                Ok(request)
            } else {
                Err("Device revoked".into())
            }
        });
    if let Some(request) = request
        .as_ref()
        .ok()
        .filter(|request| request["watch"] == true)
    {
        let (sink, updates) = mpsc::channel(64);
        let watching = watcher(peer.to_string(), request.clone(), sink);
        return Accepted::Watch(WatchRun {
            stream,
            updates,
            watching,
            admits,
            peer: peer.to_string(),
            options,
        });
    }
    let response = match request {
        Ok(request) => timeout(options.handler_timeout, handler(peer.to_string(), request))
            .await
            .map_or_else(
                |_| Err("Desktop request timed out".into()),
                |result| result.map_err(Into::into),
            ),
        Err(error) => Err(error.into()),
    };
    let _ = timeout(
        options.write_timeout,
        stream.write_final(&frame(response), RESPONSE_LIMIT),
    )
    .await;
    Accepted::Finished
}

/// An accepted watch, not yet answered.
pub(crate) struct WatchRun<S, A, WFut> {
    stream: S,
    updates: mpsc::Receiver<Value>,
    watching: WFut,
    admits: A,
    peer: String,
    options: ServeOptions,
}
impl<S, A, WFut, WE> WatchRun<S, A, WFut>
where
    S: Duplex,
    A: Fn(&str) -> bool + Send + 'static,
    WFut: Future<Output = Result<(), WE>> + Send,
    WE: Into<Failure>,
{
    pub(crate) async fn run(self) {
        let Self {
            mut stream,
            mut updates,
            watching,
            admits,
            peer,
            options,
        } = self;
        // The watcher answers first, as a request would: refused, or watching.
        if let Err(error) = timeout(options.handler_timeout, watching)
            .await
            .map_or_else(
                |_| Err("Desktop request timed out".into()),
                |result: Result<(), WE>| result.map_err(Into::into),
            )
        {
            let _ = timeout(
                options.write_timeout,
                stream.write_final(&frame(Err(error)), RESPONSE_LIMIT),
            )
            .await;
            return;
        }
        if stream
            .write(&json!({"ok":true,"watching":true}), RESPONSE_LIMIT)
            .await
            .is_err()
        {
            return;
        }
        // The peer closes its side to stop watching.
        let mut closed = stream.closed();
        let mut heartbeat = tokio::time::interval(options.heartbeat);
        heartbeat.tick().await;
        loop {
            let value = tokio::select! {
                update = updates.recv() => match update {
                    Some(value) => frame(Ok(value)),
                    None => break,
                },
                _ = heartbeat.tick() => json!({"ok":true,"heartbeat":true}),
                _ = &mut closed => return,
            };
            if !admits(&peer) {
                let _ = stream
                    .write_final(&frame(Err("Device revoked".into())), RESPONSE_LIMIT)
                    .await;
                return;
            }
            let written =
                timeout(options.write_timeout, stream.write(&value, RESPONSE_LIMIT)).await;
            if !matches!(written, Ok(Ok(()))) {
                return;
            }
        }
        let _ = stream.finish();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_coded_failure_writes_its_retry_after_and_a_plain_one_does_not() {
        let waiting = Failure::coded("busy", "Desktop is busy; try again").retry_after(30);
        assert_eq!(
            frame(Err(waiting)),
            json!({"ok":false,"error":"Desktop is busy; try again","code":"busy","retryAfter":30})
        );
        assert_eq!(
            frame(Err(Failure::coded("busy", "x"))),
            json!({"ok":false,"error":"x","code":"busy"})
        );
        assert_eq!(
            frame(Err(Failure::from("plain"))),
            json!({"ok":false,"error":"plain"})
        );
    }
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    };
    use tokio::sync::Notify;

    /// A stream in memory: a request to read, the frames written, an end flag.
    struct Memory {
        request: Option<LinkResult<Value>>,
        written: Arc<Mutex<Vec<Value>>>,
        finished: Arc<AtomicBool>,
        wrote: Arc<Notify>,
        close: Arc<Notify>,
    }
    impl Duplex for Memory {
        async fn read_request(&mut self, _: usize) -> LinkResult<Value> {
            self.request.take().expect("request read once")
        }
        async fn write(&mut self, value: &Value, _: usize) -> LinkResult<()> {
            self.written.lock().unwrap().push(value.clone());
            self.wrote.notify_one();
            Ok(())
        }
        fn finish(&mut self) -> LinkResult<()> {
            self.finished.store(true, Ordering::SeqCst);
            Ok(())
        }
        fn closed(&mut self) -> Pin<Box<dyn Future<Output = ()> + Send>> {
            let close = self.close.clone();
            Box::pin(async move { close.notified().await })
        }
    }

    struct Rig {
        written: Arc<Mutex<Vec<Value>>>,
        finished: Arc<AtomicBool>,
        wrote: Arc<Notify>,
        close: Arc<Notify>,
    }
    fn memory(request: LinkResult<Value>) -> (Memory, Rig) {
        let rig = Rig {
            written: Arc::default(),
            finished: Arc::default(),
            wrote: Arc::default(),
            close: Arc::default(),
        };
        let stream = Memory {
            request: Some(request),
            written: rig.written.clone(),
            finished: rig.finished.clone(),
            wrote: rig.wrote.clone(),
            close: rig.close.clone(),
        };
        (stream, rig)
    }
    async fn no_watch(_: String, _: Value, _: WatchSink) -> LinkResult<()> {
        Err("Unsupported mobile operation".into())
    }
    const FAST: ServeOptions = ServeOptions {
        heartbeat: Duration::from_millis(30),
        handler_timeout: Duration::from_millis(100),
        write_timeout: Duration::from_millis(100),
    };

    #[test]
    fn the_presets_carry_each_transports_heartbeat() {
        assert_eq!(ServeOptions::QUIC.heartbeat, Duration::from_secs(10));
        assert_eq!(ServeOptions::WEB.heartbeat, Duration::from_secs(25));
        for options in [ServeOptions::QUIC, ServeOptions::WEB] {
            assert_eq!(options.handler_timeout, Duration::from_secs(30));
            assert_eq!(options.write_timeout, Duration::from_secs(10));
        }
    }

    #[tokio::test]
    async fn a_request_is_wrapped_and_the_stream_ends() {
        let (stream, rig) = memory(Ok(json!({"method":"x"})));
        serve_stream(
            stream,
            "peer",
            |_| true,
            |peer, request| async move {
                Ok::<_, String>(json!({"peer":peer,"echo":request["method"]}))
            },
            no_watch,
            FAST,
        )
        .await;
        assert_eq!(
            *rig.written.lock().unwrap(),
            vec![json!({"ok":true,"result":{"peer":"peer","echo":"x"}})]
        );
        assert!(rig.finished.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn a_revoked_peer_never_reaches_the_handler() {
        let (stream, rig) = memory(Ok(json!({})));
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        serve_stream(
            stream,
            "peer",
            |_| false,
            move |_, _| {
                count.fetch_add(1, Ordering::SeqCst);
                async { Ok::<_, String>(Value::Null) }
            },
            no_watch,
            FAST,
        )
        .await;
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            *rig.written.lock().unwrap(),
            vec![json!({"ok":false,"error":"Device revoked"})]
        );
    }

    #[tokio::test]
    async fn a_slow_handler_times_out_with_the_quic_message() {
        let (stream, rig) = memory(Ok(json!({})));
        serve_stream(
            stream,
            "peer",
            |_| true,
            |_, _| async {
                tokio::time::sleep(Duration::from_secs(5)).await;
                Ok::<_, String>(Value::Null)
            },
            no_watch,
            FAST,
        )
        .await;
        assert_eq!(
            *rig.written.lock().unwrap(),
            vec![json!({"ok":false,"error":"Desktop request timed out"})]
        );
    }

    /// A text error writes today's exact bytes; a coded failure adds `code`.
    #[tokio::test]
    async fn a_failure_writes_code_only_when_it_has_one() {
        let (stream, rig) = memory(Ok(json!({})));
        serve_stream(
            stream,
            "peer",
            |_| true,
            |_, _| async { Err::<Value, String>("Plain".into()) },
            no_watch,
            FAST,
        )
        .await;
        assert_eq!(
            serde_json::to_string(&rig.written.lock().unwrap()[0]).unwrap(),
            serde_json::to_string(&json!({"ok":false,"error":"Plain"})).unwrap()
        );
        assert!(rig.written.lock().unwrap()[0].get("code").is_none());

        let (stream, rig) = memory(Ok(json!({})));
        serve_stream(
            stream,
            "peer",
            |_| true,
            |_, _| async { Err::<Value, Failure>(Failure::coded("busy", "Later")) },
            no_watch,
            FAST,
        )
        .await;
        assert_eq!(
            rig.written.lock().unwrap()[0],
            json!({"ok":false,"error":"Later","code":"busy"})
        );

        let (stream, rig) = memory(Ok(json!({"watch":true})));
        serve_stream(
            stream,
            "peer",
            |_| true,
            |_, _| async { Ok::<_, String>(Value::Null) },
            |_, _, _| async { Err::<(), Failure>(Failure::coded("unsupported", "No")) },
            FAST,
        )
        .await;
        assert_eq!(
            rig.written.lock().unwrap()[0],
            json!({"ok":false,"error":"No","code":"unsupported"})
        );
    }

    #[tokio::test]
    async fn a_watch_accepts_updates_beats_and_stops_when_the_peer_closes() {
        let (stream, rig) = memory(Ok(json!({"watch":true})));
        let (sinks, mut held) = mpsc::channel(1);
        let task = tokio::spawn(serve_stream(
            stream,
            "peer",
            |_| true,
            |_, _| async { Ok::<_, String>(Value::Null) },
            move |_, _, sink| {
                let sinks = sinks.clone();
                async move {
                    sinks.send(sink).await.unwrap();
                    Ok::<_, String>(())
                }
            },
            FAST,
        ));
        let sink = held.recv().await.unwrap();
        sink.send(json!({"n":1})).await.unwrap();
        // Idle long enough for a heartbeat.
        let beat = async {
            loop {
                if rig
                    .written
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|frame| frame["heartbeat"] == true)
                {
                    return;
                }
                rig.wrote.notified().await;
            }
        };
        timeout(Duration::from_secs(5), beat).await.unwrap();
        rig.close.notify_one();
        timeout(Duration::from_secs(5), task)
            .await
            .unwrap()
            .unwrap();
        let written = rig.written.lock().unwrap().clone();
        assert_eq!(written[0], json!({"ok":true,"watching":true}));
        assert_eq!(written[1], json!({"ok":true,"result":{"n":1}}));
        assert!(written[2..]
            .iter()
            .all(|frame| *frame == json!({"ok":true,"heartbeat":true})));
        assert!(!rig.finished.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn a_watch_ends_with_its_sink_and_a_revoked_peer_gets_one_error() {
        let (stream, rig) = memory(Ok(json!({"watch":true})));
        serve_stream(
            stream,
            "peer",
            |_| true,
            |_, _| async { Ok::<_, String>(Value::Null) },
            |_, _, sink: WatchSink| async move {
                tokio::spawn(async move { sink.send(json!({"n":1})).await.unwrap() });
                Ok::<_, String>(())
            },
            FAST,
        )
        .await;
        assert_eq!(rig.written.lock().unwrap().len(), 2);
        assert!(rig.finished.load(Ordering::SeqCst));

        let (stream, rig) = memory(Ok(json!({"watch":true})));
        let revoked = Arc::new(AtomicBool::new(false));
        let flag = revoked.clone();
        serve_stream(
            stream,
            "peer",
            move |_| !flag.load(Ordering::SeqCst),
            |_, _| async { Ok::<_, String>(Value::Null) },
            move |_, _, sink: WatchSink| {
                let revoked = revoked.clone();
                async move {
                    tokio::spawn(async move {
                        revoked.store(true, Ordering::SeqCst);
                        let _ = sink.send(json!({"n":1})).await;
                    });
                    Ok::<_, String>(())
                }
            },
            FAST,
        )
        .await;
        assert_eq!(
            *rig.written.lock().unwrap(),
            vec![
                json!({"ok":true,"watching":true}),
                json!({"ok":false,"error":"Device revoked"})
            ]
        );
        assert!(rig.finished.load(Ordering::SeqCst));
    }
}
