//! Vendor protocol adapters. Git verification is supplied by the host, not a vendor.
mod claude;
mod codex;
pub mod mcp;

pub use claude::{claude_args, claude_worker, parse_claude};
pub use codex::{codex_args, codex_worker, parse_codex};

use polyclave_os::{canonical_real_path, create_private_dir};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid worker configuration")]
    Invalid,
    #[error("Claude worker not started: no verified external filesystem and network sandbox is available")]
    ClaudeIsolationUnavailable,
    #[error("worker setup failed")]
    Io(#[from] std::io::Error),
}

/// Added backend implementations must return the actually confined launch command,
/// not merely assert that some sandbox is available.
pub trait ClaudeIsolation: Send + Sync {
    fn verified_command(
        &self,
        worker: &Path,
        args: &[OsString],
        roots: &[PathBuf],
    ) -> Result<(PathBuf, Vec<OsString>), Error>;
}

pub struct WorkerCommand {
    pub executable: PathBuf,
    pub args: Vec<OsString>,
    pub env: Vec<(OsString, OsString)>,
    pub cwd: PathBuf,
}

pub struct WorkerContext<'a> {
    pub executable: &'a Path,
    pub state_dir: &'a Path,
    pub worktree: &'a Path,
    pub staging: &'a Path,
    pub primary_repo: &'a Path,
    pub final_mirror: &'a Path,
    pub mcp_url: &'a str,
    pub turn_id: i64,
    pub token: &'a str,
    pub model: Option<&'a str>,
    pub thread: Option<&'a str>,
}

pub fn tmp_config_path(state_dir: &Path, turn_id: i64) -> PathBuf {
    state_dir
        .join("tmp")
        .join(format!("mcp-turn-{turn_id}.json"))
}

fn home(state_dir: &Path, vendor: &str) -> Result<PathBuf, Error> {
    let parent = state_dir.join("isolated-homes");
    for p in [&parent, &parent.join(vendor)] {
        match create_private_dir(p) {
            Ok(()) => {}
            Err(e)
                if e.kind() == std::io::ErrorKind::AlreadyExists
                    && std::fs::symlink_metadata(p)?.file_type().is_dir() => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(canonical_real_path(&parent.join(vendor))?)
}

fn roots(ctx: &WorkerContext<'_>) -> Result<[PathBuf; 2], Error> {
    let roots = [
        canonical_real_path(ctx.worktree)?,
        canonical_real_path(ctx.staging)?,
    ];
    let protected = [
        canonical_real_path(ctx.primary_repo)?,
        canonical_real_path(ctx.final_mirror)?,
    ];
    if roots.iter().any(|w| {
        protected
            .iter()
            .any(|p| w.starts_with(p) || p.starts_with(w))
    }) || roots[0].starts_with(&roots[1])
        || roots[1].starts_with(&roots[0])
    {
        return Err(Error::Invalid);
    }
    Ok(roots)
}

fn environment() -> Vec<(OsString, OsString)> {
    const KEYS: &[&str] = &[
        "PATH",
        "HOME",
        "USERPROFILE",
        "SYSTEMROOT",
        "WINDIR",
        "COMSPEC",
        "PATHEXT",
        "PROGRAMFILES",
        "PROGRAMFILES(X86)",
        "LOCALAPPDATA",
        "APPDATA",
        "TEMP",
        "TMP",
        "TMPDIR",
        "LANG",
        "LC_ALL",
        "TERM",
    ];
    KEYS.iter()
        .filter_map(|k| std::env::var_os(k).map(|v| (OsString::from(k), v)))
        .chain([(OsString::from("GIT_TERMINAL_PROMPT"), OsString::from("0"))])
        .collect()
}

fn push_option(args: &mut Vec<OsString>, flag: &str, value: impl AsRef<OsStr>) {
    args.push(flag.into());
    args.push(value.as_ref().to_owned());
}
