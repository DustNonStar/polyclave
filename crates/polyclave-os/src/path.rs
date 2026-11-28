use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

pub fn canonical_real_path(path: &Path) -> io::Result<PathBuf> {
    dunce::canonicalize(path)
}

/// Returns `PATH` augmented with standard macOS/user CLI bin locations and
/// `POLYCLAVE_EXTRA_PATH` so Finder/Tauri launches still find installed CLIs.
pub fn augmented_path_env() -> OsString {
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    let mut push_unique = |candidate: PathBuf| {
        if !candidate.as_os_str().is_empty() && !dirs.iter().any(|existing| existing == &candidate)
        {
            dirs.push(candidate);
        }
    };
    if let Some(extra) = std::env::var_os("POLYCLAVE_EXTRA_PATH") {
        for p in std::env::split_paths(&extra) {
            push_unique(p);
        }
    }
    #[cfg(unix)]
    {
        for sys in ["/opt/homebrew/bin", "/usr/local/bin"] {
            push_unique(PathBuf::from(sys));
        }
        if let Some(home) = std::env::var_os("HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
        {
            push_unique(home.join(".local").join("bin"));
            push_unique(home.join(".cargo").join("bin"));
        }
    }
    std::env::join_paths(dirs).unwrap_or_else(|_| std::env::var_os("PATH").unwrap_or_default())
}

pub fn resolve_cli_binary(path: &Path) -> io::Result<PathBuf> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    which::which_in(path, Some(augmented_path_env()), cwd)
        .map_err(|_| io::Error::new(io::ErrorKind::NotFound, "CLI executable not found"))
        .and_then(|path| canonical_real_path(&path))
}

pub fn prepare_isolated_link(source: &Path, destination: &Path) -> io::Result<()> {
    super::platform::prepare_isolated_link(source, destination)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn augmented_path_env_includes_standard_fallback_locations() {
        let joined = augmented_path_env();
        let entries: Vec<PathBuf> = std::env::split_paths(&joined).collect();
        #[cfg(unix)]
        {
            assert!(entries.contains(&PathBuf::from("/opt/homebrew/bin")));
            assert!(entries.contains(&PathBuf::from("/usr/local/bin")));
            if let Some(home) = std::env::var_os("HOME")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
            {
                assert!(entries.contains(&home.join(".local").join("bin")));
                assert!(entries.contains(&home.join(".cargo").join("bin")));
            }
        }
        #[cfg(not(unix))]
        let _ = entries;
    }
}
