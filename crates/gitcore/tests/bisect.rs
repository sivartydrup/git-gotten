//! Stage 11 acceptance (SPEC §10.9): bisect — `bisect_start` / `bisect_verdict`
//! / `bisect_test` / `bisect_reset` plans (exact argv), the live `bisect_state()`
//! read (candidate, good/bad, remaining, found, log), and auto-convergence to a
//! known culprit commit via a test command's exit code, end to end against real
//! git repositories.
//! The full path the app uses is exercised: `Repo::plan(&Op)` →
//! `GitExecutor::run_plan` → the real `git` binary. Requires a `git` binary on
//! `PATH` (macOS primary; the Windows gate for the test-command process is
//! noted where a Windows machine is unavailable).

use git_gotten_protocol::{BisectVerdict, Op, Risk};
use git_gotten_testutil as testutil;
use gitcore::{GitExecutor, Repo};

/// A four-commit history where commit **c3** introduces `bug.txt`; the bug
/// persists through c4. `culprit` is c3's hash — what a correct bisect must
/// converge to. `check.sh` (present since c1) exits 0 while the bug is absent
/// and non-zero once it is present: git's `bisect run` convention (0 = good).
struct BugFixture {
    fx: testutil::Fixture,
    repo: Repo,
    good: String,
    culprit: String,
    bad: String,
}

fn bug_fixture(label: &str) -> BugFixture {
    let fx = testutil::Fixture::new(label);
    fx.write_file("check.sh", "#!/bin/sh\ntest ! -f bug.txt\n");
    fx.git(&["add", "-A"]);
    fx.git(&["commit", "-qm", "c1"]);
    let good = testutil::git(&fx.path, &["rev-parse", "HEAD"]);
    fx.commit_file("a.txt", "one\n", "c2");
    fx.commit_file("bug.txt", "bug\n", "c3 introduce bug");
    let culprit = testutil::git(&fx.path, &["rev-parse", "HEAD"]);
    fx.commit_file("a.txt", "one\ntwo\n", "c4");
    let bad = testutil::git(&fx.path, &["rev-parse", "HEAD"]);
    let repo = Repo::open(&fx.path).unwrap();
    BugFixture {
        fx,
        repo,
        good,
        culprit,
        bad,
    }
}

async fn run(bx: &BugFixture, op: Op) -> git_gotten_protocol::OpResult {
    let plan = bx.repo.plan(&op).unwrap();
    let result = GitExecutor::new()
        .run_plan(&bx.repo.id, &bx.fx.path, &plan)
        .await;
    assert!(
        result.ok,
        "op failed: plan `{}`\nstderr: {}",
        plan.display, result.stderr
    );
    result
}

async fn start(bx: &BugFixture) {
    run(
        bx,
        Op::BisectStart {
            good: bx.good.clone(),
            bad: bx.bad.clone(),
            extra: Vec::new(),
        },
    )
    .await;
}

#[tokio::test]
async fn bisect_state_tracks_the_session_candidate_and_range() {
    let bx = bug_fixture("state");
    assert!(
        !bx.repo.bisect_state().unwrap().active,
        "no session before `bisect start`"
    );

    start(&bx).await;
    let state = bx.repo.bisect_state().unwrap();
    assert!(state.active);
    let candidate = state
        .current
        .clone()
        .expect("a candidate is checked out to test");
    assert_ne!(candidate, bx.good);
    assert_ne!(candidate, bx.bad);
    assert!(
        state.good.contains(&bx.good),
        "the start good ref is in the log: {:?}",
        state.good
    );
    assert!(
        state.bad.contains(&bx.bad),
        "the start bad ref is in the log: {:?}",
        state.bad
    );
    assert_eq!(state.remaining, 2, "c2 and c3 are the untested candidates");
    assert_eq!(state.found, None);
    assert!(
        state.log.iter().any(|l| l.starts_with("git bisect start")),
        "replayable log: {:#?}",
        state.log
    );
}

#[tokio::test]
async fn bisect_test_command_verdicts_on_exit_code_and_converges_on_the_culprit() {
    let bx = bug_fixture("test-cmd");
    start(&bx).await;

    let mut steps = 0;
    loop {
        steps += 1;
        assert!(
            steps <= 6,
            "bisect must converge in log2(N) steps, got {steps}"
        );
        let state = bx.repo.bisect_state().unwrap();
        if let Some(found) = state.found {
            assert_eq!(found, bx.culprit, "git's first-bad-commit is the culprit");
            break;
        }
        // The app's `bisect_test` flow: run the test plan, verdict on exit
        // code — a failing test is a *result*, not an op failure.
        let test_plan = bx
            .repo
            .plan(&Op::BisectTest {
                command: "sh check.sh".into(),
            })
            .unwrap();
        let test = GitExecutor::new()
            .run_plan(&bx.repo.id, &bx.fx.path, &test_plan)
            .await;
        assert_ne!(
            test.exit, -1,
            "the test command must spawn: {}",
            test_plan.display
        );
        let candidate = bx
            .repo
            .bisect_state()
            .unwrap()
            .current
            .expect("candidate under test");
        let is_culprit_or_later = candidate == bx.culprit || candidate == bx.bad;
        assert_eq!(
            test.ok, !is_culprit_or_later,
            "exit code must reflect the bug in {candidate}"
        );
        run(
            &bx,
            Op::BisectVerdict {
                verdict: if test.ok {
                    BisectVerdict::Good
                } else {
                    BisectVerdict::Bad
                },
            },
        )
        .await;
    }

    let state = bx.repo.bisect_state().unwrap();
    assert_eq!(
        state.remaining, 0,
        "converged sessions have no candidates left"
    );
    assert!(state.log.iter().any(|l| l.contains("first bad commit")));
}

