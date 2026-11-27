use std::io;
use std::path::{Path, PathBuf};

pub fn canonical_real_path(path: &Path) -> io::Result<PathBuf> {
    dunce::canonicalize(path)
}

pub fn resolve_cli_binary(path: &Path) -> io::Result<PathBuf> {
    which::which(path)
        .map_err(|_| io::Error::new(io::ErrorKind::NotFound, "CLI executable not found"))
        .and_then(|path| canonical_real_path(&path))
}

pub fn prepare_isolated_link(source: &Path, destination: &Path) -> io::Result<()> {
    super::platform::prepare_isolated_link(source, destination)
}
