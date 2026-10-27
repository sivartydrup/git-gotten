//! Stage 8 acceptance (SPEC §10.5): in-app conflict resolution — the 3-way
//! read of an unmerged file, whole-side and per-hunk resolution, and
//! continue / abort / external-editor for the three in-progress operations
//! (merge, rebase, cherry-pick), end to end against real git repositories
//! (the full path the app uses: `Repo::plan` → `GitExecutor::run_plan`).
//! Requires a `git` binary on `PATH` (the stage's platform gate).

use git_gotten_protocol::{
    ConflictResolution, ConflictSide, ConflictSegment, HunkChoice, Op, Risk,
};
use git_gotten_testutil as testutil;
use gitcore::{GitError, GitExecutor, Repo};

/// A stopped merge: `main` and `side` diverge on `f.txt` and the merge
/// stopped on a conflict (base `base` / ours `main` / theirs `side`).
fn merge_fixture() -> (testutil::Fixture, Repo) {
    let fx = testutil::merge_conflict_fixture();
    let repo = Repo::open(&fx.path).unwrap();
    (fx, repo)
}

/// A stopped rebase (the same divergence, rebased instead of merged).
fn rebase_fixture() -> (testutil::Fixture, Repo) {
    let fx = testutil::rebase_conflict_fixture();
    let repo = Repo::open(&fx.path).unwrap();
    (fx, repo)
}

/// A stopped cherry-pick of `side` onto a diverged `main`.
fn cherry_pick_fixture() -> (testutil::Fixture, Repo) {
    let fx = testutil::Fixture::new("cp-conflict");
    fx.commit_file("f.txt", "base\n", "c1");
    fx.git(&["checkout", "-b", "side"]);
    fx.commit_file("f.txt", "side\n", "c-side");
    fx.git(&["checkout", "main"]);
    fx.commit_file("f.txt", "main\n", "c-main");
    let (code, _, _) = fx.git_run(&["cherry-pick", "side"]);
    assert_ne!(code, 0, "the cherry-pick was expected to conflict");
    let repo = Repo::open(&fx.path).unwrap();
    (fx, repo)
}

/// A clean repository (no in-progress operation).
fn clean_fixture() -> (testutil::Fixture, Repo) {
    let fx = testutil::Fixture::new("conflict-clean");
    fx.commit_file("a.txt", "one\n", "c1");
    let repo = Repo::open(&fx.path).unwrap();
    (fx, repo)
}

/// Plan an op, run it through the executor, and return the result (no success
/// assertion: the safe-failure tests expect `ok: false`).
async fn run(repo: &Repo, op: &Op) -> gitcore::OpResult {
    let plan = repo.plan(op).expect("plan must compile");
    GitExecutor::new()
        .run_plan(&repo.id, &repo.path, &plan)
        .await
}

// ── the 3-way read (load_conflict's engine) ─────────────────────────────────

#[test]
fn conflict_file_reads_the_three_sides_from_the_index() {
    let (fx, repo) = merge_fixture();
    let cf = repo.conflict_file("f.txt").unwrap();
    assert_eq!(cf.path, "f.txt");
    assert!(!cf.binary);
    assert_eq!(cf.base, "base\n");
    assert_eq!(cf.ours, "main\n");
    assert_eq!(cf.theirs, "side\n");
    // One conflict region; the concatenated segment text is the marked file.
    assert_eq!(cf.segments.len(), 1);
    let mut all = String::new();
    for segment in &cf.segments {
        if let ConflictSegment::Conflict(h) = segment {
            all.push_str(&h.ours);
            all.push_str(&h.theirs);
        }
    }
    assert!(all.contains("main") && all.contains("side"));
    let _ = fx;
}

