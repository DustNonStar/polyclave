use reqwest::{header, Client, Method, StatusCode};
use serde::{Deserialize, Serialize};
use std::{fmt, future::Future, pin::Pin};
use zeroize::Zeroizing;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

const DEFAULT_REST_BASE_URL: &str = "https://api.github.com";
const DEFAULT_GRAPHQL_URL: &str = "https://api.github.com/graphql";
const GITHUB_API_VERSION: &str = "2022-11-28";

/// Secret bearer token stored in zeroizing memory and redacted from Debug/Display.
#[derive(Clone)]
pub struct SecretToken(Zeroizing<String>);

impl SecretToken {
    pub fn new(token: impl Into<String>) -> Self {
        Self(Zeroizing::new(token.into()))
    }

    pub(crate) fn expose(&self) -> &str {
        self.0.as_str()
    }
}

impl fmt::Debug for SecretToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

impl fmt::Display for SecretToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[REDACTED]")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ScmError {
    #[error("invalid repository slug '{0}' (expected owner/repo)")]
    InvalidRepoSlug(String),
    #[error("empty GitHub credential for {0}")]
    EmptyToken(&'static str),
    #[error("bot account and reviewer account must be distinct (both resolved to '{login}')")]
    SameBotAndReviewerAccount { login: String },
    #[error("GitHub transport error during {op}")]
    Transport { op: &'static str },
    #[error("GitHub API returned HTTP {status} during {op}: {message}")]
    Http {
        op: &'static str,
        status: u16,
        message: String,
    },
    #[error("GitHub GraphQL error during {op}: {message}")]
    GraphQl { op: &'static str, message: String },
    #[error("GitHub API returned an invalid response during {op}")]
    InvalidResponse { op: &'static str },
}

#[derive(Clone, Debug)]
pub struct GitHubConfig {
    pub rest_base_url: String,
    pub graphql_url: String,
    pub bot_token: SecretToken,
    pub reviewer_token: SecretToken,
}

impl GitHubConfig {
    pub fn new(bot_token: SecretToken, reviewer_token: SecretToken) -> Self {
        Self {
            rest_base_url: DEFAULT_REST_BASE_URL.to_owned(),
            graphql_url: DEFAULT_GRAPHQL_URL.to_owned(),
            bot_token,
            reviewer_token,
        }
    }

    pub fn with_base_urls(
        mut self,
        rest_base_url: impl Into<String>,
        graphql_url: impl Into<String>,
    ) -> Self {
        self.rest_base_url = rest_base_url.into().trim_end_matches('/').to_owned();
        self.graphql_url = graphql_url.into().trim_end_matches('/').to_owned();
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PreflightReport {
    pub bot_login: String,
    pub reviewer_login: String,
    pub default_branch: String,
    pub branch_protected: bool,
    pub approval_required: bool,
    pub required_approving_review_count: u32,
    pub squash_merge_allowed: bool,
    pub auto_merge_allowed: bool,
    pub passed: bool,
    pub issues: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OpenPrRequest {
    pub repo_slug: String,
    pub head_branch: String,
    pub base_branch: String,
    pub title: String,
    pub body: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PullRequestInfo {
    pub number: u64,
    pub node_id: String,
    pub head_sha: String,
    pub state: String,
    pub html_url: String,
    pub created: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ReviewEvent {
    Approve,
    RequestChanges,
    Comment,
}

impl ReviewEvent {
    fn as_github_str(self) -> &'static str {
        match self {
            Self::Approve => "APPROVE",
            Self::RequestChanges => "REQUEST_CHANGES",
            Self::Comment => "COMMENT",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SubmitReviewRequest {
    pub repo_slug: String,
    pub pr_number: u64,
    pub reviewed_sha: String,
    pub vendor: String,
    pub model: String,
    pub event: ReviewEvent,
    pub body: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SubmittedReview {
    pub id: u64,
    pub commit_id: String,
    pub state: String,
    pub html_url: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PrState {
    pub number: u64,
    pub node_id: String,
    pub state: String,
    pub merged: bool,
    pub mergeable: Option<bool>,
    pub mergeable_state: Option<String>,
    #[serde(default)]
    pub merge_state_status: Option<String>,
    pub head_sha: String,
    pub auto_merge_enabled: bool,
    pub html_url: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CheckRunInfo {
    pub id: u64,
    pub name: String,
    pub status: String,
    pub conclusion: Option<String>,
    pub html_url: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PrCommentInfo {
    pub id: u64,
    pub author_login: String,
    pub body: String,
    pub created_at: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PrReviewInfo {
    pub id: u64,
    pub author_login: String,
    pub state: String,
    pub commit_id: Option<String>,
    pub body: String,
    pub submitted_at: Option<String>,
    pub is_agent_reviewer: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PrChecksAndComments {
    pub check_runs: Vec<CheckRunInfo>,
    /// `true` only when every page of the check-run list was read. A partial
    /// list may be missing the one failing check, so it is never treated as a
    /// complete set of evidence for a SHA.
    pub checks_complete: bool,
    pub comments: Vec<PrCommentInfo>,
    pub reviews: Vec<PrReviewInfo>,
}

pub fn format_agent_reviewer_marker(vendor: &str, model: &str, sha: &str) -> String {
    format!("[agent-reviewer · {vendor}/{model} · {sha}]")
}

pub fn is_agent_reviewer_marker(body: &str) -> bool {
    body.contains("[agent-reviewer · ")
}

pub trait ScmClient: Send + Sync {
    fn preflight_repo<'a>(
        &'a self,
        repo_slug: &'a str,
        base_branch: &'a str,
    ) -> BoxFuture<'a, Result<PreflightReport, ScmError>>;

    fn open_or_find_pr<'a>(
        &'a self,
        req: OpenPrRequest,
    ) -> BoxFuture<'a, Result<PullRequestInfo, ScmError>>;

    fn submit_review<'a>(
        &'a self,
        req: SubmitReviewRequest,
    ) -> BoxFuture<'a, Result<SubmittedReview, ScmError>>;

    fn enable_auto_merge<'a>(
        &'a self,
        pr_node_id: &'a str,
        expected_head_oid: &'a str,
    ) -> BoxFuture<'a, Result<(), ScmError>>;

    fn disable_auto_merge<'a>(&'a self, pr_node_id: &'a str)
        -> BoxFuture<'a, Result<(), ScmError>>;

    fn merge_pull_request<'a>(
        &'a self,
        pr_node_id: &'a str,
        expected_head_oid: &'a str,
    ) -> BoxFuture<'a, Result<(), ScmError>>;

    fn get_pr_merge_state_status<'a>(
        &'a self,
        _pr_node_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<String>, ScmError>> {
        Box::pin(async { Ok(None) })
    }

    fn get_pr_state<'a>(
        &'a self,
        repo_slug: &'a str,
        pr_number: u64,
    ) -> BoxFuture<'a, Result<PrState, ScmError>>;

    fn list_pr_checks_and_comments<'a>(
        &'a self,
        repo_slug: &'a str,
        pr_number: u64,
        head_sha: &'a str,
    ) -> BoxFuture<'a, Result<PrChecksAndComments, ScmError>>;
}

pub struct GitHubScmClient {
    http: Client,
    config: GitHubConfig,
}

impl fmt::Debug for GitHubScmClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("GitHubScmClient")
            .field("config", &self.config)
            .finish()
    }
}

impl GitHubScmClient {
    pub fn new(config: GitHubConfig) -> Result<Self, ScmError> {
        if config.bot_token.expose().trim().is_empty() {
            return Err(ScmError::EmptyToken("bot"));
        }
        if config.reviewer_token.expose().trim().is_empty() {
            return Err(ScmError::EmptyToken("reviewer"));
        }
        let http = Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .user_agent("polyclave-hub/0.5")
            .build()
            .map_err(|_| ScmError::Transport { op: "build_client" })?;
        Ok(Self { http, config })
    }

    async fn rest_json<T: for<'de> Deserialize<'de>>(
        &self,
        method: Method,
        path: &str,
        token: &SecretToken,
        body: Option<&impl Serialize>,
        op: &'static str,
    ) -> Result<T, ScmError> {
        let url = format!("{}{}", self.config.rest_base_url, path);
        let mut req = self
            .http
            .request(method, &url)
            .header(header::ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", GITHUB_API_VERSION)
            .bearer_auth(token.expose());
        if let Some(payload) = body {
            req = req.json(payload);
        }
        let resp = req.send().await.map_err(|_| ScmError::Transport { op })?;
        let status = resp.status();
        let bytes = resp.bytes().await.map_err(|_| ScmError::Transport { op })?;
        if !status.is_success() {
            return Err(ScmError::Http {
                op,
                status: status.as_u16(),
                message: self.sanitize_error_body(&bytes),
            });
        }
        serde_json::from_slice(&bytes).map_err(|_| ScmError::InvalidResponse { op })
    }

    async fn rest_optional<T: for<'de> Deserialize<'de>>(
        &self,
        path: &str,
        token: &SecretToken,
        op: &'static str,
    ) -> Result<Option<T>, ScmError> {
        let url = format!("{}{}", self.config.rest_base_url, path);
        let resp = self
            .http
            .get(&url)
            .header(header::ACCEPT, "application/vnd.github+json")
            .header("X-GitHub-Api-Version", GITHUB_API_VERSION)
            .bearer_auth(token.expose())
            .send()
            .await
            .map_err(|_| ScmError::Transport { op })?;
        let status = resp.status();
        if status == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let bytes = resp.bytes().await.map_err(|_| ScmError::Transport { op })?;
        if !status.is_success() {
            return Err(ScmError::Http {
                op,
                status: status.as_u16(),
                message: self.sanitize_error_body(&bytes),
            });
        }
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|_| ScmError::InvalidResponse { op })
    }

    async fn graphql<V: Serialize>(
        &self,
        query: &str,
        variables: V,
        op: &'static str,
    ) -> Result<serde_json::Value, ScmError> {
        let payload = serde_json::json!({
            "query": query,
            "variables": variables,
        });
        let resp = self
            .http
            .post(&self.config.graphql_url)
            .header(header::ACCEPT, "application/vnd.github+json")
            .bearer_auth(self.config.bot_token.expose())
            .json(&payload)
            .send()
            .await
            .map_err(|_| ScmError::Transport { op })?;
        let status = resp.status();
        let bytes = resp.bytes().await.map_err(|_| ScmError::Transport { op })?;
        if !status.is_success() {
            return Err(ScmError::Http {
                op,
                status: status.as_u16(),
                message: self.sanitize_error_body(&bytes),
            });
        }
        let parsed: GraphQlEnvelope =
            serde_json::from_slice(&bytes).map_err(|_| ScmError::InvalidResponse { op })?;
        if let Some(errors) = parsed.errors {
            if !errors.is_empty() {
                let msg = errors
                    .into_iter()
                    .map(|e| e.message)
                    .collect::<Vec<_>>()
                    .join("; ");
                return Err(ScmError::GraphQl {
                    op,
                    message: self.redact_tokens(&msg),
                });
            }
        }
        Ok(parsed.data.unwrap_or(serde_json::Value::Null))
    }

    fn sanitize_error_body(&self, bytes: &[u8]) -> String {
        let raw = serde_json::from_slice::<GitHubApiError>(bytes)
            .map(|e| e.message)
            .unwrap_or_else(|_| String::from_utf8_lossy(bytes).into_owned());
        let trimmed = raw.trim();
        let bounded = &trimmed[..trimmed.floor_char_boundary(trimmed.len().min(256))];
        self.redact_tokens(bounded)
    }

    fn redact_tokens(&self, input: &str) -> String {
        let mut out = input.to_owned();
        for tok in [
            self.config.bot_token.expose(),
            self.config.reviewer_token.expose(),
        ] {
            if !tok.is_empty() {
                out = out.replace(tok, "[REDACTED]");
            }
        }
        out
    }
}

impl ScmClient for GitHubScmClient {
    fn preflight_repo<'a>(
        &'a self,
        repo_slug: &'a str,
        base_branch: &'a str,
    ) -> BoxFuture<'a, Result<PreflightReport, ScmError>> {
        Box::pin(async move {
            let (owner, repo) = parse_repo_slug(repo_slug)?;

            let bot_user: GitHubUser = self
                .rest_json(
                    Method::GET,
                    "/user",
                    &self.config.bot_token,
                    Option::<&()>::None,
                    "get_bot_user",
                )
                .await?;
            let reviewer_user: GitHubUser = self
                .rest_json(
                    Method::GET,
                    "/user",
                    &self.config.reviewer_token,
                    Option::<&()>::None,
                    "get_reviewer_user",
                )
                .await?;

            if bot_user.login.eq_ignore_ascii_case(&reviewer_user.login) {
                return Err(ScmError::SameBotAndReviewerAccount {
                    login: bot_user.login,
                });
            }

            let repo_path = format!("/repos/{owner}/{repo}");
            let repo_info: GitHubRepo = self
                .rest_json(
                    Method::GET,
                    &repo_path,
                    &self.config.bot_token,
                    Option::<&()>::None,
                    "get_repo",
                )
                .await?;

            let prot_path = format!("/repos/{owner}/{repo}/branches/{base_branch}/protection");
            let protection: Option<GitHubBranchProtection> = self
                .rest_optional(&prot_path, &self.config.bot_token, "get_branch_protection")
                .await?;

            let branch_protected = protection.is_some();
            let required_approving_review_count = protection
                .as_ref()
                .and_then(|p| p.required_pull_request_reviews.as_ref())
                .map(|r| r.required_approving_review_count)
                .unwrap_or(0);
            let approval_required = required_approving_review_count >= 1;
            let squash_merge_allowed = repo_info.allow_squash_merge.unwrap_or(false);
            let auto_merge_allowed = repo_info.allow_auto_merge.unwrap_or(false);

            let mut issues = Vec::new();
            if !branch_protected {
                issues.push(format!("branch '{base_branch}' has no protection rules"));
            }
            if !approval_required {
                issues.push(format!(
                    "branch '{base_branch}' does not require at least 1 approving review"
                ));
            }
            if !squash_merge_allowed {
                issues.push("repository does not allow squash merges".to_owned());
            }
            if !auto_merge_allowed {
                issues.push("repository does not have auto-merge enabled".to_owned());
            }

            Ok(PreflightReport {
                bot_login: bot_user.login,
                reviewer_login: reviewer_user.login,
                default_branch: repo_info.default_branch,
                branch_protected,
                approval_required,
                required_approving_review_count,
                squash_merge_allowed,
                auto_merge_allowed,
                passed: issues.is_empty(),
                issues,
            })
        })
    }

    fn open_or_find_pr<'a>(
        &'a self,
        req: OpenPrRequest,
    ) -> BoxFuture<'a, Result<PullRequestInfo, ScmError>> {
        Box::pin(async move {
            let (owner, repo) = parse_repo_slug(&req.repo_slug)?;
            let find_path = format!(
                "/repos/{owner}/{repo}/pulls?state=open&head={owner}:{}&base={}",
                req.head_branch, req.base_branch
            );
            let existing: Vec<GitHubPull> = self
                .rest_json(
                    Method::GET,
                    &find_path,
                    &self.config.bot_token,
                    Option::<&()>::None,
                    "find_pr",
                )
                .await?;
            if let Some(pr) = existing.into_iter().next() {
                return Ok(PullRequestInfo {
                    number: pr.number,
                    node_id: pr.node_id,
                    head_sha: pr.head.sha,
                    state: pr.state,
                    html_url: pr.html_url,
                    created: false,
                });
            }

            let create_path = format!("/repos/{owner}/{repo}/pulls");
            let payload = CreatePullPayload {
                title: &req.title,
                head: &req.head_branch,
                base: &req.base_branch,
                body: &req.body,
            };
            let pr: GitHubPull = self
                .rest_json(
                    Method::POST,
                    &create_path,
                    &self.config.bot_token,
                    Some(&payload),
                    "create_pr",
                )
                .await?;
            Ok(PullRequestInfo {
                number: pr.number,
                node_id: pr.node_id,
                head_sha: pr.head.sha,
                state: pr.state,
                html_url: pr.html_url,
                created: true,
            })
        })
    }

    fn submit_review<'a>(
        &'a self,
        req: SubmitReviewRequest,
    ) -> BoxFuture<'a, Result<SubmittedReview, ScmError>> {
        Box::pin(async move {
            let (owner, repo) = parse_repo_slug(&req.repo_slug)?;
            let marker = format_agent_reviewer_marker(&req.vendor, &req.model, &req.reviewed_sha);
            let formatted_body = if req.body.contains(&marker) {
                req.body
            } else if req.body.trim().is_empty() {
                marker
            } else {
                format!("{marker}\n\n{}", req.body)
            };

            let path = format!("/repos/{owner}/{repo}/pulls/{}/reviews", req.pr_number);
            let payload = CreateReviewPayload {
                commit_id: &req.reviewed_sha,
                body: &formatted_body,
                event: req.event.as_github_str(),
            };
            let created: GitHubReviewResponse = self
                .rest_json(
                    Method::POST,
                    &path,
                    &self.config.reviewer_token,
                    Some(&payload),
                    "submit_review",
                )
                .await?;

            Ok(SubmittedReview {
                id: created.id,
                commit_id: created.commit_id.unwrap_or(req.reviewed_sha),
                state: created.state,
                html_url: created.html_url.unwrap_or_default(),
            })
        })
    }

    fn enable_auto_merge<'a>(
        &'a self,
        pr_node_id: &'a str,
        expected_head_oid: &'a str,
    ) -> BoxFuture<'a, Result<(), ScmError>> {
        Box::pin(async move {
            const MUTATION: &str = r#"
                mutation EnableAutoMerge($pullRequestId: ID!, $expectedHeadOid: GitObjectID!, $mergeMethod: PullRequestMergeMethod!) {
                    enablePullRequestAutoMerge(input: {
                        pullRequestId: $pullRequestId,
                        expectedHeadOid: $expectedHeadOid,
                        mergeMethod: $mergeMethod
                    }) {
                        pullRequest {
                            id
                            autoMergeRequest { enabledAt }
                        }
                    }
                }
            "#;
            self.graphql(
                MUTATION,
                serde_json::json!({
                    "pullRequestId": pr_node_id,
                    "expectedHeadOid": expected_head_oid,
                    "mergeMethod": "SQUASH",
                }),
                "enable_auto_merge",
            )
            .await?;
            Ok(())
        })
    }

    fn disable_auto_merge<'a>(
        &'a self,
        pr_node_id: &'a str,
    ) -> BoxFuture<'a, Result<(), ScmError>> {
        Box::pin(async move {
            const MUTATION: &str = r#"
                mutation DisableAutoMerge($pullRequestId: ID!) {
                    disablePullRequestAutoMerge(input: {
                        pullRequestId: $pullRequestId
                    }) {
                        pullRequest {
                            id
                            autoMergeRequest { enabledAt }
                        }
                    }
                }
            "#;
            self.graphql(
                MUTATION,
                serde_json::json!({
                    "pullRequestId": pr_node_id,
                }),
                "disable_auto_merge",
            )
            .await?;
            Ok(())
        })
    }

    fn merge_pull_request<'a>(
        &'a self,
        pr_node_id: &'a str,
        expected_head_oid: &'a str,
    ) -> BoxFuture<'a, Result<(), ScmError>> {
        Box::pin(async move {
            const MUTATION: &str = r#"
                mutation MergePullRequest($pullRequestId: ID!, $expectedHeadOid: GitObjectID!, $mergeMethod: PullRequestMergeMethod!) {
                    mergePullRequest(input: {
                        pullRequestId: $pullRequestId,
                        expectedHeadOid: $expectedHeadOid,
                        mergeMethod: $mergeMethod
                    }) {
                        pullRequest {
                            id
                            merged
                        }
                    }
                }
            "#;
            self.graphql(
                MUTATION,
                serde_json::json!({
                    "pullRequestId": pr_node_id,
                    "expectedHeadOid": expected_head_oid,
                    "mergeMethod": "SQUASH",
                }),
                "merge_pull_request",
            )
            .await?;
            Ok(())
        })
    }

    fn get_pr_merge_state_status<'a>(
        &'a self,
        pr_node_id: &'a str,
    ) -> BoxFuture<'a, Result<Option<String>, ScmError>> {
        Box::pin(async move {
            const MERGE_STATE_QUERY: &str = r#"
                query PrMergeState($id: ID!) {
                    node(id: $id) {
                        ... on PullRequest {
                            mergeStateStatus
                        }
                    }
                }
            "#;
            let data = self
                .graphql(
                    MERGE_STATE_QUERY,
                    serde_json::json!({ "id": pr_node_id }),
                    "get_pr_merge_state_status",
                )
                .await?;
            Ok(data
                .get("node")
                .and_then(|node| node.get("mergeStateStatus"))
                .and_then(|v| v.as_str())
                .map(str::to_owned))
        })
    }

    fn get_pr_state<'a>(
        &'a self,
        repo_slug: &'a str,
        pr_number: u64,
    ) -> BoxFuture<'a, Result<PrState, ScmError>> {
        Box::pin(async move {
            let (owner, repo) = parse_repo_slug(repo_slug)?;
            let path = format!("/repos/{owner}/{repo}/pulls/{pr_number}");
            let pr: GitHubPull = self
                .rest_json(
                    Method::GET,
                    &path,
                    &self.config.bot_token,
                    Option::<&()>::None,
                    "get_pr_state",
                )
                .await?;
            Ok(PrState {
                number: pr.number,
                node_id: pr.node_id,
                state: pr.state,
                merged: pr.merged.unwrap_or(false),
                mergeable: pr.mergeable,
                mergeable_state: pr.mergeable_state,
                merge_state_status: None,
                head_sha: pr.head.sha,
                auto_merge_enabled: pr.auto_merge.is_some(),
                html_url: pr.html_url,
            })
        })
    }

    fn list_pr_checks_and_comments<'a>(
        &'a self,
        repo_slug: &'a str,
        pr_number: u64,
        head_sha: &'a str,
    ) -> BoxFuture<'a, Result<PrChecksAndComments, ScmError>> {
        Box::pin(async move {
            let (owner, repo) = parse_repo_slug(repo_slug)?;
            let checks_path = format!("/repos/{owner}/{repo}/commits/{head_sha}/check-runs");
            let comments_path = format!("/repos/{owner}/{repo}/issues/{pr_number}/comments");
            let reviews_path = format!("/repos/{owner}/{repo}/pulls/{pr_number}/reviews");

            // GitHub caps a page at 100 check runs. A failing check may sit on a
            // later page, so every page is read before anything is recorded, and
            // `checks_complete` records whether the list really was complete.
            let mut check_runs = Vec::new();
            let mut checks_complete = false;
            let mut page = 1u32;
            while page <= MAX_CHECK_RUN_PAGES {
                let path = format!("{checks_path}?per_page=100&page={page}");
                let checks_resp: GitHubCheckRunsResponse = match self
                    .rest_json(
                        Method::GET,
                        &path,
                        &self.config.bot_token,
                        Option::<&()>::None,
                        "list_check_runs",
                    )
                    .await
                {
                    Ok(resp) => resp,
                    // A later page that cannot be read leaves the list partial.
                    Err(_) if page > 1 => break,
                    Err(err) => return Err(err),
                };
                let total = checks_resp.total_count;
                let received = checks_resp.check_runs.len();
                check_runs.extend(checks_resp.check_runs);
                // `total_count` is authoritative when GitHub sends it; a short
                // page only means "done" when it did not. An empty page before
                // the total is reached leaves the list partial.
                let complete = match total {
                    Some(total) => check_runs.len() as u64 >= total,
                    None => received < 100,
                };
                if complete {
                    checks_complete = true;
                    break;
                }
                if received == 0 {
                    break;
                }
                page += 1;
            }
            let comments_resp: Vec<GitHubIssueComment> = self
                .rest_json(
                    Method::GET,
                    &comments_path,
                    &self.config.bot_token,
                    Option::<&()>::None,
                    "list_issue_comments",
                )
                .await?;
            let reviews_resp: Vec<GitHubReviewResponse> = self
                .rest_json(
                    Method::GET,
                    &reviews_path,
                    &self.config.bot_token,
                    Option::<&()>::None,
                    "list_pr_reviews",
                )
                .await?;

            Ok(PrChecksAndComments {
                checks_complete,
                check_runs: check_runs
                    .into_iter()
                    .map(|c| CheckRunInfo {
                        id: c.id,
                        name: c.name,
                        status: c.status,
                        conclusion: c.conclusion,
                        html_url: c.html_url,
                    })
                    .collect(),
                comments: comments_resp
                    .into_iter()
                    .map(|c| PrCommentInfo {
                        id: c.id,
                        author_login: c.user.map(|u| u.login).unwrap_or_default(),
                        body: c.body.unwrap_or_default(),
                        created_at: c.created_at,
                    })
                    .collect(),
                reviews: reviews_resp
                    .into_iter()
                    .map(|r| {
                        let body = r.body.unwrap_or_default();
                        let is_agent_reviewer = is_agent_reviewer_marker(&body);
                        PrReviewInfo {
                            id: r.id,
                            author_login: r.user.map(|u| u.login).unwrap_or_default(),
                            state: r.state,
                            commit_id: r.commit_id,
                            body,
                            submitted_at: r.submitted_at,
                            is_agent_reviewer,
                        }
                    })
                    .collect(),
            })
        })
    }
}

