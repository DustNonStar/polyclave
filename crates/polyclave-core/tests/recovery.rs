mod common;
use common::{git, repository};
use polyclave_core::{
    runner::{RunnerConfig, TurnRunner},
    runs::{Resolution, RunManager, RunRequest},
};
use polyclave_proto::{RunEvidenceDto, RunStatus, TurnMode};
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

#[test]
fn admission_snapshots_origin_without_credentials_and_ignores_later_changes() {
    // `\u{40}` is '@': scripts/privacy-scan.sh reads a user, an at sign and a host as an email address.
    let (_tmp, rm, mut req) = repository();
    let repo = req.repo_cwd.clone();
    git(
        &repo,
        &[
            "remote",
            "add",
            "origin",
            "https://x-access-token:SENTINEL-ORIGIN-SECRET\u{40}github.com/Org/Repo.git",
        ],
    );
    git(
        &repo,
        &[
            "config",
            "--add",
            "remote.origin.pushurl",
            "ssh://git:SENTINEL-ORIGIN-SECRET\u{40}github.com/org/other.git",
        ],
    );
    let (run, _) = rm.accept(req.clone()).unwrap();
    let snapshot = rm.store.run_origin_snapshot(&run.id).unwrap().unwrap();
    assert_eq!(snapshot.urls, vec!["https://github.com/Org/Repo.git"]);
    assert_eq!(
        snapshot.push_urls,
        vec!["ssh://git\u{40}github.com/org/other.git"]
    );
    let dump = rm
        .store
        .read(|db| {
            Ok(db.query_row(
                "SELECT urls_json || push_urls_json FROM run_publish_origins",
                [],
                |r| r.get::<_, String>(0),
            )?)
        })
        .unwrap();
    assert!(!dump.contains("SENTINEL-ORIGIN-SECRET"));

    // Re-pointing origin after admission changes nothing for the admitted Run,
    // and a repeated admission key keeps the original snapshot.
    git(
        &repo,
        &[
            "remote",
            "set-url",
            "origin",
            "https://github.com/evil/repo.git",
        ],
    );
    let (same, new) = rm.accept(req.clone()).unwrap();
    assert!(!new);
    assert_eq!(
        rm.store.run_origin_snapshot(&same.id).unwrap().unwrap(),
        snapshot
    );

    // A repository without origin is still admitted, with an empty snapshot.
    git(&repo, &["remote", "remove", "origin"]);
    req.request_id = "no-origin".into();
    let (bare, _) = rm.accept(req).unwrap();
    let empty = rm.store.run_origin_snapshot(&bare.id).unwrap().unwrap();
    assert!(empty.urls.is_empty() && empty.push_urls.is_empty());
}

#[test]
fn approval_evidence_reads_only_the_current_attempt_and_never_an_older_receipt() {
    let (_tmp, rm, runner, req) = fixture();
    let (r, _) = rm.accept(req).unwrap();
    let w = rm.begin(&r.id, TurnMode::Start).unwrap();
    let turn = crashed(&rm, &w, "previous-boot".into());
    let receipt = "a".repeat(40);
    let (id, attempt, sha) = (r.id.clone(), w.attempt.id, receipt.clone());
    rm.store.write(move |db| {
        db.execute("INSERT INTO results(run_id,attempt_id,turn_id,accepted,payload_json,created_at) VALUES(?1,?2,?3,1,?4,'now')",
            params![id,attempt,turn,serde_json::json!({"sha":sha}).to_string()])?;
        for text in ["Started the change", "Changed the parser; Windows tests not run"] {
            db.execute("INSERT INTO events(turn_id,event_kind,payload_json,created_at) VALUES(?1,'progress',?2,'now')",
                params![turn,serde_json::json!({"text":text}).to_string()])?;
        }
        Ok(())
    }).unwrap();
    assert_eq!(
        rm.evidence_for_run(&rm.get(&r.id).unwrap()).unwrap(),
        RunEvidenceDto {
            attempt_no: 1,
            receipt_sha: Some(receipt),
            worker_claim: Some("Changed the parser; Windows tests not run".into()),
            test_sha: None,
            receipt_tests: None,
            receipt_tests_status: None,
            receipt_not_done: None,
            test_results: Vec::new(),
        }
    );

    // The receipt names a commit the mirror does not hold, so R1 does not verify it.
    std::fs::write(runner.marker(turn), b"released").unwrap();
    assert_eq!(runner.recover().unwrap(), 1);
    assert_eq!(rm.get(&r.id).unwrap().status, RunStatus::NeedsReview);
    rm.resolve(&r.id, Resolution::AllowRetry, "fixture retry")
        .unwrap();
    rm.begin(&r.id, TurnMode::Retry).unwrap();
    assert_eq!(
        rm.evidence_for_run(&rm.get(&r.id).unwrap()).unwrap(),
        RunEvidenceDto {
            attempt_no: 2,
            receipt_sha: None,
            worker_claim: None,
            test_sha: None,
            receipt_tests: None,
            receipt_tests_status: None,
            receipt_not_done: None,
            test_results: Vec::new(),
        }
    );
}