#[test]
fn conflict_file_reports_a_missing_stage_as_an_empty_side() {
    // base `one`; `side` deletes the file; `main` rewrites it → the merge
    // stops on a modify/delete conflict with no stage 1 (no base).
    let fx = testutil::Fixture::new("modify-delete");
    fx.commit_file("g.txt", "one\n", "c1");
    fx.git(&["checkout", "-b", "side"]);
    fx.git(&["rm", "g.txt"]);
    fx.git(&["commit", "-m", "delete g"]);
    fx.git(&["checkout", "main"]);
    fx.write_file("g.txt", "two\n");
    fx.add("g.txt");
    fx.git(&["commit", "-m", "rewrite g"]);
    let (code, _, _) = fx.git_run(&["merge", "side"]);
    assert_ne!(code, 0, "a modify/delete merge conflicts");
    let repo = Repo::open(&fx.path).unwrap();
    let cf = repo.conflict_file("g.txt").unwrap();
    assert_eq!(cf.base, "one\n"); // the file existed in the base commit
    assert_eq!(cf.ours, "two\n");
    assert_eq!(cf.theirs, ""); // the file was deleted on the theirs side
    assert!(cf.segments.iter().any(
        |s| matches!(s, ConflictSegment::Conflict(_))
    ));
    let _ = fx;
}

#[test]
fn conflict_file_on_a_non_conflicted_path_is_no_conflict() {
    let (fx, repo) = clean_fixture();
    let err = repo.conflict_file("a.txt").unwrap_err();
    assert!(matches!(err, GitError::NoConflict { .. }));
    let _ = fx;
}

#[test]
fn binary_conflict_file_carries_the_flag_and_no_text() {
    // Different binary bytes on each side: a content conflict with no text.
    let fx = testutil::Fixture::new("bin-conflict");
    fx.write_file("f.bin", "b\x00\x01 base");
    fx.add("f.bin");
    fx.git(&["commit", "-m", "c1"]);
    fx.git(&["checkout", "-b", "side"]);
    fx.write_file("f.bin", "b\x00\x02 side");
    fx.add("f.bin");
    fx.git(&["commit", "-m", "c-side"]);
    fx.git(&["checkout", "main"]);
    fx.write_file("f.bin", "b\x00\x03 main");
    fx.add("f.bin");
    fx.git(&["commit", "-m", "c-main"]);
    let (code, _, _) = fx.git_run(&["merge", "side"]);
    assert_ne!(code, 0, "binary bytes conflict");
    let repo = Repo::open(&fx.path).unwrap();
    let cf = repo.conflict_file("f.bin").unwrap();
    assert!(cf.binary);
    assert!(cf.segments.is_empty());
    assert!(cf.base.is_empty() && cf.ours.is_empty() && cf.theirs.is_empty());
    let _ = fx;
}

// ── resolve plans ───────────────────────────────────────────────────────────

#[test]
fn resolve_side_plan_is_checkout_then_add() {
    let (fx, repo) = merge_fixture();
    let plan = repo
        .plan(&Op::ResolveConflict {
            path: "f.txt".into(),
            resolution: ConflictResolution::Side(ConflictSide::Theirs),
        })
        .unwrap();
    assert_eq!(plan.steps.len(), 2);
    assert_eq!(plan.steps[0].argv, vec!["git", "checkout", "--theirs", "--", "f.txt"]);
    assert_eq!(plan.steps[1].argv, vec!["git", "add", "--", "f.txt"]);
    assert_eq!(
        plan.display,
        "git checkout --theirs -- f.txt && git add -- f.txt"
    );
    assert!(plan.steps.iter().all(|s| s.risk == Risk::Mutating));
    let preview = plan.preview.as_ref().unwrap();
    assert!(!preview.requires_type_to_confirm);
    let _ = fx;
}

#[test]
fn resolve_hunks_plan_is_apply_then_add() {
    let (fx, repo) = merge_fixture();
    let plan = repo
        .plan(&Op::ResolveConflict {
            path: "f.txt".into(),
            resolution: ConflictResolution::Hunks(vec![HunkChoice::AcceptTheirs]),
        })
        .unwrap();
    assert_eq!(plan.steps.len(), 2);
    assert_eq!(plan.steps[0].argv, vec!["git", "apply"]);
    assert!(plan.steps[0].input.is_some(), "the patch rides on stdin");
    assert!(plan.steps[0].input.as_ref().unwrap().contains("f.txt"));
    assert_eq!(plan.steps[1].argv, vec!["git", "add", "--", "f.txt"]);
    assert_eq!(
        plan.display,
        "git apply && git add -- f.txt"
    );
    let _ = fx;
}

