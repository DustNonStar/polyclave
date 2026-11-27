//! Pure private, self-hosted notification delivery (`ntfy` and UnifiedPush).
//! Strictly rejects public-internet / third-party IM cloud targets and sends
//! metadata-only wakeups (never conversation text, code, or secrets).
use crate::{Error, Result};
use polyclave_proto::{NotificationChannelDto, NotificationChannelKind};
use polyclave_store::{Changes, Store};
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

/// Maps high-attention `change_log` events to metadata-only wakeup notifications.
/// Never copies conversation text, diffs, code, or secrets.
pub fn classify_attention_change(
    topic: &str,
    entity_id: &str,
    payload: &serde_json::Value,
) -> Option<NotificationMetadata> {
    // A `pr_gate` observation carries `kind`, not `status`: the SCM observer
    // records what it saw on GitHub, and those kinds are exactly what needs a
    // look. Only the kind is used, never the summary text.
    if topic == "pr_gate" {
        if let Some(kind) = payload.get("kind").and_then(|v| v.as_str()) {
            let title = match kind {
                "ci_failure" => "A PR check failed",
                "merge_conflict" => "A PR has merge conflicts",
                "human_review" => "A reviewer left a pull-request review",
                _ => return None,
            };
            return Some(NotificationMetadata::sanitized(topic, entity_id, title, 1));
        }
    }
    let status = payload.get("status").and_then(|v| v.as_str())?;
    let title = match (topic, status) {
        ("run", "needs_review") => "Run needs review",
        ("run", "failed") => "Run failed",
        ("ticket", "blocked_on_user") => "Ticket awaiting your input",
        ("council", "awaiting_user") => "Council awaiting decision",
        ("pr_gate", "failed" | "blocked") => "PR gate needs attention",
        _ => return None,
    };
    Some(NotificationMetadata::sanitized(topic, entity_id, title, 1))
}

