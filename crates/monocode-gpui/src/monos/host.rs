//! Capability-checked app actions used by Mono provider subprocesses.
use super::*;
use monocode_core::mono::{Document, Launch, bridge::Call, files, memory};
use monocode_core::{session_groups::Groups, time::now_ms};
use serde_json::{Value, json};
use std::path::Path;
enum Effect {
    Card(Value),
    None,
    Start(Box<Started>),
    Groups(String, Groups),
}
struct Started {
    summary: Session,
    settings: Settings,
    prompt: String,
    draft: bool,
    reveal: bool,
    parent: Option<i64>,
    folder: Option<Folder>,
}
/// Where a launch is filed: an existing folder, or a new one made for it.
enum Folder {
    Existing { id: String, name: String },
    New { id: String, name: String },
}
impl Folder {
    fn entry(&self) -> (&str, &str) {
        match self {
            Folder::Existing { id, name } | Folder::New { id, name } => (id, name),
        }
    }
}
/// An existing folder wins over the name, which only names a new folder when
/// the id is unknown or absent. A bare unknown id is an error.
fn folder_choice(groups: &Groups, input: &Value) -> Result<Option<Folder>, String> {
    let field = |key: &str| input.get(key).and_then(Value::as_str).map(str::trim);
    let (id, name) = (field("folderId"), field("folderName"));
    if let Some(folder) = id.and_then(|id| groups.folder(id)) {
        return Ok(Some(Folder::Existing {
            id: folder.id.clone(),
            name: folder.name.clone(),
        }));
    }
    match (name, id) {
        (Some(name), _) => Ok(Some(Folder::New {
            id: mono::new_id(),
            name: name.into(),
        })),
        (None, Some(_)) => Err("Folder was not found in this project".into()),
        (None, None) => Ok(None),
    }
}
/// Whether a launch was itself started under another of this Mono's launches.
/// The shared database also links a Tauri direct launch (`app-<chat>-<id>`) to
/// the Mono chat; that link does not make it a child.
pub(super) fn is_child_launch(
    links: &std::collections::HashMap<i64, i64>,
    launches: &[Launch],
    session: i64,
) -> bool {
    links
        .get(&session)
        .is_some_and(|caller| launches.iter().any(|l| l.session_id == *caller))
}
/// One completion report: a turn's group, or a single "each" launch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Batch {
    Group(u64),
    Each(i64),
}
const RESULT_CHARS: usize = 12_000;
const BATCH_CHARS: usize = 48_000;
/// Reports that can go out now. A group waits for its launching turn to end and
/// for every child to stop; an "each" launch only for itself.
pub(super) fn ready_batches(
    launches: &[Launch],
    source_busy: impl Fn(i64) -> bool,
    stopped: impl Fn(i64) -> bool,
) -> Vec<(Batch, Vec<(i64, String)>)> {
    let waiting = |l: &&Launch| l.notify_on_complete && !l.completed && !l.dismissed;
    let mut ready = Vec::new();
    let mut seen = HashSet::new();
    for launch in launches.iter().filter(waiting) {
        if launch.each {
            if stopped(launch.session_id) {
                ready.push((
                    Batch::Each(launch.session_id),
                    vec![(launch.session_id, launch.title.clone())],
                ));
            }
            continue;
        }
        let turn = launch.origin_turn;
        if !seen.insert(turn) || source_busy((turn >> 32) as i64) {
            continue;
        }
        let children: Vec<_> = launches
            .iter()
            .filter(waiting)
            .filter(|l| !l.each && l.origin_turn == turn)
            .collect();
        if children.iter().all(|l| stopped(l.session_id)) {
            let sessions = children
                .iter()
                .map(|l| (l.session_id, l.title.clone()))
                .collect();
            ready.push((Batch::Group(turn), sessions));
        }
    }
    ready
}
/// Marks a delivered report's launches done, so it is never sent twice.
pub(super) fn finish_batch(launches: &mut [Launch], batch: Batch) {
    for launch in launches {
        let part = match batch {
            Batch::Group(turn) => !launch.each && launch.origin_turn == turn,
            Batch::Each(session) => launch.each && launch.session_id == session,
        };
        if part {
            launch.completed = true;
        }
    }
}
/// The turn a launch reports under, and whether it reports on its own. A habit
/// run has no turn to group by, so its sessions report to the Mono's chat, each
/// as soon as it stops.
pub(super) fn report_to(
    caller: i64,
    turn: u64,
    run_chat: Option<i64>,
    input: &Value,
) -> (u64, bool) {
    match run_chat {
        Some(chat) => ((chat as u64) << 32, true),
        None => (
            ((caller as u64) << 32) | turn,
            input.get("completionDelivery").and_then(Value::as_str) == Some("each"),
        ),
    }
}
/// The message the Mono receives. The results share one budget, so a large
/// group still names every session.
pub(super) fn completion_message(batch: Batch, results: &[(i64, String, String)]) -> String {
    let budget = (BATCH_CHARS / results.len().max(1)).clamp(200, RESULT_CHARS);
    let report = results
        .iter()
        .map(|(id, title, prose)| {
            format!(
                "Session {id}: {title}\n{}",
                monocode_core::response::limit(prose, budget)
            )
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let lead = match batch {
        Batch::Group(_) => {
            "All sessions in this group have stopped. Inspect their results and give an integrated update."
        }
        Batch::Each(_) => {
            "This session has stopped. Inspect its result and continue; other sessions report separately."
        }
    };
    format!("[Delegated work completed]\n{lead}\n\n{report}")
}
/// The tab group palette index of a Mono's color.
fn folder_color(color: u32) -> Option<usize> {
    mono::COLORS_MONO
        .iter()
        .position(|c| *c == color)
        .map(|n| if n == 8 { 6 } else { n + 1 })
}
fn text(v: &Value, key: &str, max: usize) -> Result<String, String> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty() && s.len() <= max)
        .map(str::to_owned)
        .ok_or_else(|| format!("Provide {key} (up to {max} bytes)"))
}
fn id(v: &Value, key: &str) -> Result<i64, String> {
    v.get(key)
        .and_then(|v| v.as_i64().or_else(|| v.as_str()?.parse().ok()))
        .filter(|n| *n > 0)
        .ok_or_else(|| format!("Provide {key}"))
}
fn flag(v: &Value, key: &str, default: bool) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(default)
}
fn fingerprint(call: &Call) -> String {
    files::content_hash(&format!("{}:{}", call.action, call.input))
}
impl Monos {
    fn finish_call(&mut self, call: Call, result: Result<Value, String>, cx: &mut Context<Self>) {
        let key = (call.session, call.request_id.clone());
        self.app_pending.remove(&key);
        if self.app_results.len() > 1024
            && let Some(key) = self.app_results.keys().next().cloned()
        {
            self.app_results.remove(&key);
        }
        self.app_results
            .insert(key, (fingerprint(&call), result.clone()));
        let store = self.store.clone();
        let receipt_key = format!(
            "mono-app-receipt:{}:{}",
            call.session,
            files::content_hash(&call.request_id)
        );
        let receipt =
            serde_json::to_string(&(fingerprint(&call), result.clone())).unwrap_or_default();
        let previous = self.writes.take();
        self.writes = Some(cx.spawn(async move |_, cx| {
            if let Some(previous) = previous {
                previous.await;
            }
            let work = cx
                .background_executor()
                .spawn(async move { locked(&store)?.save_setting(&receipt_key, &receipt) });
            let reply = work
                .await
                .map(|_| ())
                .map_err(|e| format!("Could not persist app result: {e}"));
            let _ = call.reply.send(reply.and(result));
        }));
    }
    fn call_work(
        &mut self,
        call: Call,
        mono_id: String,
        work: impl FnOnce(Database) -> Result<(Value, Effect), String> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        let store = self.store.clone();
        let work = cx.background_executor().spawn(async move { work(store) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update_in(cx, |this, window, cx| {
                let result = result.map(|(value, effect)| {
                    match effect {
                        Effect::None => (),
                        Effect::Card(value) => {
                            this.hold_card(call.session, &mono_id, value);
                            this.save(cx);
                        }
                        Effect::Groups(project, groups) => {
                            let _ = this.owner.update(cx, |owner, cx| {
                                owner.mono_apply_groups(project, groups, cx)
                            });
                        }
                        Effect::Start(started) => {
                            let Started {
                                summary,
                                settings,
                                prompt,
                                draft,
                                reveal,
                                parent,
                                folder,
                            } = *started;
                            let id = summary.id;
                            let project = summary.cwd.clone();
                            let caller = call.session;
                            let origin = this
                                .chats
                                .values()
                                .chain(this.running.values().map(|r| &r.view))
                                .find(|v| v.read(cx).summary.id == caller)
                                .map(|v| v.read(cx).mono_turn)
                                .unwrap_or(0);
                            let run_chat = this.run_chat(caller, &mono_id);
                            let (origin_turn, each) =
                                report_to(caller, origin, run_chat, &call.input);
                            if let Some(mono) = this.roster.get_mut(&mono_id) {
                                mono.launches.push(Launch {
                                    session_id: id,
                                    cwd: summary.cwd.clone(),
                                    title: summary.title.clone(),
                                    harness: summary.provider.clone(),
                                    model: settings.model.clone(),
                                    origin_turn,
                                    notify_on_complete: flag(
                                        &call.input,
                                        "notifyOnComplete",
                                        !draft,
                                    ),
                                    each,
                                    completed: false,
                                    dismissed: false,
                                });
                            }
                            this.note_run_session(caller, id, true);
                            let _ = this.owner.update(cx, |owner, cx| {
                                owner.mono_install(summary, settings, prompt, draft, cx);
                                if let Some(parent) = parent {
                                    owner.mono_link_started(id, parent, cx);
                                }
                            });
                            this.save(cx);
                            match folder {
                                Some(folder) => {
                                    this.file_launch(&mono_id, &project, id, folder, cx)
                                }
                                None => this.group_launch(&mono_id, id, cx),
                            }
                            if reveal {
                                this.open_workspace(id, window, cx);
                            }
                        }
                    }
                    this.reload_files(&mono_id, cx);
                    this.changed(cx);
                    value
                });
                this.finish_call(call, result, cx);
            });
        })
        .detach();
    }
    pub(super) fn app_call(&mut self, call: Call, window: &mut Window, cx: &mut Context<Self>) {
        let mono = self
            .roster
            .of_session(call.session)
            .or_else(|| {
                self.running
                    .get(&call.session)
                    .and_then(|r| self.roster.get(&r.mono))
            })
            .cloned();
        let Some(mono) = mono else {
            let _ = call.reply.send(Err(
                "This Mono no longer owns an active app capability".into()
            ));
            return;
        };
        if call.action.starts_with("habits.")
            && call.action != "habits.list"
            && self.running.contains_key(&call.session)
        {
            let _ = call
                .reply
                .send(Err("A habit run cannot schedule more runs".into()));
            return;
        }
        if let Err(error) = mono::api::validate(&call.action, &call.input) {
            let _ = call.reply.send(Err(error));
            return;
        }
        let receipt_key = format!(
            "mono-app-receipt:{}:{}",
            call.session,
            files::content_hash(&call.request_id)
        );
        let stored = locked(&self.store)
            .ok()
            .and_then(|db| db.setting(&receipt_key).ok().flatten())
            .and_then(|s| serde_json::from_str::<(String, Result<Value, String>)>(&s).ok());
        if let Some((hash, result)) = stored {
            let _ = call.reply.send(if hash == fingerprint(&call) {
                result
            } else {
                Err("Request ID was already used with different input".into())
            });
            return;
        }
        let key = (call.session, call.request_id.clone());
        if let Some((hash, result)) = self.app_results.get(&key) {
            let _ = call.reply.send(if hash == &fingerprint(&call) {
                result.clone()
            } else {
                Err("Request ID was already used with different input".into())
            });
            return;
        }
        if !self.app_pending.insert(key) {
            let _ = call.reply.send(Err(
                "Request is still running; retry with the same request ID".into(),
            ));
            return;
        }
        if !call.input.is_object() {
            self.finish_call(call, Err("Input must be a JSON object".into()), cx);
            return;
        }
        let input = call.input.clone();
        let action = call.action.clone();
        let root = self.root.clone();
        let target = mono.id.clone();
        let project = if action.starts_with("sessions.")
            || action.starts_with("folders.")
            || action.starts_with("worktrees.")
        {
            match mono.project(input.get("project").and_then(Value::as_str)) {
                Ok(p) => p,
                Err(e) => {
                    self.finish_call(call, Err(e), cx);
                    return;
                }
            }
        } else {
            String::new()
        };
        let hidden = self.roster.hidden_sessions.clone();
        let run_chat = self.run_chat(call.session, &mono.id);
        let source = self
            .chats
            .get(&mono.id)
            .or_else(|| self.running.get(&call.session).map(|r| &r.view))
            .cloned();
        let result=match action.as_str(){
            "models.list"=>Ok(json!(Kind::ALL.into_iter().map(|kind|json!({"harness":kind.id(),"name":kind.label(),"models":self.chats.values().find(|v|{let v=v.read(cx);let profile=source.as_ref().filter(|s|s.read(cx).summary.provider==kind.id()).map(|s|s.read(cx).settings.account.clone()).unwrap_or_else(||crate::accounts::get(cx).map(|center|center.read(cx).selected(kind,&project)).unwrap_or_default());v.summary.provider==kind.id()&&v.settings.account.id==profile.id}).map(|v|v.read(cx).settings_models()).unwrap_or_default()})).collect::<Vec<_>>())),
            "sessions.start"=>{
                self.start_call(call,mono,project,source,cx);return;
            }
            "sessions.list"|"sessions.read"=>{
                self.call_work(call,target,move|store|{let db=locked(&store)?;let all=db.sessions()?;let sessions:Vec<_>=all.iter().filter(|s|!hidden.contains(&s.id)&&monocode_core::files::path_key(&s.cwd)==monocode_core::files::path_key(&project)).collect();
                    let value=if action=="sessions.list"{json!(sessions.iter().map(|s|json!({"sessionId":s.id,"title":s.title,"harness":s.provider,"project":s.cwd,"archived":s.archived})).collect::<Vec<_>>())}else{let session=id(&input,"sessionId")?;if !sessions.iter().any(|s|s.id==session){return Err("Session is outside the assigned project".into());}let rows=db.load(session)?.rows;let end=input.get("before").and_then(Value::as_u64).unwrap_or(rows.len() as u64).min(rows.len() as u64) as usize;let limit=input.get("limit").and_then(Value::as_u64).unwrap_or(40).clamp(1,200) as usize;let start=end.saturating_sub(limit);let max=input.get("maxChars").and_then(Value::as_u64).unwrap_or(24000).clamp(100,200000) as usize;json!({"sessionId":session,"messages":rows[start..end].iter().map(|r|json!({"role":r.role,"text":monocode_core::response::limit(&r.text,max/limit.max(1))})).collect::<Vec<_>>(),"before":start,"hasMore":start>0})};Ok((value,Effect::None))},cx);return;
            }
            "sessions.send"|"sessions.draft"|"sessions.stop"|"sessions.archive"|"sessions.delete"=>{
                let session=match id(&input,"sessionId"){Ok(id)=>id,Err(e)=>{self.finish_call(call,Err(e),cx);return;}};
                let view=self.owner.upgrade().and_then(|owner|owner.update(cx, |owner, cx| owner.mono_ensure_session(session, cx))).filter(|v|monocode_core::files::path_key(&v.read(cx).summary.cwd)==monocode_core::files::path_key(&project));
                let Some(view)=view else{self.finish_call(call,Err("Session is outside the assigned project".into()),cx);return;};
                if action=="sessions.send"||action=="sessions.draft"{if view.read(cx).busy||view.read(cx).mono_external_pending()||!view.read(cx).input.read(cx).content.trim().is_empty(){self.finish_call(call,Err("Session is busy or has an unsent draft".into()),cx);return;}match text(&input,"prompt",65536){Ok(prompt)=>{let sent=view.update(cx,|v,cx|v.mono_external_send(prompt,action=="sessions.draft",cx));if let Err(e)=sent{self.finish_call(call,Err(e),cx);return;}
if flag(&input,"notifyOnComplete",false){if let Some(m)=self.roster.get_mut(&mono.id){let origin=source.map(|v|v.read(cx).mono_turn).unwrap_or(0);let (origin_turn,each)=report_to(call.session,origin,run_chat,&input);if let Some(launch)=m.launches.iter_mut().find(|l|l.session_id==session){launch.completed=false;launch.dismissed=false;launch.notify_on_complete=true;launch.origin_turn=origin_turn;launch.each=each;}else{let summary=&view.read(cx).summary;m.launches.push(Launch{session_id:session,cwd:summary.cwd.clone(),title:summary.title.clone(),harness:summary.provider.clone(),model:view.read(cx).mono_model(),origin_turn,notify_on_complete:true,each,completed:false,dismissed:false});}}self.save(cx);}
if action=="sessions.send"{self.note_run_session(call.session,session,false);}Ok(json!({"accepted":true}))},Err(e)=>Err(e)}}else{
                    for m in &mut self.roster.monos{for launch in &mut m.launches{if launch.session_id==session{launch.dismissed=true;}}}self.save(cx);
                    if action=="sessions.stop"{view.update(cx,|v,cx|v.stop_if_busy(cx));}else{let delete=action=="sessions.delete";let owner=self.owner.clone();let handle=window.window_handle();let app:&mut App=cx;app.defer(move|cx|{let _=handle.update(cx,|_,window,cx|{let _=owner.update(cx,|owner,cx|owner.mono_remove(session,delete,window,cx));});});}Ok(json!({"accepted":true}))
                }
            }
            "folders.list"=>Ok(json!(self.owner.upgrade().map(|o|o.read(cx).mono_groups(&project)).unwrap_or_default().folders)),
            "folders.move"=>{
                let session=match id(&input,"sessionId"){Ok(id)=>id,Err(e)=>{self.finish_call(call,Err(e),cx);return;}};
                let allowed=self.owner.upgrade().and_then(|o|o.update(cx, |owner, cx| owner.mono_ensure_session(session, cx))).is_some_and(|v|v.read(cx).summary.cwd==project);
                if !allowed{self.finish_call(call,Err("Session is outside this project".into()),cx);return;}
                let mut groups=self.owner.upgrade().map(|o|o.read(cx).mono_groups(&project)).unwrap_or_default();
                if let Some(name)=input.get("newFolderName").and_then(Value::as_str){if let Some(folder)=groups.create_folder(&[session],mono::new_id()){groups.rename_folder(&folder,name);}}
                else if let Some(folder)=input.get("folderId").and_then(Value::as_str){if groups.folder(folder).is_none(){self.finish_call(call,Err("Folder not found".into()),cx);return;}groups.add_to_folder(folder,session);}else{groups.remove_sessions(&[session]);}
                self.call_work(call,target,move|store|{locked(&store)?.save_groups(&project,&groups)?;Ok((json!({"moved":true}),Effect::Groups(project,groups)))},cx);return;
            }
            "artifacts.list"=>Ok(json!(mono.documents.iter().filter(|d|input.get("kind").and_then(Value::as_str).is_none_or(|k|k==d.kind)).skip(input.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize).take(input.get("limit").and_then(Value::as_u64).unwrap_or(50).min(200) as usize).collect::<Vec<_>>())),
            "artifacts.read"=>input.get("id").and_then(Value::as_str).and_then(|id|mono.documents.iter().find(|d|d.id==id)).map(|d|json!(d)).ok_or("Document not found".into()),
            "chat.card"=>{
                let mut card=match mono::cards::parse(&input){Ok(c)=>c,Err(e)=>{self.finish_call(call,Err(e),cx);return;}};
                card["id"]=json!(mono::new_id());
                if card["type"]=="session"{
                    let session=card["sessionId"].as_i64().unwrap();
                    let view=self.owner.upgrade().and_then(|o|o.update(cx, |owner, cx| owner.mono_ensure_session(session, cx))).filter(|v|mono.works_on(&v.read(cx).summary.cwd));
                    let Some(view)=view else{self.finish_call(call,Err("Session is outside assigned projects".into()),cx);return;};
                    card["title"]=json!(view.read(cx).summary.title);card["state"]=json!(if view.read(cx).busy{"Working"}else{"Saved"});
                }
                if card["type"]=="pr"{
                    let cwd=mono.projects.first().cloned().unwrap_or_else(||std::env::var("HOME").unwrap_or_else(|_|".".into()));
                    self.call_work(call,target,move|_|{let metadata=mono::cards::pull_request(&cwd,card["repo"].as_str(),card["number"].as_u64().unwrap())?;for key in ["title","state","url","isDraft","additions","deletions"]{card[key]=metadata[key].clone();}Ok((card.clone(),Effect::Card(card)))},cx);return;
                }
                self.hold_card(call.session,&mono.id,card.clone());self.save(cx);Ok(card)
            }
            "artifacts.write"=>{
                let kind=input.get("kind").and_then(Value::as_str).unwrap_or("document");
                if !["document","html","markdown"].contains(&kind){Err("Use document or html".into())}else{match text(&input,"title",512).and_then(|title|Ok((title,text(&input,"body",512*1024)?))){Err(e)=>Err(e),Ok((title,body))=>{
                    let id=input.get("id").and_then(Value::as_str).map(str::to_owned).unwrap_or_else(mono::new_id);let now=now_ms();let document=Document{id:id.clone(),kind:kind.into(),title,summary:input.get("summary").and_then(Value::as_str).unwrap_or_default().chars().take(4000).collect(),body,created_at:mono.documents.iter().find(|d|d.id==id).map(|d|d.created_at).unwrap_or(now),updated_at:now};
                    if let Some(m)=self.roster.get_mut(&mono.id){if let Some(old)=m.documents.iter_mut().find(|d|d.id==id){*old=document.clone();}else{m.documents.push(document.clone());}}
                    self.documents.remove(&id);self.html_documents.remove(&id);self.hold_card(call.session,&mono.id,json!({"id":mono::new_id(),"type":"document","documentId":id,"title":document.title,"summary":document.summary}));self.save(cx);Ok(json!(document))
                }}}
            }
            "soul.read"|"soul.update"|"memory.read"|"memory.search"|"memory.add"|"memory.replace"|"memory.remove"|"habits.list"|"habits.add"|"habits.update"|"habits.run"|"habits.remove"|"notes.list"|"notes.read"|"notes.write"|"worktrees.list"|"worktrees.create"=>{
                let caller=call.session;let projects=mono.projects.clone();
                self.call_work(call,target.clone(),move|store|{
                    let value=match action.as_str(){
                        "soul.read"=>json!(files::read(&root,&target,None,"SOUL.md")?),
                        "soul.update"=>{let body=input.get("text").and_then(Value::as_str).filter(|t|t.len()<=128*1024).ok_or("Provide text")?;let expected=text(&input,"expectedHash",128)?;json!({"hash":files::save(&root,&target,None,"SOUL.md",body,Some(&expected))?})},
                        "memory.read"=>json!(memory::read(&root,&target,input.get("topic").and_then(Value::as_str))?),
                        "memory.search"=>json!(memory::search(&root,&target,&text(&input,"query",4000)?,input.get("since").and_then(Value::as_str))?),
                        "memory.add"|"memory.replace"|"memory.remove"=>{let entry=if action=="memory.remove"{None}else{Some(memory::entry(&text(&input,"fact",4000)?,&memory::date(),input.get("until").and_then(Value::as_str))?)};let find=input.get("find").and_then(Value::as_str).unwrap_or_default();let hash=memory::edit(&root,&target,input.get("topic").and_then(Value::as_str),|old|if action=="memory.add"{Ok(memory::add(old,entry.as_deref().unwrap()))}else{memory::replace(old,find,entry.as_deref(),&memory::date())})?;json!({"hash":hash})},
                        "habits.list"=>json!(habits::load(&root,&target)?),
                        "habits.add"=>{let schedule=habits::Schedule::from_cli(input.get("schedule").ok_or("Provide schedule")?)?;let h=Habit::new(text(&input,"name",480)?,text(&input,"instructions",32000)?,schedule,now_ms())?;habits::update(&root,&target,|all|{all.push(h.clone());Ok(())})?;json!(h)},
                        "habits.update"|"habits.run"|"habits.remove"=>{let id=text(&input,"id",128)?;habits::update(&root,&target,|all|{let index=all.iter().position(|h|h.id==id).ok_or("Habit not found")?;if action=="habits.remove"{all.remove(index);return Ok(json!({"removed":true}));}let h=&mut all[index];if action=="habits.run"{h.run_requested=true;}else{if let Some(name)=input.get("name").and_then(Value::as_str){if name.trim().is_empty()||name.chars().count()>120{return Err("Invalid habit name".into());}h.name=name.trim().into();}
if let Some(instructions)=input.get("instructions").and_then(Value::as_str){if instructions.trim().is_empty()||instructions.len()>32000{return Err("Invalid habit instructions".into());}h.instructions=instructions.into();}
if let Some(schedule)=input.get("schedule"){h.reschedule(habits::Schedule::from_cli(schedule)?,now_ms())?;}
if let Some(enabled)=input.get("enabled").and_then(Value::as_bool){h.enabled=enabled;if enabled{h.next_run_at=h.schedule.next(now_ms())?;}}}Ok(json!(h))})?},
                        "notes.list"|"notes.read"|"notes.write"=>{let db=locked(&store)?;let notes=db.notes()?;let allowed=|n:&&monocode_core::response::Note|projects.iter().any(|p|monocode_core::files::path_key(p)==monocode_core::files::path_key(&n.source_cwd));match action.as_str(){"notes.list"=>json!(notes.iter().filter(allowed).skip(input.get("offset").and_then(Value::as_u64).unwrap_or(0) as usize).take(input.get("limit").and_then(Value::as_u64).unwrap_or(50).min(200) as usize).collect::<Vec<_>>()),"notes.read"=>{let id=id(&input,"id")? as u64;json!(notes.iter().filter(allowed).find(|n|n.id==id).ok_or("Note not found in assigned projects")?)},_=>{let id=input.get("id").and_then(Value::as_u64).unwrap_or_else(now_ms);if notes.iter().any(|n|n.id==id&&!projects.iter().any(|p|p==&n.source_cwd)){return Err("Note is outside assigned projects".into());}let cwd=notes.iter().find(|n|n.id==id).map(|n|n.source_cwd.clone()).or_else(||projects.first().cloned()).ok_or("Assign a project before writing notes")?;let note=monocode_core::response::Note{id,title:text(&input,"title",512)?,body:text(&input,"body",256*1024)?,tags:input.get("tags").and_then(Value::as_array).map(|tags|tags.iter().filter_map(Value::as_str).map(str::to_owned).collect()).or_else(||notes.iter().find(|n|n.id==id).map(|n|n.tags.clone())).unwrap_or_default(),source_session:caller,source_cwd:cwd};db.save_note(&note)?;json!(note)}}},
                        "worktrees.list"=>json!(monocode_core::git::snapshot(Path::new(&project))?.worktrees.iter().map(|w|json!({"path":w.path,"branch":w.branch,"head":w.head,"main":w.main})).collect::<Vec<_>>()),
                        "worktrees.create"=>{let branch=text(&input,"branch",240)?;let tree=monocode_core::git::create_project_worktree(Path::new(&project),&branch,input.get("base").and_then(Value::as_str),flag(&input,"existing",false))?;json!({"path":tree.path,"branch":tree.branch})},
                        _=>unreachable!(),
                    };Ok((value,Effect::None))
                },cx);return;
            }
            _=>Err("Unknown action. Run app help to see supported actions".into()),
        };
        self.finish_call(call, result, cx);
    }
    /// sessions.start. Every check runs before the session row exists.
    fn start_call(
        &mut self,
        call: Call,
        mono: mono::Mono,
        project: String,
        source: Option<Entity<Conversation>>,
        cx: &mut Context<Self>,
    ) {
        let input = call.input.clone();
        let target = mono.id.clone();
        let prompt = match text(&input, "prompt", 65536) {
            Ok(t) => t,
            Err(e) => return self.finish_call(call, Err(e), cx),
        };
        let provider = input
            .get("harness")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or_else(|| source.as_ref().map(|v| v.read(cx).summary.provider.clone()))
            .unwrap_or_else(|| "codex".into());
        if Kind::parse(&provider).is_none() {
            return self.finish_call(call, Err("Unknown provider".into()), cx);
        }
        let same_provider = source
            .as_ref()
            .is_some_and(|v| v.read(cx).summary.provider == provider);
        let mut settings = source
            .map(|v| v.read(cx).settings.clone())
            .unwrap_or_default();
        settings.mono = None;
        settings.environment.clear();
        settings.internal_message = false;
        settings.turns.clear();
        settings.attachments.clear();
        settings.usage_limit = None;
        if !same_provider {
            settings.account = crate::accounts::get(cx)
                .map(|center| center.read(cx).selected(Kind::from_id(&provider), &project))
                .unwrap_or_default();
            settings.model = None;
            settings.effort = None;
            settings.model_settings.clear();
            settings.fast = false;
            settings.service_tier = None;
            settings.thinking = None;
        }
        if let Some(model) = input.get("model").and_then(Value::as_str) {
            settings.model = Some(model.into());
        }
        if let Some(values) = input.get("modelSettings") {
            match serde_json::from_value(values.clone()) {
                Ok(values) => settings.model_settings = values,
                Err(_) => {
                    let error = "modelSettings must be an object of strings".into();
                    return self.finish_call(call, Err(error), cx);
                }
            }
        }
        if let Some(mode) = input.get("runtimeMode").and_then(Value::as_str) {
            settings.access = AccessMode::from_id(mode);
        }
        let launches = mono.launches.clone();
        let (draft, reveal) = (flag(&input, "draft", false), flag(&input, "reveal", false));
        // The parent must be a session this Mono (or its habit run) started.
        let parent = match input.get("parentSessionId") {
            None => None,
            Some(_) => match id(&input, "parentSessionId") {
                Ok(parent) if mono.launches.iter().any(|l| l.session_id == parent) => Some(parent),
                Ok(_) => {
                    let error = "parentSessionId must be a session this Mono started".into();
                    return self.finish_call(call, Err(error), cx);
                }
                Err(_) => {
                    let error = "parentSessionId must be a session ID (a positive number)".into();
                    return self.finish_call(call, Err(error), cx);
                }
            },
        };
        // A launch hidden from the sidebar is validated like any other but not filed.
        let groups = self
            .owner
            .upgrade()
            .map(|o| o.read(cx).mono_groups(&project))
            .unwrap_or_default();
        let folder = match folder_choice(&groups, &input) {
            Ok(folder) => folder.filter(|_| mono.show_started_sessions_in_sidebar),
            Err(e) => return self.finish_call(call, Err(e), cx),
        };
        let filed = folder.as_ref().map(|f| {
            let (id, name) = f.entry();
            (id.to_owned(), name.to_owned())
        });
        self.call_work(
            call,
            target,
            move |store| {
                let path = Path::new(&project);
                let worktree = input.get("workspaceMode").and_then(Value::as_str) == Some("worktree");
                if worktree && monocode_core::git::unborn(path) {
                    return Err("This project has no commits yet; make an initial commit before using workspaceMode worktree".into());
                }
                let chosen = match input.get("worktreeCwd").and_then(Value::as_str) {
                    Some(cwd) => {
                        let snapshot = monocode_core::git::snapshot(path)?;
                        let key = monocode_core::files::path_key(cwd);
                        if !snapshot.worktrees.iter().any(|w| monocode_core::files::path_key(&w.path) == key) {
                            return Err("Choose an existing worktree of the assigned project".into());
                        }
                        Some(cwd.to_owned())
                    }
                    None => None,
                };
                let mut db = locked(&store)?;
                if let Some(parent) = parent {
                    let key = monocode_core::files::path_key(&project);
                    if !db.sessions()?.iter().any(|s| s.id == parent && monocode_core::files::path_key(&s.cwd) == key) {
                        return Err("Session was not found in this project".into());
                    }
                    if is_child_launch(&db.started_links()?, &launches, parent) {
                        return Err("parentSessionId cannot be a session started under another one; a child of a child is not allowed".into());
                    }
                }
                // Always a chat: the app drives this session, whatever the surface preference.
                let mut summary = db.create(&provider, &project)?;
                let mut made = None;
                let built = (|| {
                    if let Some(cwd) = chosen {
                        settings.worktree_cwd = Some(cwd);
                    } else if worktree {
                        let base = input.get("worktreeBase").and_then(Value::as_str).unwrap_or("HEAD");
                        let tree = monocode_core::git::create_worktree(path, base, summary.id)?;
                        settings.worktree_cwd = Some(tree.to_string_lossy().into());
                        made = Some(tree);
                    }
                    summary.title = input.get("title").and_then(Value::as_str).unwrap_or(&prompt).chars().take(160).collect();
                    db.title(summary.id, &summary.title)?;
                    db.configure_agent(summary.id, &provider, &settings)?;
                    if let Some(parent) = parent {
                        db.save_started_link(summary.id, parent)?;
                    }
                    Ok::<_, String>(())
                })();
                if let Err(e) = built {
                    // Nothing else refers to the row or its worktree yet.
                    let _ = db.delete(summary.id);
                    let e = match made.map(|tree| monocode_core::worktree_management::discard_new(&tree).map_err(|c| (tree, c))) {
                        Some(Err((tree, cause))) => format!("{e} (the new worktree {} was left on disk: {cause})", tree.display()),
                        _ => e,
                    };
                    return Err(e);
                }
                let mut result = json!({"sessionId":summary.id,"accepted":true,"project":project});
                if let Some((id, name)) = filed {
                    result["folderId"] = json!(id);
                    result["folderName"] = json!(name);
                }
                Ok((
                    result,
                    Effect::Start(Box::new(Started { summary, settings, prompt, draft, reveal, parent, folder })),
                ))
            },
            cx,
        );
    }
    /// The Mono chat that gets a habit run's reports (0 before it has a chat),
    /// or `None` when the caller is not a habit run.
    fn run_chat(&self, caller: i64, mono: &str) -> Option<i64> {
        self.running.contains_key(&caller).then(|| {
            self.roster
                .get(mono)
                .and_then(|m| m.session_id)
                .unwrap_or(0)
        })
    }
    /// Remembers a session a habit run started or sent a follow-up to, for its post.
    /// `created` marks the ones the run started itself: only those are stopped with
    /// the run, and only those count as "kept running" when it is stopped.
    pub(super) fn note_run_session(&mut self, caller: i64, session: i64, created: bool) {
        if let Some(run) = self.running.get_mut(&caller) {
            if !run.sessions.contains(&session) {
                run.sessions.push(session);
            }
            if created && !run.created.contains(&session) {
                run.created.push(session);
            }
        }
    }
    fn apply_groups(&mut self, project: String, groups: Groups, cx: &mut Context<Self>) {
        let _ = self.owner.update(cx, |o, cx| {
            o.mono_apply_groups(project.clone(), groups.clone(), cx)
        });
        let store = self.store.clone();
        cx.background_executor()
            .spawn(async move {
                let _ = locked(&store).and_then(|db| db.save_groups(&project, &groups));
            })
            .detach();
    }
    /// Files a launch where the request chose.
    fn file_launch(
        &mut self,
        mono: &str,
        project: &str,
        session: i64,
        folder: Folder,
        cx: &mut Context<Self>,
    ) {
        let color = self.roster.get(mono).and_then(|m| folder_color(m.color));
        let mut groups = self
            .owner
            .upgrade()
            .map(|o| o.read(cx).mono_groups(project))
            .unwrap_or_default();
        match folder {
            Folder::Existing { id, .. } => groups.add_to_folder(&id, session),
            Folder::New { id, name } => {
                if let Some(folder) = groups.create_folder(&[session], id) {
                    groups.rename_folder(&folder, &name);
                    groups.set_color(&folder, color);
                }
            }
        }
        self.apply_groups(project.to_owned(), groups, cx);
    }
    pub(super) fn group_launch(&mut self, id: &str, session: i64, cx: &mut Context<Self>) {
        let Some(mono) = self
            .roster
            .get(id)
            .filter(|m| m.show_started_sessions_in_sidebar && m.use_sidebar_folders)
            .cloned()
        else {
            return;
        };
        let Some(launch) = mono.launches.iter().find(|l| l.session_id == session) else {
            return;
        };
        let project = launch.cwd.clone();
        let mut groups = self
            .owner
            .upgrade()
            .map(|o| o.read(cx).mono_groups(&project))
            .unwrap_or_default();
        let folder = groups
            .folders
            .iter()
            .find(|f| f.name == mono.label())
            .map(|f| f.id.clone())
            .or_else(|| groups.create_folder(&[session], mono::new_id()));
        if let Some(folder) = folder {
            groups.rename_folder(&folder, &mono.label());
            groups.set_color(&folder, folder_color(mono.color));
            groups.add_to_folder(&folder, session);
        }
        self.apply_groups(project, groups, cx);
    }
    pub(super) fn check_completions(&mut self, cx: &mut Context<Self>) {
        let views = self
            .owner
            .upgrade()
            .map(|o| o.read(cx).mono_session_views(cx))
            .unwrap_or_default();
        let mut ready = Vec::new();
        for mono in &self.roster.monos {
            let source_busy = |source: i64| {
                self.chats
                    .values()
                    .chain(self.running.values().map(|r| &r.view))
                    .any(|v| v.read(cx).summary.id == source && v.read(cx).busy)
            };
            let stopped = |id: i64| {
                views
                    .iter()
                    .find(|v| v.id == id)
                    .is_none_or(|v| !v.busy && v.has_messages)
            };
            for (batch, sessions) in ready_batches(&mono.launches, source_busy, stopped) {
                if !self.completion_work.contains(&(mono.id.clone(), batch)) {
                    ready.push((mono.id.clone(), batch, sessions));
                }
            }
        }
        for (id, batch, sessions) in ready {
            self.completion_work.insert((id.clone(), batch));
            let store = self.store.clone();
            let work = cx.background_executor().spawn(async move {
                sessions
                    .into_iter()
                    .map(|(id, title)| {
                        let prose = locked(&store)
                            .and_then(|db| db.load(id))
                            .map(|t| monocode_core::response::prose(&t.rows))
                            .unwrap_or_else(|_| "Session was removed".into());
                        (id, title, prose)
                    })
                    .collect::<Vec<_>>()
            });
            cx.spawn(async move |this, cx| {
                let results = work.await;
                let _ = this.update(cx, |this, cx| {
                    this.completion_work.remove(&(id.clone(), batch));
                    if let Some(mono) = this.roster.get_mut(&id) {
                        finish_batch(&mut mono.launches, batch);
                    }
                    // Always the Mono's chat: a habit run's sessions report there too.
                    this.enqueue(
                        &id,
                        completion_message(batch, &results),
                        Vec::new(),
                        true,
                        cx,
                    );
                    this.ensure_chat(&id, cx);
                });
            })
            .detach();
        }
    }
}
