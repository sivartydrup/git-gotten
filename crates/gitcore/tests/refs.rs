//! Stage 6 acceptance (SPEC §10.3): branch / remote / tag management — the
//! read model, the `CommandPlan` shape of every op (exact argv, risk, and
//! effects), and the post-op repository state, end to end against real git
//! repositories with a **local bare remote** (no network).
//!
//! The full path the app uses is exercised: `Repo::plan(&Op)` →
//! `GitExecutor::run_plan` → the real `git` binary, with the result asserted
//! through the fixture's own git calls. Requires a `git` binary on `PATH`
//! (platform gate of the stage: macOS primary; the Windows gate for
//! fetch/push against a local bare remote is noted where unavailable).

use git_gotten_protocol::{Op, Risk};
use git_gotten_testutil as testutil;
use gitcore::{GitExecutor, Repo};

/// The stage-6 fixture: `main` (ahead 1 of `origin/main`, upstream set) + a
/// `side` branch at the same tip, plus an annotated `v1` and a lightweight
/// `v2` tag. `origin` is a local bare remote (no network).
fn refs_fixture() -> (testutil::Fixture, Repo) {
    let mut fx = testutil::Fixture::new("refs");
    fx.commit_file("a.txt", "one\n", "c1");
    let _remote = fx.add_origin("main"); // pushes main with -u
    fx.commit_file("b.txt", "two\n", "c2"); // main now ahead 1 of origin/main
    fx.git(&["branch", "side"]);
    fx.git(&["tag", "-a", "v1", "-m", "first version", "HEAD"]);
    fx.git(&["tag", "v2", "HEAD"]);
    let repo = Repo::open(&fx.path).unwrap();
    (fx, repo)
}

/// Plan an op, run it through the executor, and expect success.
async fn run(repo: &Repo, fx: &testutil::Fixture, op: Op) -> gitcore::OpResult {
    let plan = repo.plan(&op).expect("plan must compile");
    let result = GitExecutor::new()
        .run_plan(&repo.id, &repo.path, &plan)
        .await;
    assert!(result.ok, "op must succeed, stderr: {}", result.stderr);
    let _ = fx; // fixtures are the assertion vehicle; kept for call-site symmetry
    result
}

/// A unique suffix for per-test scratch directories.
fn unique() -> u64 {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// Make one commit on the *remote side* (via a throwaway clone of the bare
/// remote at `remote_path`), so the local repository is behind.
fn remote_side_commit(fx: &testutil::Fixture, remote_path: &std::path::Path) {
    let clone = std::env::temp_dir().join(format!(
        "gitgotten-refs-clone-{}-{}",
        std::process::id(),
        unique()
    ));
    // Let git create the directory (cloning into `.` in a pre-made dir fails
    // obscurely when a non-empty leftover exists; `git clone <src> <dir>` is loud).
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
    inner.write_file("remote.txt", "from the remote\n");
    inner.git(&["add", "remote.txt"]);
    inner.git(&["commit", "-m", "remote commit"]);
    // The commit must land on the bare remote (a clone's local commit is invisible
    // to it until pushed) — mirroring the testutil `behind_fixture` pattern.
    inner.git(&["push", "origin", "main"]);
    let _ = std::fs::remove_dir_all(&clone);
    let _ = fx;
}

/// Assert the bare remote at `dir` has a ref (`git ls-remote`).
fn assert_remote_has_ref(dir: &str, refname: &str) {
    let out = std::process::Command::new("git")
        .args(["ls-remote", dir, refname])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!stdout.is_empty(), "{dir} must have {refname}: {stdout}");
}

/// Assert the bare remote at `dir` lacks a ref.
fn assert_remote_lacks_ref(dir: &str, refname: &str) {
    let out = std::process::Command::new("git")
        .args(["ls-remote", dir, refname])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.is_empty(), "{dir} must lack {refname}: {stdout}");
}

/// Assert a ref no longer resolves in the fixture. Newer gits exit 128 ("fatal:
/// Needed a single revision") rather than 1, so only non-zero is asserted.
fn assert_ref_gone(fx: &testutil::Fixture, refname: &str) {
    let (code, _, stderr) = fx.git_run(&["rev-parse", "--verify", refname]);
    assert!(code != 0, "{refname} must be gone (exit {code}): {stderr}");
}

