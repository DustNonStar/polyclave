//! Vendor protocol adapters. Git verification is supplied by the host, not a vendor.
mod agy;
mod claude;
mod codex;
mod dsh;
pub mod mcp;

pub use agy::{agy_args, agy_worker, parse_agy};
#[cfg(target_os = "macos")]
pub use claude::MacosSeatbeltIsolation;
pub use claude::{claude_args, claude_worker, default_claude_isolation, parse_claude};
pub use codex::{codex_args, codex_worker, parse_codex};
pub use dsh::{dsh_args, dsh_worker, parse_dsh};

use polyclave_os::{canonical_real_path, create_private_dir, prepare_isolated_link};
use polyclave_proto::Vendor;
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

fn user_home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

fn default_vendor_source(vendor: Vendor) -> Option<PathBuf> {
    match vendor {
        Vendor::Codex => std::env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .or_else(|| user_home_dir().map(|h| h.join(".codex"))),
        Vendor::Claude => std::env::var_os("CLAUDE_CONFIG_DIR")
            .map(PathBuf::from)
            .or_else(|| user_home_dir().map(|h| h.join(".claude"))),
        Vendor::Agy => std::env::var_os("AGY_HOME")
            .or_else(|| std::env::var_os("GEMINI_HOME"))
            .map(PathBuf::from)
            .or_else(|| user_home_dir().map(|h| h.join(".gemini"))),
        Vendor::Dsh => std::env::var_os("DSH_HOME")
            .map(PathBuf::from)
            .or_else(|| user_home_dir().map(|h| h.join(".dsh"))),
    }
}

/// Returns the `CLAUDE_SECURESTORAGE_CONFIG_DIR` value matching the user's active
/// Claude login so an isolated `CLAUDE_CONFIG_DIR` still reads existing credentials.
pub fn claude_secure_storage_dir() -> OsString {
    std::env::var_os("CLAUDE_CONFIG_DIR").unwrap_or_default()
}

/// Prepares `<state_dir>/isolated-homes/<vendor>` and links only credentials and `skills`.
pub fn prepare_isolated_home(state_dir: &Path, vendor: Vendor) -> Result<PathBuf, Error> {
    prepare_isolated_home_from(state_dir, vendor, default_vendor_source(vendor).as_deref())
}

/// Variant of `prepare_isolated_home` with an explicit source directory for deterministic tests.
pub fn prepare_isolated_home_from(
    state_dir: &Path,
    vendor: Vendor,
    source_dir: Option<&Path>,
) -> Result<PathBuf, Error> {
    let vendor_name = match vendor {
        Vendor::Codex => "codex",
        Vendor::Claude => "claude",
        Vendor::Agy => "agy",
        Vendor::Dsh => "dsh",
    };
    let entries: &[&str] = match vendor {
        Vendor::Codex => &["auth.json", "skills"],
        Vendor::Claude => &[".credentials.json", "skills"],
        Vendor::Agy => &["credentials.json", "oauth_creds.json", "skills"],
        Vendor::Dsh => &["auth.json", "credentials.json", "skills"],
    };
    let parent = state_dir.join("isolated-homes");
    let isolated = parent.join(vendor_name);
    for p in [&parent, &isolated] {
        match create_private_dir(p) {
            Ok(()) => {}
            Err(e)
                if e.kind() == std::io::ErrorKind::AlreadyExists
                    && std::fs::symlink_metadata(p)?.file_type().is_dir() => {}
            Err(e) => return Err(e.into()),
        }
    }
    let isolated = canonical_real_path(&isolated)?;
    if let Some(src_root) = source_dir {
        if canonical_real_path(src_root).ok().as_deref() == Some(isolated.as_path()) {
            return Ok(isolated);
        }
        for name in entries {
            sync_isolated_entry(&src_root.join(name), &isolated.join(name))?;
        }
    }
    Ok(isolated)
}

fn sync_isolated_entry(src: &Path, dst: &Path) -> Result<(), Error> {
    let existing = std::fs::symlink_metadata(dst).ok();
    if src.exists() {
        let target = canonical_real_path(src)?;
        if let Some(meta) = existing {
            if meta.file_type().is_symlink() && std::fs::read_link(dst).is_ok_and(|l| l == target) {
                return Ok(());
            }
            remove_isolated_entry(dst, &meta)?;
        }
        prepare_isolated_link(&target, dst)?;
    } else if let Some(meta) = existing {
        if meta.file_type().is_symlink() {
            remove_isolated_entry(dst, &meta)?;
        }
    }
    Ok(())
}

fn remove_isolated_entry(path: &Path, meta: &std::fs::Metadata) -> Result<(), Error> {
    if meta.file_type().is_symlink() {
        #[cfg(windows)]
        if std::fs::remove_file(path).is_err() {
            std::fs::remove_dir(path)?;
        }
        #[cfg(not(windows))]
        std::fs::remove_file(path)?;
    } else if meta.file_type().is_dir() {
        std::fs::remove_dir_all(path)?;
    } else {
        std::fs::remove_file(path)?;
    }
    Ok(())
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

/// Returns the minimal environment allowlist shared by Worker and Hub CLI subprocesses.
pub fn base_environment() -> Vec<(OsString, OsString)> {
    const KEYS: &[&str] = &[
        "PATH",
        "HOME",
        "USER",
        "LOGNAME",
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
