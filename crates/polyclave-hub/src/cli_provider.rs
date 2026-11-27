//! Read-only CLI Hub wire format and host-owned continuation.
use crate::{
    provider::{
        assemble, ChatMessage, HubModelProvider, HubReply, HubRequest, ProviderEvent,
        ProviderFuture, Role,
    },
    Error, Result,
};
use polyclave_proto::{AgentEvent, MemoryItemDto, Vendor};
use polyclave_store::{HubCompletion, HubTurn, Store, ThreadDelta};
use serde_json::{json, Value};
use std::{
    ffi::OsString,
    io::{BufRead, Read},
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::sync::broadcast;

const HUB_TOOLS: &[&str] = &[
    "list_repo_files",
    "read_repo_file",
    "search_repo",
    "get_run_diff",
    "memory_brief",
    "memory_search",
    "memory_propose",
];
const CENTRAL_TOOLS: &[&str] = &[
    "create_or_update_ticket",
    "dispatch_task",
    "memory_commit",
    "memory_supersede",
    "memory_archive",
];

fn args(
    vendor: Vendor,
    model: &str,
    thread: Option<&str>,
    manager: bool,
    mcp_url: &str,
    config: &Path,
) -> Vec<OsString> {
    match vendor {
        Vendor::Codex => {
            let mut a = vec!["exec".into()];
            if thread.is_some() {
                a.push("resume".into());
            }
            a.extend(
                ["--json", "--ignore-user-config", "--skip-git-repo-check"].map(OsString::from),
            );
            for opt in [
                "sandbox_mode=\"read-only\"",
                "approval_policy=\"never\"",
                "features.shell_tool=false",
                "features.view_image=false",
                "web_search=\"disabled\"",
                "features.remote_plugin=false",
                "mcp_servers.host.bearer_token_env_var=\"POLYCLAVE_MCP_TOKEN\"",
                "mcp_servers.host.required=true",
            ] {
                a.extend(["-c".into(), opt.into()]);
            }
            a.extend([
                "-c".into(),
                format!("mcp_servers.host.url={}", json!(mcp_url)).into(),
            ]);
            if model != "default" {
                a.extend(["--model".into(), model.into()]);
            }
            a.push("--".into());
            if let Some(t) = thread {
                a.push(t.into());
            }
            a.push("-".into());
            a
        }
        Vendor::Claude => {
            let mut a = [
                "-p",
                "--verbose",
                "--output-format",
                "stream-json",
                "--include-partial-messages",
                "--strict-mcp-config",
                "--setting-sources",
                "",
                "--tools",
                "",
                "--max-turns",
                "8",
                "--mcp-config",
            ]
            .map(OsString::from)
            .to_vec();
            a.push(config.into());
            if model != "default" {
                a.extend(["--model".into(), model.into()]);
            }
            if let Some(t) = thread {
                a.extend(["--resume".into(), t.into()]);
            }
            let offered = HUB_TOOLS
                .iter()
                .chain(if manager { &[] } else { CENTRAL_TOOLS });
            a.extend([
                "--allowedTools".into(),
                offered
                    .map(|t| format!("mcp__host__{t}"))
                    .collect::<Vec<_>>()
                    .join(",")
                    .into(),
            ]);
            a
        }
    }
}

#[derive(Clone, Debug)]
pub struct Conversation {
    pub native_thread: Option<String>,
    pub history: Vec<ChatMessage>,
    pub included: Vec<i64>,
}

pub struct CliHubProvider {
    store: Store,
    transport: Arc<dyn CliTransport>,
}
pub trait CliTransport: Send + Sync + 'static {
    fn execute(
        &self,
        claim: &HubTurn,
        session: &Conversation,
        events: broadcast::Sender<ProviderEvent>,
    ) -> Result<HubReply>;
    fn terminal(&self, _turn: i64) {}
}

