use polyclave_core::{
    mirror::MirrorManager,
    runner::{RunnerConfig, TurnRunner},
    runs::{Resolution, RunManager, RunRequest},
};
use polyclave_proto::{RunStatus, TurnMode, Vendor};
use polyclave_store::{Error, Store};
use rusqlite::params;
use std::{path::Path, process::Command, sync::Arc};

fn git(path: &Path, args: &[&str]) -> String {
    let result = Command::new("git")
        .arg("-C")
        .arg(path)
        .args(args)
        .output()
        .unwrap();
    assert!(
        result.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).unwrap().trim().to_owned()
}
fn fixture() -> (tempfile::TempDir, RunManager, TurnRunner, RunRequest) {
    let tmp = tempfile::tempdir().unwrap();
    let state = tmp.path().join("state");
    let repo = tmp.path().join("repo");
    std::fs::create_dir(&state).unwrap();
    std::fs::create_dir(&repo).unwrap();
    git(&repo, &["init", "-b", "main", "--template="]);
    git(
        &repo,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.invalid",
            "commit",
            "--allow-empty",
            "-m",
            "base",
        ],
    );
    let repo = polyclave_os::canonical_real_path(&repo).unwrap();
    let store = Store::open(state.join("test.sqlite")).unwrap();
    let repo_cwd = repo.to_string_lossy().into_owned();
    store
        .write(move |db| {
            db.execute_batch(
                "INSERT INTO provider_profiles(id,display_name,kind,models_json,created_at)
            VALUES('p','Test','codex_cli','[]','now');",
            )?;
            db.execute(
                "INSERT INTO sessions(id,name,role,repo_cwd,profile_id,provider_model,created_at)
            VALUES('hub','Hub','hub',?1,'p','fixture','now')",
                [repo_cwd],
            )?;
            Ok(())
        })
        .unwrap();
    let rm = RunManager::new(store, Arc::new(MirrorManager::new(&state).unwrap()));
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
    let request = RunRequest {
        hub_session_id: "hub".into(),
        request_id: "create".into(),
        vendor: Vendor::Codex,
        model: None,
        repo_cwd: repo,
        instructions: "change worktree".into(),
        base_branch: "main".into(),
        ticket_id: None,
    };
    (tmp, rm, runner, request)
}

fn crashed(rm: &RunManager, work: &polyclave_core::runs::RunWork, boot: String) -> i64 {
    let id = work.run.id.clone();
    let sid = work.run.worker_session_id.clone();
    let aid = work.attempt.id;
    rm.store.write(move|db|{
        db.execute("INSERT INTO turns(session_id,run_id,attempt_id,mode,status,phase,host_instance_id,boot_epoch,mcp_token,started_at)
            VALUES(?1,?2,?3,'start','running','authorized','old',?4,'fixture-only','now')",params![sid,id,aid,boot])?;
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
