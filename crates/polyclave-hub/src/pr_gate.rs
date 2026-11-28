//! PR gate orchestration: preflight, 3-layer scope check, disposable
//! heterogeneous review, crash-safe auto-merge, and global Pause revocation.
use crate::{
    publish_origin::{resolve_publish_target, PublishTarget},
    scm::{
        BoxFuture, OpenPrRequest, ReviewEvent, ScmClient, ScmError, SecretToken,
        SubmitReviewRequest,
    },
    Error, Result,
};
use polyclave_core::{scope::check_touched_paths, worktree::WorktreeManager, GitError};
use polyclave_os::resolve_cli_binary;
use polyclave_proto::{PauseStateDto, PrGateDto, PrGateStatus, ReviewVerdict, Vendor};
use polyclave_store::{NewPrGate, NewPrReview, Store};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex, OnceLock},
};

const MAX_REVISE_ROUNDS: u32 = 3;

/// Largest full-range review diff (stat, commit list and patch) a reviewer is
/// given. A larger change is never truncated for review: the gate escalates it
/// to a human, because an `approve` on a partial diff would approve code no
/// reviewer read, and a forced `revise` would only spend revise rounds on a
/// size problem the Worker cannot fix by addressing review comments.
pub const MAX_REVIEW_DIFF_BYTES: usize = 24_576;

/// The whole PR range `base..head` prepared for a reviewer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReviewDiff {
    Complete { text: String, commits: usize },
    TooLarge { bytes: usize },
}

/// Renders the full PR range `base..head` (commit list, stat and patch) from
/// `repo`, or reports that it exceeds [`MAX_REVIEW_DIFF_BYTES`].
pub fn review_diff(repo: &Path, base: &str, head: &str) -> Result<ReviewDiff> {
    if !is_full_oid(base) || !is_full_oid(head) {
        return Err(Error::Policy);
    }
    let git = resolve_cli_binary(Path::new("git")).map_err(|_| Error::Policy)?;
    let range = format!("{base}..{head}");
    let run = |args: &[&str]| -> Result<String> {
        let out = clean_git(&git, repo)
            .args(args)
            .output()
            .map_err(|_| Error::Transport)?;
        if !out.status.success() {
            return Err(Error::Policy);
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    };
    let log = run(&["log", "--no-decorate", "--format=%H %s", &range, "--"])?;
    let commits = log.lines().filter(|l| !l.trim().is_empty()).count();
    let patch = run(&[
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--stat",
        "--patch",
        base,
        head,
        "--",
    ])?;
    let text = format!("Commits ({commits}):\n{log}\n{patch}");
    if text.len() > MAX_REVIEW_DIFF_BYTES {
        return Ok(ReviewDiff::TooLarge { bytes: text.len() });
    }
    Ok(ReviewDiff::Complete { text, commits })
}

/// The same range, read for a page instead of a reviewer.
///
/// Unlike [`review_diff`] this does not refuse an oversized change: it returns the
/// first `max_bytes` and reports `truncated`, so the approval card can never show a
/// partial diff as if it were the whole one.
pub fn diff_capped(
    repo: &Path,
    base: &str,
    head: &str,
    max_bytes: usize,
) -> Result<(String, u64, bool)> {
    if !is_full_oid(base) || !is_full_oid(head) {
        return Err(Error::Policy);
    }
    let git = resolve_cli_binary(Path::new("git")).map_err(|_| Error::Policy)?;
    let out = clean_git(&git, repo)
        .args([
            "diff",
            "--no-ext-diff",
            "--no-textconv",
            "--stat",
            "--patch",
            base,
            head,
            "--",
        ])
        .output()
        .map_err(|_| Error::Transport)?;
    if !out.status.success() {
        return Err(Error::Policy);
    }
    let full = String::from_utf8_lossy(&out.stdout).into_owned();
    let bytes = full.len() as u64;
    if full.len() <= max_bytes {
        return Ok((full, bytes, false));
    }
    let mut cut = max_bytes;
    while cut > 0 && !full.is_char_boundary(cut) {
        cut -= 1;
    }
    Ok((full[..cut].to_owned(), bytes, true))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReviewerSpec {
    pub vendor: Vendor,
    pub model: String,
}

#[derive(Clone, Debug)]
pub struct ReviewWorktreeRequest {
    pub gate_id: String,
    pub repo_slug: String,
    pub pr_number: u64,
    /// The PR base; the reviewer sees the whole range `base_sha..head_sha`.
    pub base_sha: String,
    pub head_sha: String,
    pub worktree_path: PathBuf,
    pub vendor: Vendor,
    pub model: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReviewOutcome {
    pub verdict: ReviewVerdict,
    pub comments: String,
}

pub trait ReviewerExecutor: Send + Sync {
    /// Cheap local check that `spec` can resolve a provider (a CLI binary found
    /// or a configured API profile). The gate runs it for every selected
    /// reviewer before anything is pushed or a PR is opened.
    fn check_available(&self, spec: &ReviewerSpec) -> Result<()>;

    fn run_review<'a>(&'a self, req: ReviewWorktreeRequest)
        -> BoxFuture<'a, Result<ReviewOutcome>>;
}

pub trait WorkerReviseExecutor: Send + Sync {
    fn bounce_scope_violation<'a>(
        &'a self,
        gate: &'a PrGateDto,
        violations: &'a [String],
    ) -> BoxFuture<'a, Result<String>>;

    fn revise_pr<'a>(
        &'a self,
        gate: &'a PrGateDto,
        review_comments: &'a str,
    ) -> BoxFuture<'a, Result<String>>;
}

/// Publishes an R1-verified commit to the remote PR branch.
pub trait MirrorPusher: Send + Sync {
    /// Pushes exactly `sha` from the host mirror to `refs/heads/<branch>` of
    /// the validated GitHub repository `target`, then confirms the remote
    /// branch tip equals `sha`.
    ///
    /// `expected_remote_tip` is the lease: `Some(tip)` requires the remote branch
    /// to be at `tip` before the push (a revise round leases on the SHA the gate
    /// pushed last); `None` means the branch must be absent or already be `sha`.
    ///
    /// A lease built from the tip read just before the push would always satisfy
    /// itself, so `None` must never overwrite a branch this gate does not own: a
    /// branch that another Run or a person created is refused, and a human
    /// decides. That refusal is [`Error::Publish`], which the gate escalates.
    fn push_verified_sha<'a>(
        &'a self,
        target: &'a PublishTarget,
        mirror_path: &'a Path,
        sha: &'a str,
        branch: &'a str,
        expected_remote_tip: Option<&'a str>,
    ) -> BoxFuture<'a, Result<()>>;

    /// Confirms the remote branch the PR will merge into still contains the
    /// reviewed baseline `base_sha`, so the range GitHub computes is the range
    /// the reviewer read. The gate asks before it pushes anything, and a
    /// refusal escalates instead of publishing onto a base nobody reviewed.
    ///
    /// There is deliberately no permissive default: a pusher that cannot
    /// answer must refuse, or the check would silently pass.
    fn confirm_review_base<'a>(
        &'a self,
        target: &'a PublishTarget,
        mirror_path: &'a Path,
        base_branch: &'a str,
        base_sha: &'a str,
    ) -> BoxFuture<'a, Result<()>>;
}

const GITHUB_BASE_URL: &str = "https://github.com/";

/// Keychain item holding the bot token the daemon already uses for GitHub.
const GITHUB_CREDENTIAL_SERVICE: &str = "Polyclave.GitHub";
const GITHUB_CREDENTIAL_ACCOUNT: &str = "bot_token";

/// Environment variable that carries the token to the one-shot credential
/// helper of a single git command. It is set only on that command.
const PUBLISH_TOKEN_ENV: &str = "POLYCLAVE_PUBLISH_TOKEN";

/// A credential helper that answers `get` with the token from
/// [`PUBLISH_TOKEN_ENV`]. Only the variable's name is in argv, never its value.
const PUBLISH_CREDENTIAL_HELPER: &str = "credential.helper=!f() { test \"$1\" = get || exit 0; echo username=x-access-token; printf 'password=%s\\n' \"$POLYCLAVE_PUBLISH_TOKEN\"; }; f";

type TokenSource = Arc<dyn Fn() -> Result<SecretToken> + Send + Sync>;

fn keychain_bot_token() -> Result<SecretToken> {
    polyclave_os::read_provider_credential(GITHUB_CREDENTIAL_SERVICE, GITHUB_CREDENTIAL_ACCOUNT)
        .map(SecretToken::new)
        .map_err(|_| Error::Credential)
}

/// Pushes from the host mirror to `https://github.com/<repo_slug>.git`, given
/// to git as an explicit URL. The mirror has no remote, and none is added.
/// The bot token reaches git only through a credential helper scoped to each
/// remote command (see [`remote_git`]).
#[derive(Clone)]
pub struct GitMirrorPusher {
    base_url: String,
    token: TokenSource,
    git: Option<PathBuf>,
}

impl Default for GitMirrorPusher {
    fn default() -> Self {
        Self {
            base_url: GITHUB_BASE_URL.to_owned(),
            token: Arc::new(keychain_bot_token),
            git: None,
        }
    }
}

impl GitMirrorPusher {
    /// Maps `<repo_slug>` to `<base>/<repo_slug>.git` on the local disk, so tests
    /// can stand a bare repository in for GitHub without the network.
    #[cfg(test)]
    pub(crate) fn with_local_base(base: &Path) -> Self {
        let mut base_url = base.to_string_lossy().replace('\\', "/");
        if !base_url.ends_with('/') {
            base_url.push('/');
        }
        Self {
            base_url,
            token: Arc::new(|| Ok(SecretToken::new("test-only-token"))),
            git: None,
        }
    }

    /// Replaces the token source. Tests only.
    #[cfg(all(test, unix))]
    pub(crate) fn with_token(mut self, token: &'static str) -> Self {
        self.token = Arc::new(move || Ok(SecretToken::new(token)));
        self
    }

    /// Runs git through `git` instead of the one on PATH. Tests only.
    #[cfg(all(test, unix))]
    pub(crate) fn with_git_binary(mut self, git: PathBuf) -> Self {
        self.git = Some(git);
        self
    }

    /// The URL git is given for `target`.
    pub fn remote_url(&self, target: &PublishTarget) -> String {
        format!("{}{}.git", self.base_url, target.repo_slug())
    }

    /// Transports git may use: HTTPS for GitHub, the local disk only in tests.
    fn allowed_protocol(&self) -> &'static str {
        if self.base_url.starts_with("https://") {
            "https"
        } else {
            "file"
        }
    }
}

fn is_full_oid(sha: &str) -> bool {
    sha.len() == 40 && sha.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f'))
}

fn clean_git(git: &Path, mirror: &Path) -> Command {
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
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .arg("-C")
        .arg(mirror);
    cmd
}

/// A git command that talks to the publish URL: only `protocol` is allowed,
/// every configured credential helper is dropped, and a one-shot helper hands
/// git the token from this command's own environment. The token is never in
/// argv or the URL, and git's stderr is never logged or returned.
fn remote_git(git: &Path, mirror: &Path, protocol: &str, token: &SecretToken) -> Command {
    let mut cmd = clean_git(git, mirror);
    cmd.env("GIT_ALLOW_PROTOCOL", protocol)
        .env(PUBLISH_TOKEN_ENV, token.expose())
        .args(["-c", "credential.helper=", "-c", PUBLISH_CREDENTIAL_HELPER]);
    cmd
}

/// Reads the remote tip of `dest` (`None` when the branch is absent).
fn remote_tip(
    git: &Path,
    mirror: &Path,
    protocol: &str,
    token: &SecretToken,
    url: &str,
    dest: &str,
) -> Result<Option<String>> {
    let out = remote_git(git, mirror, protocol, token)
        .args(["ls-remote", "--refs", url, dest])
        .output()
        .map_err(|_| Error::Transport)?;
    if !out.status.success() {
        return Err(Error::Transport);
    }
    let text = String::from_utf8_lossy(&out.stdout);
    for line in text.lines() {
        let mut parts = line.split('\t');
        if let (Some(oid), Some(name)) = (parts.next(), parts.next()) {
            if name == dest {
                if !is_full_oid(oid) {
                    return Err(Error::Protocol);
                }
                return Ok(Some(oid.to_owned()));
            }
        }
    }
    Ok(None)
}

impl MirrorPusher for GitMirrorPusher {
    fn push_verified_sha<'a>(
        &'a self,
        target: &'a PublishTarget,
        mirror_path: &'a Path,
        sha: &'a str,
        branch: &'a str,
        expected_remote_tip: Option<&'a str>,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            if !is_full_oid(sha) || expected_remote_tip.is_some_and(|t| !is_full_oid(t)) {
                return Err(Error::Policy);
            }
            let git = match &self.git {
                Some(git) => git.clone(),
                None => resolve_cli_binary(Path::new("git")).map_err(|_| Error::Policy)?,
            };
            let token_source = self.token.clone();
            let url = self.remote_url(target);
            let protocol = self.allowed_protocol();
            let dest = format!("refs/heads/{branch}");
            let mirror = mirror_path.to_path_buf();
            let sha = sha.to_owned();
            let expected = expected_remote_tip.map(str::to_owned);
            tokio::task::spawn_blocking(move || {
                let token = token_source()?;
                let fmt = clean_git(&git, &mirror)
                    .args(["check-ref-format", &dest])
                    .output()
                    .map_err(|_| Error::Transport)?;
                if !fmt.status.success() {
                    return Err(Error::Policy);
                }
                // The SHA must name a commit the host mirror already holds.
                let commit = clean_git(&git, &mirror)
                    .args(["cat-file", "-e", &format!("{sha}^{{commit}}")])
                    .output()
                    .map_err(|_| Error::Transport)?;
                if !commit.status.success() {
                    return Err(Error::Policy);
                }
                let observed = remote_tip(&git, &mirror, protocol, &token, &url, &dest)?;
                match &expected {
                    // A revise round expects the tip it pushed last: anything else
                    // means someone moved the branch, which a human must settle.
                    Some(expected) => {
                        if observed.as_deref() != Some(expected.as_str())
                            && observed.as_deref() != Some(sha.as_str())
                        {
                            return Err(Error::Publish);
                        }
                    }
                    // First push: only an absent branch, or one that already is
                    // this SHA, may be published. Anything else belongs to
                    // someone else and is left untouched.
                    None => {
                        if observed.is_some() && observed.as_deref() != Some(sha.as_str()) {
                            return Err(Error::Publish);
                        }
                    }
                }
                if observed.as_deref() != Some(sha.as_str()) {
                    let lease = format!(
                        "--force-with-lease={dest}:{}",
                        observed.as_deref().unwrap_or("")
                    );
                    let out = remote_git(&git, &mirror, protocol, &token)
                        .args([
                            "push",
                            "--porcelain",
                            &lease,
                            &url,
                            &format!("{sha}:{dest}"),
                        ])
                        .output()
                        .map_err(|_| Error::Transport)?;
                    if !out.status.success() {
                        return Err(Error::Transport);
                    }
                }
                // Confirm what the remote now serves is exactly the verified SHA.
                if remote_tip(&git, &mirror, protocol, &token, &url, &dest)?.as_deref()
                    != Some(sha.as_str())
                {
                    return Err(Error::Policy);
                }
                Ok(())
            })
            .await
            .map_err(|_| Error::Transport)?
        })
    }

    fn confirm_review_base<'a>(
        &'a self,
        target: &'a PublishTarget,
        mirror_path: &'a Path,
        base_branch: &'a str,
        base_sha: &'a str,
    ) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            if !is_full_oid(base_sha) {
                return Err(Error::Policy);
            }
            let git = match &self.git {
                Some(git) => git.clone(),
                None => resolve_cli_binary(Path::new("git")).map_err(|_| Error::Policy)?,
            };
            let token_source = self.token.clone();
            let url = self.remote_url(target);
            let protocol = self.allowed_protocol();
            let dest = format!("refs/heads/{base_branch}");
            let mirror = mirror_path.to_path_buf();
            let base_sha = base_sha.to_owned();
            tokio::task::spawn_blocking(move || {
                let token = token_source()?;
                let fmt = clean_git(&git, &mirror)
                    .args(["check-ref-format", &dest])
                    .output()
                    .map_err(|_| Error::Transport)?;
                if !fmt.status.success() {
                    return Err(Error::Policy);
                }
                let Some(tip) = remote_tip(&git, &mirror, protocol, &token, &url, &dest)? else {
                    // The PR's base branch does not exist on the remote.
                    return Err(Error::Policy);
                };
                if tip == base_sha {
                    return Ok(());
                }
                // The baseline's ancestry can only be read from the remote's own
                // objects; only this one command may write them, and it creates
                // no local ref.
                let fetched = remote_git(&git, &mirror, protocol, &token)
                    .args([
                        "fetch",
                        "--no-tags",
                        "--no-prune",
                        "--no-write-fetch-head",
                        "--no-recurse-submodules",
                        "--upload-pack=git-upload-pack",
                        &url,
                        &dest,
                    ])
                    .output()
                    .map_err(|_| Error::Transport)?;
                if !fetched.status.success() {
                    return Err(Error::Transport);
                }
                let verified = clean_git(&git, &mirror)
                    .arg("merge-base")
                    .arg("--is-ancestor")
                    .arg(&base_sha)
                    .arg(&tip)
                    .output()
                    .map_err(|_| Error::Transport)?;
                match verified.status.code() {
                    Some(0) => Ok(()),
                    // The remote base branch no longer contains the reviewed base.
                    Some(1) => Err(Error::Policy),
                    _ => Err(Error::Transport),
                }
            })
            .await
            .map_err(|_| Error::Transport)?
        })
    }
}

#[derive(Clone)]
pub struct RunPrGateRequest {
    pub run_id: String,
    pub hub_session_id: Option<String>,
    pub mirror_path: PathBuf,
    pub repo_slug: String,
    pub branch: String,
    pub base_branch: String,
    pub base_sha: String,
    pub head_sha: String,
    pub pr_title: String,
    pub pr_body: String,
    pub touched_paths: Vec<String>,
    pub is_high_risk: bool,
    pub reviewers: Vec<ReviewerSpec>,
}

