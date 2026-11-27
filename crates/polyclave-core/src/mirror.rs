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

pub struct MirrorManager {
    pub(crate) root: PathBuf,
    git: PathBuf,
    hooks: PathBuf,
    locks: Mutex<RepoLocks>,
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
        for name in ["mirrors", "worktrees", "staging", "empty-hooks"] {
            private_subdir(&root.join(name))?;
        }
        Ok(Self {
            git: resolve_cli_binary(Path::new("git"))?,
            hooks: root.join("empty-hooks"),
            root,
            locks: Mutex::new(HashMap::new()),
        })
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
        Ok(cmd.output()?)
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

    fn staging_ref(&self, staging: &Path, name: &str) -> Result<Option<String>> {
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
        self.with_repo(&a.run.repo, |repo| {
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

    fn branch_problem(&self, a: &AttemptGit, tip: &str) -> Result<Option<String>> {
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
