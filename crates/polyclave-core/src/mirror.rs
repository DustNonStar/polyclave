use crate::{classify::BranchCheck, GitError};
use polyclave_os::{canonical_real_path, create_private_dir, resolve_cli_binary};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::{Arc, Mutex},
};

type Result<T> = std::result::Result<T, GitError>;
type RepoLocks = HashMap<PathBuf, Arc<Mutex<()>>>;
type GitOutput = dyn Fn(Command) -> Result<Output> + Send + Sync;

pub struct MirrorManager {
    pub(crate) root: PathBuf,
    git: PathBuf,
    hooks: PathBuf,
    locks: Arc<Mutex<RepoLocks>>,
    output: Option<Box<GitOutput>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunGit {
    pub repo: PathBuf,
    pub mirror: PathBuf,
    pub id: String,
    pub base: String,
}
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttemptGit {
    pub run: RunGit,
    pub number: u32,
}

impl AttemptGit {
    pub fn branch(&self) -> String {
        format!("agent/{}/a{}", self.run.id, self.number)
    }
    pub fn full_ref(&self) -> String {
        format!("refs/heads/{}", self.branch())
    }
    pub fn name(&self) -> String {
        format!("{}-a{}", self.run.id, self.number)
    }
}
impl RunGit {
    pub fn base_ref(&self) -> String {
        format!("refs/bases/{}", self.id)
    }
}

impl MirrorManager {
    pub fn new(root: &Path) -> Result<Self> {
        let root = canonical_real_path(root)?;
        for name in [
            "mirrors",
            "worktrees",
            "staging",
            "empty-hooks",
            "commit-stage",
        ] {
            private_subdir(&root.join(name))?;
        }
        Ok(Self {
            git: resolve_cli_binary(Path::new("git"))?,
            hooks: root.join("empty-hooks"),
            root,
            locks: Arc::default(),
            output: None,
        })
    }

    pub(crate) fn with_git_output(
        &self,
        output: impl Fn(Command) -> Result<Output> + Send + Sync + 'static,
    ) -> Self {
        Self {
            root: self.root.clone(),
            git: self.git.clone(),
            hooks: self.hooks.clone(),
            locks: self.locks.clone(),
            output: Some(Box::new(output)),
        }
    }

    pub(crate) fn with_repo<T>(
        &self,
        repo: &Path,
        job: impl FnOnce(&Path) -> Result<T>,
    ) -> Result<T> {
        let repo = canonical_real_path(repo)?;
        let lock = self
            .locks
            .lock()
            .map_err(|_| GitError::Invalid("repository locks unavailable"))?
            .entry(repo.clone())
            .or_default()
            .clone();
        let _guard = lock
            .lock()
            .map_err(|_| GitError::Invalid("repository lock unavailable"))?;
        job(&repo)
    }

