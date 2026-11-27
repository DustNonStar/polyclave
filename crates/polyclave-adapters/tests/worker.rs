use polyclave_adapters::{
    claude_args, claude_worker, codex_args, codex_worker, parse_claude, parse_codex, Error,
    WorkerContext,
};
use polyclave_proto::AgentEvent;
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[test]
fn vendor_args_do_not_enable_user_configuration_or_disable_claude_tools() {
    let paths: Vec<_> = ["worktree", "staging"]
        .into_iter()
        .map(PathBuf::from)
        .collect();
    let args = codex_args(
        &paths,
        "http://127.0.0.1:4310/mcp",
        Some("model"),
        Some("native-thread"),
    )
    .unwrap();
    let args: Vec<_> = args.iter().map(|s| s.to_str().unwrap()).collect();
    assert_eq!(&args[..2], ["exec", "resume"]);
    assert_eq!(&args[args.len() - 3..], ["--", "native-thread", "-"]);
    assert!(!args.contains(&"--ignore-user-config"));
    assert!(args.contains(&"sandbox_workspace_write.network_access=false"));
    assert!(args.contains(&"sandbox_workspace_write.writable_roots=[\"worktree\",\"staging\"]"));
    let id = Uuid::nil();
    for resume in [false, true] {
        let args = claude_args(Path::new("mock-config.json"), id, resume, None);
        assert!(args
            .iter()
            .any(|s| s == if resume { "--resume" } else { "--session-id" }));
        assert!(args.iter().any(|s| s == "--strict-mcp-config"));
        assert!(args
            .windows(2)
            .any(|p| p[0] == "--setting-sources" && p[1].is_empty()));
        assert!(!args
            .iter()
            .any(|s| s == "--bare" || s == "--tools" || s == "--dangerously-skip-permissions"));
        assert!(args
            .windows(2)
            .any(|p| p[0] == "--permission-mode" && p[1] == "bypassPermissions"));
    }
}

#[test]
fn unverified_claude_launch_fails_before_any_config_write_or_cli_resolution() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = WorkerContext {
        executable: Path::new("fake-unavailable-worker"),
        state_dir: dir.path(),
        worktree: dir.path(),
        staging: dir.path(),
        primary_repo: dir.path(),
        final_mirror: dir.path(),
        mcp_url: "http://127.0.0.1:4310/mcp",
        turn_id: 1,
        token: "fixture-turn-key",
        model: None,
        thread: None,
    };
    assert!(matches!(
        claude_worker(&ctx, Uuid::nil(), None),
        Err(Error::ClaudeIsolationUnavailable)
    ));
    assert!(dir.path().read_dir().unwrap().next().is_none());
}

#[test]
fn stream_events_use_the_same_contract() {
    let started = AgentEvent::Session {
        id: "fixture-thread".into(),
    };
    let message = AgentEvent::TextDelta {
        text: "hello".into(),
    };
    let thought = AgentEvent::ReasoningDelta {
        text: "thinking".into(),
    };
    assert_eq!(
        parse_codex(r#"{"type":"thread.started","thread_id":"fixture-thread"}"#),
        Some(started.clone())
    );
    assert_eq!(
        parse_claude(r#"{"type":"system","subtype":"init","session_id":"fixture-thread"}"#),
        Some(started)
    );
    assert_eq!(
        parse_codex(r#"{"type":"item.completed","item":{"type":"agent_message","text":"hello"}}"#),
        Some(message.clone())
    );
    assert_eq!(
        parse_claude(
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"hello"}}}"#
        ),
        Some(message)
    );
    assert_eq!(
        parse_claude(
            r#"{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"thinking_delta","thinking":"thinking"}}}"#
        ),
        Some(thought)
    );
    assert_eq!(
        parse_codex(r#"{"type":"turn.failed"}"#),
        Some(AgentEvent::Finished { success: false })
    );
    assert_eq!(
        parse_claude(r#"{"type":"result","is_error":false}"#),
        Some(AgentEvent::Finished { success: true })
    );
    assert_eq!(
        parse_claude(r#"{"type":"result","subtype":"success","result":"done"}"#),
        Some(AgentEvent::Finished { success: true })
    );
    assert_eq!(
        parse_claude(
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"non-stream"}]}}"#
        ),
        Some(AgentEvent::TextDelta {
            text: "non-stream".into(),
        })
    );
    assert_eq!(parse_claude("bad json"), None);
}

#[test]
fn worker_write_roots_cannot_overlap_host_paths() {
    let tmp = tempfile::tempdir().unwrap();
    let stage = tmp.path().join("staging");
    let mirror = stage.join("mirror");
    let work = tmp.path().join("work");
    let primary = tmp.path().join("primary");
    for path in [&stage, &mirror, &work, &primary] {
        std::fs::create_dir(path).unwrap();
    }
    let ctx = WorkerContext {
        executable: Path::new("offline-only"),
        state_dir: tmp.path(),
        worktree: &work,
        staging: &stage,
        primary_repo: &primary,
        final_mirror: &mirror,
        mcp_url: "http://127.0.0.1:4310/mcp",
        turn_id: 1,
        token: "fixture-turn-key",
        model: None,
        thread: None,
    };
    assert!(matches!(codex_worker(&ctx), Err(Error::Invalid)));
}