#[test]
fn resolve_on_a_non_conflicted_file_errors() {
    let (fx, repo) = clean_fixture();
    let err = repo
        .plan(&Op::ResolveConflict {
            path: "a.txt".into(),
            resolution: ConflictResolution::Side(ConflictSide::Ours),
        })
        .unwrap_err();
    assert!(matches!(err, GitError::NoConflict { .. }));
    let _ = fx;
}

#[test]
fn per_hunk_resolution_of_a_binary_file_errors() {
    let fx = testutil::Fixture::new("bin-conflict-plan");
    fx.write_file("f.bin", "b\x00\x01 base");
    fx.add("f.bin");
    fx.git(&["commit", "-m", "c1"]);
    fx.git(&["checkout", "-b", "side"]);
    fx.write_file("f.bin", "b\x00\x02 side");
    fx.add("f.bin");
    fx.git(&["commit", "-m", "c-side"]);
    fx.git(&["checkout", "main"]);
    fx.write_file("f.bin", "b\x00\x03 main");
    fx.add("f.bin");
    fx.git(&["commit", "-m", "c-main"]);
    let (code, _, _) = fx.git_run(&["merge", "side"]);
    assert_ne!(code, 0);
    let repo = Repo::open(&fx.path).unwrap();
    let err = repo
        .plan(&Op::ResolveConflict {
            path: "f.bin".into(),
            resolution: ConflictResolution::Hunks(vec![HunkChoice::AcceptTheirs]),
        })
        .unwrap_err();
    assert!(matches!(err, GitError::BinaryConflict { .. }));
    let _ = fx;
}

// ── continue / abort plans ──────────────────────────────────────────────────

#[test]
fn continue_plan_labels_each_in_progress_op() {
    let (fx, repo) = merge_fixture();
    assert_eq!(repo.in_progress_op(), Some(gitcore::InProgressOp::Merge));
    let plan = repo.plan(&Op::ContinueOp).unwrap();
    assert_eq!(plan.display, "git merge --continue");
    assert_eq!(plan.steps[0].risk, Risk::Mutating);
    assert!(!plan.preview.as_ref().unwrap().requires_type_to_confirm);
    let _ = fx;

    let (fx, repo) = rebase_fixture();
    assert_eq!(repo.in_progress_op(), Some(gitcore::InProgressOp::Rebase));
    assert_eq!(repo.plan(&Op::ContinueOp).unwrap().display, "git rebase --continue");
    let _ = fx;

    let (fx, repo) = cherry_pick_fixture();
    assert_eq!(repo.in_progress_op(), Some(gitcore::InProgressOp::CherryPick));
    assert_eq!(
        repo.plan(&Op::ContinueOp).unwrap().display,
        "git cherry-pick --continue"
    );
    let _ = fx;
}

#[test]
fn continue_outside_an_operation_errors() {
    let (fx, repo) = clean_fixture();
    assert_eq!(repo.in_progress_op(), None);
    let err = repo.plan(&Op::ContinueOp).unwrap_err();
    assert!(matches!(err, GitError::NoOpInProgress));
    let _ = fx;
}

#[test]
fn abort_plan_is_dangerous_with_type_to_confirm() {
    let (fx, repo) = merge_fixture();
    let plan = repo.plan(&Op::AbortOp).unwrap();
    assert_eq!(plan.display, "git merge --abort");
    assert_eq!(plan.steps[0].risk, Risk::Dangerous);
    let preview = plan.preview.as_ref().unwrap();
    assert!(preview.requires_type_to_confirm);
    assert!(preview.lines.iter().any(|l| l.contains("restore")));
    let _ = fx;
}

#[test]
fn abort_outside_an_operation_errors() {
    let (fx, repo) = clean_fixture();
    let err = repo.plan(&Op::AbortOp).unwrap_err();
    assert!(matches!(err, GitError::NoOpInProgress));
    let _ = fx;
}

// ── external editor ─────────────────────────────────────────────────────────