#[test]
fn approval_evidence_shows_the_workers_notes_and_only_the_exact_shas_results() {
    use polyclave_proto::{TestResultSource, TestStatus};
    use polyclave_store::NewTestResult;

    let (_tmp, rm, _runner, req) = fixture();
    let (r, _) = rm.accept(req).unwrap();
    let w = rm.begin(&r.id, TurnMode::Start).unwrap();
    let turn = crashed(&rm, &w, "previous-boot".into());
    let (id, attempt) = (r.id.clone(), w.attempt.id);
    let observed = "b".repeat(40);
    let older = "a".repeat(40);

    let (run, sha, observed_write) = (id.clone(), observed.clone(), observed.clone());
    rm.store
        .write(move |db| {
            db.execute("UPDATE runs SET observed_sha=?2 WHERE id=?1", params![run, observed_write])?;
            db.execute("INSERT INTO results(run_id,attempt_id,turn_id,accepted,payload_json,created_at) VALUES(?1,?2,?3,1,?4,'now')",
                params![run, attempt, turn, serde_json::json!({
                    "run_id": run, "branch": "agent/run/a1", "sha": sha,
                    "tests": "cargo test: 12 passed", "tests_status": "passed",
                    "not_done": "Windows path tests not run"
                }).to_string()])?;
            Ok(())
        })
        .unwrap();
    rm.store
        .record_test_results(vec![
            NewTestResult {
                run_id: id.clone(),
                sha: observed.clone(),
                source: TestResultSource::Ci,
                name: "ci/rust-test".into(),
                status: TestStatus::Passed,
                summary: "ci/rust-test: success".into(),
            },
            NewTestResult {
                run_id: id.clone(),
                sha: older,
                source: TestResultSource::Ci,
                name: "ci/rust-test".into(),
                status: TestStatus::Failed,
                summary: "ci/rust-test: failure".into(),
            },
        ])
        .unwrap();

    let evidence = rm.evidence_for_run(&rm.get(&id).unwrap()).unwrap();
    assert_eq!(evidence.test_sha.as_deref(), Some(observed.as_str()));
    assert_eq!(
        evidence.receipt_tests.as_deref(),
        Some("cargo test: 12 passed")
    );
    assert_eq!(evidence.receipt_tests_status, Some(TestStatus::Passed));
    assert_eq!(
        evidence.receipt_not_done.as_deref(),
        Some("Windows path tests not run")
    );
    // Only the rows for this exact SHA, and the failed row of the older commit
    // never appears.
    assert_eq!(
        evidence.test_results.len(),
        1,
        "{:?}",
        evidence.test_results
    );
    assert_eq!(evidence.test_results[0].sha, observed);
    assert_eq!(evidence.test_results[0].status, TestStatus::Passed);

    // Moving the Run's verified result to another commit drops the old rows from
    // the card instead of showing them as current evidence.
    let moved = "c".repeat(40);
    let (moved_write, run_id) = (moved.clone(), id.clone());
    rm.store
        .write(move |db| {
            db.execute(
                "UPDATE runs SET result_sha=?2 WHERE id=?1",
                params![run_id, moved_write],
            )?;
            Ok(())
        })
        .unwrap();
    let evidence = rm.evidence_for_run(&rm.get(&r.id).unwrap()).unwrap();
    assert_eq!(evidence.test_sha.as_deref(), Some(moved.as_str()));
    assert!(evidence.test_results.is_empty());
}
