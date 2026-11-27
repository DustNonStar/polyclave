use polyclave_core::{
    classify::{classify_run, BranchCheck},
    mirror::{AttemptGit, MirrorManager},
};
use polyclave_proto::{RunStatus, WriterCheck};
use std::{
    path::Path,
    process::Command,
    sync::{Arc, Barrier},
};

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
fn commit(path: &Path, message: &str, trailer: Option<&str>) -> String {
    let mut args = vec![
        "-c",
        "user.name=Test",
        "-c",
        "user.email=test@example.invalid",
        "-c",
        "commit.gpgSign=false",
        "commit",
        "--allow-empty",
        "-m",
        message,
    ];
    let tr = trailer.map(|v| format!("Run-Id: {v}"));
    if let Some(tr) = &tr {
        args.extend(["--trailer", tr]);
    }
    git(path, &args);
    git(path, &["rev-parse", "HEAD"])
}
const ID: &str = "ra0000001";
const OTHER: &str = "ra0000002";
fn fixture() -> (tempfile::TempDir, std::path::PathBuf, MirrorManager) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("project");
    let state = dir.path().join("state");
    std::fs::create_dir(&repo).unwrap();
    std::fs::create_dir(&state).unwrap();
    git(&repo, &["init", "-b", "main", "--template="]);
    commit(&repo, "original", None);
    let mirror = MirrorManager::new(&state).unwrap();
    (dir, repo, mirror)
}

#[test]
fn upstream_is_not_the_final_target_and_the_baseline_cannot_be_advanced() {
    let (_tmp, repo, m) = fixture();
    let original = m.admit(&repo, ID, "main").unwrap();
    git(
        &repo,
        &[
            "remote",
            "add",
            "origin",
            "https://example.invalid/project.git",
        ],
    );
    assert_eq!(original, m.admit(&repo, ID, "main").unwrap());
    git(
        &repo,
        &[
            "remote",
            "set-url",
            "origin",
            "ssh://example.invalid/project.git",
        ],
    );
    assert_eq!(original, m.admit(&repo, ID, "main").unwrap());
    git(
        &repo,
        &[
            "remote",
            "set-url",
            "--add",
            "--push",
            "origin",
            "https://example.invalid/elsewhere.git",
        ],
    );
    assert_eq!(original, m.admit(&repo, ID, "main").unwrap());
    git(
        &repo,
        &[
            "remote",
            "set-url",
            "--add",
            "--push",
            "origin",
            "ssh://example.invalid/another.git",
        ],
    );
    git(
        &repo,
        &[
            "remote",
            "set-url",
            "--add",
            "origin",
            "https://example.invalid/another.git",
        ],
    );
    assert_eq!(original, m.admit(&repo, ID, "main").unwrap());
    commit(&repo, "main grows", None);
    assert!(m.admit(&repo, ID, "main").is_err());
    assert_eq!(
        git(&original.mirror, &["rev-parse", &original.base_ref()]),
        original.base
    );
    for key in ["receive.denyDeletes", "receive.denyNonFastForwards"] {
        assert_eq!("true", git(&original.mirror, &["config", "--get", key]));
    }
}

fn push(cwd: &Path, branch: &str) {
    git(
        cwd,
        &["push", "polyclave-staging", &format!("{branch}:{branch}")],
    );
}

#[test]
fn recovery_import_without_receipt_is_review_and_missing_trailer_returns_the_real_tip() {
    let (_tmp, repo, m) = fixture();
    let run = m.admit(&repo, ID, "main").unwrap();
    let a = AttemptGit {
        run: run.clone(),
        number: 1,
    };
    let w = m.prepare_attempt(&a).unwrap();
    assert_eq!(m.reconcile(&a), BranchCheck::Absent);
    let wrong = commit(&w.cwd, "bad intermediate message", None);
    let tip = commit(&w.cwd, "valid tip", Some(ID));
    push(&w.cwd, &a.full_ref());
    git(&w.cwd, &["tag", "attempt-tag"]);
    git(
        &w.cwd,
        &["push", "polyclave-staging", "refs/tags/attempt-tag"],
    );
    git(
        &w.cwd,
        &[
            "push",
            "polyclave-staging",
            &format!("HEAD:refs/heads/agent/{OTHER}/a1"),
        ],
    );
    assert_eq!(m.check_run_branch(&a), BranchCheck::Absent);
    let found = m.reconcile(&a);
    assert_eq!(
        found,
        BranchCheck::Present {
            tip: tip.clone(),
            problem: Some(format!("commit {wrong} is missing its Run-Id trailer"))
        }
    );
    assert_eq!(
        classify_run(WriterCheck::Excluded, None, found.clone()).status,
        RunStatus::NeedsReview
    );
    assert_eq!(
        classify_run(WriterCheck::Excluded, Some(&tip), found).status,
        RunStatus::NeedsReview
    );
    assert_eq!(git(&run.mirror, &["rev-parse", &run.base_ref()]), run.base);
    assert_eq!(
        git(
            &run.mirror,
            &[
                "for-each-ref",
                "--format=%(refname)",
                "refs/heads",
                "refs/tags"
            ]
        ),
        a.full_ref()
    );
    let new = AttemptGit { run, number: 2 };
    let w2 = m.prepare_attempt(&new).unwrap();
    assert_eq!(git(&w2.cwd, &["rev-parse", "HEAD"]), new.run.base);
    assert_ne!(w2.cwd, w.cwd);
    assert_ne!(w2.staging, w.staging);
    assert_eq!(m.reconcile(&new), BranchCheck::Absent);
}