    pub(crate) fn git<I, S>(&self, dir: &Path, args: I) -> Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let mut cmd = Command::new(&self.git);
        for (key, _) in std::env::vars_os() {
            if key
                .to_string_lossy()
                .to_ascii_uppercase()
                .starts_with("GIT_")
            {
                cmd.env_remove(key);
            }
        }
        cmd.env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "")
            .env("GIT_TERMINAL_PROMPT", "0")
            .env("GIT_ALLOW_PROTOCOL", "file")
            .env("GIT_NO_LAZY_FETCH", "1")
            .env("GIT_NO_REPLACE_OBJECTS", "1")
            .env("LC_ALL", "C")
            .stdin(Stdio::null())
            .arg("-C")
            .arg(dir)
            .arg("-c")
            .arg(format!("core.hooksPath={}", self.hooks.display()))
            .args(args);
        match &self.output {
            Some(output) => output(cmd),
            None => Ok(cmd.output()?),
        }
    }

    pub(crate) fn require<I, S>(&self, dir: &Path, args: I, op: &'static str) -> Result<String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        let out = self.git(dir, args)?;
        if !out.status.success() {
            return Err(GitError::Command(op));
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim_end().to_owned())
    }

    pub(crate) fn mirror_path(&self, repo: &Path) -> PathBuf {
        let key = format!("{:x}", Sha256::digest(repo.as_os_str().as_encoded_bytes()));
        self.root
            .join("mirrors")
            .join(format!("{}.git", &key[..16]))
    }

    /// Reads the repository's `origin` once, for the PR gate's post-R1 publish
    /// step only. Admission, the mirror and R1 never use it, and a missing or
    /// unreadable origin never blocks admission: it yields an empty snapshot,
    /// which the gate later refuses to publish from. Credentials embedded in a
    /// URL are removed before the snapshot leaves this function.
    pub fn origin_snapshot(&self, repo: &Path) -> polyclave_store::OriginSnapshot {
        let read = |key: &str| -> Vec<String> {
            match self.git(repo, ["config", "--local", "--get-all", key]) {
                Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .map(str::trim)
                    .filter(|l| !l.is_empty())
                    .map(strip_url_credentials)
                    .collect(),
                _ => Vec::new(),
            }
        };
        polyclave_store::OriginSnapshot {
            urls: read("remote.origin.url"),
            push_urls: read("remote.origin.pushurl"),
        }
    }

    pub fn admit(&self, repo: &Path, id: &str, base_branch: &str) -> Result<RunGit> {
        validate_id(id)?;
        self.with_repo(repo, |repo| {
            // Ignore origin entirely: this reads a local refs/heads name.
            let branch = format!("refs/heads/{base_branch}");
            self.require(
                repo,
                ["check-ref-format", branch.as_str()],
                "invalid base branch",
            )?;
            let base = self.oid(repo, &format!("{branch}^{{commit}}"))?;
            let mirror = self.mirror_path(repo);
            if !mirror.exists() {
                create_private_dir(&mirror)?;
                self.require(
                    &mirror,
                    ["init", "--bare", "--template="],
                    "init host mirror",
                )?;
            }
            ensure_real(&mirror)?;
            if self.require(
                &mirror,
                ["rev-parse", "--is-bare-repository"],
                "inspect host mirror",
            )? != "true"
            {
                return Err(GitError::Invalid("host mirror must be bare"));
            }
            for key in ["receive.denyDeletes", "receive.denyNonFastForwards"] {
                self.require(
                    &mirror,
                    ["config", "--local", key, "true"],
                    "set mirror policy",
                )?;
            }
            let run = RunGit {
                repo: repo.to_path_buf(),
                mirror,
                id: id.to_owned(),
                base,
            };
            match self.ref_tip(&run.mirror, &run.base_ref())? {
                Some(existing) if existing == run.base => (),
                Some(_) => return Err(GitError::Invalid("an immutable base already exists")),
                None => self.fetch(
                    &run.mirror,
                    repo,
                    &format!("{}:{}", run.base, run.base_ref()),
                )?,
            }
            self.validate_run(&run, repo)?;
            Ok(run)
        })
    }

    pub(crate) fn validate_run(&self, run: &RunGit, repo: &Path) -> Result<()> {
        validate_id(&run.id)?;
        validate_oid(&run.base)?;
        if run.repo != repo || run.mirror != self.mirror_path(repo) {
            return Err(GitError::Invalid("run does not name its host-owned mirror"));
        }
        ensure_real(&run.mirror)?;
        if self.ref_tip(&run.mirror, &run.base_ref())?.as_deref() != Some(&run.base) {
            return Err(GitError::Invalid(
                "mirror baseline differs from the stored SHA",
            ));
        }
        Ok(())
    }

    pub(crate) fn fetch(&self, to: &Path, from: &Path, spec: &str) -> Result<()> {
        let mut args: Vec<OsString> = [
            "fetch",
            "--atomic",
            "--no-tags",
            "--no-prune",
            "--no-recurse-submodules",
            "--no-write-fetch-head",
            "--no-auto-maintenance",
            "--upload-pack=git-upload-pack",
            "--",
        ]
        .into_iter()
        .map(Into::into)
        .collect();
        args.extend([from.as_os_str().to_owned(), spec.into()]);
        self.require(to, args, "fetch local exact ref")?;
        Ok(())
    }

    pub(crate) fn oid(&self, dir: &Path, rev: &str) -> Result<String> {
        let sha = self.require(
            dir,
            ["rev-parse", "--verify", "--end-of-options", rev],
            "read commit",
        )?;
        validate_oid(&sha)?;
        Ok(sha)
    }

    fn ref_tip(&self, repo: &Path, name: &str) -> Result<Option<String>> {
        let out = self.git(repo, ["show-ref", "--verify", "--hash", "--", name])?;
        if out.status.success() {
            let tip = String::from_utf8_lossy(&out.stdout).trim().to_owned();
            validate_oid(&tip)?;
            return Ok(Some(tip));
        }
        // show-ref -q verifies the identity but does not dereference a bad object.
        let exists = self.git(repo, ["for-each-ref", "--format=%(refname)", name])?;
        if exists.status.success()
            && !String::from_utf8_lossy(&exists.stdout)
                .lines()
                .any(|l| l == name)
        {
            return Ok(None);
        }
        Err(GitError::Command("read mirror ref"))
    }

    pub(crate) fn staging_ref(&self, staging: &Path, name: &str) -> Result<Option<String>> {
        // ls-remote uses Git's upload-pack boundary; never execute general
        // repository commands against Worker-writable .git configuration.
        let out = self.git(
            &self.root,
            [
                OsStr::new("ls-remote"),
                OsStr::new("--exit-code"),
                OsStr::new("--refs"),
                OsStr::new("--upload-pack=git-upload-pack"),
                OsStr::new("--"),
                staging.as_os_str(),
                OsStr::new(name),
            ],
        )?;
        match out.status.code() {
            Some(0) => {
                let s = String::from_utf8_lossy(&out.stdout);
                let (oid, reference) = s
                    .trim_end()
                    .split_once('\t')
                    .ok_or(GitError::Invalid("invalid staging ref"))?;
                if reference != name {
                    return Err(GitError::Invalid("unexpected staging ref"));
                }
                validate_oid(oid)?;
                Ok(Some(oid.to_owned()))
            }
            Some(2) => Ok(None),
            _ => Err(GitError::Command("read staging ref")),
        }
    }

    /// Must run for submit, exit and Recovery (even without a receipt).
    pub fn reconcile(&self, a: &AttemptGit) -> BranchCheck {
        let staged = self.with_repo(&a.run.repo, |repo| {
            self.validate_run(&a.run, repo)?;
            validate_attempt(a)?;
            let stage = self.staging_path(a);
            ensure_real(&stage)?;
            let reference = a.full_ref();
            if let Some(staged) = self.staging_ref(&stage, &reference)? {
                let published = self.ref_tip(&a.run.mirror, &reference)?;
                if published.as_deref() != Some(&staged) {
                    let spec = format!("{reference}:{reference}");
                    self.fetch(&a.run.mirror, &stage, &spec)?;
                    if self.ref_tip(&a.run.mirror, &reference)?.as_deref() != Some(&staged) {
                        return Err(GitError::Invalid("staging moved during transfer"));
                    }
                }
            }
            self.check_under_lock(a)
        });
        if let Ok(check) = staged {
            return check;
        }
        self.with_repo(&a.run.repo, |repo| {
            self.validate_run(&a.run, repo)?;
            validate_attempt(a)?;
            if self.ref_tip(&a.run.mirror, &a.full_ref())?.is_some() {
                self.check_under_lock(a)
            } else {
                Ok(BranchCheck::Unreadable)
            }
        })
        .unwrap_or(BranchCheck::Unreadable)
    }

    pub fn check_run_branch(&self, a: &AttemptGit) -> BranchCheck {
        self.with_repo(&a.run.repo, |repo| {
            self.validate_run(&a.run, repo)?;
            validate_attempt(a)?;
            self.check_under_lock(a)
        })
        .unwrap_or(BranchCheck::Unreadable)
    }

    fn check_under_lock(&self, a: &AttemptGit) -> Result<BranchCheck> {
        let Some(tip) = self.ref_tip(&a.run.mirror, &a.full_ref())? else {
            return Ok(BranchCheck::Absent);
        };
        let problem = self.branch_problem(a, &tip)?;
        Ok(BranchCheck::Present { tip, problem })
    }

    pub(crate) fn branch_problem(&self, a: &AttemptGit, tip: &str) -> Result<Option<String>> {
        let mirror = &a.run.mirror;
        let base = &a.run.base;
        if tip == base {
            return Ok(Some("commit range is empty".into()));
        }
        let is_ancestor = self.git(mirror, ["merge-base", "--is-ancestor", base, tip])?;
        match is_ancestor.status.code() {
            Some(0) => (),
            Some(1) => return Ok(Some("immutable base is not an ancestor".into())),
            _ => return Err(GitError::Command("verify base ancestry")),
        }
        let rev = format!("{base}..{tip}");
        let all = self.require(mirror, ["rev-list", &rev], "enumerate commit range")?;
        if all.is_empty() {
            return Ok(Some("commit range is empty".into()));
        }
        for oid in all.lines() {
            validate_oid(oid)?;
            let trailers = self.require(
                mirror,
                [
                    "show",
                    "-s",
                    "--format=%(trailers:key=Run-Id,valueonly,unfold)",
                    "--no-notes",
                    oid,
                ],
                "read Run-Id trailers",
            )?;
            if !trailers.lines().any(|t| t.trim() == a.run.id) {
                return Ok(Some(format!("commit {oid} is missing its Run-Id trailer")));
            }
        }
        Ok(None)
    }

    pub fn commit_summary(&self, a: &AttemptGit, tip: &str) -> Result<CommitSummary> {
        self.with_repo(&a.run.repo, |repo| {
            self.validate_run(&a.run, repo)?;
            validate_oid(tip)?;
            let range = format!("{}..{tip}", a.run.base);
            let files = self.require(
                &a.run.mirror,
                [
                    "diff",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--numstat",
                    &range,
                    "--",
                ],
                "summarize files",
            )?;
            let commits = self.require(
                &a.run.mirror,
                ["log", "--format=%H%x09%s", &range, "--"],
                "summarize commits",
            )?;
            Ok(CommitSummary { files, commits })
        })
    }

    /// Called after the terminal state is already committed. Errors are retryable
    /// synchronization warnings and cannot alter the result.
    pub fn sync_done(&self, a: &AttemptGit, result: &str) -> Result<()> {
        self.with_repo(&a.run.repo, |repo| {
            self.validate_run(&a.run, repo)?;
            validate_attempt(a)?;
            validate_oid(result)?;
            let r = a.full_ref();
            if self.ref_tip(&a.run.mirror, &r)?.as_deref() != Some(result) {
                return Err(GitError::Invalid("mirror tip changed since the result"));
            }
            let dest = format!("refs/heads/polyclave/{}", a.run.id);
            self.fetch(repo, &a.run.mirror, &format!("{r}:{dest}"))?;
            if self.ref_tip(repo, &dest)?.as_deref() != Some(result) {
                return Err(GitError::Invalid(
                    "local result ref differs from verified tip",
                ));
            }
            Ok(())
        })
    }

    pub fn staging_path(&self, a: &AttemptGit) -> PathBuf {
        self.root.join("staging").join(format!("{}.git", a.name()))
    }
}

