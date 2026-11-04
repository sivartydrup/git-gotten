//! Stage 13 hardening (SPEC §8/§18 — "safe failure"): a step that fails must stop
//! the remaining steps, surface git's own output verbatim, leave the repository in a
//! recoverable state, and never hand an argument to a shell.
//!
//! Same path the app takes: `Repo::plan(&Op)` → `GitExecutor::run_plan` (or the
//! hand-built plan, for the case no single `Op` produces) → the real `git` binary.

use git_gotten_protocol::{CommandPlan, GitStep, Op, Risk};
use git_gotten_testutil as testutil;
use gitcore::{GitExecutor, Repo};

fn step(argv: &[&str], risk: Risk) -> GitStep {
    GitStep {
        argv: argv.iter().map(|s| s.to_string()).collect(),
        summary: argv.join(" "),
        risk,
        effect: None,
        input: None,
        program: None,
    }
}

/// A failing step ends the plan. The steps before it ran, the steps after it never
/// do, and the failure is git's own diagnosis — a value the UI can show, not a Rust
/// error that would end the operation with nothing recorded (SPEC §18).
#[tokio::test]
async fn a_failing_step_stops_the_plan_and_keeps_gits_words() {
    let fx = testutil::Fixture::new("failure-steps");
    fx.commit_file("a.txt", "one\n", "c1");
    let repo = Repo::open(&fx.path).unwrap();

    let plan = CommandPlan {
        steps: vec![
            step(&["git", "rev-parse", "--short", "HEAD"], Risk::ReadOnly),
            step(&["git", "switch", "no-such-branch"], Risk::Mutating),
            step(&["git", "tag", "MUST_NOT_EXIST"], Risk::Mutating),
        ],
        display: "git rev-parse --short HEAD; git switch no-such-branch; git tag MUST_NOT_EXIST".into(),
        preview: None,
    };

    let result = GitExecutor::new()
        .run_plan(repo.queue_key(), &repo.path, &plan)
        .await;

    assert!(!result.ok, "a non-zero exit is data, not an error");
    assert_ne!(result.exit, 0);
    assert!(!result.stdout.trim().is_empty(), "step 1 ran: {result:?}");
    assert!(
        !result.stderr.trim().is_empty() && result.stderr.contains("no-such-branch"),
        "git's own diagnosis must survive verbatim, got stderr {:?}",
        result.stderr
    );
    assert!(
        !fx.git(&["tag", "--list"]).contains("MUST_NOT_EXIST"),
        "the step after the failure must never run"
    );
    // The transparency contract: the result carries the plan that was run.
    assert_eq!(result.plan.display, plan.display);
    assert_eq!(result.plan.steps.len(), 3);
}

/// A real multi-step op that fails halfway: the first git command is done, the
/// second refuses, and the repository is left in a state the UI can recover from
/// (the merge is still in progress, so continue/abort remain available — SPEC §18).
#[tokio::test]
async fn a_mid_plan_failure_leaves_a_recoverable_state() {
    let fx = testutil::merge_conflict_fixture(); // merge stopped on `f.txt`
    let mut repo = Repo::open(&fx.path).unwrap();
    assert!(repo.status().unwrap().in_merge, "fixture must be mid-merge");

    let plan = repo
        .plan(&Op::BranchCreate {
            name: "half-done".into(),
            from: fx.git(&["rev-parse", "HEAD"]),
            checkout: true,
        })
        .unwrap();
    assert_eq!(plan.steps.len(), 2, "create the branch, then switch to it");

    let result = GitExecutor::new()
        .run_plan(repo.queue_key(), &repo.path, &plan)
        .await;
    repo.refresh().unwrap();

    assert!(
        !result.ok,
        "git must refuse to switch across unmerged files: {:?}",
        result
    );
    assert_ne!(result.exit, 0);
    assert!(
        fx.git(&["branch", "--list", "half-done"]).contains("half-done"),
        "the completed first step must still be there"
    );
    assert!(
        repo.status().unwrap().in_merge,
        "the interrupted merge must stay recoverable (continue / abort)"
    );
    assert_eq!(
        fx.git(&["rev-parse", "--abbrev-ref", "HEAD"]),
        "main",
        "the failed second step must not have moved HEAD"
    );
    let output = format!("{}{}", result.stdout, result.stderr);
    assert!(
        output.contains("merging") || output.contains("unmerged") || output.contains("local changes"),
        "git must say why the second step stopped, got {output:?}"
    );
    assert_eq!(result.plan.display, plan.display, "the log names the whole op");
}

