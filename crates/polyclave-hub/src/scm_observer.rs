//! Background SCM Observer polling active `pr_gates`, filtering self-reviews
//! (`[agent-reviewer · …]`), updating `scm_observer_cursors`, and forwarding
//! external events into the Hub inbox (`change_log` + triage).
use crate::{
    scm::{is_agent_reviewer_marker, ScmClient, ScmError},
    Error, Result,
};
use polyclave_proto::{ChangeTopic, PrGateStatus};
use polyclave_store::{ScmCursor, Store};
use serde::Serialize;
use std::{sync::Arc, time::Duration};
use tokio::{sync::watch, task::JoinHandle};

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ObservedEventKind {
    CiFailure,
    HumanReview,
    Comment,
    MergeConflict,
    Merged,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ObservedEvent {
    pub gate_id: String,
    pub repo_slug: String,
    pub pr_number: u32,
    pub kind: ObservedEventKind,
    pub summary: String,
}

#[derive(Clone)]
pub struct ScmObserver {
    store: Store,
    scm: Arc<dyn ScmClient>,
    hub_session_id: Option<String>,
}

fn map_store_err(err: polyclave_store::Error) -> Error {
    match err {
        polyclave_store::Error::ConflictError
        | polyclave_store::Error::Invalid(_)
        | polyclave_store::Error::NotFound => Error::Policy,
        _ => Error::Store,
    }
}

fn map_scm_err(err: ScmError) -> Error {
    match err {
        ScmError::Http {
            status, message, ..
        } => Error::Http {
            status,
            detail: message,
        },
        ScmError::Transport { .. } => Error::Transport,
        ScmError::InvalidResponse { .. } => Error::Protocol,
        _ => Error::Policy,
    }
}

impl ScmObserver {
    pub fn new(store: Store, scm: Arc<dyn ScmClient>, hub_session_id: Option<String>) -> Self {
        Self {
            store,
            scm,
            hub_session_id,
        }
    }

    fn emit_event(&self, event: &ObservedEvent, dedupe_key: &str) {
        if let Some(ref hub_id) = self.hub_session_id {
            let intent = format!("scm:{}:{}:{}", event.repo_slug, event.pr_number, dedupe_key);
            let _ = self.store.hub_enqueue(hub_id, &intent, &event.summary);
        }
        let payload = serde_json::to_value(event).unwrap_or(serde_json::Value::Null);
        let gate_id = event.gate_id.clone();
        let _ = self
            .store
            .write(move |db| Store::append_change(db, ChangeTopic::PrGate, &gate_id, &payload));
    }

    /// Polls all active `pr_gates` once and returns newly observed events.
    pub async fn poll_once(&self) -> Result<Vec<ObservedEvent>> {
        let gates = self.store.list_pr_gates().map_err(map_store_err)?;
        let mut events = Vec::new();

        for gate in gates {
            if matches!(gate.status, PrGateStatus::Merged | PrGateStatus::Failed) {
                continue;
            }
            let Some(pr_number) = gate.pr_number else {
                continue;
            };

            let mut cursor = self
                .store
                .scm_cursor(&gate.repo_slug, pr_number)
                .map_err(map_store_err)?
                .unwrap_or_else(|| ScmCursor {
                    repo_slug: gate.repo_slug.clone(),
                    pr_number,
                    ..Default::default()
                });

            let pr_state = self
                .scm
                .get_pr_state(&gate.repo_slug, pr_number as u64)
                .await
                .map_err(map_scm_err)?;

            if pr_state.merged {
                let _ = self
                    .store
                    .set_pr_gate_status(&gate.id, PrGateStatus::Merged);
                let ev = ObservedEvent {
                    gate_id: gate.id.clone(),
                    repo_slug: gate.repo_slug.clone(),
                    pr_number,
                    kind: ObservedEventKind::Merged,
                    summary: format!("PR #{pr_number} ({}) merged", gate.repo_slug),
                };
                self.emit_event(&ev, "merged");
                events.push(ev);
                self.store.save_scm_cursor(cursor).map_err(map_store_err)?;
                continue;
            }

            if (pr_state.mergeable == Some(false)
                || pr_state.mergeable_state.as_deref() == Some("dirty"))
                && cursor.last_event_etag.as_deref() != Some("conflict")
            {
                cursor.last_event_etag = Some("conflict".to_owned());
                let ev = ObservedEvent {
                    gate_id: gate.id.clone(),
                    repo_slug: gate.repo_slug.clone(),
                    pr_number,
                    kind: ObservedEventKind::MergeConflict,
                    summary: format!("PR #{pr_number} ({}) has merge conflicts", gate.repo_slug),
                };
                self.emit_event(&ev, &format!("conflict:{}", pr_state.head_sha));
                events.push(ev);
            }

            let details = self
                .scm
                .list_pr_checks_and_comments(&gate.repo_slug, pr_number as u64, &pr_state.head_sha)
                .await
                .map_err(map_scm_err)?;

            let mut max_seen_id = cursor.last_comment_id.unwrap_or(0);

            for review in details.reviews {
                // Ignore host-submitted reviews carrying `[agent-reviewer · …]`.
                if review.is_agent_reviewer || is_agent_reviewer_marker(&review.body) {
                    continue;
                }
                let rid = review.id as i64;
                if rid > cursor.last_comment_id.unwrap_or(0) {
                    if rid > max_seen_id {
                        max_seen_id = rid;
                    }
                    let ev = ObservedEvent {
                        gate_id: gate.id.clone(),
                        repo_slug: gate.repo_slug.clone(),
                        pr_number,
                        kind: ObservedEventKind::HumanReview,
                        summary: format!(
                            "PR #{pr_number} ({}) review by {} ({}): {}",
                            gate.repo_slug, review.author_login, review.state, review.body
                        ),
                    };
                    self.emit_event(&ev, &format!("review:{rid}"));
                    events.push(ev);
                }
            }

            for comment in details.comments {
                if is_agent_reviewer_marker(&comment.body) {
                    continue;
                }
                let cid = comment.id as i64;
                if cid > cursor.last_comment_id.unwrap_or(0) {
                    if cid > max_seen_id {
                        max_seen_id = cid;
                    }
                    let ev = ObservedEvent {
                        gate_id: gate.id.clone(),
                        repo_slug: gate.repo_slug.clone(),
                        pr_number,
                        kind: ObservedEventKind::Comment,
                        summary: format!(
                            "PR #{pr_number} ({}) comment by {}: {}",
                            gate.repo_slug, comment.author_login, comment.body
                        ),
                    };
                    self.emit_event(&ev, &format!("comment:{cid}"));
                    events.push(ev);
                }
            }

            if max_seen_id > cursor.last_comment_id.unwrap_or(0) {
                cursor.last_comment_id = Some(max_seen_id);
            }

            if cursor.last_check_sha.as_deref() != Some(&pr_state.head_sha) {
                for check in details.check_runs {
                    if matches!(
                        check.conclusion.as_deref(),
                        Some("failure" | "timed_out" | "action_required")
                    ) {
                        let ev = ObservedEvent {
                            gate_id: gate.id.clone(),
                            repo_slug: gate.repo_slug.clone(),
                            pr_number,
                            kind: ObservedEventKind::CiFailure,
                            summary: format!(
                                "PR #{pr_number} ({}) check '{}' failed ({:?}) at {}",
                                gate.repo_slug, check.name, check.conclusion, pr_state.head_sha
                            ),
                        };
                        self.emit_event(&ev, &format!("ci:{}:{}", check.id, pr_state.head_sha));
                        events.push(ev);
                    }
                }
                cursor.last_check_sha = Some(pr_state.head_sha);
            }

            self.store.save_scm_cursor(cursor).map_err(map_store_err)?;
        }

        Ok(events)
    }

    /// Spawns the background observer loop until `shutdown` signals `true`.
    pub fn spawn(self, interval: Duration, mut shutdown: watch::Receiver<bool>) -> JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                if *shutdown.borrow() {
                    break;
                }
                let _ = self.poll_once().await;
                tokio::select! {
                    _ = tokio::time::sleep(interval) => {}
                    changed = shutdown.changed() => {
                        if changed.is_err() || *shutdown.borrow() {
                            break;
                        }
                    }
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scm::{GitHubConfig, GitHubScmClient, SecretToken};
    use axum::{extract::State, routing::get, Json, Router};
    use polyclave_store::NewPrGate;
    use serde_json::json;
    use std::sync::atomic::{AtomicBool, Ordering};
    use tokio::net::TcpListener;

    struct ObserverGhState {
        merged: AtomicBool,
    }

    async fn spawn_observer_gh() -> (String, Arc<ObserverGhState>, tokio::task::JoinHandle<()>) {
        let state = Arc::new(ObserverGhState {
            merged: AtomicBool::new(false),
        });
        let router = Router::new()
            .route(
                "/repos/{owner}/{repo}/pulls/{num}",
                get(|State(st): State<Arc<ObserverGhState>>| async move {
                    let merged = st.merged.load(Ordering::SeqCst);
                    Json(json!({
                        "number": 42,
                        "node_id": "PR_node_42",
                        "state": if merged { "closed" } else { "open" },
                        "merged": merged,
                        "mergeable": true,
                        "mergeable_state": "clean",
                        "html_url": "https://github.com/org/repo/pull/42",
                        "head": { "sha": "sha_abc" },
                        "auto_merge": null
                    }))
                }),
            )
            .route(
                "/repos/{owner}/{repo}/commits/{sha}/check-runs",
                get(|| async {
                    Json(json!({
                        "check_runs": [
                            { "id": 101, "name": "ci/test", "status": "completed", "conclusion": "failure", "html_url": null }
                        ]
                    }))
                }),
            )
            .route(
                "/repos/{owner}/{repo}/issues/{num}/comments",
                get(|| async {
                    Json(json!([
                        {
                            "id": 201,
                            "user": { "login": "alice" },
                            "body": "Please add a doc comment",
                            "created_at": "2026-11-27T00:00:00Z"
                        }
                    ]))
                }),
            )
            .route(
                "/repos/{owner}/{repo}/pulls/{num}/reviews",
                get(|| async {
                    Json(json!([
                        {
                            "id": 301,
                            "user": { "login": "polyclave-reviewer" },
                            "state": "APPROVED",
                            "commit_id": "sha_abc",
                            "body": "[agent-reviewer · claude/claude-sonnet-4-6 · sha_abc]\nLGTM",
                            "submitted_at": "2026-11-27T00:00:00Z"
                        },
                        {
                            "id": 302,
                            "user": { "login": "bob" },
                            "state": "CHANGES_REQUESTED",
                            "commit_id": "sha_abc",
                            "body": "Fix edge case",
                            "submitted_at": "2026-11-27T00:01:00Z"
                        }
                    ]))
                }),
            )
            .with_state(state.clone());

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        (format!("http://{addr}"), state, handle)
    }

    #[tokio::test]
    async fn observer_filters_agent_self_reviews_updates_cursors_and_tracks_merge() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("ledger")).unwrap();
        store
            .write(|db| {
                db.execute_batch(
                    "INSERT INTO provider_profiles(id,display_name,kind,models_json,created_at)
                     VALUES('p','test','codex_cli','[]','now');
                     INSERT INTO sessions(id,name,role,repo_cwd,profile_id,provider_model,created_at)
                     VALUES('hub','hub','hub','/tmp/repo','p','test','now');
                     INSERT INTO sessions(id,name,role,repo_cwd,profile_id,provider_model,created_at)
                     VALUES('w1','w1','worker','/tmp/repo','p','gpt-5','now');
                     INSERT INTO tickets(id,hub_session_id,repo_cwd,title,mandate_json,source_kind,max_runs,runs_used,status,created_at,updated_at)
                     VALUES('t1','hub','/tmp/repo','t1','{}','explicit_user',4,1,'active','now','now');
                     INSERT INTO runs(id,ticket_id,hub_session_id,worker_session_id,request_id,repo_cwd,vendor,model,instructions,mirror_remote,base_branch,base_sha,status,created_at,updated_at)
                     VALUES('run1','t1','hub','w1','req1','/tmp/repo','codex','gpt-5','do it','origin','main','base','done','now','now');",
                )?;
                Ok(())
            })
            .unwrap();

        let gate = store
            .create_pr_gate(NewPrGate {
                run_id: "run1".into(),
                repo_slug: "org/repo".into(),
                branch: "feat/x".into(),
                base_sha: "base".into(),
                head_sha: "sha_abc".into(),
                touched_paths: vec!["src/**".into()],
            })
            .unwrap();
        store
            .record_pr_opened(&gate.id, 42, "https://github.com/org/repo/pull/42")
            .unwrap();

        let (base_url, gh_state, server) = spawn_observer_gh().await;
        let scm = Arc::new(
            GitHubScmClient::new(
                GitHubConfig::new(SecretToken::new("bot-tok"), SecretToken::new("rev-tok"))
                    .with_base_urls(&base_url, format!("{base_url}/graphql")),
            )
            .unwrap(),
        );
        let observer = ScmObserver::new(store.clone(), scm, Some("hub".into()));

        // First poll: sees 1 human review, 1 human comment, 1 CI failure; ignores agent self-review
        let events = observer.poll_once().await.unwrap();
        assert_eq!(events.len(), 3);
        assert!(events
            .iter()
            .any(|e| e.kind == ObservedEventKind::HumanReview));
        assert!(events.iter().any(|e| e.kind == ObservedEventKind::Comment));
        assert!(events
            .iter()
            .any(|e| e.kind == ObservedEventKind::CiFailure));
        assert!(!events
            .iter()
            .any(|e| e.summary.contains("polyclave-reviewer")));

        // Cursor persisted; second poll emits nothing
        let events2 = observer.poll_once().await.unwrap();
        assert!(events2.is_empty());

        // Mark merged on GitHub; third poll transitions gate to Merged
        gh_state.merged.store(true, Ordering::SeqCst);
        let events3 = observer.poll_once().await.unwrap();
        assert_eq!(events3.len(), 1);
        assert_eq!(events3[0].kind, ObservedEventKind::Merged);
        assert_eq!(
            store.pr_gate(&gate.id).unwrap().status,
            PrGateStatus::Merged
        );

        server.abort();
    }
}
