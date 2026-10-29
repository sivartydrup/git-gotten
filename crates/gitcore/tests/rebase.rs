//! Stage 9 acceptance (SPEC §10.4): interactive rebase (reorder / squash /
//! fixup / reword / edit / drop), rebase-onto, and targeted cherry-pick —
//! conflict-aware throughout, with published-history rebases classified
//! `Dangerous`. End to end against real git repositories through the full app
//! path (`Repo::plan` → `GitExecutor::run_plan`), plus a mid-rebase stop that
//! resolves through the Stage 8 flow and continues to completion.
//! Requires a `git` binary on `PATH` (the stage's platform gate).

use git_gotten_protocol::{ConflictResolution, HunkChoice, Op, RebaseAction, RebaseEdit, Risk};
use git_gotten_testutil as testutil;
use gitcore::{GitError, GitExecutor, Repo};

/// `main` (base only) plus a `feature` branch three commits ahead:
/// `one` (a.txt), `two` (b.txt), `three` (c.txt), checked out on `feature`.
/// Returns the repo and the three full hashes oldest-first.
fn feature_fixture() -> (testutil::Fixture, Repo, [String; 3]) {
    let fx = testutil::Fixture::new("rebase-feature");
    fx.commit_file("base.txt", "base\n", "base");
    fx.git(&["checkout", "-b", "feature"]);
    fx.commit_file("a.txt", "one\n", "one");
    let h1 = fx.git(&["rev-parse", "HEAD"]);
    fx.commit_file("b.txt", "two\n", "two");
    let h2 = fx.git(&["rev-parse", "HEAD"]);
    fx.commit_file("c.txt", "three\n", "three");
    let h3 = fx.git(&["rev-parse", "HEAD"]);
    let repo = Repo::open(&fx.path).unwrap();
    let hashes = [h1, h2, h3].map(|h| h.trim().to_string());
    (fx, repo, hashes)
}

fn edit(hash: &str, action: RebaseAction, message: Option<&str>) -> RebaseEdit {
    RebaseEdit {
        hash: hash.to_string(),
        action,
        message: message.map(str::to_string),
    }
}

/// Commit subjects (newest-first) over a rev range.
fn subjects(fx: &testutil::Fixture, range: &str) -> Vec<String> {
    fx.git(&["log", "--format=%s", range])
        .lines()
        .map(str::to_string)
        .collect()
}

/// Plan an op, run it through the executor, and return the result (no success
/// assertion: the stopping tests expect `ok: false`).
async fn run(repo: &Repo, op: &Op) -> gitcore::OpResult {
    let plan = repo.plan(op).expect("plan must compile");
    GitExecutor::new()
        .run_plan(&repo.id, &repo.path, &plan)
        .await
}

/// The prepared todo script from the plan's `sequence.editor` config arg.
fn injected_todo(plan: &gitcore::CommandPlan) -> String {
    let config = &plan.steps[0].argv[2];
    let path = config
        .trim_start_matches("sequence.editor=sh '")
        .trim_end_matches('\'')
        .to_string();
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("todo script at {path}: {e}"))
}

// ── plan shape ───────────────────────────────────────────────────────────────

#[test]
fn plan_rebase_injects_the_edited_todo_as_the_sequence_editor() {
    let (fx, repo, [h1, h2, h3]) = feature_fixture();
    // UI order (newest-first): pick `one`, squash `three` into `two` with a
    // new message, pick `two` — the core must emit git's todo oldest-first.
    let op = Op::Rebase {
        target: "main".to_string(),
        edits: vec![
            edit(&h1, RebaseAction::Pick, None),
            edit(&h3, RebaseAction::Squash, Some("B+C")),
            edit(&h2, RebaseAction::Pick, None),
        ],
    };
    let plan = repo.plan(&op).unwrap();

    assert_eq!(plan.steps.len(), 1);
    assert_eq!(plan.steps[0].risk, Risk::Mutating);
    let argv = &plan.steps[0].argv;
    assert_eq!(argv[0], "git");
    assert_eq!(argv[1], "-c");
    assert!(argv[2].starts_with("sequence.editor=sh '"));
    assert!(argv[2].ends_with("gitgotten-todo.sh'"));
    assert_eq!(&argv[3..], ["rebase", "-i", "main"]);
    assert!(plan.display.starts_with("git -c \"sequence.editor=sh '"));
    assert!(
        plan.display
            .ends_with("gitgotten-todo.sh'\" rebase -i main")
    );

    let todo = injected_todo(&plan);
    let lines: Vec<&str> = todo.lines().skip(2).collect(); // shebang + heredoc opener
    assert_eq!(
        lines,
        [
            format!("pick {h2}").as_str(),
            format!("squash {h3}").as_str(),
            "exec git commit --amend --allow-empty -m 'B+C'",
            format!("pick {h1}").as_str(),
            "GITGOTTEN_TODO",
        ]
    );

    let preview = plan.preview.unwrap();
    assert_eq!(preview.headline, "Rebase feature onto main");
    assert!(!preview.requires_type_to_confirm);
    let _ = fx;
}

