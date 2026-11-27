use polyclave_proto::{
    ChangeTopic, CouncilLevel, CouncilStatus, MemoryKind, MemoryScope, MemoryStatus,
    PlanningPreference, PrGateStatus, ProposalPayload, ReviewVerdict, Vendor,
};
use polyclave_store::{
    reserve_ticket, CandidateMemory, Changes, Error, MemoryFilter, NewCouncil, NewPrGate,
    NewPrReview, NewProposal, ScmCursor, Store,
};
use rusqlite::Connection;
use serde_json::json;
use std::sync::{Arc, Barrier};

fn fixture() -> (tempfile::TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().join("state.sqlite")).unwrap();
    store.write(|db| {
        db.execute_batch("
            INSERT INTO provider_profiles(id,display_name,kind,models_json,created_at)
                VALUES('profile','Test','codex_cli','[]','now');
            INSERT INTO sessions(id,name,role,repo_cwd,profile_id,provider_model,created_at)
                VALUES('hub','Hub','hub','repo','profile','test','now'),
                      ('worker','Worker','worker','repo','profile','test','now');
            INSERT INTO tickets(id,hub_session_id,repo_cwd,title,mandate_json,source_kind,max_runs,status,created_at,updated_at)
                VALUES('ticket','hub','repo','Test','{}','explicit_user',1,'active','now','now');
            INSERT INTO runs(id,ticket_id,hub_session_id,worker_session_id,request_id,repo_cwd,vendor,instructions,
                mirror_remote,base_branch,base_sha,status,current_attempt,created_at,updated_at)
                VALUES('run','ticket','hub','worker','request','repo','codex','test','mirror','main','base','running',1,'now','now');
            INSERT INTO attempts(run_id,attempt_no,branch,worktree_path,status,created_at)
                VALUES('run',1,'agent/run/a1','worktree','running','now');
        ")?;
        Ok(())
    }).unwrap();
    (dir, store)
}

fn contenders<T: Send + 'static>(
    store: &Store,
    action: impl Fn(Store, usize) -> T + Send + Sync + 'static,
) -> [T; 2] {
    let start = Arc::new(Barrier::new(2));
    let action = Arc::new(action);
    [0, 1]
        .map(|i| {
            let (store, start, action) = (store.clone(), start.clone(), action.clone());
            std::thread::spawn(move || {
                start.wait();
                action(store, i)
            })
        })
        .map(|thread| thread.join().unwrap())
}

fn worker(db: &Connection, id: usize) -> polyclave_store::Result<()> {
    db.execute(
        "INSERT INTO turns(session_id,run_id,attempt_id,mode,status,phase,host_instance_id,boot_epoch,mcp_token,started_at)
        VALUES('worker','run',1,'start','running','authorized','host','boot',?1,'now')",
        [format!("fixture-worker-{id}")],
    )?;
    Ok(())
}

#[test]
fn writer_serializes_run_and_session_locks_but_consult_does_not_acquire_them() {
    let (_dir, store) = fixture();
    let results = contenders(&store, |s, i| s.write(move |db| worker(db, i)));
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Err(Error::BusyError)))
            .count(),
        1
    );

    let results = contenders(&store, |s, i| {
        s.write(move |db| {
        db.execute(
            "INSERT INTO turns(session_id,mode,status,phase,host_instance_id,boot_epoch,mcp_token,started_at)
            VALUES('hub','hub','running','authorized','host','boot',?1,'now')",
            [format!("fixture-hub-{i}")],
        )?;
        Ok(())
    })
    });
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Err(Error::BusyError)))
            .count(),
        1
    );
    store.write(|db| {
        db.execute_batch("
            INSERT INTO turns(session_id,parent_turn_id,mode,status,phase,host_instance_id,boot_epoch,mcp_token,started_at)
                SELECT 'hub',id,'consult','running','authorized','host','boot','fixture-hub-consult','now'
                FROM turns WHERE mode='hub';
            INSERT INTO turns(session_id,run_id,attempt_id,parent_turn_id,mode,status,phase,host_instance_id,boot_epoch,mcp_token,started_at)
                SELECT 'worker','run',1,id,'consult','running','authorized','host','boot','fixture-worker-consult','now'
                FROM turns WHERE mode='start';
        ")?;
        Ok(())
    }).unwrap();
    store
        .read(|db| {
            let active: i64 = db.query_row(
                "SELECT count(*) FROM turns WHERE status='running'",
                [],
                |r| r.get(0),
            )?;
            assert_eq!(active, 4);
            let journal: String = db.pragma_query_value(None, "journal_mode", |r| r.get(0))?;
            let timeout: u32 = db.pragma_query_value(None, "busy_timeout", |r| r.get(0))?;
            assert_eq!(journal, "wal");
            assert_eq!(timeout, 5000);
            Ok(())
        })
        .unwrap();
}