/// Assert a remote is gone (any non-zero: `git remote get-url` fatals on a missing remote).
fn assert_remote_gone(fx: &testutil::Fixture, name: &str) {
    let (code, _, stderr) = fx.git_run(&["remote", "get-url", name]);
    assert!(
        code != 0,
        "remote {name} must be gone (exit {code}): {stderr}"
    );
}

// ── read model: branches ────────────────────────────────────────────────────

#[test]
fn branches_lists_local_and_remote_with_tracking_state() {
    let (fx, repo) = refs_fixture();
    let branches = repo.branches().unwrap();

    let main = branches.iter().find(|b| b.name == "main").unwrap();
    assert!(main.is_current, "main is checked out");
    assert!(!main.is_remote);
    assert_eq!(main.upstream.as_deref(), Some("origin/main"));
    assert_eq!((main.ahead, main.behind), (1, 0));
    assert_eq!(main.tip, fx.git(&["rev-parse", "main"]));

    let side = branches.iter().find(|b| b.name == "side").unwrap();
    assert!(!side.is_current);
    assert!(side.upstream.is_none());
    assert_eq!((side.ahead, side.behind), (0, 0));

    let origin_main = branches.iter().find(|b| b.name == "origin/main").unwrap();
    assert!(origin_main.is_remote);
    assert!(!origin_main.is_current);
    assert!(origin_main.upstream.is_none());
    assert_eq!(
        origin_main.tip,
        fx.git(&["rev-parse", "refs/remotes/origin/main"])
    );

    // The origin/HEAD pseudo-ref (a symbolic) must not appear as a branch.
    assert!(
        branches.iter().all(|b| b.name != "origin/HEAD"),
        "symbolic refs must not be listed as branches: {branches:?}"
    );
}

#[test]
fn branches_are_sorted_current_first_then_alphabetical() {
    let (_fx, repo) = refs_fixture();
    let branches = repo.branches().unwrap();
    let names: Vec<&str> = branches.iter().map(|b| b.name.as_str()).collect();
    assert_eq!(names, vec!["main", "origin/main", "side"]);
}

// ── read model: remotes ─────────────────────────────────────────────────────

#[test]
fn remotes_reports_names_and_urls() {
    let (fx, repo) = refs_fixture();
    let remotes = repo.remotes().unwrap();
    assert_eq!(remotes.len(), 1);
    let origin = &remotes[0];
    assert_eq!(origin.name, "origin");
    // The bare remote's url is the path `add_origin` configured.
    assert!(origin.url.contains("gitgotten-remote"), "got {origin:?}");
    assert_eq!(origin.fetch.as_deref(), Some(origin.url.as_str()));
    assert!(origin.push.is_none(), "no pushurl configured: {origin:?}");
    let _ = fx;
}

#[test]
fn remotes_reports_a_distinct_push_url() {
    let mut fx = testutil::Fixture::new("refs-pushurl");
    fx.commit_file("a.txt", "one\n", "c1");
    let remote = fx.add_origin("main");
    fx.git(&["remote", "set-url", "--push", "origin", "push-target"]);
    let repo = Repo::open(&fx.path).unwrap();
    let origin = &repo.remotes().unwrap()[0];
    assert_eq!(origin.name, "origin");
    assert_eq!(origin.fetch.as_deref(), Some(remote.to_str().unwrap()));
    assert_eq!(origin.push.as_deref(), Some("push-target"));
    // The display url falls back to the fetch url.
    assert_eq!(origin.url, remote.to_str().unwrap());
    let _ = remote;
}

// ── read model: tags ────────────────────────────────────────────────────────

#[test]
fn tags_list_annotated_and_lightweight() {
    let (fx, repo) = refs_fixture();
    let tags = repo.tags().unwrap();
    let names: Vec<&str> = tags.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, vec!["v1", "v2"]);

    let v1 = &tags[0];
    assert!(v1.annotated);
    assert_eq!(v1.message.as_deref(), Some("first version"));
    assert_eq!(v1.target, fx.git(&["rev-parse", "HEAD"]));

    let v2 = &tags[1];
    assert!(!v2.annotated);
    assert!(v2.message.is_none());
    assert_eq!(v2.target, fx.git(&["rev-parse", "HEAD"]));
}