#[tokio::test]
async fn manual_good_bad_verdicts_converge_on_the_culprit() {
    let bx = bug_fixture("manual");
    start(&bx).await;

    for _ in 0..6 {
        let state = bx.repo.bisect_state().unwrap();
        if state.found.is_some() {
            break;
        }
        // The graph-button path: the user (here: the marker file) decides.
        let has_bug = bx.fx.path.join("bug.txt").is_file();
        run(
            &bx,
            Op::BisectVerdict {
                verdict: if has_bug {
                    BisectVerdict::Bad
                } else {
                    BisectVerdict::Good
                },
            },
        )
        .await;
    }

    let state = bx.repo.bisect_state().unwrap();
    assert_eq!(state.found.as_deref(), Some(bx.culprit.as_str()));
    assert!(
        state.active,
        "the session stays open until reset so the culprit can be inspected"
    );
}

#[tokio::test]
async fn bisect_reset_restores_the_original_checkout() {
    let bx = bug_fixture("reset");
    start(&bx).await;
    run(
        &bx,
        Op::BisectVerdict {
            verdict: BisectVerdict::Good,
        },
    )
    .await;

    run(&bx, Op::BisectReset).await;

    let state = bx.repo.bisect_state().unwrap();
    assert!(!state.active);
    assert_eq!(state.current, None);
    assert!(state.log.is_empty());
    let status = bx.repo.status().unwrap();
    assert_eq!(
        status.branch.as_deref(),
        Some("main"),
        "back on the original branch"
    );
    assert_eq!(
        status.head.as_deref(),
        Some(bx.bad.as_str()),
        "and at the original commit"
    );
}

#[test]
fn bisect_argv_and_preview_name_the_range() {
    let bx = bug_fixture("argv");

    let start = bx
        .repo
        .plan(&Op::BisectStart {
            good: bx.good.clone(),
            bad: bx.bad.clone(),
            extra: Vec::new(),
        })
        .unwrap();
    assert_eq!(
        start.steps[0].argv,
        vec!["git", "bisect", "start", bx.bad.as_str(), bx.good.as_str()]
    );
    assert_eq!(start.steps[0].risk, Risk::Mutating);
    let preview = start.preview.unwrap();
    assert!(
        preview.lines.iter().any(|l| l.contains(&bx.good)),
        "preview names the good end: {:?}",
        preview.lines
    );
    assert!(
        preview.lines.iter().any(|l| l.contains(&bx.bad)),
        "preview names the bad end: {:?}",
        preview.lines
    );

    let good_line = bx
        .repo
        .plan(&Op::BisectVerdict {
            verdict: BisectVerdict::Good,
        })
        .is_err();
    assert!(
        good_line,
        "a verdict outside a bisect session cannot be planned"
    );
}

#[test]
fn extra_range_refs_shrink_the_starting_candidate_count() {
    let bx = bug_fixture("extra");
    let c2 = testutil::git(&bx.fx.path, &["rev-parse", "HEAD~2"]);
    let plan = bx
        .repo
        .plan(&Op::BisectStart {
            good: bx.good.clone(),
            bad: bx.bad.clone(),
            extra: vec![c2.clone()],
        })
        .unwrap();
    assert_eq!(
        plan.steps[0].argv,
        vec![
            "git",
            "bisect",
            "start",
            bx.bad.as_str(),
            bx.good.as_str(),
            c2.as_str()
        ]
    );
    let preview = plan.preview.unwrap();
    assert!(
        preview.lines.iter().any(|l| l.contains("1 commit")),
        "with c2 known good only c3 remains under test: {:?}",
        preview.lines
    );
}

#[test]
fn verdict_and_reset_without_an_active_bisect_are_clean_errors() {
    let bx = bug_fixture("inactive");
    for op in [
        Op::BisectVerdict {
            verdict: BisectVerdict::Bad,
        },
        Op::BisectTest {
            command: "sh check.sh".into(),
        },
        Op::BisectReset,
    ] {
        let err = bx.repo.plan(&op).expect_err("no bisect is running");
        assert!(
            err.to_string().contains("no bisect"),
            "the error names the missing session: {err}"
        );
    }
}

#[test]
fn bisect_test_argv_is_the_literal_split_of_the_command() {
    let bx = bug_fixture("test-argv");
    testutil::git(&bx.fx.path, &["bisect", "start", "HEAD", &bx.good]);
    let plan = bx
        .repo
        .plan(&Op::BisectTest {
            command: "sh check.sh --flag".into(),
        })
        .unwrap();
    let step = &plan.steps[0];
    assert_eq!(
        step.argv,
        vec!["sh", "check.sh", "--flag"],
        "argv vector, no shell"
    );
    assert_eq!(
        step.program.as_deref(),
        Some("sh"),
        "argv[0] names the spawned program"
    );
    assert_eq!(step.risk, Risk::Mutating);
    testutil::git(&bx.fx.path, &["bisect", "reset"]);
}
