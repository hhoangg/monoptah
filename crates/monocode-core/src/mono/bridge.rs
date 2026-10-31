//! Session-scoped local app access. Provider children inherit only their own
//! capability; all requests are checked again against the Mono's project list.
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc, Arc,
    },
    time::Duration,
};

pub const HELP: &str = r#"Monoptah app access
Usage: monocode-gpui app ACTION --json '{...}' [--request-id ID]
Pass --input FILE or --input - to read a JSON object instead.
Actions:
models.list {}
sessions.list {project?}
sessions.read {sessionId,project?,before?,limit?,maxChars?}
sessions.start {prompt,project?,harness?,model?,modelSettings?,effort?,runtimeMode?,title?,draft?,reveal?,workspaceMode?,worktreeBase?,worktreeCwd?,placement?,besideSessionId?,notifyOnComplete?,completionDelivery?,parentSessionId?,folderName?,folderId?}
sessions.send {sessionId,prompt,project?,notifyOnComplete?,completionDelivery?}
sessions.draft {sessionId,prompt,project?}
sessions.start folders: folderName (up to 100 characters) files the new session into a new folder of that name; folderId files it into an existing folder of the project, which is never renamed and wins over folderName; an unknown folderId alone is an error. Sessions are only filed when the Mono shows started sessions in the sidebar.
sessions.start parentSessionId: a session ID (a number) of a session this Mono started in the same project; the new session shows under it in the sidebar. A session started under a parent cannot be a parent itself. Combine with folderId to keep a reviewer in its dev's folder.
completionDelivery: "group" (default) delivers one report after every session of the calling turn has stopped; "each" delivers a session's report as soon as it stops, without waiting for the turn or its siblings (use it when one result starts the next step). A habit run always uses "each": the sessions it starts report to the Mono's chat, not to the run, even after the run has ended, so start them, mention what you started and finish. sessions.start notifyOnComplete defaults to true, or to false for a draft, and sessions.send notifyOnComplete defaults to false; draft:true with notifyOnComplete:true is refused, since an unsent draft cannot report. Not used when notifyOnComplete is false.
sessions.stop / sessions.archive / sessions.delete {sessionId,project?}
worktrees.list {project?}
worktrees.create {branch,base?,existing?,project?}
folders.list {project?}
folders.move {sessionId,folderId?,newFolderName?,project?}
notes.list {limit?,offset?}; notes.read {id}; notes.write {id?,title?,body?,tags?}
artifacts.list {kind?,limit?,offset?}; artifacts.read {id}; artifacts.write {id?,kind:"document",title,body,summary?}
soul.read {}; soul.update {text,expectedHash}
memory.read {topic?}; memory.search {query,since?}; memory.add {fact,topic?,until?}; memory.replace {find,fact,topic?,until?}; memory.remove {find,topic?}
habits.list {}; habits.add {name,instructions,schedule:{kind,time?,minute?,dayOfWeek?,hours?}}; habits.update {id,name?,instructions?,schedule?,enabled?}; habits.run {id}; habits.remove {id}
Habit schedule kind: hourly (with minute), daily, weekdays, weekly (dayOfWeek 0 = Sunday) or interval (hours 1 to 24; {"kind":"interval","hours":4,"time":"09:00"} runs at 09:00, 13:00, 17:00, 21:00, 01:00 and on across days; time is the anchor, default 00:00). time is local 24-hour HH:MM.
workflows.list {}; workflows.get {id}; workflows.guide {}; workflows.validate {workflow|yaml,values?,id?}; workflows.add {workflow|yaml,values?}; workflows.update {id,workflow}; workflows.setInputs {id,values}; workflows.run {id,values?}; workflows.stop {id}; workflows.pause {id}; workflows.resume {id}; workflows.resumeRun {id,runId}; workflows.remove {id}; workflows.export {id}; workflows.import {yaml,values?}; workflows.runs {id,limit?}
Workflows (a Mono's own conversation only, not a habit run) are graphs of steps the app runs for you. Read workflows.guide before adding or changing one, check the definition with workflows.validate (a dry run of workflows.add: same checks, nothing is saved, and it lists every command and prompt the steps would run), then save it with workflows.add or workflows.update. Add only after the user saw the steps and agreed. workflow is {"name":"...","description":"...","inputs":{"project":{"type":"project"}},"inputValues":{"project":"<path or name>"},"nodes":[...],"edges":[{"from":"a","to":"b"}]}; yaml is an exported file and values gives its input values. A workflow added here is always saved paused, however clean it is: the user enables it in the Workflows panel. A yaml file (workflows.import, or workflows.add with yaml) is also marked as needing review: you cannot resume or run it until the user approves it, and the result lists its commands and prompts. "issues" lists what would stop it from running, including every input needing a valid value; fix them first.
Workflow node types: schedule (schedule as in habits.add) or manual, session (project is "{{inputs.<name>}}", prompt, checkout root | newWorktree | {"worktreeOf":"<nodeId>"}, output fields; harness, model and effort are optional; the model is written "<harness>:<id from models.list>", for Claude without the "claude-" prefix, for example "claude:sonnet-5" as in workflows.guide; models.list does not list efforts, workflows.validate flags an effort the model does not offer), command, askMono (question, answers), if (condition or decide "mono"), forEach (in, as, maxParallel; its child nodes have "parent") and one report.
workflows.update replaces the definition; the id, enabled state and run state are kept unless given (workflow.enabled). Values of inputs the new definition still declares are kept unless inputValues is given; those of removed inputs are dropped. workflows.run starts a resumed (not paused) workflow now and returns at once; its report arrives in the Mono's chat. values on workflows.run overrides declared inputs for that one run only: it is checked like setInputs, a project must be one of this Mono's, and nothing is saved. A workflow may declare requires (a list of {"cli":"name"}, {"env":"NAME"} or {"mcp":"server"}, each with an optional hint) that are checked before a run or resume starts; if one is missing workflows.run and workflows.resumeRun fail with "Missing requirements:" followed by a JSON list of what is missing, which you pass on to the user. workflows.get includes requiresStatus (each requirement with met), and workflows.validate returns warnings about CLIs a command step seems to use but requires does not declare. workflows.stop ends the run that is going; workflows.pause and workflows.resume switch its trigger off or on; workflows.resumeRun continues an interrupted run. workflows.export returns shareable YAML; workflows.runs returns the run history (limit 1 to 20, default 5) with each node's status per branch.
chat.card {type:"pr",repo?,number,note?} / {type:"session",sessionId,note?} / {type:"choices",options:[text]} / {type:"habit",name,instructions,schedule}
Ordinary Operator sessions are visible in project history. Mono launches follow its Show started sessions preference. reveal defaults to false and controls navigation, not history visibility. placement right/down explicitly opens a split beside besideSessionId (or the caller).
sessions.read returns up to three prose exchanges and nextBefore for older turns; pass that turn ID as before.
notes.write requires body for a new note; existing notes accept title, body or tags independently.
Artifacts, memory, soul, habits, cards and notifyOnComplete:true require a Mono.
The CLI returns {ok,result,requestId}. Retry uncertain mutations with the same request ID and identical input.
Assigned projects alone may be addressed. Credentials are provided in the environment; never print them.
"#;
#[derive(Serialize, Deserialize)]
struct Wire {
    session: i64,
    token: String,
    request_id: String,
    action: String,
    input: Value,
}
pub struct Call {
    pub session: i64,
    pub request_id: String,
    pub action: String,
    pub input: Value,
    pub reply: mpsc::Sender<Result<Value, String>>,
}
pub struct Server {
    endpoint: String,
    secret: Arc<String>,
    stop: Arc<AtomicBool>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Wake the blocking accept so its thread sees the stop flag.
        let _ = TcpStream::connect(&self.endpoint);
    }
}
impl Server {
    pub fn start() -> Result<(Self, async_channel::Receiver<Call>), String> {
        let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| e.to_string())?;
        let endpoint = listener
            .local_addr()
            .map_err(|e| e.to_string())?
            .to_string();
        let secret = Arc::new(format!("{:032x}", rand::random::<u128>()) + &super::new_id());
        let stop = Arc::new(AtomicBool::new(false));
        let connections = Arc::new(AtomicUsize::new(0));
        let (tx, rx) = async_channel::bounded(32);
        let owner = stop.clone();
        let key = secret.clone();
        std::thread::spawn(move || {
            while !owner.load(Ordering::Relaxed) && !tx.is_closed() {
                match listener.accept() {
                    Ok(_) if owner.load(Ordering::Relaxed) => break,
                    Ok((stream, _)) => {
                        if connections.fetch_add(1, Ordering::Relaxed) >= 16 {
                            connections.fetch_sub(1, Ordering::Relaxed);
                            continue;
                        }
                        let tx = tx.clone();
                        let key = key.clone();
                        let connections = connections.clone();
                        std::thread::spawn(move || {
                            let _ = serve(stream, &key, &tx);
                            connections.fetch_sub(1, Ordering::Relaxed);
                        });
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
        });
        Ok((
            Self {
                endpoint,
                secret,
                stop,
            },
            rx,
        ))
    }
    pub fn environment(&self, session: i64) -> HashMap<String, String> {
        HashMap::from([
            ("MONOCODE_APP_ENDPOINT".into(), self.endpoint.clone()),
            ("MONOCODE_APP_TOKEN".into(), token(&self.secret, session)),
            ("MONOCODE_APP_SESSION".into(), session.to_string()),
        ])
    }
}
fn token(secret: &str, session: i64) -> String {
    Sha256::digest(format!("{secret}:{session}").as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
fn serve(
    mut stream: TcpStream,
    secret: &str,
    tx: &async_channel::Sender<Call>,
) -> Result<(), String> {
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| e.to_string())?;
    let mut reader = BufReader::new((&stream).take(1_048_577));
    let mut line = String::new();
    reader.read_line(&mut line).map_err(|e| e.to_string())?;
    if line.len() > 1_048_576 || !line.ends_with('\n') {
        return Err("Invalid app request".into());
    }
    let wire: Wire = serde_json::from_str(&line).map_err(|_| "Invalid app request")?;
    let expected = token(secret, wire.session);
    let equal = wire.token.len() == expected.len()
        && wire
            .token
            .bytes()
            .zip(expected.bytes())
            .fold(0, |diff, (a, b)| diff | a ^ b)
            == 0;
    if !equal
        || !wire.input.is_object()
        || wire.request_id.len() > 128
        || wire.request_id.is_empty()
    {
        return Err("Invalid app capability".into());
    }
    let (reply, result) = mpsc::channel();
    let request_id = wire.request_id;
    tx.send_blocking(Call {
        session: wire.session,
        request_id: request_id.clone(),
        action: wire.action,
        input: wire.input,
        reply,
    })
    .map_err(|_| "App closed")?;
    let response = match result.recv_timeout(Duration::from_secs(60)) {
        Ok(Ok(value)) => json!({"ok":true,"result":value,"requestId":request_id}),
        Ok(Err(error)) => json!({"ok":false,"error":error,"requestId":request_id}),
        Err(_) => {
            json!({"ok":false,"error":"App request timed out; retry using the same request ID","requestId":request_id})
        }
    };
    writeln!(stream, "{response}").map_err(|e| e.to_string())
}
/// Runs before any GUI initialization, so a provider tool never opens a window.
pub fn client(args: &[String]) -> Result<Value, String> {
    if args
        .first()
        .is_none_or(|s| matches!(s.as_str(), "help" | "--help" | "-h"))
    {
        return Ok(json!({"help":HELP}));
    }
    let action = args[0].clone();
    let mut input = json!({});
    let mut request_id = super::new_id();
    let mut args = args[1..].iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--json" => {
                input = serde_json::from_str(args.next().ok_or("--json needs JSON")?)
                    .map_err(|e| e.to_string())?
            }
            "--input" => {
                let path = args.next().ok_or("--input needs a path")?;
                let text = if path == "-" {
                    let mut text = String::new();
                    std::io::stdin()
                        .take(1_048_577)
                        .read_to_string(&mut text)
                        .map_err(|e| e.to_string())?;
                    text
                } else {
                    std::fs::read_to_string(path).map_err(|e| e.to_string())?
                };
                if text.len() > 1_048_576 {
                    return Err("Input too large".into());
                }
                input = serde_json::from_str(&text).map_err(|e| e.to_string())?;
            }
            "--request-id" => request_id = args.next().ok_or("--request-id needs an ID")?.clone(),
            json if json.starts_with('{') => {
                input = serde_json::from_str(json).map_err(|e| e.to_string())?
            }
            _ => return Err(format!("Unknown app argument: {arg}")),
        }
    }
    let endpoint: SocketAddr = std::env::var("MONOCODE_APP_ENDPOINT")
        .map_err(|_| "App access is unavailable outside a Mono's provider process")?
        .parse()
        .map_err(|_| "Invalid app endpoint")?;
    if !endpoint.ip().is_loopback() {
        return Err("App access requires a local endpoint".into());
    }
    let token = std::env::var("MONOCODE_APP_TOKEN").map_err(|_| "App capability missing")?;
    let session = std::env::var("MONOCODE_APP_SESSION")
        .map_err(|_| "App session missing")?
        .parse()
        .map_err(|_| "Invalid app session")?;
    let wire = Wire {
        session,
        token,
        request_id,
        action,
        input,
    };
    let mut stream =
        TcpStream::connect_timeout(&endpoint, Duration::from_secs(3)).map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(65)))
        .map_err(|e| e.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .map_err(|e| e.to_string())?;
    writeln!(
        stream,
        "{}",
        serde_json::to_string(&wire).map_err(|e| e.to_string())?
    )
    .map_err(|e| e.to_string())?;
    let mut line = String::new();
    BufReader::new(stream.take(4_194_305))
        .read_line(&mut line)
        .map_err(|e| e.to_string())?;
    if line.len() > 4_194_304 || line.is_empty() {
        return Err("App response unavailable".into());
    }
    serde_json::from_str(&line).map_err(|e| e.to_string())
}