#[test]
fn open_editor_plan_uses_the_configured_editor() {
    // The fixture pins `core.editor` to `git --version` (testutil): a real
    // program that exits 0 on any input — the hand-off shape, no GUI.
    let (fx, repo) = clean_fixture();
    let plan = repo
        .plan(&Op::OpenEditor {
            paths: vec!["a.txt".into()],
        })
        .unwrap();
    assert_eq!(plan.steps[0].argv, vec!["git", "--version", "a.txt"]);
    assert_eq!(plan.steps[0].program.as_deref(), Some("git"));
    assert_eq!(plan.display, "git --version a.txt");
    assert_eq!(plan.steps[0].risk, Risk::Mutating);
    let _ = fx;
}

#[test]
fn open_editor_with_no_paths_errors() {
    let (fx, repo) = clean_fixture();
    let err = repo
        .plan(&Op::OpenEditor { paths: vec![] })
        .unwrap_err();
    assert!(matches!(err, GitError::EmptySelection));
    let _ = fx;
}

// ── resolve: execution ──────────────────────────────────────────────────────

#[tokio::test]
async fn resolve_take_ours_then_continue_completes_the_merge() {
    let (fx, repo) = merge_fixture();
    let result = run(
        &repo,
        &Op::ResolveConflict {
            path: "f.txt".into(),
            resolution: ConflictResolution::Side(ConflictSide::Ours),
        },
    )
    .await;
    assert!(result.ok, "take-ours: {}", result.stderr);
    assert_eq!(
        std::fs::read_to_string(fx.path.join("f.txt")).unwrap(),
        "main\n",
        "the worktree holds the ours side"
    );
    assert!(
        repo.unmerged_files().unwrap().is_empty(),
        "the unmerged entry is cleared once resolved"
    );

    let result = run(&repo, &Op::ContinueOp).await;
    assert!(result.ok, "merge --continue: {}", result.stderr);
    assert!(
        fx.git(&["log", "-1", "--format=%P"])
            .split(' ')
            .count()
            == 2,
        "the merge commit has two parents"
    );
    assert_eq!(repo.in_progress_op(), None, "the merge concluded");
    assert!(!repo.status().unwrap().in_merge);
}

#[tokio::test]
async fn resolve_hunks_splices_their_side_into_the_file() {
    let (fx, repo) = merge_fixture();
    let result = run(
        &repo,
        &Op::ResolveConflict {
            path: "f.txt".into(),
            resolution: ConflictResolution::Hunks(vec![HunkChoice::AcceptTheirs]),
        },
    )
    .await;
    assert!(result.ok, "hunk splice: {}", result.stderr);
    assert_eq!(
        std::fs::read_to_string(fx.path.join("f.txt")).unwrap(),
        "side\n",
        "accepting the hunk's theirs side yields their content"
    );
    let _ = fx;
}

#[tokio::test]
async fn resolve_custom_choice_replaces_the_region() {
    let (fx, repo) = merge_fixture();
    let result = run(
        &repo,
        &Op::ResolveConflict {
            path: "f.txt".into(),
            resolution: ConflictResolution::Hunks(vec![HunkChoice::Custom(
                "both sides, reconciled\n".into(),
            )]),
        },
    )
    .await;
    assert!(result.ok, "custom splice: {}", result.stderr);
    assert_eq!(
        std::fs::read_to_string(fx.path.join("f.txt")).unwrap(),
        "both sides, reconciled\n"
    );
    let _ = fx;
}

#[tokio::test]
async fn stale_hunk_choices_are_rejected_at_plan_time() {
    let (fx, repo) = merge_fixture();
    // Two choices for one conflict region: the file cannot have changed
    // between viewing and resolving — this is a client error, surfaced
    // before anything runs.
    let err = repo
        .plan(&Op::ResolveConflict {
            path: "f.txt".into(),
            resolution: ConflictResolution::Hunks(vec![
                HunkChoice::AcceptOurs,
                HunkChoice::AcceptTheirs,
            ]),
        })
        .unwrap_err();
    assert!(
        matches!(err, GitError::ConflictMismatch { expected: 1, given: 2 }),
        "got {err:?}"
    );
    let _ = fx;
}

