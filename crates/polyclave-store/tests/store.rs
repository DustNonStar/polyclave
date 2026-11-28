use polyclave_proto::{
    ChangeTopic, CouncilLevel, CouncilStatus, MemoryKind, MemoryScope, MemoryStatus,
    PlanningPreference, PrGateStatus, ProposalPayload, ReviewVerdict, Vendor,
};
use polyclave_store::{
    reserve_ticket, CandidateMemory, Changes, Error, MemoryFilter, NewCouncil, NewPrGate,
    NewPrReview, NewProposal, OriginSnapshot, ScmCursor, Store,
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

#[test]
fn stage4_cua_mutex_hybrid_memory_evolution_and_skills_work() {
    use polyclave_proto::{EvolutionKind, EvolutionStatus};
    use polyclave_store::stage4::{NewEvolutionSuggestion, NewLocalSkill};

    let (_dir, store) = fixture();

    // CUA mutex: rejects when user active, acquires when idle, blocks 2nd caller, 1-click reclaim unlocks.
    assert!(matches!(
        store.cua_acquire("r1", "Polyclave", 5),
        Err(Error::BusyError)
    ));
    let lock = store.cua_acquire("r1", "Polyclave", 45).unwrap();
    assert!(lock.locked);
    assert!(matches!(
        store.cua_acquire("r2", "Polyclave", 60),
        Err(Error::BusyError)
    ));
    let reclaimed = store.cua_reclaim().unwrap();
    assert!(!reclaimed.locked);
    assert!(reclaimed.reclaimed_at.is_some());

    // Hybrid semantic search on verified memory_items.embedding_blob.
    store
        .write(|db| {
            worker(db, 0)?;
            Store::record_run_memory(db, "run", "repo", "Use Rustls without OpenSSL", None)
        })
        .unwrap();
    let m1 = store
        .find_memory(MemoryFilter::default())
        .unwrap()
        .pop()
        .unwrap();
    store
        .store_memory_embedding(&m1.id, &[1.0, 0.0, 0.0])
        .unwrap();
    let hits = store.hybrid_search_memory(&[1.0, 0.0, 0.0], 5).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].0.id, m1.id);

    // Evolution suggestion lifecycle + rollback.
    let sug = store
        .create_evolution_suggestion(NewEvolutionSuggestion {
            kind: EvolutionKind::Routing,
            title: "Prefer Flash".into(),
            rationale: "Lower latency on simple triage".into(),
            payload_json: json!({"default": "deepseek-flash"}),
            supersedes_id: None,
        })
        .unwrap();
    let applied = store
        .set_evolution_status(&sug.id, EvolutionStatus::Applied)
        .unwrap();
    assert_eq!(applied.status, EvolutionStatus::Applied);
    let rolled = store
        .set_evolution_status(&sug.id, EvolutionStatus::RolledBack)
        .unwrap();
    assert_eq!(rolled.status, EvolutionStatus::RolledBack);

    // Teach-Once versioned local skills & rollback.
    let v1 = store
        .save_local_skill(NewLocalSkill {
            name: "verify-rust".into(),
            description: "Run fmt, clippy, test".into(),
            body_md: "# Verify\nRun cargo test".into(),
            synced_vendors: vec![Vendor::Codex, Vendor::Claude],
        })
        .unwrap();
    assert_eq!(v1.version, 1);
    let v2 = store
        .save_local_skill(NewLocalSkill {
            name: "verify-rust".into(),
            description: "Run fmt, clippy, test v2".into(),
            body_md: "# Verify v2\nRun cargo clippy && cargo test".into(),
            synced_vendors: vec![Vendor::Codex, Vendor::Claude, Vendor::Agy],
        })
        .unwrap();
    assert_eq!(v2.version, 2);
    assert_eq!(v2.supersedes_id.as_deref(), Some(v1.id.as_str()));
    let back = store.rollback_local_skill(&v2.id).unwrap();
    assert_eq!(back.id, v1.id);
    assert!(back.enabled);

    // Community opt-in defaults to false.
    assert!(!store.community_opt_in().unwrap());
    assert!(store.set_community_opt_in(true).unwrap());
}