// ── branch ops: plan shapes ─────────────────────────────────────────────────

#[test]
fn plan_branch_delete_local() {
    let (fx, repo) = refs_fixture();
    let plan = repo
        .plan(&Op::BranchDelete {
            name: "side".into(),
            remote: None,
            force: false,
        })
        .unwrap();
    assert_eq!(plan.display, "git branch -d side");
    assert_eq!(
        plan.steps[0].argv,
        vec![
            "git".to_string(),
            "branch".to_string(),
            "-d".to_string(),
            "side".to_string()
        ]
    );
    assert_eq!(plan.steps[0].risk, Risk::Mutating);

    let force = repo
        .plan(&Op::BranchDelete {
            name: "side".into(),
            remote: None,
            force: true,
        })
        .unwrap();
    assert_eq!(force.display, "git branch -D side");
    let _ = fx;
}

#[test]
fn plan_branch_delete_of_the_checked_out_branch_shows_the_effect() {
    let (_fx, repo) = refs_fixture();
    let plan = repo
        .plan(&Op::BranchDelete {
            name: "main".into(),
            remote: None,
            force: true,
        })
        .unwrap();
    let effect = plan.steps[0].effect.as_deref().unwrap_or("");
    assert!(
        effect.contains("checked out"),
        "the preview must warn the branch is checked out: {effect:?}"
    );
}

#[test]
fn plan_branch_delete_keeps_the_remote_upstream() {
    let (_fx, repo) = refs_fixture();
    // Deleting `main` (upstream `origin/main`) must say the remote copy is kept.
    let plan = repo
        .plan(&Op::BranchDelete {
            name: "main".into(),
            remote: None,
            force: true,
        })
        .unwrap();
    let effect = plan.steps[0].effect.as_deref().unwrap_or("");
    assert!(
        effect.contains("origin/main"),
        "the preview must name the untouched remote branch: {effect:?}"
    );
}

#[test]
fn plan_branch_delete_remote() {
    let (fx, repo) = refs_fixture();
    let plan = repo
        .plan(&Op::BranchDelete {
            name: "main".into(),
            remote: Some("origin".into()),
            force: false,
        })
        .unwrap();
    assert_eq!(plan.display, "git push origin --delete main");
    assert_eq!(plan.steps[0].risk, Risk::Mutating);
    assert!(plan.steps[0].effect.is_some());
    let _ = fx;
}

#[test]
fn plan_branch_delete_rejects_an_unknown_branch() {
    let (_fx, repo) = refs_fixture();
    let err = repo
        .plan(&Op::BranchDelete {
            name: "nope".into(),
            remote: None,
            force: false,
        })
        .unwrap_err();
    assert!(
        matches!(err, gitcore::GitError::UnknownBranch { .. }),
        "got {err:?}"
    );
}

#[test]
fn plan_branch_rename() {
    let (fx, repo) = refs_fixture();
    let plan = repo
        .plan(&Op::BranchRename {
            name: "side".into(),
            new_name: "side2".into(),
        })
        .unwrap();
    assert_eq!(plan.display, "git branch --move side side2");
    assert_eq!(plan.steps[0].risk, Risk::Mutating);
    let _ = fx;
}

#[test]
fn plan_branch_rename_rejects_invalid_or_taken_names() {
    let (_fx, repo) = refs_fixture();
    // Invalid: a path traversal.
    let err = repo
        .plan(&Op::BranchRename {
            name: "side".into(),
            new_name: "a/../b".into(),
        })
        .unwrap_err();
    assert!(
        matches!(err, gitcore::GitError::BadRefName { .. }),
        "got {err:?}"
    );
    // Taken: `main` already exists.
    let err = repo
        .plan(&Op::BranchRename {
            name: "side".into(),
            new_name: "main".into(),
        })
        .unwrap_err();
    assert!(
        matches!(err, gitcore::GitError::BranchExists { .. }),
        "got {err:?}"
    );
    // Unknown: the old name.
    let err = repo
        .plan(&Op::BranchRename {
            name: "nope".into(),
            new_name: "x".into(),
        })
        .unwrap_err();
    assert!(
        matches!(err, gitcore::GitError::UnknownBranch { .. }),
        "got {err:?}"
    );
}

