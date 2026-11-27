//! Host-owned Git state and run orchestration.
pub mod classify;
pub mod mirror;
pub mod worktree;

#[derive(Debug, thiserror::Error)]
pub enum GitError {
    #[error("invalid host Git state: {0}")]
    Invalid(&'static str),
    #[error("host Git I/O failed")]
    Io(#[from] std::io::Error),
    #[error("Git operation failed: {0}")]
    Command(&'static str),
}
