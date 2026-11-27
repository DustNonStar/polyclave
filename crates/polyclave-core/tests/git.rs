use polyclave_core::{
    classify::{classify_run, BranchCheck},
    mirror::{AttemptGit, MirrorManager},
    scope::{check_touched_paths, validate_touched_paths, ScopeCheckResult},
    worktree::WorktreeManager,
    GitError,
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

#[test]
fn body_only_trailers_empty_ranges_wrong_bases_and_unreadable_refs_never_certify() {
    let (_tmp, repo, m) = fixture();
    let run = m.admit(&repo, ID, "main").unwrap();
    let a = AttemptGit { run, number: 1 };
    let w = m.prepare_attempt(&a).unwrap();
    push(&w.cwd, &a.full_ref());
    let BranchCheck::Present { tip, problem } = m.reconcile(&a) else {
        panic!("missing empty reference")
    };
    assert_eq!(tip, a.run.base);
    assert_eq!(problem.as_deref(), Some("commit range is empty"));
    commit(
        &w.cwd,
        &format!("Run-Id: {ID}\n\nText after apparent trailer"),
        None,
    );
    push(&w.cwd, &a.full_ref());
    let BranchCheck::Present { tip, problem } = m.reconcile(&a) else {
        panic!("missing malformed reference")
    };
    assert!(problem.unwrap().contains(&tip));

    let b = AttemptGit {
        run: a.run,
        number: 2,
    };
    let isolated = m.prepare_attempt(&b).unwrap();
    git(&isolated.cwd, &["checkout", "--orphan", "unrelated"]);
    let wrong = commit(&isolated.cwd, "wrong base", Some(ID));
    git(
        &isolated.cwd,
        &[
            "push",
            "polyclave-staging",
            &format!("HEAD:{}", b.full_ref()),
        ],
    );
    assert_eq!(
        m.reconcile(&b),
        BranchCheck::Present {
            tip: wrong,
            problem: Some("immutable base is not an ancestor".into())
        }
    );
}

#[test]
fn reconcile_preserves_mirror_on_damaged_stage() {
    let (_tmp, repo, m) = fixture();
    let run = m.admit(&repo, ID, "main").unwrap();
    let a = AttemptGit { run, number: 1 };
    let w = m.prepare_attempt(&a).unwrap();
    let sha = commit(&w.cwd, "valid mirror commit", Some(ID));
    push(&w.cwd, &a.full_ref());
    assert_eq!(
        m.reconcile(&a),
        BranchCheck::Present {
            tip: sha.clone(),
            problem: None
        }
    );
    std::fs::remove_dir_all(&w.staging).unwrap();
    assert_eq!(
        m.reconcile(&a),
        BranchCheck::Present {
            tip: sha,
            problem: None
        }
    );
}

#[test]
fn touched_paths_scope_check_validates_globs_and_prefixes_on_mirror() {
    let (_tmp, repo, m) = fixture();
    let run = m.admit(&repo, ID, "main").unwrap();
    let a = AttemptGit {
        run: run.clone(),
        number: 1,
    };
    let w = m.prepare_attempt(&a).unwrap();

    std::fs::create_dir_all(w.cwd.join("crates/core/src")).unwrap();
    std::fs::create_dir_all(w.cwd.join("crates/other")).unwrap();
    std::fs::write(w.cwd.join("crates/core/src/lib.rs"), "pub fn a() {}\n").unwrap();
    std::fs::write(w.cwd.join("crates/core/README.md"), "# docs\n").unwrap();
    std::fs::write(w.cwd.join("crates/other/hack.rs"), "// outside\n").unwrap();
    git(&w.cwd, &["add", "."]);
    let tip = commit(&w.cwd, "add files across crates", Some(ID));
    push(&w.cwd, &a.full_ref());
    assert_eq!(
        m.reconcile(&a),
        BranchCheck::Present {
            tip: tip.clone(),
            problem: None
        }
    );

    // Prefix + glob allowing only crates/core/**
    let allowed =
        check_touched_paths(&run.mirror, &run.base, &tip, &["crates/core/**".to_owned()]).unwrap();
    assert_eq!(
        allowed,
        ScopeCheckResult {
            allowed: false,
            violations: vec!["crates/other/hack.rs".to_owned()],
        }
    );

    // Exact + prefix + wildcard allowing both
    let ok = check_touched_paths(
        &run.mirror,
        &run.base,
        &tip,
        &["crates/core/".to_owned(), "crates/other/*.rs".to_owned()],
    )
    .unwrap();
    assert_eq!(
        ok,
        ScopeCheckResult {
            allowed: true,
            violations: vec![],
        }
    );

    // Pure rule edge cases: prefix must not match sibling with shared prefix
    let sibling = validate_touched_paths(
        &[
            "src/foo/bar.rs".to_owned(),
            "src/foo.rs".to_owned(),
            "docs/a/b/c.md".to_owned(),
        ],
        &["src/foo".to_owned(), "docs/**/*.md".to_owned()],
    );
    assert_eq!(
        sibling,
        ScopeCheckResult {
            allowed: false,
            violations: vec!["src/foo.rs".to_owned()],
        }
    );
}

#[test]
fn disposable_review_worktree_isolates_writes_and_cleans_up_on_error_and_panic() {
    let (_tmp, repo, m) = fixture();
    let run = m.admit(&repo, ID, "main").unwrap();
    let a = AttemptGit {
        run: run.clone(),
        number: 1,
    };
    let w = m.prepare_attempt(&a).unwrap();
    std::fs::write(w.cwd.join("clean.txt"), "hello\n").unwrap();
    git(&w.cwd, &["add", "clean.txt"]);
    let head_sha = commit(&w.cwd, "clean commit", Some(ID));
    push(&w.cwd, &a.full_ref());
    assert_eq!(
        m.reconcile(&a),
        BranchCheck::Present {
            tip: head_sha.clone(),
            problem: None
        }
    );

    // 1. Success path: accidental writes & commits inside worktree do not pollute PR branch, and dir is removed
    let mut observed_wt_path = std::path::PathBuf::new();
    let res = WorktreeManager::with_disposable_review_worktree(&run.mirror, &head_sha, |wt| {
        observed_wt_path = wt.to_path_buf();
        assert_eq!(git(wt, &["rev-parse", "HEAD"]), head_sha);
        assert_eq!(
            std::fs::read_to_string(wt.join("clean.txt")).unwrap(),
            "hello\n"
        );
        std::fs::write(wt.join("accidental.txt"), "dirty").unwrap();
        git(wt, &["add", "accidental.txt"]);
        commit(wt, "accidental reviewer commit", None);
        Ok("reviewed")
    })
    .unwrap();
    assert_eq!(res, "reviewed");
    assert!(!observed_wt_path.exists());
    assert!(!observed_wt_path.parent().unwrap().exists());
    assert_eq!(git(&run.mirror, &["rev-parse", &a.full_ref()]), head_sha);

    // 2. Error path: worktree is still cleaned up
    let mut err_wt_path = std::path::PathBuf::new();
    let err = WorktreeManager::with_disposable_review_worktree(&run.mirror, &head_sha, |wt| {
        err_wt_path = wt.to_path_buf();
        assert!(err_wt_path.exists());
        Err::<(), _>(GitError::Invalid("simulated reviewer failure"))
    });
    assert!(err.is_err());
    assert!(!err_wt_path.exists());
    assert!(!err_wt_path.parent().unwrap().exists());

    // 3. Panic path: worktree is still cleaned up via Drop
    let panic_wt_path = std::sync::Arc::new(std::sync::Mutex::new(std::path::PathBuf::new()));
    let panic_wt_ref = panic_wt_path.clone();
    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _: Result<(), GitError> =
            WorktreeManager::with_disposable_review_worktree(&run.mirror, &head_sha, |wt| {
                *panic_wt_ref.lock().unwrap() = wt.to_path_buf();
                panic!("simulated reviewer panic");
            });
    }));
    assert!(panicked.is_err());
    let p = panic_wt_path.lock().unwrap().clone();
    assert!(!p.exists());
    assert!(!p.parent().unwrap().exists());

    // Mirror worktree list should only contain the main bare mirror
    let wt_list = git(&run.mirror, &["worktree", "list", "--porcelain"]);
    assert_eq!(wt_list.matches("worktree ").count(), 1);
}
