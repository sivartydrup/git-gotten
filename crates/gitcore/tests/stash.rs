//! Stage 11 acceptance (SPEC §10.6): stashes — the `stashes()` read model
//! (message, branch, date, files), the `CommandPlan` shape of push/apply/pop/drop
//! (exact argv + risk tier), and the post-op working tree / stash list, end to
//! end against real git repositories.
//! The full path the app uses is exercised: `Repo::plan(&Op)` →
//! `GitExecutor::run_plan` → the real `git` binary. Requires a `git` binary on
//! `PATH` (macOS primary; the Windows gate for stash paths is noted where a
//! Windows machine is unavailable).

use git_gotten_protocol::{ChangeKind, Op, Risk};
use git_gotten_testutil as testutil;
use gitcore::{GitExecutor, Repo};

fn stash_fixture(label: &str) -> (testutil::Fixture, Repo) {
    let fx = testutil::Fixture::new(label);
    fx.commit_file("a.txt", "one\n", "c1");
    fx.commit_file("b.txt", "two\n", "c2");
    let repo = Repo::open(&fx.path).unwrap();
    (fx, repo)
}

async fn run(fx: &testutil::Fixture, repo: &Repo, op: Op) -> git_gotten_protocol::OpResult {
    let plan = repo.plan(&op).unwrap();
    let result = GitExecutor::new().run_plan(&repo.id, &fx.path, &plan).await;
    assert!(
        result.ok,
        "op failed: plan `{}`\nstderr: {}",
        plan.display, result.stderr
    );
    result
}

#[tokio::test]
async fn stash_push_cleans_the_worktree_and_lists_the_saved_files() {
    let (fx, repo) = stash_fixture("push");
    fx.write_file("a.txt", "one\nedited\n");

    run(
        &fx,
        &repo,
        Op::StashPush {
            include_untracked: false,
        },
    )
    .await;

    let status = repo.status().unwrap();
    assert!(
        status.changes.is_empty(),
        "stash push must clean the worktree: {:?}",
        status.changes
    );
    assert_eq!(
        std::fs::read_to_string(fx.path.join("a.txt")).unwrap(),
        "one\n"
    );

    let stashes = repo.stashes().unwrap();
    assert_eq!(stashes.len(), 1);
    let s = &stashes[0];
    assert_eq!(s.id, 0);
    assert_eq!(s.name, "stash@{0}");
    assert_eq!(s.files, vec!["a.txt".to_string()]);
    assert_eq!(
        s.branch.as_deref(),
        Some("main"),
        "the stash names its branch: {}",
        s.message
    );
    assert!(
        s.message.contains("WIP on main"),
        "git's default subject: {}",
        s.message
    );
    assert!(!s.date.is_empty(), "stashes carry a creation date");
}

#[tokio::test]
async fn stash_push_without_untracked_leaves_new_files_on_disk() {
    let (fx, repo) = stash_fixture("keep-untracked");
    fx.write_file("a.txt", "one\nedited\n");
    fx.write_file("brand-new.txt", "new\n");

    run(
        &fx,
        &repo,
        Op::StashPush {
            include_untracked: false,
        },
    )
    .await;

    assert!(
        fx.path.join("brand-new.txt").is_file(),
        "untracked files are not stashed by default"
    );
    let stashes = repo.stashes().unwrap();
    assert_eq!(stashes[0].files, vec!["a.txt".to_string()]);

    run(&fx, &repo, Op::StashPop { id: 0 }).await;
    assert_eq!(
        std::fs::read_to_string(fx.path.join("a.txt")).unwrap(),
        "one\nedited\n"
    );
}

#[tokio::test]
async fn stash_push_with_untracked_saves_new_files_too() {
    let (fx, repo) = stash_fixture("untracked");
    fx.write_file("brand-new.txt", "new\n");

    run(
        &fx,
        &repo,
        Op::StashPush {
            include_untracked: true,
        },
    )
    .await;

    assert!(
        !fx.path.join("brand-new.txt").exists(),
        "-u must remove the untracked file"
    );
    let stashes = repo.stashes().unwrap();
    assert_eq!(stashes[0].files, vec!["brand-new.txt".to_string()]);

    run(&fx, &repo, Op::StashPop { id: 0 }).await;
    assert_eq!(
        std::fs::read_to_string(fx.path.join("brand-new.txt")).unwrap(),
        "new\n"
    );
    assert!(repo.stashes().unwrap().is_empty());
}

