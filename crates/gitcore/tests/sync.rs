//! Stage 7 acceptance (SPEC §10.4): everyday sync (pull / pull --rebase,
//! alongside the stage-6 fetch/push) and the `--no-ff` merge — the
//! `CommandPlan` shape of every op (exact argv, risk, preview) and the
//! post-op repository state, end to end against real git repositories with
//! a **local bare remote** (no network).
//!
//! The full path the app uses is exercised: `Repo::plan(&Op)` →
//! `GitExecutor::run_plan` → the real `git` binary, with the result asserted
//! through the fixture's own git calls. Requires a `git` binary on `PATH`
//! (platform gate of the stage: macOS primary; the Windows gate for
//! network fetch/push is noted where unavailable).

use git_gotten_protocol::{Op, Risk};
use git_gotten_testutil as testutil;
use gitcore::{GitExecutor, Repo};

/// A unique suffix for per-test scratch directories.
static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
fn unique() -> u64 {
    SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// Make one commit on the *remote side* (via a throwaway clone of the bare
/// remote at `remote_path`) and refresh the local remote-tracking ref, so the
/// local repository is behind.
fn remote_side_commit(
    fx: &testutil::Fixture,
    remote_path: &std::path::Path,
    file: &str,
    message: &str,
) {
    let clone = std::env::temp_dir().join(format!(
        "gitgotten-sync-clone-{}-{}",
        std::process::id(),
        unique()
    ));
    let (code, _, stderr) = fx.git_run(&[
        "clone",
        "--quiet",
        remote_path.to_str().unwrap(),
        clone.to_str().unwrap(),
    ]);
    assert_eq!(code, 0, "clone of the bare remote failed: {stderr}");
    let inner = testutil::Fixture::from_path(clone.clone());
    inner.git(&["config", "user.name", "Remote Bot"]);
    inner.git(&["config", "user.email", "remote@example.com"]);
    inner.write_file(file, "from the remote\n");
    inner.git(&["add", file]);
    inner.git(&["commit", "-m", message]);
    inner.git(&["push", "origin", "main"]);
    let _ = std::fs::remove_dir_all(&clone);
    // Refresh the local tracking ref so the read model (ahead/behind) sees it.
    fx.git(&["fetch", "-q", "origin"]);
}

/// `main` on a bare `origin`, with the branch state the stage-7 tests need:
/// local **ahead 1 / behind 1** (different files, so pull / pull --rebase /
/// merge are all conflict-free), plus a `side` branch that carries its own
/// commit (a different file: the merge tests' conflict-free, non-ancestor
/// source).
fn diverged_fixture() -> (testutil::Fixture, Repo) {
    let mut fx = testutil::Fixture::new("sync-div");
    fx.commit_file("a.txt", "base\n", "c1");
    let remote = fx.add_origin("main");
    fx.commit_file("b.txt", "local\n", "local c2"); // ahead 1
    remote_side_commit(&fx, &remote, "c.txt", "remote c2"); // behind 1
    // `side` diverges with its own commit: `git merge --no-ff` must really
    // merge (an ancestor source would only print "Already up to date").
    fx.git(&["checkout", "-b", "side", "HEAD~1"]);
    fx.commit_file("d.txt", "side\n", "side c3");
    fx.git(&["checkout", "main"]);
    let repo = Repo::open(&fx.path).unwrap();
    (fx, repo)
}

/// Plan an op, run it through the executor, and return the result (no success
/// assertion: the conflicted-merge tests expect `ok: false`).
async fn run(repo: &Repo, op: &Op) -> gitcore::OpResult {
    let plan = repo.plan(op).expect("plan must compile");
    GitExecutor::new()
        .run_plan(&repo.id, &repo.path, &plan)
        .await
}

/// Assert the plan is a single step with exactly this display string and risk.
fn assert_step(plan: &gitcore::CommandPlan, display: &str, risk: Risk) {
    assert_eq!(plan.steps.len(), 1);
    assert_eq!(plan.display, display);
    assert_eq!(plan.steps[0].risk, risk);
}

// ── pull: plan shapes ───────────────────────────────────────────────────────

#[test]
fn plan_pull_defaults_to_merge() {
    let (fx, repo) = diverged_fixture();
    let plan = repo.plan(&Op::Pull { rebase: false }).unwrap();
    // `--no-rebase` pins the merge reconciliation: git ≥ 2.21 refuses a bare
    // `git pull` on a diverged branch without an explicit preference.
    assert_step(&plan, "git pull --no-rebase", Risk::Mutating);
    let preview = plan.preview.as_ref().expect("pull previews");
    assert!(
        preview.lines.iter().any(|l| l.contains("origin/main")),
        "preview must name the upstream: {:?}",
        preview.lines
    );
    assert!(
        !preview.requires_type_to_confirm,
        "pull never type-to-confirms"
    );
    let _ = fx;
}

#[test]
fn plan_pull_rebase_adds_the_flag() {
    let (fx, repo) = diverged_fixture();
    let plan = repo.plan(&Op::Pull { rebase: true }).unwrap();
    assert_step(&plan, "git pull --rebase", Risk::Mutating);
    let _ = fx;
}

#[test]
fn plan_pull_without_upstream_warns_instead_of_running() {
    let fx = testutil::Fixture::new("sync-no-up");
    fx.commit_file("a.txt", "x\n", "c1");
    // No origin at all: `git pull` has no tracking information.
    let repo = Repo::open(&fx.path).unwrap();
    let plan = repo.plan(&Op::Pull { rebase: false }).unwrap();
    let preview = plan.preview.as_ref().expect("pull previews");
    assert!(
        preview.lines.iter().any(|l| l.contains("no upstream")),
        "preview must warn about the missing upstream: {:?}",
        preview.lines
    );
}

#[test]
fn plan_pull_on_detached_head_errors() {
    let fx = testutil::detached_fixture();
    let repo = Repo::open(&fx.path).unwrap();
    let err = repo.plan(&Op::Pull { rebase: false }).unwrap_err();
    assert!(matches!(err, gitcore::GitError::DetachedHead), "got {err}");
}

// ── pull: execution ─────────────────────────────────────────────────────────

#[tokio::test]
async fn pull_reconciles_a_behind_branch() {
    let (fx, mut repo) = diverged_fixture();
    let result = run(&repo, &Op::Pull { rebase: false }).await;
    assert!(result.ok, "stderr: {}", result.stderr);
    repo.refresh().unwrap();

    // The remote file is in the tree, and the branch caught up.
    assert!(fx.path.join("c.txt").exists(), "pulled file must exist");
    let status = repo.status().unwrap();
    assert_eq!(status.behind, 0, "must be up to date after pull");
    // The merge commit itself is ahead too: a subsequent push will send the
    // local commit *and* the merge (git's standard post-merge state).
    assert_eq!(status.ahead, 2);
    // A merge (not a rebase) was performed: HEAD has two parents.
    assert_eq!(
        fx.git_run(&["rev-parse", "--verify", "HEAD^2"]).0,
        0,
        "pull must merge"
    );
}

#[tokio::test]
async fn pull_rebase_reconciles_without_a_merge_commit() {
    let (fx, mut repo) = diverged_fixture();
    let result = run(&repo, &Op::Pull { rebase: true }).await;
    assert!(result.ok, "stderr: {}", result.stderr);
    repo.refresh().unwrap();

    // History is linear: the remote commit sits directly under the (rebased)
    // local commit, and the branch tip is the local commit.
    assert_eq!(fx.git(&["log", "-1", "--format=%s"]), "local c2");
    assert_eq!(
        fx.git(&["rev-parse", "HEAD~1"]),
        fx.git(&["rev-parse", "origin/main"]),
        "remote commit must be the new base"
    );
    let status = repo.status().unwrap();
    assert_eq!(status.behind, 0);
    assert_eq!(status.ahead, 1);
}

// ── merge: plan shapes ──────────────────────────────────────────────────────

#[test]
fn plan_merge_is_no_ff_with_a_commit_file_preview() {
    let (fx, repo) = diverged_fixture();
    let plan = repo
        .plan(&Op::Merge {
            source: "side".into(),
        })
        .unwrap();
    assert_step(&plan, "git merge --no-ff side", Risk::Mutating);
    let preview = plan.preview.as_ref().expect("merge previews");
    assert!(
        preview
            .lines
            .iter()
            .any(|l| l.contains("1 commit") && l.contains("1 file")),
        "preview must count commits + files coming in: {:?}",
        preview.lines
    );
    let _ = fx;
}

#[test]
fn plan_merge_of_an_ancestor_says_up_to_date() {
    let fx = testutil::Fixture::new("sync-merge-udd");
    fx.commit_file("a.txt", "one\n", "c1");
    fx.commit_file("b.txt", "two\n", "c2");
    fx.git(&["branch", "old", "HEAD~1"]);
    let repo = Repo::open(&fx.path).unwrap();
    let plan = repo
        .plan(&Op::Merge {
            source: "old".into(),
        })
        .unwrap();
    let preview = plan.preview.as_ref().expect("merge previews");
    assert!(
        preview.lines.iter().any(|l| l.contains("up to date")),
        "preview must say up to date: {:?}",
        preview.lines
    );
    let _ = fx;
}

#[test]
fn plan_merge_unknown_source_errors() {
    let (fx, repo) = diverged_fixture();
    let err = repo
        .plan(&Op::Merge {
            source: "nope".into(),
        })
        .unwrap_err();
    assert!(
        matches!(err, gitcore::GitError::NoCommit { .. }),
        "got {err}"
    );
    let _ = fx;
}

#[test]
fn plan_merge_with_blank_source_errors() {
    let (fx, repo) = diverged_fixture();
    let err = repo
        .plan(&Op::Merge {
            source: "   ".into(),
        })
        .unwrap_err();
    assert!(matches!(err, gitcore::GitError::EmptyTarget), "got {err}");
    let _ = fx;
}

// ── merge: execution ────────────────────────────────────────────────────────

#[tokio::test]
async fn clean_merge_produces_a_merge_commit() {
    let (fx, mut repo) = diverged_fixture();
    // `side` carries one commit on a different file: the merge is
    // conflict-free, `--no-ff` forces the merge commit.
    let result = run(
        &repo,
        &Op::Merge {
            source: "side".into(),
        },
    )
    .await;
    assert!(result.ok, "stderr: {}", result.stderr);
    repo.refresh().unwrap();

    let status = repo.status().unwrap();
    assert!(!status.in_merge, "no merge in progress after a clean merge");
    assert!(repo.unmerged_files().unwrap().is_empty());
    // HEAD is a real merge commit with `side` as the second parent.
    let (code, _, stderr) = fx.git_run(&["rev-parse", "--verify", "HEAD^2"]);
    assert_eq!(code, 0, "HEAD must have a second parent: {stderr}");
    assert_eq!(
        fx.git(&["rev-parse", "HEAD^2"]),
        fx.git(&["rev-parse", "side"])
    );
    assert_eq!(fx.git(&["log", "-1", "--format=%s"]), "Merge branch 'side'");
}

#[tokio::test]
async fn conflicted_merge_stops_and_reports_the_files() {
    // Two branches editing the same line of the same file.
    let fx = testutil::Fixture::new("sync-merge-conf");
    fx.commit_file("f.txt", "base\n", "c1");
    fx.git(&["checkout", "-b", "side"]);
    fx.commit_file("f.txt", "side\n", "c-side");
    fx.git(&["checkout", "main"]);
    fx.commit_file("f.txt", "main\n", "c-main");
    let mut repo = Repo::open(&fx.path).unwrap();

    let result = run(
        &repo,
        &Op::Merge {
            source: "side".into(),
        },
    )
    .await;
    assert!(!result.ok, "the merge must stop on the conflict");
    repo.refresh().unwrap();

    // The handoff to the conflict view (stage 8): the unmerged file list and
    // the mid-merge state.
    assert_eq!(repo.unmerged_files().unwrap(), vec!["f.txt".to_string()]);
    let status = repo.status().unwrap();
    assert!(status.in_merge, "repo must be mid-merge");
    assert!(
        status
            .changes
            .iter()
            .any(|c| c.kind == gitcore::ChangeKind::Conflicted && c.path == "f.txt"),
        "status must list f.txt as conflicted: {:?}",
        status.changes
    );
    // The conflicted file in the worktree carries git's markers.
    let content = std::fs::read_to_string(fx.path.join("f.txt")).unwrap();
    assert!(
        content.contains("<<<<<<<") && content.contains(">>>>>>>"),
        "conflict markers: {content}"
    );
}

// ── unmerged_files read model ───────────────────────────────────────────────

#[test]
fn unmerged_files_is_empty_outside_a_conflict() {
    let fx = testutil::status_fixture();
    let repo = Repo::open(&fx.path).unwrap();
    assert!(repo.unmerged_files().unwrap().is_empty());
}

#[test]
fn unmerged_files_cleared_after_abort() {
    let fx = testutil::merge_conflict_fixture();
    let mut repo = Repo::open(&fx.path).unwrap();
    assert_eq!(repo.unmerged_files().unwrap(), vec!["f.txt".to_string()]);
    // Abort the merge (stage 8's `abort_op` runs the same git command) and
    // the unmerged state must be gone.
    fx.git(&["merge", "--abort"]);
    repo.refresh().unwrap();
    assert!(repo.unmerged_files().unwrap().is_empty(), "after abort");
}