fn parse_repo_slug(slug: &str) -> Result<(&str, &str), ScmError> {
    let mut parts = slug.split('/');
    let owner = parts.next().unwrap_or_default();
    let repo = parts.next().unwrap_or_default();
    if parts.next().is_some()
        || owner.is_empty()
        || repo.is_empty()
        || owner.contains("..")
        || repo.contains("..")
        || slug.contains(['?', '#', '\\'])
    {
        return Err(ScmError::InvalidRepoSlug(slug.to_owned()));
    }
    Ok((owner, repo))
}

#[derive(Deserialize)]
struct GitHubApiError {
    message: String,
}

#[derive(Deserialize)]
struct GraphQlEnvelope {
    data: Option<serde_json::Value>,
    errors: Option<Vec<GraphQlErrorItem>>,
}

#[derive(Deserialize)]
struct GraphQlErrorItem {
    message: String,
}

#[derive(Deserialize)]
struct GitHubUser {
    login: String,
}

#[derive(Deserialize)]
struct GitHubRepo {
    default_branch: String,
    allow_squash_merge: Option<bool>,
    allow_auto_merge: Option<bool>,
}

#[derive(Deserialize)]
struct GitHubBranchProtection {
    required_pull_request_reviews: Option<GitHubRequiredReviews>,
}

