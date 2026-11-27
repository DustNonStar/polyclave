//! Hub providers and the canonical conversation boundary.
pub mod api_provider;
pub mod cli_provider;
pub mod council;
pub mod curation;
pub mod evolution;
pub mod notify;
pub mod pr_gate;
pub mod provider;
pub mod publish_origin;
pub mod repo_tools;
pub mod scm;
pub mod scm_observer;
pub mod triage;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("provider profile is invalid or unavailable")]
    Profile,
    #[error("provider credential is unavailable")]
    Credential,
    #[error("the provider thread is unavailable")]
    MissingThread,
    #[error("the user stopped the Hub turn")]
    Cancelled,
    #[error("the user stopped the Hub turn before release")]
    CancelledBeforeStart,
    #[error("the provider did not start")]
    NotStarted,
    #[error("provider could not complete the request")]
    Transport,
    #[error("provider returned HTTP {status}{}", if detail.is_empty() { String::new() } else { format!(": {detail}") })]
    Http { status: u16, detail: String },
    #[error("provider returned an invalid or incomplete stream")]
    Protocol,
    #[error("provider response exceeded the configured bound")]
    Limit,
    #[error("the local conversation ledger could not be accessed")]
    Store,
    #[error("requested action is outside the registered scope")]
    Policy,
}
pub type Result<T> = std::result::Result<T, Error>;
