mod common;
use common::{git, repository};
use polyclave_adapters::mcp::Mcp;
use polyclave_core::{
    runner::{RunnerConfig, TurnRunner},
    runs::{RunManager, RunRequest},
};
use polyclave_proto::{AgentEvent, RunDto, RunStatus, TurnMode};
use polyclave_store::Error;
use std::{
    io::Read,
    sync::{Arc, Mutex},
    time::Duration,
};

struct Fixture {
    _temp: tempfile::TempDir,
    manager: RunManager,
    runner: TurnRunner,
    request: RunRequest,
    server: tokio::task::JoinHandle<std::io::Result<()>>,
    events: Arc<Mutex<Vec<AgentEvent>>>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

async fn fixture() -> Fixture {
    let (tmp, manager, request) = repository();
    let host = "worker-suite".to_owned();
    let (addr, server) = Mcp::new(
        manager.store.clone(),
        host.clone(),
        Arc::new(manager.clone()),
    )
    .listen()
    .await
    .unwrap();
    let fake = std::path::PathBuf::from(env!("CARGO_BIN_EXE_polyclave-fake-agent"));
    let events = Arc::new(Mutex::new(Vec::new()));
    let event_log = events.clone();
    let runner = TurnRunner::new(
        manager.clone(),
        &tmp.path().join("state"),
        host,
        RunnerConfig {
            supervisor: fake.clone(),
            mcp_url: format!("http://{addr}/mcp"),
            codex: fake,
            claude: "unavailable".into(),
            claude_isolation: None,
        },
        move |_, event| event_log.lock().unwrap().push(event),
    )
    .unwrap();
    Fixture {
        _temp: tmp,
        manager,
        runner,
        request,
        server,
        events,
    }
}

async fn finished(job: std::thread::JoinHandle<Result<RunDto, Error>>) -> RunDto {
    tokio::time::timeout(
        Duration::from_secs(20),
        tokio::task::spawn_blocking(move || job.join().unwrap()),
    )
    .await
    .unwrap()
    .unwrap()
    .unwrap()
}
async fn run(f: &Fixture, prompt: &str) -> RunDto {
    let mut req = f.request.clone();
    req.instructions = prompt.into();
    let (admitted, _) = f.manager.accept(req).unwrap();
    let job = f
        .runner
        .schedule(&admitted.id, TurnMode::Start, prompt.into())
        .unwrap();
    finished(job).await
}
fn facts(f: &Fixture, id: &str) -> (u32, u32, i32) {
    f.manager.store.read(|db|Ok(db.query_row(
        "SELECT a.corrections,(SELECT count(*) FROM results WHERE run_id=a.run_id AND accepted=1),
            (SELECT COALESCE(exit_code,-1) FROM turns WHERE run_id=a.run_id ORDER BY id DESC LIMIT 1)
         FROM attempts a WHERE run_id=?1 ORDER BY attempt_no DESC LIMIT 1",[id],
         |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?)))?)).unwrap()
}
fn assert_cleanup(f: &Fixture) {
    assert!(f
        ._temp
        .path()
        .join("state/tmp")
        .read_dir()
        .unwrap()
        .next()
        .is_none());
    assert!(f._temp.path().join("state/isolated-homes/codex").is_dir());
}

#[tokio::test]
async fn verified_receipt_wins_even_if_the_worker_exits_nonzero() {
    let f = fixture().await;
    let done = run(&f, "Fixture: nonzero after receipt").await;
    assert_eq!(done.status, RunStatus::Done, "{done:?}");
    assert_eq!(facts(&f, &done.id), (0, 1, 19));
    assert_eq!(done.result_sha, done.observed_sha);
    assert_eq!(
        git(
            &f.request.repo_cwd,
            &["rev-parse", &format!("polyclave/{}", done.id)]
        ),
        done.result_sha.unwrap()
    );
    assert!(f
        .events
        .lock()
        .unwrap()
        .iter()
        .any(|e| matches!(e,AgentEvent::Session{id} if id=="fixture-thread")));
    assert!(!f
        .events
        .lock()
        .unwrap()
        .iter()
        .any(|e| matches!(e, AgentEvent::TextDelta { .. })));
    assert_cleanup(&f);
}

