use crate::{
    mirror::{ensure_real, validate_attempt, AttemptGit, MirrorManager},
    GitError,
};
use polyclave_os::create_private_dir;
use std::{ffi::OsString, path::PathBuf};

#[derive(Clone, Debug)]
pub struct Workspace {
    pub cwd: PathBuf,
    pub staging: PathBuf,
    pub branch: String,
}

impl MirrorManager {
    /// A standalone checkout has no shared writable Git metadata with either
    /// the owner's primary checkout or another Attempt.
    pub fn prepare_attempt(&self, a: &AttemptGit) -> Result<Workspace, GitError> {
        self.with_repo(&a.run.repo, |repo| {
            self.validate_run(&a.run, repo)?;
            validate_attempt(a)?;
            let staging = self.staging_path(a);
            let cwd = self.root.join("worktrees").join(a.name());
            create_private_dir(&staging)?;
            self.require(
                &staging,
                ["init", "--bare", "--template="],
                "init Attempt staging",
            )?;
            for key in ["receive.denyDeletes", "receive.denyNonFastForwards"] {
                self.require(
                    &staging,
                    ["config", "--local", key, "true"],
                    "set staging policy",
                )?;
            }
            create_private_dir(&cwd)?;
            self.require(&cwd, ["init", "--template="], "init Attempt checkout")?;
            let base_ref = a.run.base_ref();
            self.fetch(
                &cwd,
                &a.run.mirror,
                &format!("{base_ref}:refs/polyclave/base"),
            )?;
            if self.oid(&cwd, "refs/polyclave/base")? != a.run.base {
                return Err(GitError::Invalid("checkout baseline differs from the Run"));
            }
            let branch = a.branch();
            self.require(
                &cwd,
                [
                    "checkout",
                    "--no-recurse-submodules",
                    "-b",
                    &branch,
                    &a.run.base,
                ],
                "checkout immutable base",
            )?;
            let args = [
                OsString::from("remote"),
                "add".into(),
                "polyclave-staging".into(),
                staging.as_os_str().to_owned(),
            ];
            self.require(&cwd, args, "set the writable staging destination")?;
            ensure_real(&cwd)?;
            ensure_real(&staging)?;
            Ok(Workspace {
                cwd,
                staging,
                branch,
            })
        })
    }
}
