//! Stage 5 acceptance (SPEC §10.2): per-hunk staging, commit, amend, and
//! discard — end to end against real git repositories.
//!
//! These exercise the full path the app uses: `Repo::plan(&Op)` →
//! `GitExecutor::run_plan` → the real `git` binary — and assert on the
//! resulting repository state (index, HEAD, worktree) through the fixture's
//! own git calls. Requires a `git` binary on `PATH` (platform gate of the
//! stage: macOS primary, Windows where marked).

use std::path::Path;

use git_gotten_protocol::{Author, Op, Risk};
use git_gotten_testutil as testutil;
use gitcore::{GitExecutor, Repo};

/// A committed 20-line file with lines 1 and 15 modified: far enough apart
/// (with 3 lines of context) to yield **two** hunks.
fn two_hunk_fixture(label: &str) -> testutil::Fixture {
    let fx = testutil::Fixture::new(label);
    let content = (1..=20).map(|i| format!("l{i}")).collect::<Vec<_>>().join("\n") + "\n";
    fx.commit_file("f.txt", &content, "c1");
    let mut lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();
    lines[0] = "l1X".to_string();
    lines[14] = "l15X".to_string();
    fx.write_file("f.txt", &(lines.join("\n") + "\n"));
    fx
}

/// Plan an op, run it through the executor, and expect success.
async fn run(repo: &Repo, fx: &testutil::Fixture, op: Op) -> gitcore::OpResult {
    let plan = repo.plan(&op).expect("plan must compile");
    let result = GitExecutor::new().run_plan(&repo.id, &repo.path, &plan).await;
    assert!(result.ok, "op must succeed, stderr: {}", result.stderr);
    let _ = fx; // fixtures are the assertion vehicle; kept for call-site symmetry
    result
}

// ── diff_file ──────────────────────────────────────────────────────────────

#[test]
fn diff_file_reports_both_hunks_as_unstaged() {
    let fx = two_hunk_fixture("diff-two-hunk");
    let repo = Repo::open(&fx.path).unwrap();

    let diff = repo.diff_file("f.txt").unwrap();
    assert_eq!(diff.path, "f.txt");
    assert!(!diff.binary);
    assert_eq!(diff.hunks.len(), 2, "expected two hunks, patch:\n{}", diff.patch);
    assert!(diff.hunks.iter().all(|h| !h.staged));
    // `patch` is the side the user acts on: the unstaged diff.
    assert!(diff.patch.contains("+l1X"));
    assert!(diff.patch.contains("+l15X"));
}

#[test]
fn diff_file_of_an_untracked_file_is_all_added() {
    let fx = testutil::Fixture::new("diff-untracked");
    fx.commit_file("a.txt", "one\n", "c1");
    fx.write_file("u.txt", "alpha\nbeta\n");
    let repo = Repo::open(&fx.path).unwrap();

    let diff = repo.diff_file("u.txt").unwrap();
    assert_eq!(diff.hunks.len(), 1);
    assert!(diff.hunks.iter().all(|h| !h.staged));
    assert!(diff.hunks.iter().all(|h| h.lines.iter().all(|l| l.starts_with('+'))));
}

#[tokio::test]
async fn diff_file_flags_staged_and_unstaged_hunks_separately() {
    // 36-line file, changes at lines 1 / 15 / 30 (three isolated hunks).
    let fx = testutil::Fixture::new("diff-mixed");
    let content = (1..=36).map(|i| format!("l{i}")).collect::<Vec<_>>().join("\n") + "\n";
    fx.commit_file("f.txt", &content, "c1");
    let mut lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();
    lines[0] = "l1A".to_string();
    lines[14] = "l15B".to_string();
    fx.write_file("f.txt", &(lines.join("\n") + "\n"));
    // Stage only the first hunk.
    let repo = Repo::open(&fx.path).unwrap();
    let _ = run(&repo, &fx, Op::StageHunks { path: "f.txt".into(), selected: vec![0] }).await;
    // Now modify a third region in the worktree.
    let mut lines: Vec<String> = content.lines().map(|l| l.to_string()).collect();
    lines[0] = "l1A".to_string();
    lines[14] = "l15B".to_string();
    lines[29] = "l30C".to_string();
    fx.write_file("f.txt", &(lines.join("\n") + "\n"));

    let repo = Repo::open(&fx.path).unwrap(); // fresh handle: re-reads the new index
    let diff = repo.diff_file("f.txt").unwrap();
    assert_eq!(diff.hunks.len(), 3, "patch:\n{}", diff.patch);
    let flags: Vec<bool> = diff.hunks.iter().map(|h| h.staged).collect();
    assert_eq!(flags, vec![true, false, false]);
}

// ── hunk staging ────────────────────────────────────────────────────────────

