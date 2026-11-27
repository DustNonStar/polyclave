use polyclave_os::{
    boot_epoch, canonical_real_path, process_identity, spawn_supervised, SpawnSpec,
};
use std::io::BufRead;
use std::path::{Path, PathBuf};

fn spec(root: &Path) -> SpawnSpec {
    let executable = PathBuf::from(env!("CARGO_BIN_EXE_os-fixture"));
    SpawnSpec {
        supervisor: executable.clone(),
        worker: executable,
        args: Vec::new(),
        cwd: canonical_real_path(root).unwrap(),
        env: std::env::vars_os()
            .filter(|(key, _)| key.eq_ignore_ascii_case("SYSTEMROOT"))
            .collect(),
        released: root.join("turn-test.released"),
    }
}

#[test]
fn close_before_go_never_spawns_worker() {
    let dir = tempfile::tempdir().unwrap();
    let spec = spec(dir.path());
    let mut child = spawn_supervised(&spec).unwrap();
    assert_eq!(
        process_identity(child.identity().pid).unwrap().as_ref(),
        Some(child.identity())
    );
    child.cancel().unwrap();
    assert_eq!(child.wait().unwrap(), 125);
    assert!(!spec.released.exists());
}

#[test]
fn marker_is_written_before_worker_output_and_outlives_wrapper() {
    let dir = tempfile::tempdir().unwrap();
    let spec = spec(dir.path());
    let mut child = spawn_supervised(&spec).unwrap();
    let mut out = child.take_stdout().unwrap();
    child.go(b"fixture message").unwrap();
    let mut line = String::new();
    out.read_line(&mut line).unwrap();
    assert_eq!(line.trim(), "worker:fixture message");
    assert_eq!(std::fs::read(&spec.released).unwrap(), b"released\n");
    assert_eq!(child.wait().unwrap(), 0);
    assert!(spec.released.exists());
}

#[test]
fn closing_the_control_handle_stops_a_released_worker() {
    let dir = tempfile::tempdir().unwrap();
    let mut spec = spec(dir.path());
    spec.args.push("hold".into());
    let mut child = spawn_supervised(&spec).unwrap();
    let mut out = child.take_stdout().unwrap();
    child.go(b"").unwrap();
    let mut line = String::new();
    out.read_line(&mut line).unwrap();
    assert_eq!(line.trim(), "worker-ready");
    child.cancel().unwrap();
    assert_ne!(child.wait().unwrap(), 0);
    assert!(spec.released.exists());
}

#[test]
fn platform_identity_and_canonical_path_are_stable() {
    assert_eq!(boot_epoch().unwrap(), boot_epoch().unwrap());
    let pid = std::process::id();
    assert_eq!(process_identity(pid).unwrap().unwrap().pid, pid);
    let root = tempfile::tempdir().unwrap();
    let target = root.path().join("target");
    let alias = root.path().join("alias");
    std::fs::create_dir(&target).unwrap();
    polyclave_os::prepare_isolated_link(&target, &alias).unwrap();
    assert_eq!(
        canonical_real_path(&target).unwrap(),
        canonical_real_path(&alias).unwrap()
    );
}

#[test]
fn host_kill_excludes_a_released_descendant() {
    use std::io::BufReader;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    let dir = tempfile::tempdir().unwrap();
    let mut host = Command::new(env!("CARGO_BIN_EXE_os-fixture"))
        .arg("host")
        .arg(dir.path())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut output = BufReader::new(host.stdout.take().unwrap());
    let mut line = String::new();
    output.read_line(&mut line).unwrap();
    let leaf = line.trim().strip_prefix("leaf:").unwrap().parse().unwrap();
    let id = process_identity(leaf).unwrap().unwrap();
    host.kill().unwrap();
    host.wait().unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while process_identity(leaf).unwrap().as_ref() == Some(&id) {
        assert!(Instant::now() < deadline, "descendant survived host death");
        std::thread::yield_now();
    }
    assert!(dir.path().join("turn-tree.released").exists());
}