#[test]
fn plan_branch_set_upstream() {
    let (fx, repo) = refs_fixture();
    let plan = repo
        .plan(&Op::BranchSetUpstream {
            branch: "side".into(),
            remote: Some("origin".into()),
            name: "main".into(),
        })
        .unwrap();
    assert_eq!(
        plan.display,
        "git branch side --set-upstream-to=origin/main"
    );
    assert_eq!(plan.steps[0].risk, Risk::Mutating);
    let _ = fx;
}

#[test]
fn plan_branch_unset_upstream() {
    let (fx, repo) = refs_fixture();
    let plan = repo
        .plan(&Op::BranchSetUpstream {
            branch: "main".into(),
            remote: None,
            name: String::new(),
        })
        .unwrap();
    assert_eq!(plan.display, "git branch main --unset-upstream");
    assert_eq!(plan.steps[0].risk, Risk::Mutating);
    let _ = fx;
}

#[test]
fn plan_push_set_upstream_adds_the_u_flag() {
    let (fx, repo) = refs_fixture();
    let plan = repo
        .plan(&Op::Push {
            remote: "origin".into(),
            branch: None,
            force: false,
            set_upstream: true,
        })
        .unwrap();
    assert_eq!(plan.display, "git push -u origin main");
    let _ = fx;
}

// ── remote ops: plan shapes ─────────────────────────────────────────────────

#[test]
fn plan_remote_add() {
    let (fx, repo) = refs_fixture();
    let plan = repo
        .plan(&Op::RemoteAdd {
            name: "upstream".into(),
            url: "git@host:org/repo.git".into(),
        })
        .unwrap();
    assert_eq!(
        plan.display,
        "git remote add upstream git@host:org/repo.git"
    );
    assert_eq!(plan.steps[0].risk, Risk::Mutating);
    let _ = fx;
}

#[test]
fn plan_remote_add_rejects_duplicates_and_bad_names() {
    let (_fx, repo) = refs_fixture();
    let err = repo
        .plan(&Op::RemoteAdd {
            name: "origin".into(),
            url: "x".into(),
        })
        .unwrap_err();
    assert!(
        matches!(err, gitcore::GitError::RemoteExists { .. }),
        "got {err:?}"
    );
    let err = repo
        .plan(&Op::RemoteAdd {
            name: "".into(),
            url: "x".into(),
        })
        .unwrap_err();
    assert!(
        matches!(err, gitcore::GitError::BadRefName { .. }),
        "got {err:?}"
    );
}

#[test]
fn plan_remote_remove_counts_the_tracking_branches() {
    let (fx, repo) = refs_fixture();
    let plan = repo
        .plan(&Op::RemoteRemove {
            name: "origin".into(),
        })
        .unwrap();
    assert_eq!(plan.display, "git remote remove origin");
    assert_eq!(plan.steps[0].risk, Risk::Mutating);
    let effect = plan.steps[0].effect.as_deref().unwrap_or("");
    assert!(
        effect.contains("1 remote-tracking"),
        "the preview must count the refs that will be deleted: {effect:?}"
    );
    let _ = fx;
}

#[test]
fn plan_remote_remove_rejects_an_unknown_remote() {
    let (_fx, repo) = refs_fixture();
    let err = repo
        .plan(&Op::RemoteRemove {
            name: "nope".into(),
        })
        .unwrap_err();
    assert!(
        matches!(err, gitcore::GitError::UnknownRemote { .. }),
        "got {err:?}"
    );
}

#[test]
fn plan_remote_fetch() {
    let (fx, repo) = refs_fixture();
    let plan = repo
        .plan(&Op::RemoteFetch {
            remote: "origin".into(),
            refspec: None,
        })
        .unwrap();
    assert_eq!(plan.display, "git fetch origin");
    assert_eq!(plan.steps[0].risk, Risk::Mutating);
    let _ = fx;
}

