//! Stage 10 acceptance (SPEC §10.7): worktrees — the `worktrees()` read model,
//! the `CommandPlan` shape of add/remove (exact argv + risk tier), per-worktree
//! graph checkout markers, and the post-op repository state, end to end against
//! real git repositories.
//!
//! The full path the app uses is exercised: `Repo::plan(&Op)` →
//! `GitExecutor::run_plan` → the real `git` binary. Requires a `git` binary on
//! `PATH` (macOS primary; the Windows gate for worktree paths + remove is
//! noted where a Windows machine is unavailable).

use std::path::{Path, PathBuf};

use git_gotten_protocol::{Op, RefKind, Risk};
use git_gotten_testutil as testutil;
use gitcore::{GitExecutor, Repo};

/// A unique scratch directory path for a linked worktree, outside the fixture
/// (so the fixture's drop never races a live worktree tree). git creates it.
fn wt_dir(label: &str) -> PathBuf {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!("gitgotten-wt-{}-{seq}-{label}", std::process::id()))
}

/// The canonical form the read model reports for a worktree path.
fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// The stage-10 fixture: `main` (one commit) plus a `side` branch at the same tip.
fn worktree_fixture(label: &str) -> (testutil::Fixture, Repo) {
    let fx = testutil::Fixture::new(label);
    fx.commit_file("a.txt", "one\n", "c1");
    fx.git(&["branch", "side"]);
    let repo = Repo::open(&fx.path).unwrap();
    (fx, repo)
}

// ── read model: list ────────────────────────────────────────────────────────

#[test]
fn worktrees_lists_main_and_linked_with_branch_and_head() {
    let (fx, repo) = worktree_fixture("list");
    let wt = wt_dir("list");
    fx.git(&["worktree", "add", wt.to_str().unwrap(), "side"]);

    let worktrees = repo.worktrees().unwrap();
    assert_eq!(worktrees.len(), 2, "main + linked: {worktrees:?}");

    let main = worktrees
        .iter()
        .find(|w| w.is_main)
        .expect("a main worktree");
    assert_eq!(Path::new(&main.path), fx.path.as_path());
    assert_eq!(main.branch.as_deref(), Some("main"));
    assert_eq!(main.head, fx.git(&["rev-parse", "main"]));
    assert!(!main.locked && !main.prunable);

    let linked = worktrees.iter().find(|w| !w.is_main).unwrap();
    assert_eq!(Path::new(&linked.path), canonical(&wt).as_path());
    assert_eq!(linked.branch.as_deref(), Some("side"));
    assert_eq!(linked.head, fx.git(&["rev-parse", "side"]));

    fx.git(&["worktree", "remove", wt.to_str().unwrap()]);
}

#[test]
fn worktrees_reports_a_detached_checkout_without_a_branch() {
    let (fx, repo) = worktree_fixture("detached");
    let wt = wt_dir("detached");
    fx.git(&["worktree", "add", "--detach", wt.to_str().unwrap(), "HEAD"]);

    let linked = repo
        .worktrees()
        .unwrap()
        .into_iter()
        .find(|w| !w.is_main)
        .expect("the detached worktree is listed");
    assert_eq!(linked.branch, None);
    assert_eq!(linked.head, fx.git(&["rev-parse", "HEAD"]));

    fx.git(&["worktree", "remove", wt.to_str().unwrap()]);
}

// ── add ─────────────────────────────────────────────────────────────────────

#[tokio::test]
async fn add_worktree_for_existing_branch_checks_it_out_at_the_path() {
    let (fx, repo) = worktree_fixture("add-existing");
    let wt = wt_dir("add-existing");

    let plan = repo
        .plan(&Op::WorktreeAdd {
            committish: "side".into(),
            path: wt.to_str().unwrap().into(),
            new_branch: None,
        })
        .unwrap();
    assert_eq!(plan.steps[0].argv[0], "git");
    assert_eq!(plan.steps[0].argv[1], "worktree");
    assert_eq!(plan.steps[0].argv[2], "add");
    assert_eq!(plan.steps[0].argv.last().unwrap(), "side");
    assert_eq!(plan.steps[0].risk, Risk::Mutating);

    let result = GitExecutor::new()
        .run_plan(&repo.id, &repo.path, &plan)
        .await;
    assert!(result.ok, "stderr: {}", result.stderr);

    assert!(canonical(&wt).is_dir());
    let branch = testutil::git(&canonical(&wt), &["rev-parse", "--abbrev-ref", "HEAD"]);
    assert_eq!(branch, "side");
    assert!(canonical(&wt).join("a.txt").is_file());

    fx.git(&["worktree", "remove", wt.to_str().unwrap()]);
}

