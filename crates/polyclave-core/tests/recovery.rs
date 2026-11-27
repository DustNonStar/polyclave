mod common;
use common::{git, repository};
use polyclave_core::{
    runner::{RunnerConfig, TurnRunner},
    runs::{Resolution, RunManager, RunRequest},
};
use polyclave_proto::{RunStatus, TurnMode};
use polyclave_store::Error;
use rusqlite::params;
fn fixture() -> (tempfile::TempDir, RunManager, TurnRunner, RunRequest) {
    let (tmp, rm, request) = repository();
    let state = tmp.path().join("state");
    let runner = TurnRunner::new(
        rm.clone(),
        &state,
        "new".into(),
        RunnerConfig {
            supervisor: "fixture-only".into(),
            mcp_url: "http://127.0.0.1:4310/mcp".into(),
            codex: "fixture-only".into(),
            claude: "fixture-only".into(),
            claude_isolation: None,
        },
        |_, _| {},
    )
    .unwrap();
    (tmp, rm, runner, request)
}

fn crashed(rm: &RunManager, work: &polyclave_core::runs::RunWork, boot: String) -> i64 {
    let id = work.run.id.clone();
    let sid = work.run.worker_session_id.clone();
    let aid = work.attempt.id;
    rm.store.write(move|db|{
        db.execute("INSERT INTO turns(session_id,run_id,attempt_id,mode,status,phase,host_instance_id,boot_epoch,mcp_token,started_at)
            VALUES(?1,?2,?3,'start','running','authorized','old',?4,?5,'now')",params![sid,id,aid,boot,format!("fixture-turn-{aid}")])?;
        Ok(db.last_insert_rowid())
    }).unwrap()
}
fn end(rm: &RunManager, turn: i64) -> (String, String) {
    rm.store
        .read(|db| {
            Ok(db.query_row(
                "SELECT end_reason,writer_check FROM turns WHERE id=?1",
                [turn],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?)
        })
        .unwrap()
}

#[test]
fn authorized_but_not_released_is_never_started_and_cleanup_is_selective() {
    let (_tmp, rm, runner, req) = fixture();
    let (r, new) = rm.accept(req.clone()).unwrap();
    assert!(new);
    let (dup, new) = rm.accept(req).unwrap();
    assert!(!new);
    assert_eq!(r.id, dup.id);
    assert_eq!(rm.list().unwrap().len(), 1);
    let work = rm.begin(&r.id, TurnMode::Start).unwrap();
    let turn = crashed(&rm, &work, polyclave_os::boot_epoch().unwrap());
    let config = runner
        .marker(turn)
        .with_file_name(format!("mcp-turn-{turn}.json"));
    std::fs::write(&config, b"fixture-only").unwrap();
    let unrelated = config.with_file_name("do-not-delete");
    std::fs::write(&unrelated, b"unrelated").unwrap();
    assert_eq!(runner.recover().unwrap(), 1);
    assert_eq!(end(&rm, turn), ("never_started".into(), "excluded".into()));
    assert_eq!(rm.get(&r.id).unwrap().status, RunStatus::Interrupted);
    assert!(!config.exists());
    assert!(unrelated.exists());
}

#[test]
fn staged_but_unsigned_after_reboot_requires_review_and_new_attempt_is_clean() {
    let (_tmp, rm, runner, req) = fixture();
    let (r, _) = rm.accept(req).unwrap();
    let work = rm.begin(&r.id, TurnMode::Start).unwrap();
    let turn = crashed(&rm, &work, "previous-boot".into());
    std::fs::write(runner.marker(turn), b"released").unwrap();
    let cwd = &work.workspace.cwd;
    std::fs::write(cwd.join("staged.txt"), b"new").unwrap();
    git(cwd, &["add", "staged.txt"]);
    git(
        cwd,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "-m",
            "work",
            "--trailer",
            &format!("Run-Id: {}", r.id),
        ],
    );
    let reference = work.id.full_ref();
    git(
        cwd,
        &[
            "push",
            "polyclave-staging",
            &format!("{reference}:{reference}"),
        ],
    );
    let commit = git(cwd, &["rev-parse", "HEAD"]);
    assert_eq!(runner.recover().unwrap(), 1);
    assert_eq!(end(&rm, turn), ("host_lost".into(), "excluded".into()));
    let review = rm.get(&r.id).unwrap();
    assert_eq!(review.status, RunStatus::NeedsReview);
    assert_eq!(review.observed_sha.as_deref(), Some(commit.as_str()));
    assert!(review.result_sha.is_none());
    assert!(!runner.marker(turn).exists());
    let allow = rm
        .resolve(&r.id, Resolution::AllowRetry, "create fresh generation")
        .unwrap();
    assert_eq!(allow.resolution.as_deref(), Some("allow_retry"));
    let next = rm.begin(&r.id, TurnMode::Retry).unwrap();
    assert_eq!(next.attempt.attempt_no, 2);
    assert_eq!(git(&next.workspace.cwd, &["rev-parse", "HEAD"]), r.base_sha);
    assert!(!next.workspace.cwd.join("staged.txt").exists());
    assert_eq!(git(&work.id.run.mirror, &["rev-parse", &reference]), commit);
}

#[test]
fn released_same_boot_uses_platform_writer_guarantee() {
    let (_tmp, rm, runner, req) = fixture();
    let (r, _) = rm.accept(req).unwrap();
    let w = rm.begin(&r.id, TurnMode::Start).unwrap();
    let turn = crashed(&rm, &w, polyclave_os::boot_epoch().unwrap());
    std::fs::write(runner.marker(turn), b"released").unwrap();
    assert_eq!(runner.recover().unwrap(), 1);
    let (writer, status) = if polyclave_os::hard_stop_excludes_writers() {
        ("excluded", RunStatus::Interrupted)
    } else {
        ("possible", RunStatus::NeedsReview)
    };
    assert_eq!(end(&rm, turn), ("host_lost".into(), writer.into()));
    assert_eq!(rm.get(&r.id).unwrap().status, status);
    rm.resolve(&r.id, Resolution::Fail, "fixture failure")
        .unwrap();
    assert!(matches!(
        runner.schedule(&r.id, TurnMode::WorkerChat, "must not run".into()),
        Err(Error::ConflictError)
    ));
}

#[test]
fn reboot_recovers_a_signed_staged_result_and_is_idempotent() {
    let (_tmp, rm, runner, req) = fixture();
    let (r, _) = rm.accept(req).unwrap();
    let w = rm.begin(&r.id, TurnMode::Start).unwrap();
    let turn = crashed(&rm, &w, "previous-boot".into());
    std::fs::write(runner.marker(turn), b"released").unwrap();
    let cwd = &w.workspace.cwd;
    git(
        cwd,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "--allow-empty",
            "-m",
            "verified",
            "--trailer",
            &format!("Run-Id: {}", r.id),
        ],
    );
    let reference = w.id.full_ref();
    let tip = git(cwd, &["rev-parse", "HEAD"]);
    git(
        cwd,
        &[
            "push",
            "polyclave-staging",
            &format!("{reference}:{reference}"),
        ],
    );
    let (id, attempt, tip1) = (r.id.clone(), w.attempt.id, tip.clone());
    rm.store.write(move|db|{
        db.execute("INSERT INTO results(run_id,attempt_id,turn_id,accepted,payload_json,created_at) VALUES(?1,?2,?3,1,?4,'now')",
            params![id,attempt,turn,serde_json::json!({"sha":tip1}).to_string()])?;
        Ok(())
    }).unwrap();
    assert_eq!(runner.recover().unwrap(), 1);
    let done = rm.get(&r.id).unwrap();
    assert_eq!(done.status, RunStatus::Done);
    assert_eq!(done.result_sha, Some(tip));
    assert_eq!(runner.recover().unwrap(), 0);
    assert_eq!(rm.get(&r.id).unwrap(), done);
    assert_eq!(rm.store.find_memory(Default::default()).unwrap().len(), 1);
}