#[test]
fn plan_remote_fetch_with_refspec() {
    let (fx, repo) = refs_fixture();
    let plan = repo
        .plan(&Op::RemoteFetch {
            remote: "origin".into(),
            refspec: Some("+refs/heads/*:refs/remotes/origin/*".into()),
        })
        .unwrap();
    assert_eq!(
        plan.display,
        "git fetch origin +refs/heads/*:refs/remotes/origin/*"
    );
    let _ = fx;
}

// ── tag ops: plan shapes ────────────────────────────────────────────────────

#[test]
fn plan_tag_create_annotated() {
    let (fx, repo) = refs_fixture();
    let head = fx.git(&["rev-parse", "HEAD"]);
    let plan = repo
        .plan(&Op::TagCreate {
            name: "v3".into(),
            target: head.clone(),
            annotated: true,
            message: Some("third version".into()),
        })
        .unwrap();
    assert_eq!(
        plan.steps[0].argv,
        vec![
            "git".to_string(),
            "tag".to_string(),
            "-a".to_string(),
            "v3".to_string(),
            "-m".to_string(),
            "third version".to_string(),
            head
        ]
    );
    assert_eq!(plan.steps[0].risk, Risk::Mutating);
    assert!(!plan.preview.as_ref().unwrap().requires_type_to_confirm);
}

#[test]
fn plan_tag_create_lightweight() {
    let (fx, repo) = refs_fixture();
    let head = fx.git(&["rev-parse", "HEAD"]);
    let plan = repo
        .plan(&Op::TagCreate {
            name: "v3".into(),
            target: head.clone(),
            annotated: false,
            message: None,
        })
        .unwrap();
    assert_eq!(
        plan.steps[0].argv,
        vec!["git".to_string(), "tag".to_string(), "v3".to_string(), head]
    );
    assert_eq!(plan.steps[0].risk, Risk::Mutating);
}

#[test]
fn plan_tag_create_rejects_bad_names_and_unknown_targets() {
    let (_fx, repo) = refs_fixture();
    let head = repo.status().unwrap().head.expect("fixture has commits");
    let err = repo
        .plan(&Op::TagCreate {
            name: "bad tag".into(),
            target: head.clone(),
            annotated: false,
            message: None,
        })
        .unwrap_err();
    assert!(
        matches!(err, gitcore::GitError::BadRefName { .. }),
        "got {err:?}"
    );
    let err = repo
        .plan(&Op::TagCreate {
            name: "v9".into(),
            target: "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz".into(),
            annotated: false,
            message: None,
        })
        .unwrap_err();
    assert!(
        matches!(err, gitcore::GitError::NoCommit { .. }),
        "got {err:?}"
    );
}

#[test]
fn plan_tag_checkout() {
    let (fx, repo) = refs_fixture();
    let plan = repo
        .plan(&Op::TagCheckout {
            name: "v1".into(),
            force: false,
        })
        .unwrap();
    assert_eq!(plan.display, "git switch --detach v1");
    assert_eq!(plan.steps[0].risk, Risk::Mutating);
    let _ = fx;
}

#[test]
fn plan_tag_checkout_rejects_an_unknown_tag() {
    let (_fx, repo) = refs_fixture();
    let err = repo
        .plan(&Op::TagCheckout {
            name: "nope".into(),
            force: false,
        })
        .unwrap_err();
    assert!(
        matches!(err, gitcore::GitError::UnknownTag { .. }),
        "got {err:?}"
    );
}

#[test]
fn plan_tag_delete_local_and_remote() {
    let (fx, repo) = refs_fixture();
    let local = repo
        .plan(&Op::TagDelete {
            name: "v1".into(),
            remote: None,
        })
        .unwrap();
    assert_eq!(local.display, "git tag -d v1");
    assert_eq!(local.steps[0].risk, Risk::Mutating);

    let remote = repo
        .plan(&Op::TagDelete {
            name: "v1".into(),
            remote: Some("origin".into()),
        })
        .unwrap();
    assert_eq!(remote.display, "git push origin --delete v1");
    assert_eq!(remote.steps[0].risk, Risk::Mutating);
    assert!(remote.steps[0].effect.is_some());
    let _ = fx;
}