#[test]
fn plan_rebase_escapes_quotes_in_reword_messages() {
    let (fx, repo, [h1, h2, h3]) = feature_fixture();
    let op = Op::Rebase {
        target: "main".to_string(),
        edits: vec![
            edit(&h3, RebaseAction::Pick, None),
            edit(&h2, RebaseAction::Pick, None),
            edit(&h1, RebaseAction::Reword, Some("it's done")),
        ],
    };
    let plan = repo.plan(&op).unwrap();
    let todo = injected_todo(&plan);
    assert!(
        todo.contains("exec git commit --amend --allow-empty -m 'it'\\''s done'"),
        "todo: {todo}"
    );
    let _ = fx;
}

#[test]
fn plan_rebase_rejects_a_todo_that_does_not_match_the_range() {
    let (fx, repo, [h1, h2, _h3]) = feature_fixture();
    // Drops one commit from the todo and reuses another hash twice.
    let op = Op::Rebase {
        target: "main".to_string(),
        edits: vec![
            edit(&h1, RebaseAction::Pick, None),
            edit(&h1, RebaseAction::Pick, None),
            edit(&h2, RebaseAction::Pick, None),
        ],
    };
    let err = repo.plan(&op).unwrap_err();
    assert!(matches!(err, GitError::RebaseTodoMismatch { .. }), "{err}");
    let _ = fx;
}

#[test]
fn plan_rebase_rejects_squashing_the_oldest_rebased_commit() {
    let (fx, repo, [h1, h2, h3]) = feature_fixture();
    // UI order newest-first puts the Squash on `one` — the oldest commit,
    // which has no earlier commit to fold into.
    let op = Op::Rebase {
        target: "main".to_string(),
        edits: vec![
            edit(&h3, RebaseAction::Pick, None),
            edit(&h2, RebaseAction::Pick, None),
            edit(&h1, RebaseAction::Squash, Some("nowhere to fold")),
        ],
    };
    let err = repo.plan(&op).unwrap_err();
    assert!(matches!(err, GitError::RebaseTodoOrder), "{err}");
    let _ = fx;
}

#[test]
fn plan_rebase_rejects_a_reword_without_a_message() {
    let (fx, repo, [h1, h2, h3]) = feature_fixture();
    let op = Op::Rebase {
        target: "main".to_string(),
        edits: vec![
            edit(&h3, RebaseAction::Reword, None),
            edit(&h2, RebaseAction::Pick, None),
            edit(&h1, RebaseAction::Pick, None),
        ],
    };
    let err = repo.plan(&op).unwrap_err();
    assert!(
        matches!(err, GitError::RebaseEditNeedsMessage { .. }),
        "{err}"
    );
    let _ = fx;
}