#[test]
fn an_unreadable_mirror_on_restart_does_not_automatically_retry() {
    let (_tmp, rm, runner, req) = fixture();
    let (r, _) = rm.accept(req).unwrap();
    let w = rm.begin(&r.id, TurnMode::Start).unwrap();
    let turn = crashed(&rm, &w, "previous-boot".into());
    std::fs::write(runner.marker(turn), b"released").unwrap();
    let moved = w.id.run.mirror.with_extension("moved");
    std::fs::rename(&w.id.run.mirror, &moved).unwrap();
    assert_eq!(runner.recover().unwrap(), 1);
    let view = rm.get(&r.id).unwrap();
    assert_eq!(view.status, RunStatus::Interrupted);
    assert!(view.detail.unwrap().contains("mirror check failed"));
    assert!(matches!(
        rm.begin(&r.id, TurnMode::Retry),
        Err(Error::ConflictError)
    ));
    std::fs::rename(moved, &w.id.run.mirror).unwrap();
    let retry = rm.begin(&r.id, TurnMode::Retry).unwrap();
    assert_eq!(retry.attempt.attempt_no, 2);
}

#[test]
fn concurrent_admission_replays_without_spending_the_budget_twice_and_rejects_mutation() {
    let (_tmp, rm, _runner, req) = fixture();
    let start = std::sync::Arc::new(std::sync::Barrier::new(2));
    let [a, b] = [0, 1]
        .map(|_| {
            let (rm, req, start) = (rm.clone(), req.clone(), start.clone());
            std::thread::spawn(move || {
                start.wait();
                rm.accept(req).unwrap()
            })
        })
        .map(|t| t.join().unwrap());
    assert_eq!(a.0.id, b.0.id);
    assert_ne!(a.1, b.1);
    let mut changed = req.clone();
    changed.instructions.push_str(" changed");
    assert!(matches!(rm.accept(changed), Err(Error::ConflictError)));
    let (_, new) = rm.accept(req).unwrap();
    assert!(!new);
    rm.store
        .read(|db| {
            assert_eq!(
                db.query_row("SELECT sum(runs_used) FROM tickets", [], |r| r
                    .get::<_, u32>(0))?,
                1
            );
            assert_eq!(
                db.query_row(
                    "SELECT count(*) FROM sessions WHERE role='worker'",
                    [],
                    |r| r.get::<_, u32>(0)
                )?,
                1
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn allow_retry_survives_restart_and_attempt_limit_is_enforced() {
    let (_tmp, rm, runner, req) = fixture();
    let (r, _) = rm.accept(req).unwrap();
    let mut mode = TurnMode::Start;
    for n in 1..=3 {
        let work = rm.begin(&r.id, mode).unwrap();
        assert_eq!(work.attempt.attempt_no, n);
        crashed(&rm, &work, "previous-boot".into());
        assert_eq!(runner.recover().unwrap(), 1);
        if n < 3 {
            rm.resolve(&r.id, Resolution::AllowRetry, "continue fresh")
                .unwrap();
            assert_eq!(runner.recover().unwrap(), 0);
            assert_eq!(
                rm.get(&r.id).unwrap().resolution.as_deref(),
                Some("allow_retry")
            );
        }
        mode = TurnMode::Retry;
    }
    assert!(matches!(
        rm.begin(&r.id, mode),
        Err(Error::BudgetExceededError)
    ));
}

#[test]
fn restart_does_not_replay_pending_run_or_clean_current_release_evidence() {
    let (_tmp, rm, runner, req) = fixture();
    let (r, _) = rm.accept(req).unwrap();
    rm.store.write(|db|{
        db.execute("INSERT INTO turns(id,session_id,mode,status,phase,host_instance_id,boot_epoch,mcp_token,started_at)
            VALUES(99,'hub','hub','running','authorized','new','fixture','fixture-live','now')",[])?;
        Ok(())
    }).unwrap();
    std::fs::write(runner.marker(99), b"fixture").unwrap();
    assert_eq!(runner.recover().unwrap(), 0);
    assert!(runner.marker(99).exists());
    let interrupted = rm.get(&r.id).unwrap();
    assert_eq!(interrupted.status, RunStatus::Interrupted);
    assert_eq!(interrupted.current_attempt, 0);
    assert_eq!(
        rm.begin(&r.id, TurnMode::Retry).unwrap().attempt.attempt_no,
        1
    );
}

#[test]
fn recovery_clears_zombie_running_run_without_running_turn() {
    let (_tmp, rm, runner, req) = fixture();
    let (r, _) = rm.accept(req).unwrap();
    let _work = rm.begin(&r.id, TurnMode::Start).unwrap();
    assert_eq!(rm.get(&r.id).unwrap().status, RunStatus::Running);
    assert_eq!(runner.recover().unwrap(), 0);
    let recovered = rm.get(&r.id).unwrap();
    assert_eq!(recovered.status, RunStatus::Interrupted);
}