#[test]
fn plan_tag_push() {
    let (fx, repo) = refs_fixture();
    let plan = repo
        .plan(&Op::TagPush {
            remote: "origin".into(),
            name: "v1".into(),
            force: false,
        })
        .unwrap();
    assert_eq!(plan.display, "git push origin v1");
    assert_eq!(plan.steps[0].risk, Risk::Mutating);
    let _ = fx;
}

#[test]
fn plan_tag_push_force_is_dangerous() {
    let (fx, repo) = refs_fixture();
    let plan = repo
        .plan(&Op::TagPush {
            remote: "origin".into(),
            name: "v1".into(),
            force: true,
        })
        .unwrap();
    assert_eq!(plan.steps[0].risk, Risk::Dangerous);
    assert!(plan.preview.as_ref().unwrap().requires_type_to_confirm);
    let _ = fx;
}

// ── branch ops: execution + post-op state ───────────────────────────────────

#[tokio::test]
async fn branch_delete_removes_a_local_branch() {
    let (fx, repo) = refs_fixture();
    run(
        &repo,
        &fx,
        Op::BranchDelete {
            name: "side".into(),
            remote: None,
            force: false,
        },
    )
    .await;
    assert_ref_gone(&fx, "refs/heads/side");
    // The worktree is untouched.
    assert_eq!(fx.git(&["rev-parse", "--abbrev-ref", "HEAD"]), "main");
}

#[tokio::test]
async fn branch_delete_of_the_checked_out_branch_fails_safely() {
    let (fx, repo) = refs_fixture();
    let plan = repo
        .plan(&Op::BranchDelete {
            name: "main".into(),
            remote: None,
            force: true,
        })
        .unwrap();
    let result = GitExecutor::new()
        .run_plan(&repo.id, &repo.path, &plan)
        .await;
    assert!(!result.ok, "git must refuse to delete a checked-out branch");
    // The branch must still exist.
    assert_eq!(fx.git(&["rev-parse", "--abbrev-ref", "HEAD"]), "main");
}

#[tokio::test]
async fn branch_delete_remote_removes_the_remote_branch() {
    let (fx, repo) = refs_fixture();
    let remote_dir = fx.git(&["remote", "get-url", "origin"]).trim().to_string();
    // Push `side` to the remote (creates the branch + the tracking ref).
    run(
        &repo,
        &fx,
        Op::Push {
            remote: "origin".into(),
            branch: Some("side".into()),
            force: false,
            set_upstream: false,
        },
    )
    .await;
    assert_remote_has_ref(&remote_dir, "refs/heads/side");
    assert!(
        fx.git_run(&["rev-parse", "--verify", "refs/remotes/origin/side"])
            .0
            == 0
    );

    run(
        &repo,
        &fx,
        Op::BranchDelete {
            name: "side".into(),
            remote: Some("origin".into()),
            force: false,
        },
    )
    .await;
    assert_remote_lacks_ref(&remote_dir, "refs/heads/side");
    // The local remote-tracking ref follows the deletion.
    assert_ref_gone(&fx, "refs/remotes/origin/side");
    // The *local* branch survives.
    assert_eq!(fx.git(&["rev-parse", "--abbrev-ref", "HEAD"]), "main");
    let (code, _, _) = fx.git_run(&["rev-parse", "--verify", "refs/heads/side"]);
    assert_eq!(code, 0, "the local branch must survive");
}

#[tokio::test]
async fn branch_rename_moves_the_pointer() {
    let (fx, repo) = refs_fixture();
    run(
        &repo,
        &fx,
        Op::BranchRename {
            name: "side".into(),
            new_name: "side2".into(),
        },
    )
    .await;
    assert_ref_gone(&fx, "refs/heads/side");
    assert_eq!(
        fx.git(&["rev-parse", "refs/heads/side2"]),
        fx.git(&["rev-parse", "main"])
    );
}

#[tokio::test]
async fn branch_rename_of_the_checked_out_branch_keeps_head() {
    let (fx, repo) = refs_fixture();
    run(
        &repo,
        &fx,
        Op::BranchRename {
            name: "main".into(),
            new_name: "primary".into(),
        },
    )
    .await;
    assert_eq!(fx.git(&["rev-parse", "--abbrev-ref", "HEAD"]), "primary");
}