#[test]
fn budget_and_non_lock_uniqueness_report_distinct_errors() {
    let (_dir, store) = fixture();
    let results = contenders(&store, |s, _| {
        s.write(|db| reserve_ticket(db, "ticket", "repo"))
    });
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|r| matches!(r, Err(Error::BudgetExceededError)))
            .count(),
        1
    );
    let duplicate = store.write(|db| {
        db.execute_batch(
            "INSERT INTO attempts(run_id,attempt_no,branch,worktree_path,status,created_at)
            VALUES('run',1,'other','other','running','now');",
        )?;
        Ok(())
    });
    assert!(matches!(duplicate, Err(Error::ConflictError)));
}

fn search(store: &Store, query: &str) -> Vec<String> {
    store
        .find_memory(MemoryFilter {
            repo: Some("repo"),
            query: Some(query),
            ..Default::default()
        })
        .unwrap()
        .into_iter()
        .map(|m| m.id)
        .collect()
}

#[test]
fn fts5_syncs_insert_update_and_delete_and_candidate_never_becomes_visible() {
    let (_dir, store) = fixture();
    let turn = store
        .write(|db| {
            worker(db, 0)?;
            Ok(db.last_insert_rowid())
        })
        .unwrap();
    let candidate = store
        .propose_memory(
            turn,
            CandidateMemory {
                kind: MemoryKind::CodebaseFact,
                title: "candidateword".into(),
                content: "unverified worker observation".into(),
                tags: vec!["worker-tag".into()],
            },
        )
        .unwrap();
    assert_eq!(candidate.scope, MemoryScope::Project);
    assert_eq!(candidate.status, MemoryStatus::Candidate);
    assert_eq!(candidate.source_run_id.as_deref(), Some("run"));
    assert_eq!(candidate.source_turn_id, Some(turn));
    assert!(search(&store, "candidateword").is_empty());
    assert!(store
        .find_memory(MemoryFilter::default())
        .unwrap()
        .is_empty());
    store
        .write(|db| {
            Store::record_run_memory(db, "run", "repo", "verifiedword", Some("commit"))?;
            Ok(())
        })
        .unwrap();
    let indexed = search(&store, "verifiedword");
    assert_eq!(indexed.len(), 1);
    store.write(|db| {
        db.execute("UPDATE memory_items SET title='replacementword',content='replacementword' WHERE status='verified'", [])?;
        Ok(())
    }).unwrap();
    assert!(search(&store, "verifiedword").is_empty());
    assert_eq!(search(&store, "replacementword"), indexed);
    store
        .write(|db| {
            db.execute("DELETE FROM memory_items WHERE status='verified'", [])?;
            Ok(())
        })
        .unwrap();
    assert!(search(&store, "replacementword").is_empty());
}