#[test]
fn plan_rebase_onto_uses_plain_rebase() {
    let (fx, repo, _) = feature_fixture();
    let plan = repo
        .plan(&Op::RebaseOnto {
            target: "main".to_string(),
        })
        .unwrap();
    assert_eq!(plan.display, "git rebase main");
    assert_eq!(plan.steps[0].risk, Risk::Mutating);
    assert!(!plan.preview.unwrap().requires_type_to_confirm);
    let _ = fx;
}
#[test]
fn plan_cherry_pick_with_a_target_switches_first() {
    let fx = testutil::Fixture::new("plan-cp-target");
    fx.commit_file("a.txt", "one\n", "one");
    fx.git(&["branch", "side"]);
    let pick = fx.git(&["rev-parse", "HEAD"]).trim().to_string();
    let repo = Repo::open(&fx.path).unwrap();
    let plan = repo
        .plan(&Op::CherryPick {
            hashes: vec![pick.clone()],
            target: Some("side".to_string()),
        })
        .unwrap();
    assert_eq!(plan.steps.len(), 2);
    assert_eq!(
        plan.steps[0].argv,
        vec!["git".to_string(), "switch".to_string(), "side".to_string()]
    );
    assert_eq!(
        plan.steps[1].argv,
        vec!["git".to_string(), "cherry-pick".to_string(), pick.clone()]
    );
    assert_eq!(
        plan.display,
        format!("git switch side && git cherry-pick {pick}")
    );
    assert!(
        plan.preview
            .unwrap()
            .lines
            .iter()
            .any(|l| l.contains("side"))
    );
    let _ = fx;
}

// ── end to end: history rewritten exactly as configured ──────────────────────

#[tokio::test]
async fn interactive_rebase_reorders_and_squashes() {
    let (fx, repo, [h1, h2, h3]) = feature_fixture();
    // Reorder `one` to the top and fold `three` into `two` with a new message
    // (UI order, newest-first).
    let op = Op::Rebase {
        target: "main".to_string(),
        edits: vec![
            edit(&h1, RebaseAction::Pick, None),
            edit(&h3, RebaseAction::Squash, Some("B+C")),
            edit(&h2, RebaseAction::Pick, None),
        ],
    };
    let result = run(&repo, &op).await;
    assert!(result.ok, "stderr: {}", result.stderr);

    assert_eq!(subjects(&fx, "main..feature"), ["one", "B+C"]);
    assert_eq!(subjects(&fx, "feature").len(), 3); // base, B+C, one
    // All three file changes survive the rewrite, in the new order.
    assert_eq!(fx.git(&["show", "HEAD:a.txt"]).trim(), "one");
    assert_eq!(fx.git(&["show", "HEAD:b.txt"]).trim(), "two");
    assert_eq!(fx.git(&["show", "HEAD:c.txt"]).trim(), "three");
    let _ = (h1, h2, h3, fx);
}

#[tokio::test]
async fn interactive_rebase_rewords_fixups_and_drops() {
    let (fx, repo, [h1, h2, h3]) = feature_fixture();
    // Drop `three`, fixup `two` into `one`, and reword `one` (UI order).
    let op = Op::Rebase {
        target: "main".to_string(),
        edits: vec![
            edit(&h3, RebaseAction::Drop, None),
            edit(&h2, RebaseAction::Fixup, None),
            edit(&h1, RebaseAction::Reword, Some("first")),
        ],
    };
    let result = run(&repo, &op).await;
    assert!(result.ok, "stderr: {}", result.stderr);

    assert_eq!(subjects(&fx, "main..feature"), ["first"]);
    assert_eq!(fx.git(&["show", "HEAD:a.txt"]).trim(), "one");
    assert_eq!(fx.git(&["show", "HEAD:b.txt"]).trim(), "two");
    assert_ne!(fx.git_run(&["show", "HEAD:c.txt"]).0, 0);
    let _ = (h1, h2, h3, fx);
}

#[tokio::test]
async fn interactive_rebase_edit_stop_resumes_with_continue() {
    let (fx, repo, [h1, h2, h3]) = feature_fixture();
    // `edit` on the newest commit: git must stop mid-rebase, and the existing
    // Continue op must resume the todo to completion.
    let op = Op::Rebase {
        target: "main".to_string(),
        edits: vec![
            edit(&h3, RebaseAction::Edit, None),
            edit(&h2, RebaseAction::Pick, None),
            edit(&h1, RebaseAction::Pick, None),
        ],
    };
    let result = run(&repo, &op).await;
    assert!(
        !result.ok || repo.status().unwrap().in_rebase,
        "the edit stop must leave the rebase in progress (stderr: {})",
        result.stderr
    );
    assert!(repo.status().unwrap().in_rebase);

    let continued = run(&repo, &Op::ContinueOp).await;
    assert!(continued.ok, "stderr: {}", continued.stderr);
    assert!(!repo.status().unwrap().in_rebase);
    assert_eq!(subjects(&fx, "main..feature"), ["three", "two", "one"]);
    let _ = (h1, h2, h3, fx);
}

