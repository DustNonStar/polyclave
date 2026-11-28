use crate::{
    mirror::{ensure_real, validate_attempt, validate_oid, AttemptGit, MirrorManager},
    GitError,
};
use polyclave_os::{canonical_real_path, create_private_dir, resolve_cli_binary};
use std::{
    ffi::{OsStr, OsString},
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
};

#[derive(Clone, Debug)]
pub struct Workspace {
    pub cwd: PathBuf,
    pub staging: PathBuf,
    pub branch: String,
}

impl MirrorManager {
    pub(crate) fn attempt_workspace(&self, a: &AttemptGit) -> Workspace {
        Workspace {
            cwd: self.root.join("worktrees").join(a.name()),
            staging: self.staging_path(a),
            branch: a.branch(),
        }
    }

    /// A standalone checkout has no shared writable Git metadata with either
    /// the owner's primary checkout or another Attempt.
    pub fn prepare_attempt(&self, a: &AttemptGit) -> Result<Workspace, GitError> {
        self.prepare_attempt_at(a, None)
    }

    /// A revision Attempt continues the previous Attempt's verified work instead
    /// of restarting from the Run's baseline. The Run's `base_sha` never moves,
    /// so the reviewed range still starts where it always did; only this
    /// Attempt's start commit differs.
    pub fn prepare_revision_attempt(
        &self,
        a: &AttemptGit,
        from: &str,
    ) -> Result<Workspace, GitError> {
        validate_oid(from)?;
        if a.number < 2 {
            return Err(GitError::Invalid("a revision needs an earlier Attempt"));
        }
        self.prepare_attempt_at(a, Some(from))
    }

    fn prepare_attempt_at(
        &self,
        a: &AttemptGit,
        from: Option<&str>,
    ) -> Result<Workspace, GitError> {
        self.with_repo(&a.run.repo, |repo| {
            self.validate_run(&a.run, repo)?;
            validate_attempt(a)?;
            let workspace = self.attempt_workspace(a);
            let Workspace {
                cwd,
                staging,
                branch,
            } = &workspace;
            create_private_dir(staging)?;
            self.require(
                staging,
                ["init", "--bare", "--template="],
                "init Attempt staging",
            )?;
            for key in ["receive.denyDeletes", "receive.denyNonFastForwards"] {
                self.require(
                    staging,
                    ["config", "--local", key, "true"],
                    "set staging policy",
                )?;
            }
            create_private_dir(cwd)?;
            self.require(cwd, ["init", "--template="], "init Attempt checkout")?;
            let base_ref = a.run.base_ref();
            self.fetch(
                cwd,
                &a.run.mirror,
                &format!("{base_ref}:refs/polyclave/base"),
            )?;
            if self.oid(cwd, "refs/polyclave/base")? != a.run.base {
                return Err(GitError::Invalid("checkout baseline differs from the Run"));
            }
            // A revision starts at the previous Attempt's verified SHA. It is
            // fetched by its own attempt ref, so the start commit must be
            // exactly what that ref serves - never a caller's loose SHA.
            let start = match from {
                Some(from) => {
                    let mut matched = false;
                    for prev_no in (1..a.number).rev() {
                        let candidate = AttemptGit {
                            run: a.run.clone(),
                            number: prev_no,
                        };
                        let candidate_ref = candidate.full_ref();
                        if self
                            .fetch(
                                cwd,
                                &a.run.mirror,
                                &format!("{candidate_ref}:refs/polyclave/revision"),
                            )
                            .is_ok()
                            && self.oid(cwd, "refs/polyclave/revision")? == from
                        {
                            matched = true;
                            break;
                        }
                    }
                    if !matched {
                        return Err(GitError::Invalid(
                            "no prior Attempt tip matches the revision base",
                        ));
                    }
                    let ancestor =
                        self.git(cwd, ["merge-base", "--is-ancestor", &a.run.base, from])?;
                    if ancestor.status.code() != Some(0) {
                        return Err(GitError::Invalid(
                            "the revision base is not descended from the Run baseline",
                        ));
                    }
                    from
                }
                None => &a.run.base,
            };
            self.require(
                cwd,
                ["checkout", "--no-recurse-submodules", "-b", branch, start],
                if from.is_some() {
                    "checkout revision base"
                } else {
                    "checkout immutable base"
                },
            )?;
            let args = [
                OsString::from("remote"),
                "add".into(),
                "polyclave-staging".into(),
                staging.as_os_str().to_owned(),
            ];
            self.require(cwd, args, "set the writable staging destination")?;
            ensure_real(cwd)?;
            ensure_real(staging)?;
            // A reservation made by older code could have a Worker-writable
            // Git configuration. Only freshly prepared staging is brokered.
            create_private_dir(&self.commit_stage_dir(a))?;
            Ok(workspace)
        })
    }
}

