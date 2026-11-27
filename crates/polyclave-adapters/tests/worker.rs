use polyclave_adapters::{
    agy_args, agy_worker, claude_args, claude_worker, codex_args, codex_worker, dsh_args,
    dsh_worker, parse_agy, parse_claude, parse_codex, parse_dsh, prepare_isolated_home_from,
    ClaudeIsolation, Error, WorkerContext,
};
use polyclave_proto::{AgentEvent, Vendor};
use std::ffi::OsString;
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

struct MockClaudeIsolation;
impl ClaudeIsolation for MockClaudeIsolation {
    fn verified_command(
        &self,
        worker: &Path,
        args: &[OsString],
        _roots: &[PathBuf],
    ) -> Result<(PathBuf, Vec<OsString>), Error> {
        Ok((worker.to_path_buf(), args.to_vec()))
    }
}

#[test]
fn prepare_isolated_home_links_only_credentials_and_skills() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");
    let src_codex = dir.path().join("user-codex");
    let src_claude = dir.path().join("user-claude");
    for d in [
        &state,
        &src_codex,
        &src_codex.join("skills"),
        &src_claude,
        &src_claude.join("skills"),
    ] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(src_codex.join("auth.json"), "{}").unwrap();
    std::fs::write(src_codex.join("config.toml"), "ignored").unwrap();
    std::fs::write(src_claude.join(".credentials.json"), "{}").unwrap();
    std::fs::write(src_claude.join("settings.json"), "ignored").unwrap();

    let codex_iso = prepare_isolated_home_from(&state, Vendor::Codex, Some(&src_codex)).unwrap();
    assert!(codex_iso.join("auth.json").exists());
    assert!(codex_iso.join("skills").is_dir());
    assert!(!codex_iso.join("config.toml").exists());

    // Self-referential source_dir must be a safe no-op
    prepare_isolated_home_from(&state, Vendor::Codex, Some(&codex_iso)).unwrap();
    assert!(codex_iso.join("skills").is_dir());

    // Idempotent re-run when auth.json is removed from source
    std::fs::remove_file(src_codex.join("auth.json")).unwrap();
    prepare_isolated_home_from(&state, Vendor::Codex, Some(&src_codex)).unwrap();
    assert!(codex_iso.join("skills").is_dir());

    let claude_iso = prepare_isolated_home_from(&state, Vendor::Claude, Some(&src_claude)).unwrap();
    assert!(claude_iso.join(".credentials.json").exists());
    assert!(claude_iso.join("skills").is_dir());
    assert!(!claude_iso.join("settings.json").exists());
}

#[test]
fn claude_worker_sets_secure_storage_dir_when_isolated() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");
    let work = dir.path().join("work");
    let stage = dir.path().join("stage");
    let primary = dir.path().join("primary");
    let mirror = dir.path().join("mirror");
    let fake_cli = dir.path().join("fake-claude");
    for d in [&state, &state.join("tmp"), &work, &stage, &primary, &mirror] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(&fake_cli, "#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake_cli, std::fs::Permissions::from_mode(0o700)).unwrap();
    }
    let ctx = WorkerContext {
        executable: &fake_cli,
        state_dir: &state,
        worktree: &work,
        staging: &stage,
        primary_repo: &primary,
        final_mirror: &mirror,
        mcp_url: "http://127.0.0.1:4310/mcp",
        turn_id: 7,
        token: "fixture-turn-key",
        model: None,
        thread: None,
    };
    let cmd = claude_worker(&ctx, Uuid::nil(), Some(&MockClaudeIsolation)).unwrap();
    assert!(cmd.env.iter().any(|(k, _)| k == "CLAUDE_CONFIG_DIR"));
    assert!(cmd
        .env
        .iter()
        .any(|(k, _)| k == "CLAUDE_SECURESTORAGE_CONFIG_DIR"));
    if std::env::var_os("USER").is_some() {
        assert!(cmd.env.iter().any(|(k, _)| k == "USER"));
    }
    assert!(state.join("tmp/mcp-turn-7.json").is_file());
}