/// Arguments are spawned directly (SPEC §16): shell grammar inside an argument is
/// data for git, never a command. A hostile commit message must land verbatim and
/// must not create or delete anything.
#[tokio::test]
async fn plan_arguments_never_reach_a_shell() {
    let fx = testutil::Fixture::new("failure-shell");
    fx.commit_file("a.txt", "one\n", "c1");
    fx.write_file("b.txt", "two\n");
    fx.git(&["add", "-A"]);
    let message = "fix: $(touch pwned) `touch pwned2` ; rm -rf .";

    let repo = Repo::open(&fx.path).unwrap();
    let plan = repo
        .plan(&Op::Commit {
            message: message.into(),
            amend: false,
            author: None,
            co_authors: Vec::new(),
        })
        .unwrap();
    assert!(
        plan.steps.iter().any(|s| s.argv.iter().any(|arg| arg == message)),
        "the message must stay one argv element: {:?}",
        plan.steps
    );

    let result = GitExecutor::new()
        .run_plan(repo.queue_key(), &repo.path, &plan)
        .await;
    assert!(result.ok, "the commit must simply succeed: {}", result.stderr);
    assert_eq!(
        fx.git(&["log", "-1", "--format=%s"]),
        message,
        "the message lands verbatim"
    );
    assert!(
        !fx.path.join("pwned").exists() && !fx.path.join("pwned2").exists(),
        "nothing was ever interpreted by a shell"
    );
    assert_eq!(fx.git(&["status", "--porcelain"]), "", "and the tree is still clean");
}

/// SPEC §18's named example: a push to a dead remote. The op runs, git's own
/// diagnosis is returned as data, and the repository is left exactly as it was —
/// no half-pushed state, no orphan ref, nothing for the UI to roll back (SPEC §18).
#[tokio::test]
async fn a_push_to_a_dead_remote_fails_cleanly() {
    let fx = testutil::Fixture::new("failure-push");
    fx.commit_file("a.txt", "one\n", "c1");
    let dead = fx.path.join("nowhere.git");
    fx.git(&["remote", "add", "origin", dead.to_str().unwrap()]);

    let repo = Repo::open(&fx.path).unwrap();
    let plan = repo
        .plan(&Op::Push {
            remote: "origin".into(),
            branch: Some("main".into()),
            force: false,
            set_upstream: false,
        })
        .expect("a configured remote must plan");
    let before = fx.git(&["rev-parse", "HEAD"]);

    let result = GitExecutor::new()
        .run_plan(repo.queue_key(), &repo.path, &plan)
        .await;

    assert!(!result.ok, "the push must fail as a value, not a crash");
    assert_ne!(result.exit, 0);
    assert!(
        !result.stderr.trim().is_empty(),
        "git's reason must reach the log verbatim: {result:?}"
    );
    assert_eq!(fx.git(&["rev-parse", "HEAD"]), before, "nothing local moved");
    assert_eq!(
        fx.git(&["for-each-ref", "refs/remotes"]),
        "",
        "no half-written remote ref is left behind"
    );
    assert_eq!(fx.git(&["status", "--porcelain"]), "", "no orphan state in the tree");
    assert_eq!(result.plan.display, plan.display);
}