/// Manages disposable review worktrees detached at an exact `head_sha`.
pub struct WorktreeManager;

/// RAII guard for a detached review worktree that guarantees cleanup on drop.
pub struct DisposableReviewWorktree {
    mirror: PathBuf,
    git: PathBuf,
    parent_dir: PathBuf,
    worktree_dir: PathBuf,
}

impl DisposableReviewWorktree {
    pub fn path(&self) -> &Path {
        &self.worktree_dir
    }

    fn create(mirror: &Path, head_sha: &str) -> Result<Self, GitError> {
        ensure_real(mirror)?;
        validate_oid(head_sha)?;
        let mirror = canonical_real_path(mirror)?;
        let git = resolve_cli_binary(Path::new("git"))?;

        let commit_ref = format!("{head_sha}^{{commit}}");
        let resolved = run_git(&git, &mirror, ["rev-parse", "--verify", &commit_ref])?;
        if !resolved.status.success()
            || String::from_utf8_lossy(&resolved.stdout).trim() != head_sha
        {
            return Err(GitError::Invalid(
                "review head_sha does not resolve to a commit in mirror",
            ));
        }

        let parent_dir =
            std::env::temp_dir().join(format!("polyclave-review-{}", uuid::Uuid::new_v4()));
        create_private_dir(&parent_dir)?;
        let parent_dir = canonical_real_path(&parent_dir)?;
        let worktree_dir = parent_dir.join("wt");

        let guard = Self {
            mirror: mirror.clone(),
            git: git.clone(),
            parent_dir,
            worktree_dir: worktree_dir.clone(),
        };

        let wt_arg = worktree_dir.as_os_str();
        let add_args: [&OsStr; 4] = [
            OsStr::new("worktree"),
            OsStr::new("add"),
            OsStr::new("--detach"),
            wt_arg,
        ];
        let mut cmd_args = add_args.to_vec();
        cmd_args.push(OsStr::new(head_sha));
        let out = run_git(&git, &mirror, cmd_args)?;
        if !out.status.success() {
            return Err(GitError::Command("create disposable review worktree"));
        }

        let head = run_git(&git, &worktree_dir, ["rev-parse", "HEAD"])?;
        if !head.status.success() || String::from_utf8_lossy(&head.stdout).trim() != head_sha {
            return Err(GitError::Invalid(
                "disposable review worktree HEAD differs from head_sha",
            ));
        }

        Ok(guard)
    }
}

impl Drop for DisposableReviewWorktree {
    fn drop(&mut self) {
        let _ = run_git(
            &self.git,
            &self.mirror,
            [
                OsStr::new("worktree"),
                OsStr::new("remove"),
                OsStr::new("--force"),
                self.worktree_dir.as_os_str(),
            ],
        );
        let _ = run_git(&self.git, &self.mirror, ["worktree", "prune"]);
        let _ = std::fs::remove_dir_all(&self.parent_dir);
    }
}

impl WorktreeManager {
    /// Creates a detached worktree at exact `head_sha`, runs `f`, and guarantees
    /// worktree removal on exit, error, or panic.
    pub fn with_disposable_review_worktree<T, F>(
        mirror: &Path,
        head_sha: &str,
        f: F,
    ) -> Result<T, GitError>
    where
        F: FnOnce(&Path) -> Result<T, GitError>,
    {
        let wt = DisposableReviewWorktree::create(mirror, head_sha)?;
        f(wt.path())
    }

    /// Async variant of [`Self::with_disposable_review_worktree`].
    pub async fn with_disposable_review_worktree_async<T, Fut, F>(
        mirror: &Path,
        head_sha: &str,
        f: F,
    ) -> Result<T, GitError>
    where
        F: FnOnce(PathBuf) -> Fut,
        Fut: std::future::Future<Output = Result<T, GitError>>,
    {
        let wt = DisposableReviewWorktree::create(mirror, head_sha)?;
        f(wt.path().to_path_buf()).await
    }
}

fn run_git<I, S>(git: &Path, dir: &Path, args: I) -> Result<Output, GitError>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut cmd = Command::new(git);
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
        .args(args);
    Ok(cmd.output()?)
}
