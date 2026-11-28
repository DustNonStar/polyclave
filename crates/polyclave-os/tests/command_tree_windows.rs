#![cfg(windows)]

use polyclave_os::{cancellable_output, process_identity};
use std::{
    cell::Cell,
    io,
    process::Command,
    time::{Duration, Instant},
};

fn leaf_pid(text: &str) -> Option<u32> {
    text.lines()
        .find_map(|line| line.strip_prefix("leaf:")?.trim().parse().ok())
}

#[test]
fn cancelling_broker_waits_for_git_like_descendant() {
    let scratch = tempfile::tempdir().unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_os-fixture"));
    command.arg("tree");
    let leaf = Cell::new(None);
    let deadline = Instant::now() + Duration::from_secs(10);
    let result = cancellable_output(command, scratch.path(), || {
        let stdout = std::fs::read_to_string(scratch.path().join("command.stdout"));
        if let Some(pid) = stdout.ok().as_deref().and_then(leaf_pid) {
            assert!(process_identity(pid)?.is_some());
            leaf.set(Some(pid));
            return Err(io::ErrorKind::Interrupted.into());
        }
        if Instant::now() >= deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        Ok(())
    });
    assert_eq!(result.unwrap_err().kind(), io::ErrorKind::Interrupted);
    assert!(process_identity(leaf.get().unwrap()).unwrap().is_none());
}

#[test]
fn broker_normal_exit_drains_descendants_before_returning_stdout() {
    let scratch = tempfile::tempdir().unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_os-fixture"));
    command.arg("orphan");
    let deadline = Instant::now() + Duration::from_secs(10);
    let output = cancellable_output(command, scratch.path(), || {
        if Instant::now() >= deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        Ok(())
    })
    .unwrap();
    assert!(output.status.success());
    let pid = leaf_pid(std::str::from_utf8(&output.stdout).unwrap()).unwrap();
    assert!(process_identity(pid).unwrap().is_none());
}