#[tokio::test]
async fn stage_hunks_stages_exactly_the_selected_hunks() {
    let fx = two_hunk_fixture("stage-hunk");
    let repo = Repo::open(&fx.path).unwrap();

    // Select only hunk 1 (the line-1 change).
    run(&repo, &fx, Op::StageHunks { path: "f.txt".into(), selected: vec![0] }).await;

    // The index gained exactly hunk 1: `git diff --cached` shows it and only it.
    let staged = fx.git(&["diff", "--cached", "--", "f.txt"]);
    assert!(staged.contains("+l1X"), "hunk 1 must be staged:\n{staged}");
    assert!(!staged.contains("l15X"), "hunk 2 must NOT be staged:\n{staged}");
    // The worktree is untouched: `git diff` now shows only hunk 2.
    let unstaged = fx.git(&["diff", "--", "f.txt"]);
    assert!(unstaged.contains("+l15X"), "hunk 2 stays in the worktree:\n{unstaged}");
    assert!(!unstaged.contains("l1X"), "hunk 1 no longer differs from the index:\n{unstaged}");
}

#[tokio::test]
async fn commit_after_partial_staging_contains_only_the_staged_hunk() {
    let fx = two_hunk_fixture("commit-partial");
    let repo = Repo::open(&fx.path).unwrap();

    run(&repo, &fx, Op::StageHunks { path: "f.txt".into(), selected: vec![0] }).await;
    run(&repo, &fx, Op::Commit {
        message: "partial".into(),
        amend: false,
        author: None,
        co_authors: vec![],
    }).await;

    let committed = fx.git(&["show", "HEAD:f.txt"]);
    assert!(committed.contains("l1X"), "the commit contains hunk 1:\n{committed}");
    assert!(!committed.contains("l15X"), "the commit excludes hunk 2:\n{committed}");
    // The worktree still carries hunk 2 as an unstaged change.
    let status = Repo::open(&fx.path).unwrap().status().unwrap();
    let f = status.changes.iter().find(|c| c.path == "f.txt").unwrap();
    assert!(!f.staged, "hunk 2 remains unstaged after the commit");
}

// ── amend ───────────────────────────────────────────────────────────────────

#[tokio::test]
async fn amend_rewrites_head_in_place() {
    let fx = two_hunk_fixture("amend");
    let repo = Repo::open(&fx.path).unwrap();

    // Commit both hunks, then change one line again and amend.
    run(&repo, &fx, Op::Stage { path: None }).await;
    run(&repo, &fx, Op::Commit {
        message: "both".into(),
        amend: false,
        author: None,
        co_authors: vec![],
    }).await;
    assert_eq!(fx.git(&["rev-list", "--count", "HEAD"]).trim(), "2");

    let mut lines: Vec<String> = (1..=20).map(|i| format!("l{i}")).collect();
    lines[0] = "l1Y".to_string();
    fx.write_file("f.txt", &(lines.join("\n") + "\n"));
    run(&repo, &fx, Op::Stage { path: Some("f.txt".into()) }).await;
    run(&repo, &fx, Op::Commit {
        message: "both, amended".into(),
        amend: true,
        author: None,
        co_authors: vec![],
    }).await;

    // Amended in place: still two commits, HEAD carries the new content.
    assert_eq!(fx.git(&["rev-list", "--count", "HEAD"]).trim(), "2");
    assert_eq!(fx.git(&["log", "-1", "--format=%s"]).trim(), "both, amended");
    let committed = fx.git(&["show", "HEAD:f.txt"]);
    assert!(committed.contains("l1Y"));
    assert!(!committed.contains("l1X"));
}

#[tokio::test]
async fn commit_with_author_override_sets_the_author() {
    let fx = two_hunk_fixture("author-override");
    let repo = Repo::open(&fx.path).unwrap();
    run(&repo, &fx, Op::Stage { path: None }).await;

    let result = run(
        &repo,
        &fx,
        Op::Commit {
            message: "by jane".into(),
            amend: false,
            author: Some(Author {
                name: "Jane Doe".into(),
                email: "jane@example.com".into(),
            }),
            co_authors: vec!["Joe <joe@example.com>".into()],
        },
    )
    .await;
    assert!(result.plan.display.contains("--author"), "{}", result.plan.display);

    assert_eq!(fx.git(&["log", "-1", "--format=%an <%ae>"]).trim(), "Jane Doe <jane@example.com>");
    // The trailer landed in the message; the committer stays the fixture identity.
    assert!(fx.git(&["log", "-1", "--format=%B"]).contains("Co-authored-by: Joe <joe@example.com>"));
    assert_ne!(fx.git(&["log", "-1", "--format=%cn"]).trim(), "Jane Doe");
}