#[tokio::test]
async fn branch_push_creates_the_remote_branch_and_upstream() {
    let (fx, repo) = refs_fixture();
    run(
        &repo,
        &fx,
        Op::Push {
            remote: "origin".into(),
            branch: Some("side".into()),
            force: false,
            set_upstream: true,
        },
    )
    .await;
    // The remote gained the branch.
    let remote_dir = fx.git(&["remote", "get-url", "origin"]).trim().to_string();
    assert_remote_has_ref(&remote_dir, "refs/heads/side");
    // The local branch now tracks it.
    assert_eq!(fx.git(&["config", "branch.side.remote"]), "origin");
    // The remote-tracking ref appeared (a push updates it in-repo).
    assert_eq!(
        fx.git(&["rev-parse", "refs/remotes/origin/side"]),
        fx.git(&["rev-parse", "refs/heads/side"])
    );
}

#[tokio::test]
async fn branch_set_upstream_writes_the_config() {
    let (fx, repo) = refs_fixture();
    run(
        &repo,
        &fx,
        Op::BranchSetUpstream {
            branch: "side".into(),
            remote: Some("origin".into()),
            name: "main".into(),
        },
    )
    .await;
    assert_eq!(fx.git(&["config", "branch.side.remote"]), "origin");
    assert_eq!(fx.git(&["config", "branch.side.merge"]), "refs/heads/main");

    // And unsetting removes it.
    run(
        &repo,
        &fx,
        Op::BranchSetUpstream {
            branch: "side".into(),
            remote: None,
            name: String::new(),
        },
    )
    .await;
    let (code, _, _) = fx.git_run(&["config", "branch.side.remote"]);
    assert_eq!(code, 1, "branch.side.remote must be gone");
}

// ── remote ops: execution + post-op state ───────────────────────────────────

#[tokio::test]
async fn remote_add_then_remove_round_trips() {
    let (fx, repo) = refs_fixture();
    let remote_dir = fx.git(&["remote", "get-url", "origin"]).trim().to_string();
    run(
        &repo,
        &fx,
        Op::RemoteAdd {
            name: "origin2".into(),
            url: remote_dir.clone(),
        },
    )
    .await;
    assert_eq!(fx.git(&["remote", "get-url", "origin2"]), remote_dir);

    // The add rewrote `.git/config`; this test's single `gix` handle caches the
    // config at open time, so re-open for the remove — the app does the same via
    // `RepoRegistry::refresh` after every op.
    let repo = Repo::open(&fx.path).unwrap();
    run(
        &repo,
        &fx,
        Op::RemoteRemove {
            name: "origin2".into(),
        },
    )
    .await;
    assert_remote_gone(&fx, "origin2");
    // `origin` is untouched.
    assert_eq!(fx.git(&["remote", "get-url", "origin"]), remote_dir);
}

#[tokio::test]
async fn remote_remove_deletes_the_tracking_refs() {
    let (fx, repo) = refs_fixture();
    run(
        &repo,
        &fx,
        Op::RemoteRemove {
            name: "origin".into(),
        },
    )
    .await;
    assert_remote_gone(&fx, "origin");
    assert_ref_gone(&fx, "refs/remotes/origin/main");
}

#[tokio::test]
async fn remote_fetch_updates_the_tracking_branch() {
    let (fx, repo) = refs_fixture();
    let remote_dir = fx.git(&["remote", "get-url", "origin"]).trim().to_string();
    // A commit lands on the remote side: local is behind 1.
    remote_side_commit(&fx, std::path::Path::new(&remote_dir));

    run(
        &repo,
        &fx,
        Op::RemoteFetch {
            remote: "origin".into(),
            refspec: None,
        },
    )
    .await;
    // The tracking branch advanced to the remote's actual tip.
    let remote_tip = fx
        .git(&["ls-remote", &remote_dir, "refs/heads/main"])
        .trim()
        .split_whitespace()
        .next()
        .unwrap()
        .to_string();
    assert_eq!(
        fx.git(&["rev-parse", "refs/remotes/origin/main"]),
        remote_tip
    );
    // Status now reports the new behind count.
    assert_eq!(repo.status().unwrap().behind, 1);
}

// ── tag ops: execution + post-op state ──────────────────────────────────────