#[derive(Deserialize)]
struct GitHubRequiredReviews {
    #[serde(default)]
    required_approving_review_count: u32,
}

#[derive(Serialize)]
struct CreatePullPayload<'a> {
    title: &'a str,
    head: &'a str,
    base: &'a str,
    body: &'a str,
}

#[derive(Deserialize)]
struct GitHubPull {
    number: u64,
    node_id: String,
    state: String,
    html_url: String,
    head: GitHubPullHead,
    merged: Option<bool>,
    mergeable: Option<bool>,
    mergeable_state: Option<String>,
    auto_merge: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct GitHubPullHead {
    sha: String,
}

#[derive(Serialize)]
struct CreateReviewPayload<'a> {
    commit_id: &'a str,
    body: &'a str,
    event: &'a str,
}

#[derive(Deserialize)]
struct GitHubReviewResponse {
    id: u64,
    state: String,
    commit_id: Option<String>,
    body: Option<String>,
    html_url: Option<String>,
    submitted_at: Option<String>,
    user: Option<GitHubUser>,
}

/// GitHub serves at most 100 checks per page; 50 pages (5,000 checks) is far
/// beyond any real pull request, and reaching it means the list is partial.
const MAX_CHECK_RUN_PAGES: u32 = 50;

#[derive(Deserialize)]
struct GitHubCheckRunsResponse {
    #[serde(default)]
    total_count: Option<u64>,
    check_runs: Vec<GitHubCheckRun>,
}

