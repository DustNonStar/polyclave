use polyclave_proto::{ChangeTopic, MemoryKind, MemoryScope, MemoryStatus};
use polyclave_store::{reserve_ticket, CandidateMemory, Changes, Error, MemoryFilter, Store};
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