#[tokio::test]
async fn apply_keeps_the_stash_pop_removes_it_and_drop_discards_it() {
    let (fx, repo) = stash_fixture("apply-pop-drop");
    // Two independent stashes (different files) so no op ever conflicts.
    fx.write_file("a.txt", "one\nedited-a\n");
    run(
        &fx,
        &repo,
        Op::StashPush {
            include_untracked: false,
        },
    )
    .await; // stash@{0} = a
    fx.write_file("b.txt", "two\nedited-b\n");
    run(
        &fx,
        &repo,
        Op::StashPush {
            include_untracked: false,
        },
    )
    .await; // stash@{0} = b, {1} = a

    // apply: the work comes back, the stash list keeps both entries.
    run(&fx, &repo, Op::StashApply { id: 1 }).await;
    assert_eq!(
        std::fs::read_to_string(fx.path.join("a.txt")).unwrap(),
        "one\nedited-a\n"
    );
    assert_eq!(repo.stashes().unwrap().len(), 2, "apply keeps the stash");

    // pop: applied and dropped, the other entry renumbers down.
    run(&fx, &repo, Op::StashPop { id: 0 }).await;
    assert_eq!(
        std::fs::read_to_string(fx.path.join("b.txt")).unwrap(),
        "two\nedited-b\n"
    );
    let stashes = repo.stashes().unwrap();
    assert_eq!(stashes.len(), 1);
    assert_eq!(
        stashes[0].files,
        vec!["a.txt".to_string()],
        "the remaining stash is the a.txt one"
    );

    // drop: gone without touching the worktree.
    run(&fx, &repo, Op::StashDrop { id: 0 }).await;
    assert!(repo.stashes().unwrap().is_empty());
    assert_eq!(
        std::fs::read_to_string(fx.path.join("a.txt")).unwrap(),
        "one\nedited-a\n"
    );
    let status = repo.status().unwrap();
    assert!(
        status
            .changes
            .iter()
            .any(|c| c.path == "a.txt" && c.kind == ChangeKind::Modified),
        "the applied work is still in the worktree: {:?}",
        status.changes
    );
}

#[test]
fn stash_ops_on_an_unknown_stash_id_are_clean_errors() {
    let (fx, repo) = stash_fixture("unknown-id");
    for op in [
        Op::StashApply { id: 1 },
        Op::StashPop { id: 1 },
        Op::StashDrop { id: 1 },
    ] {
        let err = repo.plan(&op).expect_err("stash@{1} does not exist");
        assert!(
            err.to_string().contains("no such stash"),
            "the error names the missing stash: {err}"
        );
    }
    drop(fx);
}

#[test]
fn stash_argv_is_literal_and_drop_is_the_dangerous_tier() {
    let (fx, repo) = stash_fixture("argv");
    fx.write_file("a.txt", "one\nedited\n");

    // The apply/pop/drop plans validate against the real stash list, so one
    // must exist before they can be compiled.
    fx.git(&["stash", "push", "-q", "-u"]);

    let push = repo
        .plan(&Op::StashPush {
            include_untracked: true,
        })
        .unwrap();
    assert_eq!(push.steps[0].argv, vec!["git", "stash", "push", "-u"]);
    assert_eq!(push.steps[0].risk, Risk::Mutating);

    let apply = repo.plan(&Op::StashApply { id: 0 }).unwrap();
    assert_eq!(
        apply.steps[0].argv,
        vec!["git", "stash", "apply", "stash@{0}"]
    );

    let pop = repo.plan(&Op::StashPop { id: 0 }).unwrap();
    assert_eq!(pop.steps[0].argv, vec!["git", "stash", "pop", "stash@{0}"]);

    let drop_plan = repo.plan(&Op::StashDrop { id: 0 }).unwrap();
    assert_eq!(
        drop_plan.steps[0].argv,
        vec!["git", "stash", "drop", "stash@{0}"]
    );
    assert_eq!(
        drop_plan.steps[0].risk,
        Risk::Dangerous,
        "dropping discards the snapshot"
    );
    assert!(drop_plan.preview.unwrap().requires_type_to_confirm);
}

#[tokio::test]
async fn stash_drop_all_clears_every_snapshot_and_refuses_an_empty_list() {
    let (fx, repo) = stash_fixture("drop-all");
    fx.write_file("a.txt", "one\nedited\n");
    fx.git(&["stash", "push", "-q", "-u"]);
    fx.write_file("b.txt", "two\nedited\n");
    fx.git(&["stash", "push", "-q", "-u"]);

    let plan = repo.plan(&Op::StashDropAll).unwrap();
    assert_eq!(plan.steps[0].argv, vec!["git", "stash", "clear"]);
    assert_eq!(plan.steps[0].risk, Risk::Dangerous, "all snapshots vanish");
    let preview = plan.preview.unwrap();
    assert!(preview.requires_type_to_confirm);
    assert_eq!(
        preview.lines.len(),
        2,
        "the preview names every stash to be deleted: {:?}",
        preview.lines
    );

    run(&fx, &repo, Op::StashDropAll).await;
    assert!(repo.stashes().unwrap().is_empty(), "the stash list is empty");
    assert!(
        repo.status().unwrap().changes.is_empty(),
        "clearing touches no working tree"
    );

    let err = repo.plan(&Op::StashDropAll).expect_err("nothing left to clear");
    assert!(err.to_string().contains("no stashes"), "clean error: {err}");
}
