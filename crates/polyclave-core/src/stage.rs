use crate::{
    mirror::{AttemptGit, MirrorManager},
    scope::{read_touched_paths, validate_touched_paths},
    GitError,
};
use polyclave_os::{canonical_real_path, create_private_dir};
use std::{
    ffi::{OsStr, OsString},
    path::{Component, Path, PathBuf},
};

type Result<T> = std::result::Result<T, GitError>;

struct CommitScratch(PathBuf);

impl Drop for CommitScratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

impl MirrorManager {
    pub(crate) fn commit_stage_dir(&self, attempt: &AttemptGit) -> PathBuf {
        self.root.join("commit-stage").join(attempt.name())
    }

    pub(crate) fn commit_and_stage(
        &self,
        attempt: &AttemptGit,
        message: &str,
        paths: Option<&[String]>,
        allowed_paths: &[String],
        can_publish: impl Fn() -> bool,
    ) -> Result<String> {
        if message.lines().any(|line| {
            line.split_once(':')
                .is_some_and(|(key, _)| key.trim().eq_ignore_ascii_case("Run-Id"))
        }) {
            return Err(GitError::Invalid("Run-Id is assigned by the host"));
        }
        if paths.is_some_and(|paths| paths.iter().any(|path| !is_literal_path(path))) {
            return Err(GitError::Invalid("Expected literal Attempt-relative paths"));
        }

        self.with_repo(&attempt.run.repo, |repo| {
            self.validate_run(&attempt.run, repo)?;
            let workspace = self.attempt_workspace(attempt);
            let stage_dir = self.commit_stage_dir(attempt);
            for path in [&workspace.cwd, &workspace.staging, &stage_dir] {
                if canonical_real_path(path)? != *path || !path.is_dir() {
                    return Err(GitError::Invalid("Attempt has no private staging boundary"));
                }
            }
            if !can_publish() {
                return Err(GitError::Invalid("Turn is no longer authorized to stage"));
            }
            let scratch = stage_dir.join(uuid::Uuid::new_v4().simple().to_string());
            create_private_dir(&scratch)?;
            let scratch = CommitScratch(scratch);
            let admin = &scratch.0;
            self.require(
                admin,
                ["init", "--bare", "--template="],
                "initialize host commit",
            )?;
            let base_ref = attempt.run.base_ref();
            self.fetch(
                admin,
                &attempt.run.mirror,
                &format!("{base_ref}:{base_ref}"),
            )?;
            let target = attempt.full_ref();
            let staged = self.staging_ref(&workspace.staging, &target)?;
            let parent = if let Some(sha) = &staged {
                self.fetch(admin, &workspace.staging, &format!("{target}:{target}"))?;
                let mut candidate = attempt.clone();
                candidate.run.mirror = admin.clone();
                if self.oid(admin, &target)? != *sha
                    || self.branch_problem(&candidate, sha)?.is_some()
                {
                    return Err(GitError::Invalid("Invalid prior staged commit"));
                }
                sha
            } else {
                &attempt.run.base
            };

            // Use only the host-owned Git configuration and index. The Worker
            // checkout is a work tree for reading files, even with .git read-only.
            let git = |args: &[&OsStr]| {
                let prefix = [
                    OsStr::new("--git-dir"),
                    admin.as_os_str(),
                    OsStr::new("--work-tree"),
                    workspace.cwd.as_os_str(),
                ];
                self.git(
                    &workspace.cwd,
                    prefix.into_iter().chain(args.iter().copied()),
                )
            };
            self.require(
                admin,
                ["update-ref", "HEAD", parent],
                "set host commit parent",
            )?;
            if !git(&["read-tree", "HEAD"].map(OsStr::new))?
                .status
                .success()
            {
                return Err(GitError::Command("initialize host commit index"));
            }
            let mut add: Vec<&OsStr> = ["--literal-pathspecs", "add", "--all", "--"]
                .map(OsStr::new)
                .to_vec();
            if let Some(paths) = paths {
                add.extend(paths.iter().map(OsStr::new));
            } else {
                add.push(OsStr::new("."));
            }
            if !git(&add)?.status.success() {
                return Err(GitError::Command("stage Attempt changes"));
            }
            let changed = git(&["diff", "--cached", "--quiet", "--"].map(OsStr::new))?;
            let sha = match changed.status.code() {
                Some(0) => {
                    staged.ok_or(GitError::Invalid("No changes to commit in this Attempt"))?
                }
                Some(1) => {
                    let trailer = format!("Run-Id: {}", attempt.run.id);
                    let args = [
                        "-c",
                        "user.name=Polyclave Worker",
                        "-c",
                        "user.email=worker@example.invalid",
                        "commit",
                        "--no-gpg-sign",
                        "--no-verify",
                        "--trailer",
                        &trailer,
                        "-m",
                        message,
                    ];
                    if !git(&args.map(OsStr::new))?.status.success() {
                        return Err(GitError::Command("commit Attempt changes"));
                    }
                    self.oid(admin, "HEAD")?
                }
                _ => return Err(GitError::Command("inspect staged changes")),
            };
            let changed_paths = read_touched_paths(admin, &attempt.run.base, &sha)?;
            if changed_paths.iter().any(|path| !is_literal_path(path))
                || !validate_touched_paths(&changed_paths, allowed_paths).allowed
            {
                return Err(GitError::Invalid(
                    "Changes are outside the Ticket path scope",
                ));
            }
            if !can_publish() {
                return Err(GitError::Invalid("Turn is no longer authorized to stage"));
            }
            let args: Vec<OsString> = ["push", "--no-verify", "--"]
                .map(OsString::from)
                .into_iter()
                .chain([
                    workspace.staging.as_os_str().to_owned(),
                    format!("{sha}:{target}").into(),
                ])
                .collect();
            self.require(admin, args, "publish to Attempt staging")?;
            if self.staging_ref(&workspace.staging, &target)?.as_deref() != Some(sha.as_str()) {
                return Err(GitError::Invalid(
                    "Attempt staging did not retain the committed SHA",
                ));
            }
            Ok(sha)
        })
    }
}

fn is_literal_path(path: &str) -> bool {
    !path.is_empty()
        && !path.contains(['\\', '\0', ':'])
        && path.split('/').all(|part| !part.is_empty() && !matches!(part, "." | ".."))
        && Path::new(path).components().all(|component| {
            matches!(component, Component::Normal(name) if !name.eq_ignore_ascii_case(".git"))
        })
}
