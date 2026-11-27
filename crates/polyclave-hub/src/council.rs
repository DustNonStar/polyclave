//! Multi-model blind-review council engine with checkpoint replay and escalation.
use crate::{
    provider::{ChatMessage, HubModelProvider, HubRequest, ProviderEvent, Role},
    Error, Result,
};
use futures_util::future::join_all;
use polyclave_proto::{
    CouncilDto, CouncilLevel, CouncilProposalDto, CouncilStatus, PlanningPreference,
    ProposalPayload, Vendor,
};
use polyclave_store::{NewCouncil, NewProposal, Store};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::sync::Arc;

const BLIND_LABELS: &[&str] = &["方案甲", "方案乙", "方案丙", "方案丁", "方案戊"];
const MAX_SCHEMA_RETRIES: usize = 2;

#[derive(Clone)]
pub struct CouncilParticipant {
    pub vendor: Vendor,
    pub model: String,
    pub provider: Arc<dyn HubModelProvider>,
}

#[derive(Clone)]
pub struct StartCouncilRequest {
    pub council_id: Option<String>,
    pub hub_session_id: Option<String>,
    pub ticket_id: Option<String>,
    pub repo_cwd: String,
    pub level: Option<CouncilLevel>,
    pub preference: PlanningPreference,
    pub prompt: String,
    pub participants: Vec<CouncilParticipant>,
}

fn map_store_err(err: polyclave_store::Error) -> Error {
    match err {
        polyclave_store::Error::ConflictError | polyclave_store::Error::BudgetExceededError => {
            Error::Policy
        }
        polyclave_store::Error::Invalid(_) | polyclave_store::Error::NotFound => Error::Policy,
        _ => Error::Store,
    }
}

fn vendor_slug(vendor: Vendor) -> &'static str {
    match vendor {
        Vendor::Codex => "codex",
        Vendor::Claude => "claude",
        Vendor::Agy => "agy",
        Vendor::Dsh => "dsh",
    }
}