#[cfg(test)]
mod capability_tests {
    use super::*;
    fn send(
        server: &Server,
        session: i64,
        credential: String,
    ) -> std::thread::JoinHandle<Option<Value>> {
        let endpoint = server.endpoint.clone();
        std::thread::spawn(move || {
            let mut stream = TcpStream::connect(endpoint).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let wire = Wire {
                session,
                token: credential,
                request_id: "read-1".into(),
                action: "models.list".into(),
                input: json!({}),
            };
            writeln!(stream, "{}", serde_json::to_string(&wire).unwrap()).unwrap();
            let mut result = String::new();
            let _ = BufReader::new(stream).read_line(&mut result);
            serde_json::from_str(&result).ok()
        })
    }
    #[test]
    fn capability_is_bound_to_its_session_and_only_authenticated_calls_reach_the_app() {
        let (server, rx) = Server::start().unwrap();
        let credential = server.environment(7)["MONOCODE_APP_TOKEN"].clone();
        assert_ne!(credential, server.environment(8)["MONOCODE_APP_TOKEN"]);
        assert!(send(&server, 8, credential.clone())
            .join()
            .unwrap()
            .is_none());
        assert!(rx.try_recv().is_err());
        let request = send(&server, 7, credential);
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let call = loop {
            if let Ok(call) = rx.try_recv() {
                break call;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "authenticated request was not dispatched"
            );
            std::thread::sleep(Duration::from_millis(5));
        };
        assert_eq!(call.session, 7);
        call.reply.send(Ok(json!({"models":[]}))).unwrap();
        assert_eq!(request.join().unwrap().unwrap()["ok"], true);
    }
}