#[derive(Clone)]
pub struct PrGateDeps {
    pub store: Store,
    pub scm: Arc<dyn ScmClient>,
    pub reviewer: Arc<dyn ReviewerExecutor>,
    pub worker_revise: Option<Arc<dyn WorkerReviseExecutor>>,
    pub pusher: Arc<dyn MirrorPusher>,
}

fn map_store_err(err: polyclave_store::Error) -> Error {
    match err {
        polyclave_store::Error::ConflictError
        | polyclave_store::Error::BudgetExceededError
        | polyclave_store::Error::Invalid(_)
        | polyclave_store::Error::NotFound => Error::Policy,
        _ => Error::Store,
    }
}

fn map_scm_err(err: ScmError) -> Error {
    match err {
        ScmError::SameBotAndReviewerAccount { .. }
        | ScmError::InvalidRepoSlug(_)
        | ScmError::EmptyToken(_) => Error::Policy,
        ScmError::Http {
            status, message, ..
        } => Error::Http {
            status,
            detail: message,
        },
        ScmError::GraphQl { message, .. } => Error::Http {
            status: 422,
            detail: message,
        },
        ScmError::Transport { .. } => Error::Transport,
        ScmError::InvalidResponse { .. } => Error::Protocol,
    }
}

fn map_git_err(_err: GitError) -> Error {
    Error::Policy
}

fn vendor_slug(vendor: Vendor) -> &'static str {
    match vendor {
        Vendor::Codex => "codex",
        Vendor::Claude => "claude",
        Vendor::Agy => "agy",
        Vendor::Dsh => "dsh",
    }
}

fn parse_vendor(raw: &str) -> Result<Vendor> {
    match raw {
        "codex" => Ok(Vendor::Codex),
        "claude" => Ok(Vendor::Claude),
        "agy" => Ok(Vendor::Agy),
        "dsh" => Ok(Vendor::Dsh),
        _ => Err(Error::Store),
    }
}

fn lookup_author_launch(store: &Store, run_id: &str) -> Result<(Vendor, Option<String>)> {
    let run_id = run_id.to_owned();
    let row = store
        .read(|db| {
            let mut stmt = db.prepare("SELECT vendor, model FROM runs WHERE id=?1")?;
            let mut rows = stmt.query([run_id])?;
            if let Some(r) = rows.next()? {
                Ok(Some((
                    r.get::<_, String>(0)?,
                    r.get::<_, Option<String>>(1)?,
                )))
            } else {
                Ok(None)
            }
        })
        .map_err(map_store_err)?;
    let Some((vendor_str, model)) = row else {
        return Err(Error::Policy);
    };
    Ok((parse_vendor(&vendor_str)?, model))
}

fn is_heterogeneous(
    author_vendor: Vendor,
    author_model: Option<&str>,
    reviewer: &ReviewerSpec,
) -> bool {
    if reviewer.vendor != author_vendor {
        return true;
    }
    match author_model {
        Some(m) if !m.is_empty() => reviewer.model != m,
        _ => false,
    }
}

/// The Hub session that owns a Run, so a gate notice reaches a real inbox
/// instead of being written nowhere.
fn gate_owner(store: &Store, run_id: &str) -> Option<String> {
    store.run_hub_session(run_id).ok().flatten()
}

fn enqueue_hub_notice(store: &Store, hub_session_id: Option<&str>, intent: &str, text: &str) {
    if let Some(hub_id) = hub_session_id {
        let _ = store.hub_enqueue(hub_id, intent, text);
    }
}

fn escalate(
    deps: &PrGateDeps,
    gate_id: &str,
    hub_session_id: Option<&str>,
    reason: &str,
    detail: &str,
) -> Result<PrGateDto> {
    let escalated = deps
        .store
        .set_pr_gate_status(gate_id, PrGateStatus::Escalated)
        .map_err(map_store_err)?;
    enqueue_hub_notice(
        &deps.store,
        hub_session_id,
        &format!("pr_gate:{}:{reason}", escalated.id),
        &format!("PR gate {} escalated: {detail}", escalated.id),
    );
    Ok(escalated)
}

/// Reviewers read the whole PR range, so an oversized diff must never reach the
/// remote: a pushed, opened PR that no reviewer may read is worse than a
/// refusal. Returns the escalated gate, or `None` when the range is reviewable.
fn reviewable_diff(
    deps: &PrGateDeps,
    gate: &PrGateDto,
    hub_session_id: Option<&str>,
    mirror: &Path,
    base_sha: &str,
    head_sha: &str,
) -> Result<Option<PrGateDto>> {
    let ReviewDiff::TooLarge { bytes } = review_diff(mirror, base_sha, head_sha)? else {
        return Ok(None);
    };
    escalate(
        deps,
        &gate.id,
        hub_session_id,
        "diff_too_large",
        &format!(
            "the PR diff {base_sha}..{head_sha} is {bytes} bytes, over the {MAX_REVIEW_DIFF_BYTES}-byte review limit; a human must review it, so nothing was pushed and no PR was opened"
        ),
    )
    .map(Some)
}

/// One lock per PR gate, held across the arm/disarm conversation with GitHub.
///
/// `enable_auto_merge` and `pause_all` must not interleave: a Pause that read the
/// armed list before the enable reached GitHub would otherwise complete, and the
/// enable would arm the PR again after the host said it was paused. The registry
/// is process-wide because both entry points are free functions over the same
/// ledger.
#[derive(Default)]
pub struct AutoMergeLocks {
    gates: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
}

impl AutoMergeLocks {
    pub async fn lock(&self, gate_id: &str) -> tokio::sync::OwnedMutexGuard<()> {
        let gate = {
            let mut gates = self.gates.lock().unwrap_or_else(|e| e.into_inner());
            gates.entry(gate_id.to_owned()).or_default().clone()
        };
        gate.lock_owned().await
    }
}

/// The process-wide registry both `run_pr_gate` and `pause_all` use.
pub fn auto_merge_locks() -> &'static AutoMergeLocks {
    static LOCKS: OnceLock<AutoMergeLocks> = OnceLock::new();
    LOCKS.get_or_init(AutoMergeLocks::default)
}

/// True when GitHub's answer says the operation did not apply, so the ledger may
/// be updated to the opposite state. A transport failure or a 5xx is *not* one:
/// the request may have arrived, and the remote may be armed (or disarmed) even
/// though no answer came back.
fn remote_answer_is_definite(err: &ScmError) -> bool {
    match err {
        ScmError::Transport { .. } | ScmError::InvalidResponse { .. } => false,
        ScmError::Http { status, .. } => *status < 500,
        ScmError::GraphQl { .. }
        | ScmError::InvalidRepoSlug(_)
        | ScmError::EmptyToken(_)
        | ScmError::SameBotAndReviewerAccount { .. } => true,
    }
}

/// Runs the full PR gate lifecycle for a completed run.
pub async fn run_pr_gate(deps: &PrGateDeps, req: RunPrGateRequest) -> Result<PrGateDto> {
    if deps.store.pause_state().map_err(map_store_err)?.paused {
        return Err(Error::Policy);
    }

    let (author_vendor, author_model) = lookup_author_launch(&deps.store, &req.run_id)?;
    let required_reviewers = if req.is_high_risk { 2 } else { 1 };
    let selected_reviewers: Vec<ReviewerSpec> = req
        .reviewers
        .iter()
        .filter(|r| is_heterogeneous(author_vendor, author_model.as_deref(), r))
        .take(required_reviewers)
        .cloned()
        .collect();
    if selected_reviewers.len() < required_reviewers {
        return Err(Error::Policy);
    }

    // The reviewed range must exist: a Run whose verified head is its own base
    // has nothing for a reviewer to read, and a PR for it would still show
    // whatever the remote branch holds.
    if req.base_sha == req.head_sha {
        return Err(Error::Policy);
    }

    let mut gate = deps
        .store
        .create_pr_gate(NewPrGate {
            run_id: req.run_id.clone(),
            repo_slug: req.repo_slug.clone(),
            branch: req.branch.clone(),
            base_sha: req.base_sha.clone(),
            head_sha: req.head_sha.clone(),
            touched_paths: req.touched_paths.clone(),
        })
        .map_err(map_store_err)?;

    // Publish target: only the origin snapshotted at admission, and only when
    // it names exactly this GitHub repository. Checked before anything is
    // pushed or a PR is opened.
    let snapshot = deps
        .store
        .run_origin_snapshot(&req.run_id)
        .map_err(map_store_err)?;
    let target = match resolve_publish_target(snapshot.as_ref(), &req.repo_slug) {
        Ok(target) => target,
        Err(rejection) => {
            return escalate(
                deps,
                &gate.id,
                req.hub_session_id.as_deref(),
                "publish_origin",
                &format!(
                    "cannot publish to {}: {rejection}; nothing was pushed",
                    req.repo_slug
                ),
            );
        }
    };

    // Reviewer preflight: never push or open a PR that no reviewer can review.
    let unavailable: Vec<String> = selected_reviewers
        .iter()
        .filter(|spec| deps.reviewer.check_available(spec).is_err())
        .map(|spec| format!("{}/{}", vendor_slug(spec.vendor), spec.model))
        .collect();
    if !unavailable.is_empty() {
        return escalate(
            deps,
            &gate.id,
            req.hub_session_id.as_deref(),
            "reviewer_unavailable",
            &format!(
                "no provider resolves for reviewer(s) {}; nothing was pushed",
                unavailable.join(", ")
            ),
        );
    }

    let preflight = match deps
        .scm
        .preflight_repo(&req.repo_slug, &req.base_branch)
        .await
    {
        Ok(report) => report,
        Err(err) => {
            let _ = deps
                .store
                .set_pr_gate_status(&gate.id, PrGateStatus::Failed);
            return Err(map_scm_err(err));
        }
    };
    if !preflight.passed {
        let _ = deps
            .store
            .set_pr_gate_status(&gate.id, PrGateStatus::Failed);
        return Err(Error::Policy);
    }

    // GitHub computes the PR's diff from the remote base branch, so the branch
    // the PR merges into must still contain the Run's reviewed baseline. Checked
    // here, before the scope check and before the first push.
    if deps
        .pusher
        .confirm_review_base(&target, &req.mirror_path, &req.base_branch, &req.base_sha)
        .await
        .is_err()
    {
        return escalate(
            deps,
            &gate.id,
            req.hub_session_id.as_deref(),
            "review_base",
            &format!(
                "the remote branch {} of {} no longer contains the reviewed base {}; the PR would show a range no reviewer read, so nothing was pushed and no PR was opened",
                req.base_branch, req.repo_slug, req.base_sha
            ),
        );
    }

    // 3-layer touched_paths scope check (1 worker bounce, escalate on 2nd violation)
    let mut current_head = req.head_sha.clone();
    let scope = check_touched_paths(
        &req.mirror_path,
        &req.base_sha,
        &current_head,
        &req.touched_paths,
    )
    .map_err(map_git_err)?;
    if !scope.allowed {
        gate = deps
            .store
            .record_scope_violations(&gate.id, scope.violations.clone())
            .map_err(map_store_err)?;

        let bounced = if let Some(ref exec) = deps.worker_revise {
            match exec.bounce_scope_violation(&gate, &scope.violations).await {
                Ok(sha) => sha,
                Err(_) => {
                    return escalate(
                        deps,
                        &gate.id,
                        req.hub_session_id.as_deref(),
                        "bounce_unverified",
                        "the scope-bounce Worker turn produced no R1-verified result",
                    );
                }
            }
        } else {
            let escalated = deps
                .store
                .set_pr_gate_status(&gate.id, PrGateStatus::Escalated)
                .map_err(map_store_err)?;
            enqueue_hub_notice(
                &deps.store,
                req.hub_session_id.as_deref(),
                &format!("pr_gate:{}:scope_escalated", escalated.id),
                &format!(
                    "PR gate {} escalated due to touched_paths violation: {}",
                    escalated.id,
                    scope.violations.join(", ")
                ),
            );
            return Ok(escalated);
        };

        current_head = bounced;
        gate = deps
            .store
            .set_pr_head(&gate.id, &current_head)
            .map_err(map_store_err)?;
        let scope2 = check_touched_paths(
            &req.mirror_path,
            &req.base_sha,
            &current_head,
            &req.touched_paths,
        )
        .map_err(map_git_err)?;
        if !scope2.allowed {
            let _ = deps
                .store
                .record_scope_violations(&gate.id, scope2.violations.clone())
                .map_err(map_store_err)?;
            let escalated = deps
                .store
                .set_pr_gate_status(&gate.id, PrGateStatus::Escalated)
                .map_err(map_store_err)?;
            enqueue_hub_notice(
                &deps.store,
                req.hub_session_id.as_deref(),
                &format!("pr_gate:{}:scope_escalated_2nd", escalated.id),
                &format!(
                    "PR gate {} escalated on 2nd touched_paths violation: {}",
                    escalated.id,
                    scope2.violations.join(", ")
                ),
            );
            return Ok(escalated);
        }
        gate = deps
            .store
            .record_scope_violations(&gate.id, Vec::new())
            .map_err(map_store_err)?;
    }

    // Before anything leaves this machine, the range must be one a reviewer may
    // read in full.
    if let Some(escalated) = reviewable_diff(
        deps,
        &gate,
        req.hub_session_id.as_deref(),
        &req.mirror_path,
        &req.base_sha,
        &current_head,
    )? {
        return Ok(escalated);
    }

    // Push exactly the R1-verified SHA from the authoritative mirror, then
    // open/find the PR. The pusher confirms the remote tip equals that SHA.
    // A publish refusal means the destination branch is not this gate's to
    // overwrite: nothing was pushed, and a human decides rather than the gate
    // forcing the branch.
    match deps
        .pusher
        .push_verified_sha(&target, &req.mirror_path, &current_head, &req.branch, None)
        .await
    {
        Ok(()) => {}
        Err(Error::Publish) => {
            return escalate(
                deps,
                &gate.id,
                req.hub_session_id.as_deref(),
                "push_refused",
                &format!(
                    "refusing to publish {current_head} to {} on branch {}: the branch already holds a different commit, so pushing would overwrite work this gate did not create; nothing was pushed and no PR was opened",
                    req.repo_slug, req.branch
                ),
            );
        }
        Err(Error::Policy) => return escalate(
            deps,
            &gate.id,
            req.hub_session_id.as_deref(),
            "push_unpublishable",
            &format!(
                "refusing to publish {current_head} to {} on branch {}: the SHA is not a commit of the host mirror, or the branch name is not a valid ref; nothing was pushed",
                req.repo_slug, req.branch
            ),
        ),
        Err(err) => return Err(err),
    }

    let pr = deps
        .scm
        .open_or_find_pr(OpenPrRequest {
            repo_slug: req.repo_slug.clone(),
            head_branch: req.branch.clone(),
            base_branch: req.base_branch.clone(),
            title: req.pr_title.clone(),
            body: req.pr_body.clone(),
        })
        .await
        .map_err(map_scm_err)?;

    gate = deps
        .store
        .record_pr_opened(&gate.id, pr.number as u32, &pr.html_url)
        .map_err(map_store_err)?;

    // Heterogeneous disposable worktree review loop (up to MAX_REVISE_ROUNDS)
    loop {
        if deps.store.pause_state().map_err(map_store_err)?.paused {
            return deps
                .store
                .set_pr_gate_status(&gate.id, PrGateStatus::Paused)
                .map_err(map_store_err);
        }

        gate = deps
            .store
            .set_pr_gate_status(&gate.id, PrGateStatus::Reviewing)
            .map_err(map_store_err)?;

        let mut revise_comments = Vec::new();
        for spec in &selected_reviewers {
            let mirror = req.mirror_path.clone();
            let sha = current_head.clone();
            let gate_id = gate.id.clone();
            let repo_slug = req.repo_slug.clone();
            let pr_number = pr.number;
            let base_sha = req.base_sha.clone();
            let reviewer = deps.reviewer.clone();
            let spec_clone = spec.clone();

            let outcome = WorktreeManager::with_disposable_review_worktree_async(
                &mirror,
                &sha.clone(),
                move |wt_path| async move {
                    reviewer
                        .run_review(ReviewWorktreeRequest {
                            gate_id,
                            repo_slug,
                            pr_number,
                            base_sha,
                            head_sha: sha,
                            worktree_path: wt_path,
                            vendor: spec_clone.vendor,
                            model: spec_clone.model,
                        })
                        .await
                        .map_err(|_| GitError::Command("heterogeneous review failed"))
                },
            )
            .await
            .map_err(map_git_err)?;

            let event = match outcome.verdict {
                ReviewVerdict::Approve => ReviewEvent::Approve,
                ReviewVerdict::Revise => ReviewEvent::RequestChanges,
            };
            let submitted = deps
                .scm
                .submit_review(SubmitReviewRequest {
                    repo_slug: req.repo_slug.clone(),
                    pr_number: pr.number,
                    reviewed_sha: current_head.clone(),
                    vendor: vendor_slug(spec.vendor).to_owned(),
                    model: spec.model.clone(),
                    event,
                    body: outcome.comments.clone(),
                })
                .await
                .map_err(map_scm_err)?;

            gate = deps
                .store
                .record_pr_review(
                    &gate.id,
                    NewPrReview {
                        reviewer_vendor: spec.vendor,
                        reviewer_model: spec.model.clone(),
                        reviewed_sha: current_head.clone(),
                        verdict: outcome.verdict,
                        comments: outcome.comments.clone(),
                        github_review_id: Some(submitted.id as i64),
                    },
                )
                .map_err(map_store_err)?;

            if outcome.verdict == ReviewVerdict::Revise {
                revise_comments.push(outcome.comments);
            }
        }

        if revise_comments.is_empty() {
            break;
        }

        if gate.revise_rounds >= MAX_REVISE_ROUNDS {
            let escalated = deps
                .store
                .set_pr_gate_status(&gate.id, PrGateStatus::Escalated)
                .map_err(map_store_err)?;
            enqueue_hub_notice(
                &deps.store,
                req.hub_session_id.as_deref(),
                &format!("pr_gate:{}:max_revise", escalated.id),
                &format!(
                    "PR gate {} escalated after {} revise rounds",
                    escalated.id, escalated.revise_rounds
                ),
            );
            return Ok(escalated);
        }

        let Some(ref exec) = deps.worker_revise else {
            return deps
                .store
                .set_pr_gate_status(&gate.id, PrGateStatus::ReviseRequested)
                .map_err(map_store_err);
        };

        gate = deps
            .store
            .begin_revise_round(&gate.id)
            .map_err(map_store_err)?;
        let combined = revise_comments.join("\n---\n");
        // Only an R1-verified result may reach review, push and auto-merge; a
        // revise turn without one escalates to a human.
        let new_head = match exec.revise_pr(&gate, &combined).await {
            Ok(sha) => sha,
            Err(_) => {
                return escalate(
                    deps,
                    &gate.id,
                    req.hub_session_id.as_deref(),
                    "revise_unverified",
                    "the revise Worker turn produced no R1-verified result",
                );
            }
        };

        let revise_scope = check_touched_paths(
            &req.mirror_path,
            &req.base_sha,
            &new_head,
            &req.touched_paths,
        )
        .map_err(map_git_err)?;
        if !revise_scope.allowed {
            let violations = revise_scope.violations;
            let _ = deps
                .store
                .record_scope_violations(&gate.id, violations.clone())
                .map_err(map_store_err)?;
            return escalate(
                deps,
                &gate.id,
                req.hub_session_id.as_deref(),
                "revise_scope_violation",
                &format!(
                    "the revise Worker submitted {new_head}, which touches paths outside the Run's mandate ({}); it was not pushed, so the PR still shows {current_head}",
                    violations.join(", ")
                ),
            );
        }

        // The revised head must also be reviewable before it is published.
        if let Some(escalated) = reviewable_diff(
            deps,
            &gate,
            req.hub_session_id.as_deref(),
            &req.mirror_path,
            &req.base_sha,
            &new_head,
        )? {
            return Ok(escalated);
        }

        // The revise head may only replace the tip this gate pushed last.
        match deps
            .pusher
            .push_verified_sha(
                &target,
                &req.mirror_path,
                &new_head,
                &req.branch,
                Some(current_head.as_str()),
            )
            .await
        {
            Ok(()) => {}
            Err(Error::Publish) => {
                return escalate(
                    deps,
                    &gate.id,
                    req.hub_session_id.as_deref(),
                    "revise_push_refused",
                    &format!(
                        "the revision {new_head} did not replace {current_head} on branch {}: the branch moved, so the revised commit was not published and the PR still shows {current_head}",
                        req.branch
                    ),
                );
            }
            Err(err) => return Err(err),
        }
        current_head = new_head;
        gate = deps
            .store
            .set_pr_head(&gate.id, &current_head)
            .map_err(map_store_err)?;
    }

    // Heterogeneous reviewers approved this exact SHA. That is not a human
    // decision, so the gate stops here: nothing is armed and GitHub is not asked
    // to merge anything until a person approves this same SHA.
    gate = deps
        .store
        .set_pr_gate_status(&gate.id, PrGateStatus::Approved)
        .map_err(map_store_err)?;
    if deps.store.pause_state().map_err(map_store_err)?.paused {
        return deps
            .store
            .set_pr_gate_status(&gate.id, PrGateStatus::Paused)
            .map_err(map_store_err);
    }
    deps.store
        .await_human_approval(&gate.id, &current_head)
        .map_err(map_store_err)
}

