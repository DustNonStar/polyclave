use crate::{
    base_environment, claude_secure_storage_dir, prepare_isolated_home, push_option, roots,
    tmp_config_path, ClaudeIsolation, Error, WorkerCommand, WorkerContext,
};
use polyclave_os::{resolve_cli_binary, write_private_file};
use polyclave_proto::{AgentEvent, Vendor};
use serde_json::{json, Value};
#[cfg(target_os = "macos")]
use std::path::PathBuf;
use std::{ffi::OsString, path::Path, sync::Arc};
use uuid::Uuid;

#[cfg(target_os = "macos")]
pub struct MacosSeatbeltIsolation {
    sandbox_exec: PathBuf,
    isolated_claude_home: PathBuf,
}

#[cfg(target_os = "macos")]
impl MacosSeatbeltIsolation {
    pub fn detect(state_dir: &Path) -> Option<Self> {
        let sandbox_exec = PathBuf::from("/usr/bin/sandbox-exec");
        if !sandbox_exec.is_file() {
            return None;
        }
        let canonical_state = polyclave_os::canonical_real_path(state_dir)
            .unwrap_or_else(|_| state_dir.to_path_buf());
        Some(Self {
            sandbox_exec,
            isolated_claude_home: canonical_state.join("isolated-homes").join("claude"),
        })
    }

    fn escape_sbpl_path(path: &Path) -> Result<String, Error> {
        let s = path.to_str().ok_or(Error::Invalid)?;
        if s.is_empty() || s.contains(['"', '\\', '\0', '\n', '\r']) {
            return Err(Error::Invalid);
        }
        Ok(s.to_owned())
    }

    pub fn build_profile(&self, roots: &[PathBuf]) -> Result<String, Error> {
        if roots.is_empty() {
            return Err(Error::Invalid);
        }
        let claude_home = Self::escape_sbpl_path(&self.isolated_claude_home)?;
        let mut root_rules = String::new();
        for root in roots {
            let escaped = Self::escape_sbpl_path(root)?;
            root_rules.push_str(&format!(" (subpath \"{escaped}\")"));
        }
        Ok(format!(
            "(version 1)\n\
             (deny default)\n\
             (allow process*)\n\
             (allow sysctl-read)\n\
             (allow mach-lookup)\n\
             (allow ipc-posix*)\n\
             (allow signal (target self))\n\
             (allow file-read*)\n\
             (allow file-write* (literal \"/dev/null\") (literal \"/dev/tty\") (subpath \"/private/tmp\") (subpath \"/tmp\") (subpath \"{claude_home}\"){root_rules})\n\
             (allow network-outbound (remote ip \"localhost:*\") (remote tcp \"*:443\") (remote udp \"*:53\"))\n\
             (allow network-inbound (local ip \"localhost:*\"))"
        ))
    }
}

#[cfg(target_os = "macos")]
impl ClaudeIsolation for MacosSeatbeltIsolation {
    fn verified_command(
        &self,
        worker: &Path,
        args: &[OsString],
        roots: &[PathBuf],
    ) -> Result<(PathBuf, Vec<OsString>), Error> {
        let profile = self.build_profile(roots)?;
        let mut out = Vec::with_capacity(args.len() + 3);
        out.push(OsString::from("-p"));
        out.push(OsString::from(profile));
        out.push(worker.as_os_str().to_owned());
        out.extend(args.iter().cloned());
        Ok((self.sandbox_exec.clone(), out))
    }
}

pub fn default_claude_isolation(state_dir: &Path) -> Option<Arc<dyn ClaudeIsolation>> {
    #[cfg(target_os = "macos")]
    {
        MacosSeatbeltIsolation::detect(state_dir).map(|s| Arc::new(s) as Arc<dyn ClaudeIsolation>)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = state_dir;
        None
    }
}

pub fn claude_args(config: &Path, id: Uuid, resume: bool, model: Option<&str>) -> Vec<OsString> {
    let mut args = [
        "-p",
        "--verbose",
        "--output-format",
        "stream-json",
        "--include-partial-messages",
        "--permission-mode",
        "bypassPermissions",
        "--strict-mcp-config",
        "--setting-sources",
        "",
    ]
    .map(OsString::from)
    .to_vec();
    push_option(&mut args, "--mcp-config", config);
    push_option(
        &mut args,
        if resume { "--resume" } else { "--session-id" },
        id.to_string(),
    );
    if let Some(model) = model {
        push_option(&mut args, "--model", model);
    }
    args
}

pub fn claude_worker(
    ctx: &WorkerContext<'_>,
    new_id: Uuid,
    isolation: Option<&dyn ClaudeIsolation>,
) -> Result<WorkerCommand, Error> {
    let isolation = isolation.ok_or(Error::ClaudeIsolationUnavailable)?;
    let paths = roots(ctx)?;
    let config = tmp_config_path(ctx.state_dir, ctx.turn_id);
    let id = ctx
        .thread
        .map(Uuid::parse_str)
        .transpose()
        .map_err(|_| Error::Invalid)?
        .unwrap_or(new_id);
    let args = claude_args(&config, id, ctx.thread.is_some(), ctx.model);
    let (executable, args) =
        isolation.verified_command(&resolve_cli_binary(ctx.executable)?, &args, &paths)?;
    let mut env = base_environment();
    env.push((
        "CLAUDE_CONFIG_DIR".into(),
        prepare_isolated_home(ctx.state_dir, Vendor::Claude)?.into_os_string(),
    ));
    env.push((
        "CLAUDE_SECURESTORAGE_CONFIG_DIR".into(),
        claude_secure_storage_dir(),
    ));
    let payload = json!({"mcpServers":{"host":{"type":"http","url":ctx.mcp_url,
        "headers":{"Authorization":format!("Bearer {}",ctx.token)}}}});
    write_private_file(
        &config,
        &serde_json::to_vec(&payload).map_err(|_| Error::Invalid)?,
    )?;
    Ok(WorkerCommand {
        executable,
        args,
        env,
        cwd: paths[0].clone(),
    })
}

pub fn parse_claude(line: &str) -> Option<AgentEvent> {
    let v: Value = serde_json::from_str(line).ok()?;
    match v["type"].as_str()? {
        "system" if v["subtype"] == "init" => Some(AgentEvent::Session {
            id: v["session_id"].as_str()?.into(),
        }),
        "stream_event" if v["event"]["type"] == "content_block_delta" => {
            let d = &v["event"]["delta"];
            match d["type"].as_str()? {
                "text_delta" => Some(AgentEvent::TextDelta {
                    text: d["text"].as_str()?.into(),
                }),
                "thinking_delta" => Some(AgentEvent::ReasoningDelta {
                    text: d["thinking"].as_str()?.into(),
                }),
                _ => None,
            }
        }
        "assistant" => {
            let content = v["message"]["content"].as_array()?;
            let mut text = String::new();
            for block in content {
                if block["type"] == "text" {
                    if let Some(part) = block["text"].as_str() {
                        text.push_str(part);
                    }
                }
            }
            (!text.is_empty()).then_some(AgentEvent::TextDelta { text })
        }
        "result" => {
            let is_error = v["is_error"].as_bool().unwrap_or_else(|| {
                v["subtype"]
                    .as_str()
                    .is_some_and(|s| s.starts_with("error"))
            });
            Some(AgentEvent::Finished { success: !is_error })
        }
        _ => None,
    }
}
