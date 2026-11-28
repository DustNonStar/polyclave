#[cfg(target_os = "macos")]
#[test]
fn cancel_http_drains_paused_broker_push_before_acknowledging() {
    use std::{path::Path, process::Command};

    let temp = tempfile::tempdir().unwrap();
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cancel_staging_push.py");
    let output = Command::new("python3")
        .arg(fixture)
        .arg(env!("CARGO_BIN_EXE_polyclaved"))
        .arg(temp.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