#[derive(Debug, Eq, PartialEq, serde::Serialize)]
pub struct CommitSummary {
    pub files: String,
    pub commits: String,
}

pub(crate) fn ensure_real(path: &Path) -> Result<()> {
    if !std::fs::symlink_metadata(path)?.file_type().is_dir() || canonical_real_path(path)? != path
    {
        return Err(GitError::Invalid("managed directory was relocated"));
    }
    Ok(())
}
fn private_subdir(path: &Path) -> Result<()> {
    match create_private_dir(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => ensure_real(path),
        Err(e) => Err(e.into()),
    }
}
fn validate_id(id: &str) -> Result<()> {
    if id.len() != 9
        || !id.starts_with('r')
        || !id[1..]
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    {
        return Err(GitError::Invalid("invalid run identifier"));
    }
    Ok(())
}
pub(crate) fn validate_attempt(a: &AttemptGit) -> Result<()> {
    validate_id(&a.run.id)?;
    if a.number == 0 {
        return Err(GitError::Invalid("invalid Attempt generation"));
    }
    Ok(())
}
pub(crate) fn validate_oid(oid: &str) -> Result<()> {
    if oid.len() != 40
        || !oid
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
    {
        return Err(GitError::Invalid("expected a full SHA-1 commit id"));
    }
    Ok(())
}

