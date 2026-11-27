//! Deny-by-default cross-device federation and attenuated capability tokens
//! inspired by `google/agentmesh` (Apache-2.0) and Biscuit attenuation semantics.
//! R1 verification and the master SQLite ledger stay strictly on the primary host.
use polyclave_proto::MeshNodeDto;
use reqwest::redirect::Policy;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use zeroize::Zeroizing;

pub fn current_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn is_private_ipv4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || (o[0] == 100 && (64..=127).contains(&o[1]))
}

fn is_private_ipv6(ip: Ipv6Addr) -> bool {
    ip.is_loopback() || ip.is_unique_local() || ip.is_unicast_link_local()
}

/// Ensures mesh endpoints are loopback, RFC1918, Tailscale CGNAT/MagicDNS, or `.local`.
pub fn validate_private_mesh_endpoint(raw: &str) -> Result<reqwest::Url> {
    let url = reqwest::Url::parse(raw.trim()).map_err(|_| Error::Invalid("invalid mesh url"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(Error::Denied("public mesh endpoint rejected"));
    }
    let host = url
        .host_str()
        .ok_or(Error::Invalid("missing mesh host"))?;
    let normalized = host.trim_start_matches('[').trim_end_matches(']');
    if normalized.eq_ignore_ascii_case("localhost")
        || normalized.ends_with(".ts.net")
        || normalized.ends_with(".local")
    {
        return Ok(url);
    }
    if let Ok(ip) = normalized.parse::<IpAddr>() {
        let ok = match ip {
            IpAddr::V4(v4) => is_private_ipv4(v4),
            IpAddr::V6(v6) => is_private_ipv6(v6),
        };
        return if ok {
            Ok(url)
        } else {
            Err(Error::Denied("public mesh endpoint rejected"))
        };
    }
    Err(Error::Denied("public mesh endpoint rejected"))
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("mesh authorization denied: {0}")]
    Denied(&'static str),
    #[error("attenuation cannot widen parent scope")]
    InvalidAttenuation,
    #[error("invalid mesh configuration: {0}")]
    Invalid(&'static str),
    #[error("mesh transport failed")]
    Transport,
    #[error("mesh serialization failed")]
    Serde(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MeshScope {
    pub node_id: String,
    pub allowed_repos: Vec<String>,
    pub allowed_actions: Vec<String>,
    pub expires_at_epoch: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AttenuatedToken {
    pub chain: Vec<MeshScope>,
    pub mac_hex: String,
}

fn chain_mac(root_key: &[u8], chain: &[MeshScope]) -> Result<String> {
    if root_key.len() < 16 {
        return Err(Error::Invalid("root key must be at least 16 bytes"));
    }
    let mut acc = {
        let mut h = Sha256::new();
        h.update(b"polyclave-mesh-v1:");
        h.update(root_key);
        h.finalize().to_vec()
    };
    for scope in chain {
        let payload = serde_json::to_vec(scope)?;
        let mut h = Sha256::new();
        h.update(&acc);
        h.update(b":");
        h.update(&payload);
        acc = h.finalize().to_vec();
    }
    let mut hex = String::with_capacity(acc.len() * 2);
    for b in acc {
        use std::fmt::Write as _;
        let _ = write!(&mut hex, "{b:02x}");
    }
    Ok(hex)
}

fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (&x, &y)| acc | (x ^ y)) == 0
}

impl AttenuatedToken {
    pub fn issue(root_key: &[u8], scope: MeshScope) -> Result<Self> {
        if scope.node_id.trim().is_empty()
            || scope.allowed_repos.is_empty()
            || scope.allowed_actions.is_empty()
        {
            return Err(Error::Invalid(
                "scope must specify node, repos, and actions",
            ));
        }
        let chain = vec![scope];
        let mac_hex = chain_mac(root_key, &chain)?;
        Ok(Self { chain, mac_hex })
    }

    /// Monotonically narrows an existing token: child scope must match `node_id`,
    /// must not exceed `expires_at_epoch`, and its repos/actions must be subsets.
    pub fn attenuate(&self, root_key: &[u8], narrower: MeshScope) -> Result<Self> {
        let parent = self
            .chain
            .last()
            .ok_or(Error::Invalid("empty token chain"))?;
        if narrower.node_id != parent.node_id
            || narrower.expires_at_epoch > parent.expires_at_epoch
            || narrower.allowed_repos.is_empty()
            || narrower.allowed_actions.is_empty()
            || !narrower
                .allowed_repos
                .iter()
                .all(|r| parent.allowed_repos.contains(r))
            || !narrower
                .allowed_actions
                .iter()
                .all(|a| parent.allowed_actions.contains(a))
        {
            return Err(Error::InvalidAttenuation);
        }
        let mut chain = self.chain.clone();
        chain.push(narrower);
        let mac_hex = chain_mac(root_key, &chain)?;
        Ok(Self { chain, mac_hex })
    }

    pub fn verify(&self, root_key: &[u8], now_epoch: u64) -> Result<&MeshScope> {
        if self.chain.is_empty() {
            return Err(Error::Denied("empty token chain"));
        }
        let expected = chain_mac(root_key, &self.chain)?;
        if !ct_eq(expected.as_bytes(), self.mac_hex.as_bytes()) {
            return Err(Error::Denied("invalid token signature"));
        }
        for pair in self.chain.windows(2) {
            let (p, c) = (&pair[0], &pair[1]);
            if c.node_id != p.node_id
                || c.expires_at_epoch > p.expires_at_epoch
                || !c.allowed_repos.iter().all(|r| p.allowed_repos.contains(r))
                || !c
                    .allowed_actions
                    .iter()
                    .all(|a| p.allowed_actions.contains(a))
            {
                return Err(Error::Denied("non-monotonic token attenuation"));
            }
        }
        let effective = self.chain.last().ok_or(Error::Denied("empty chain"))?;
        if now_epoch >= effective.expires_at_epoch {
            return Err(Error::Denied("token expired"));
        }
        Ok(effective)
    }
}

/// Deny-by-default authorization policy for cross-device mesh actions.
pub struct MeshPolicy {
    root_key: Zeroizing<Vec<u8>>,
}

impl MeshPolicy {
    pub fn new(root_key: impl Into<Vec<u8>>) -> Result<Self> {
        let key = Zeroizing::new(root_key.into());
        if key.len() < 16 {
            return Err(Error::Invalid("root key must be at least 16 bytes"));
        }
        Ok(Self { root_key: key })
    }

    pub fn issue(&self, scope: MeshScope) -> Result<AttenuatedToken> {
        AttenuatedToken::issue(&self.root_key, scope)
    }

    pub fn attenuate(
        &self,
        parent: &AttenuatedToken,
        narrower: MeshScope,
    ) -> Result<AttenuatedToken> {
        parent.attenuate(&self.root_key, narrower)
    }

    pub fn authorize(
        &self,
        token: &AttenuatedToken,
        node: &MeshNodeDto,
        repo_cwd: &str,
        action: &str,
        now_epoch: u64,
    ) -> Result<()> {
        if node.status != polyclave_proto::MeshNodeStatus::Online {
            return Err(Error::Denied("node is not online"));
        }
        let scope = token.verify(&self.root_key, now_epoch)?;
        if scope.node_id != node.id {
            return Err(Error::Denied("node mismatch"));
        }
        if !scope.allowed_repos.iter().any(|r| r == repo_cwd)
            || !node.allowed_repos.iter().any(|r| r == repo_cwd)
        {
            return Err(Error::Denied("repository not permitted"));
        }
        if !scope.allowed_actions.iter().any(|a| a == action)
            || !node.allowed_actions.iter().any(|a| a == action)
        {
            return Err(Error::Denied("action not permitted"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RemoteDispatchRequest {
    pub repo_cwd: String,
    pub action: String,
    pub instructions: String,
    pub token: AttenuatedToken,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct RemoteHealth {
    pub node_id: String,
    pub ok: bool,
}

pub struct MeshClient {
    http: reqwest::Client,
}

impl MeshClient {
    pub fn new() -> Result<Self> {
        let http = reqwest::Client::builder()
            .redirect(Policy::none())
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|_| Error::Transport)?;
        Ok(Self { http })
    }

    pub async fn health(&self, endpoint: &str) -> Result<RemoteHealth> {
        let base = validate_private_mesh_endpoint(endpoint)?;
        let url = base
            .join("mesh/health")
            .map_err(|_| Error::Invalid("invalid mesh url"))?;
        let resp = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|_| Error::Transport)?;
        if !resp.status().is_success() {
            return Err(Error::Transport);
        }
        resp.json::<RemoteHealth>()
            .await
            .map_err(|_| Error::Transport)
    }

    pub async fn dispatch(
        &self,
        endpoint: &str,
        req: &RemoteDispatchRequest,
    ) -> Result<serde_json::Value> {
        let base = validate_private_mesh_endpoint(endpoint)?;
        let url = base
            .join("mesh/dispatch")
            .map_err(|_| Error::Invalid("invalid mesh url"))?;
        let resp = self
            .http
            .post(url)
            .json(req)
            .send()
            .await
            .map_err(|_| Error::Transport)?;
        if !resp.status().is_success() {
            return Err(Error::Denied("remote node rejected request"));
        }
        resp.json::<serde_json::Value>()
            .await
            .map_err(|_| Error::Transport)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        routing::{get, post},
        Json, Router,
    };
    use polyclave_proto::MeshNodeStatus;
    use tokio::net::TcpListener;

    fn sample_node(endpoint: String) -> MeshNodeDto {
        MeshNodeDto {
            id: "node-win".into(),
            name: "Win Box".into(),
            endpoint,
            os_family: "windows".into(),
            allowed_repos: vec!["/repo/a".into(), "/repo/b".into()],
            allowed_actions: vec!["worker_run".into(), "read_file".into()],
            status: MeshNodeStatus::Online,
            last_heartbeat_at: None,
            created_at: "2026-11-27T00:00:00Z".into(),
        }
    }

    #[test]
    fn deny_by_default_and_monotonic_attenuation() {
        let policy = MeshPolicy::new(b"0123456789abcdef0123456789abcdef".to_vec()).unwrap();
        let node = sample_node("http://100.64.0.5:4311".into());
        let parent = policy
            .issue(MeshScope {
                node_id: "node-win".into(),
                allowed_repos: vec!["/repo/a".into(), "/repo/b".into()],
                allowed_actions: vec!["worker_run".into(), "read_file".into()],
                expires_at_epoch: 2000,
            })
            .unwrap();
        assert!(policy
            .authorize(&parent, &node, "/repo/a", "worker_run", 1000)
            .is_ok());
        assert!(matches!(
            policy.authorize(&parent, &node, "/repo/c", "worker_run", 1000),
            Err(Error::Denied(_))
        ));

        // Attenuate to /repo/a + read_file only; widening fails.
        assert!(matches!(
            policy.attenuate(
                &parent,
                MeshScope {
                    node_id: "node-win".into(),
                    allowed_repos: vec!["/repo/c".into()],
                    allowed_actions: vec!["read_file".into()],
                    expires_at_epoch: 1500,
                }
            ),
            Err(Error::InvalidAttenuation)
        ));
        let child = policy
            .attenuate(
                &parent,
                MeshScope {
                    node_id: "node-win".into(),
                    allowed_repos: vec!["/repo/a".into()],
                    allowed_actions: vec!["read_file".into()],
                    expires_at_epoch: 1500,
                },
            )
            .unwrap();
        assert!(policy
            .authorize(&child, &node, "/repo/a", "read_file", 1000)
            .is_ok());
        assert!(matches!(
            policy.authorize(&child, &node, "/repo/a", "worker_run", 1000),
            Err(Error::Denied(_))
        ));
        assert!(matches!(
            policy.authorize(&child, &node, "/repo/a", "read_file", 1500),
            Err(Error::Denied(_))
        ));
    }

    #[tokio::test]
    async fn mesh_client_talks_to_mock_peer() {
        let app = Router::new()
            .route(
                "/mesh/health",
                get(|| async {
                    Json(RemoteHealth {
                        node_id: "node-win".into(),
                        ok: true,
                    })
                }),
            )
            .route(
                "/mesh/dispatch",
                post(|Json(req): Json<RemoteDispatchRequest>| async move {
                    Json(serde_json::json!({"accepted": true, "repo": req.repo_cwd}))
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let endpoint = format!("http://{addr}");
        let client = MeshClient::new().unwrap();
        let health = client.health(&endpoint).await.unwrap();
        assert!(health.ok);

        let policy = MeshPolicy::new(b"0123456789abcdef0123456789abcdef".to_vec()).unwrap();
        let token = policy
            .issue(MeshScope {
                node_id: "node-win".into(),
                allowed_repos: vec!["/repo/a".into()],
                allowed_actions: vec!["worker_run".into()],
                expires_at_epoch: 2000,
            })
            .unwrap();
        let res = client
            .dispatch(
                &endpoint,
                &RemoteDispatchRequest {
                    repo_cwd: "/repo/a".into(),
                    action: "worker_run".into(),
                    instructions: "cargo test".into(),
                    token,
                },
            )
            .await
            .unwrap();
        assert_eq!(res["accepted"], true);
    }

    #[test]
    fn rejects_public_mesh_endpoints_and_allows_private_mesh() {
        assert!(matches!(
            validate_private_mesh_endpoint("https://example.com"),
            Err(Error::Denied(_))
        ));
        assert!(matches!(
            validate_private_mesh_endpoint("http://8.8.8.8:4310"),
            Err(Error::Denied(_))
        ));
        assert!(validate_private_mesh_endpoint("http://127.0.0.1:4310").is_ok());
        assert!(validate_private_mesh_endpoint("http://100.64.0.5:4310").is_ok());
        assert!(validate_private_mesh_endpoint("https://winbox.tailnet.ts.net").is_ok());
    }
}
