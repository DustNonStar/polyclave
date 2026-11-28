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
    home_dir: Option<PathBuf>,
    proxy_urls: Vec<String>,
}

#[cfg(target_os = "macos")]
impl MacosSeatbeltIsolation {
    pub fn detect(state_dir: &Path) -> Option<Self> {
        let sandbox_exec = PathBuf::from("/usr/bin/sandbox-exec");
        if !sandbox_exec.is_file() {
            return None;
        }
        let state_dir = polyclave_os::canonical_real_path(state_dir).ok()?;
        let home_dir =
            crate::user_home_dir().and_then(|home| polyclave_os::canonical_real_path(&home).ok());
        let proxy_urls = [
            "HTTPS_PROXY",
            "https_proxy",
            "HTTP_PROXY",
            "http_proxy",
            "ALL_PROXY",
            "all_proxy",
        ]
        .into_iter()
        .filter_map(|k| std::env::var(k).ok())
        .filter(|v| !v.trim().is_empty())
        .collect();
        Some(Self {
            sandbox_exec,
            isolated_claude_home: state_dir.join("isolated-homes").join("claude"),
            state_dir,
            home_dir,
            proxy_urls,
        })
    }

    pub fn with_home_dir(mut self, home_dir: PathBuf) -> Self {
        self.home_dir = Some(polyclave_os::canonical_real_path(&home_dir).unwrap_or(home_dir));
        self
    }

    pub fn with_proxy_urls(mut self, proxy_urls: Vec<String>) -> Self {
        self.proxy_urls = proxy_urls;
        self
    }

    fn parse_proxy_sbpl_rule(raw: &str) -> Option<String> {
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.contains(['"', '\\', '\0', '\n', '\r', ' ', '\t', '*']) {
            return None;
        }
        let (scheme, rest) = match trimmed.split_once("://") {
            Some((s, r)) => (Some(s.to_ascii_lowercase()), r),
            None => (None, trimmed),
        };
        let authority = rest.rsplit_once('@').map_or(rest, |(_, host)| host);
        let host_port = authority.split(['/', '?', '#']).next()?;
        if host_port.is_empty() {
            return None;
        }
        let default_port = match scheme.as_deref() {
            Some("http") => Some(80u16),
            Some("https") => Some(443u16),
            Some("socks4" | "socks4a" | "socks5" | "socks5h") => Some(1080u16),
            _ => None,
        };
        let (host, port) = if let Some(stripped) = host_port.strip_prefix('[') {
            let (h, after) = stripped.split_once(']')?;
            let p = if let Some(p_str) = after.strip_prefix(':') {
                p_str.parse::<u16>().ok()?
            } else if after.is_empty() {
                default_port?
            } else {
                return None;
            };
            (h, p)
        } else if let Some((h, p_str)) = host_port.rsplit_once(':') {
            if h.contains(':') {
                return None;
            }
            (h, p_str.parse::<u16>().ok()?)
        } else {
            (host_port, default_port?)
        };
        if port == 0 || host.is_empty() {
            return None;
        }
        if host.eq_ignore_ascii_case("localhost") {
            return Some(format!("(remote tcp \"localhost:{port}\")"));
        }
        if let Ok(ip) = host.parse::<std::net::IpAddr>() {
            if ip.is_loopback() {
                return Some(format!("(remote tcp \"localhost:{port}\")"));
            }
            return Some(format!("(remote tcp \"{ip}:{port}\")"));
        }
        if !host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-'))
        {
            return None;
        }
        Some(format!("(remote tcp \"*:{port}\")"))
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
        let mut sensitive_reads = String::from(
            "(subpath \"/Library/Keychains\") (subpath \"/System/Library/Keychains\")",
        );
        if let Some(home) = &self.home_dir {
            for rel in [
                "Library/Keychains",
                ".ssh",
                ".codex",
                ".config/gh",
                ".aws",
                ".gnupg",
                ".kube",
            ] {
                let path = Self::escape_sbpl_path(&home.join(rel))?;
                sensitive_reads.push_str(&format!(" (subpath \"{path}\")"));
            }
            for rel in [".netrc", ".git-credentials"] {
                let path = Self::escape_sbpl_path(&home.join(rel))?;
                sensitive_reads.push_str(&format!(" (literal \"{path}\")"));
            }
        }
        let mcp = mcp_port.map_or_else(String::new, |port| {
            format!(" (remote tcp \"localhost:{port}\")")
        });
        let mut proxy_rules = Vec::new();
        for raw in &self.proxy_urls {
            if let Some(rule) = Self::parse_proxy_sbpl_rule(raw) {
                if !proxy_rules.contains(&rule) {
                    proxy_rules.push(rule);
                }
            }
        }
        let proxies = if proxy_rules.is_empty() {
            String::new()
        } else {
            format!(" {}", proxy_rules.join(" "))
        };
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
             (deny file-read* {sensitive_reads})\n\
             (allow file-write* (literal \"/dev/null\") (literal \"/dev/tty\") {write_roots})\n\
             (allow network-outbound{mcp}{proxies}\n\
                  (remote unix-socket (path-literal \"/private/var/run/mDNSResponder\"))\n\
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
    env.extend(crate::claude_auth_environment());
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
