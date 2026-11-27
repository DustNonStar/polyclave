use crate::{Error, Result};
use polyclave_core::runs::RunManager;
use serde::Serialize;
use std::{
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
};

const MAX_FILE: usize = 256 * 1024;
const MAX_ENTRIES: usize = 10_000;

pub struct RepoScope(PathBuf);
impl RepoScope {
    /// Only call with the repository from the durable session, never a tool argument.
    pub fn from_registered(root: &Path) -> Result<Self> {
        let p = polyclave_os::canonical_real_path(root).map_err(|_| Error::Policy)?;
        if !p.is_dir() {
            return Err(Error::Policy);
        }
        Ok(Self(p))
    }
    pub fn root(&self) -> &Path {
        &self.0
    }
    pub fn matches_requested(&self, requested: &Path) -> Result<()> {
        let canonical = polyclave_os::canonical_real_path(requested).map_err(|_| Error::Policy)?;
        if canonical != self.0 {
            return Err(Error::Policy);
        }
        Ok(())
    }
    fn resolve(&self, rel: &Path) -> Result<PathBuf> {
        let mut target = self.0.clone();
        for component in rel.components() {
            let Component::Normal(part) = component else {
                return Err(Error::Policy);
            };
            let text = part.to_str().ok_or(Error::Policy)?;
            if !allow_component(text) {
                return Err(Error::Policy);
            }
            target.push(part);
            if fs::symlink_metadata(&target)
                .map_err(|_| Error::Policy)?
                .file_type()
                .is_symlink()
            {
                return Err(Error::Policy);
            }
        }
        let real = polyclave_os::canonical_real_path(&target).map_err(|_| Error::Policy)?;
        if !real.starts_with(&self.0) {
            return Err(Error::Policy);
        }
        Ok(real)
    }
    fn read_text(&self, rel: &Path) -> Result<String> {
        let path = self.resolve(rel)?;
        let metadata = fs::metadata(&path).map_err(|_| Error::Policy)?;
        if !metadata.is_file() || metadata.len() > MAX_FILE as u64 {
            return Err(Error::Policy);
        }
        let mut bytes = vec![];
        fs::File::open(path)
            .map_err(|_| Error::Policy)?
            .take((MAX_FILE + 1) as u64)
            .read_to_end(&mut bytes)
            .map_err(|_| Error::Policy)?;
        if bytes.len() > MAX_FILE
            || bytes
                .iter()
                .any(|b| *b < 32 && !matches!(b, b'\n' | b'\r' | b'\t'))
        {
            return Err(Error::Policy);
        }
        let text = String::from_utf8(bytes).map_err(|_| Error::Policy)?;
        if text.contains("PRIVATE KEY-----") {
            return Err(Error::Policy);
        }
        Ok(text)
    }
    pub fn list_repo_files(
        &self,
        dir: Option<&Path>,
        pattern: Option<&str>,
    ) -> Result<Vec<String>> {
        let dir = dir.unwrap_or(Path::new(""));
        let start = self.resolve(dir)?;
        if !start.is_dir() {
            return Err(Error::Policy);
        }
        let glob = pattern
            .map(globset::Glob::new)
            .transpose()
            .map_err(|_| Error::Policy)?
            .map(|g| g.compile_matcher());
        let mut pending = vec![start];
        let mut scanned = 0;
        let mut matches = vec![];
        while let Some(directory) = pending.pop() {
            let mut files = fs::read_dir(directory)
                .map_err(|_| Error::Policy)?
                .collect::<std::io::Result<Vec<_>>>()
                .map_err(|_| Error::Policy)?;
            files.sort_by_key(fs::DirEntry::file_name);
            for entry in files {
                scanned += 1;
                if scanned > MAX_ENTRIES {
                    return Err(Error::Limit);
                }
                let name = entry.file_name();
                let Some(name) = name.to_str() else {
                    continue;
                };
                if !allow_component(name) {
                    continue;
                }
                let kind = entry.file_type().map_err(|_| Error::Policy)?;
                let absolute = entry.path();
                let relative = absolute.strip_prefix(&self.0).map_err(|_| Error::Policy)?;
                if kind.is_dir() && relative.components().count() <= 16 {
                    pending.push(absolute);
                } else if kind.is_file()
                    && glob.as_ref().is_none_or(|g| g.is_match(relative))
                    && self.read_text(relative).is_ok()
                {
                    matches.push(relative.to_str().ok_or(Error::Policy)?.replace('\\', "/"));
                }
            }
        }
        matches.sort_unstable();
        matches.truncate(500);
        Ok(matches)
    }
    pub fn read_repo_file(
        &self,
        rel: &Path,
        start: Option<usize>,
        end: Option<usize>,
    ) -> Result<String> {
        let start = start.unwrap_or(1);
        let end = end.unwrap_or_else(|| start.saturating_add(199));
        if start == 0 || end < start || end - start >= 200 {
            return Err(Error::Policy);
        }
        let text = self.read_text(rel)?;
        let result = text
            .lines()
            .skip(start - 1)
            .take(end - start + 1)
            .collect::<Vec<_>>()
            .join("\n");
        if result.len() > 64 * 1024 {
            return Err(Error::Limit);
        }
        Ok(result)
    }
    pub fn search_repo(&self, pattern: &str, glob: Option<&str>) -> Result<Vec<RepoMatch>> {
        if pattern.is_empty() || pattern.len() > 256 {
            return Err(Error::Policy);
        }
        let mut result = vec![];
        for rel in self.list_repo_files(None, glob)? {
            let text = self.read_text(Path::new(&rel))?;
            for (n, line) in text.lines().enumerate() {
                if line.contains(pattern) {
                    result.push(RepoMatch {
                        path: rel.clone(),
                        line: n + 1,
                        text: line[..line.floor_char_boundary(line.len().min(400))].to_owned(),
                    });
                    if result.len() == 100 {
                        return Ok(result);
                    }
                }
            }
        }
        Ok(result)
    }
    pub fn get_run_diff(&self, runs: &RunManager, id: &str) -> Result<(Vec<String>, String)> {
        let result = runs
            .read_review_diff(id, self.root(), |p| p.split('/').all(allow_component))
            .map_err(|_| Error::Policy)?;
        if result.1.contains("PRIVATE KEY-----") {
            return Err(Error::Policy);
        }
        Ok(result)
    }
}