#[tokio::test]
async fn unsigned_push_stops_without_correction() {
    let f = fixture().await;
    let r = run(&f, "Fixture: no receipt").await;
    assert_eq!(r.status, RunStatus::NeedsReview);
    assert_eq!(facts(&f, &r.id), (0, 0, 0));
    assert!(r.observed_sha.is_some());
    assert!(r.result_sha.is_none());
    assert_cleanup(&f);
}

#[tokio::test]
async fn branch_advance_after_receipt_is_not_done() {
    let f = fixture().await;
    let r = run(&f, "Fixture: move tip after receipt").await;
    assert_eq!(r.status, RunStatus::NeedsReview);
    assert_eq!(facts(&f, &r.id), (0, 1, 0));
    assert!(r.observed_sha.is_some());
    assert!(r.result_sha.is_none());
    let wrong = "a".repeat(40);
    assert!(matches!(
        f.manager.resolve_seen(
            &r.id,
            polyclave_core::runs::Resolution::Accept,
            "seen stale SHA",
            Some(&wrong)
        ),
        Err(Error::ConflictError)
    ));
    let result = f
        .manager
        .resolve_seen(
            &r.id,
            polyclave_core::runs::Resolution::Accept,
            "seen current SHA",
            r.observed_sha.as_deref(),
        )
        .unwrap();
    assert_eq!(result.status, RunStatus::Done);
    assert_eq!(result.result_sha, r.observed_sha);
}

#[tokio::test]
async fn missing_result_gets_only_two_corrections() {
    let f = fixture().await;
    let r = run(&f, "Fixture: no result").await;
    assert_eq!(r.status, RunStatus::Failed, "{r:?}");
    assert_eq!(facts(&f, &r.id), (2, 0, 0));
    assert!(matches!(
        f.runner
            .schedule(&r.id, TurnMode::WorkerChat, "closed".into()),
        Err(Error::ConflictError)
    ));
    assert_cleanup(&f);
}