impl CliHubProvider {
    pub fn new(store: Store, transport: Arc<dyn CliTransport>) -> Self {
        Self { store, transport }
    }
    pub fn prepare(
        &self,
        claim: &HubTurn,
        rebuild: bool,
        system: &str,
        memory: &[MemoryItemDto],
    ) -> Result<Conversation> {
        let ThreadDelta {
            handle, messages, ..
        } = self
            .store
            .hub_delta(claim, rebuild)
            .map_err(|_| Error::Store)?;
        let included = messages.iter().map(|m| m.id).collect();
        let history: Vec<_> = messages
            .into_iter()
            .filter(|m| m.id != claim.input.id)
            .map(|m| {
                let role = if m.source == "agent" {
                    Role::Assistant
                } else {
                    Role::User
                };
                let mut msg = ChatMessage::text(role, m.text);
                msg.reasoning_content = m.reasoning;
                msg
            })
            .collect();
        let input = ChatMessage::text(Role::User, claim.input.text.clone());
        let history = if handle.is_none() {
            assemble(
                system,
                memory,
                &claim.summary,
                &history,
                &Value::Null,
                input,
            )?
        } else {
            history.into_iter().chain([input]).collect()
        };
        Ok(Conversation {
            native_thread: handle,
            history,
            included,
        })
    }
    pub fn reply(
        &self,
        mut claim: HubTurn,
        system: &str,
        memory: &[MemoryItemDto],
        events: broadcast::Sender<ProviderEvent>,
    ) -> Result<HubReply> {
        let mut session = self.prepare(&claim, false, system, memory)?;
        let mut rebuilt = false;
        let output = self.transport.execute(&claim, &session, events.clone());
        let result = match output {
            Err(Error::MissingThread) if session.native_thread.is_some() => {
                rebuilt = true;
                let old = claim.id;
                claim = self
                    .store
                    .hub_replace_missing_turn(claim)
                    .map_err(|_| Error::Store)?;
                self.transport.terminal(old);
                session = self.prepare(&claim, true, system, memory)?;
                self.transport.execute(&claim, &session, events)
            }
            other => other,
        };
        let reply = match result {
            Ok(reply) => reply,
            Err(e) => {
                let reason = if matches!(e, Error::NotStarted) {
                    "never_started"
                } else {
                    "exit"
                };
                self.store
                    .hub_end_without_reply(claim.id, reason)
                    .map_err(|_| Error::Store)?;
                self.transport.terminal(claim.id);
                return Err(e);
            }
        };
        if reply.native_thread.is_none() {
            self.store
                .hub_end_without_reply(claim.id, "exit")
                .map_err(|_| Error::Store)?;
            self.transport.terminal(claim.id);
            return Err(Error::Protocol);
        }
        let id = claim.id;
        self.store
            .hub_finish(
                claim,
                HubCompletion {
                    text: reply.text.clone(),
                    reasoning: Some(reply.reasoning.clone()),
                    metadata: (!reply.calls.is_empty()).then(|| json!({"tool_calls":reply.calls})),
                    native_thread: reply.native_thread.clone(),
                    delivered: session.included,
                    rebuilt,
                },
            )
            .map_err(|_| Error::Store)?;
        self.transport.terminal(id);
        Ok(reply)
    }
}

/// One provider request consumes exactly one durable claim.
pub struct ClaimedCliProvider {
    provider: Arc<CliHubProvider>,
    claim: std::sync::Mutex<Option<HubTurn>>,
    system: String,
    memory: Vec<MemoryItemDto>,
}
impl ClaimedCliProvider {
    pub fn new(
        provider: Arc<CliHubProvider>,
        claim: HubTurn,
        system: String,
        memory: Vec<MemoryItemDto>,
    ) -> Self {
        Self {
            provider,
            claim: std::sync::Mutex::new(Some(claim)),
            system,
            memory,
        }
    }
}
impl HubModelProvider for ClaimedCliProvider {
    fn complete(
        &self,
        request: HubRequest,
        events: broadcast::Sender<ProviderEvent>,
    ) -> ProviderFuture<'_> {
        Box::pin(async move {
            let claim = {
                let mut slot = self.claim.lock().map_err(|_| Error::Store)?;
                if slot.as_ref().is_none_or(|item| item.model != request.model) {
                    return Err(Error::Profile);
                }
                slot.take().ok_or(Error::Store)?
            };
            let (provider, system, memory) = (
                self.provider.clone(),
                self.system.clone(),
                self.memory.clone(),
            );
            tokio::task::spawn_blocking(move || provider.reply(claim, &system, &memory, events))
                .await
                .map_err(|_| Error::Transport)?
        })
    }
}