// ── conflict-aware rebase (Stage 8 flow reused, SPEC §10.5) ──────────────────

#[tokio::test]
async fn rebase_stop_resolves_and_continues_to_completion() {
    let fx = testutil::rebase_conflict_fixture();
    let repo = Repo::open(&fx.path).unwrap();
    assert!(repo.status().unwrap().in_rebase);
    assert_eq!(repo.unmerged_files().unwrap(), ["f.txt"]);

    // Resolve the region with custom text, then conclude: the rebase must run
    // its remaining todo to completion.
    let resolved = run(
        &repo,
        &Op::ResolveConflict {
            path: "f.txt".to_string(),
            resolution: ConflictResolution::Hunks(vec![HunkChoice::Custom("chosen\n".to_string())]),
        },
    )
    .await;
    assert!(resolved.ok, "stderr: {}", resolved.stderr);
    assert!(
        repo.status().unwrap().in_rebase,
        "the todo has a stop to conclude"
    );

    let continued = run(&repo, &Op::ContinueOp).await;
    assert!(continued.ok, "stderr: {}", continued.stderr);
    let status = repo.status().unwrap();
    assert!(!status.in_rebase);
    assert!(!status.in_merge);
    assert_eq!(
        std::fs::read_to_string(fx.path.join("f.txt")).unwrap(),
        "chosen\n"
    );
    assert_eq!(subjects(&fx, "main"), ["c-main", "c-side", "c1"]);
    let _ = fx;
}

// ── published-history protection (SPEC §8) ──────────────────────────────────

#[tokio::test]
async fn rebasing_published_commits_is_dangerous_and_runs_after() {
    let mut fx = testutil::Fixture::new("rebase-published");
    fx.commit_file("a.txt", "one\n", "one");
    let h1 = fx.git(&["rev-parse", "HEAD"]).trim().to_string();
    let remote = fx.add_origin("main"); // `one` is on the remote
    fx.commit_file("b.txt", "two\n", "two");
    fx.git(&["push", "-q", "origin", "main"]); // `two` is published too
    fx.commit_file("c.txt", "three\n", "three"); // local-only
    let h2 = fx.git(&["rev-parse", "HEAD~1"]).trim().to_string();
    let h3 = fx.git(&["rev-parse", "HEAD"]).trim().to_string();
    let repo = Repo::open(&fx.path).unwrap();

    // Rebase `two` and `three` onto `one`: `two` is already on the remote.
    let op = Op::Rebase {
        target: h1,
        edits: vec![
            edit(&h3, RebaseAction::Pick, None),
            edit(&h2, RebaseAction::Pick, None),
        ],
    };
    let plan = repo.plan(&op).unwrap();
    assert_eq!(plan.steps[0].risk, Risk::Dangerous);
    let preview = plan.preview.unwrap();
    assert!(preview.requires_type_to_confirm);
    assert_eq!(
        plan.steps[0].effect.as_deref(),
        Some("rewrites 1 commit already on a remote")
    );
    assert!(
        preview
            .lines
            .iter()
            .any(|l| l.contains("1 commit on a remote"))
    );

    let result = run(&repo, &op).await;
    assert!(result.ok, "stderr: {}", result.stderr);
    assert_eq!(subjects(&fx, "main").len(), 3);
    let _ = (remote, fx);
}

#[test]
fn rebasing_unpublished_commits_stays_mutating() {
    let (fx, repo, [h1, h2, h3]) = feature_fixture();
    let plan = repo
        .plan(&Op::Rebase {
            target: "main".to_string(),
            edits: vec![
                edit(&h3, RebaseAction::Pick, None),
                edit(&h2, RebaseAction::Pick, None),
                edit(&h1, RebaseAction::Pick, None),
            ],
        })
        .unwrap();
    assert_eq!(plan.steps[0].risk, Risk::Mutating);
    assert!(!plan.preview.unwrap().requires_type_to_confirm);
    let _ = fx;
}

