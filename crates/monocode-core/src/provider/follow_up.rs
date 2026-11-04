//! The same mid-turn wire messages used by the React provider adapters.
use super::*;
use crate::follow_up::Message;

static STEER_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl Kind {
    pub fn can_steer(self) -> bool {
        matches!(
            self,
            Self::Codex
                | Self::Claude
                | Self::Cursor
                | Self::Pi
                | Self::Omp
                | Self::Hermes
                | Self::OpenCode
        )
    }
}

impl Protocol {
    pub(super) fn ready(&self) -> bool {
        match self.request.kind {
            Kind::Codex => self.active_turn.is_some(),
            Kind::Claude | Kind::Pi | Kind::Omp => self.follow_up_ready,
            _ => self.acp_active_session().is_some(),
        }
    }
    pub(super) fn steer(
        &mut self,
        message: Message,
        stdin: &mut impl Write,
    ) -> Result<Vec<Event>, String> {
        if !self.request.kind.can_steer() || !self.ready() || self.done {
            return Ok(vec![Event::FollowUpFailed {
                id: message.id,
                error: "The provider is not ready for a follow-up. Resume the queue to retry."
                    .into(),
            }]);
        }
        let mut request = self.request.clone();
        request.settings.attachments = message.attachments.clone();
        request.prompt = super::commands::prepare_prompt(
            request.kind,
            message.prepared_prompt.as_deref().unwrap_or(&message.text),
            &request.cwd.to_string_lossy(),
            &message.attachments,
        )?;
        // A retained message can be retried after its acknowledgement deadline.
        // Never let a late reply from that attempt acknowledge the retry.
        let wire = format!(
            "native-steer-{}-{}",
            message.id,
            STEER_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        );
        match request.kind {
            Kind::Codex => write_json(
                stdin,
                json!({"id":wire,"method":"turn/steer","params":{"threadId":self.request.provider_id,"expectedTurnId":self.active_turn,"input":request.codex_input()}}),
            )?,
            Kind::Claude => {
                let prompt = if request.settings.effort.as_deref() == Some("ultrathink") {
                    if request.prompt.is_empty() {
                        "Ultrathink:".into()
                    } else {
                        format!("Ultrathink:\n{}", request.prompt)
                    }
                } else {
                    request.prompt.clone()
                };
                let uuid = uuid::Uuid::new_v4().to_string();
                write_json(
                    stdin,
                    json!({"type":"user","uuid":uuid,"session_id":"","parent_tool_use_id":null,"message":{"role":"user","content":request.claude_content(&prompt)?}}),
                )?;
                self.claude_unstarted_steers.insert(uuid);
                self.text_key = None;
                return Ok(vec![Event::FollowUp(message)]);
            }
            Kind::Pi | Kind::Omp => {
                let mut command = json!({"id":wire,"type":"steer","message":request.prompt.trim()});
                let images = request.pi_images()?;
                if images.as_array().is_some_and(|images| !images.is_empty()) {
                    command["images"] = images;
                }
                // OMP routes slash commands through prompt even in flight.
                // A steer command would deliver the slash text as user prose.
                if request.kind == Kind::Omp && request.prompt.trim_start().starts_with('/') {
                    command["type"] = "prompt".into();
                    command["streamingBehavior"] = "steer".into();
                }
                write_json(stdin, command)?;
            }
            Kind::Cursor => {
                let params =
                    json!({"sessionId":self.acp_active_session(),"prompt":request.acp_content()?});
                // Match Cursor's adapter: retry the older notification name
                // only when writing the current one fails.
                if write_json(
                    stdin,
                    json!({"jsonrpc":"2.0","method":"session/steer","params":params}),
                )
                .is_err()
                {
                    write_json(
                        stdin,
                        json!({"jsonrpc":"2.0","method":"_session/steer","params":params}),
                    )?;
                }
                self.text_key = None;
                return Ok(vec![Event::FollowUp(message)]);
            }
            Kind::Hermes | Kind::OpenCode => write_json(
                stdin,
                json!({"jsonrpc":"2.0","id":wire,"method":"session/prompt","params":{"sessionId":self.acp_active_session(),"prompt":request.acp_content()?}}),
            )?,
            _ => unreachable!(),
        }
        self.steering.insert(wire, (message, Instant::now()));
        Ok(Vec::new())
    }
    pub(super) fn steer_response(&mut self, value: &Value) -> Option<Vec<Event>> {
        let wire = value["id"].as_str()?;
        let error = value.get("error").filter(|error| !error.is_null());
        let response = if self.request.kind.is_pi() {
            value["type"] == "response"
        } else {
            value.get("result").is_some() || error.is_some()
        };
        if !response {
            return None;
        }
        // A timed-out request can answer later. It must never be interpreted as
        // a fatal error for the unrelated turn still running.
        if wire.starts_with("native-steer-") && !self.steering.contains_key(wire) {
            return Some(Vec::new());
        }
        let (message, _) = self.steering.remove(wire)?;
        if error.is_some() || (self.request.kind.is_pi() && value["success"] != true) {
            let error = value["error"]["message"]
                .as_str()
                .or_else(|| value["error"].as_str())
                .unwrap_or("The provider could not accept this follow-up");
            Some(vec![Event::FollowUpFailed {
                id: message.id,
                error: error.into(),
            }])
        } else {
            self.text_key = None;
            Some(vec![Event::FollowUp(message)])
        }
    }
    pub(super) fn expire_steering(&mut self) -> Vec<Event> {
        // These are acknowledgement deadlines, not turn deadlines. Codex's
        // JSON-RPC steer request deliberately waits without a response timer.
        let timeout = match self.request.kind {
            Kind::Pi | Kind::Omp => Duration::from_secs(15),
            Kind::Hermes => Duration::from_secs(20),
            Kind::OpenCode => Duration::from_secs(30),
            _ => return Vec::new(),
        };
        let expired: Vec<_> = self
            .steering
            .iter()
            .filter(|(_, (_, at))| at.elapsed() > timeout)
            .map(|(id, _)| id.clone())
            .collect();
        expired
            .into_iter()
            .filter_map(|id| self.steering.remove(&id))
            .map(|(message, _)| Event::FollowUpFailed {
                id: message.id,
                error: "Timed out waiting for the provider to accept this follow-up".into(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn protocol(kind: Kind) -> Protocol {
        let mut p = Protocol::new(Request {
            kind,
            binary: "unused".into(),
            cwd: "/tmp".into(),
            prompt: "hello".into(),
            provider_id: Some("thread".into()),
            model: None,
            settings: Default::default(),
        });
        p.active_turn = Some("turn".into());
        p.follow_up_ready = true;
        p
    }
    #[test]
    fn codex_keeps_messages_until_ack_and_rejection_does_not_end_the_turn() {
        let mut p = protocol(Kind::Codex);
        let message = Message {
            id: "one".into(),
            text: "new direction 🙂".into(),
            ..Default::default()
        };
        let mut out = Vec::new();
        assert!(p.steer(message.clone(), &mut out).unwrap().is_empty());
        let wire: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(wire["method"], "turn/steer");
        assert_eq!(wire["params"]["expectedTurnId"], "turn");
        assert_eq!(wire["params"]["input"][0]["text"], message.text);
        let events = p
            .frame(
                json!({"id":wire["id"],"error":{"message":"turn is closing"}}),
                &mut out,
            )
            .unwrap();
        assert!(
            matches!(&events[0], Event::FollowUpFailed { id, error } if id == "one" && error == "turn is closing")
        );
        assert!(!p.done);
        out.clear();
        p.steer(message.clone(), &mut out).unwrap();
        let retry: Value = serde_json::from_slice(&out).unwrap();
        assert_ne!(retry["id"], wire["id"]);
        assert!(p
            .frame(json!({"id":wire["id"],"result":{}}), &mut out)
            .unwrap()
            .is_empty());
        assert_eq!(
            p.frame(json!({"id":retry["id"],"result":{}}), &mut out)
                .unwrap(),
            vec![Event::FollowUp(message)]
        );
    }
    #[test]
    fn claude_and_pi_use_their_existing_stream_protocols() {
        for kind in [Kind::Claude, Kind::Pi, Kind::Omp] {
            let mut p = protocol(kind);
            let mut out = Vec::new();
            let message = Message {
                id: "two".into(),
                text: "follow up".into(),
                ..Default::default()
            };
            let events = p.steer(message.clone(), &mut out).unwrap();
            let wire: Value = serde_json::from_slice(&out).unwrap();
            if kind == Kind::Claude {
                assert_eq!(wire["type"], "user");
                assert_eq!(events, vec![Event::FollowUp(message)]);
            } else {
                assert_eq!(wire["type"], "steer");
                assert!(events.is_empty());
                assert_eq!(
                    p.frame(
                        json!({"type":"response","id":wire["id"],"success":true}),
                        &mut out
                    )
                    .unwrap(),
                    vec![Event::FollowUp(message)]
                );
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn running_process_waits_for_turn_readiness_and_streams_after_steering() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let binary = temp.path().join("provider");
        std::fs::write(&binary, r##"#!/usr/bin/env python3
import json, sys

def send(value):
    print(json.dumps(value), flush=True)

for line in sys.stdin:
    frame = json.loads(line)
    method = frame.get('method')
    if method == 'initialize':
        send({'id': frame['id'], 'result': {}})
    elif method == 'thread/start':
        send({'id': frame['id'], 'result': {'thread': {'id': 'thread'}}})
    elif method == 'turn/start':
        send({'id': frame['id'], 'result': {'turn': {'id': 'turn'}}})
        send({'method': 'item/agentMessage/delta', 'params': {'itemId': 'before', 'delta': 'Before'}})
    elif method == 'turn/steer':
        send({'id': frame['id'], 'result': {}})
        send({'method': 'item/agentMessage/delta', 'params': {'itemId': 'after', 'delta': 'After'}})
        send({'method': 'turn/completed', 'params': {'turn': {'status': 'completed'}}})
"##).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        let (controls, receive) = mpsc::channel();
        let mut delivered = false;
        let mut output = String::new();
        run_with_controls(
            Request {
                kind: Kind::Codex,
                binary,
                cwd: temp.path().into(),
                prompt: "hello".into(),
                provider_id: None,
                model: None,
                settings: Default::default(),
            },
            Arc::new(AtomicBool::new(false)),
            Some(receive),
            |events| {
                for event in events {
                    match event {
                        Event::Ready => controls
                            .send(Control {
                                id: "test".into(),
                                decision: Decision::Steer(Message {
                                    id: "test".into(),
                                    text: "new direction".into(),
                                    ..Default::default()
                                }),
                            })
                            .unwrap(),
                        Event::FollowUp(message) => {
                            assert_eq!(message.id, "test");
                            delivered = true;
                        }
                        Event::Text(text) => {
                            if text == "After" {
                                assert!(
                                    delivered,
                                    "the accepted user message must precede its response"
                                );
                            }
                            output.push_str(&text);
                        }
                        _ => {}
                    }
                }
                Ok(())
            },
        )
        .unwrap();
        assert!(delivered);
        assert_eq!(output, "BeforeAfter");
    }

    fn claude_steer(p: &mut Protocol) -> String {
        let mut stdin = Vec::new();
        let message = Message {
            id: "queued".into(),
            text: "second question".into(),
            ..Default::default()
        };
        p.steer(message, &mut stdin).unwrap();
        let frame: Value = serde_json::from_slice(&stdin).unwrap();
        frame["uuid"].as_str().unwrap_or_default().to_owned()
    }
    fn claude_frame(p: &mut Protocol, frame: Value) -> Vec<Event> {
        p.frame(frame, &mut Vec::new()).unwrap()
    }
    fn claude_replay(uuid: &str) -> Value {
        json!({"type":"user","isReplay":true,"uuid":uuid,"parent_tool_use_id":null,
            "message":{"role":"user","content":[{"type":"text","text":"second question"}]}})
    }
    fn claude_answer(id: &str, text: &str) -> Value {
        json!({"type":"assistant","parent_tool_use_id":null,
            "message":{"id":id,"content":[{"type":"text","text":text}]}})
    }

    #[test]
    fn claude_turn_stays_open_for_a_prompt_queued_before_end_turn() {
        let mut p = protocol(Kind::Claude);
        let uuid = claude_steer(&mut p);
        // The CLI finishes the running request first and only then starts the
        // queued prompt, as a separate run with its own result.
        let first = claude_frame(
            &mut p,
            json!({"type":"result","num_turns":2,"result":"first"}),
        );
        assert!(
            !first.contains(&Event::Done),
            "the queued prompt is unanswered"
        );
        assert!(!p.done);
        assert!(p
            .lifecycle_tick(Instant::now(), &mut Vec::new())
            .unwrap()
            .iter()
            .all(|e| *e != Event::Done));

        claude_frame(
            &mut p,
            json!({"type":"system","subtype":"init","session_id":"s"}),
        );
        claude_frame(&mut p, claude_replay(&uuid));
        let answer = claude_frame(&mut p, claude_answer("second", "BRAVO"));
        assert!(answer.contains(&Event::Text("BRAVO".into())));
        let last = claude_frame(
            &mut p,
            json!({"type":"result","num_turns":1,"result":"BRAVO"}),
        );
        assert!(last.contains(&Event::Done));
        assert!(p.done);
    }

    #[test]
    fn claude_steer_taken_into_the_running_request_needs_no_second_result() {
        let mut p = protocol(Kind::Claude);
        let uuid = claude_steer(&mut p);
        assert!(
            !uuid.is_empty(),
            "the steer must be identifiable when replayed"
        );
        // Delivered at a tool boundary: the CLI replays it and reports one result.
        claude_frame(&mut p, claude_replay(&uuid));
        let events = claude_frame(
            &mut p,
            json!({"type":"result","num_turns":2,"result":"both"}),
        );
        assert!(events.contains(&Event::Done));
    }

    #[test]
    fn claude_replay_of_the_opening_prompt_does_not_count_for_a_queued_steer() {
        let mut p = protocol(Kind::Claude);
        claude_steer(&mut p);
        // The opening prompt can be replayed after the steer was already written.
        claude_frame(&mut p, claude_replay("opening-prompt"));
        let events = claude_frame(
            &mut p,
            json!({"type":"result","num_turns":1,"result":"first"}),
        );
        assert!(!events.contains(&Event::Done));
    }

    #[test]
    fn claude_replayed_task_notification_result_does_not_end_the_turn() {
        let mut p = protocol(Kind::Claude);
        // A resume re-reports an unfinished background task and answers it
        // with an empty zero-turn result, before running the real prompt.
        claude_frame(
            &mut p,
            json!({"type":"system","subtype":"task_notification","task_id":"bash","status":"stopped"}),
        );
        claude_frame(
            &mut p,
            json!({"type":"system","subtype":"init","session_id":"s"}),
        );
        let replayed = claude_frame(
            &mut p,
            json!({"type":"result","subtype":"success","num_turns":0,"result":"","origin":{"kind":"task-notification"}}),
        );
        assert!(!replayed.contains(&Event::Done));
        assert!(!p.done);
        assert!(!p.claude_result_seen);

        claude_frame(
            &mut p,
            json!({"type":"system","subtype":"init","session_id":"s"}),
        );
        claude_frame(&mut p, claude_answer("real", "GAMMA"));
        let real = claude_frame(
            &mut p,
            json!({"type":"result","num_turns":1,"result":"GAMMA"}),
        );
        assert!(real.contains(&Event::Done));
    }

    #[test]
    fn claude_turn_with_a_real_task_notification_run_still_ends() {
        let mut p = protocol(Kind::Claude);
        let events = claude_frame(
            &mut p,
            json!({"type":"result","num_turns":1,"result":"woke","origin":{"kind":"task-notification"}}),
        );
        assert!(events.contains(&Event::Done), "this run did real work");
    }

    #[test]
    fn claude_turn_ends_when_a_queued_prompt_never_starts() {
        let mut p = protocol(Kind::Claude);
        claude_steer(&mut p);
        claude_frame(
            &mut p,
            json!({"type":"result","num_turns":1,"result":"first"}),
        );
        assert!(!p.done);
        assert!(
            p.claude_resume_at
                .is_some_and(|at| at <= Instant::now() + Duration::from_secs(60)),
            "the wait for a queued prompt must be bounded"
        );
        p.claude_resume_at = Some(Instant::now() - Duration::from_secs(1));
        let events = p.lifecycle_tick(Instant::now(), &mut Vec::new()).unwrap();
        assert!(events.contains(&Event::Done));
        assert!(p.done);
    }

    #[cfg(unix)]
    fn fake_claude(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let binary = dir.join("claude");
        let script = format!(
            r#"#!/usr/bin/env python3
import json, sys
assert '--replay-user-messages' in sys.argv, 'steers cannot be acknowledged without replay'
def send(value):
    print(json.dumps(value), flush=True)
def text(id, value):
    send({{'type': 'assistant', 'parent_tool_use_id': None, 'message': {{'id': id, 'content': [{{'type': 'text', 'text': value}}]}}}})
def result(turns=1, **rest):
    send({{'type': 'result', 'subtype': 'success', 'num_turns': turns, **rest}})
def init():
    send({{'type': 'system', 'subtype': 'init', 'session_id': 's'}})
def replay(frame):
    send({{'type': 'user', 'isReplay': True, 'uuid': frame.get('uuid'), 'parent_tool_use_id': None, 'message': frame['message']}})
prompts = 0
for line in sys.stdin:
    frame = json.loads(line)
    if frame['type'] == 'control_request' and frame['request']['subtype'] == 'initialize':
        send({{'type': 'control_response', 'response': {{'subtype': 'success', 'request_id': frame['request_id']}}}})
    elif frame['type'] == 'user':
        prompts += 1
{body}
"#
        );
        std::fs::write(&binary, script).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        binary
    }
    #[cfg(unix)]
    fn run_fake_claude(
        binary: std::path::PathBuf,
        cwd: &std::path::Path,
    ) -> (Result<(), String>, String) {
        let (controls, receive) = mpsc::channel();
        let mut output = String::new();
        let mut steered = false;
        let result = run_with_controls(
            Request {
                kind: Kind::Claude,
                binary,
                cwd: cwd.into(),
                prompt: "first question".into(),
                provider_id: None,
                model: None,
                settings: Default::default(),
            },
            Arc::new(AtomicBool::new(false)),
            Some(receive),
            |events| {
                for event in events {
                    match event {
                        Event::Ready if !steered => {
                            steered = true;
                            controls
                                .send(Control {
                                    id: "queued".into(),
                                    decision: Decision::Steer(Message {
                                        id: "queued".into(),
                                        text: "second question".into(),
                                        ..Default::default()
                                    }),
                                })
                                .unwrap();
                        }
                        Event::Text(text) => output.push_str(&text),
                        _ => {}
                    }
                }
                Ok(())
            },
        );
        (result, output)
    }

    #[cfg(unix)]
    #[test]
    fn claude_process_streams_the_reply_to_a_prompt_queued_before_end_turn() {
        let dir = tempfile::tempdir().unwrap();
        let binary = fake_claude(
            dir.path(),
            r#"        if prompts == 2:
            text('first', 'ALPHA ')
            result()
            init()
            replay(frame)
            text('second', 'BRAVO')
            result()"#,
        );
        let (result, output) = run_fake_claude(binary, dir.path());
        result.unwrap();
        assert_eq!(output, "ALPHA BRAVO");
    }

    #[cfg(unix)]
    #[test]
    fn claude_process_exit_during_the_wait_reports_an_error_instead_of_hanging() {
        let dir = tempfile::tempdir().unwrap();
        let binary = fake_claude(
            dir.path(),
            r#"        if prompts == 2:
            text('first', 'ALPHA')
            result()
            sys.exit(0)"#,
        );
        let started = Instant::now();
        let (result, output) = run_fake_claude(binary, dir.path());
        assert_eq!(output, "ALPHA");
        assert!(result.unwrap_err().contains("Provider exited"));
        assert!(started.elapsed() < Duration::from_secs(10));
    }
}