/// Polls `change_log` after `after_seq` and sends metadata-only wakeups to all
/// enabled private notification channels. Returns `(next_cursor, delivered_count)`.
pub async fn dispatch_pending_notifications(
    store: &Store,
    notifier: &dyn PrivateNotifier,
    after_seq: i64,
) -> Result<(i64, usize)> {
    let changes = store.changes(Some(after_seq)).map_err(|_| Error::Store)?;
    let events = match changes {
        Changes::ResyncRequired => {
            let max_seq: i64 = store
                .read(|db| {
                    db.query_row("SELECT COALESCE(MAX(seq), 0) FROM change_log", [], |r| {
                        r.get(0)
                    })
                    .map_err(Into::into)
                })
                .map_err(|_| Error::Store)?;
            return Ok((max_seq, 0));
        }
        Changes::Events(events) => events,
    };
    let next_seq = events.last().map(|e| e.seq).unwrap_or(after_seq);
    let mut alerts = Vec::new();
    for ev in events {
        let topic_val = serde_json::to_value(ev.topic).map_err(|_| Error::Store)?;
        let Some(topic_str) = topic_val.as_str() else {
            continue;
        };
        if let Some(meta) = classify_attention_change(topic_str, &ev.entity_id, &ev.payload) {
            alerts.push(meta);
        }
    }
    if alerts.is_empty() {
        return Ok((next_seq, 0));
    }
    let channels = store
        .list_notification_channels()
        .map_err(|_| Error::Store)?;
    let enabled: Vec<_> = channels.into_iter().filter(|c| c.enabled).collect();
    if enabled.is_empty() {
        return Ok((next_seq, 0));
    }
    let mut delivered = 0usize;
    for meta in &alerts {
        for ch in &enabled {
            if notifier.notify(ch, meta).await.is_ok() {
                delivered += 1;
            }
        }
    }
    Ok((next_seq, delivered))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{routing::post, Json, Router};
    use std::sync::{Arc, Mutex};
    use tokio::net::TcpListener;

    #[test]
    fn observed_pr_gate_kinds_wake_the_phone_without_any_payload_text() {
        let summary = "PR #42 (org/repo) check 'ci/test' failed (Some(\"failure\")) at abc";
        for (kind, expected) in [
            ("ci_failure", "A PR check failed"),
            ("merge_conflict", "A PR has merge conflicts"),
            ("human_review", "A reviewer left a pull-request review"),
        ] {
            let payload = serde_json::json!({"kind": kind, "summary": summary});
            let meta = classify_attention_change("pr_gate", "gate-1", &payload).unwrap();
            assert_eq!(meta.title, expected);
            assert_eq!(meta.topic, "pr_gate");
            assert_eq!(meta.entity_id, "gate-1");
            assert!(!meta.title.contains("PR #42"), "no event text leaks out");
        }
        // A plain comment is not an attention event, and neither is an unknown kind.
        for payload in [
            serde_json::json!({"kind": "comment"}),
            serde_json::json!({"kind": "merged"}),
            serde_json::json!({}),
        ] {
            assert!(classify_attention_change("pr_gate", "gate-1", &payload).is_none());
        }
        // The status-based paths keep working.
        assert_eq!(
            classify_attention_change("run", "r1", &serde_json::json!({"status": "needs_review"}))
                .unwrap()
                .title,
            "Run needs review"
        );
    }

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

    #[test]
    fn classify_attention_change_only_emits_for_attention_states() {
        let secret_payload = serde_json::json!({
            "status": "needs_review",
            "instructions": "super secret prompt",
            "detail": "private diff"
        });
        let meta = classify_attention_change("run", "r999", &secret_payload).unwrap();
        assert_eq!(meta.topic, "run");
        assert_eq!(meta.entity_id, "r999");
        assert_eq!(meta.title, "Run needs review");

        assert!(classify_attention_change(
            "run",
            "r999",
            &serde_json::json!({"status": "running"})
        )
        .is_none());
        assert!(classify_attention_change(
            "message",
            "m1",
            &serde_json::json!({"status": "needs_review"})
        )
        .is_none());
        assert!(classify_attention_change(
            "council",
            "c1",
            &serde_json::json!({"status": "awaiting_user"})
        )
        .is_some());
    }

    #[tokio::test]
    async fn dispatch_pending_notifications_advances_cursor_and_sends_metadata_only() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("notify.sqlite")).unwrap();
        let seen = Arc::new(Mutex::new(Vec::<NotificationMetadata>::new()));
        let seen_c = seen.clone();
        let app = Router::new().route(
            "/poly",
            post(move |Json(m): Json<NotificationMetadata>| {
                let s = seen_c.clone();
                async move {
                    s.lock().unwrap().push(m);
                    "ok"
                }
            }),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        store
            .upsert_notification_channel(polyclave_store::stage3::NewNotificationChannel {
                id: None,
                kind: NotificationChannelKind::Ntfy,
                label: "Local".into(),
                endpoint: format!("http://{addr}"),
                topic: "poly".into(),
                enabled: true,
            })
            .unwrap();
        store
            .write(|db| {
                Store::append_change(
                    db,
                    polyclave_proto::ChangeTopic::Run,
                    "r777",
                    &serde_json::json!({"status": "needs_review", "text": "secret"}),
                )
            })
            .unwrap();

        let notifier = HttpPrivateNotifier::new().unwrap();
        let (cursor, delivered) = dispatch_pending_notifications(&store, &notifier, 0)
            .await
            .unwrap();
        assert_eq!(delivered, 1);
        assert!(cursor > 0);
        assert_eq!(seen.lock().unwrap().len(), 1);
        assert_eq!(seen.lock().unwrap()[0].entity_id, "r777");

        // Second tick at the updated cursor does not re-send.
        let (cursor2, delivered2) = dispatch_pending_notifications(&store, &notifier, cursor)
            .await
            .unwrap();
        assert_eq!(cursor2, cursor);
        assert_eq!(delivered2, 0);
    }
}
