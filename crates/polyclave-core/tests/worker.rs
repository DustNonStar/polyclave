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
