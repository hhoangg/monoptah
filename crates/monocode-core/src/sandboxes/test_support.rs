//! A fake `sbx` that models Docker accounts: a sandbox is visible only to the account that
//! created it, so a call made as another account answers "not found" (what the real CLI does,
//! and how a wrong account would make the app forget a sandbox that still bills).
use super::{SbxOutput, SbxRunner};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

#[derive(Clone, Debug)]
struct Entry {
    id: String,
    name: String,
    state: String,
    account: Option<String>,
}

/// One recorded call: the verb (`ls`, `create`, ...) and the account it ran as.
pub(crate) type Call = (String, Option<String>);

#[derive(Default)]
pub(crate) struct AccountSbx {
    boxes: Mutex<Vec<Entry>>,
    calls: Mutex<Vec<Call>>,
    /// `create` makes the sandbox but answers with a timeout, and the next `ls` fails too:
    /// the app can not tell whether the sandbox exists (an ambiguous create).
    ambiguous_create: AtomicBool,
    fail_next_ls: AtomicBool,
}

impl AccountSbx {
    pub(crate) fn new() -> Self {
        Self::default()
    }
    /// A sandbox that lives in `account` (`None`: the machine login).
    pub(crate) fn add(&self, id: &str, name: &str, state: &str, account: Option<&str>) {
        self.boxes.lock().unwrap().push(Entry {
            id: id.into(),
            name: name.into(),
            state: state.into(),
            account: account.map(str::to_owned),
        });
    }
    /// Makes the next `create` ambiguous, see [`Self::ambiguous_create`].
    pub(crate) fn make_create_ambiguous(&self) {
        self.ambiguous_create.store(true, Ordering::SeqCst);
    }
    pub(crate) fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }
    /// The accounts the calls of `verb` ran as, in order.
    pub(crate) fn accounts_of(&self, verb: &str) -> Vec<Option<String>> {
        self.calls()
            .into_iter()
            .filter(|(v, _)| v == verb)
            .map(|(_, account)| account)
            .collect()
    }
    pub(crate) fn exists(&self, id: &str) -> bool {
        self.boxes.lock().unwrap().iter().any(|b| b.id == id)
    }
}

fn out(success: bool, stdout: &str, stderr: &str) -> Result<SbxOutput, String> {
    Ok(SbxOutput {
        success,
        stdout: stdout.into(),
        stderr: stderr.into(),
    })
}

impl SbxRunner for AccountSbx {
    fn run(&self, args: &[String], stdin: Option<&str>) -> Result<SbxOutput, String> {
        self.run_as(None, args, stdin)
    }
    fn run_as(
        &self,
        account: Option<&str>,
        args: &[String],
        _stdin: Option<&str>,
    ) -> Result<SbxOutput, String> {
        let verb = args
            .iter()
            .find(|a| !a.starts_with('-'))
            .cloned()
            .unwrap_or_default();
        self.calls
            .lock()
            .unwrap()
            .push((verb.clone(), account.map(str::to_owned)));
        let mut boxes = self.boxes.lock().unwrap();
        let mine = |b: &Entry| b.account.as_deref() == account;
        let id_arg = args
            .iter()
            .skip_while(|a| **a != verb)
            .skip(1)
            .find(|a| a.starts_with("sbx_"))
            .cloned()
            .unwrap_or_default();
        let missing = || out(false, "", &format!("error: sandbox '{id_arg}' not found"));
        match verb.as_str() {
            "ls" if self.fail_next_ls.swap(false, Ordering::SeqCst) => {
                Err("Could not reach Docker Sandboxes (network).".into())
            }
            "ls" => {
                let items: Vec<serde_json::Value> = boxes
                    .iter()
                    .filter(|b| mine(b))
                    .map(|b| {
                        serde_json::json!({
                            "id": b.id, "name": b.name, "agent": "shell", "state": b.state
                        })
                    })
                    .collect();
                out(true, &serde_json::to_string(&items).unwrap(), "")
            }
            "inspect" => match boxes.iter().find(|b| b.id == id_arg && mine(b)) {
                Some(b) => out(
                    true,
                    &serde_json::json!({
                        "id": b.id, "name": b.name, "agent": "shell", "state": b.state
                    })
                    .to_string(),
                    "",
                ),
                None => missing(),
            },
            "stop" => match boxes.iter_mut().find(|b| b.id == id_arg && mine(b)) {
                Some(b) => {
                    b.state = "stopped".into();
                    out(true, "", "")
                }
                None => missing(),
            },
            "rm" => {
                let before = boxes.len();
                let id = id_arg.clone();
                boxes.retain(|b| !(b.id == id && mine(b)));
                if boxes.len() == before {
                    missing()
                } else {
                    out(true, "", "")
                }
            }
            "create" => {
                let name = args
                    .iter()
                    .position(|a| a == "--name")
                    .and_then(|i| args.get(i + 1))
                    .cloned()
                    .unwrap_or_default();
                let id = format!("sbx_new{}", boxes.len() + 1);
                boxes.push(Entry {
                    id: id.clone(),
                    name,
                    state: "running".into(),
                    account: account.map(str::to_owned),
                });
                if self.ambiguous_create.swap(false, Ordering::SeqCst) {
                    self.fail_next_ls.store(true, Ordering::SeqCst);
                    return Err(
                        "Docker Sandboxes CLI did not answer within 120 seconds (`sbx create`)."
                            .into(),
                    );
                }
                out(true, &format!("{id}\n"), "")
            }
            other => panic!("unexpected sbx call: {other} ({args:?})"),
        }
    }
}