impl MirrorManager {
    /// Bounded read of authoritative mirror evidence, never from a Worker's cwd.
    pub fn review_diff(
        &self,
        a: &AttemptGit,
        tip: &str,
        allow_path: impl Fn(&str) -> bool,
    ) -> Result<(Vec<String>, String)> {
        self.with_repo(&a.run.repo, |repo| {
            self.validate_run(&a.run, repo)?;
            validate_attempt(a)?;
            validate_oid(tip)?;
            let range = format!("{}..{tip}", a.run.base);
            let names = self.git(
                &a.run.mirror,
                [
                    "diff",
                    "--no-ext-diff",
                    "--no-textconv",
                    "--no-renames",
                    "--name-only",
                    "-z",
                    &range,
                    "--",
                ],
            )?;
            if !names.status.success() {
                return Err(GitError::Command("read review changed paths"));
            }
            if names.stdout.len() > 131_072 {
                return Err(GitError::Invalid("too many changed paths"));
            }
            let names = std::str::from_utf8(&names.stdout)
                .map_err(|_| GitError::Invalid("non-Unicode review path"))?;
            let files: Vec<String> = names
                .split('\0')
                .filter(|p| !p.is_empty() && allow_path(p))
                .take(100)
                .map(str::to_owned)
                .collect();
            if files.is_empty() {
                return Ok((files, String::new()));
            }
            let mut args: Vec<&str> = vec![
                "--literal-pathspecs",
                "diff",
                "--no-ext-diff",
                "--no-textconv",
                "--no-renames",
                &range,
                "--",
            ];
            args.extend(files.iter().map(String::as_str));
            let output = self.require(&a.run.mirror, args, "read review diff")?;
            let mut diff =
                output[..output.floor_char_boundary(output.len().min(32_768))].to_owned();
            if diff.len() < output.len() {
                diff.push_str("\n[Diff truncated by host]");
            }
            Ok((files, diff))
        })
    }
}

