use crate::{
    base_environment, claude_secure_storage_dir, prepare_isolated_home, push_option, roots,
    tmp_config_path, ClaudeIsolation, Error, WorkerCommand, WorkerContext,
};
use polyclave_os::{create_private_dir, resolve_cli_binary, write_private_file};
use polyclave_proto::{AgentEvent, Vendor};
use serde_json::{json, Value};
#[cfg(target_os = "macos")]
use std::path::PathBuf;
use std::{ffi::OsString, path::Path, sync::Arc};
use uuid::Uuid;

#[cfg(target_os = "macos")]
pub struct MacosSeatbeltIsolation {
    sandbox_exec: PathBuf,
    state_dir: PathBuf,
    isolated_claude_home: PathBuf,
}

#[cfg(target_os = "macos")]
impl MacosSeatbeltIsolation {
    pub fn detect(state_dir: &Path) -> Option<Self> {
        let sandbox_exec = PathBuf::from("/usr/bin/sandbox-exec");
        if !sandbox_exec.is_file() {
            return None;
        }
        let state_dir = polyclave_os::canonical_real_path(state_dir).ok()?;
        Some(Self {
            sandbox_exec,
            isolated_claude_home: state_dir.join("isolated-homes").join("claude"),
            state_dir,
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
        self.profile(roots, None, None)
    }

    fn profile(
        &self,
        roots: &[PathBuf],
        config: Option<&Path>,
        mcp_port: Option<u16>,
    ) -> Result<String, Error> {
        if roots.is_empty() {
            return Err(Error::Invalid);
        }
        let state = Self::escape_sbpl_path(&self.state_dir)?;
        let claude_home = Self::escape_sbpl_path(&self.isolated_claude_home)?;
        let mut write_roots = format!("(subpath \"{claude_home}\")");
        for root in roots {
            let escaped = Self::escape_sbpl_path(root)?;
            write_roots.push_str(&format!(" (subpath \"{escaped}\")"));
        }
        let mut state_read_exceptions = write_roots.clone();
        if let Some(config) = config {
            let config = Self::escape_sbpl_path(config)?;
            state_read_exceptions.push_str(&format!(" (literal \"{config}\")"));
        }
        let mut keychains = String::from(
            "(subpath \"/Library/Keychains\") (subpath \"/System/Library/Keychains\")",
        );
        if let Some(home) = crate::user_home_dir() {
            let home = polyclave_os::canonical_real_path(&home)?;
            let path = Self::escape_sbpl_path(&home.join("Library/Keychains"))?;
            keychains.push_str(&format!(" (subpath \"{path}\")"));
        }
        let mcp = mcp_port.map_or_else(String::new, |port| {
            format!(" (remote tcp \"localhost:{port}\")")
        });
        Ok(format!(
            "(version 1)\n\
             (deny default)\n\
             (allow process*)\n\
             (allow sysctl-read)\n\
             (allow mach-lookup)\n\
             (allow ipc-posix*)\n\
             (allow signal (target self))\n\
             (allow file-read*)\n\
             (deny file-read* (require-all (subpath \"{state}\")\n\
                  (require-not (require-any {state_read_exceptions}))))\n\
             (deny file-read* {keychains})\n\
             (allow file-write* (literal \"/dev/null\") (literal \"/dev/tty\") {write_roots})\n\
             (allow network-outbound{mcp}\n\
                  (require-all (remote tcp \"*:443\") (require-not (remote ip \"localhost:*\")))\n\
                  (require-all (remote udp \"*:53\") (require-not (remote ip \"localhost:*\"))))"
        ))
    }

    fn wrap(&self, worker: &Path, args: &[OsString], profile: String) -> (PathBuf, Vec<OsString>) {
        let mut out = Vec::with_capacity(args.len() + 3);
        out.push(OsString::from("-p"));
        out.push(OsString::from(profile));
        out.push(worker.as_os_str().to_owned());
        out.extend(args.iter().cloned());
        (self.sandbox_exec.clone(), out)
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
        Ok(self.wrap(worker, args, self.build_profile(roots)?))
    }

    fn verified_worker_command(
        &self,
        worker: &Path,
        args: &[OsString],
        roots: &[PathBuf],
        config: &Path,
        mcp_url: &str,
    ) -> Result<(PathBuf, Vec<OsString>), Error> {
        let port = mcp_url
            .strip_prefix("http://127.0.0.1:")
            .and_then(|url| url.strip_suffix("/mcp"))
            .and_then(|port| port.parse::<u16>().ok())
            .filter(|port| *port != 0)
            .ok_or(Error::Invalid)?;
        let [worktree, staging] = roots else {
            return Err(Error::Invalid);
        };
        let write_roots = [worktree.clone(), staging.join("polyclave-tmp")];
        let mut profile = self.profile(&write_roots, Some(config), Some(port))?;
        let git = Self::escape_sbpl_path(&worktree.join(".git"))?;
        profile.push_str(&format!("\n(deny file-write* (subpath \"{git}\"))"));
        Ok(self.wrap(worker, args, profile))
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
    let (executable, args) = isolation.verified_worker_command(
        &resolve_cli_binary(ctx.executable)?,
        &args,
        &paths,
        &config,
        ctx.mcp_url,
    )?;
    let temp = paths[1].join("polyclave-tmp");
    match create_private_dir(&temp) {
        Ok(()) => {}
        Err(error)
            if error.kind() == std::io::ErrorKind::AlreadyExists
                && temp.symlink_metadata()?.file_type().is_dir() => {}
        Err(error) => return Err(error.into()),
    }
    let mut env = base_environment();
    env.retain(|(key, _)| key != "TMPDIR" && key != "TMP" && key != "TEMP");
    for key in ["TMPDIR", "TMP", "TEMP"] {
        env.push((key.into(), temp.as_os_str().to_owned()));
    }
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