#[derive(Deserialize)]
struct GitHubCheckRun {
    id: u64,
    name: String,
    status: String,
    conclusion: Option<String>,
    html_url: Option<String>,
}

#[derive(Deserialize)]
struct GitHubIssueComment {
    id: u64,
    body: Option<String>,
    created_at: String,
    user: Option<GitHubUser>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        extract::{Path as AxumPath, Query, State},
        http::{HeaderMap, StatusCode},
        routing::{get, post},
        Json, Router,
    };
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex},
    };
    use tokio::net::TcpListener;

    #[derive(Clone, Default)]
    struct MockGitHubState {
        bot_token: String,
        reviewer_token: String,
        same_user_mode: bool,
        allow_auto_merge: bool,
        allow_squash_merge: bool,
        branch_protected: bool,
        required_approvals: u32,
        prs: Vec<serde_json::Value>,
        reviews: Vec<serde_json::Value>,
        auto_merge_calls: Vec<(String, serde_json::Value)>,
        fail_next_graphql: Option<String>,
    }

    fn bearer(headers: &HeaderMap) -> Option<String> {
        headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "))
            .map(str::to_owned)
    }

    async fn spawn_mock_github(state: Arc<Mutex<MockGitHubState>>) -> String {
        let app = Router::new()
            .route(
                "/user",
                get(
                    |State(st): State<Arc<Mutex<MockGitHubState>>>, headers: HeaderMap| async move {
                        let tok = bearer(&headers).unwrap_or_default();
                        let guard = st.lock().unwrap();
                        if tok == guard.bot_token {
                            (StatusCode::OK, Json(serde_json::json!({ "login": "polyclave-bot" })))
                        } else if tok == guard.reviewer_token {
                            let login = if guard.same_user_mode {
                                "PolyClave-Bot"
                            } else {
                                "polyclave-reviewer"
                            };
                            (StatusCode::OK, Json(serde_json::json!({ "login": login })))
                        } else {
                            (StatusCode::UNAUTHORIZED, Json(serde_json::json!({ "message": "Bad credentials" })))
                        }
                    },
                ),
            )
            .route(
                "/repos/{owner}/{repo}",
                get(|State(st): State<Arc<Mutex<MockGitHubState>>>| async move {
                    let guard = st.lock().unwrap();
                    Json(serde_json::json!({
                        "default_branch": "main",
                        "allow_squash_merge": guard.allow_squash_merge,
                        "allow_auto_merge": guard.allow_auto_merge,
                    }))
                }),
            )
            .route(
                "/repos/{owner}/{repo}/branches/{branch}/protection",
                get(|State(st): State<Arc<Mutex<MockGitHubState>>>| async move {
                    let guard = st.lock().unwrap();
                    if !guard.branch_protected {
                        return (
                            StatusCode::NOT_FOUND,
                            Json(serde_json::json!({ "message": "Branch not protected" })),
                        );
                    }
                    (
                        StatusCode::OK,
                        Json(serde_json::json!({
                            "required_pull_request_reviews": {
                                "required_approving_review_count": guard.required_approvals
                            }
                        })),
                    )
                }),
            )
            .route(
                "/repos/{owner}/{repo}/pulls",
                get(
                    |State(st): State<Arc<Mutex<MockGitHubState>>>,
                     Query(params): Query<HashMap<String, String>>| async move {
                        let guard = st.lock().unwrap();
                        let head = params.get("head").cloned().unwrap_or_default();
                        let matches: Vec<_> = guard
                            .prs
                            .iter()
                            .filter(|pr| {
                                pr.get("head_filter").and_then(|v| v.as_str()) == Some(head.as_str())
                            })
                            .cloned()
                            .collect();
                        Json(serde_json::Value::Array(matches))
                    },
                )
                .post(
                    |State(st): State<Arc<Mutex<MockGitHubState>>>,
                     AxumPath((owner, repo)): AxumPath<(String, String)>,
                     Json(body): Json<serde_json::Value>| async move {
                        let mut guard = st.lock().unwrap();
                        let num = (guard.prs.len() as u64) + 1;
                        let head_branch = body["head"].as_str().unwrap().to_owned();
                        let pr = serde_json::json!({
                            "number": num,
                            "node_id": format!("PR_node_{num}"),
                            "state": "open",
                            "html_url": format!("https://github.com/{owner}/{repo}/pull/{num}"),
                            "head": { "sha": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" },
                            "head_filter": format!("{owner}:{head_branch}"),
                            "merged": false,
                            "mergeable": true,
                            "mergeable_state": "clean",
                            "auto_merge": null
                        });
                        guard.prs.push(pr.clone());
                        (StatusCode::CREATED, Json(pr))
                    },
                ),
            )
            .route(
                "/repos/{owner}/{repo}/pulls/{number}",
                get(
                    |State(st): State<Arc<Mutex<MockGitHubState>>>,
                     AxumPath((_, _, number)): AxumPath<(String, String, u64)>| async move {
                        let guard = st.lock().unwrap();
                        let pr = guard
                            .prs
                            .iter()
                            .find(|p| p["number"].as_u64() == Some(number))
                            .cloned()
                            .unwrap();
                        Json(pr)
                    },
                ),
            )
            .route(
                "/repos/{owner}/{repo}/pulls/{number}/reviews",
                get(|State(st): State<Arc<Mutex<MockGitHubState>>>| async move {
                    let guard = st.lock().unwrap();
                    Json(serde_json::Value::Array(guard.reviews.clone()))
                })
                .post(
                    |State(st): State<Arc<Mutex<MockGitHubState>>>,
                     headers: HeaderMap,
                     Json(body): Json<serde_json::Value>| async move {
                        let tok = bearer(&headers).unwrap_or_default();
                        let mut guard = st.lock().unwrap();
                        // Must be reviewer token, never bot token
                        if tok != guard.reviewer_token {
                            return (
                                StatusCode::FORBIDDEN,
                                Json(serde_json::json!({ "message": "review must use reviewer_token" })),
                            );
                        }
                        let id = (guard.reviews.len() as u64) + 100;
                        let review = serde_json::json!({
                            "id": id,
                            "state": if body["event"] == "APPROVE" { "APPROVED" } else { "CHANGES_REQUESTED" },
                            "commit_id": body["commit_id"],
                            "body": body["body"],
                            "html_url": format!("https://github.com/o/r/pull/1#pullrequestreview-{id}"),
                            "submitted_at": "2026-11-27T00:00:00Z",
                            "user": { "login": "polyclave-reviewer" }
                        });
                        guard.reviews.push(review.clone());
                        (StatusCode::OK, Json(review))
                    },
                ),
            )
            .route(
                "/repos/{owner}/{repo}/commits/{sha}/check-runs",
                get(|| async {
                    Json(serde_json::json!({
                        "check_runs": [
                            {
                                "id": 10,
                                "name": "ci / rust-test",
                                "status": "completed",
                                "conclusion": "success",
                                "html_url": "https://github.com/o/r/runs/10"
                            }
                        ]
                    }))
                }),
            )
            .route(
                "/repos/{owner}/{repo}/issues/{number}/comments",
                get(|| async {
                    Json(serde_json::json!([
                        {
                            "id": 20,
                            "body": "Please also check Windows CI.",
                            "created_at": "2026-11-27T00:01:00Z",
                            "user": { "login": "maintainer" }
                        }
                    ]))
                }),
            )
            .route(
                "/graphql",
                post(
                    |State(st): State<Arc<Mutex<MockGitHubState>>>,
                     headers: HeaderMap,
                     Json(body): Json<serde_json::Value>| async move {
                        let tok = bearer(&headers).unwrap_or_default();
                        let mut guard = st.lock().unwrap();
                        if tok != guard.bot_token {
                            return (
                                StatusCode::FORBIDDEN,
                                Json(serde_json::json!({ "message": "graphql must use bot_token" })),
                            );
                        }
                        if let Some(err) = guard.fail_next_graphql.take() {
                            return (
                                StatusCode::OK,
                                Json(serde_json::json!({
                                    "data": null,
                                    "errors": [{ "message": err }]
                                })),
                            );
                        }
                        let query = body["query"].as_str().unwrap_or_default().to_owned();
                        let op_name = if query.contains("enablePullRequestAutoMerge") {
                            for pr in &mut guard.prs {
                                if pr["node_id"] == body["variables"]["pullRequestId"] {
                                    pr["auto_merge"] = serde_json::json!({ "enabled_at": "2026-11-27T00:00:00Z" });
                                }
                            }
                            "enable"
                        } else {
                            for pr in &mut guard.prs {
                                if pr["node_id"] == body["variables"]["pullRequestId"] {
                                    pr["auto_merge"] = serde_json::Value::Null;
                                }
                            }
                            "disable"
                        };
                        guard
                            .auto_merge_calls
                            .push((op_name.to_owned(), body["variables"].clone()));
                        (
                            StatusCode::OK,
                            Json(serde_json::json!({
                                "data": {
                                    "pullRequest": { "id": body["variables"]["pullRequestId"] }
                                }
                            })),
                        )
                    },
                ),
            )
            .with_state(state);

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn preflight_refuses_same_bot_and_reviewer_account() {
        let state = Arc::new(Mutex::new(MockGitHubState {
            bot_token: "bot-secret-123".into(),
            reviewer_token: "rev-secret-456".into(),
            same_user_mode: true,
            allow_auto_merge: true,
            allow_squash_merge: true,
            branch_protected: true,
            required_approvals: 1,
            ..Default::default()
        }));
        let base = spawn_mock_github(state).await;
        let client = GitHubScmClient::new(
            GitHubConfig::new(
                SecretToken::new("bot-secret-123"),
                SecretToken::new("rev-secret-456"),
            )
            .with_base_urls(&base, format!("{base}/graphql")),
        )
        .unwrap();

        let err = client
            .preflight_repo("owner/repo", "main")
            .await
            .unwrap_err();
        assert!(matches!(err, ScmError::SameBotAndReviewerAccount { .. }));
    }

    #[tokio::test]
    async fn full_pr_lifecycle_review_pinning_auto_merge_and_token_redaction() {
        let bot_tok = "ghp_bot_super_secret_token_999";
        let rev_tok = "ghp_rev_super_secret_token_888";
        let state = Arc::new(Mutex::new(MockGitHubState {
            bot_token: bot_tok.into(),
            reviewer_token: rev_tok.into(),
            same_user_mode: false,
            allow_auto_merge: true,
            allow_squash_merge: true,
            branch_protected: true,
            required_approvals: 1,
            reviews: vec![serde_json::json!({
                "id": 1,
                "state": "COMMENTED",
                "commit_id": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                "body": "Human feedback: please rename helper.",
                "submitted_at": "2026-11-27T00:00:00Z",
                "user": { "login": "human-dev" }
            })],
            ..Default::default()
        }));
        let base = spawn_mock_github(state.clone()).await;
        let cfg = GitHubConfig::new(SecretToken::new(bot_tok), SecretToken::new(rev_tok))
            .with_base_urls(&base, format!("{base}/graphql"));
        let client = GitHubScmClient::new(cfg.clone()).unwrap();

        // 0. Debug/Display never leaks tokens
        let cfg_dbg = format!("{cfg:?}");
        let cli_dbg = format!("{client:?}");
        assert!(!cfg_dbg.contains(bot_tok));
        assert!(!cfg_dbg.contains(rev_tok));
        assert!(!cli_dbg.contains(bot_tok));
        assert!(!cli_dbg.contains(rev_tok));

        // 1. Preflight passes
        let preflight = client.preflight_repo("owner/repo", "main").await.unwrap();
        assert!(preflight.passed);
        assert_eq!(preflight.bot_login, "polyclave-bot");
        assert_eq!(preflight.reviewer_login, "polyclave-reviewer");

        // 2. Open PR, then find existing PR
        let pr1 = client
            .open_or_find_pr(OpenPrRequest {
                repo_slug: "owner/repo".into(),
                head_branch: "polyclave/ra0000001".into(),
                base_branch: "main".into(),
                title: "Stage 2 PR".into(),
                body: "Automated PR".into(),
            })
            .await
            .unwrap();
        assert!(pr1.created);
        assert_eq!(pr1.number, 1);

        let pr2 = client
            .open_or_find_pr(OpenPrRequest {
                repo_slug: "owner/repo".into(),
                head_branch: "polyclave/ra0000001".into(),
                base_branch: "main".into(),
                title: "Stage 2 PR".into(),
                body: "Automated PR".into(),
            })
            .await
            .unwrap();
        assert!(!pr2.created);
        assert_eq!(pr2.node_id, pr1.node_id);

        // 3. Submit review with explicit commit_id and [agent-reviewer ...] marker
        let head_sha = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
        let rev = client
            .submit_review(SubmitReviewRequest {
                repo_slug: "owner/repo".into(),
                pr_number: pr1.number,
                reviewed_sha: head_sha.into(),
                vendor: "claude".into(),
                model: "claude-opus-4-5".into(),
                event: ReviewEvent::Approve,
                body: "LGTM after checking scope.".into(),
            })
            .await
            .unwrap();
        assert_eq!(rev.commit_id, head_sha);
        assert_eq!(rev.state, "APPROVED");

        // 4. Enable auto-merge with expectedHeadOid, then disable
        client
            .enable_auto_merge(&pr1.node_id, head_sha)
            .await
            .unwrap();
        let st_armed = client.get_pr_state("owner/repo", pr1.number).await.unwrap();
        assert!(st_armed.auto_merge_enabled);

        client.disable_auto_merge(&pr1.node_id).await.unwrap();
        let st_disarmed = client.get_pr_state("owner/repo", pr1.number).await.unwrap();
        assert!(!st_disarmed.auto_merge_enabled);

        {
            let guard = state.lock().unwrap();
            assert_eq!(guard.auto_merge_calls.len(), 2);
            assert_eq!(guard.auto_merge_calls[0].0, "enable");
            assert_eq!(
                guard.auto_merge_calls[0].1["expectedHeadOid"],
                serde_json::json!(head_sha)
            );
            assert_eq!(
                guard.auto_merge_calls[0].1["mergeMethod"],
                serde_json::json!("SQUASH")
            );
            assert_eq!(guard.auto_merge_calls[1].0, "disable");
        }

        // 5. GraphQL error propagation (e.g. head OID mismatch)
        state.lock().unwrap().fail_next_graphql =
            Some("Head branch was modified. expectedHeadOid mismatch.".into());
        let gql_err = client
            .enable_auto_merge(&pr1.node_id, head_sha)
            .await
            .unwrap_err();
        assert!(matches!(gql_err, ScmError::GraphQl { .. }));

        // 6. List checks and comments + self-trigger marker classification
        let obs = client
            .list_pr_checks_and_comments("owner/repo", pr1.number, head_sha)
            .await
            .unwrap();
        assert_eq!(obs.check_runs.len(), 1);
        assert_eq!(obs.comments.len(), 1);
        assert_eq!(obs.reviews.len(), 2);
        assert!(!obs.reviews[0].is_agent_reviewer);
        assert!(obs.reviews[1].is_agent_reviewer);
        assert!(obs.reviews[1].body.contains(
            "[agent-reviewer · claude/claude-opus-4-5 · aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa]"
        ));
    }
}