#[tokio::test]
async fn success_after_one_correction_stays_on_one_attempt() {
    let f = fixture().await;
    let r = run(&f, "Fixture: first turn asks for correction").await;
    assert_eq!(r.status, RunStatus::Done, "{r:?}");
    assert_eq!(r.current_attempt, 1);
    assert_eq!(facts(&f, &r.id), (1, 1, 0));
}
#[tokio::test]
async fn released_worker_cancels_without_correction_and_removes_only_turn_files() {
    let f = fixture().await;
    let (r, _) = f.manager.accept(f.request.clone()).unwrap();
    let barrier = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
    let address = barrier.local_addr().unwrap();
    let job = f
        .runner
        .schedule(
            &r.id,
            TurnMode::Start,
            format!("Fixture barrier: {address}"),
        )
        .unwrap();
    let (mut pending, _) = tokio::time::timeout(
        Duration::from_secs(10),
        tokio::task::spawn_blocking(move || barrier.accept().unwrap()),
    )
    .await
    .unwrap()
    .unwrap();
    pending
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut signal = [0; 8];
    pending.read_exact(&mut signal).unwrap();
    assert_eq!(&signal, b"started\n");
    let turn = f
        .manager
        .store
        .read(|db| {
            Ok(db.query_row(
                "SELECT id FROM turns WHERE run_id=?1 AND phase='authorized'",
                [&r.id],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert!(f.runner.marker(turn).exists());
    assert!(matches!(
        f.runner
            .schedule(&r.id, TurnMode::WorkerChat, "concurrent".into()),
        Err(Error::BusyError)
    ));
    f.runner.cancel(&r.id).unwrap();
    let r = finished(job).await;
    let expected = if polyclave_os::hard_stop_excludes_writers() {
        RunStatus::Interrupted
    } else {
        RunStatus::NeedsReview
    };
    assert_eq!(r.status, expected);
    assert_eq!(facts(&f, &r.id).0, 0);
    assert_cleanup(&f);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn burst_at_worker_exit_drains_stdout_and_releases_turn() {
    let (tmp, manager, request) = repository();
    let host = "worker-burst".to_owned();
    let (addr, server) = Mcp::new(
        manager.store.clone(),
        host.clone(),
        Arc::new(manager.clone()),
    )
    .listen()
    .await
    .unwrap();
    let (run, _) = manager.accept(request).unwrap();
    let fake = std::path::PathBuf::from(env!("CARGO_BIN_EXE_polyclave-fake-agent"));
    let count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let event_count = count.clone();
    let store = manager.store.clone();
    let runner = TurnRunner::new(
        manager.clone(),
        &tmp.path().join("state"),
        host,
        RunnerConfig {
            supervisor: fake.clone(),
            mcp_url: format!("http://{addr}/mcp"),
            codex: fake,
            claude: "unavailable".into(),
            claude_isolation: None,
        },
        move |run_id, event| match event {
            AgentEvent::Session { .. } => {
                let identity = store
                    .read(|db| {
                        Ok(db.query_row(
                            "SELECT os_proc_id,proc_started_at FROM turns WHERE run_id=?1",
                            [run_id],
                            |row| Ok((row.get::<_, String>(0)?, row.get(1)?)),
                        )?)
                    })
                    .unwrap();
                let supervisor = polyclave_os::ProcessIdentity {
                    pid: identity.0.parse().unwrap(),
                    started_at: identity.1,
                };
                // Hold the consumer while the producer fills the queue. The
                // supervisor's exit releases this barrier.
                let deadline = std::time::Instant::now() + Duration::from_secs(10);
                while polyclave_os::process_identity(supervisor.pid)
                    .unwrap()
                    .as_ref()
                    == Some(&supervisor)
                {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "supervisor did not exit"
                    );
                    std::thread::yield_now();
                }
                #[cfg(unix)]
                loop {
                    // Darwin's identity lookup can stop reporting an exiting
                    // child before waitpid reports the zombie.
                    let status = std::process::Command::new("ps")
                        .args(["-p", &supervisor.pid.to_string(), "-o", "stat="])
                        .output()
                        .unwrap();
                    let state = String::from_utf8(status.stdout).unwrap();
                    if state.trim().starts_with('Z') || !status.status.success() {
                        break;
                    }
                    assert!(
                        std::time::Instant::now() < deadline,
                        "supervisor was not waitable"
                    );
                }
            }
            AgentEvent::TextDelta { text } if text == "burst" => {
                event_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            _ => {}
        },
    )
    .unwrap();
    let job = runner
        .schedule(
            &run.id,
            TurnMode::Start,
            "Fixture: stdout burst before exit".into(),
        )
        .unwrap();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || done_tx.send(job.join()).ok());
    let done = done_rx.recv_timeout(Duration::from_secs(15));
    server.abort();
    assert_eq!(
        done.expect("worker did not settle")
            .unwrap()
            .unwrap()
            .status,
        RunStatus::Done
    );
    assert_eq!(count.load(std::sync::atomic::Ordering::Relaxed), 280);
    let revoked = manager
        .store
        .read(|db| {
            Ok(db.query_row(
                "SELECT mcp_revoked FROM turns WHERE run_id=?1",
                [&run.id],
                |r| r.get::<_, bool>(0),
            )?)
        })
        .unwrap();
    assert!(revoked);
    assert!(matches!(runner.cancel(&run.id), Err(Error::ConflictError)));
}
#[tokio::test]
async fn commit_and_stage_uses_only_the_authorized_attempt_and_preserves_r1() {
    use serde_json::{json, Value};

    let (tmp, manager, mut request) = repository();
    let source = manager
        .store
        .hub_enqueue("hub", "stage-test", "Modify allowed.txt")
        .unwrap();
    let ticket = manager
        .store
        .hub_ticket(
            "hub",
            source.id,
            "Stage test",
            json!({
                "goal": "Modify allowed.txt",
                "allowed_paths": ["allowed.txt"],
                "forbidden_actions": [],
                "acceptance_criteria": []
            }),
            2,
        )
        .unwrap();
    request.ticket_id = Some(ticket.id);
    let (run, _) = manager.accept(request).unwrap();
    let work = manager.begin(&run.id, TurnMode::Start).unwrap();
    let (host, key) = ("stage-host", "stage-fixture-token");
    let rid = run.id.clone();
    let worker = run.worker_session_id.clone();
    let attempt = work.attempt.id;
    manager.store.write(move |db| {
        db.execute("UPDATE attempts SET status='running' WHERE id=?1", [attempt])?;
        db.execute(
            "INSERT INTO turns(id,session_id,run_id,attempt_id,mode,status,phase,host_instance_id,boot_epoch,mcp_token,started_at)
             VALUES(1,?1,?2,?3,'start','running','authorized',?4,'boot',?5,'now')",
            rusqlite::params![worker, rid, attempt, host, key],
        )?;
        Ok(())
    }).unwrap();
    let (addr, server) = Mcp::new(manager.store.clone(), host, Arc::new(manager.clone()))
        .listen()
        .await
        .unwrap();
    let call = |token: &'static str, name: &'static str, args: Value| {
        tokio::task::spawn_blocking(move || staging_rpc(addr, token, name, args))
    };
    std::fs::write(work.workspace.cwd.join("allowed.txt"), "approved").unwrap();
    std::fs::write(work.workspace.cwd.join("outside.txt"), "outside mandate").unwrap();

    let denied = call(
        key,
        "commit_and_stage",
        json!({"message":"scope test","paths":["outside.txt"]}),
    )
    .await
    .unwrap();
    assert_eq!(denied["result"]["isError"], true, "{denied}");
    let unfiltered = call(key, "commit_and_stage", json!({"message":"all changes"}))
        .await
        .unwrap();
    assert_eq!(unfiltered["result"]["isError"], true, "{unfiltered}");
    let wrong_turn = call(
        "stale-fixture-token",
        "commit_and_stage",
        json!({"message":"stale"}),
    )
    .await
    .unwrap();
    assert_eq!(wrong_turn["http_status"], 401);
    let target = work.id.full_ref();
    assert!(git(
        &work.workspace.staging,
        &["for-each-ref", "--format=%(objectname)", &target]
    )
    .is_empty());

    let git_binary = polyclave_os::resolve_cli_binary(std::path::Path::new("git")).unwrap();
    let state = polyclave_os::canonical_real_path(&tmp.path().join("state")).unwrap();
    std::fs::create_dir_all(state.join("tmp")).unwrap();
    let url = format!("http://{addr}/mcp");
    let ctx = polyclave_adapters::WorkerContext {
        executable: &git_binary,
        state_dir: &state,
        worktree: &work.workspace.cwd,
        staging: &work.workspace.staging,
        primary_repo: &work.id.run.repo,
        final_mirror: &work.id.run.mirror,
        mcp_url: &url,
        turn_id: 1,
        token: key,
        model: None,
        thread: None,
    };

    let codex = polyclave_adapters::codex_worker(&ctx).unwrap();
    let sandbox_roots = codex
        .args
        .iter()
        .find_map(|arg| {
            arg.to_str()?
                .strip_prefix("sandbox_workspace_write.writable_roots=")
        })
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(sandbox_roots).unwrap(),
        json!([work.workspace.cwd])
    );
    assert!(codex
        .args
        .iter()
        .any(|arg| arg == "sandbox_workspace_write.exclude_tmpdir_env_var=true"));
    assert!(codex
        .args
        .iter()
        .any(|arg| arg == "sandbox_workspace_write.exclude_slash_tmp=true"));

    #[cfg(target_os = "macos")]
    {
        let isolation = polyclave_adapters::default_claude_isolation(&state).unwrap();
        let cmd =
            polyclave_adapters::claude_worker(&ctx, uuid::Uuid::new_v4(), Some(isolation.as_ref()))
                .unwrap();
        for path in [
            work.workspace.cwd.join(".git/fixture"),
            work.workspace.staging.join("fixture"),
        ] {
            let output = std::process::Command::new(&cmd.executable)
                .args(&cmd.args[..2])
                .args(["/bin/sh", "-c", "printf denied > \"$1\"", "fixture"])
                .arg(&path)
                .env_clear()
                .envs(cmd.env.iter().cloned())
                .output()
                .unwrap();
            assert!(!output.status.success(), "worker could write Git metadata");
            assert!(!path.exists());
        }
    }

    let staged = call(
        key,
        "commit_and_stage",
        json!({"message":"Commit allowed file","paths":["allowed.txt"]}),
    )
    .await
    .unwrap();
    assert_eq!(staged["result"]["isError"], false, "{staged}");
    let sha = staged["result"]["structuredContent"]["sha"]
        .as_str()
        .unwrap();
    let replay = call(
        key,
        "commit_and_stage",
        json!({"message":"retry","paths":["allowed.txt"]}),
    )
    .await
    .unwrap();
    assert_eq!(replay["result"]["structuredContent"]["sha"], sha);
    assert_eq!(git(&work.workspace.staging, &["rev-parse", &target]), sha);
    assert_eq!(
        git(
            &work.workspace.staging,
            &[
                "log",
                "-1",
                "--format=%(trailers:key=Run-Id,valueonly)",
                sha
            ]
        ),
        run.id
    );
    assert_eq!(
        git(
            &work.workspace.staging,
            &["diff", "--name-only", &run.base_sha, sha]
        ),
        "allowed.txt"
    );
    assert_eq!(
        git(&work.workspace.cwd, &["rev-parse", "HEAD"]),
        run.base_sha
    );
    assert_eq!(git(&work.id.run.repo, &["rev-parse", "HEAD"]), run.base_sha);
    assert!(!work.id.run.repo.join("allowed.txt").exists());
    assert!(git(
        &work.id.run.mirror,
        &["for-each-ref", "--format=%(objectname)", &target]
    )
    .is_empty());
    assert!(manager.get(&run.id).unwrap().result_sha.is_none());

    let receipt = call(
        key,
        "submit_result",
        json!({"run_id":run.id,"branch":work.attempt.branch,"sha":sha}),
    )
    .await
    .unwrap();
    assert_eq!(receipt["result"]["isError"], false, "{receipt}");
    assert_eq!(git(&work.id.run.mirror, &["rev-parse", &target]), sha);
    let late = call(
        key,
        "commit_and_stage",
        json!({"message":"after receipt","paths":["allowed.txt"]}),
    )
    .await
    .unwrap();
    assert_eq!(late["result"]["isError"], true, "{late}");
    server.abort();
}

fn staging_rpc(
    addr: std::net::SocketAddr,
    token: &str,
    name: &str,
    args: serde_json::Value,
) -> serde_json::Value {
    use std::io::Write;
    let mut stream = std::net::TcpStream::connect(addr).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let body = serde_json::json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":name,"arguments":args}}).to_string();
    write!(stream, "POST /mcp HTTP/1.1\r\nHost: {addr}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    let (headers, body) = response.split_once("\r\n\r\n").unwrap();
    if headers.starts_with("HTTP/1.1 401") {
        return serde_json::json!({"http_status": 401});
    }
    assert!(headers.starts_with("HTTP/1.1 200"), "{headers}");
    serde_json::from_str(body).unwrap()
}

#[tokio::test]
async fn revision_commit_and_stage_preserves_prior_attempt_changes_with_partial_paths() {
    use serde_json::{json, Value};

    let (_tmp, manager, mut request) = repository();
    let source = manager
        .store
        .hub_enqueue("hub", "rev-stage-test", "Modify a.rs and b.rs")
        .unwrap();
    let ticket = manager
        .store
        .hub_ticket(
            "hub",
            source.id,
            "Revision stage test",
            json!({
                "goal": "Modify a.rs and b.rs",
                "allowed_paths": ["a.rs", "b.rs"],
                "forbidden_actions": [],
                "acceptance_criteria": []
            }),
            2,
        )
        .unwrap();
    request.ticket_id = Some(ticket.id);
    let (run, _) = manager.accept(request).unwrap();
    let work1 = manager.begin(&run.id, TurnMode::Start).unwrap();
    let (host, key1, key2) = ("rev-stage-host", "rev-key-1", "rev-key-2");
    let rid = run.id.clone();
    let worker = run.worker_session_id.clone();
    let attempt1 = work1.attempt.id;
    manager
        .store
        .write(move |db| {
            db.execute("UPDATE attempts SET status='running' WHERE id=?1", [attempt1])?;
            db.execute(
                "INSERT INTO turns(id,session_id,run_id,attempt_id,mode,status,phase,host_instance_id,boot_epoch,mcp_token,started_at)
                 VALUES(1,?1,?2,?3,'start','running','authorized',?4,'boot',?5,'now')",
                rusqlite::params![worker, rid, attempt1, host, key1],
            )?;
            Ok(())
        })
        .unwrap();

    let (addr, server) = Mcp::new(manager.store.clone(), host, Arc::new(manager.clone()))
        .listen()
        .await
        .unwrap();
    let call = |token: &'static str, name: &'static str, args: Value| {
        tokio::task::spawn_blocking(move || staging_rpc(addr, token, name, args))
    };

    std::fs::write(work1.workspace.cwd.join("a.rs"), "v1 a\n").unwrap();
    std::fs::write(work1.workspace.cwd.join("b.rs"), "v1 b\n").unwrap();
    let staged1 = call(
        key1,
        "commit_and_stage",
        json!({"message": "attempt 1", "paths": ["a.rs", "b.rs"]}),
    )
    .await
    .unwrap();
    assert_eq!(staged1["result"]["isError"], false, "{staged1}");
    let sha1 = staged1["result"]["structuredContent"]["sha"]
        .as_str()
        .unwrap()
        .to_owned();

    let receipt1 = call(
        key1,
        "submit_result",
        json!({"run_id": run.id, "branch": work1.attempt.branch, "sha": sha1}),
    )
    .await
    .unwrap();
    assert_eq!(receipt1["result"]["isError"], false, "{receipt1}");

    let rid = run.id.clone();
    let sha1_db = sha1.clone();
    manager
        .store
        .write(move |db| {
            db.execute("UPDATE turns SET status='completed', mcp_revoked=1 WHERE id=1", [])?;
            db.execute("UPDATE attempts SET status='completed' WHERE id=?1", [attempt1])?;
            db.execute(
                "UPDATE runs SET status='done', result_sha=?2, result_source='agent', accepted_attempt_id=?3, detail=NULL WHERE id=?1",
                rusqlite::params![rid, sha1_db, attempt1],
            )?;
            Ok(())
        })
        .unwrap();

    let work2 = manager.begin_revision(&run.id).unwrap();
    assert_eq!(work2.attempt.attempt_no, 2);
    assert_eq!(
        manager
            .store
            .attempt_from_sha(work2.attempt.id)
            .unwrap()
            .as_deref(),
        Some(sha1.as_str())
    );

    let rid = run.id.clone();
    let worker = run.worker_session_id.clone();
    let attempt2 = work2.attempt.id;
    manager
        .store
        .write(move |db| {
            db.execute("UPDATE attempts SET status='running' WHERE id=?1", [attempt2])?;
            db.execute(
                "INSERT INTO turns(id,session_id,run_id,attempt_id,mode,status,phase,host_instance_id,boot_epoch,mcp_token,started_at)
                 VALUES(2,?1,?2,?3,'worker_chat','running','authorized',?4,'boot',?5,'now')",
                rusqlite::params![worker, rid, attempt2, host, key2],
            )?;
            Ok(())
        })
        .unwrap();

    // Modify ONLY a.rs in Attempt 2 and pass only paths: ["a.rs"]
    std::fs::write(work2.workspace.cwd.join("a.rs"), "v2 a\n").unwrap();
    let staged2 = call(
        key2,
        "commit_and_stage",
        json!({"message": "attempt 2 partial", "paths": ["a.rs"]}),
    )
    .await
    .unwrap();
    assert_eq!(staged2["result"]["isError"], false, "{staged2}");
    let sha2 = staged2["result"]["structuredContent"]["sha"]
        .as_str()
        .unwrap()
        .to_owned();

    assert_eq!(
        git(
            &work2.workspace.staging,
            &["rev-parse", &format!("{sha2}^")]
        ),
        sha1
    );
    assert_eq!(
        git(&work2.workspace.staging, &["show", &format!("{sha2}:a.rs")]),
        "v2 a"
    );
    assert_eq!(
        git(&work2.workspace.staging, &["show", &format!("{sha2}:b.rs")]),
        "v1 b"
    );

    let receipt2 = call(
        key2,
        "submit_result",
        json!({"run_id": run.id, "branch": work2.attempt.branch, "sha": sha2}),
    )
    .await
    .unwrap();
    assert_eq!(receipt2["result"]["isError"], false, "{receipt2}");
    let target2 = work2.id.full_ref();
    assert_eq!(git(&work2.id.run.mirror, &["rev-parse", &target2]), sha2);
    server.abort();
}

#[tokio::test]
async fn prelaunch_error_persists_into_run_detail_and_default_max_attempts_is_five() {
    let f = fixture().await;
    let mut req = f.request.clone();
    req.vendor = polyclave_proto::Vendor::Claude;
    let (admitted, _) = f.manager.accept(req).unwrap();
    assert_eq!(admitted.max_attempts, 5);
    let job = f
        .runner
        .schedule(
            &admitted.id,
            TurnMode::Start,
            "claude without isolation".into(),
        )
        .unwrap();
    let settled = finished(job).await;
    assert_eq!(settled.status, RunStatus::Interrupted);
    assert_eq!(
        settled.detail.as_deref(),
        Some("Claude worker not started: no verified external filesystem and network sandbox is available")
    );
}

#[tokio::test]
async fn revision_commit_and_stage_resolves_earlier_attempt_when_previous_attempt_has_no_commit() {
    use serde_json::{json, Value};

    let (_tmp, manager, mut request) = repository();
    let source = manager
        .store
        .hub_enqueue("hub", "rev-skip-test", "Modify a.rs and b.rs")
        .unwrap();
    let ticket = manager
        .store
        .hub_ticket(
            "hub",
            source.id,
            "Revision skip test",
            json!({
                "goal": "Modify a.rs and b.rs",
                "allowed_paths": ["a.rs", "b.rs"],
                "forbidden_actions": [],
                "acceptance_criteria": []
            }),
            2,
        )
        .unwrap();
    request.ticket_id = Some(ticket.id);
    let (run, _) = manager.accept(request).unwrap();
    let work1 = manager.begin(&run.id, TurnMode::Start).unwrap();
    let (host, key1, key3) = ("rev-skip-host", "rev-skip-1", "rev-skip-3");
    let rid = run.id.clone();
    let worker = run.worker_session_id.clone();
    let attempt1 = work1.attempt.id;
    manager
        .store
        .write(move |db| {
            db.execute("UPDATE attempts SET status='running' WHERE id=?1", [attempt1])?;
            db.execute(
                "INSERT INTO turns(id,session_id,run_id,attempt_id,mode,status,phase,host_instance_id,boot_epoch,mcp_token,started_at)
                 VALUES(1,?1,?2,?3,'start','running','authorized',?4,'boot',?5,'now')",
                rusqlite::params![worker, rid, attempt1, host, key1],
            )?;
            Ok(())
        })
        .unwrap();

    let (addr, server) = Mcp::new(manager.store.clone(), host, Arc::new(manager.clone()))
        .listen()
        .await
        .unwrap();
    let call = |token: &'static str, name: &'static str, args: Value| {
        tokio::task::spawn_blocking(move || staging_rpc(addr, token, name, args))
    };

    std::fs::write(work1.workspace.cwd.join("a.rs"), "v1 a\n").unwrap();
    std::fs::write(work1.workspace.cwd.join("b.rs"), "v1 b\n").unwrap();
    let staged1 = call(
        key1,
        "commit_and_stage",
        json!({"message": "attempt 1", "paths": ["a.rs", "b.rs"]}),
    )
    .await
    .unwrap();
    assert_eq!(staged1["result"]["isError"], false, "{staged1}");
    let sha1 = staged1["result"]["structuredContent"]["sha"]
        .as_str()
        .unwrap()
        .to_owned();
    let receipt1 = call(
        key1,
        "submit_result",
        json!({"run_id": run.id, "branch": work1.attempt.branch, "sha": sha1}),
    )
    .await
    .unwrap();
    assert_eq!(receipt1["result"]["isError"], false, "{receipt1}");

    let rid = run.id.clone();
    let sha1_db = sha1.clone();
    manager
        .store
        .write(move |db| {
            db.execute("UPDATE turns SET status='completed', mcp_revoked=1 WHERE id=1", [])?;
            db.execute("UPDATE attempts SET status='completed' WHERE id=?1", [attempt1])?;
            db.execute(
                "UPDATE runs SET status='done', result_sha=?2, result_source='agent', accepted_attempt_id=?3, detail=NULL WHERE id=?1",
                rusqlite::params![rid, sha1_db, attempt1],
            )?;
            Ok(())
        })
        .unwrap();

    // Attempt 2 starts a revision from sha1, but is superseded without committing.
    let work2 = manager.begin_revision(&run.id).unwrap();
    assert_eq!(work2.attempt.attempt_no, 2);
    let rid = run.id.clone();
    let sha1_db = sha1.clone();
    let attempt2 = work2.attempt.id;
    manager
        .store
        .write(move |db| {
            db.execute("UPDATE attempts SET status='superseded' WHERE id=?1", [attempt2])?;
            db.execute(
                "UPDATE runs SET status='done', result_sha=?2, result_source='agent', accepted_attempt_id=?3, detail=NULL WHERE id=?1",
                rusqlite::params![rid, sha1_db, attempt1],
            )?;
            Ok(())
        })
        .unwrap();

    // Attempt 3 also revises from Attempt 1's sha1; Attempt 2 has no ref in the mirror.
    let work3 = manager.begin_revision(&run.id).unwrap();
    assert_eq!(work3.attempt.attempt_no, 3);
    let rid = run.id.clone();
    let worker = run.worker_session_id.clone();
    let attempt3 = work3.attempt.id;
    manager
        .store
        .write(move |db| {
            db.execute("UPDATE attempts SET status='running' WHERE id=?1", [attempt3])?;
            db.execute(
                "INSERT INTO turns(id,session_id,run_id,attempt_id,mode,status,phase,host_instance_id,boot_epoch,mcp_token,started_at)
                 VALUES(3,?1,?2,?3,'worker_chat','running','authorized',?4,'boot',?5,'now')",
                rusqlite::params![worker, rid, attempt3, host, key3],
            )?;
            Ok(())
        })
        .unwrap();

    std::fs::write(work3.workspace.cwd.join("a.rs"), "v3 a\n").unwrap();
    let staged3 = call(
        key3,
        "commit_and_stage",
        json!({"message": "attempt 3 partial", "paths": ["a.rs"]}),
    )
    .await
    .unwrap();
    assert_eq!(staged3["result"]["isError"], false, "{staged3}");
    let sha3 = staged3["result"]["structuredContent"]["sha"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        git(
            &work3.workspace.staging,
            &["rev-parse", &format!("{sha3}^")]
        ),
        sha1
    );
    assert_eq!(
        git(&work3.workspace.staging, &["show", &format!("{sha3}:b.rs")]),
        "v1 b"
    );
    server.abort();
}