#[test]
fn agy_and_dsh_args_match_expected_flags() {
    let paths: Vec<_> = ["worktree", "staging"]
        .into_iter()
        .map(PathBuf::from)
        .collect();

    let agy = agy_args(
        &paths,
        Path::new("tmp/mcp-turn-3.json"),
        Some("gemini-3.8-flash"),
        Some("agy-thread-1"),
    );
    let agy_strs: Vec<_> = agy.iter().map(|s| s.to_str().unwrap()).collect();
    assert_eq!(
        &agy_strs[..6],
        [
            "-p",
            "--output-format",
            "stream-json",
            "--sandbox",
            "--mode",
            "auto"
        ]
    );
    assert!(agy_strs.windows(2).any(|w| w == ["--add-dir", "worktree"]));
    assert!(agy_strs.windows(2).any(|w| w == ["--add-dir", "staging"]));
    assert!(agy_strs
        .windows(2)
        .any(|w| w == ["--settings", "tmp/mcp-turn-3.json"]));
    assert!(agy_strs
        .windows(2)
        .any(|w| w == ["--resume", "agy-thread-1"]));
    assert!(agy_strs
        .windows(2)
        .any(|w| w == ["--model", "gemini-3.8-flash"]));

    let dsh = dsh_args(&paths, Some("deepseek-v4"), Some("dsh-session-9"));
    let dsh_strs: Vec<_> = dsh.iter().map(|s| s.to_str().unwrap()).collect();
    assert_eq!(
        &dsh_strs[..7],
        [
            "--profile",
            "headless",
            "--json",
            "--permission-mode",
            "workspace-write",
            "--approval-policy",
            "never"
        ]
    );
    assert!(dsh_strs.windows(2).any(|w| w == ["--add-dir", "worktree"]));
    assert!(dsh_strs.windows(2).any(|w| w == ["--add-dir", "staging"]));
    assert!(dsh_strs
        .windows(2)
        .any(|w| w == ["--session-id", "dsh-session-9"]));
    assert!(dsh_strs.windows(2).any(|w| w == ["--model", "deepseek-v4"]));
    assert_eq!(dsh_strs.last().copied(), Some("-"));
}

#[test]
fn agy_and_dsh_worker_reject_overlapping_write_roots_and_prepare_env() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");
    let work = dir.path().join("work");
    let stage = dir.path().join("stage");
    let primary = dir.path().join("primary");
    let mirror = dir.path().join("mirror");
    let overlap_mirror = stage.join("mirror");
    let fake_cli = dir.path().join("fake-cli");
    for d in [
        &state,
        &state.join("tmp"),
        &work,
        &stage,
        &primary,
        &mirror,
        &overlap_mirror,
    ] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(&fake_cli, "#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&fake_cli, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    let overlap_ctx = WorkerContext {
        executable: &fake_cli,
        state_dir: &state,
        worktree: &work,
        staging: &stage,
        primary_repo: &primary,
        final_mirror: &overlap_mirror,
        mcp_url: "http://127.0.0.1:4310/mcp",
        turn_id: 10,
        token: "fixture-turn-key",
        model: None,
        thread: None,
    };
    assert!(matches!(agy_worker(&overlap_ctx), Err(Error::Invalid)));
    assert!(matches!(dsh_worker(&overlap_ctx), Err(Error::Invalid)));

    let valid_ctx = WorkerContext {
        executable: &fake_cli,
        state_dir: &state,
        worktree: &work,
        staging: &stage,
        primary_repo: &primary,
        final_mirror: &mirror,
        mcp_url: "http://127.0.0.1:4310/mcp",
        turn_id: 11,
        token: "fixture-turn-key",
        model: None,
        thread: None,
    };
    let agy_cmd = agy_worker(&valid_ctx).unwrap();
    assert!(state.join("tmp/mcp-turn-11.json").is_file());
    assert!(agy_cmd
        .env
        .iter()
        .any(|(k, v)| k == "POLYCLAVE_MCP_TOKEN" && v == "fixture-turn-key"));
    assert!(agy_cmd
        .env
        .iter()
        .any(|(k, v)| k == "GEMINI_HOME" && Path::new(v).ends_with("isolated-homes/agy")));

    let dsh_cmd = dsh_worker(&valid_ctx).unwrap();
    assert!(dsh_cmd
        .env
        .iter()
        .any(|(k, v)| k == "POLYCLAVE_MCP_TOKEN" && v == "fixture-turn-key"));
    assert!(dsh_cmd
        .env
        .iter()
        .any(|(k, v)| k == "DSH_HOME" && Path::new(v).ends_with("isolated-homes/dsh")));
}