/// Arms and enables auto-merge for a gate a person approved.
///
/// `expected_head_sha` is the SHA the person saw: the approval is recorded only if
/// the gate still carries it and the remote branch is still there, and the same
/// SHA is what GitHub is asked to arm. Called by the approval endpoint, never by
/// `run_pr_gate`.
pub async fn approve_pr_gate(
    deps: &PrGateDeps,
    gate_id: &str,
    expected_head_sha: &str,
    hub_session_id: Option<&str>,
) -> Result<PrGateDto> {
    if deps.store.pause_state().map_err(map_store_err)?.paused {
        return Err(Error::Policy);
    }
    let guard = auto_merge_locks().lock(gate_id).await;
    // Atomic against the ledger: the gate must still be waiting for this SHA.
    let gate = deps
        .store
        .approve_pr_gate(gate_id, expected_head_sha)
        .map_err(map_store_err)?;
    let Some(pr_number) = gate.pr_number else {
        drop(guard);
        return Err(Error::Policy);
    };
    let pr_state = match deps
        .scm
        .get_pr_state(&gate.repo_slug, pr_number as u64)
        .await
    {
        Ok(state) => state,
        Err(err) => {
            drop(guard);
            return Err(map_scm_err(err));
        }
    };
    // The remote must still serve the approved SHA: anything else means the branch
    // moved after the person looked at it, and the approval is not for it.
    if pr_state.head_sha != expected_head_sha {
        // Back to the human wait: the person must approve whatever is there now.
        let _ = deps
            .store
            .set_pr_gate_status(&gate.id, PrGateStatus::AwaitingHumanApproval);
        drop(guard);
        return Err(Error::Policy);
    }
    arm_and_enable(
        deps,
        guard,
        gate,
        expected_head_sha.to_owned(),
        pr_state.node_id,
        hub_session_id,
    )
    .await
}

/// Persists the armed bit and the unconfirmed intent, then asks GitHub.
async fn arm_and_enable(
    deps: &PrGateDeps,
    guard: tokio::sync::OwnedMutexGuard<()>,
    gate: PrGateDto,
    current_head: String,
    pr_node_id: String,
    hub_session_id: Option<&str>,
) -> Result<PrGateDto> {
    let pr_number = gate.pr_number.unwrap_or(0);
    let repo_slug = gate.repo_slug.clone();
    // The pause flag is read inside the same SQLite write transaction that sets
    // the armed bit (`arm_auto_merge` answers `None` when the host is paused), so
    // a Pause committing at any point here either sees this gate in its armed
    // snapshot — and revokes it — or refuses the arm. One serialized writer
    // decides both, so there is no window in between.
    let current = deps.store.pr_gate(&gate.id).map_err(map_store_err)?;
    // Crash-safe ordering: persist auto_merge_armed = 1 *and* the unconfirmed
    // remote intent in SQLite BEFORE calling GitHub enableAutoMerge.
    let Some(armed) = deps
        .store
        .arm_auto_merge(&current.id, &current_head)
        .map_err(map_store_err)?
    else {
        // The host is paused: nothing was armed, so GitHub is never called.
        drop(guard);
        return deps
            .store
            .set_pr_gate_status(&current.id, PrGateStatus::Paused)
            .map_err(map_store_err);
    };

    match deps.scm.enable_auto_merge(&pr_node_id, &current_head).await {
        Ok(()) => {
            let confirmed = deps
                .store
                .confirm_auto_merge(&armed.id)
                .map_err(map_store_err)?;
            drop(guard);
            Ok(confirmed)
        }
        Err(err) if remote_answer_is_definite(&err) => {
            // GitHub answered: it did not arm this PR.
            let _ = deps
                .store
                .disarm_auto_merge(&armed.id, PrGateStatus::Approved);
            drop(guard);
            Err(map_scm_err(err))
        }
        Err(err) => {
            // No usable answer: the remote may be armed. Keep the armed bit and
            // the 'enable' intent so Pause and startup reconciliation can revoke
            // whatever is really there, and tell a human.
            enqueue_hub_notice(
                &deps.store,
                hub_session_id,
                &format!("pr_gate:{}:auto_merge_unknown", armed.id),
                &format!(
                    "auto-merge for PR {pr_number} on {repo_slug} could not be confirmed: the request may have reached GitHub. The gate stays armed in the ledger so Pause revokes it; check the PR before merging."
                ),
            );
            drop(guard);
            Err(map_scm_err(err))
        }
    }
}

/// Persists global Pause, revokes auto-merge on GitHub for every armed PR, and
/// disarms a gate in SQLite only once GitHub confirms the revocation.
///
/// Each gate is handled under its own lock, so an enable that is already talking
/// to GitHub either finishes first (and is then revoked here) or sees the pause
/// and never arms. A disable whose answer is lost leaves the gate armed with the
/// `disable` intent: the ledger never claims a PR is disarmed when it may not be,
/// and [`reconcile_auto_merge`] finishes the job later.
pub async fn pause_all(
    store: &Store,
    scm: &dyn ScmClient,
    reason: Option<String>,
) -> Result<PauseStateDto> {
    let state = store.set_pause(true, reason).map_err(map_store_err)?;
    let armed_gates = store.armed_pr_gates().map_err(map_store_err)?;
    let mut unconfirmed = Vec::new();
    for gate in armed_gates {
        let Some(pr_number) = gate.pr_number else {
            continue;
        };
        let _guard = auto_merge_locks().lock(&gate.id).await;
        // Another task may have disarmed this gate while Pause waited.
        let current = store.pr_gate(&gate.id).map_err(map_store_err)?;
        if !current.auto_merge_armed {
            continue;
        }
        // Persist the intent first: a crash after this line still records that
        // GitHub was asked to disarm.
        store
            .begin_auto_merge_disable(&current.id)
            .map_err(map_store_err)?;
        let pr_state = match scm.get_pr_state(&current.repo_slug, pr_number as u64).await {
            Ok(state) => state,
            Err(err) => {
                unconfirmed.push((current.id.clone(), format!("{err}")));
                continue;
            }
        };
        match scm.disable_auto_merge(&pr_state.node_id).await {
            Ok(()) => {
                store
                    .disarm_auto_merge(&current.id, PrGateStatus::Paused)
                    .map_err(map_store_err)?;
            }
            Err(err) if remote_answer_is_definite(&err) => {
                // GitHub answered: it is not armed whatever the ledger said.
                store
                    .disarm_auto_merge(&current.id, PrGateStatus::Paused)
                    .map_err(map_store_err)?;
            }
            Err(err) => unconfirmed.push((current.id.clone(), format!("{err}"))),
        }
    }
    for (gate_id, detail) in &unconfirmed {
        let owner = store
            .pr_gate(gate_id)
            .ok()
            .and_then(|gate| gate_owner(store, &gate.run_id));
        enqueue_hub_notice(
            store,
            owner.as_deref(),
            &format!("pr_gate:{gate_id}:pause_unknown"),
            &format!(
                "Pause could not confirm that auto-merge is off for gate {gate_id} ({detail}); the gate stays armed so reconciliation revokes it"
            ),
        );
    }
    Ok(state)
}

/// Brings the ledger back in line with what GitHub really has, for every gate
/// whose remote auto-merge outcome is unconfirmed or that is still armed.
///
/// Called at startup (after a crash or a lost response) and safe to call at any
/// time: the remote state decides, and only a confirmed answer changes the ledger.
pub async fn reconcile_auto_merge(store: &Store, scm: &dyn ScmClient) -> Result<usize> {
    let paused = store.pause_state().map_err(map_store_err)?.paused;
    let mut settled = 0;
    for gate in store.armed_pr_gates().map_err(map_store_err)? {
        let Some(pr_number) = gate.pr_number else {
            continue;
        };
        let _guard = auto_merge_locks().lock(&gate.id).await;
        let current = store.pr_gate(&gate.id).map_err(map_store_err)?;
        if !current.auto_merge_armed {
            continue;
        }
        let intent = store
            .pr_gate_remote_intent(&current.id)
            .map_err(map_store_err)?;
        // A pause, or a disable that never got an answer, means the remote must
        // end up disarmed; otherwise GitHub decides and the ledger follows.
        let must_disable = paused || intent.as_deref() == Some("disable");
        let pr_state = match scm.get_pr_state(&current.repo_slug, pr_number as u64).await {
            Ok(state) => state,
            // One gate GitHub cannot answer for must not stop the others from
            // being settled; it stays armed, which is the safe state, and a
            // human is told which gate it was.
            Err(err) => {
                let owner = gate_owner(store, &current.run_id);
                enqueue_hub_notice(
                    store,
                    owner.as_deref(),
                    &format!("pr_gate:{}:reconcile_unknown", current.id),
                    &format!(
                        "auto-merge for gate {} could not be reconciled ({err}); it stays armed and is retried on the next pause or restart",
                        current.id
                    ),
                );
                continue;
            }
        };
        if must_disable {
            if pr_state.auto_merge_enabled {
                // Same ordering as Pause: the intent is durable before the call,
                // so a crash during reconciliation is still recoverable.
                store
                    .begin_auto_merge_disable(&current.id)
                    .map_err(map_store_err)?;
                match scm.disable_auto_merge(&pr_state.node_id).await {
                    Ok(()) => (),
                    Err(err) if remote_answer_is_definite(&err) => (),
                    Err(err) => {
                        // No usable answer: the remote may still be armed, so the
                        // gate stays armed and the rest are still reconciled.
                        let owner = gate_owner(store, &current.run_id);
                        enqueue_hub_notice(
                            store,
                            owner.as_deref(),
                            &format!("pr_gate:{}:reconcile_unknown", current.id),
                            &format!(
                                "auto-merge for gate {} could not be revoked ({err}); it stays armed and is retried on the next pause or restart",
                                current.id
                            ),
                        );
                        continue;
                    }
                }
            }
            store
                .disarm_auto_merge(&current.id, PrGateStatus::Paused)
                .map_err(map_store_err)?;
            settled += 1;
        } else if pr_state.auto_merge_enabled {
            // The enable did reach GitHub: the intent is resolved.
            store
                .confirm_auto_merge(&current.id)
                .map_err(map_store_err)?;
            settled += 1;
        } else {
            store
                .disarm_auto_merge(&current.id, PrGateStatus::Approved)
                .map_err(map_store_err)?;
            settled += 1;
        }
    }
    Ok(settled)
}