#[test]
fn change_log_rejects_secret_fields_and_resyncs_at_pruned_or_future_cursor() {
    let (_dir, store) = fixture();
    let rejected = store.write(|db| {
        Store::append_change(
            db,
            ChangeTopic::Turn,
            "1",
            &json!({"nested":[{"mcp_token":"test-only"}]}),
        )
    });
    assert!(matches!(rejected, Err(Error::Invalid(_))));
    assert!(matches!(store.changes(Some(0)).unwrap(), Changes::Events(v) if v.is_empty()));
    store
        .write(|db| {
            for i in 1..=10_002 {
                Store::append_change(db, ChangeTopic::Run, "run", &json!({"index":i}))?;
            }
            Ok(())
        })
        .unwrap();
    for cursor in [0, 1, 10_003] {
        assert!(matches!(
            store.changes(Some(cursor)).unwrap(),
            Changes::ResyncRequired
        ));
    }
    let Changes::Events(events) = store.changes(Some(2)).unwrap() else {
        panic!("retained cursor")
    };
    assert_eq!(events.len(), 10_000);
    assert_eq!(events.first().unwrap().seq, 3);
    assert_eq!(events.last().unwrap().seq, 10_002);
}

#[test]
fn fts_multi_word_nul_safety_tag_bounds_and_hub_receipt_edge_cases() {
    let (_dir, store) = fixture();
    store.builtin_profiles().unwrap();
    store
        .write(|db| {
            db.execute("INSERT INTO sessions(id,name,role,repo_cwd,profile_id,provider_model,created_at) VALUES('s1','hub','hub','repo','builtin-hub','default','now')", [])?;
            db.execute("INSERT INTO sessions(id,name,role,repo_cwd,profile_id,provider_model,created_at) VALUES('w1','worker','worker','repo','builtin-hub','default','now')", [])?;
            db.execute("INSERT INTO tickets(id,hub_session_id,repo_cwd,title,mandate_json,source_kind,max_runs,runs_used,status,created_at,updated_at) VALUES('t1','s1','repo','title','{}','explicit_user',3,0,'active','now','now')", [])?;
            db.execute("INSERT INTO runs(id,ticket_id,hub_session_id,worker_session_id,request_id,vendor,instructions,repo_cwd,base_branch,base_sha,mirror_remote,status,current_attempt,max_attempts,created_at,updated_at) VALUES('r1','t1','s1','w1','req1','codex','do','repo','main','base','mirror','interrupted',1,3,'now','now')", [])?;
            db.execute("INSERT INTO attempts(id,run_id,attempt_no,branch,worktree_path,status,created_at) VALUES(91,'r1',1,'b','wt','interrupted','now')", [])?;
            db.execute("INSERT INTO turns(id,session_id,run_id,attempt_id,mode,status,phase,host_instance_id,boot_epoch,mcp_token,started_at) VALUES(91,'w1','r1',91,'start','running','authorized','h','b','tok','now')", [])?;
            Ok(())
        })
        .unwrap();

    // Tag bounds
    let too_many_tags = CandidateMemory {
        kind: MemoryKind::Constraint,
        title: "Title".into(),
        content: "Body".into(),
        tags: (0..33).map(|i| format!("t{i}")).collect(),
    };
    assert!(matches!(
        store.propose_memory(91, too_many_tags),
        Err(Error::Invalid(_))
    ));

    // FTS multi-word & embedded NUL safety
    store
        .write(|db| {
            Store::record_run_memory(db, "r1", "repo", "alpha middle omega", Some("sha"))?;
            Ok(())
        })
        .unwrap();
    let found = store
        .find_memory(MemoryFilter {
            query: Some("alpha\0omega"),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(found.len(), 1);

    // Wrong repo must not block active ticket
    assert!(matches!(
        store.hub_ticket_for_dispatch("s1", "t1", "other-repo"),
        Err(Error::ConflictError)
    ));
    assert!(store.hub_ticket_for_dispatch("s1", "t1", "repo").is_ok());

    // Retried attempts ending in same status still emit hub_worker_receipt,
    // and never_started re-queues host receipts.
    assert_eq!(
        store.hub_worker_receipt("r1").unwrap().as_deref(),
        Some("s1")
    );
    store
        .write(|db| {
            db.execute("UPDATE runs SET current_attempt=2 WHERE id='r1'", [])?;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        store.hub_worker_receipt("r1").unwrap().as_deref(),
        Some("s1")
    );

    let turn = store.hub_claim("s1", "h", "b").unwrap().unwrap();
    assert_eq!(turn.input.source, "host");
    store
        .hub_end_without_reply(turn.id, "never_started")
        .unwrap();
    let reclaimed = store.hub_claim("s1", "h", "b").unwrap().unwrap();
    assert_eq!(reclaimed.input.id, turn.input.id);
}

// ---- Stage 2: council, pr_gate, scm cursor and global pause ----------------

fn proposal(label: &str, vendor: Vendor, model: &str, hash: &str) -> NewProposal {
    NewProposal {
        label: label.into(),
        vendor,
        model: model.into(),
        payload: ProposalPayload {
            summary: format!("summary {label}"),
            touched_paths: vec!["src/lib.rs".into()],
            ..Default::default()
        },
        input_hash: hash.into(),
    }
}

fn new_council(store: &Store) -> polyclave_proto::CouncilDto {
    store
        .create_council(NewCouncil {
            ticket_id: Some("ticket".into()),
            repo_cwd: "repo".into(),
            level: CouncilLevel::Direction,
            preference: PlanningPreference::Balanced,
            prompt: "plan the change".into(),
        })
        .unwrap()
}

fn council_events(store: &Store) -> Vec<polyclave_proto::SseEnvelope> {
    let Changes::Events(events) = store.changes(Some(0)).unwrap() else {
        panic!("events")
    };
    events
        .into_iter()
        .filter(|e| e.topic == ChangeTopic::Council)
        .collect()
}

#[test]
fn council_checkpoint_replay_is_idempotent_and_survives_reopen() {
    let (dir, store) = fixture();
    let council = new_council(&store);
    assert_eq!(council.status, CouncilStatus::Running);
    let first = store
        .record_council_checkpoint(&council.id, "fanout:0", "hash-a", json!({"step":1}))
        .unwrap();
    assert_eq!(first, json!({"step":1}));
    // Same input hash: replay returns the stored output, never overwrites it.
    let replay = store
        .record_council_checkpoint(&council.id, "fanout:0", "hash-a", json!({"step":2}))
        .unwrap();
    assert_eq!(replay, json!({"step":1}));
    // Different input for the same step is a conflict.
    assert!(matches!(
        store.record_council_checkpoint(&council.id, "fanout:0", "hash-b", json!({})),
        Err(Error::ConflictError)
    ));
    assert!(matches!(
        store.record_council_checkpoint("missing", "fanout:0", "hash-a", json!({})),
        Err(Error::NotFound)
    ));
    // Proposal insertion replays idempotently on the same input hash.
    store
        .add_council_proposal(&council.id, proposal("方案甲", Vendor::Codex, "m1", "h1"))
        .unwrap();
    let before = council_events(&store).len();
    store
        .add_council_proposal(&council.id, proposal("方案甲", Vendor::Codex, "m1", "h1"))
        .unwrap();
    assert_eq!(council_events(&store).len(), before);
    assert!(matches!(
        store.add_council_proposal(&council.id, proposal("方案甲", Vendor::Claude, "m2", "h2")),
        Err(Error::ConflictError)
    ));
    drop(store);
    let reopened = Store::open(dir.path().join("state.sqlite")).unwrap();
    let stored = reopened
        .council_checkpoint(&council.id, "fanout:0")
        .unwrap()
        .unwrap();
    assert_eq!(stored.input_hash, "hash-a");
    assert_eq!(stored.output, json!({"step":1}));
    assert!(reopened
        .council_checkpoint(&council.id, "other")
        .unwrap()
        .is_none());
    assert_eq!(reopened.council(&council.id).unwrap().proposals.len(), 1);
}

#[test]
fn council_proposals_stay_blind_until_decided_including_change_log() {
    let (_dir, store) = fixture();
    let council = new_council(&store);
    store
        .add_council_proposal(
            &council.id,
            proposal("方案甲", Vendor::Codex, "gpt-x", "h1"),
        )
        .unwrap();
    store
        .add_council_proposal(
            &council.id,
            proposal("方案乙", Vendor::Claude, "opus-y", "h2"),
        )
        .unwrap();
    let awaiting = store
        .update_council_status(
            &council.id,
            CouncilStatus::AwaitingUser,
            Some(vec!["level_direction".into()]),
        )
        .unwrap();
    assert_eq!(awaiting.escalation_reasons, vec!["level_direction"]);
    // Insertion order is preserved (not Unicode order of the labels).
    let labels: Vec<_> = awaiting
        .proposals
        .iter()
        .map(|p| p.label.as_str())
        .collect();
    assert_eq!(labels, ["方案甲", "方案乙"]);
    for blind in [&awaiting, &store.council(&council.id).unwrap()] {
        assert!(blind
            .proposals
            .iter()
            .all(|p| p.vendor.is_none() && p.model.is_none()));
    }
    for event in council_events(&store) {
        let text = event.payload.to_string();
        assert!(!text.contains("gpt-x") && !text.contains("opus-y"));
        assert!(!text.contains("\"codex\"") && !text.contains("\"claude\""));
    }
    // Decided is only reachable through resolve; unknown labels are refused.
    assert!(matches!(
        store.update_council_status(&council.id, CouncilStatus::Decided, None),
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        store.resolve_council(&council.id, "方案丙", None),
        Err(Error::Invalid(_))
    ));
    let decided = store
        .resolve_council(&council.id, "方案乙", Some("smaller diff".into()))
        .unwrap();
    assert_eq!(decided.status, CouncilStatus::Decided);
    assert_eq!(decided.chosen_label.as_deref(), Some("方案乙"));
    assert!(decided.decided_at.is_some());
    assert_eq!(decided.proposals[0].vendor, Some(Vendor::Codex));
    assert_eq!(decided.proposals[1].model.as_deref(), Some("opus-y"));
    let last = council_events(&store).pop().unwrap();
    assert_eq!(last.payload["proposals"][1]["vendor"], "claude");
    // A decided council is closed.
    assert!(matches!(
        store.resolve_council(&council.id, "方案甲", None),
        Err(Error::ConflictError)
    ));
    assert!(matches!(
        store.add_council_proposal(&council.id, proposal("方案丙", Vendor::Agy, "m", "h3")),
        Err(Error::ConflictError)
    ));
    assert_eq!(store.list_councils().unwrap().len(), 1);
}

fn review(sha: &str, verdict: ReviewVerdict) -> NewPrReview {
    NewPrReview {
        reviewer_vendor: Vendor::Claude,
        reviewer_model: "reviewer".into(),
        reviewed_sha: sha.into(),
        verdict,
        comments: "looks fine".into(),
        github_review_id: Some(7),
    }
}

#[test]
fn pr_gate_auto_merge_ledger_and_pause_flag_persist_across_reopen() {
    let (dir, store) = fixture();
    assert!(!store.pause_state().unwrap().paused);
    let gate = store
        .create_pr_gate(NewPrGate {
            run_id: "run".into(),
            repo_slug: "owner/repo".into(),
            branch: "agent/run".into(),
            base_sha: "base".into(),
            head_sha: "head1".into(),
            touched_paths: vec!["src/**".into()],
        })
        .unwrap();
    assert_eq!(gate.ticket_id, "ticket");
    assert_eq!(gate.status, PrGateStatus::Pending);
    assert!(!gate.auto_merge_armed);
    let violated = store
        .record_scope_violations(&gate.id, vec![".github/workflows/ci.yml".into()])
        .unwrap();
    assert_eq!(violated.status, PrGateStatus::ScopeViolation);
    store.record_scope_violations(&gate.id, vec![]).unwrap();
    store
        .record_pr_opened(&gate.id, 42, "https://example.invalid/pr/42")
        .unwrap();
    // Reviews are pinned to the recorded head.
    assert!(matches!(
        store.record_pr_review(&gate.id, review("stale", ReviewVerdict::Approve)),
        Err(Error::ConflictError)
    ));
    store
        .record_pr_review(&gate.id, review("head1", ReviewVerdict::Revise))
        .unwrap();
    assert_eq!(store.begin_revise_round(&gate.id).unwrap().revise_rounds, 1);
    store.set_pr_head(&gate.id, "head2").unwrap();
    store
        .record_pr_review(&gate.id, review("head2", ReviewVerdict::Approve))
        .unwrap();
    assert!(matches!(
        store.set_pr_gate_status(&gate.id, PrGateStatus::AutoMergeArmed),
        Err(Error::Invalid(_))
    ));
    // Not approved yet, then stale head: arming is refused.
    assert!(matches!(
        store.arm_auto_merge(&gate.id, "head2"),
        Err(Error::ConflictError)
    ));
    store
        .set_pr_gate_status(&gate.id, PrGateStatus::Approved)
        .unwrap();
    assert!(matches!(
        store.arm_auto_merge(&gate.id, "head1"),
        Err(Error::ConflictError)
    ));
    let armed = store.arm_auto_merge(&gate.id, "head2").unwrap();
    assert!(armed.auto_merge_armed);
    assert_eq!(armed.status, PrGateStatus::AutoMergeArmed);
    assert_eq!(armed.reviews.len(), 2);
    // Armed gates refuse head moves and generic transitions.
    assert!(matches!(
        store.set_pr_head(&gate.id, "head3"),
        Err(Error::ConflictError)
    ));
    assert!(matches!(
        store.set_pr_gate_status(&gate.id, PrGateStatus::Reviewing),
        Err(Error::ConflictError)
    ));
    let paused = store.set_pause(true, Some("user pause".into())).unwrap();
    assert!(paused.paused);

    // Simulated crash: the ledger and the pause flag survive.
    drop(store);
    let store = Store::open(dir.path().join("state.sqlite")).unwrap();
    let pause = store.pause_state().unwrap();
    assert!(pause.paused);
    assert_eq!(pause.reason.as_deref(), Some("user pause"));
    let in_flight = store.armed_pr_gates().unwrap();
    assert_eq!(in_flight.len(), 1);
    assert_eq!(in_flight[0].id, gate.id);
    assert_eq!(in_flight[0].head_sha, "head2");
    let revoked = store
        .disarm_auto_merge(&gate.id, PrGateStatus::Paused)
        .unwrap();
    assert!(!revoked.auto_merge_armed);
    assert!(store.armed_pr_gates().unwrap().is_empty());
    // New arming is refused while paused.
    store
        .set_pr_gate_status(&gate.id, PrGateStatus::Approved)
        .unwrap();
    assert!(matches!(
        store.arm_auto_merge(&gate.id, "head2"),
        Err(Error::ConflictError)
    ));
    assert!(!store.set_pause(false, None).unwrap().paused);
    store.arm_auto_merge(&gate.id, "head2").unwrap();
    let merged = store
        .set_pr_gate_status(&gate.id, PrGateStatus::Merged)
        .unwrap();
    assert!(!merged.auto_merge_armed);
    assert_eq!(store.list_pr_gates().unwrap().len(), 1);

    // Every mutation is on the change log, in order, with monotonic seq.
    let Changes::Events(events) = store.changes(Some(0)).unwrap() else {
        panic!("events")
    };
    let topics: Vec<_> = events.iter().map(|e| e.topic).collect();
    assert!(topics.contains(&ChangeTopic::PrGate));
    assert_eq!(
        topics.iter().filter(|t| **t == ChangeTopic::Pause).count(),
        2
    );
    assert!(events.windows(2).all(|w| w[0].seq < w[1].seq));
    assert_eq!(events.last().unwrap().payload["status"], "merged");
}

#[test]
fn scm_cursor_upserts_and_change_log_rejects_github_tokens() {
    let (_dir, store) = fixture();
    assert!(store.scm_cursor("owner/repo", 42).unwrap().is_none());
    let mut cursor = ScmCursor {
        repo_slug: "owner/repo".into(),
        pr_number: 42,
        last_event_etag: Some("W/\"1\"".into()),
        last_check_sha: Some("head1".into()),
        last_comment_id: Some(10),
    };
    store.save_scm_cursor(cursor.clone()).unwrap();
    cursor.last_comment_id = Some(11);
    cursor.last_event_etag = None;
    store.save_scm_cursor(cursor.clone()).unwrap();
    assert_eq!(store.scm_cursor("owner/repo", 42).unwrap(), Some(cursor));
    for key in ["bot_token", "reviewer_token"] {
        let rejected = store.write(move |db| {
            Store::append_change(db, ChangeTopic::PrGate, "g", &json!({ key: "test-only" }))
        });
        assert!(matches!(rejected, Err(Error::Invalid(_))));
    }
}

#[test]
fn stage3_pairing_revocation_mesh_and_notifications_are_durable() {
    use polyclave_proto::{ClientKind, MeshNodeStatus, NotificationChannelKind};
    use polyclave_store::stage3::{NewMeshNode, NewNotificationChannel};

    let (_dir, store) = fixture();
    let (ticket, secret) = store
        .create_pairing_ticket(ClientKind::Ios, "Dust iPhone", 600)
        .unwrap();
    assert!(ticket.used_at.is_none());

    // Exchange once succeeds; replay fails with ConflictError.
    let (device, raw_token) = store
        .exchange_pairing_ticket(&secret, Some("dust@tailnet".into()))
        .unwrap();
    assert!(matches!(
        store.exchange_pairing_ticket(&secret, Some("dust@tailnet".into())),
        Err(Error::ConflictError)
    ));

    // Dual-identity check: matching peer passes, mismatched peer fails.
    assert!(store
        .verify_device_token(&raw_token, Some("dust@tailnet"))
        .unwrap()
        .is_some());
    assert!(store
        .verify_device_token(&raw_token, Some("other@tailnet"))
        .unwrap()
        .is_none());

    // Expired ticket is rejected.
    let (exp_ticket, exp_secret) = store
        .create_pairing_ticket(ClientKind::Android, "Expired Phone", 60)
        .unwrap();
    store
        .write(move |db| {
            db.execute(
                "UPDATE pairing_tickets SET expires_at='2020-01-01T00:00:00.000Z' WHERE id=?1",
                [&exp_ticket.id],
            )?;
            Ok(())
        })
        .unwrap();
    assert!(matches!(
        store.exchange_pairing_ticket(&exp_secret, None),
        Err(Error::ConflictError)
    ));

    // Instant revocation invalidates token and emits ChangeTopic::Device without secrets.
    let revoked = store.revoke_paired_device(&device.id).unwrap();
    assert!(revoked.revoked_at.is_some());
    assert!(store
        .verify_device_token(&raw_token, Some("dust@tailnet"))
        .unwrap()
        .is_none());

    // Mesh node & notification channel CRUD.
    let node = store
        .upsert_mesh_node(NewMeshNode {
            id: "node-win".into(),
            name: "Win Workstation".into(),
            endpoint: "http://100.64.0.12:4311".into(),
            os_family: "windows".into(),
            allowed_repos: vec!["/repo".into()],
            allowed_actions: vec!["worker_run".into()],
            status: MeshNodeStatus::Online,
        })
        .unwrap();
    assert_eq!(store.list_mesh_nodes().unwrap().len(), 1);
    let rev_node = store.revoke_mesh_node(&node.id).unwrap();
    assert_eq!(rev_node.status, MeshNodeStatus::Revoked);
    assert!(rev_node.allowed_actions.is_empty());

    let ch = store
        .upsert_notification_channel(NewNotificationChannel {
            id: None,
            kind: NotificationChannelKind::Ntfy,
            label: "Private ntfy".into(),
            endpoint: "http://127.0.0.1:8090".into(),
            topic: "polyclave".into(),
            enabled: true,
        })
        .unwrap();
    assert_eq!(store.list_notification_channels().unwrap().len(), 1);
    store.delete_notification_channel(&ch.id).unwrap();
    assert!(store.list_notification_channels().unwrap().is_empty());

    let Changes::Events(events) = store.changes(Some(0)).unwrap() else {
        panic!("events");
    };
    let serialized = serde_json::to_string(&events).unwrap();
    assert!(!serialized.contains(&secret));
    assert!(!serialized.contains(&raw_token));
}
