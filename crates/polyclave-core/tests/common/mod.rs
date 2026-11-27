use polyclave_core::{
    mirror::MirrorManager,
    runs::{RunManager, RunRequest},
};
use polyclave_proto::Vendor;
use polyclave_store::Store;
use std::{path::Path, process::Command, sync::Arc};
pub fn git(path: &Path, args: &[&str]) -> String {
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
pub fn repository() -> (tempfile::TempDir, RunManager, RunRequest) {
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
    (tmp, rm, request)
}