#[test]
fn agy_and_dsh_ndjson_parsers_emit_agent_events() {
    assert_eq!(
        parse_agy(r#"{"type":"init","session_id":"agy-1"}"#),
        Some(AgentEvent::Session { id: "agy-1".into() })
    );
    assert_eq!(
        parse_agy(r#"{"type":"message","role":"assistant","content":"hello agy"}"#),
        Some(AgentEvent::TextDelta {
            text: "hello agy".into()
        })
    );
    assert_eq!(
        parse_agy(r#"{"type":"message","role":"thought","content":"agy thinking"}"#),
        Some(AgentEvent::ReasoningDelta {
            text: "agy thinking".into()
        })
    );
    assert_eq!(
        parse_agy(r#"{"type":"result","status":"success"}"#),
        Some(AgentEvent::Finished { success: true })
    );
    assert_eq!(
        parse_agy(r#"{"type":"result","is_error":true}"#),
        Some(AgentEvent::Finished { success: false })
    );
    assert_eq!(parse_agy("not-json"), None);

    assert_eq!(
        parse_dsh(r#"{"type":"session.started","session_id":"dsh-1"}"#),
        Some(AgentEvent::Session { id: "dsh-1".into() })
    );
    assert_eq!(
        parse_dsh(
            r#"{"type":"item.completed","item":{"type":"agent_message","text":"hello dsh"}}"#
        ),
        Some(AgentEvent::TextDelta {
            text: "hello dsh".into()
        })
    );
    assert_eq!(
        parse_dsh(r#"{"type":"item.completed","item":{"type":"reasoning","text":"dsh thinking"}}"#),
        Some(AgentEvent::ReasoningDelta {
            text: "dsh thinking".into()
        })
    );
    assert_eq!(
        parse_dsh(r#"{"type":"turn.completed"}"#),
        Some(AgentEvent::Finished { success: true })
    );
    assert_eq!(
        parse_dsh(r#"{"type":"run.failed"}"#),
        Some(AgentEvent::Finished { success: false })
    );
    assert_eq!(parse_dsh("not-json"), None);
}

#[test]
fn prepare_isolated_home_links_only_credentials_and_skills_for_agy_and_dsh() {
    let dir = tempfile::tempdir().unwrap();
    let state = dir.path().join("state");
    let src_agy = dir.path().join("user-agy");
    let src_dsh = dir.path().join("user-dsh");
    for d in [
        &state,
        &src_agy,
        &src_agy.join("skills"),
        &src_dsh,
        &src_dsh.join("skills"),
    ] {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(src_agy.join("oauth_creds.json"), "{}").unwrap();
    std::fs::write(src_agy.join("settings.json"), "ignored").unwrap();
    std::fs::write(src_dsh.join("credentials.json"), "{}").unwrap();
    std::fs::write(src_dsh.join("config.toml"), "ignored").unwrap();

    let agy_iso = prepare_isolated_home_from(&state, Vendor::Agy, Some(&src_agy)).unwrap();
    assert!(agy_iso.join("oauth_creds.json").exists());
    assert!(agy_iso.join("skills").is_dir());
    assert!(!agy_iso.join("settings.json").exists());

    let dsh_iso = prepare_isolated_home_from(&state, Vendor::Dsh, Some(&src_dsh)).unwrap();
    assert!(dsh_iso.join("credentials.json").exists());
    assert!(dsh_iso.join("skills").is_dir());
    assert!(!dsh_iso.join("config.toml").exists());
}

#[cfg(target_os = "macos")]
#[test]
fn macos_seatbelt_isolation_allows_worktree_writes_and_blocks_outside_writes() {
    use polyclave_adapters::{ClaudeIsolation, MacosSeatbeltIsolation};
    use std::ffi::OsString;

    let dir = tempfile::tempdir().unwrap();
    let root = polyclave_os::canonical_real_path(dir.path()).unwrap();
    let state = root.join("state");
    let worktree = root.join("worktree");
    let staging = root.join("staging");
    let outside = root.join("outside");
    for d in [&state, &worktree, &staging, &outside] {
        std::fs::create_dir_all(d).unwrap();
    }

    let iso = MacosSeatbeltIsolation::detect(&state).expect("sandbox-exec on macOS");
    let bad_path = root.join("bad\"quote");
    assert!(iso.build_profile(&[bad_path]).is_err());

    let allowed_target = worktree.join("ok.txt");
    let (exe, args) = iso
        .verified_command(
            Path::new("/bin/sh"),
            &[
                OsString::from("-c"),
                OsString::from(format!("echo ok > {}", allowed_target.display())),
            ],
            &[worktree.clone(), staging.clone()],
        )
        .unwrap();
    let status = std::process::Command::new(&exe)
        .args(&args)
        .status()
        .unwrap();
    assert!(status.success(), "allowed write inside worktree failed");
    assert!(allowed_target.is_file());

    let blocked_target = outside.join("bad.txt");
    let (exe, args) = iso
        .verified_command(
            Path::new("/bin/sh"),
            &[
                OsString::from("-c"),
                OsString::from(format!("echo bad > {}", blocked_target.display())),
            ],
            &[worktree, staging],
        )
        .unwrap();
    let status = std::process::Command::new(&exe)
        .args(&args)
        .status()
        .unwrap();
    assert!(!status.success(), "write outside worktree must be denied");
    assert!(!blocked_target.exists());
}