#[tokio::test]
async fn add_worktree_with_new_branch_creates_it_from_the_committish() {
    let (fx, repo) = worktree_fixture("add-new");
    fx.commit_file("b.txt", "two\n", "c2");
    let head = fx.git(&["rev-parse", "HEAD"]);
    let wt = wt_dir("add-new");

    let plan = repo
        .plan(&Op::WorktreeAdd {
            committish: head.clone(),
            path: wt.to_str().unwrap().into(),
            new_branch: Some("feature".into()),
        })
        .unwrap();
    let argv = &plan.steps[0].argv;
    let dash_b = argv
        .iter()
        .position(|a| a == "-b")
        .expect("-b for new branch");
    assert_eq!(argv[dash_b + 1], "feature");

    let result = GitExecutor::new()
        .run_plan(&repo.id, &repo.path, &plan)
        .await;
    assert!(result.ok, "stderr: {}", result.stderr);

    let branch = testutil::git(&canonical(&wt), &["rev-parse", "--abbrev-ref", "HEAD"]);
    assert_eq!(branch, "feature");
    assert_eq!(
        fx.git(&["rev-parse", "feature"]),
        head,
        "the new branch starts at the requested commit"
    );

    fx.git(&["worktree", "remove", wt.to_str().unwrap()]);
}

#[tokio::test]
async fn adding_a_branch_checked_out_in_two_worktrees_is_a_clean_git_error() {
    let (fx, repo) = worktree_fixture("double");
    let first = wt_dir("double-a");
    fx.git(&["worktree", "add", first.to_str().unwrap(), "side"]);
    let second = wt_dir("double-b");

    let op = Op::WorktreeAdd {
        committish: "side".into(),
        path: second.to_str().unwrap().into(),
        new_branch: None,
    };
    let plan = repo
        .plan(&op)
        .expect("the plan compiles; git enforces the rule");
    let result = GitExecutor::new()
        .run_plan(&repo.id, &repo.path, &plan)
        .await;
    assert!(!result.ok, "git must refuse the double checkout");
    assert!(
        result.stderr.contains("already used by worktree"),
        "the refusal must name the checkout conflict: {}",
        result.stderr
    );
    assert!(
        !canonical(&second).is_dir(),
        "no second worktree was created"
    );

    fx.git(&["worktree", "remove", first.to_str().unwrap()]);
}

#[test]
fn add_worktree_validates_target_and_new_branch_name() {
    let (_fx, repo) = worktree_fixture("validate");
    let err = repo
        .plan(&Op::WorktreeAdd {
            committish: "  ".into(),
            path: wt_dir("validate-empty").to_str().unwrap().into(),
            new_branch: None,
        })
        .expect_err("an empty committish is rejected");
    assert!(err.to_string().contains("target is empty"), "{err}");

    let err = repo
        .plan(&Op::WorktreeAdd {
            committish: "main".into(),
            path: "   ".into(),
            new_branch: None,
        })
        .expect_err("an empty path is rejected");
    assert!(err.to_string().contains("target is empty"), "{err}");

    let err = repo
        .plan(&Op::WorktreeAdd {
            committish: "main".into(),
            path: wt_dir("validate-name").to_str().unwrap().into(),
            new_branch: Some("bad name".into()),
        })
        .expect_err("an invalid branch name is rejected");
    assert!(err.to_string().contains("invalid name"), "{err}");
}

// ── remove ──────────────────────────────────────────────────────────────────

#[tokio::test]
async fn remove_clean_worktree_is_mutating_and_removes_it() {
    let (fx, repo) = worktree_fixture("rm-clean");
    let wt = wt_dir("rm-clean");
    fx.git(&["worktree", "add", wt.to_str().unwrap(), "side"]);

    let op = Op::WorktreeRemove {
        path: canonical(&wt).to_string_lossy().into_owned(),
        force: false,
    };
    let plan = repo.plan(&op).unwrap();
    assert_eq!(plan.steps[0].argv[1], "worktree");
    assert_eq!(plan.steps[0].argv[2], "remove");
    assert!(!plan.steps[0].argv.contains(&"--force".to_string()));
    assert_eq!(plan.steps[0].risk, Risk::Mutating);

    let result = GitExecutor::new()
        .run_plan(&repo.id, &repo.path, &plan)
        .await;
    assert!(result.ok, "stderr: {}", result.stderr);
    assert!(!canonical(&wt).exists(), "the worktree directory is gone");
    assert_eq!(repo.worktrees().unwrap().len(), 1);
}

#[test]
fn removing_a_dirty_worktree_without_force_is_rejected_at_plan_time() {
    let (fx, repo) = worktree_fixture("rm-dirty");
    let wt = wt_dir("rm-dirty");
    fx.git(&["worktree", "add", wt.to_str().unwrap(), "side"]);
    std::fs::write(canonical(&wt).join("scratch.txt"), "uncommitted\n").unwrap();

    let err = repo
        .plan(&Op::WorktreeRemove {
            path: canonical(&wt).to_string_lossy().into_owned(),
            force: false,
        })
        .expect_err("a dirty worktree needs force");
    assert!(
        err.to_string().contains("uncommitted"),
        "the error names the uncommitted work: {err}"
    );

    fx.git(&["worktree", "remove", "--force", wt.to_str().unwrap()]);
}

