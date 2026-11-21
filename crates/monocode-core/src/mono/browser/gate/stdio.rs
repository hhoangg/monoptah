//! The gate's client for a Playwright server it started itself: JSON-RPC lines over the child's
//! stdin and stdout. There is no port, so no other process can reach the server.
use super::Upstream;
use serde_json::Value;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{ChildStdin, ChildStdout};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

/// Longer than any page action Playwright allows itself.
const REPLY_TIMEOUT: Duration = Duration::from_secs(300);

pub struct StdioUpstream {
    /// Taken (closed) when the server is given up on: it exits when its stdin ends.
    stdin: Mutex<Option<Box<dyn Write + Send>>>,
    timeout: Duration,
    pending: Arc<Mutex<HashMap<u64, mpsc::Sender<Value>>>>,
    open: Arc<AtomicBool>,
}
impl StdioUpstream {
    /// Reads replies from `stdout` on a thread until it ends.
    pub fn new(stdin: ChildStdin, stdout: ChildStdout) -> Arc<Self> {
        Self::over(Box::new(stdin), Box::new(stdout))
    }
    /// Any writer and reader pair (tests use pipes).
    pub fn over(stdin: Box<dyn Write + Send>, stdout: Box<dyn std::io::Read + Send>) -> Arc<Self> {
        Self::over_with_timeout(stdin, stdout, REPLY_TIMEOUT)
    }
    pub fn over_with_timeout(
        stdin: Box<dyn Write + Send>,
        stdout: Box<dyn std::io::Read + Send>,
        timeout: Duration,
    ) -> Arc<Self> {
        let pending: Arc<Mutex<HashMap<u64, mpsc::Sender<Value>>>> = Arc::default();
        let open = Arc::new(AtomicBool::new(true));
        let (table, flag) = (pending.clone(), open.clone());
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                let Ok(message) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                // Replies only: a request or notification from the server is not for us.
                if message.get("method").is_some() {
                    continue;
                }
                if let Some(id) = message["id"].as_u64() {
                    if let Some(waiting) =
                        table.lock().unwrap_or_else(|e| e.into_inner()).remove(&id)
                    {
                        let _ = waiting.send(message);
                    }
                }
            }
            flag.store(false, Ordering::SeqCst);
            // Everyone still waiting learns the server is gone.
            table.lock().unwrap_or_else(|e| e.into_inner()).clear();
        });
        Arc::new(Self {
            stdin: Mutex::new(Some(stdin)),
            timeout,
            pending,
            open,
        })
    }
    /// Gives up on the server: closing its stdin makes it exit, and from now on every call fails,
    /// so the app replaces it at the next run.
    pub fn close(&self) {
        self.open.store(false, Ordering::SeqCst);
        self.stdin.lock().unwrap_or_else(|e| e.into_inner()).take();
        self.pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }
    /// The server's output has not ended.
    pub fn is_open(&self) -> bool {
        self.open.load(Ordering::SeqCst)
    }
}
impl Upstream for StdioUpstream {
    fn post(
        &self,
        _server: &str,
        _session: Option<&str>,
        message: &Value,
    ) -> Result<(Option<Value>, Option<String>), String> {
        let id = message.get("id").and_then(Value::as_u64);
        // Registered before the open check: a server that ends in between clears the table, and
        // the wait below then fails instead of hanging.
        let waiting = id.map(|id| {
            let (tx, rx) = mpsc::channel();
            self.pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(id, tx);
            (id, rx)
        });
        let forget = |id: u64| {
            self.pending
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .remove(&id);
        };
        if !self.is_open() {
            if let Some((id, _)) = &waiting {
                forget(*id);
            }
            return Err("The browser server stopped.".into());
        }
        let sent = {
            let mut stdin = self.stdin.lock().unwrap_or_else(|e| e.into_inner());
            match stdin.as_mut() {
                Some(stdin) => writeln!(stdin, "{message}").and_then(|_| stdin.flush()),
                None => Err(std::io::ErrorKind::BrokenPipe.into()),
            }
        };
        if sent.is_err() {
            if let Some((id, _)) = &waiting {
                forget(*id);
            }
            self.close();
            return Err("The browser server stopped.".into());
        }
        let Some((id, rx)) = waiting else {
            return Ok((None, None));
        };
        match rx.recv_timeout(self.timeout) {
            Ok(reply) => Ok((Some(reply), None)),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                forget(id);
                // Its state is unknown (the call may still run): give the server up.
                self.close();
                Err("The browser server did not answer in time.".into())
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => Err("The browser server stopped.".into()),
        }
    }
    fn session_survives(&self, _error: &str) -> bool {
        // Never start a second session on this server; if it is unusable it is already closed.
        true
    }
}
