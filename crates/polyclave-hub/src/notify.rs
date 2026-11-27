//! Pure private, self-hosted notification delivery (`ntfy` and UnifiedPush).
//! Strictly rejects public-internet / third-party IM cloud targets and sends
//! metadata-only wakeups (never conversation text, code, or secrets).
use crate::{Error, Result};
use polyclave_proto::{NotificationChannelDto, NotificationChannelKind};
use reqwest::redirect::Policy;
use serde::{Deserialize, Serialize};
use std::future::Future;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::pin::Pin;
use std::time::Duration;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct NotificationMetadata {
    pub topic: String,
    pub entity_id: String,
    pub title: String,
    pub attention_count: usize,
}

impl NotificationMetadata {
    pub fn sanitized(
        topic: impl Into<String>,
        entity_id: impl Into<String>,
        title: impl Into<String>,
        attention_count: usize,
    ) -> Self {
        let clamp = |s: String, max: usize| -> String {
            let clean: String = s.chars().filter(|c| !c.is_control()).take(max).collect();
            clean
        };
        Self {
            topic: clamp(topic.into(), 48),
            entity_id: clamp(entity_id.into(), 64),
            title: clamp(title.into(), 120),
            attention_count,
        }
    }
}

fn is_private_ipv4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        // Tailscale / CGNAT 100.64.0.0/10
        || (o[0] == 100 && (64..=127).contains(&o[1]))
}

fn is_private_ipv6(ip: Ipv6Addr) -> bool {
    ip.is_loopback() || ip.is_unique_local() || ip.is_unicast_link_local()
}

/// Ensures the endpoint is loopback, RFC1918, Tailscale CGNAT/MagicDNS, or `.local`.
pub fn validate_private_endpoint(raw: &str) -> Result<reqwest::Url> {
    let url = reqwest::Url::parse(raw.trim()).map_err(|_| Error::Policy)?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(Error::Policy);
    }
    let host = url.host_str().ok_or(Error::Policy)?;
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
        return if ok { Ok(url) } else { Err(Error::Policy) };
    }
    Err(Error::Policy)
}

pub type NotifyFuture<'a> = Pin<Box<dyn Future<Output = Result<()>> + Send + 'a>>;

pub trait PrivateNotifier: Send + Sync {
    fn notify<'a>(
        &'a self,
        channel: &'a NotificationChannelDto,
        meta: &'a NotificationMetadata,
    ) -> NotifyFuture<'a>;
}

pub struct HttpPrivateNotifier {
    http: reqwest::Client,
}

impl HttpPrivateNotifier {
    pub fn new() -> Result<Self> {
        let http = reqwest::Client::builder()
            .redirect(Policy::none())
            .timeout(Duration::from_secs(5))
            .build()
            .map_err(|_| Error::Transport)?;
        Ok(Self { http })
    }
}

impl PrivateNotifier for HttpPrivateNotifier {
    fn notify<'a>(
        &'a self,
        channel: &'a NotificationChannelDto,
        meta: &'a NotificationMetadata,
    ) -> NotifyFuture<'a> {
        Box::pin(async move {
            if !channel.enabled {
                return Ok(());
            }
            let base = validate_private_endpoint(&channel.endpoint)?;
            let safe = NotificationMetadata::sanitized(
                meta.topic.clone(),
                meta.entity_id.clone(),
                meta.title.clone(),
                meta.attention_count,
            );
            let target = match channel.kind {
                NotificationChannelKind::Ntfy => base
                    .join(channel.topic.trim_start_matches('/'))
                    .map_err(|_| Error::Policy)?,
                NotificationChannelKind::UnifiedPush => base,
            };
            let resp = self
                .http
                .post(target)
                .header("X-Polyclave-Wakeup", "1")
                .json(&safe)
                .send()
                .await
                .map_err(|_| Error::Transport)?;
            if !resp.status().is_success() {
                return Err(Error::Http {
                    status: resp.status().as_u16(),
                    detail: "private notifier rejected wakeup".into(),
                });
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{routing::post, Json, Router};
    use std::sync::{Arc, Mutex};
    use tokio::net::TcpListener;

    #[test]
    fn rejects_public_endpoints_and_allows_private_tailscale() {
        for bad in [
            "https://api.telegram.org/bot123/sendMessage",
            "https://open.feishu.cn/open-apis/bot/v2/hook/abc",
            "https://ntfy.sh/polyclave",
            "http://8.8.8.8:8080",
            "ftp://127.0.0.1/topic",
        ] {
            assert!(matches!(validate_private_endpoint(bad), Err(Error::Policy)));
        }
        for good in [
            "http://127.0.0.1:8090",
            "http://localhost:8090/up",
            "http://100.64.10.2:8080",
            "https://macmini.tailnet.ts.net/ntfy",
            "http://192.168.1.20:8080",
        ] {
            assert!(validate_private_endpoint(good).is_ok());
        }
    }

    #[tokio::test]
    async fn delivers_metadata_only_to_ntfy_and_unified_push() {
        let seen = Arc::new(Mutex::new(Vec::<(String, NotificationMetadata)>::new()));
        let seen_ntfy = seen.clone();
        let seen_up = seen.clone();
        let app = Router::new()
            .route(
                "/polyclave",
                post(move |Json(m): Json<NotificationMetadata>| {
                    let s = seen_ntfy.clone();
                    async move {
                        s.lock().unwrap().push(("ntfy".into(), m));
                        "ok"
                    }
                }),
            )
            .route(
                "/up",
                post(move |Json(m): Json<NotificationMetadata>| {
                    let s = seen_up.clone();
                    async move {
                        s.lock().unwrap().push(("up".into(), m));
                        "ok"
                    }
                }),
            );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let notifier = HttpPrivateNotifier::new().unwrap();
        let meta = NotificationMetadata::sanitized("run", "r123", "Needs review", 2);
        let ntfy = NotificationChannelDto {
            id: "n1".into(),
            kind: NotificationChannelKind::Ntfy,
            label: "Local ntfy".into(),
            endpoint: format!("http://{addr}"),
            topic: "polyclave".into(),
            enabled: true,
            created_at: "2026-11-27T00:00:00Z".into(),
        };
        let up = NotificationChannelDto {
            id: "n2".into(),
            kind: NotificationChannelKind::UnifiedPush,
            label: "Local UP".into(),
            endpoint: format!("http://{addr}/up"),
            topic: "unused".into(),
            enabled: true,
            created_at: "2026-11-27T00:00:00Z".into(),
        };
        notifier.notify(&ntfy, &meta).await.unwrap();
        notifier.notify(&up, &meta).await.unwrap();
        let guard = seen.lock().unwrap();
        assert_eq!(guard.len(), 2);
        assert_eq!(guard[0].1.entity_id, "r123");
        assert_eq!(guard[1].1.attention_count, 2);
    }
}