// ── cherry-pick onto a target branch ────────────────────────────────────────

#[tokio::test]
async fn cherry_pick_onto_another_branch_switches_and_picks() {
    let fx = testutil::Fixture::new("cp-target");
    fx.commit_file("b.txt", "base\n", "base");
    fx.git(&["branch", "side"]); // side forks at the base commit
    fx.commit_file("p.txt", "pick me\n", "pickup");
    let pick = fx.git(&["rev-parse", "HEAD"]).trim().to_string();
    let repo = Repo::open(&fx.path).unwrap();

    let result = run(
        &repo,
        &Op::CherryPick {
            hashes: vec![pick],
            target: Some("side".to_string()),
        },
    )
    .await;
    assert!(result.ok, "stderr: {}", result.stderr);

    let status = repo.status().unwrap();
    assert_eq!(status.branch.as_deref(), Some("side"));
    assert_eq!(subjects(&fx, "side"), ["pickup", "base"]);
    assert_eq!(
        std::fs::read_to_string(fx.path.join("p.txt")).unwrap(),
        "pick me\n"
    );
    let _ = fx;
}

#[tokio::test]
async fn cherry_pick_conflict_on_the_target_resolves_and_continues() {
    let fx = testutil::Fixture::new("cp-target-conflict");
    fx.commit_file("f.txt", "base\n", "c1");
    fx.git(&["checkout", "-b", "side"]);
    fx.commit_file("f.txt", "side\n", "c-side");
    fx.git(&["checkout", "main"]);
    fx.commit_file("f.txt", "main\n", "c-main");
    let pick = fx.git(&["rev-parse", "HEAD"]).trim().to_string();
    let repo = Repo::open(&fx.path).unwrap();

    // Pick main's c-main onto side: divergent `f.txt` content conflicts.
    let picked = run(
        &repo,
        &Op::CherryPick {
            hashes: vec![pick],
            target: Some("side".to_string()),
        },
    )
    .await;
    assert!(!picked.ok, "the pick was expected to conflict");
    assert_eq!(
        picked.conflicts,
        Vec::<String>::new(),
        "the core does not fill conflicts"
    );
    let status = repo.status().unwrap();
    assert_eq!(status.branch.as_deref(), Some("side"));
    assert!(status.in_cherry_pick);

    let resolved = run(
        &repo,
        &Op::ResolveConflict {
            path: "f.txt".to_string(),
            resolution: ConflictResolution::Hunks(vec![HunkChoice::Custom("chosen\n".to_string())]),
        },
    )
    .await;
    assert!(resolved.ok, "stderr: {}", resolved.stderr);
    let continued = run(&repo, &Op::ContinueOp).await;
    assert!(continued.ok, "stderr: {}", continued.stderr);

    let status = repo.status().unwrap();
    assert!(!status.in_cherry_pick);
    assert_eq!(status.branch.as_deref(), Some("side"));
    assert_eq!(
        std::fs::read_to_string(fx.path.join("f.txt")).unwrap(),
        "chosen\n"
    );
    assert_eq!(subjects(&fx, "side"), ["c-main", "c-side", "c1"]);
    let _ = fx;
}

// ── the range read behind the RebaseDialog ──────────────────────────────────

#[test]
fn rebase_commits_lists_the_range_newest_first() {
    let (fx, repo, [h1, h2, h3]) = feature_fixture();
    let commits = repo.rebase_commits("main").unwrap();
    let listed: Vec<(String, String)> = commits.into_iter().map(|c| (c.hash, c.subject)).collect();
    assert_eq!(
        listed,
        [
            (h3, "three".to_string()),
            (h2, "two".to_string()),
            (h1, "one".to_string()),
        ]
    );
    let _ = fx;
}

#[test]
fn rebase_commits_rejects_an_unknown_target() {
    let (fx, repo, _) = feature_fixture();
    let err = repo.rebase_commits("no-such-ref").unwrap_err();
    assert!(matches!(err, GitError::NoCommit { .. }), "{err}");
    let _ = fx;
}