/// Only the daemon supplies the executable, host-owned workspace, and MCP
/// endpoint. The workspace is a dedicated empty directory, never the user's repo.
pub struct SupervisedCli {
    pub store: Store,
    pub vendor: Vendor,
    pub supervisor: PathBuf,
    pub executable: PathBuf,
    pub home: PathBuf,
    pub scratch: PathBuf,
    pub tmp: PathBuf,
    pub mcp_url: String,
    pub manager: bool,
}
impl CliTransport for SupervisedCli {
    fn execute(
        &self,
        claim: &HubTurn,
        conversation: &Conversation,
        events: broadcast::Sender<ProviderEvent>,
    ) -> Result<HubReply> {
        let url = reqwest::Url::parse(&self.mcp_url).map_err(|_| Error::NotStarted)?;
        if url.scheme() != "http"
            || url.host_str() != Some("127.0.0.1")
            || url.path() != "/mcp"
            || url.port().is_none()
            || url.fragment().is_some()
            || url.query().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(Error::NotStarted);
        }
        let scratch =
            polyclave_os::canonical_real_path(&self.scratch).map_err(|_| Error::NotStarted)?;
        let repo = polyclave_os::canonical_real_path(Path::new(&claim.repo))
            .map_err(|_| Error::NotStarted)?;
        if scratch.starts_with(&repo) || repo.starts_with(&scratch) {
            return Err(Error::NotStarted);
        }
        let config = self.tmp.join(format!("mcp-turn-{}.json", claim.id));
        let marker = self.tmp.join(format!("turn-{}.released", claim.id));
        let cleanup = Cleanup::new(config.clone());
        if self.vendor == Vendor::Claude {
            let data = json!({"mcpServers":{"host":{"type":"http","url":self.mcp_url,
                "headers":{"Authorization":format!("Bearer {}",claim.turn_credential())}}}});
            polyclave_os::write_private_file(
                &config,
                &serde_json::to_vec(&data).map_err(|_| Error::NotStarted)?,
            )
            .map_err(|_| Error::NotStarted)?;
        }
        let mut env: Vec<(OsString, OsString)> = [
            "PATH",
            "HOME",
            "PATHEXT",
            "SYSTEMROOT",
            "PROGRAMFILES",
            "WINDIR",
            "COMSPEC",
            "USERPROFILE",
            "LOCALAPPDATA",
            "APPDATA",
            "TEMP",
            "TMP",
            "TMPDIR",
            "LANG",
        ]
        .into_iter()
        .filter_map(|k| std::env::var_os(k).map(|v| (k.into(), v)))
        .collect();
        if self.vendor == Vendor::Codex {
            env.push(("CODEX_HOME".into(), self.home.clone().into_os_string()));
            env.push(("POLYCLAVE_MCP_TOKEN".into(), claim.turn_credential().into()));
        } else {
            env.push((
                "CLAUDE_CONFIG_DIR".into(),
                self.home.clone().into_os_string(),
            ));
        }
        let mut child = polyclave_os::spawn_supervised(&polyclave_os::SpawnSpec {
            supervisor: self.supervisor.clone(),
            worker: polyclave_os::resolve_cli_binary(&self.executable)
                .map_err(|_| Error::NotStarted)?,
            args: args(
                self.vendor,
                &claim.model,
                conversation.native_thread.as_deref(),
                self.manager,
                &self.mcp_url,
                &config,
            ),
            cwd: scratch,
            env,
            released: marker,
        })
        .map_err(|_| Error::NotStarted)?;
        let identity = child.identity().clone();
        self.store
            .hub_authorize(claim.id, Some((identity.pid, identity.started_at)))
            .map_err(|_| Error::NotStarted)?;
        let prompt = serde_json::to_vec(&conversation.history).map_err(|_| Error::NotStarted)?;
        child.go(&prompt).map_err(|_| Error::Transport)?;
        if let Some(mut stderr) = child.take_stderr() {
            std::thread::spawn(move || {
                let _ = std::io::copy(&mut stderr, &mut std::io::sink());
            });
        }
        let mut output = child.take_stdout().ok_or(Error::Transport)?;
        let mut result = HubReply::default();
        let mut done = false;
        let mut refused_thread = false;
        let mut saw_activity = false;
        let mut size = 0usize;
        loop {
            let mut line = String::new();
            let n = (&mut output)
                .take(1024 * 1024)
                .read_line(&mut line)
                .map_err(|_| Error::Transport)?;
            if n == 0 {
                break;
            }
            size += n;
            if size > 4 * 1024 * 1024 || n == 1024 * 1024 {
                return Err(Error::Limit);
            }
            if line.contains(claim.turn_credential()) {
                continue;
            }
            if let Ok(v) = serde_json::from_str::<Value>(&line) {
                if v["type"] == "error"
                    && matches!(
                        v["code"].as_str(),
                        Some("session_not_found" | "thread_not_found")
                    )
                {
                    refused_thread = true;
                }
                if matches!(
                    v["type"].as_str(),
                    Some("item.started" | "item.completed" | "assistant" | "stream_event")
                ) {
                    saw_activity = true;
                }
            }
            let event = match self.vendor {
                Vendor::Codex => polyclave_adapters::parse_codex(&line),
                Vendor::Claude => polyclave_adapters::parse_claude(&line),
            };
            match event {
                Some(AgentEvent::Session { id }) => {
                    if id.is_empty() || id.len() > 128 {
                        return Err(Error::Protocol);
                    }
                    self.store
                        .hub_record_handle(&claim.session, &claim.profile, claim.id, &id)
                        .map_err(|_| Error::Store)?;
                    result.native_thread = Some(id);
                }
                Some(AgentEvent::TextDelta { text }) => {
                    result.text.push_str(&text);
                    let _ = events.send(ProviderEvent::Text(text));
                }
                Some(AgentEvent::ReasoningDelta { text }) => {
                    result.reasoning.push_str(&text);
                    let _ = events.send(ProviderEvent::Reasoning(text));
                }
                Some(AgentEvent::Finished { success }) => done = success,
                _ => (),
            }
        }
        drop(cleanup);
        if child.wait().map_err(|_| Error::Transport)? != 0 || !done {
            if refused_thread && !saw_activity && result.native_thread.is_none() {
                return Err(Error::MissingThread);
            }
            return Err(Error::Transport);
        }
        if result.native_thread.is_none() {
            result.native_thread = conversation.native_thread.clone();
        }
        let _ = events.send(ProviderEvent::Finished);
        Ok(result)
    }
    fn terminal(&self, turn: i64) {
        let _ = std::fs::remove_file(self.tmp.join(format!("turn-{turn}.released")));
    }
}
struct Cleanup(PathBuf);
impl Cleanup {
    fn new(p: PathBuf) -> Self {
        Self(p)
    }
}
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn claude_subscription_hub_strips_builtin_tools_and_manager_lacks_dispatch() {
        let arguments = args(
            Vendor::Claude,
            "fixture",
            Some("thread"),
            true,
            "http://127.0.0.1:1/mcp",
            Path::new("config.json"),
        );
        assert!(arguments
            .windows(2)
            .any(|p| p[0] == "--tools" && p[1].is_empty()));
        assert!(arguments
            .windows(2)
            .any(|p| p[0] == "--resume" && p[1] == "thread"));
        assert!(arguments
            .iter()
            .any(|a| a.to_string_lossy().contains("mcp__host__read_repo_file")));
        assert!(!arguments
            .iter()
            .any(|a| a.to_string_lossy().contains("dispatch_task") || a == "--bare"));
    }
}