#[tokio::test]
async fn force_removing_a_dirty_worktree_is_dangerous_and_removes_it() {
    let (fx, repo) = worktree_fixture("rm-force");
    let wt = wt_dir("rm-force");
    fx.git(&["worktree", "add", wt.to_str().unwrap(), "side"]);
    std::fs::write(canonical(&wt).join("scratch.txt"), "uncommitted\n").unwrap();

    let plan = repo
        .plan(&Op::WorktreeRemove {
            path: canonical(&wt).to_string_lossy().into_owned(),
            force: true,
        })
        .unwrap();
    assert_eq!(plan.steps[0].risk, Risk::Dangerous);
    assert!(
        plan.preview.as_ref().unwrap().requires_type_to_confirm,
        "force-removing uncommitted work is type-to-confirm"
    );
    assert!(plan.steps[0].argv.contains(&"--force".to_string()));

    let result = GitExecutor::new()
        .run_plan(&repo.id, &repo.path, &plan)
        .await;
    assert!(result.ok, "stderr: {}", result.stderr);
    assert!(!canonical(&wt).exists());
    assert_eq!(repo.worktrees().unwrap().len(), 1);
}

#[test]
fn removing_the_main_worktree_is_a_clean_error() {
    let (_fx, repo) = worktree_fixture("rm-main");
    let err = repo
        .plan(&Op::WorktreeRemove {
            path: repo.path.to_string_lossy().into_owned(),
            force: true,
        })
        .expect_err("the main worktree is never removable here");
    assert!(err.to_string().contains("main worktree"), "{err}");
}

#[test]
fn removing_an_unknown_worktree_is_a_clean_error() {
    let (_fx, repo) = worktree_fixture("rm-unknown");
    let err = repo
        .plan(&Op::WorktreeRemove {
            path: "/definitely/not/a/worktree".into(),
            force: false,
        })
        .expect_err("unknown paths are refused");
    assert!(err.to_string().contains("no such worktree"), "{err}");
}

// ── graph markers ───────────────────────────────────────────────────────────

#[test]
fn graph_pins_each_worktrees_checkout_with_a_worktree_label() {
    let (fx, repo) = worktree_fixture("markers");
    let wt = wt_dir("markers");
    fx.git(&["worktree", "add", wt.to_str().unwrap(), "side"]);
    fx.commit_file("b.txt", "two\n", "c2"); // main moves ahead of side

    let graph = repo.graph().unwrap();
    let marked = |hash: String| {
        graph.nodes.iter().find(|n| n.hash == hash).and_then(|n| {
            n.refs
                .iter()
                .find(|r| r.kind == RefKind::Worktree)
                .map(|r| (n, r))
        })
    };

    let main_head = fx.git(&["rev-parse", "main"]);
    let side_head = fx.git(&["rev-parse", "side"]);

    let (side_node, wt_label) = marked(side_head.clone()).expect("side's checkout is marked");
    assert_eq!(
        wt_label.name, "side",
        "the label names the checked-out branch"
    );
    let line_color = graph
        .lines
        .iter()
        .find(|l| {
            l.segments.iter().any(|s| {
                s.lane == side_node.lane && s.from_row <= side_node.row && side_node.row <= s.to_row
            })
        })
        .expect("the node's line")
        .color;
    assert_eq!(
        wt_label.color, line_color,
        "the marker carries its node's line color"
    );

    assert!(
        marked(main_head).is_some(),
        "the main worktree's checkout is marked too"
    );

    // Single-worktree repositories stay unmarked.
    fx.git(&["worktree", "remove", wt.to_str().unwrap()]);
    let plain = repo.graph().unwrap();
    assert!(
        plain
            .nodes
            .iter()
            .all(|n| n.refs.iter().all(|r| r.kind != RefKind::Worktree)),
        "a repository with only the main worktree adds no markers"
    );
}

// ── retargeting: the linked worktree is a fully openable repository ─────────

#[test]
fn set_current_worktree_retargets_status_and_graph_to_the_new_checkout() {
    let (fx, main_repo) = worktree_fixture("switch");
    fx.commit_file("b.txt", "two\n", "c2"); // main is ahead of side
    let wt = wt_dir("switch");
    fx.git(&["worktree", "add", wt.to_str().unwrap(), "side"]);

    // `set_current_worktree` retargets by opening the worktree root as the
    // active repository (its canonical path is its RepoId, SPEC §10.7).
    let linked = Repo::open(&canonical(&wt)).unwrap();

    let status = linked.status().unwrap();
    assert_eq!(status.branch.as_deref(), Some("side"));

    let main_status = main_repo.status().unwrap();
    assert_eq!(main_status.branch.as_deref(), Some("main"));
    assert_ne!(linked.id, main_repo.id, "each worktree is its own repo id");

    // The graph read from the linked worktree still shows both branches,
    // with `side` emphasized as HEAD there.
    let graph = linked.graph().unwrap();
    let side_tip = graph
        .nodes
        .iter()
        .find(|n| n.refs.iter().any(|r| r.name == "side"))
        .expect("side is in the graph");
    assert!(
        side_tip.is_head,
        "HEAD in the linked worktree is side's tip"
    );

    fx.git(&["worktree", "remove", wt.to_str().unwrap()]);
}