fn allow_component(text: &str) -> bool {
    let s = text.to_ascii_lowercase();
    !(s.is_empty()
        || s == "."
        || s == ".."
        || s.contains([':', '\\'])
        || s.ends_with([' ', '.'])
        || s.starts_with(".env")
        || [".pem", ".key", ".p12", ".pfx"]
            .iter()
            .any(|end| s.ends_with(end))
        || s.starts_with("id_rsa")
        || s.starts_with("id_ed25519")
        || matches!(
            s.as_str(),
            ".git"
                | ".ssh"
                | ".aws"
                | ".gnupg"
                | ".netrc"
                | ".npmrc"
                | "credentials"
                | "auth.json"
                | "local-auth.token"
                | "node_modules"
                | "target"
                | "con"
                | "nul"
                | "prn"
                | "aux"
        ))
}
#[derive(Debug, Serialize)]
pub struct RepoMatch {
    pub path: String,
    pub line: usize,
    pub text: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_outside_symlink_secrets_and_binary_while_searching_in_bounds() {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("repo");
        fs::create_dir(&repo).unwrap();
        fs::write(repo.join("ok.txt"), "one\nneedle\nthree").unwrap();
        fs::write(repo.join(".env.local"), "not surfaced").unwrap();
        fs::write(repo.join("data.bin"), [0, 77, 88]).unwrap();
        fs::write(
            repo.join("seemingly-text.txt"),
            concat!("-----BEGIN ", "PRIVATE KEY-----"),
        )
        .unwrap();
        polyclave_os::prepare_isolated_link(root.path(), &repo.join("outside")).unwrap();
        let r = RepoScope::from_registered(&repo).unwrap();
        assert!(r.matches_requested(root.path()).is_err());
        assert_eq!(
            r.read_repo_file(Path::new("ok.txt"), Some(2), Some(2))
                .unwrap(),
            "needle"
        );
        for path in [
            ".env.local",
            "data.bin",
            "seemingly-text.txt",
            "outside/repo/ok.txt",
            "../repo/ok.txt",
        ] {
            assert!(
                r.read_repo_file(Path::new(path), None, None).is_err(),
                "{path}"
            );
        }
        assert_eq!(r.list_repo_files(None, None).unwrap(), ["ok.txt"]);
        let result = r.search_repo("needle", Some("*.txt")).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].line, 2);
    }
}