#[cfg(test)]
mod ledger_tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    };

    struct Fake {
        store: Store,
        calls: Mutex<Vec<Conversation>>,
        fail_resume: AtomicBool,
    }
    impl CliTransport for Fake {
        fn execute(
            &self,
            claim: &HubTurn,
            conversation: &Conversation,
            events: broadcast::Sender<ProviderEvent>,
        ) -> Result<HubReply> {
            self.store.hub_authorize(claim.id, None).unwrap();
            let mut calls = self.calls.lock().unwrap();
            calls.push(conversation.clone());
            if conversation.native_thread.is_some()
                && self.fail_resume.swap(false, Ordering::AcqRel)
            {
                return Err(Error::MissingThread);
            }
            let text = format!("reply-{}", calls.len());
            let _ = events.send(ProviderEvent::Text(text.clone()));
            Ok(HubReply {
                text,
                native_thread: Some(
                    conversation
                        .native_thread
                        .clone()
                        .unwrap_or_else(|| format!("native-{}", calls.len())),
                ),
                ..Default::default()
            })
        }
    }
    fn fixture() -> (tempfile::TempDir, Store, Arc<Fake>) {
        let d = tempfile::tempdir().unwrap();
        let s = Store::open(d.path().join("fixture.db")).unwrap();
        s.write(|db|{
            db.execute_batch("
                INSERT INTO provider_profiles(id,display_name,kind,models_json,created_at)
                    VALUES('cli','CLI','codex_cli','[\"test\"]','now'),('api','API','openai_compat','[\"test\"]','now');
                INSERT INTO sessions(id,name,role,repo_cwd,profile_id,provider_model,rolling_summary,created_at)
                    VALUES('s','Hub','hub','project','cli','test','project summary','now');
            ")?;
            Ok(())
        }).unwrap();
        let fake = Arc::new(Fake {
            store: s.clone(),
            calls: Mutex::new(vec![]),
            fail_resume: AtomicBool::new(false),
        });
        (d, s, fake)
    }
    fn switch(s: &Store, profile: &'static str) {
        s.write(move |db| {
            db.execute("UPDATE sessions SET profile_id=?1 WHERE id='s'", [profile])?;
            Ok(())
        })
        .unwrap();
    }
    fn claim(s: &Store) -> HubTurn {
        s.hub_claim("s", "fixture-host", "boot").unwrap().unwrap()
    }
    fn execute(p: &CliHubProvider, t: HubTurn) {
        let (tx, _) = broadcast::channel(8);
        p.reply(t, "system", &[], tx).unwrap();
    }

    #[test]
    fn queued_input_below_reply_id_is_not_skipped_and_thread_is_reused() {
        let (_d, s, f) = fixture();
        let p = CliHubProvider::new(s.clone(), f.clone());
        let first = s.hub_enqueue("s", "one", "first").unwrap();
        let one = claim(&s);
        let second = s.hub_enqueue("s", "two", "second").unwrap();
        assert!(second.queued);
        assert!(matches!(
            s.hub_claim("s", "fixture-host", "boot"),
            Err(polyclave_store::Error::BusyError)
        ));
        assert_eq!(s.hub_enqueue("s", "one", "first").unwrap().id, first.id);
        assert!(matches!(
            s.hub_enqueue("s", "one", "different"),
            Err(polyclave_store::Error::ConflictError)
        ));
        execute(&p, one);
        let two = claim(&s);
        assert_eq!(two.input.id, second.id);
        execute(&p, two);
        let calls = f.calls.lock().unwrap();
        assert_eq!(calls[1].native_thread.as_deref(), Some("native-1"));
        assert_eq!(
            calls[1]
                .history
                .iter()
                .map(|c| c.content.as_str())
                .collect::<Vec<_>>(),
            ["second"]
        );
        let saved = s.hub_messages("s").unwrap();
        assert_eq!(
            saved
                .iter()
                .map(|m| (m.source.as_str(), m.text.as_str()))
                .collect::<Vec<_>>(),
            [
                ("user", "first"),
                ("user", "second"),
                ("agent", "reply-1"),
                ("agent", "reply-2")
            ]
        );
        assert!(saved.iter().all(|m| !m.queued));
    }

    #[test]
    fn switch_provider_replays_only_the_gap_and_missing_thread_rebuilds_without_new_input() {
        let (_d, s, f) = fixture();
        let p = CliHubProvider::new(s.clone(), f.clone());
        s.hub_enqueue("s", "one", "original").unwrap();
        execute(&p, claim(&s));
        switch(&s, "api");
        s.hub_enqueue("s", "two", "api prompt").unwrap();
        let a = claim(&s);
        s.hub_authorize(a.id, None).unwrap();
        s.hub_finish(
            a,
            HubCompletion {
                text: "api answer".into(),
                ..Default::default()
            },
        )
        .unwrap();
        switch(&s, "cli");
        s.hub_enqueue("s", "three", "switch-back").unwrap();
        execute(&p, claim(&s));
        let calls = f.calls.lock().unwrap();
        assert_eq!(calls[1].native_thread.as_deref(), Some("native-1"));
        assert_eq!(
            calls[1]
                .history
                .iter()
                .map(|c| c.content.as_str())
                .collect::<Vec<_>>(),
            ["api prompt", "api answer", "switch-back"]
        );
        drop(calls);
        s.hub_enqueue("s", "four", "rebuild").unwrap();
        f.fail_resume.store(true, Ordering::Release);
        execute(&p, claim(&s));
        let calls = f.calls.lock().unwrap();
        assert_eq!(calls[2].native_thread.as_deref(), Some("native-1"));
        assert!(calls[3].native_thread.is_none());
        assert!(calls[3]
            .history
            .iter()
            .any(|m| m.content.contains("project summary")));
        assert!(calls[3].history.iter().any(|m| m.content == "original"));
        assert_eq!(
            s.hub_messages("s")
                .unwrap()
                .iter()
                .filter(|m| m.text == "rebuild")
                .count(),
            1
        );
    }

    #[test]
    fn before_launch_requeues_but_after_release_appends_only_one_interruption_note() {
        let (_d, s, f) = fixture();
        let p = CliHubProvider::new(s.clone(), f.clone());
        let m = s.hub_enqueue("s", "one", "first").unwrap();
        let turn = claim(&s);
        s.hub_end_without_reply(turn.id, "never_started").unwrap();
        let turn = claim(&s);
        assert_eq!(turn.input.id, m.id);
        s.hub_authorize(turn.id, None).unwrap();
        s.hub_record_handle("s", "cli", turn.id, "native-on-crash")
            .unwrap();
        s.hub_end_without_reply(turn.id, "host_lost").unwrap();
        s.hub_recover_inputs().unwrap();
        s.hub_recover_inputs().unwrap();
        s.hub_enqueue("s", "two", "second").unwrap();
        execute(&p, claim(&s));
        s.hub_enqueue("s", "three", "third").unwrap();
        execute(&p, claim(&s));
        let c = f.calls.lock().unwrap();
        assert_eq!(c[0].native_thread.as_deref(), Some("native-on-crash"));
        assert!(c[0]
            .history
            .iter()
            .any(|m| m.content.contains("interrupted")));
        assert!(!c[0].history.iter().any(|m| m.content == "first"));
        assert!(!c[1]
            .history
            .iter()
            .any(|m| m.content.contains("interrupted")));
    }
}