#[tokio::test]
async fn tag_create_annotated_makes_a_tag_object() {
    let (fx, repo) = refs_fixture();
    let head = fx.git(&["rev-parse", "HEAD"]);
    run(
        &repo,
        &fx,
        Op::TagCreate {
            name: "v3".into(),
            target: head,
            annotated: true,
            message: Some("third".into()),
        },
    )
    .await;
    assert_eq!(fx.git(&["cat-file", "-t", "v3"]), "tag");
    assert_eq!(
        fx.git(&["rev-parse", "v3^{commit}"]),
        fx.git(&["rev-parse", "HEAD"])
    );

    let tags = repo.tags().unwrap();
    assert!(tags.iter().any(|t| t.name == "v3" && t.annotated));
}

#[tokio::test]
async fn tag_create_lightweight_points_at_the_commit() {
    let (fx, repo) = refs_fixture();
    let head = fx.git(&["rev-parse", "HEAD"]);
    run(
        &repo,
        &fx,
        Op::TagCreate {
            name: "v3".into(),
            target: head,
            annotated: false,
            message: None,
        },
    )
    .await;
    assert_eq!(fx.git(&["cat-file", "-t", "v3"]), "commit");
    let tags = repo.tags().unwrap();
    let v3 = tags.into_iter().find(|t| t.name == "v3").unwrap();
    assert!(!v3.annotated);
    assert_eq!(v3.target, fx.git(&["rev-parse", "HEAD"]));
}

#[tokio::test]
async fn tag_checkout_detaches_head() {
    let (fx, repo) = refs_fixture();
    let tip = fx.git(&["rev-parse", "HEAD"]);
    run(
        &repo,
        &fx,
        Op::TagCheckout {
            name: "v1".into(),
            force: false,
        },
    )
    .await;
    // Detached at the tag's commit.
    assert_eq!(fx.git(&["rev-parse", "--abbrev-ref", "HEAD"]), "HEAD");
    assert_eq!(fx.git(&["rev-parse", "HEAD"]), tip);
    let st = repo.status().unwrap();
    assert!(st.branch.is_none());
    assert_eq!(st.detached_head.as_deref(), Some(tip.as_str()));
    assert_eq!(st.head.as_deref(), Some(tip.as_str()));
}

#[tokio::test]
async fn tag_delete_local_and_remote() {
    let (fx, repo) = refs_fixture();
    let remote_dir = fx.git(&["remote", "get-url", "origin"]).trim().to_string();
    // Push v1 to the remote so the remote delete has something to remove.
    run(
        &repo,
        &fx,
        Op::TagPush {
            remote: "origin".into(),
            name: "v1".into(),
            force: false,
        },
    )
    .await;
    assert_remote_has_ref(&remote_dir, "refs/tags/v1");

    run(
        &repo,
        &fx,
        Op::TagDelete {
            name: "v1".into(),
            remote: None,
        },
    )
    .await;
    assert!(fx.git(&["tag", "-l"]).split('\n').all(|l| l.trim() != "v1"));
    // The remote copy survives the local delete.
    assert_remote_has_ref(&remote_dir, "refs/tags/v1");

    run(
        &repo,
        &fx,
        Op::TagDelete {
            name: "v1".into(),
            remote: Some("origin".into()),
        },
    )
    .await;
    assert_remote_lacks_ref(&remote_dir, "refs/tags/v1");
}

#[tokio::test]
async fn tag_push_reaches_the_remote() {
    let (fx, repo) = refs_fixture();
    let remote_dir = fx.git(&["remote", "get-url", "origin"]).trim().to_string();
    run(
        &repo,
        &fx,
        Op::TagPush {
            remote: "origin".into(),
            name: "v2".into(),
            force: false,
        },
    )
    .await;
    assert_remote_has_ref(&remote_dir, "refs/tags/v2");
}

// ── status.head ─────────────────────────────────────────────────────────────

#[test]
fn status_carries_the_head_oid() {
    let (fx, repo) = refs_fixture();
    let st = repo.status().unwrap();
    assert_eq!(
        st.head.as_deref(),
        Some(fx.git(&["rev-parse", "HEAD"]).trim())
    );
    assert!(st.detached_head.is_none(), "a branch is checked out");
}