/// Removes credentials from a remote URL: all userinfo of an `http(s)://` URL
/// (a token may sit in the user name), and any `:password` part elsewhere.
/// scp-like `user@host:path` addresses carry no password and stay as they are.
pub fn strip_url_credentials(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_owned();
    };
    let (authority, path) = match rest.find('/') {
        Some(i) => rest.split_at(i),
        None => (rest, ""),
    };
    let Some((userinfo, host)) = authority.rsplit_once('@') else {
        return url.to_owned();
    };
    let lower = scheme.to_ascii_lowercase();
    if lower == "http" || lower == "https" {
        return format!("{scheme}://{host}{path}");
    }
    let user = userinfo.split(':').next().unwrap_or_default();
    if user.is_empty() {
        format!("{scheme}://{host}{path}")
    } else {
        format!("{scheme}://{user}@{host}{path}")
    }
}

#[cfg(test)]
mod tests {
    use super::strip_url_credentials;

    #[cfg(unix)]
    #[test]
    fn revoking_live_broker_git_releases_the_mirror_without_staging_a_commit() {
        use super::{AttemptGit, BranchCheck, GitError, MirrorManager};
        use std::{
            fs::{self, OpenOptions},
            io::Write,
            os::unix::fs::PermissionsExt,
            process::Command,
            sync::mpsc,
            time::Duration,
        };

        let temp = tempfile::tempdir().unwrap();
        let repo = temp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        let mut mirror = MirrorManager::new(temp.path()).unwrap();
        let repo = polyclave_os::canonical_real_path(&repo).unwrap();
        mirror
            .require(&repo, ["init", "-b", "main", "--template="], "init")
            .unwrap();
        mirror
            .require(
                &repo,
                [
                    "-c",
                    "user.name=Test",
                    "-c",
                    "user.email=test@example.invalid",
                    "commit",
                    "--allow-empty",
                    "-m",
                    "base",
                ],
                "base",
            )
            .unwrap();
        let run = mirror.admit(&repo, "r1234abcd", "main").unwrap();
        let attempt = AttemptGit { run, number: 1 };
        let workspace = mirror.prepare_attempt(&attempt).unwrap();
        fs::write(workspace.cwd.join("allowed.txt"), "change").unwrap();

        let root = &mirror.root;
        let fifo = root.join("hold");
        assert!(Command::new("/usr/bin/mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success());
        let mut release = OpenOptions::new()
            .read(true)
            .write(true)
            .open(&fifo)
            .unwrap();
        let wrapper = root.join("stall-git");
        let started = root.join("started");
        fs::write(
            &wrapper,
            r#"#!/bin/sh
for arg do
  if [ "$arg" = "--trailer" ]; then
    dir=${0%/*}
    : > "$dir/started"
    read -r line < "$dir/hold"
    break
  fi
done
exec git "$@"
"#,
        )
        .unwrap();
        fs::set_permissions(&wrapper, fs::Permissions::from_mode(0o700)).unwrap();
        mirror.git = wrapper;

        let (tx, rx) = mpsc::channel();
        let child_attempt = attempt.clone();
        let child_started = started.clone();
        let job = std::thread::spawn(move || {
            let result = mirror.commit_and_stage(
                &child_attempt,
                "change",
                None,
                &["allowed.txt".into()],
                move || !child_started.exists(),
            );
            tx.send(result).unwrap();
            mirror
        });
        let result = rx.recv_timeout(Duration::from_secs(15));
        if result.is_err() {
            writeln!(release, "unblock failed test").unwrap();
        }
        let mirror = job.join().unwrap();
        assert!(started.exists());
        assert!(matches!(
            result.unwrap(),
            Err(GitError::Invalid("Turn is no longer authorized to stage"))
        ));
        assert_eq!(mirror.reconcile(&attempt), BranchCheck::Absent);
        assert_eq!(
            mirror
                .commit_stage_dir(&attempt)
                .read_dir()
                .unwrap()
                .count(),
            0
        );
    }

    #[test]
    fn strips_credentials_from_remote_urls() {
        for (raw, clean) in [
            (
                "https://tok@example.com/o/r.git",
                "https://example.com/o/r.git",
            ),
            (
                "HTTPS://u:p@example.com/o/r.git",
                "HTTPS://example.com/o/r.git",
            ),
            ("https://example.com/o/r", "https://example.com/o/r"),
            (
                "ssh://git:pw@example.com/o/r.git",
                "ssh://git@example.com/o/r.git",
            ),
            (
                "ssh://git@example.com/o/r.git",
                "ssh://git@example.com/o/r.git",
            ),
            ("git@example.com:o/r.git", "git@example.com:o/r.git"),
            ("/local/path", "/local/path"),
        ] {
            assert_eq!(strip_url_credentials(raw), clean, "{raw}");
        }
    }
}