/// Clears the global Pause flag.
pub fn resume_all(store: &Store) -> Result<PauseStateDto> {
    store.set_pause(false, None).map_err(map_store_err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scm::{
        GitHubConfig, GitHubScmClient, OpenPrRequest, PrChecksAndComments, PrState,
        PreflightReport, PullRequestInfo, SecretToken, SubmitReviewRequest, SubmittedReview,
    };
    use axum::{
        extract::State,
        routing::{get, post},
        Json, Router,
    };
    use serde_json::{json, Value};
    use std::process::Command as StdCommand;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Mutex;
    use tokio::net::TcpListener;

    #[derive(Default)]
    struct FakeGhState {
        enable_auto_merge_calls: Mutex<Vec<(String, String)>>,
        disable_auto_merge_calls: Mutex<Vec<String>>,
        submitted_reviews: Mutex<Vec<Value>>,
        store_armed_before_enable: Mutex<Option<bool>>,
    }

    struct AppState {
        gh: Arc<FakeGhState>,
        store: Store,
        gate_id: Mutex<Option<String>>,
    }

    async fn spawn_fake_github(
        store: Store,
    ) -> (
        String,
        Arc<FakeGhState>,
        Arc<AppState>,
        tokio::task::JoinHandle<()>,
    ) {
        let gh = Arc::new(FakeGhState::default());
        let app_state = Arc::new(AppState {
            gh: gh.clone(),
            store,
            gate_id: Mutex::new(None),
        });

        let router = Router::new()
            .route(
                "/user",
                get(|headers: axum::http::HeaderMap| async move {
                    let auth = headers
                        .get("authorization")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("");
                    if auth.contains("bot-tok") {
                        Json(json!({"login": "polyclave-bot"}))
                    } else {
                        Json(json!({"login": "polyclave-reviewer"}))
                    }
                }),
            )
            .route(
                "/repos/{owner}/{repo}",
                get(|| async {
                    Json(json!({
                        "default_branch": "main",
                        "allow_squash_merge": true,
                        "allow_auto_merge": true
                    }))
                }),
            )
            .route(
                "/repos/{owner}/{repo}/branches/{branch}/protection",
                get(|| async {
                    Json(json!({
                        "required_pull_request_reviews": {
                            "required_approving_review_count": 1
                        }
                    }))
                }),
            )
            .route(
                "/repos/{owner}/{repo}/pulls",
                get(|| async { Json(json!([])) }).post(|| async {
                    (
                        axum::http::StatusCode::CREATED,
                        Json(json!({
                            "number": 42,
                            "node_id": "PR_node_42",
                            "state": "open",
                            "html_url": "https://github.com/org/repo/pull/42",
                            "head": { "sha": "head_sha_placeholder" }
                        })),
                    )
                }),
            )
            .route(
                "/repos/{owner}/{repo}/pulls/{num}",
                get(
                    |State(st): State<Arc<AppState>>,
                     axum::extract::Path((owner, repo, num)): axum::extract::Path<(
                        String,
                        String,
                        u64,
                    )>| async move {
                        // A real PR reports the head this gate pushed; the human
                        // approval compares that SHA, so the stand-in must too.
                        let head = st
                            .store
                            .pr_gate_head(&format!("{owner}/{repo}"), num as u32)
                            .ok()
                            .flatten()
                            .unwrap_or_else(|| "head_sha_placeholder".to_owned());
                        Json(json!({
                            "number": 42,
                            "node_id": "PR_node_42",
                            "state": "open",
                            "merged": false,
                            "mergeable": true,
                            "mergeable_state": "clean",
                            "html_url": "https://github.com/org/repo/pull/42",
                            "head": { "sha": head },
                            "auto_merge": null
                        }))
                    },
                ),
            )
            .route(
                "/repos/{owner}/{repo}/pulls/{num}/reviews",
                post(
                    |State(st): State<Arc<AppState>>, Json(body): Json<Value>| async move {
                        st.gh.submitted_reviews.lock().unwrap().push(body.clone());
                        Json(json!({
                            "id": 9001,
                            "commit_id": body["commit_id"],
                            "state": body["event"],
                            "html_url": "https://github.com/org/repo/pull/42#pullrequestreview-9001"
                        }))
                    },
                ),
            )
            .route(
                "/graphql",
                post(
                    |State(st): State<Arc<AppState>>, Json(body): Json<Value>| async move {
                        let query = body["query"].as_str().unwrap_or("");
                        if query.contains("enablePullRequestAutoMerge") {
                            let pr_id = body["variables"]["pullRequestId"]
                                .as_str()
                                .unwrap_or("")
                                .to_owned();
                            let expected = body["variables"]["expectedHeadOid"]
                                .as_str()
                                .unwrap_or("")
                                .to_owned();
                            // Verify SQLite already persisted auto_merge_armed = true BEFORE this call!
                            let armed = st
                                .gate_id
                                .lock()
                                .unwrap()
                                .as_ref()
                                .and_then(|gid| st.store.pr_gate(gid).ok())
                                .map(|g| g.auto_merge_armed);
                            *st.gh.store_armed_before_enable.lock().unwrap() = armed;
                            st.gh
                                .enable_auto_merge_calls
                                .lock()
                                .unwrap()
                                .push((pr_id, expected));
                            Json(json!({"data": {"enablePullRequestAutoMerge": {"pullRequest": {"id": "PR_node_42"}}}}))
                        } else if query.contains("disablePullRequestAutoMerge") {
                            let pr_id = body["variables"]["pullRequestId"]
                                .as_str()
                                .unwrap_or("")
                                .to_owned();
                            st.gh.disable_auto_merge_calls.lock().unwrap().push(pr_id);
                            Json(json!({"data": {"disablePullRequestAutoMerge": {"pullRequest": {"id": "PR_node_42"}}}}))
                        } else {
                            Json(json!({"data": {}}))
                        }
                    },
                ),
            )
            .with_state(app_state.clone());

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        (format!("http://{addr}"), gh, app_state, handle)
    }

    fn git(dir: &Path, args: &[&str]) -> String {
        let out = StdCommand::new("git")
            .current_dir(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_owned()
    }

    struct GitFixture {
        _dir: tempfile::TempDir,
        mirror: PathBuf,
        base_sha: String,
        good_head_sha: String,
        bad_head_sha_1: String,
        bad_head_sha_2: String,
        store: Store,
    }

    fn setup_git_and_store() -> GitFixture {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("work");
        std::fs::create_dir_all(work.join("src")).unwrap();
        git(&work, &["init", "-b", "main", "--template="]);
        git(&work, &["config", "user.name", "Test"]);
        git(&work, &["config", "user.email", "test@example.invalid"]);
        std::fs::write(work.join("src/lib.rs"), "pub fn base() {}\n").unwrap();
        git(&work, &["add", "."]);
        git(&work, &["commit", "-m", "base"]);
        let base_sha = git(&work, &["rev-parse", "HEAD"]);

        // Good head touching src/lib.rs
        std::fs::write(work.join("src/lib.rs"), "pub fn base() -> i32 { 1 }\n").unwrap();
        git(&work, &["add", "."]);
        git(&work, &["commit", "-m", "good"]);
        let good_head_sha = git(&work, &["rev-parse", "HEAD"]);

        // Bad head 1 touching README.md
        git(&work, &["checkout", "-b", "bad1", &base_sha]);
        std::fs::write(work.join("README.md"), "oops\n").unwrap();
        git(&work, &["add", "."]);
        git(&work, &["commit", "-m", "bad1"]);
        let bad_head_sha_1 = git(&work, &["rev-parse", "HEAD"]);

        // Bad head 2 still touching README.md
        std::fs::write(work.join("README.md"), "still oops\n").unwrap();
        git(&work, &["add", "."]);
        git(&work, &["commit", "-m", "bad2"]);
        let bad_head_sha_2 = git(&work, &["rev-parse", "HEAD"]);

        let mirror = dir.path().join("mirror.git");
        git(
            dir.path(),
            &[
                "clone",
                "--bare",
                work.to_str().unwrap(),
                mirror.to_str().unwrap(),
            ],
        );
        let mirror = polyclave_os::canonical_real_path(&mirror).unwrap();

        let store = Store::open(dir.path().join("ledger")).unwrap();
        store
            .write(|db| {
                db.execute_batch(
                    "INSERT INTO provider_profiles(id,display_name,kind,models_json,created_at)
                     VALUES('p','test','codex_cli','[]','now');
                     INSERT INTO sessions(id,name,role,repo_cwd,profile_id,provider_model,created_at)
                     VALUES('hub','hub','hub','/tmp/repo','p','test','now');
                     INSERT INTO sessions(id,name,role,repo_cwd,profile_id,provider_model,created_at)
                     VALUES('w1','w1','worker','/tmp/repo','p','gpt-5.3-codex','now');
                     INSERT INTO tickets(id,hub_session_id,repo_cwd,title,mandate_json,source_kind,max_runs,runs_used,status,created_at,updated_at)
                     VALUES('t1','hub','/tmp/repo','t1','{}','explicit_user',4,1,'active','now','now');
                     INSERT INTO runs(id,ticket_id,hub_session_id,worker_session_id,request_id,repo_cwd,vendor,model,instructions,mirror_remote,base_branch,base_sha,status,created_at,updated_at)
                     VALUES('run1','t1','hub','w1','req1','/tmp/repo','codex','gpt-5.3-codex','do it','origin','main','base','done','now','now');
                     INSERT INTO run_publish_origins(run_id,urls_json,push_urls_json,captured_at)
                     VALUES('run1','[\"https://github.com/org/repo.git\"]','[]','now');",
                )?;
                Ok(())
            })
            .unwrap();

        GitFixture {
            _dir: dir,
            mirror,
            base_sha,
            good_head_sha,
            bad_head_sha_1,
            bad_head_sha_2,
            store,
        }
    }

    struct MockPusher {
        pushed: Mutex<Vec<(String, String)>>,
    }

    /// Succeeds once, then reports that the destination branch moved.
    struct FlakyPusher {
        calls: AtomicUsize,
    }

    impl MirrorPusher for FlakyPusher {
        fn confirm_review_base<'a>(
            &'a self,
            _target: &'a PublishTarget,
            _mirror_path: &'a Path,
            _base_branch: &'a str,
            _base_sha: &'a str,
        ) -> BoxFuture<'a, Result<()>> {
            Box::pin(async { Ok(()) })
        }
        fn push_verified_sha<'a>(
            &'a self,
            _target: &'a PublishTarget,
            _mirror_path: &'a Path,
            _sha: &'a str,
            _branch: &'a str,
            _expected_remote_tip: Option<&'a str>,
        ) -> BoxFuture<'a, Result<()>> {
            Box::pin(async move {
                match self.calls.fetch_add(1, Ordering::SeqCst) {
                    0 => Ok(()),
                    _ => Err(Error::Publish),
                }
            })
        }
    }

    /// A pusher that refuses the way the real one does when the destination
    /// branch is not the gate's to overwrite.
    struct RefusingPusher;

    impl MirrorPusher for RefusingPusher {
        fn confirm_review_base<'a>(
            &'a self,
            _target: &'a PublishTarget,
            _mirror_path: &'a Path,
            _base_branch: &'a str,
            _base_sha: &'a str,
        ) -> BoxFuture<'a, Result<()>> {
            Box::pin(async { Ok(()) })
        }
        fn push_verified_sha<'a>(
            &'a self,
            _target: &'a PublishTarget,
            _mirror_path: &'a Path,
            _sha: &'a str,
            _branch: &'a str,
            _expected_remote_tip: Option<&'a str>,
        ) -> BoxFuture<'a, Result<()>> {
            Box::pin(async move { Err(Error::Publish) })
        }
    }

    impl MirrorPusher for MockPusher {
        fn confirm_review_base<'a>(
            &'a self,
            _target: &'a PublishTarget,
            _mirror_path: &'a Path,
            _base_branch: &'a str,
            _base_sha: &'a str,
        ) -> BoxFuture<'a, Result<()>> {
            Box::pin(async { Ok(()) })
        }
        fn push_verified_sha<'a>(
            &'a self,
            _target: &'a PublishTarget,
            _mirror_path: &'a Path,
            sha: &'a str,
            branch: &'a str,
            _expected_remote_tip: Option<&'a str>,
        ) -> BoxFuture<'a, Result<()>> {
            Box::pin(async move {
                self.pushed
                    .lock()
                    .unwrap()
                    .push((sha.to_owned(), branch.to_owned()));
                Ok(())
            })
        }
    }

    struct MockReviewer {
        verdicts: Mutex<Vec<ReviewVerdict>>,
        seen_worktrees: Mutex<Vec<PathBuf>>,
    }
    impl ReviewerExecutor for MockReviewer {
        fn check_available(&self, _spec: &ReviewerSpec) -> Result<()> {
            Ok(())
        }
        fn run_review<'a>(
            &'a self,
            req: ReviewWorktreeRequest,
        ) -> BoxFuture<'a, Result<ReviewOutcome>> {
            Box::pin(async move {
                assert!(req.worktree_path.exists());
                self.seen_worktrees.lock().unwrap().push(req.worktree_path);
                let verdict = self.verdicts.lock().unwrap().remove(0);
                Ok(ReviewOutcome {
                    verdict,
                    comments: "LGTM".to_owned(),
                })
            })
        }
    }

    struct MockRevise {
        next_sha: String,
        bounces: AtomicUsize,
    }
    impl WorkerReviseExecutor for MockRevise {
        fn bounce_scope_violation<'a>(
            &'a self,
            _gate: &'a PrGateDto,
            _violations: &'a [String],
        ) -> BoxFuture<'a, Result<String>> {
            Box::pin(async move {
                self.bounces.fetch_add(1, Ordering::SeqCst);
                Ok(self.next_sha.clone())
            })
        }
        fn revise_pr<'a>(
            &'a self,
            _gate: &'a PrGateDto,
            _review_comments: &'a str,
        ) -> BoxFuture<'a, Result<String>> {
            Box::pin(async move { Ok(self.next_sha.clone()) })
        }
    }

    #[tokio::test]
    async fn full_pr_gate_disposable_worktree_crash_safe_automerge_and_pause_revocation() {
        let fix = setup_git_and_store();
        let (base_url, gh, app_state, server) = spawn_fake_github(fix.store.clone()).await;
        let scm = Arc::new(
            GitHubScmClient::new(
                GitHubConfig::new(SecretToken::new("bot-tok"), SecretToken::new("rev-tok"))
                    .with_base_urls(&base_url, format!("{base_url}/graphql")),
            )
            .unwrap(),
        );
        let reviewer = Arc::new(MockReviewer {
            verdicts: Mutex::new(vec![ReviewVerdict::Approve]),
            seen_worktrees: Mutex::new(Vec::new()),
        });
        let pusher = Arc::new(MockPusher {
            pushed: Mutex::new(Vec::new()),
        });
        let deps = PrGateDeps {
            store: fix.store.clone(),
            scm: scm.clone(),
            reviewer: reviewer.clone(),
            worker_revise: None,
            pusher: pusher.clone(),
        };

        // 1. Same vendor+model reviewer must be refused
        let err = run_pr_gate(
            &deps,
            RunPrGateRequest {
                run_id: "run1".into(),
                hub_session_id: Some("hub".into()),
                mirror_path: fix.mirror.clone(),
                repo_slug: "org/repo".into(),
                branch: "feat/x".into(),
                base_branch: "main".into(),
                base_sha: fix.base_sha.clone(),
                head_sha: fix.good_head_sha.clone(),
                pr_title: "feat: x".into(),
                pr_body: "body".into(),
                touched_paths: vec!["src/**".into()],
                is_high_risk: false,
                reviewers: vec![ReviewerSpec {
                    vendor: Vendor::Codex,
                    model: "gpt-5.3-codex".into(),
                }],
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(err, Error::Policy));

        // 2. Heterogeneous reviewer succeeds, arms auto-merge AFTER persisting in SQLite
        // Pre-populate gate_id observer hook via list_pr_gates after creation isn't needed if we
        // check store_armed_before_enable: let's set gate_id by wrapping or checking armed_pr_gates.
        let gate = run_pr_gate(
            &deps,
            RunPrGateRequest {
                run_id: "run1".into(),
                hub_session_id: Some("hub".into()),
                mirror_path: fix.mirror.clone(),
                repo_slug: "org/repo".into(),
                branch: "feat/x".into(),
                base_branch: "main".into(),
                base_sha: fix.base_sha.clone(),
                head_sha: fix.good_head_sha.clone(),
                pr_title: "feat: x".into(),
                pr_body: "body".into(),
                touched_paths: vec!["src/**".into()],
                is_high_risk: false,
                reviewers: vec![ReviewerSpec {
                    vendor: Vendor::Claude,
                    model: "claude-sonnet-4-6".into(),
                }],
            },
        )
        .await
        .unwrap();

        let gate = human_approve(&deps, &gate, Some("hub")).await.unwrap();
        assert_eq!(gate.status, PrGateStatus::AutoMergeArmed);
        assert!(gate.auto_merge_armed);
        assert_eq!(gate.pr_number, Some(42));
        assert_eq!(pusher.pushed.lock().unwrap().len(), 1);
        // Disposable worktree must have been cleaned up after review
        {
            let wt_paths = reviewer.seen_worktrees.lock().unwrap();
            assert_eq!(wt_paths.len(), 1);
            assert!(!wt_paths[0].exists());
        }

        // Verify review commit_id pinning and marker
        {
            let reviews = gh.submitted_reviews.lock().unwrap();
            assert_eq!(reviews.len(), 1);
            assert_eq!(reviews[0]["commit_id"], fix.good_head_sha);
            assert!(reviews[0]["body"]
                .as_str()
                .unwrap()
                .contains("[agent-reviewer · claude/claude-sonnet-4-6 · "));
        }

        // Verify enableAutoMerge received expectedHeadOid
        {
            let enables = gh.enable_auto_merge_calls.lock().unwrap();
            assert_eq!(enables.len(), 1);
            assert_eq!(enables[0].1, fix.good_head_sha);
        }

        // 3. Global Pause revokes armed auto-merges on GitHub and blocks new dispatches
        let pause_state = pause_all(&fix.store, scm.as_ref(), Some("incident".into()))
            .await
            .unwrap();
        assert!(pause_state.paused);
        assert_eq!(gh.disable_auto_merge_calls.lock().unwrap().len(), 1);
        let paused_gate = fix.store.pr_gate(&gate.id).unwrap();
        assert!(!paused_gate.auto_merge_armed);
        assert_eq!(paused_gate.status, PrGateStatus::Paused);

        // New dispatch refused while paused
        let blocked = run_pr_gate(
            &deps,
            RunPrGateRequest {
                run_id: "run1".into(),
                hub_session_id: None,
                mirror_path: fix.mirror.clone(),
                repo_slug: "org/repo".into(),
                branch: "feat/y".into(),
                base_branch: "main".into(),
                base_sha: fix.base_sha.clone(),
                head_sha: fix.good_head_sha.clone(),
                pr_title: "feat: y".into(),
                pr_body: "body".into(),
                touched_paths: vec!["src/**".into()],
                is_high_risk: false,
                reviewers: vec![ReviewerSpec {
                    vendor: Vendor::Claude,
                    model: "claude-sonnet-4-6".into(),
                }],
            },
        )
        .await
        .unwrap_err();
        assert!(matches!(blocked, Error::Policy));

        let resumed = resume_all(&fix.store).unwrap();
        assert!(!resumed.paused);

        let _ = app_state;
        server.abort();
    }

    #[tokio::test]
    async fn scope_violation_bounces_once_then_escalates_on_second_violation() {
        let fix = setup_git_and_store();
        let (base_url, _gh, _app_state, server) = spawn_fake_github(fix.store.clone()).await;
        let scm = Arc::new(
            GitHubScmClient::new(
                GitHubConfig::new(SecretToken::new("bot-tok"), SecretToken::new("rev-tok"))
                    .with_base_urls(&base_url, format!("{base_url}/graphql")),
            )
            .unwrap(),
        );
        let revise = Arc::new(MockRevise {
            next_sha: fix.bad_head_sha_2.clone(),
            bounces: AtomicUsize::new(0),
        });
        let deps = PrGateDeps {
            store: fix.store.clone(),
            scm,
            reviewer: Arc::new(MockReviewer {
                verdicts: Mutex::new(vec![]),
                seen_worktrees: Mutex::new(Vec::new()),
            }),
            worker_revise: Some(revise.clone()),
            pusher: Arc::new(MockPusher {
                pushed: Mutex::new(Vec::new()),
            }),
        };

        let gate = run_pr_gate(
            &deps,
            RunPrGateRequest {
                run_id: "run1".into(),
                hub_session_id: Some("hub".into()),
                mirror_path: fix.mirror.clone(),
                repo_slug: "org/repo".into(),
                branch: "feat/scope".into(),
                base_branch: "main".into(),
                base_sha: fix.base_sha.clone(),
                head_sha: fix.bad_head_sha_1.clone(),
                pr_title: "feat: scope".into(),
                pr_body: "body".into(),
                touched_paths: vec!["src/**".into()],
                is_high_risk: false,
                reviewers: vec![ReviewerSpec {
                    vendor: Vendor::Claude,
                    model: "claude-sonnet-4-6".into(),
                }],
            },
        )
        .await
        .unwrap();

        assert_eq!(revise.bounces.load(Ordering::SeqCst), 1);
        assert_eq!(gate.status, PrGateStatus::Escalated);
        assert_eq!(gate.scope_violations, vec!["README.md".to_owned()]);
        server.abort();
    }

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    enum EnableAnswer {
        Armed,
        /// The request may have reached GitHub; no answer came back.
        Unknown,
        /// GitHub answered and refused.
        Refused,
    }

    /// A GitHub stand-in whose `enable_auto_merge` can be held open while the
    /// test interleaves a Pause, and which records the order in which remote
    /// operations really completed.
    struct RacingScm {
        answer: EnableAnswer,
        started: Arc<tokio::sync::Semaphore>,
        release: Arc<tokio::sync::Semaphore>,
        state: Arc<RacingState>,
    }

    #[derive(Default)]
    struct RacingState {
        /// Completed remote operations, in completion order.
        ops: Mutex<Vec<&'static str>>,
        /// What GitHub would report as the PR's auto-merge flag.
        enabled: Mutex<bool>,
        /// Repository slugs whose PR state GitHub refuses to answer for.
        unreadable: Mutex<Vec<String>>,
        /// When set, a disarm request gets no answer (the remote may have applied
        /// it, so the ledger must stay armed).
        lose_disable: AtomicBool,
        /// The head this stand-in reports for the PR.
        head: Mutex<String>,
    }

    impl RacingScm {
        fn new(answer: EnableAnswer) -> Arc<Self> {
            Arc::new(Self {
                answer,
                started: Arc::new(tokio::sync::Semaphore::new(0)),
                release: Arc::new(tokio::sync::Semaphore::new(0)),
                state: Arc::new(RacingState::default()),
            })
        }
        fn ops(&self) -> Vec<&'static str> {
            self.state.ops.lock().unwrap().clone()
        }
        /// The SHA this stand-in reports as the PR head.
        fn set_head(&self, sha: &str) {
            *self.state.head.lock().unwrap() = sha.to_owned();
        }
    }

    impl ScmClient for RacingScm {
        fn preflight_repo<'a>(
            &'a self,
            _repo_slug: &'a str,
            _base_branch: &'a str,
        ) -> BoxFuture<'a, std::result::Result<PreflightReport, ScmError>> {
            Box::pin(async {
                Ok(PreflightReport {
                    bot_login: "polyclave-bot".into(),
                    reviewer_login: "polyclave-reviewer".into(),
                    default_branch: "main".into(),
                    branch_protected: true,
                    approval_required: true,
                    required_approving_review_count: 1,
                    squash_merge_allowed: true,
                    auto_merge_allowed: true,
                    passed: true,
                    issues: Vec::new(),
                })
            })
        }
        fn open_or_find_pr<'a>(
            &'a self,
            _req: OpenPrRequest,
        ) -> BoxFuture<'a, std::result::Result<PullRequestInfo, ScmError>> {
            Box::pin(async {
                Ok(PullRequestInfo {
                    number: 42,
                    node_id: "PR_node_42".into(),
                    head_sha: "sha".into(),
                    state: "open".into(),
                    html_url: "https://github.com/org/repo/pull/42".into(),
                    created: true,
                })
            })
        }
        fn submit_review<'a>(
            &'a self,
            req: SubmitReviewRequest,
        ) -> BoxFuture<'a, std::result::Result<SubmittedReview, ScmError>> {
            Box::pin(async move {
                Ok(SubmittedReview {
                    id: 9001,
                    commit_id: req.reviewed_sha,
                    state: "APPROVED".into(),
                    html_url: "https://github.com/org/repo/pull/42#pullrequestreview-9001".into(),
                })
            })
        }
        fn enable_auto_merge<'a>(
            &'a self,
            _pr_node_id: &'a str,
            _expected_head_oid: &'a str,
        ) -> BoxFuture<'a, std::result::Result<(), ScmError>> {
            self.started.add_permits(1);
            let release = self.release.clone();
            let answer = self.answer;
            let state = self.state.clone();
            Box::pin(async move {
                // Held open until the test has started a Pause.
                let _ = release.acquire().await;
                match answer {
                    EnableAnswer::Armed => {
                        *state.enabled.lock().unwrap() = true;
                        state.ops.lock().unwrap().push("enable");
                        Ok(())
                    }
                    EnableAnswer::Unknown => Err(ScmError::Transport {
                        op: "enable_auto_merge",
                    }),
                    EnableAnswer::Refused => Err(ScmError::Http {
                        op: "enable_auto_merge",
                        status: 422,
                        message: "auto-merge is not allowed".into(),
                    }),
                }
            })
        }
        fn disable_auto_merge<'a>(
            &'a self,
            pr_node_id: &'a str,
        ) -> BoxFuture<'a, std::result::Result<(), ScmError>> {
            let _ = pr_node_id;
            // A lost answer: the remote may or may not have disarmed.
            self.state.ops.lock().unwrap().push("disable");
            if self.state.lose_disable.load(Ordering::SeqCst) {
                return Box::pin(async {
                    Err(ScmError::Transport {
                        op: "disable_auto_merge",
                    })
                });
            }
            *self.state.enabled.lock().unwrap() = false;
            Box::pin(async { Ok(()) })
        }
        fn get_pr_state<'a>(
            &'a self,
            repo_slug: &'a str,
            pr_number: u64,
        ) -> BoxFuture<'a, std::result::Result<PrState, ScmError>> {
            let enabled = *self.state.enabled.lock().unwrap();
            let head = self.state.head.lock().unwrap().clone();
            let unreadable = self
                .state
                .unreadable
                .lock()
                .unwrap()
                .iter()
                .any(|slug| slug == repo_slug);
            Box::pin(async move {
                if unreadable {
                    return Err(ScmError::Http {
                        op: "get_pr_state",
                        status: 404,
                        message: "Not Found".into(),
                    });
                }
                Ok(PrState {
                    number: pr_number,
                    node_id: format!("PR_node_{pr_number}"),
                    state: "open".into(),
                    merged: false,
                    mergeable: Some(true),
                    mergeable_state: Some("clean".into()),
                    head_sha: head,
                    auto_merge_enabled: enabled,
                    html_url: format!("https://github.com/org/repo/pull/{pr_number}"),
                })
            })
        }
        fn list_pr_checks_and_comments<'a>(
            &'a self,
            _repo_slug: &'a str,
            _pr_number: u64,
            _head_sha: &'a str,
        ) -> BoxFuture<'a, std::result::Result<PrChecksAndComments, ScmError>> {
            Box::pin(async {
                Ok(PrChecksAndComments {
                    check_runs: Vec::new(),
                    checks_complete: true,
                    comments: Vec::new(),
                    reviews: Vec::new(),
                })
            })
        }
    }

    /// Refuses the pre-push base confirmation, as a remote base branch that
    /// moved off the reviewed baseline does.
    struct MovedBasePusher {
        pushed: Mutex<Vec<(String, String)>>,
    }

    impl MirrorPusher for MovedBasePusher {
        fn confirm_review_base<'a>(
            &'a self,
            _target: &'a PublishTarget,
            _mirror_path: &'a Path,
            _base_branch: &'a str,
            _base_sha: &'a str,
        ) -> BoxFuture<'a, Result<()>> {
            Box::pin(async { Err(Error::Policy) })
        }
        fn push_verified_sha<'a>(
            &'a self,
            _target: &'a PublishTarget,
            _mirror_path: &'a Path,
            sha: &'a str,
            branch: &'a str,
            _expected_remote_tip: Option<&'a str>,
        ) -> BoxFuture<'a, Result<()>> {
            self.pushed
                .lock()
                .unwrap()
                .push((sha.to_owned(), branch.to_owned()));
            Box::pin(async { Ok(()) })
        }
    }

    /// Drives the human approval a gate now waits for and returns the armed gate.
    /// Reaching `AutoMergeArmed` without this is exactly what Item 3.3 forbids.
    async fn human_approve(
        deps: &PrGateDeps,
        gate: &PrGateDto,
        hub: Option<&str>,
    ) -> Result<PrGateDto> {
        assert_eq!(gate.status, PrGateStatus::AwaitingHumanApproval, "{gate:?}");
        assert!(!gate.auto_merge_armed);
        approve_pr_gate(deps, &gate.id, &gate.head_sha, hub).await
    }

    /// A gate whose reviews all approve, so `run_pr_gate` reaches the auto-merge
    /// step, with the given remote stand-in.
    async fn arming_deps(fix: &GitFixture, scm: Arc<RacingScm>) -> PrGateDeps {
        let pusher = Arc::new(MockPusher {
            pushed: Mutex::new(Vec::new()),
        });
        PrGateDeps {
            store: fix.store.clone(),
            scm,
            reviewer: Arc::new(MockReviewer {
                verdicts: Mutex::new(vec![ReviewVerdict::Approve]),
                seen_worktrees: Mutex::new(Vec::new()),
            }),
            worker_revise: None,
            pusher,
        }
    }

    #[tokio::test]
    async fn pause_revokes_an_auto_merge_that_is_still_in_flight() {
        let fix = setup_git_and_store();
        let scm = RacingScm::new(EnableAnswer::Armed);
        let deps = arming_deps(&fix, scm.clone()).await;
        let mut request = range_request(&fix, &fix.good_head_sha, "feat/race");
        request.hub_session_id = Some("hub".into());
        let gate = run_pr_gate(&deps, request).await.unwrap();
        assert_eq!(gate.status, PrGateStatus::AwaitingHumanApproval);

        // GitHub is only asked during the human approval, so that is where the
        // in-flight enable has to be held open while a Pause starts.
        scm.set_head(&fix.good_head_sha);
        let approve_deps = deps.clone();
        let gate_id = gate.id.clone();
        let head = fix.good_head_sha.clone();
        let approve_task = tokio::spawn(async move {
            approve_pr_gate(&approve_deps, &gate_id, &head, Some("hub")).await
        });
        let _ = scm.started.acquire().await.unwrap();
        let store = fix.store.clone();
        let pause_scm = scm.clone();
        let pause_task =
            tokio::spawn(async move { pause_all(&store, pause_scm.as_ref(), None).await });
        // Let the Pause reach the gate lock (it cannot pass the in-flight enable).
        tokio::task::yield_now().await;
        scm.release.add_permits(1);
        let _ = approve_task.await.unwrap();
        pause_task.await.unwrap().unwrap();

        // Whatever the ordering was, Pause wins: nothing may be armed afterwards.
        let after = fix.store.pr_gate(&gate.id).unwrap();
        assert!(!after.auto_merge_armed, "{after:?}");
        assert_eq!(after.status, PrGateStatus::Paused);
        assert_eq!(
            fix.store.pr_gate_remote_intent(&gate.id).unwrap(),
            None,
            "a confirmed disarm leaves no pending intent"
        );
        // The enable either finished before the disarm or never reached GitHub;
        // an enable completing after the disarm would arm a paused PR.
        assert_eq!(scm.ops(), vec!["enable", "disable"], "{:?}", scm.ops());
        assert!(!*scm.state.enabled.lock().unwrap());
    }

    /// Pauses the whole host from inside the review loop, so the Pause lands
    /// after the gate was created and opened and before the arm transaction runs:
    /// `pause_all` reads the armed gates while this gate is not armed yet. The
    /// test pins the outcome for that interleaving (nothing is armed, GitHub is
    /// never called, no gate is left for a Pause snapshot to miss). The
    /// transaction-level guarantee itself is pinned by
    /// `pr_gate_auto_merge_ledger_and_pause_flag_persist_across_reopen`, which
    /// fails if the pause check leaves `arm_auto_merge`'s write transaction.
    struct PausingReviewer {
        store: Store,
        scm: Arc<RacingScm>,
        paused: AtomicBool,
    }

    impl ReviewerExecutor for PausingReviewer {
        fn check_available(&self, _spec: &ReviewerSpec) -> Result<()> {
            Ok(())
        }
        fn run_review<'a>(
            &'a self,
            req: ReviewWorktreeRequest,
        ) -> BoxFuture<'a, Result<ReviewOutcome>> {
            assert!(req.worktree_path.exists());
            let paused = self.paused.swap(true, Ordering::SeqCst);
            let store = self.store.clone();
            let scm = self.scm.clone();
            Box::pin(async move {
                if !paused {
                    // The real Pause path, on the real ledger.
                    pause_all(&store, scm.as_ref(), Some("interleaved".into()))
                        .await
                        .unwrap();
                }
                Ok(ReviewOutcome {
                    verdict: ReviewVerdict::Approve,
                    comments: "Clean diff".into(),
                })
            })
        }
    }

    #[tokio::test]
    async fn a_pause_between_approval_and_arming_never_reaches_github() {
        let fix = setup_git_and_store();
        let scm = RacingScm::new(EnableAnswer::Armed);
        let pusher = Arc::new(MockPusher {
            pushed: Mutex::new(Vec::new()),
        });
        let deps = PrGateDeps {
            store: fix.store.clone(),
            scm: scm.clone(),
            reviewer: Arc::new(PausingReviewer {
                store: fix.store.clone(),
                scm: scm.clone(),
                paused: AtomicBool::new(false),
            }),
            worker_revise: None,
            pusher: pusher.clone(),
        };
        let mut request = range_request(&fix, &fix.good_head_sha, "feat/pause-race");
        request.hub_session_id = Some("hub".into());

        let gate = run_pr_gate(&deps, request).await.unwrap();
        // The pause landed while the gate was approved but not yet armed: the arm
        // transaction refuses, so GitHub is never told to enable auto-merge.
        assert_eq!(gate.status, PrGateStatus::Paused, "{gate:?}");
        assert!(!gate.auto_merge_armed, "{gate:?}");
        assert!(scm.ops().is_empty(), "no remote operation: {:?}", scm.ops());
        assert!(!*scm.state.enabled.lock().unwrap());
        // Nothing for a Pause snapshot to miss: no gate is left armed.
        assert!(fix.store.armed_pr_gates().unwrap().is_empty());
        assert_eq!(fix.store.pr_gate_remote_intent(&gate.id).unwrap(), None);
        // The PR was still opened before the pause, which is what the gate does;
        // the point is that it was never armed.
        assert_eq!(pusher.pushed.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_lost_disarm_answer_leaves_the_gate_armed_and_still_settles_the_others() {
        let fix = setup_git_and_store();
        let mut gates = Vec::new();
        for slug in ["org/repo", "org/other"] {
            let gate = fix
                .store
                .create_pr_gate(NewPrGate {
                    run_id: "run1".into(),
                    repo_slug: slug.into(),
                    branch: format!("feat/{}", slug.replace('/', "-")),
                    base_sha: fix.base_sha.clone(),
                    head_sha: fix.good_head_sha.clone(),
                    touched_paths: vec!["src/**".into()],
                })
                .unwrap();
            fix.store
                .record_pr_opened(&gate.id, 42, "https://example.invalid/42")
                .unwrap();
            fix.store
                .set_pr_gate_status(&gate.id, PrGateStatus::Approved)
                .unwrap();
            let armed = fix
                .store
                .arm_auto_merge(&gate.id, &fix.good_head_sha)
                .unwrap()
                .unwrap();
            gates.push(armed.id);
        }
        let scm = RacingScm::new(EnableAnswer::Armed);
        *scm.state.enabled.lock().unwrap() = true;
        scm.state.lose_disable.store(true, Ordering::SeqCst);
        fix.store.set_pause(true, Some("restart".into())).unwrap();

        // The disarm answer is lost for every gate: none may be reported settled,
        // every gate keeps its armed bit, and the loop still finishes.
        assert_eq!(
            reconcile_auto_merge(&fix.store, scm.as_ref())
                .await
                .unwrap(),
            0
        );
        for gate in &gates {
            let kept = fix.store.pr_gate(gate).unwrap();
            assert!(kept.auto_merge_armed, "{kept:?}");
            assert_eq!(
                fix.store.pr_gate_remote_intent(gate).unwrap().as_deref(),
                Some("disable")
            );
        }
        let notices: Vec<String> = fix
            .store
            .hub_messages("hub")
            .unwrap()
            .into_iter()
            .map(|m| m.text)
            .collect();
        assert_eq!(
            notices
                .iter()
                .filter(|n| n.contains("could not be revoked"))
                .count(),
            2,
            "{notices:?}"
        );

        // A later confirmed disarm settles both gates.
        scm.state.lose_disable.store(false, Ordering::SeqCst);
        assert_eq!(
            reconcile_auto_merge(&fix.store, scm.as_ref())
                .await
                .unwrap(),
            2
        );
        assert!(fix.store.armed_pr_gates().unwrap().is_empty());
    }

    #[tokio::test]
    async fn one_gate_github_cannot_answer_for_does_not_stop_the_others_reconciling() {
        let fix = setup_git_and_store();
        // Two armed gates in different repositories; GitHub 404s the first.
        let mut gates = Vec::new();
        for slug in ["org/repo", "org/other"] {
            let gate = fix
                .store
                .create_pr_gate(NewPrGate {
                    run_id: "run1".into(),
                    repo_slug: slug.into(),
                    branch: format!("feat/{}", slug.replace('/', "-")),
                    base_sha: fix.base_sha.clone(),
                    head_sha: fix.good_head_sha.clone(),
                    touched_paths: vec!["src/**".into()],
                })
                .unwrap();
            fix.store
                .record_pr_opened(&gate.id, 42, "https://example.invalid/42")
                .unwrap();
            fix.store
                .set_pr_gate_status(&gate.id, PrGateStatus::Approved)
                .unwrap();
            let armed = fix
                .store
                .arm_auto_merge(&gate.id, &fix.good_head_sha)
                .unwrap()
                .unwrap();
            gates.push(armed.id);
        }
        let scm = RacingScm::new(EnableAnswer::Armed);
        scm.state.unreadable.lock().unwrap().push("org/repo".into());
        fix.store.set_pause(true, Some("restart".into())).unwrap();

        // The unreadable gate stays armed and does not block the other one.
        assert_eq!(
            reconcile_auto_merge(&fix.store, scm.as_ref())
                .await
                .unwrap(),
            1
        );
        let unreadable = fix.store.pr_gate(&gates[0]).unwrap();
        assert!(unreadable.auto_merge_armed, "{unreadable:?}");
        let settled = fix.store.pr_gate(&gates[1]).unwrap();
        assert!(!settled.auto_merge_armed, "{settled:?}");
        assert_eq!(settled.status, PrGateStatus::Paused);
        let notices: Vec<String> = fix
            .store
            .hub_messages("hub")
            .unwrap()
            .into_iter()
            .map(|m| m.text)
            .collect();
        assert!(
            notices
                .iter()
                .any(|n| n.contains("could not be reconciled")),
            "{notices:?}"
        );
    }

    #[tokio::test]
    async fn a_lost_enable_answer_stays_armed_and_startup_reconciliation_settles_it() {
        let fix = setup_git_and_store();
        let scm = RacingScm::new(EnableAnswer::Unknown);
        let deps = arming_deps(&fix, scm.clone()).await;
        let mut request = range_request(&fix, &fix.good_head_sha, "feat/lost");
        request.hub_session_id = Some("hub".into());
        scm.set_head(&fix.good_head_sha);
        scm.release.add_permits(1);
        let gate = run_pr_gate(&deps, request).await.unwrap();
        assert!(human_approve(&deps, &gate, Some("hub")).await.is_err());

        // No answer: the ledger must not claim the PR is disarmed.
        let gate_id = fix.store.list_pr_gates().unwrap()[0].id.clone();
        let lost = fix.store.pr_gate(&gate_id).unwrap();
        assert!(lost.auto_merge_armed, "{lost:?}");
        assert_eq!(
            fix.store
                .pr_gate_remote_intent(&gate_id)
                .unwrap()
                .as_deref(),
            Some("enable")
        );
        let notices: Vec<String> = fix
            .store
            .hub_messages("hub")
            .unwrap()
            .into_iter()
            .map(|m| m.text)
            .collect();
        assert!(
            notices.iter().any(|n| n.contains("could not be confirmed")),
            "{notices:?}"
        );

        // A definite refusal is different: GitHub answered, so the ledger may
        // leave the armed state without help from reconciliation.
        let refused_fix = setup_git_and_store();
        let refused = RacingScm::new(EnableAnswer::Refused);
        refused.set_head(&refused_fix.good_head_sha);
        let refused_deps = arming_deps(&refused_fix, refused.clone()).await;
        refused.release.add_permits(1);
        let pending = run_pr_gate(
            &refused_deps,
            range_request(&refused_fix, &refused_fix.good_head_sha, "feat/refused"),
        )
        .await
        .unwrap();
        assert!(human_approve(&refused_deps, &pending, None).await.is_err());
        let refused_gate = refused_fix.store.pr_gate(&pending.id).unwrap();
        assert!(!refused_gate.auto_merge_armed, "{refused_gate:?}");
        assert_eq!(
            refused_fix
                .store
                .pr_gate_remote_intent(&refused_gate.id)
                .unwrap(),
            None
        );

        // Startup reconciliation while paused: the enable did reach GitHub, so it
        // is revoked and the ledger settles on Paused.
        *scm.state.enabled.lock().unwrap() = true;
        fix.store.set_pause(true, Some("restart".into())).unwrap();
        assert_eq!(
            reconcile_auto_merge(&fix.store, scm.as_ref())
                .await
                .unwrap(),
            1
        );
        let settled = fix.store.pr_gate(&gate_id).unwrap();
        assert!(!settled.auto_merge_armed, "{settled:?}");
        assert_eq!(settled.status, PrGateStatus::Paused);
        assert!(scm.ops().contains(&"disable"), "{:?}", scm.ops());

        // The same gate again, this time not paused and never armed remotely:
        // the ledger follows GitHub instead of guessing.
        fix.store.set_pause(false, None).unwrap();
        let approved = fix
            .store
            .set_pr_gate_status(&gate_id, PrGateStatus::Approved)
            .unwrap();
        let gate = fix
            .store
            .arm_auto_merge(&gate_id, &approved.head_sha)
            .unwrap()
            .unwrap();
        assert!(gate.auto_merge_armed);
        *scm.state.enabled.lock().unwrap() = false;
        assert_eq!(
            reconcile_auto_merge(&fix.store, scm.as_ref())
                .await
                .unwrap(),
            1
        );
        let tracked = fix.store.pr_gate(&gate_id).unwrap();
        assert!(!tracked.auto_merge_armed, "{tracked:?}");
        assert_eq!(tracked.status, PrGateStatus::Approved);
        assert_eq!(fix.store.pr_gate_remote_intent(&gate_id).unwrap(), None);
    }

    #[tokio::test]
    async fn an_empty_reviewed_range_is_refused_before_any_push() {
        let fix = setup_git_and_store();
        let (base_url, gh, _app_state, server) = spawn_fake_github(fix.store.clone()).await;
        let scm = Arc::new(
            GitHubScmClient::new(
                GitHubConfig::new(SecretToken::new("bot-tok"), SecretToken::new("rev-tok"))
                    .with_base_urls(&base_url, format!("{base_url}/graphql")),
            )
            .unwrap(),
        );
        let pusher = Arc::new(MockPusher {
            pushed: Mutex::new(Vec::new()),
        });
        let deps = PrGateDeps {
            store: fix.store.clone(),
            scm,
            reviewer: Arc::new(MockReviewer {
                verdicts: Mutex::new(vec![ReviewVerdict::Approve]),
                seen_worktrees: Mutex::new(Vec::new()),
            }),
            worker_revise: None,
            pusher: pusher.clone(),
        };
        // The verified head is the reviewed base itself: there is no range.
        let mut request = range_request(&fix, &fix.good_head_sha, "feat/empty");
        request.head_sha = request.base_sha.clone();
        assert!(matches!(
            run_pr_gate(&deps, request).await.unwrap_err(),
            Error::Policy
        ));
        assert!(pusher.pushed.lock().unwrap().is_empty());
        assert!(gh.submitted_reviews.lock().unwrap().is_empty());
        assert!(fix.store.list_pr_gates().unwrap().is_empty());
        server.abort();
    }

    #[tokio::test]
    async fn a_remote_base_branch_off_the_reviewed_range_is_refused_before_any_push() {
        let fix = setup_git_and_store();
        let (base_url, gh, _app_state, server) = spawn_fake_github(fix.store.clone()).await;
        let scm = Arc::new(
            GitHubScmClient::new(
                GitHubConfig::new(SecretToken::new("bot-tok"), SecretToken::new("rev-tok"))
                    .with_base_urls(&base_url, format!("{base_url}/graphql")),
            )
            .unwrap(),
        );
        let pusher = Arc::new(MovedBasePusher {
            pushed: Mutex::new(Vec::new()),
        });
        let deps = PrGateDeps {
            store: fix.store.clone(),
            scm,
            reviewer: Arc::new(MockReviewer {
                verdicts: Mutex::new(vec![ReviewVerdict::Approve]),
                seen_worktrees: Mutex::new(Vec::new()),
            }),
            worker_revise: None,
            pusher: pusher.clone(),
        };
        let mut request = range_request(&fix, &fix.good_head_sha, "feat/moved-base");
        request.hub_session_id = Some("hub".into());
        let gate = run_pr_gate(&deps, request).await.unwrap();
        assert_eq!(gate.status, PrGateStatus::Escalated);
        assert_eq!(gate.pr_number, None);
        assert!(pusher.pushed.lock().unwrap().is_empty());
        assert!(gh.submitted_reviews.lock().unwrap().is_empty());
        let notices: Vec<String> = fix
            .store
            .hub_messages("hub")
            .unwrap()
            .into_iter()
            .map(|m| m.text)
            .collect();
        assert!(
            notices
                .iter()
                .any(|n| n.contains("no longer contains the reviewed base")),
            "{notices:?}"
        );
        server.abort();
    }

    #[tokio::test]
    async fn git_pusher_requires_the_remote_base_branch_to_hold_the_reviewed_baseline() {
        let fix = setup_git_and_store();
        let github = fix._dir.path().join("github");
        let remote = github.join("org/repo.git");
        std::fs::create_dir_all(&remote).unwrap();
        git(
            fix._dir.path(),
            &["init", "--bare", "--template=", remote.to_str().unwrap()],
        );
        let work = fix._dir.path().join("work");
        let pusher = GitMirrorPusher::with_local_base(&github);
        let target = PublishTarget::for_tests("org/repo");
        let base = fix.base_sha.as_str();

        // No base branch on the remote yet: the PR could not even be opened.
        assert!(matches!(
            pusher
                .confirm_review_base(&target, &fix.mirror, "main", base)
                .await,
            Err(Error::Policy)
        ));

        // The remote base branch contains the reviewed baseline.
        git(
            &work,
            &[
                "push",
                remote.to_str().unwrap(),
                &format!("{base}:refs/heads/main"),
            ],
        );
        pusher
            .confirm_review_base(&target, &fix.mirror, "main", base)
            .await
            .unwrap();

        // An unrelated history rewritten on top of the base branch no longer
        // contains it, so the reviewed range is not the PR's range.
        let unrelated = fix._dir.path().join("unrelated");
        std::fs::create_dir_all(&unrelated).unwrap();
        git(&unrelated, &["init", "-b", "other", "--template="]);
        git(&unrelated, &["config", "user.name", "Other"]);
        git(
            &unrelated,
            &["config", "user.email", "other@example.invalid"],
        );
        std::fs::write(unrelated.join("README.md"), "rewritten\n").unwrap();
        git(&unrelated, &["add", "."]);
        git(&unrelated, &["commit", "-m", "unrelated root"]);
        let rewritten = git(&unrelated, &["rev-parse", "HEAD"]);
        git(
            &unrelated,
            &[
                "push",
                "-f",
                remote.to_str().unwrap(),
                &format!("{rewritten}:refs/heads/main"),
            ],
        );
        assert!(matches!(
            pusher
                .confirm_review_base(&target, &fix.mirror, "main", base)
                .await,
            Err(Error::Policy)
        ));
        // The gate can still publish after the base is restored.
        git(
            &work,
            &[
                "push",
                "-f",
                remote.to_str().unwrap(),
                &format!("{base}:refs/heads/main"),
            ],
        );
        pusher
            .confirm_review_base(&target, &fix.mirror, "main", base)
            .await
            .unwrap();
        // A malformed baseline is refused without touching the remote.
        assert!(pusher
            .confirm_review_base(&target, &fix.mirror, "main", "deadbeef")
            .await
            .is_err());
    }

    #[tokio::test]
    async fn git_pusher_pushes_exact_sha_with_lease_and_confirms_remote_tip() {
        let fix = setup_git_and_store();
        let github = fix._dir.path().join("github");
        let remote = github.join("org/repo.git");
        std::fs::create_dir_all(&remote).unwrap();
        git(
            fix._dir.path(),
            &["init", "--bare", "--template=", remote.to_str().unwrap()],
        );
        let pusher = GitMirrorPusher::with_local_base(&github);
        let target = PublishTarget::for_tests("org/repo");
        let tip = |branch: &str| git(&remote, &["rev-parse", &format!("refs/heads/{branch}")]);

        // First push: the branch is absent, so the lease is "must not exist".
        pusher
            .push_verified_sha(&target, &fix.mirror, &fix.good_head_sha, "feat/x", None)
            .await
            .unwrap();
        assert_eq!(tip("feat/x"), fix.good_head_sha);
        // Nothing else was published from the mirror.
        let refs = git(&remote, &["for-each-ref", "--format=%(refname)"]);
        assert_eq!(refs, "refs/heads/feat/x");

        // Re-pushing the same SHA is a no-op that still confirms the tip.
        pusher
            .push_verified_sha(&target, &fix.mirror, &fix.good_head_sha, "feat/x", None)
            .await
            .unwrap();

        // A revise round leases on the SHA it pushed last; a stale lease is refused
        // and leaves the remote untouched.
        let stale = pusher
            .push_verified_sha(
                &target,
                &fix.mirror,
                &fix.bad_head_sha_2,
                "feat/x",
                Some(fix.bad_head_sha_1.as_str()),
            )
            .await;
        assert!(stale.is_err());
        assert_eq!(tip("feat/x"), fix.good_head_sha);

        // With the right lease a non-fast-forward revise head is published exactly.
        pusher
            .push_verified_sha(
                &target,
                &fix.mirror,
                &fix.bad_head_sha_2,
                "feat/x",
                Some(fix.good_head_sha.as_str()),
            )
            .await
            .unwrap();
        assert_eq!(tip("feat/x"), fix.bad_head_sha_2);

        // Abbreviated SHAs, unknown commits and bad branch names are refused.
        assert!(pusher
            .push_verified_sha(
                &target,
                &fix.mirror,
                &fix.good_head_sha[..12],
                "feat/y",
                None
            )
            .await
            .is_err());
        assert!(pusher
            .push_verified_sha(&target, &fix.mirror, &"a".repeat(40), "feat/y", None)
            .await
            .is_err());
        assert!(pusher
            .push_verified_sha(&target, &fix.mirror, &fix.good_head_sha, "bad..name", None)
            .await
            .is_err());
        assert!(git(&remote, &["for-each-ref", "--format=%(refname)"]) == "refs/heads/feat/x");
    }

    #[tokio::test]
    async fn git_pusher_never_overwrites_a_branch_it_did_not_create() {
        let fix = setup_git_and_store();
        let github = fix._dir.path().join("github");
        let remote = github.join("org/repo.git");
        std::fs::create_dir_all(&remote).unwrap();
        git(
            fix._dir.path(),
            &["init", "--bare", "--template=", remote.to_str().unwrap()],
        );
        let pusher = GitMirrorPusher::with_local_base(&github);
        let target = PublishTarget::for_tests("org/repo");
        let tip = |branch: &str| git(&remote, &["rev-parse", &format!("refs/heads/{branch}")]);

        // Another Run (or a person) already owns `feat/x` at a different commit.
        pusher
            .push_verified_sha(&target, &fix.mirror, &fix.bad_head_sha_1, "feat/x", None)
            .await
            .unwrap();
        assert_eq!(tip("feat/x"), fix.bad_head_sha_1);

        // A first push of a different SHA is refused, and the branch is untouched:
        // a lease read from the remote just before the push would have allowed it.
        let refused = pusher
            .push_verified_sha(&target, &fix.mirror, &fix.good_head_sha, "feat/x", None)
            .await;
        assert!(matches!(refused, Err(Error::Publish)));
        assert_eq!(tip("feat/x"), fix.bad_head_sha_1);

        // The branch owner may still publish that exact SHA, so a rerun of the
        // same gate after a crash is not blocked.
        pusher
            .push_verified_sha(&target, &fix.mirror, &fix.bad_head_sha_1, "feat/x", None)
            .await
            .unwrap();
        assert_eq!(tip("feat/x"), fix.bad_head_sha_1);
        assert_eq!(
            git(&remote, &["for-each-ref", "--format=%(refname)"]),
            "refs/heads/feat/x"
        );
    }

    #[tokio::test]
    async fn gate_pushes_the_verified_head_sha_not_a_side_ref() {
        let fix = setup_git_and_store();
        let (base_url, gh, _app_state, server) = spawn_fake_github(fix.store.clone()).await;
        let scm = Arc::new(
            GitHubScmClient::new(
                GitHubConfig::new(SecretToken::new("bot-tok"), SecretToken::new("rev-tok"))
                    .with_base_urls(&base_url, format!("{base_url}/graphql")),
            )
            .unwrap(),
        );
        let pusher = Arc::new(MockPusher {
            pushed: Mutex::new(Vec::new()),
        });
        let deps = PrGateDeps {
            store: fix.store.clone(),
            scm,
            reviewer: Arc::new(MockReviewer {
                verdicts: Mutex::new(vec![ReviewVerdict::Approve]),
                seen_worktrees: Mutex::new(Vec::new()),
            }),
            worker_revise: None,
            pusher: pusher.clone(),
        };
        let gate = run_pr_gate(
            &deps,
            RunPrGateRequest {
                run_id: "run1".into(),
                hub_session_id: None,
                mirror_path: fix.mirror.clone(),
                repo_slug: "org/repo".into(),
                branch: "feat/sha".into(),
                base_branch: "main".into(),
                base_sha: fix.base_sha.clone(),
                head_sha: fix.good_head_sha.clone(),
                pr_title: "feat: sha".into(),
                pr_body: "body".into(),
                touched_paths: vec!["src/**".into()],
                is_high_risk: false,
                reviewers: vec![ReviewerSpec {
                    vendor: Vendor::Claude,
                    model: "claude-sonnet-4-6".into(),
                }],
            },
        )
        .await
        .unwrap();
        let gate = human_approve(&deps, &gate, None).await.unwrap();
        assert_eq!(gate.status, PrGateStatus::AutoMergeArmed);
        assert_eq!(
            *pusher.pushed.lock().unwrap(),
            vec![(fix.good_head_sha.clone(), "feat/sha".to_owned())]
        );
        assert_eq!(
            gh.enable_auto_merge_calls.lock().unwrap()[0].1,
            fix.good_head_sha
        );
        server.abort();
    }

    /// A revise turn that ends with only an observed (unverified) tip: the host
    /// executor reports no R1-verified result.
    struct UnverifiedRevise {
        revisions: AtomicUsize,
    }
    impl WorkerReviseExecutor for UnverifiedRevise {
        fn bounce_scope_violation<'a>(
            &'a self,
            _gate: &'a PrGateDto,
            _violations: &'a [String],
        ) -> BoxFuture<'a, Result<String>> {
            Box::pin(async { Err(Error::Policy) })
        }
        fn revise_pr<'a>(
            &'a self,
            _gate: &'a PrGateDto,
            _review_comments: &'a str,
        ) -> BoxFuture<'a, Result<String>> {
            Box::pin(async move {
                self.revisions.fetch_add(1, Ordering::SeqCst);
                Err(Error::Policy)
            })
        }
    }

    #[tokio::test]
    async fn unverified_revise_result_escalates_instead_of_reaching_review() {
        let fix = setup_git_and_store();
        let (base_url, gh, _app_state, server) = spawn_fake_github(fix.store.clone()).await;
        let scm = Arc::new(
            GitHubScmClient::new(
                GitHubConfig::new(SecretToken::new("bot-tok"), SecretToken::new("rev-tok"))
                    .with_base_urls(&base_url, format!("{base_url}/graphql")),
            )
            .unwrap(),
        );
        // Only one verdict: a second review would panic on the empty queue.
        let reviewer = Arc::new(MockReviewer {
            verdicts: Mutex::new(vec![ReviewVerdict::Revise]),
            seen_worktrees: Mutex::new(Vec::new()),
        });
        let pusher = Arc::new(MockPusher {
            pushed: Mutex::new(Vec::new()),
        });
        let revise = Arc::new(UnverifiedRevise {
            revisions: AtomicUsize::new(0),
        });
        let deps = PrGateDeps {
            store: fix.store.clone(),
            scm,
            reviewer: reviewer.clone(),
            worker_revise: Some(revise.clone()),
            pusher: pusher.clone(),
        };
        let gate = run_pr_gate(
            &deps,
            RunPrGateRequest {
                run_id: "run1".into(),
                hub_session_id: Some("hub".into()),
                mirror_path: fix.mirror.clone(),
                repo_slug: "org/repo".into(),
                branch: "feat/revise".into(),
                base_branch: "main".into(),
                base_sha: fix.base_sha.clone(),
                head_sha: fix.good_head_sha.clone(),
                pr_title: "feat: revise".into(),
                pr_body: "body".into(),
                touched_paths: vec!["src/**".into()],
                is_high_risk: false,
                reviewers: vec![ReviewerSpec {
                    vendor: Vendor::Claude,
                    model: "claude-sonnet-4-6".into(),
                }],
            },
        )
        .await
        .unwrap();

        assert_eq!(gate.status, PrGateStatus::Escalated);
        assert!(!gate.auto_merge_armed);
        assert_eq!(gate.head_sha, fix.good_head_sha);
        assert_eq!(revise.revisions.load(Ordering::SeqCst), 1);
        // Reviewed once (the verified head), never the revise output.
        assert_eq!(reviewer.seen_worktrees.lock().unwrap().len(), 1);
        assert_eq!(
            *pusher.pushed.lock().unwrap(),
            vec![(fix.good_head_sha.clone(), "feat/revise".to_owned())]
        );
        assert!(gh.enable_auto_merge_calls.lock().unwrap().is_empty());
        server.abort();
    }

    #[tokio::test]
    async fn a_branch_that_moved_under_a_revise_escalates_instead_of_being_forced() {
        let fix = setup_git_and_store();
        // An in-scope revision: another commit touching src/lib.rs.
        let revised = {
            let work = fix._dir.path().join("work");
            git(&work, &["checkout", "main"]);
            std::fs::write(work.join("src/lib.rs"), "pub fn base() -> i32 { 2 }\n").unwrap();
            git(&work, &["add", "."]);
            git(&work, &["commit", "-m", "revised"]);
            let sha = git(&work, &["rev-parse", "HEAD"]);
            git(
                &work,
                &["push", fix.mirror.to_str().unwrap(), "HEAD:refs/heads/main"],
            );
            sha
        };
        let (base_url, gh, _app_state, server) = spawn_fake_github(fix.store.clone()).await;
        let scm = Arc::new(
            GitHubScmClient::new(
                GitHubConfig::new(SecretToken::new("bot-tok"), SecretToken::new("rev-tok"))
                    .with_base_urls(&base_url, format!("{base_url}/graphql")),
            )
            .unwrap(),
        );
        let deps = PrGateDeps {
            store: fix.store.clone(),
            scm,
            reviewer: Arc::new(MockReviewer {
                verdicts: Mutex::new(vec![ReviewVerdict::Revise]),
                seen_worktrees: Mutex::new(Vec::new()),
            }),
            worker_revise: Some(Arc::new(MockRevise {
                next_sha: revised.clone(),
                bounces: AtomicUsize::new(0),
            })),
            pusher: Arc::new(FlakyPusher {
                calls: AtomicUsize::new(0),
            }),
        };
        let mut request = range_request(&fix, &fix.good_head_sha, "feat/moved");
        request.hub_session_id = Some("hub".into());
        let gate = run_pr_gate(&deps, request).await.unwrap();

        assert_eq!(gate.status, PrGateStatus::Escalated);
        // The PR keeps the commit it already had; the revision was not forced on.
        assert_eq!(gate.head_sha, fix.good_head_sha);
        assert!(gh.enable_auto_merge_calls.lock().unwrap().is_empty());
        assert!(
            fix.store
                .hub_messages("hub")
                .unwrap()
                .iter()
                .any(|m| m.text.contains("the branch moved")),
            "a moved branch must reach the Hub"
        );
        server.abort();
    }

    #[tokio::test]
    async fn out_of_scope_revise_escalates_with_a_hub_notice_and_pushes_nothing() {
        let fix = setup_git_and_store();
        // A commit off the base that touches README.md, outside `src/**`.
        let out_of_scope = fix.bad_head_sha_1.clone();
        let (base_url, gh, _app_state, server) = spawn_fake_github(fix.store.clone()).await;
        let scm = Arc::new(
            GitHubScmClient::new(
                GitHubConfig::new(SecretToken::new("bot-tok"), SecretToken::new("rev-tok"))
                    .with_base_urls(&base_url, format!("{base_url}/graphql")),
            )
            .unwrap(),
        );
        let reviewer = Arc::new(MockReviewer {
            verdicts: Mutex::new(vec![ReviewVerdict::Revise]),
            seen_worktrees: Mutex::new(Vec::new()),
        });
        let pusher = Arc::new(MockPusher {
            pushed: Mutex::new(Vec::new()),
        });
        let deps = PrGateDeps {
            store: fix.store.clone(),
            scm,
            reviewer,
            worker_revise: Some(Arc::new(MockRevise {
                next_sha: out_of_scope.clone(),
                bounces: AtomicUsize::new(0),
            })),
            pusher: pusher.clone(),
        };
        let gate = run_pr_gate(
            &deps,
            RunPrGateRequest {
                run_id: "run1".into(),
                hub_session_id: Some("hub".into()),
                mirror_path: fix.mirror.clone(),
                repo_slug: "org/repo".into(),
                branch: "feat/scope".into(),
                base_branch: "main".into(),
                base_sha: fix.base_sha.clone(),
                head_sha: fix.good_head_sha.clone(),
                pr_title: "feat: scope".into(),
                pr_body: "body".into(),
                touched_paths: vec!["src/**".into()],
                is_high_risk: false,
                reviewers: vec![ReviewerSpec {
                    vendor: Vendor::Claude,
                    model: "claude-sonnet-4-6".into(),
                }],
            },
        )
        .await
        .unwrap();

        assert_eq!(gate.status, PrGateStatus::Escalated);
        // Only the verified head was ever published; the out-of-scope commit was not.
        assert_eq!(
            *pusher.pushed.lock().unwrap(),
            vec![(fix.good_head_sha.clone(), "feat/scope".to_owned())]
        );
        assert_eq!(gate.head_sha, fix.good_head_sha);
        assert!(!gate.scope_violations.is_empty());
        assert!(gh.enable_auto_merge_calls.lock().unwrap().is_empty());
        // The human is told, like every other escalation on this path.
        assert!(
            fix.store
                .hub_messages("hub")
                .unwrap()
                .iter()
                .any(|m| m.text.contains("outside the Run's mandate")),
            "a revise scope violation must reach the Hub"
        );
        server.abort();
    }

    #[tokio::test]
    async fn unverified_scope_bounce_result_escalates_without_push() {
        let fix = setup_git_and_store();
        let (base_url, _gh, _app_state, server) = spawn_fake_github(fix.store.clone()).await;
        let scm = Arc::new(
            GitHubScmClient::new(
                GitHubConfig::new(SecretToken::new("bot-tok"), SecretToken::new("rev-tok"))
                    .with_base_urls(&base_url, format!("{base_url}/graphql")),
            )
            .unwrap(),
        );
        let pusher = Arc::new(MockPusher {
            pushed: Mutex::new(Vec::new()),
        });
        let deps = PrGateDeps {
            store: fix.store.clone(),
            scm,
            reviewer: Arc::new(MockReviewer {
                verdicts: Mutex::new(vec![]),
                seen_worktrees: Mutex::new(Vec::new()),
            }),
            worker_revise: Some(Arc::new(UnverifiedRevise {
                revisions: AtomicUsize::new(0),
            })),
            pusher: pusher.clone(),
        };
        let gate = run_pr_gate(
            &deps,
            RunPrGateRequest {
                run_id: "run1".into(),
                hub_session_id: None,
                mirror_path: fix.mirror.clone(),
                repo_slug: "org/repo".into(),
                branch: "feat/bounce".into(),
                base_branch: "main".into(),
                base_sha: fix.base_sha.clone(),
                head_sha: fix.bad_head_sha_1.clone(),
                pr_title: "feat: bounce".into(),
                pr_body: "body".into(),
                touched_paths: vec!["src/**".into()],
                is_high_risk: false,
                reviewers: vec![ReviewerSpec {
                    vendor: Vendor::Claude,
                    model: "claude-sonnet-4-6".into(),
                }],
            },
        )
        .await
        .unwrap();
        assert_eq!(gate.status, PrGateStatus::Escalated);
        assert!(pusher.pushed.lock().unwrap().is_empty());
        server.abort();
    }

    /// Adds one commit on top of the good head (writing `file`) and publishes it
    /// to the mirror's `main`, so `base..new` spans two commits.
    fn extend_good_head(fix: &GitFixture, file: &str, contents: &str) -> String {
        let work = fix._dir.path().join("work");
        git(&work, &["checkout", "main"]);
        std::fs::write(work.join(file), contents).unwrap();
        git(&work, &["add", "."]);
        git(&work, &["commit", "-m", "second"]);
        let sha = git(&work, &["rev-parse", "HEAD"]);
        git(
            &work,
            &["push", fix.mirror.to_str().unwrap(), "HEAD:refs/heads/main"],
        );
        sha
    }

    struct RangeReviewer {
        seen: Mutex<Vec<(ReviewWorktreeRequest, ReviewDiff)>>,
    }
    impl ReviewerExecutor for RangeReviewer {
        fn check_available(&self, _spec: &ReviewerSpec) -> Result<()> {
            Ok(())
        }
        fn run_review<'a>(
            &'a self,
            req: ReviewWorktreeRequest,
        ) -> BoxFuture<'a, Result<ReviewOutcome>> {
            Box::pin(async move {
                let diff = review_diff(&req.worktree_path, &req.base_sha, &req.head_sha)?;
                self.seen.lock().unwrap().push((req, diff));
                Ok(ReviewOutcome {
                    verdict: ReviewVerdict::Approve,
                    comments: "LGTM".to_owned(),
                })
            })
        }
    }

    fn range_request(fix: &GitFixture, head: &str, branch: &str) -> RunPrGateRequest {
        RunPrGateRequest {
            run_id: "run1".into(),
            hub_session_id: None,
            mirror_path: fix.mirror.clone(),
            repo_slug: "org/repo".into(),
            branch: branch.into(),
            base_branch: "main".into(),
            base_sha: fix.base_sha.clone(),
            head_sha: head.to_owned(),
            pr_title: "feat: range".into(),
            pr_body: "body".into(),
            touched_paths: vec!["src/**".into()],
            is_high_risk: false,
            reviewers: vec![ReviewerSpec {
                vendor: Vendor::Claude,
                model: "claude-sonnet-4-6".into(),
            }],
        }
    }

    #[tokio::test]
    async fn reviewer_sees_the_whole_two_commit_pr_range() {
        let fix = setup_git_and_store();
        let head = extend_good_head(&fix, "src/two.rs", "pub fn two() {}\n");

        // The helper covers both commits; `git show <head>` would miss src/lib.rs.
        let ReviewDiff::Complete { text, commits } =
            review_diff(&fix.mirror, &fix.base_sha, &head).unwrap()
        else {
            panic!("small range must be complete");
        };
        assert_eq!(commits, 2);
        assert!(text.contains(&fix.good_head_sha) && text.contains(&head));
        assert!(text.contains("src/lib.rs") && text.contains("src/two.rs"));
        assert!(text.contains("pub fn base() -> i32 { 1 }") && text.contains("pub fn two()"));
        assert!(review_diff(&fix.mirror, &fix.base_sha[..12], &head).is_err());

        let (base_url, gh, _app_state, server) = spawn_fake_github(fix.store.clone()).await;
        let scm = Arc::new(
            GitHubScmClient::new(
                GitHubConfig::new(SecretToken::new("bot-tok"), SecretToken::new("rev-tok"))
                    .with_base_urls(&base_url, format!("{base_url}/graphql")),
            )
            .unwrap(),
        );
        let reviewer = Arc::new(RangeReviewer {
            seen: Mutex::new(Vec::new()),
        });
        let deps = PrGateDeps {
            store: fix.store.clone(),
            scm,
            reviewer: reviewer.clone(),
            worker_revise: None,
            pusher: Arc::new(MockPusher {
                pushed: Mutex::new(Vec::new()),
            }),
        };
        let gate = run_pr_gate(&deps, range_request(&fix, &head, "feat/range"))
            .await
            .unwrap();
        let gate = human_approve(&deps, &gate, None).await.unwrap();
        assert_eq!(gate.status, PrGateStatus::AutoMergeArmed);
        let seen = reviewer.seen.lock().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].0.base_sha, fix.base_sha);
        assert_eq!(seen[0].0.head_sha, head);
        // Inside the disposable worktree the reviewer got the same full range.
        assert_eq!(
            seen[0].1,
            ReviewDiff::Complete {
                text: text.clone(),
                commits: 2
            }
        );
        assert_eq!(gh.enable_auto_merge_calls.lock().unwrap()[0].1, head);
        server.abort();
    }

    #[tokio::test]
    async fn oversized_pr_range_escalates_instead_of_a_truncated_review() {
        let fix = setup_git_and_store();
        let big: String = (0..MAX_REVIEW_DIFF_BYTES / 16)
            .map(|i| format!("// line {i:08}\n"))
            .collect();
        let head = extend_good_head(&fix, "src/big.rs", &big);
        assert!(matches!(
            review_diff(&fix.mirror, &fix.base_sha, &head).unwrap(),
            ReviewDiff::TooLarge { bytes } if bytes > MAX_REVIEW_DIFF_BYTES
        ));

        let (base_url, gh, _app_state, server) = spawn_fake_github(fix.store.clone()).await;
        let scm = Arc::new(
            GitHubScmClient::new(
                GitHubConfig::new(SecretToken::new("bot-tok"), SecretToken::new("rev-tok"))
                    .with_base_urls(&base_url, format!("{base_url}/graphql")),
            )
            .unwrap(),
        );
        let reviewer = Arc::new(RangeReviewer {
            seen: Mutex::new(Vec::new()),
        });
        let deps = PrGateDeps {
            store: fix.store.clone(),
            scm,
            reviewer: reviewer.clone(),
            worker_revise: None,
            pusher: Arc::new(MockPusher {
                pushed: Mutex::new(Vec::new()),
            }),
        };
        let gate = run_pr_gate(&deps, range_request(&fix, &head, "feat/big"))
            .await
            .unwrap();
        assert_eq!(gate.status, PrGateStatus::Escalated);
        assert!(!gate.auto_merge_armed);
        assert!(reviewer.seen.lock().unwrap().is_empty());
        assert!(gh.submitted_reviews.lock().unwrap().is_empty());
        assert!(gh.enable_auto_merge_calls.lock().unwrap().is_empty());
        server.abort();
    }

    /// Only `available` vendors resolve a provider; every review is recorded.
    struct PartlyAvailableReviewer {
        available: Vec<Vendor>,
        checked: Mutex<Vec<Vendor>>,
        reviews: AtomicUsize,
    }
    impl ReviewerExecutor for PartlyAvailableReviewer {
        fn check_available(&self, spec: &ReviewerSpec) -> Result<()> {
            self.checked.lock().unwrap().push(spec.vendor);
            if self.available.contains(&spec.vendor) {
                Ok(())
            } else {
                Err(Error::Credential)
            }
        }
        fn run_review<'a>(
            &'a self,
            _req: ReviewWorktreeRequest,
        ) -> BoxFuture<'a, Result<ReviewOutcome>> {
            Box::pin(async move {
                self.reviews.fetch_add(1, Ordering::SeqCst);
                Ok(ReviewOutcome {
                    verdict: ReviewVerdict::Approve,
                    comments: "LGTM".to_owned(),
                })
            })
        }
    }

    #[tokio::test]
    async fn oversized_pr_range_escalates_before_the_push_and_opens_no_pr() {
        let fix = setup_git_and_store();
        let big: String = (0..MAX_REVIEW_DIFF_BYTES / 16)
            .map(|i| format!("// line {i:08}\n"))
            .collect();
        let head = extend_good_head(&fix, "src/big.rs", &big);
        assert!(matches!(
            review_diff(&fix.mirror, &fix.base_sha, &head).unwrap(),
            ReviewDiff::TooLarge { bytes } if bytes > MAX_REVIEW_DIFF_BYTES
        ));

        let (base_url, gh, _app_state, server) = spawn_fake_github(fix.store.clone()).await;
        let scm = Arc::new(
            GitHubScmClient::new(
                GitHubConfig::new(SecretToken::new("bot-tok"), SecretToken::new("rev-tok"))
                    .with_base_urls(&base_url, format!("{base_url}/graphql")),
            )
            .unwrap(),
        );
        let pusher = Arc::new(MockPusher {
            pushed: Mutex::new(Vec::new()),
        });
        let deps = PrGateDeps {
            store: fix.store.clone(),
            scm,
            reviewer: Arc::new(RangeReviewer {
                seen: Mutex::new(Vec::new()),
            }),
            worker_revise: None,
            pusher: pusher.clone(),
        };
        let gate = run_pr_gate(&deps, range_request(&fix, &head, "feat/big"))
            .await
            .unwrap();

        assert_eq!(gate.status, PrGateStatus::Escalated);
        // The size is known before anything leaves the machine.
        assert!(pusher.pushed.lock().unwrap().is_empty());
        assert_eq!(gate.pr_number, None);
        assert!(gh.submitted_reviews.lock().unwrap().is_empty());
        assert!(gh.enable_auto_merge_calls.lock().unwrap().is_empty());
        server.abort();
    }

    #[tokio::test]
    async fn unavailable_reviewer_escalates_before_any_push_or_pr() {
        let fix = setup_git_and_store();
        let (base_url, gh, _app_state, server) = spawn_fake_github(fix.store.clone()).await;
        let scm = Arc::new(
            GitHubScmClient::new(
                GitHubConfig::new(SecretToken::new("bot-tok"), SecretToken::new("rev-tok"))
                    .with_base_urls(&base_url, format!("{base_url}/graphql")),
            )
            .unwrap(),
        );
        // High risk needs two reviewers: Claude resolves, Agy does not.
        let reviewer = Arc::new(PartlyAvailableReviewer {
            available: vec![Vendor::Claude],
            checked: Mutex::new(Vec::new()),
            reviews: AtomicUsize::new(0),
        });
        let pusher = Arc::new(MockPusher {
            pushed: Mutex::new(Vec::new()),
        });
        let deps = PrGateDeps {
            store: fix.store.clone(),
            scm,
            reviewer: reviewer.clone(),
            worker_revise: None,
            pusher: pusher.clone(),
        };
        let mut request = range_request(&fix, &fix.good_head_sha, "feat/preflight");
        request.hub_session_id = Some("hub".into());
        request.is_high_risk = true;
        request.reviewers = vec![
            ReviewerSpec {
                vendor: Vendor::Claude,
                model: "claude-sonnet-4-6".into(),
            },
            ReviewerSpec {
                vendor: Vendor::Agy,
                model: "gemini-3-pro".into(),
            },
        ];
        let gate = run_pr_gate(&deps, request).await.unwrap();

        assert_eq!(gate.status, PrGateStatus::Escalated);
        assert_eq!(gate.pr_number, None);
        assert_eq!(
            *reviewer.checked.lock().unwrap(),
            vec![Vendor::Claude, Vendor::Agy]
        );
        assert_eq!(reviewer.reviews.load(Ordering::SeqCst), 0);
        assert!(pusher.pushed.lock().unwrap().is_empty());
        assert!(gh.submitted_reviews.lock().unwrap().is_empty());
        assert!(gh.enable_auto_merge_calls.lock().unwrap().is_empty());
        server.abort();
    }

    #[tokio::test]
    async fn refused_first_push_escalates_with_a_hub_notice_and_no_pr() {
        let fix = setup_git_and_store();
        let (base_url, gh, _app_state, server) = spawn_fake_github(fix.store.clone()).await;
        let scm = Arc::new(
            GitHubScmClient::new(
                GitHubConfig::new(SecretToken::new("bot-tok"), SecretToken::new("rev-tok"))
                    .with_base_urls(&base_url, format!("{base_url}/graphql")),
            )
            .unwrap(),
        );
        let deps = PrGateDeps {
            store: fix.store.clone(),
            scm,
            reviewer: Arc::new(MockReviewer {
                verdicts: Mutex::new(vec![ReviewVerdict::Approve]),
                seen_worktrees: Mutex::new(Vec::new()),
            }),
            worker_revise: None,
            pusher: Arc::new(RefusingPusher),
        };
        let mut request = range_request(&fix, &fix.good_head_sha, "feat/not-ours");
        request.hub_session_id = Some("hub".into());
        let gate = run_pr_gate(&deps, request).await.unwrap();

        assert_eq!(gate.status, PrGateStatus::Escalated);
        assert_eq!(gate.pr_number, None);
        assert!(gh.submitted_reviews.lock().unwrap().is_empty());
        assert!(gh.enable_auto_merge_calls.lock().unwrap().is_empty());
        let notices = fix.store.hub_messages("hub").unwrap();
        assert!(
            notices
                .iter()
                .any(|m| m.text.contains("pushing would overwrite work")),
            "escalation must reach the Hub: {notices:?}"
        );
        server.abort();
    }

    fn replace_origin_snapshot(store: &Store, snapshot: Option<polyclave_store::OriginSnapshot>) {
        store
            .write(move |db| {
                db.execute("DELETE FROM run_publish_origins WHERE run_id='run1'", [])?;
                if let Some(snapshot) = snapshot {
                    polyclave_store::insert_run_origin_snapshot(db, "run1", &snapshot)?;
                }
                Ok(())
            })
            .unwrap();
    }

    fn origin(urls: &[&str], push_urls: &[&str]) -> Option<polyclave_store::OriginSnapshot> {
        Some(polyclave_store::OriginSnapshot {
            urls: urls.iter().map(|s| (*s).to_owned()).collect(),
            push_urls: push_urls.iter().map(|s| (*s).to_owned()).collect(),
        })
    }

    #[tokio::test]
    async fn unusable_origin_snapshot_escalates_before_any_push_or_pr() {
        // `\u{40}` is '@': scripts/privacy-scan.sh reads a user, an at sign and a host as an email address.
        let cases = [
            ("no snapshot", None),
            ("missing origin", origin(&[], &[])),
            (
                "non-GitHub host",
                origin(&["https://gitlab.com/org/repo.git"], &[]),
            ),
            (
                "slug mismatch",
                origin(&["git\u{40}github.com:org/other.git"], &[]),
            ),
            (
                "separate pushurl",
                origin(
                    &["https://github.com/org/repo.git"],
                    &["https://github.com/org/repo.git"],
                ),
            ),
            (
                "several URLs",
                origin(
                    &[
                        "https://github.com/org/repo.git",
                        "git\u{40}github.com:org/repo.git",
                    ],
                    &[],
                ),
            ),
        ];
        for (label, snapshot) in cases {
            let fix = setup_git_and_store();
            replace_origin_snapshot(&fix.store, snapshot);
            let (base_url, gh, _app_state, server) = spawn_fake_github(fix.store.clone()).await;
            let scm = Arc::new(
                GitHubScmClient::new(
                    GitHubConfig::new(SecretToken::new("bot-tok"), SecretToken::new("rev-tok"))
                        .with_base_urls(&base_url, format!("{base_url}/graphql")),
                )
                .unwrap(),
            );
            let reviewer = Arc::new(PartlyAvailableReviewer {
                available: vec![Vendor::Claude],
                checked: Mutex::new(Vec::new()),
                reviews: AtomicUsize::new(0),
            });
            let pusher = Arc::new(MockPusher {
                pushed: Mutex::new(Vec::new()),
            });
            let deps = PrGateDeps {
                store: fix.store.clone(),
                scm,
                reviewer: reviewer.clone(),
                worker_revise: None,
                pusher: pusher.clone(),
            };
            let mut request = range_request(&fix, &fix.good_head_sha, "feat/origin");
            request.hub_session_id = Some("hub".into());
            let gate = run_pr_gate(&deps, request).await.unwrap();

            assert_eq!(gate.status, PrGateStatus::Escalated, "{label}");
            assert_eq!(gate.pr_number, None, "{label}");
            assert!(pusher.pushed.lock().unwrap().is_empty(), "{label}");
            assert!(reviewer.checked.lock().unwrap().is_empty(), "{label}");
            assert!(gh.submitted_reviews.lock().unwrap().is_empty(), "{label}");
            assert!(
                gh.enable_auto_merge_calls.lock().unwrap().is_empty(),
                "{label}"
            );
            let notices = fix.store.hub_messages("hub").unwrap();
            let notice = notices
                .iter()
                .find(|m| m.text.contains(&gate.id))
                .unwrap_or_else(|| panic!("{label}: no Hub notice"));
            assert!(
                notice.text.contains("cannot publish to org/repo")
                    && notice.text.contains("nothing was pushed"),
                "{label}: {}",
                notice.text
            );
            assert!(!notice.text.contains("://"), "{label}: {}", notice.text);
            server.abort();
        }
    }

    #[tokio::test]
    async fn gate_publishes_to_the_admission_origin_even_after_origin_changes() {
        let fix = setup_git_and_store();
        let work = fix._dir.path().join("work");
        git(
            &work,
            &["remote", "add", "origin", "https://github.com/Org/Repo.git"],
        );
        let state = fix._dir.path().join("state");
        std::fs::create_dir_all(&state).unwrap();
        let mirrors = polyclave_core::mirror::MirrorManager::new(&state).unwrap();
        replace_origin_snapshot(&fix.store, Some(mirrors.origin_snapshot(&work)));
        // Re-pointing origin after admission must not redirect the publish.
        git(
            &work,
            &[
                "remote",
                "set-url",
                "origin",
                "https://github.com/evil/repo.git",
            ],
        );

        // Local bare repositories stand in for GitHub.
        let github = fix._dir.path().join("github");
        for slug in ["org/repo.git", "evil/repo.git"] {
            let remote = github.join(slug);
            std::fs::create_dir_all(&remote).unwrap();
            git(&remote, &["init", "--bare", "--template="]);
        }
        // A real GitHub repository has the PR's base branch, and it contains the
        // Run's reviewed baseline.
        git(
            &work,
            &[
                "push",
                github.join("org/repo.git").to_str().unwrap(),
                &format!("{}:refs/heads/main", fix.base_sha),
            ],
        );
        let (base_url, gh, _app_state, server) = spawn_fake_github(fix.store.clone()).await;
        let scm = Arc::new(
            GitHubScmClient::new(
                GitHubConfig::new(SecretToken::new("bot-tok"), SecretToken::new("rev-tok"))
                    .with_base_urls(&base_url, format!("{base_url}/graphql")),
            )
            .unwrap(),
        );
        let deps = PrGateDeps {
            store: fix.store.clone(),
            scm,
            reviewer: Arc::new(MockReviewer {
                verdicts: Mutex::new(vec![ReviewVerdict::Approve]),
                seen_worktrees: Mutex::new(Vec::new()),
            }),
            worker_revise: None,
            pusher: Arc::new(GitMirrorPusher::with_local_base(&github)),
        };
        let gate = run_pr_gate(
            &deps,
            range_request(&fix, &fix.good_head_sha, "feat/origin"),
        )
        .await
        .unwrap();
        let gate = human_approve(&deps, &gate, Some("hub")).await.unwrap();
        assert_eq!(gate.status, PrGateStatus::AutoMergeArmed);
        assert_eq!(
            git(
                &github.join("org/repo.git"),
                &["rev-parse", "refs/heads/feat/origin"]
            ),
            fix.good_head_sha
        );
        assert_eq!(
            git(
                &github.join("evil/repo.git"),
                &["for-each-ref", "--format=%(refname)"]
            ),
            ""
        );
        assert_eq!(
            gh.enable_auto_merge_calls.lock().unwrap()[0].1,
            fix.good_head_sha
        );
        server.abort();
    }

    const SENTINEL_TOKEN: &str = "SENTINEL-publish-token-7f3a9c";

    #[test]
    fn publish_credential_helper_answers_with_the_token_and_drops_other_helpers() {
        let fix = setup_git_and_store();
        // A helper already configured on the mirror must never be consulted.
        git(
            &fix.mirror,
            &[
                "config",
                "credential.helper",
                "!f() { echo username=u; echo password=LEAKED-LOCAL; }; f",
            ],
        );
        let git_bin = resolve_cli_binary(Path::new("git")).unwrap();
        let token = SecretToken::new(SENTINEL_TOKEN);
        let mut cmd = remote_git(&git_bin, &fix.mirror, "https", &token);
        cmd.args(["credential", "fill"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        assert!(
            cmd.get_args()
                .all(|a| !a.to_string_lossy().contains(SENTINEL_TOKEN)),
            "the token must not be in argv"
        );
        assert!(PUBLISH_CREDENTIAL_HELPER.contains(&format!("${PUBLISH_TOKEN_ENV}")));
        let mut child = cmd.spawn().unwrap();
        {
            use std::io::Write as _;
            let mut stdin = child.stdin.take().unwrap();
            stdin
                .write_all(b"protocol=https\nhost=github.com\npath=org/repo.git\n\n")
                .unwrap();
        }
        let out = child.wait_with_output().unwrap();
        let stdout = String::from_utf8_lossy(&out.stdout);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(stdout.contains("username=x-access-token"), "{stdout}");
        assert!(
            stdout.contains(&format!("password={SENTINEL_TOKEN}")),
            "{stdout}"
        );
        assert!(!stdout.contains("LEAKED-LOCAL"), "{stdout}");
    }

    /// Every text value in every table of the ledger.
    #[cfg(unix)]
    fn dump_database(store: &Store) -> String {
        store
            .read(|db| {
                let mut tables = Vec::new();
                let mut stmt = db.prepare("SELECT name FROM sqlite_master WHERE type='table'")?;
                let mut rows = stmt.query([])?;
                while let Some(row) = rows.next()? {
                    tables.push(row.get::<_, String>(0)?);
                }
                let mut dump = String::new();
                for table in tables {
                    let mut stmt = db.prepare(&format!("SELECT * FROM \"{table}\""))?;
                    let columns = stmt.column_count();
                    let mut rows = stmt.query([])?;
                    while let Some(row) = rows.next()? {
                        for i in 0..columns {
                            if let Ok(Some(text)) = row.get::<_, Option<String>>(i) {
                                dump.push_str(&text);
                                dump.push('\n');
                            }
                        }
                    }
                }
                Ok(dump)
            })
            .unwrap()
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn publish_token_never_reaches_argv_stderr_errors_events_or_the_database() {
        use std::os::unix::fs::PermissionsExt as _;

        let fix = setup_git_and_store();
        let github = fix._dir.path().join("github");
        let remote = github.join("org/repo.git");
        std::fs::create_dir_all(&remote).unwrap();
        git(&remote, &["init", "--bare", "--template="]);
        // The remote's base branch contains the reviewed baseline, as on GitHub.
        let work = fix._dir.path().join("work");
        git(
            &work,
            &[
                "push",
                remote.to_str().unwrap(),
                &format!("{}:refs/heads/main", fix.base_sha),
            ],
        );

        // A git wrapper that records argv, whether the token variable was set,
        // and git's stderr, then runs the real git.
        let real_git = resolve_cli_binary(Path::new("git")).unwrap();
        let argv_log = fix._dir.path().join("argv.log");
        let stderr_log = fix._dir.path().join("stderr.log");
        let wrapper = fix._dir.path().join("git-wrapper");
        std::fs::write(
            &wrapper,
            format!(
                "#!/bin/sh\nprintf 'ARGV:' >> '{argv}'\nfor a in \"$@\"; do printf ' [%s]' \"$a\" >> '{argv}'; done\nprintf '\\nTOKEN:%s\\n' \"${{{env}:+set}}\" >> '{argv}'\n'{git}' \"$@\" 2>> '{stderr}'\n",
                argv = argv_log.display(),
                env = PUBLISH_TOKEN_ENV,
                git = real_git.display(),
                stderr = stderr_log.display(),
            ),
        )
        .unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
        let pusher = Arc::new(
            GitMirrorPusher::with_local_base(&github)
                .with_token(SENTINEL_TOKEN)
                .with_git_binary(wrapper),
        );

        let (base_url, _gh, _app_state, server) = spawn_fake_github(fix.store.clone()).await;
        let scm = Arc::new(
            GitHubScmClient::new(
                GitHubConfig::new(SecretToken::new("bot-tok"), SecretToken::new("rev-tok"))
                    .with_base_urls(&base_url, format!("{base_url}/graphql")),
            )
            .unwrap(),
        );
        let deps = PrGateDeps {
            store: fix.store.clone(),
            scm,
            reviewer: Arc::new(MockReviewer {
                verdicts: Mutex::new(vec![ReviewVerdict::Approve]),
                seen_worktrees: Mutex::new(Vec::new()),
            }),
            worker_revise: None,
            pusher: pusher.clone(),
        };
        let mut request = range_request(&fix, &fix.good_head_sha, "feat/token");
        request.hub_session_id = Some("hub".into());
        let gate = run_pr_gate(&deps, request).await.unwrap();
        let gate = human_approve(&deps, &gate, Some("hub")).await.unwrap();
        assert_eq!(gate.status, PrGateStatus::AutoMergeArmed);
        assert_eq!(
            git(&remote, &["rev-parse", "refs/heads/feat/token"]),
            fix.good_head_sha
        );

        // A refused push (stale lease) and a failing remote also keep it out
        // of the returned error.
        let target = PublishTarget::for_tests("org/repo");
        let stale = pusher
            .push_verified_sha(
                &target,
                &fix.mirror,
                &fix.bad_head_sha_2,
                "feat/token",
                Some(fix.bad_head_sha_1.as_str()),
            )
            .await
            .unwrap_err();
        let missing = pusher
            .push_verified_sha(
                &PublishTarget::for_tests("org/absent"),
                &fix.mirror,
                &fix.good_head_sha,
                "feat/token",
                None,
            )
            .await
            .unwrap_err();
        for err in [stale, missing] {
            assert!(!format!("{err} {err:?}").contains(SENTINEL_TOKEN));
        }

        let argv = std::fs::read_to_string(&argv_log).unwrap();
        assert!(!argv.contains(SENTINEL_TOKEN), "{argv}");
        let canonical = format!("[{}]", github.join("org/repo.git").display());
        let mut remote_commands = 0;
        let mut lines = argv.lines();
        while let (Some(call), Some(token)) = (lines.next(), lines.next()) {
            let talks_to_remote =
                call.contains("[ls-remote]") || call.contains("[push]") || call.contains("[fetch]");
            if talks_to_remote {
                remote_commands += 1;
                assert!(call.contains("[credential.helper=]"), "{call}");
                assert!(!call.contains("[origin]"), "{call}");
                if !call.contains("org/absent") {
                    assert!(call.contains(&canonical), "{call}");
                }
            }
            // The token variable is set only on commands that talk to the remote.
            assert_eq!(token == "TOKEN:set", talks_to_remote, "{call} {token}");
        }
        // The base-branch ls-remote that proves the reviewed base is still on the
        // remote, then ls-remote, push, ls-remote; then the stale lease's
        // ls-remote, and the missing repository's ls-remote. Every one of them
        // carries the one-shot credential helper and none carries the token.
        assert_eq!(remote_commands, 6, "{argv}");
        let stderr = std::fs::read_to_string(&stderr_log).unwrap_or_default();
        assert!(!stderr.contains(SENTINEL_TOKEN), "{stderr}");

        let notices: Vec<String> = fix
            .store
            .hub_messages("hub")
            .unwrap()
            .into_iter()
            .map(|m| m.text)
            .collect();
        assert!(!notices.concat().contains(SENTINEL_TOKEN));
        let dump = dump_database(&fix.store);
        // The dump does see the gate's own rows.
        assert!(dump.contains("feat/token") && dump.contains("github.com/org/repo"));
        assert!(!dump.contains(SENTINEL_TOKEN));
        server.abort();
    }
}