// ── stage / unstage ─────────────────────────────────────────────────────────

#[tokio::test]
async fn stage_then_unstage_round_trips_the_index() {
    let fx = two_hunk_fixture("stage-unstage");
    let repo = Repo::open(&fx.path).unwrap();

    run(&repo, &fx, Op::Stage { path: Some("f.txt".into()) }).await;
    let staged = fx.git(&["diff", "--cached", "--", "f.txt"]);
    assert!(staged.contains("+l1X"), "staged: {staged}");

    run(&repo, &fx, Op::Unstage { path: Some("f.txt".into()) }).await;
    // The index is back at HEAD; the content stays in the worktree.
    assert!(fx.git(&["diff", "--cached", "--", "f.txt"]).is_empty());
    assert!(fx.git(&["diff", "--", "f.txt"]).contains("+l1X"));
    let on_disk = std::fs::read_to_string(fx.path.join("f.txt")).unwrap();
    assert!(on_disk.contains("l1X") && on_disk.contains("l15X"), "worktree must be untouched");
}

#[tokio::test]
async fn stage_all_and_unstage_all_cover_everything_including_deletions() {
    let fx = testutil::Fixture::new("stage-all");
    fx.commit_file("a.txt", "one\n", "c1");
    fx.commit_file("b.txt", "two\n", "c1");
    fx.write_file("a.txt", "one-modified\n"); // unstaged change
    std::fs::remove_file(fx.path.join("b.txt")).unwrap(); // deletion
    fx.write_file("u.txt", "new\n"); // untracked
    let repo = Repo::open(&fx.path).unwrap();

    run(&repo, &fx, Op::Stage { path: None }).await;
    let staged = fx.git(&["diff", "--cached"]);
    assert!(staged.contains("one-modified"), "modified staged:\n{staged}");
    assert!(staged.contains("b.txt"), "deletion staged:\n{staged}");
    assert!(staged.contains("u.txt"), "untracked added:\n{staged}");

    run(&repo, &fx, Op::Unstage { path: None }).await;
    assert!(fx.git(&["diff", "--cached"]).is_empty(), "index back at HEAD");
    // Unstaging the added file returns it to untracked, content preserved.
    let status = Repo::open(&fx.path).unwrap().status().unwrap();
    let u = status.changes.iter().find(|c| c.path == "u.txt").unwrap();
    assert!(!u.staged && u.kind == gitcore::ChangeKind::Untracked);
    assert!(std::fs::read_to_string(fx.path.join("u.txt")).unwrap().contains("new"));
}

// ── discard ─────────────────────────────────────────────────────────────────

#[tokio::test]
async fn discard_restores_a_modified_file_from_head() {
    let fx = two_hunk_fixture("discard-modified");
    let repo = Repo::open(&fx.path).unwrap();

    // Stage one hunk first, so the discard must undo *both* sides.
    run(&repo, &fx, Op::StageHunks { path: "f.txt".into(), selected: vec![0] }).await;
    let result = run(&repo, &fx, Op::Discard { path: "f.txt".into() }).await;
    assert_eq!(result.plan.steps[0].risk, Risk::Dangerous);

    // The file is back at its committed content; the tree is clean.
    let on_disk = std::fs::read_to_string(fx.path.join("f.txt")).unwrap();
    assert!(!on_disk.contains("l1X") && !on_disk.contains("l15X"), "{on_disk}");
    let status = Repo::open(&fx.path).unwrap().status().unwrap();
    assert!(status.changes.is_empty(), "tree must be clean after discard: {status:?}");
}

#[tokio::test]
async fn discard_deletes_an_untracked_file() {
    let fx = testutil::Fixture::new("discard-untracked");
    fx.commit_file("a.txt", "one\n", "c1");
    fx.write_file("u.txt", "gone\n");
    let repo = Repo::open(&fx.path).unwrap();

    let result = run(&repo, &fx, Op::Discard { path: "u.txt".into() }).await;
    assert_eq!(result.plan.steps[0].risk, Risk::Dangerous);
    assert!(!Path::new(&fx.path).join("u.txt").exists(), "the file must be deleted");
    let status = Repo::open(&fx.path).unwrap().status().unwrap();
    assert!(status.changes.is_empty());
}

// ── identity ────────────────────────────────────────────────────────────────

#[test]
fn identity_reads_the_configured_user() {
    let fx = testutil::Fixture::new("identity");
    fx.commit_file("a.txt", "one\n", "c1");
    let repo = Repo::open(&fx.path).unwrap();
    // The fixture pins a local identity.
    let id = repo.identity().expect("the fixture configures a user");
    assert_eq!(id.name, "Test User");
    assert_eq!(id.email, "test@example.com");
}