#[test]
fn memory_auto_embeds_and_fuses_fts_with_semantic_search() {
    let (_dir, store) = fixture();
    store
        .write(|db| {
            worker(db, 0)?;
            Store::record_run_memory(db, "run", "repo", "Use rustls without openssl", None)
        })
        .unwrap();

    // Auto-populated embedding_blob is present immediately on verified memory.
    let has_blob: bool = store
        .read(|db| {
            db.query_row(
                "SELECT embedding_blob IS NOT NULL FROM memory_items WHERE repo_cwd='repo' LIMIT 1",
                [],
                |r| r.get(0),
            )
            .map_err(Into::into)
        })
        .unwrap();
    assert!(has_blob);

    // Exact FTS match works.
    let fts_hits = store
        .find_memory(MemoryFilter {
            repo: Some("repo"),
            query: Some("rustls"),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(fts_hits.len(), 1);

    // Morphological / trigram-similar query that FTS5 alone misses still matches via hybrid fusion.
    let sem_hits = store
        .find_memory(MemoryFilter {
            repo: Some("repo"),
            query: Some("rustlss openssll"),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(sem_hits.len(), 1);

    // Unrelated query still returns empty.
    let miss = store
        .find_memory(MemoryFilter {
            repo: Some("repo"),
            query: Some("zzzz_qqqq_xxxx"),
            ..Default::default()
        })
        .unwrap();
    assert!(miss.is_empty());
}

#[test]
fn run_origin_snapshot_migrates_from_v9_keeps_the_first_snapshot_and_survives_reopen() {
    let (dir, store) = fixture();
    let path = dir.path().join("state.sqlite");
    drop(store);
    // Turn the database back into a version-9 ledger, as an existing install has.
    {
        let db = Connection::open(&path).unwrap();
        db.execute_batch("DROP TABLE run_publish_origins; PRAGMA user_version=9;")
            .unwrap();
    }
    let store = Store::open(&path).unwrap();
    assert_eq!(
        store
            .read(|db| Ok(db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?))
            .unwrap(),
        10
    );
    // A Run admitted before snapshots existed has none.
    assert_eq!(store.run_origin_snapshot("run").unwrap(), None);

    let first = OriginSnapshot {
        urls: vec!["https://github.com/org/repo.git".into()],
        push_urls: vec![],
    };
    store
        .record_run_origin_snapshot("run", first.clone())
        .unwrap();
    // A later change of origin never replaces the admission snapshot.
    store
        .record_run_origin_snapshot(
            "run",
            OriginSnapshot {
                urls: vec!["https://github.com/evil/repo.git".into()],
                push_urls: vec![],
            },
        )
        .unwrap();
    assert_eq!(
        store.run_origin_snapshot("run").unwrap(),
        Some(first.clone())
    );
    drop(store);
    let store = Store::open(&path).unwrap();
    assert_eq!(store.run_origin_snapshot("run").unwrap(), Some(first));
}

#[test]
fn mesh_concurrency_column_upgrades_an_existing_ledger_with_default_three() {
    use polyclave_proto::MeshNodeStatus;
    use polyclave_store::stage3::NewMeshNode;
    let (dir, store) = fixture();
    let path = dir.path().join("state.sqlite");
    store
        .upsert_mesh_node(NewMeshNode {
            id: "node-a".into(),
            name: "Node A".into(),
            endpoint: "http://127.0.0.1:1".into(),
            os_family: "linux".into(),
            allowed_repos: vec!["repo".into()],
            allowed_actions: vec!["worker_run".into()],
            status: MeshNodeStatus::Online,
        })
        .unwrap();
    drop(store);
    // An install from before the cap has neither the column nor the table.
    {
        let db = Connection::open(&path).unwrap();
        db.execute_batch(
            "DROP TABLE mesh_admissions;
             ALTER TABLE mesh_nodes DROP COLUMN max_concurrent_runs;",
        )
        .unwrap();
    }
    let store = Store::open(&path).unwrap();
    let limit = store.mesh_node_limit("node-a").unwrap();
    assert_eq!((limit.max_concurrent_runs, limit.active_runs), (3, 0));

    // The owner's value persists across reopen; out-of-range values are refused.
    assert!(matches!(
        store.set_mesh_node_concurrency("node-a", 0),
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        store.set_mesh_node_concurrency("node-a", 17),
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        store.set_mesh_node_concurrency("node-missing", 2),
        Err(Error::NotFound)
    ));
    store.set_mesh_node_concurrency("node-a", 16).unwrap();
    drop(store);
    let store = Store::open(&path).unwrap();
    assert_eq!(
        store.mesh_node_limit("node-a").unwrap().max_concurrent_runs,
        16
    );
}

#[test]
fn test_results_are_keyed_by_sha_and_never_borrow_another_commit() {
    use polyclave_proto::{TestResultSource, TestStatus};
    use polyclave_store::NewTestResult;

    let (_dir, store) = fixture();
    let older = "a".repeat(40);
    let current = "b".repeat(40);
    let claim = NewTestResult {
        run_id: "run".into(),
        sha: current.clone(),
        source: TestResultSource::WorkerClaim,
        name: String::new(),
        status: TestStatus::Passed,
        summary: "cargo test: 12 passed".into(),
    };
    let ci = NewTestResult {
        run_id: "run".into(),
        sha: current.clone(),
        source: TestResultSource::Ci,
        name: "ci/rust-test".into(),
        status: TestStatus::Pending,
        summary: "ci/rust-test: queued".into(),
    };
    store.record_test_results(vec![claim, ci.clone()]).unwrap();

    // A row for an older commit is recorded and stays separate.
    store
        .record_test_result(NewTestResult {
            run_id: "run".into(),
            sha: older.clone(),
            source: TestResultSource::Ci,
            name: "ci/rust-test".into(),
            status: TestStatus::Failed,
            summary: "ci/rust-test: failure".into(),
        })
        .unwrap();

    let rows = store.test_results_for_sha("run", &current).unwrap();
    assert_eq!(rows.len(), 2, "{rows:?}");
    // Host checks lead; the Worker's claim never comes first.
    assert_eq!(rows[0].source, TestResultSource::Ci);
    assert_eq!(rows[0].status, TestStatus::Pending);
    assert_eq!(rows[1].source, TestResultSource::WorkerClaim);
    assert_eq!(rows[1].status, TestStatus::Passed);
    assert!(rows.iter().all(|r| r.sha == current));

    // The same check moving to completed replaces only its own row.
    store
        .record_test_result(NewTestResult {
            status: TestStatus::Passed,
            summary: "ci/rust-test: success".into(),
            ..ci
        })
        .unwrap();
    let rows = store.test_results_for_sha("run", &current).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].status, TestStatus::Passed);
    assert_eq!(rows[0].summary, "ci/rust-test: success");

    // The older commit keeps its own (failed) row and is never mixed in.
    let older_rows = store.test_results_for_sha("run", &older).unwrap();
    assert_eq!(older_rows.len(), 1);
    assert_eq!(older_rows[0].status, TestStatus::Failed);

    // Nothing is recorded for an unrelated SHA.
    assert!(store
        .test_results_for_sha("run", &"c".repeat(40))
        .unwrap()
        .is_empty());
}

#[test]
fn test_results_reject_a_bad_sha_empty_summary_or_unknown_run() {
    use polyclave_proto::{TestResultSource, TestStatus};
    use polyclave_store::NewTestResult;

    let (_dir, store) = fixture();
    let good = NewTestResult {
        run_id: "run".into(),
        sha: "d".repeat(40),
        source: TestResultSource::Ci,
        name: "ci/test".into(),
        status: TestStatus::Failed,
        summary: "ci/test: failure".into(),
    };
    assert!(matches!(
        store.record_test_result(NewTestResult {
            sha: "D".repeat(40),
            ..good.clone()
        }),
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        store.record_test_result(NewTestResult {
            summary: "  ".into(),
            ..good.clone()
        }),
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        store.record_test_result(NewTestResult {
            run_id: "missing".into(),
            ..good
        }),
        Err(Error::NotFound)
    ));
}

#[test]
fn run_test_sha_prefers_the_verified_result_and_falls_back_to_observed() {
    let (_dir, store) = fixture();
    assert_eq!(store.run_test_sha("run").unwrap(), None);

    let observed = "e".repeat(40);
    let verified = "f".repeat(40);
    let observed_write = observed.clone();
    store
        .write(move |db| {
            db.execute(
                "UPDATE runs SET observed_sha=?1 WHERE id='run'",
                [&observed_write],
            )?;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        store.run_test_sha("run").unwrap().as_deref(),
        Some(observed.as_str())
    );

    let verified_write = verified.clone();
    store
        .write(move |db| {
            db.execute(
                "UPDATE runs SET result_sha=?1 WHERE id='run'",
                [&verified_write],
            )?;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        store.run_test_sha("run").unwrap().as_deref(),
        Some(verified.as_str()),
        "a verified result always wins over the observed tip"
    );
}

#[test]
fn test_results_table_upgrades_without_moving_the_schema_version() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.sqlite");
    let store = Store::open(&path).unwrap();
    let version: i64 = store
        .read(|db| Ok(db.query_row("PRAGMA user_version", [], |r| r.get(0))?))
        .unwrap();
    assert_eq!(version, 10);
    // Reopening runs the additive step again and must stay harmless.
    drop(store);
    let store = Store::open(&path).unwrap();
    let columns: i64 = store
        .read(|db| {
            Ok(db.query_row(
                "SELECT count(*) FROM pragma_table_info('test_results')",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(columns, 7);
}

#[test]
fn external_hub_input_is_marked_as_an_observation_and_user_input_is_not() {
    let (_dir, store) = fixture();
    let user = store.hub_enqueue("hub", "u1", "carry on").unwrap();
    let observed = store
        .hub_enqueue_external("hub", "scm:org/repo:7:comment:1", "PR #7 comment by alice")
        .unwrap();
    assert_eq!(user.source, "user");
    assert_eq!(user.metadata, None);
    assert_eq!(observed.source, "host");
    assert_eq!(
        observed.metadata,
        Some(json!({"kind": polyclave_store::HOST_OBSERVATION_KIND}))
    );
    // Repeating the same observation is idempotent and keeps its marker.
    let again = store
        .hub_enqueue_external("hub", "scm:org/repo:7:comment:1", "PR #7 comment by alice")
        .unwrap();
    assert_eq!(again.id, observed.id);
    assert_eq!(again.metadata, observed.metadata);
    // An external event can never stand as a Ticket's basis message.
    assert!(matches!(
        store.hub_ticket("hub", observed.id, "from a comment", json!({}), 1),
        Err(Error::ConflictError)
    ));
}

#[test]
fn attempt_from_sha_upgrades_an_existing_ledger_without_moving_the_schema_version() {
    let (dir, store) = fixture();
    let path = dir.path().join("state.sqlite");
    let version: i64 = store
        .read(|db| Ok(db.query_row("PRAGMA user_version", [], |r| r.get(0))?))
        .unwrap();
    assert_eq!(version, 10);
    drop(store);
    // A ledger written before revisions has the Attempt row but no start commit.
    {
        let db = Connection::open(&path).unwrap();
        db.execute_batch(
            "ALTER TABLE attempts DROP COLUMN from_sha;
             INSERT INTO attempts(id,run_id,attempt_no,branch,worktree_path,status,created_at)
             VALUES(501,'run',8,'agent/run/a8','wt','completed','now');",
        )
        .unwrap();
    }
    let store = Store::open(&path).unwrap();
    assert_eq!(store.attempt_from_sha(501).unwrap(), None);
    store
        .write(|db| {
            db.execute(
                "INSERT INTO attempts(id,run_id,attempt_no,from_sha,branch,worktree_path,status,created_at)
                 VALUES(502,'run',9,'deadbeef','agent/run/a9','wt2','prepared','now')",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        store.attempt_from_sha(502).unwrap().as_deref(),
        Some("deadbeef")
    );
    assert!(
        store.attempt_from_sha(999).is_err(),
        "a missing Attempt is an error"
    );
    // Reopening runs the additive step again and must stay harmless.
    drop(store);
    let store = Store::open(&path).unwrap();
    let version: i64 = store
        .read(|db| Ok(db.query_row("PRAGMA user_version", [], |r| r.get(0))?))
        .unwrap();
    assert_eq!(version, 10);
}

#[test]
fn remote_intent_column_upgrades_an_existing_ledger_without_moving_the_schema_version() {
    let (dir, store) = fixture();
    let path = dir.path().join("state.sqlite");
    drop(store);
    // A ledger written before the intent column: auto-merge arming had no record
    // of the remote call it was about to make.
    {
        let db = Connection::open(&path).unwrap();
        db.execute_batch("ALTER TABLE pr_gates DROP COLUMN remote_intent;")
            .unwrap();
    }
    let store = Store::open(&path).unwrap();
    let present: i64 = store
        .read(|db| {
            Ok(db.query_row(
                "SELECT count(*) FROM pragma_table_info('pr_gates') WHERE name='remote_intent'",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(present, 1);
    let version: i64 = store
        .read(|db| Ok(db.query_row("PRAGMA user_version", [], |r| r.get(0))?))
        .unwrap();
    assert_eq!(version, 10);
    // Reopening runs the additive step again and must stay harmless.
    drop(store);
    let store = Store::open(&path).unwrap();
    assert!(!store.pause_state().unwrap().paused);
}