#[test]
fn parallel_attempts_have_isolated_metadata_and_go_through_one_mirror_lock() {
    let (_tmp, repo, m) = fixture();
    let m = Arc::new(m);
    let start = Arc::new(Barrier::new(2));
    let th = [ID, OTHER].map(|id| {
        let (m, start, repo) = (m.clone(), start.clone(), repo.clone());
        std::thread::spawn(move || {
            start.wait();
            let run = m.admit(&repo, id, "main").unwrap();
            let a = AttemptGit { run, number: 1 };
            let w = m.prepare_attempt(&a).unwrap();
            for (file, contents) in [("one.txt", "one"), ("two.txt", "two")] {
                std::fs::write(w.cwd.join(file), contents).unwrap();
                git(&w.cwd, &["add", file]);
                commit(&w.cwd, file, Some(id));
            }
            let tip = git(&w.cwd, &["rev-parse", "HEAD"]);
            push(&w.cwd, &a.full_ref());
            assert_eq!(
                m.reconcile(&a),
                BranchCheck::Present {
                    tip: tip.clone(),
                    problem: None
                }
            );
            let summary = m.commit_summary(&a, &tip).unwrap();
            for name in ["one.txt", "two.txt"] {
                assert!(summary.files.contains(name));
                assert!(summary.commits.contains(name));
            }
            assert_eq!(
                classify_run(WriterCheck::Excluded, None, m.reconcile(&a)).status,
                RunStatus::NeedsReview
            );
            let done = classify_run(WriterCheck::Excluded, Some(&tip), m.reconcile(&a));
            assert_eq!(done.status, RunStatus::Done);
            (a, w, done)
        })
    });
    let [(a, w, done), (b, v, _)] = th.map(|t| t.join().unwrap());
    assert_eq!(a.run.mirror, b.run.mirror);
    assert_ne!(w.cwd, v.cwd);
    assert_ne!(w.staging, v.staging);
    assert!(!w.cwd.join(".git").is_file());
    git(
        &repo,
        &["checkout", "-b", &format!("polyclave/{}", a.run.id)],
    );
    assert!(m
        .sync_done(&a, done.result_sha.as_deref().unwrap())
        .is_err());
    assert_eq!(done.status, RunStatus::Done);
    git(&repo, &["checkout", "main"]);
    m.sync_done(&a, done.result_sha.as_deref().unwrap())
        .unwrap();
    assert_eq!(
        git(&repo, &["rev-parse", &format!("polyclave/{}", a.run.id)]),
        done.result_sha.unwrap()
    );
}

#[test]
fn unreadable_state_always_interrupts_and_a_possible_writer_never_completes() {
    let (_tmp, repo, m) = fixture();
    let a = AttemptGit {
        run: m.admit(&repo, ID, "main").unwrap(),
        number: 1,
    };
    assert_eq!(m.reconcile(&a), BranchCheck::Unreadable);
    for writer in [WriterCheck::Excluded, WriterCheck::Possible] {
        assert_eq!(
            classify_run(writer, Some("tip"), BranchCheck::Unreadable).status,
            RunStatus::Interrupted
        );
    }
    assert_eq!(
        classify_run(
            WriterCheck::Possible,
            Some("tip"),
            BranchCheck::Present {
                tip: "tip".into(),
                problem: None
            }
        )
        .status,
        RunStatus::NeedsReview
    );
}