fn compute_input_hash(
    council_id: &str,
    level: CouncilLevel,
    repo_cwd: &str,
    prompt: &str,
    vendor: Vendor,
    model: &str,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(council_id.as_bytes());
    hasher.update(b"\0");
    hasher.update(format!("{level:?}").as_bytes());
    hasher.update(b"\0");
    hasher.update(repo_cwd.as_bytes());
    hasher.update(b"\0");
    hasher.update(prompt.as_bytes());
    hasher.update(b"\0");
    hasher.update(vendor_slug(vendor).as_bytes());
    hasher.update(b"\0");
    hasher.update(model.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn assign_blind_labels(council_id: &str, count: usize) -> Vec<String> {
    let mut labels: Vec<String> = (0..count)
        .map(|i| {
            BLIND_LABELS
                .get(i)
                .map(|s| (*s).to_owned())
                .unwrap_or_else(|| format!("方案{}", i + 1))
        })
        .collect();
    if labels.len() <= 1 {
        return labels;
    }
    let digest = Sha256::digest(council_id.as_bytes());
    for i in (1..labels.len()).rev() {
        let byte = digest[i % digest.len()] as usize;
        let j = byte % (i + 1);
        labels.swap(i, j);
    }
    labels
}

fn extract_json_candidate(raw: &str) -> &str {
    let trimmed = raw.trim();
    if let Some(rest) = trimmed.strip_prefix("```json") {
        if let Some(inner) = rest.strip_suffix("```") {
            return inner.trim();
        }
    }
    if let Some(rest) = trimmed.strip_prefix("```") {
        if let Some(inner) = rest.strip_suffix("```") {
            return inner.trim();
        }
    }
    trimmed
}

fn parse_proposal_payload(raw: &str) -> std::result::Result<ProposalPayload, String> {
    let candidate = extract_json_candidate(raw);
    let val: Value = serde_json::from_str(candidate).map_err(|e| format!("invalid JSON: {e}"))?;
    let payload: ProposalPayload =
        serde_json::from_value(val).map_err(|e| format!("schema mismatch: {e}"))?;
    if payload.summary.trim().is_empty() || payload.approach.trim().is_empty() {
        return Err("summary and approach must be non-empty".to_owned());
    }
    Ok(payload)
}

fn system_prompt() -> &'static str {
    "You are an independent technical planner participating in a blind architecture council. \
Respond ONLY with a valid JSON object matching this exact schema (no markdown commentary): \
{\"summary\": string, \"approach\": string, \"touched_paths\": string[], \
\"public_interface_changes\": bool, \"new_dependencies\": string[], \
\"security_sensitive\": bool, \"beyond_request\": bool, \"open_questions\": string[], \
\"pros\": string[], \"cons\": string[]}. \
Do not mention your vendor or model name anywhere in the JSON."
}

async fn run_isolated_participant(
    participant: &CouncilParticipant,
    repo_cwd: &str,
    level: CouncilLevel,
    prompt: &str,
) -> std::result::Result<ProposalPayload, String> {
    let user_msg = format!("Council level: {level:?}\nRepository: {repo_cwd}\n\nTask:\n{prompt}");
    let mut messages = vec![
        ChatMessage::text(Role::System, system_prompt()),
        ChatMessage::text(Role::User, user_msg),
    ];

    let mut last_err = String::new();
    for attempt in 0..=MAX_SCHEMA_RETRIES {
        let (tx, _rx) = tokio::sync::broadcast::channel::<ProviderEvent>(32);
        let req = HubRequest {
            model: participant.model.clone(),
            messages: messages.clone(),
            tools: Vec::new(),
        };
        let reply = participant
            .provider
            .complete(req, tx)
            .await
            .map_err(|e| format!("provider error on attempt {}: {e}", attempt + 1))?;

        match parse_proposal_payload(&reply.text) {
            Ok(payload) => return Ok(payload),
            Err(err) => {
                last_err = err.clone();
                if attempt < MAX_SCHEMA_RETRIES {
                    messages.push(ChatMessage::text(Role::Assistant, reply.text));
                    messages.push(ChatMessage::text(
                        Role::User,
                        format!(
                            "Your previous response failed ProposalPayload schema validation ({err}). \
Please output ONLY valid JSON matching the required ProposalPayload schema."
                        ),
                    ));
                }
            }
        }
    }
    Err(format!(
        "failed ProposalPayload validation after {} attempts: {last_err}",
        MAX_SCHEMA_RETRIES + 1
    ))
}

/// Evaluates whether a council requires user escalation based on its level and
/// the collected blind proposals.
pub fn evaluate_escalations(level: CouncilLevel, proposals: &[CouncilProposalDto]) -> Vec<String> {
    let mut reasons = Vec::new();
    if level == CouncilLevel::Direction {
        reasons.push("direction-level council requires user decision".to_owned());
    }
    if proposals.len() >= 2 {
        let min_paths = proposals
            .iter()
            .map(|p| p.payload.touched_paths.len())
            .min()
            .unwrap_or(0);
        let max_paths = proposals
            .iter()
            .map(|p| p.payload.touched_paths.len())
            .max()
            .unwrap_or(0);
        if max_paths > min_paths.saturating_mul(3) {
            reasons.push(format!(
                "touched_paths count delta exceeds 3x across proposals (min={min_paths}, max={max_paths})"
            ));
        }
    }
    for prop in proposals {
        if prop.payload.security_sensitive {
            reasons.push(format!("{} flagged security_sensitive", prop.label));
        }
        if prop
            .payload
            .touched_paths
            .iter()
            .any(|p| p == ".github" || p.starts_with(".github/") || p.contains("/.github/"))
        {
            reasons.push(format!(
                "{} touches .github/ workflows or config",
                prop.label
            ));
        }
        if prop.payload.public_interface_changes {
            reasons.push(format!(
                "{} introduces public_interface_changes",
                prop.label
            ));
        }
        if !prop.payload.open_questions.is_empty() {
            reasons.push(format!(
                "{} has {} open question(s)",
                prop.label,
                prop.payload.open_questions.len()
            ));
        }
        if prop.payload.beyond_request {
            reasons.push(format!("{} flagged beyond_request", prop.label));
        }
    }
    reasons
}

fn select_by_preference(
    preference: PlanningPreference,
    proposals: &[CouncilProposalDto],
) -> Option<String> {
    proposals
        .iter()
        .min_by(|a, b| {
            let key_a = preference_key(preference, a);
            let key_b = preference_key(preference, b);
            key_a.cmp(&key_b).then_with(|| a.label.cmp(&b.label))
        })
        .map(|p| p.label.clone())
}

fn preference_key(
    preference: PlanningPreference,
    prop: &CouncilProposalDto,
) -> (usize, usize, usize) {
    let paths = prop.payload.touched_paths.len();
    let deps = prop.payload.new_dependencies.len();
    let cons = prop.payload.cons.len();
    let pros = prop.payload.pros.len();
    match preference {
        PlanningPreference::Conservative => (paths + deps * 3, cons, usize::MAX - pros),
        PlanningPreference::Thorough => (cons + deps, usize::MAX - pros, usize::MAX - paths),
        PlanningPreference::Balanced => (paths + deps * 2 + cons, usize::MAX - pros, 0),
    }
}

fn format_blind_summary(council: &CouncilDto) -> String {
    let mut out = format!(
        "Council {} requires user decision ({:?}).\nEscalation reasons: {}\n\nProposals:",
        council.id,
        council.level,
        if council.escalation_reasons.is_empty() {
            "none".to_owned()
        } else {
            council.escalation_reasons.join("; ")
        }
    );
    for p in &council.proposals {
        out.push_str(&format!(
            "\n- {}: {} (touched_paths: {})",
            p.label,
            p.payload.summary,
            p.payload.touched_paths.len()
        ));
    }
    out
}

/// Starts (or resumes from checkpoints) a blind multi-model council.
pub async fn start_council(store: &Store, req: StartCouncilRequest) -> Result<CouncilDto> {
    if req.prompt.trim().is_empty() || req.repo_cwd.trim().is_empty() || req.participants.is_empty()
    {
        return Err(Error::Policy);
    }
    if store.pause_state().map_err(map_store_err)?.paused {
        return Err(Error::Policy);
    }

    let level = req.level.unwrap_or(CouncilLevel::Direction);
    let council = if let Some(ref existing_id) = req.council_id {
        let existing = store.council(existing_id).map_err(map_store_err)?;
        if existing.status != CouncilStatus::Running {
            return Ok(existing);
        }
        existing
    } else {
        store
            .create_council(NewCouncil {
                ticket_id: req.ticket_id.clone(),
                repo_cwd: req.repo_cwd.clone(),
                level,
                preference: req.preference,
                prompt: req.prompt.clone(),
            })
            .map_err(map_store_err)?
    };

    let labels = assign_blind_labels(&council.id, req.participants.len());

    let futs = req.participants.iter().zip(labels.iter()).enumerate().map(
        |(idx, (participant, label))| {
            let store = store.clone();
            let council_id = council.id.clone();
            let repo_cwd = req.repo_cwd.clone();
            let prompt = req.prompt.clone();
            let label = label.clone();
            let participant = participant.clone();
            async move {
                let step_key = format!(
                    "proposal:{idx}:{}:{}",
                    vendor_slug(participant.vendor),
                    participant.model
                );
                let input_hash = compute_input_hash(
                    &council_id,
                    level,
                    &repo_cwd,
                    &prompt,
                    participant.vendor,
                    &participant.model,
                );

                if let Some(cp) = store
                    .council_checkpoint(&council_id, &step_key)
                    .map_err(|e| format!("checkpoint read error: {e}"))?
                {
                    if cp.input_hash == input_hash {
                        let payload: ProposalPayload = serde_json::from_value(cp.output)
                            .map_err(|e| format!("corrupt checkpoint payload: {e}"))?;
                        return Ok((
                            label,
                            participant.vendor,
                            participant.model,
                            payload,
                            input_hash,
                        ));
                    }
                }

                let payload =
                    run_isolated_participant(&participant, &repo_cwd, level, &prompt).await?;
                let val = serde_json::to_value(&payload)
                    .map_err(|e| format!("serialize payload error: {e}"))?;
                store
                    .record_council_checkpoint(&council_id, &step_key, &input_hash, val)
                    .map_err(|e| format!("checkpoint write error: {e}"))?;
                Ok((
                    label,
                    participant.vendor,
                    participant.model,
                    payload,
                    input_hash,
                ))
            }
        },
    );

    let results = join_all(futs).await;
    let mut failure_reasons = Vec::new();
    let mut added_count = 0usize;

    for res in results {
        match res {
            Ok((label, vendor, model, payload, input_hash)) => {
                store
                    .add_council_proposal(
                        &council.id,
                        NewProposal {
                            label,
                            vendor,
                            model,
                            payload,
                            input_hash,
                        },
                    )
                    .map_err(map_store_err)?;
                added_count += 1;
            }
            Err(err) => {
                failure_reasons.push(err);
            }
        }
    }

    if added_count == 0 {
        return store
            .update_council_status(&council.id, CouncilStatus::Failed, Some(failure_reasons))
            .map_err(map_store_err);
    }

    let current = store.council(&council.id).map_err(map_store_err)?;
    let mut escalation_reasons = evaluate_escalations(level, &current.proposals);
    escalation_reasons.extend(failure_reasons);

    if level == CouncilLevel::Direction || !escalation_reasons.is_empty() {
        let awaiting = store
            .update_council_status(
                &council.id,
                CouncilStatus::AwaitingUser,
                Some(escalation_reasons),
            )
            .map_err(map_store_err)?;
        if let Some(hub_id) = req.hub_session_id {
            let intent = format!("council:{}:awaiting", awaiting.id);
            let _ = store.hub_enqueue(&hub_id, &intent, &format_blind_summary(&awaiting));
        }
        return Ok(awaiting);
    }

    let chosen = select_by_preference(req.preference, &current.proposals).ok_or(Error::Policy)?;
    let note = format!("auto-selected via {:?} preference", req.preference);
    store
        .resolve_council(&council.id, &chosen, Some(note))
        .map_err(map_store_err)
}

/// Records the user's chosen blind proposal label, transitions to `Decided`,
/// and reveals vendor/model attribution.
pub fn resolve_council(
    store: &Store,
    council_id: &str,
    chosen_label: &str,
    decision_note: Option<String>,
) -> Result<CouncilDto> {
    store
        .resolve_council(council_id, chosen_label, decision_note)
        .map_err(map_store_err)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{HubReply, ProviderFuture};
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    struct ScriptedProvider {
        responses: Mutex<VecDeque<String>>,
        calls: AtomicUsize,
        seen_prompts: Mutex<Vec<String>>,
    }

    impl ScriptedProvider {
        fn new(responses: Vec<&str>) -> Arc<Self> {
            Arc::new(Self {
                responses: Mutex::new(responses.into_iter().map(str::to_owned).collect()),
                calls: AtomicUsize::new(0),
                seen_prompts: Mutex::new(Vec::new()),
            })
        }

        fn call_count(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    impl HubModelProvider for ScriptedProvider {
        fn complete(
            &self,
            request: HubRequest,
            _events: tokio::sync::broadcast::Sender<ProviderEvent>,
        ) -> ProviderFuture<'_> {
            Box::pin(async move {
                self.calls.fetch_add(1, Ordering::SeqCst);
                let joined = request
                    .messages
                    .iter()
                    .map(|m| m.content.as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                self.seen_prompts.lock().unwrap().push(joined);
                let text = self
                    .responses
                    .lock()
                    .unwrap()
                    .pop_front()
                    .unwrap_or_else(|| "{}".to_owned());
                Ok(HubReply {
                    text,
                    ..Default::default()
                })
            })
        }
    }

    fn setup_store() -> (tempfile::TempDir, Store) {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path().join("ledger")).unwrap();
        store
            .write(|db| {
                db.execute_batch(
                    "INSERT INTO provider_profiles(id,display_name,kind,models_json,created_at)
                     VALUES('p','test','codex_cli','[]','now');
                     INSERT INTO sessions(id,name,role,repo_cwd,profile_id,provider_model,created_at)
                     VALUES('hub','hub','hub','/tmp/repo','p','test','now');",
                )?;
                Ok(())
            })
            .unwrap();
        (dir, store)
    }

    fn valid_payload_json(summary: &str, paths: &[&str]) -> String {
        serde_json::json!({
            "summary": summary,
            "approach": format!("Approach for {summary}"),
            "touched_paths": paths,
            "public_interface_changes": false,
            "new_dependencies": [],
            "security_sensitive": false,
            "beyond_request": false,
            "open_questions": [],
            "pros": ["clean"],
            "cons": []
        })
        .to_string()
    }

    #[tokio::test]
    async fn blind_stripping_direction_never_auto_decides_and_resolve_reveals_attribution() {
        let (_dir, store) = setup_store();
        let p1 = ScriptedProvider::new(vec![&valid_payload_json("Plan A", &["src/a.rs"])]);
        let p2 = ScriptedProvider::new(vec![&valid_payload_json("Plan B", &["src/b.rs"])]);
        let p3 = ScriptedProvider::new(vec![&valid_payload_json("Plan C", &["src/c.rs"])]);

        let council = start_council(
            &store,
            StartCouncilRequest {
                council_id: None,
                hub_session_id: Some("hub".into()),
                ticket_id: None,
                repo_cwd: "/tmp/repo".into(),
                level: None, // defaults to Direction
                preference: PlanningPreference::Balanced,
                prompt: "Design feature X".into(),
                participants: vec![
                    CouncilParticipant {
                        vendor: Vendor::Codex,
                        model: "gpt-5.3-codex".into(),
                        provider: p1.clone(),
                    },
                    CouncilParticipant {
                        vendor: Vendor::Claude,
                        model: "claude-sonnet-4-6".into(),
                        provider: p2.clone(),
                    },
                    CouncilParticipant {
                        vendor: Vendor::Dsh,
                        model: "deepseek-reasoner".into(),
                        provider: p3.clone(),
                    },
                ],
            },
        )
        .await
        .unwrap();

        assert_eq!(council.level, CouncilLevel::Direction);
        assert_eq!(council.status, CouncilStatus::AwaitingUser);
        assert_eq!(council.proposals.len(), 3);
        for prop in &council.proposals {
            assert!(prop.label.starts_with("方案"));
            assert_eq!(prop.vendor, None);
            assert_eq!(prop.model, None);
        }

        // Verify isolation: p2 never saw p1's output
        assert!(!p2.seen_prompts.lock().unwrap()[0].contains("Plan A"));

        // Hub inbox received blind summary without vendor/model names
        let changes = store.changes(None).unwrap();
        let polyclave_store::Changes::Events(events) = changes else {
            panic!("expected events");
        };
        let msg_payloads: Vec<String> = events
            .iter()
            .filter(|e| e.topic == polyclave_proto::ChangeTopic::Message)
            .map(|e| e.payload.to_string())
            .collect();
        assert!(!msg_payloads.is_empty());
        let joined_msgs = msg_payloads.join("\n");
        assert!(!joined_msgs.contains("gpt-5.3-codex"));
        assert!(!joined_msgs.contains("claude-sonnet-4-6"));
        assert!(!joined_msgs.contains("deepseek-reasoner"));

        // Resolve reveals vendor/model
        let chosen = council.proposals[0].label.clone();
        let decided =
            resolve_council(&store, &council.id, &chosen, Some("ship it".into())).unwrap();
        assert_eq!(decided.status, CouncilStatus::Decided);
        assert_eq!(decided.chosen_label.as_deref(), Some(chosen.as_str()));
        for prop in &decided.proposals {
            assert!(prop.vendor.is_some());
            assert!(prop.model.is_some());
        }
    }

    #[tokio::test]
    async fn checkpoint_replay_skips_completed_providers_and_retries_schema_errors() {
        let (_dir, store) = setup_store();
        let p1 = ScriptedProvider::new(vec![
            "not valid json",
            "{\"summary\": \"\" }",
            &valid_payload_json("Recovered after 2 retries", &["src/lib.rs"]),
        ]);
        let p2 = ScriptedProvider::new(vec![&valid_payload_json("Direct", &["src/main.rs"])]);

        // Pre-create council so we can simulate a restart mid-run
        let created = store
            .create_council(NewCouncil {
                ticket_id: None,
                repo_cwd: "/tmp/repo".into(),
                level: CouncilLevel::Detail,
                preference: PlanningPreference::Conservative,
                prompt: "Implement Y".into(),
            })
            .unwrap();

        // Pre-populate checkpoint for participant 1 (p2)
        let hash_p2 = compute_input_hash(
            &created.id,
            CouncilLevel::Detail,
            "/tmp/repo",
            "Implement Y",
            Vendor::Claude,
            "claude-opus",
        );
        let cp_payload =
            parse_proposal_payload(&valid_payload_json("From checkpoint", &["src/main.rs"]))
                .unwrap();
        store
            .record_council_checkpoint(
                &created.id,
                "proposal:1:claude:claude-opus",
                &hash_p2,
                serde_json::to_value(&cp_payload).unwrap(),
            )
            .unwrap();

        let result = start_council(
            &store,
            StartCouncilRequest {
                council_id: Some(created.id.clone()),
                hub_session_id: None,
                ticket_id: None,
                repo_cwd: "/tmp/repo".into(),
                level: Some(CouncilLevel::Detail),
                preference: PlanningPreference::Conservative,
                prompt: "Implement Y".into(),
                participants: vec![
                    CouncilParticipant {
                        vendor: Vendor::Codex,
                        model: "gpt-5".into(),
                        provider: p1.clone(),
                    },
                    CouncilParticipant {
                        vendor: Vendor::Claude,
                        model: "claude-opus".into(),
                        provider: p2.clone(),
                    },
                ],
            },
        )
        .await
        .unwrap();

        assert_eq!(p1.call_count(), 3); // 1 initial + 2 schema-correction retries
        assert_eq!(p2.call_count(), 0); // Replayed from checkpoint!
        assert_eq!(result.status, CouncilStatus::Decided); // Detail with no escalations auto-decides
    }

    #[test]
    fn escalation_rules_cover_all_triggers() {
        let base = ProposalPayload {
            summary: "s".into(),
            approach: "a".into(),
            touched_paths: vec!["src/a.rs".into()],
            ..Default::default()
        };
        let p_a = CouncilProposalDto {
            label: "方案甲".into(),
            payload: base.clone(),
            vendor: None,
            model: None,
        };
        assert!(evaluate_escalations(CouncilLevel::Detail, std::slice::from_ref(&p_a)).is_empty());
        assert!(
            !evaluate_escalations(CouncilLevel::Direction, std::slice::from_ref(&p_a)).is_empty()
        );

        let mut wide = base.clone();
        wide.touched_paths = vec!["1".into(), "2".into(), "3".into(), "4".into()];
        let p_wide = CouncilProposalDto {
            label: "方案乙".into(),
            payload: wide,
            vendor: None,
            model: None,
        };
        let delta_reasons = evaluate_escalations(CouncilLevel::Detail, &[p_a.clone(), p_wide]);
        assert!(delta_reasons.iter().any(|r| r.contains("3x")));

        let mut sec = base.clone();
        sec.security_sensitive = true;
        sec.public_interface_changes = true;
        sec.beyond_request = true;
        sec.open_questions = vec!["which auth?".into()];
        sec.touched_paths = vec![".github/workflows/ci.yml".into()];
        let p_sec = CouncilProposalDto {
            label: "方案丙".into(),
            payload: sec,
            vendor: None,
            model: None,
        };
        let reasons = evaluate_escalations(CouncilLevel::Detail, &[p_sec]);
        assert_eq!(reasons.len(), 5);
    }

    #[tokio::test]
    async fn three_schema_failures_fail_cleanly() {
        let (_dir, store) = setup_store();
        let bad = ScriptedProvider::new(vec!["bad1", "bad2", "bad3"]);
        let result = start_council(
            &store,
            StartCouncilRequest {
                council_id: None,
                hub_session_id: None,
                ticket_id: None,
                repo_cwd: "/tmp/repo".into(),
                level: Some(CouncilLevel::Detail),
                preference: PlanningPreference::Balanced,
                prompt: "Do Z".into(),
                participants: vec![CouncilParticipant {
                    vendor: Vendor::Codex,
                    model: "gpt-5".into(),
                    provider: bad.clone(),
                }],
            },
        )
        .await
        .unwrap();

        assert_eq!(bad.call_count(), 3);
        assert_eq!(result.status, CouncilStatus::Failed);
        assert!(!result.escalation_reasons.is_empty());
    }
}