// ── continue / abort: execution ─────────────────────────────────────────────

#[tokio::test]
async fn continue_with_unresolved_files_fails_safely() {
    let (fx, repo) = merge_fixture();
    let result = run(&repo, &Op::ContinueOp).await;
    assert!(!result.ok, "git must refuse to finish a conflicted merge");
    assert!(
        result.stderr.contains("unmerged files"),
        "git's refusal is surfaced verbatim: {}",
        result.stderr
    );
    // The operation is untouched: still in progress, still conflicted.
    assert_eq!(repo.in_progress_op(), Some(gitcore::InProgressOp::Merge));
    assert!(!repo.unmerged_files().unwrap().is_empty());
    let _ = fx;
}

#[tokio::test]
async fn abort_merge_restores_the_pre_operation_state() {
    let (fx, repo) = merge_fixture();
    // A conflicted merge leaves markers in the worktree file.
    assert!(
        std::fs::read_to_string(fx.path.join("f.txt"))
            .unwrap()
            .contains("<<<<<<<")
    );
    let result = run(&repo, &Op::AbortOp).await;
    assert!(result.ok, "merge --abort: {}", result.stderr);
    assert_eq!(repo.in_progress_op(), None, "the merge markers are gone");
    assert!(!repo.status().unwrap().in_merge);
    assert!(
        repo.unmerged_files().unwrap().is_empty(),
        "the unmerged entries are cleared"
    );
    assert_eq!(
        std::fs::read_to_string(fx.path.join("f.txt")).unwrap(),
        "main\n",
        "the worktree is back to its pre-merge state"
    );
    // The local branch tip did not move (no merge commit was made).
    assert_eq!(
        fx.git(&["log", "-1", "--format=%s"]),
        "c-main",
        "main is exactly where it was before the merge"
    );
}

#[tokio::test]
async fn abort_rebase_restores_the_pre_rebase_state() {
    let (fx, repo) = rebase_fixture();
    let tip_before = fx.git(&["rev-parse", "main"]);
    let result = run(&repo, &Op::AbortOp).await;
    assert!(result.ok, "rebase --abort: {}", result.stderr);
    assert_eq!(repo.in_progress_op(), None);
    assert!(!repo.status().unwrap().in_rebase);
    assert_eq!(
        fx.git(&["rev-parse", "main"]),
        tip_before,
        "the branch tip is restored"
    );
    assert_eq!(
        std::fs::read_to_string(fx.path.join("f.txt")).unwrap(),
        "main\n",
        "the worktree is back to its pre-rebase state"
    );
}

#[tokio::test]
async fn cherry_pick_conflict_resolves_and_continues() {
    let (fx, repo) = cherry_pick_fixture();
    let result = run(
        &repo,
        &Op::ResolveConflict {
            path: "f.txt".into(),
            resolution: ConflictResolution::Side(ConflictSide::Theirs),
        },
    )
    .await;
    assert!(result.ok, "take-theirs: {}", result.stderr);
    let result = run(&repo, &Op::ContinueOp).await;
    assert!(result.ok, "cherry-pick --continue: {}", result.stderr);
    assert_eq!(repo.in_progress_op(), None);
    assert_eq!(
        std::fs::read_to_string(fx.path.join("f.txt")).unwrap(),
        "side\n"
    );
    // A new commit landed on main carrying the picked change.
    assert_eq!(fx.git(&["log", "-1", "--format=%s"]), "c-side");
}

// ── external editor: execution ──────────────────────────────────────────────

#[tokio::test]
async fn editor_handoff_runs_and_succeeds() {
    // `core.editor` is pinned to `git --version` by the fixture: the step
    // runs a real program that exits 0 — the hand-off shape.
    let (fx, repo) = clean_fixture();
    let result = run(
        &repo,
        &Op::OpenEditor {
            paths: vec!["a.txt".into()],
        },
    )
    .await;
    assert!(result.ok, "the editor step must succeed: {}", result.stderr);
    let _ = fx;
}
