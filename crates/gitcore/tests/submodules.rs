//! Stage 11 acceptance (SPEC §10.8): submodules — the `submodules()` read model
//! (path, url, recorded vs checked-out commit, initialized/differs flags), the
//! `CommandPlan` shape of init/update, a drifted submodule showing up in
//! `status()` as a normal (badged) file change, and the post-op repository
//! state, end to end against real git repositories.
//! The full path the app uses is exercised: `Repo::plan(&Op)` →
//! `GitExecutor::run_plan` → the real `git` binary. Requires a `git` binary on
//! `PATH` (macOS primary; the Windows gate for submodule paths is noted where a
//! Windows machine is unavailable).

use git_gotten_protocol::{ChangeKind, Op, Risk};
use git_gotten_testutil as testutil;
use gitcore::{GitExecutor, Repo};

/// A superproject with `extras/dep` registered as a submodule of a second local
/// fixture repository. `dep1`/`dep2` are the submodule's two commits (`dep2` is
/// the gitlink recorded by `git submodule add`).
///
/// `protocol.file.allow` is set repository-locally: modern git blocks the
/// file:// transport in submodule helpers by default, and the fixture clones
/// the submodule from a local path. Real remotes never need this; the app
/// itself runs plain argv.
struct SubFixture {
    parent: testutil::Fixture,
    sub: testutil::Fixture,
    repo: Repo,
    dep1: String,
    dep2: String,
}

fn sub_fixture(label: &str) -> SubFixture {
    let sub = testutil::Fixture::new(&format!("{label}-sub"));
    sub.commit_file("dep.txt", "dep one\n", "dep1");
    let dep1 = testutil::git(&sub.path, &["rev-parse", "HEAD"]);
    sub.commit_file("dep.txt", "dep one\ndep two\n", "dep2");
    let dep2 = testutil::git(&sub.path, &["rev-parse", "HEAD"]);

    let parent = testutil::Fixture::new(label);
    parent.commit_file("README", "main\n", "m1");
    parent.git(&["config", "protocol.file.allow", "always"]);
    parent.git(&[
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "add",
        &sub.path.to_string_lossy(),
        "extras/dep",
    ]);
    parent.git(&["commit", "-m", "add dep"]);

    let repo = Repo::open(&parent.path).unwrap();
    SubFixture {
        parent,
        sub,
        repo,
        dep1,
        dep2,
    }
}

async fn run(sx: &SubFixture, op: Op) -> git_gotten_protocol::OpResult {
    let plan = sx.repo.plan(&op).unwrap();
    let result = GitExecutor::new()
        .run_plan(&sx.repo.id, &sx.parent.path, &plan)
        .await;
    assert!(
        result.ok,
        "op failed: plan `{}`\nstderr: {}",
        plan.display, result.stderr
    );
    result
}

#[tokio::test]
async fn submodules_lists_path_url_recorded_commit_and_state() {
    let sx = sub_fixture("list");
    let subs = sx.repo.submodules().unwrap();

    assert_eq!(subs.len(), 1);
    let s = &subs[0];
    assert_eq!(s.path, "extras/dep");
    assert_eq!(
        s.name, "extras/dep",
        "`git submodule add` defaults the name to the path"
    );
    assert_eq!(s.url, sx.sub.path.to_string_lossy());
    assert_eq!(s.recorded, sx.dep2);
    assert_eq!(s.head.as_deref(), Some(sx.dep2.as_str()));
    assert!(s.initialized);
    assert!(!s.differs);

    // A healthy submodule is not a change.
    let status = sx.repo.status().unwrap();
    assert!(
        !status.changes.iter().any(|c| c.path == "extras/dep"),
        "a synced submodule must not show in Files: {:?}",
        status.changes
    );
}

#[tokio::test]
async fn a_drifted_submodule_pointer_shows_in_files_as_a_badged_change() {
    let sx = sub_fixture("drift");
    // Drift happens *inside the checkout clone* (`extras/dep`), which is a
    // separate clone of the sub fixture — moving the source repo's HEAD would
    // not move the gitlink.
    testutil::git(
        &sx.parent.path,
        &[
            "-C",
            "extras/dep",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "sub extra",
        ],
    );

    let status = sx.repo.status().unwrap();
    let change = status
        .changes
        .iter()
        .find(|c| c.path == "extras/dep")
        .unwrap_or_else(|| {
            panic!(
                "the drifted submodule must show in Files: {:?}",
                status.changes
            )
        });
    assert_eq!(
        change.kind,
        ChangeKind::Modified,
        "a pointer move is a normal change"
    );
    assert!(!change.staged);
    assert!(change.submodule, "the Files panel badges it as a submodule");

    let subs = sx.repo.submodules().unwrap();
    assert!(subs[0].differs, "head != recorded is reported as differs");
}

#[tokio::test]
async fn submodule_update_syncs_the_checkout_back_to_the_recorded_commit() {
    let sx = sub_fixture("update");
    testutil::git(
        &sx.parent.path,
        &[
            "-C",
            "extras/dep",
            "commit",
            "-q",
            "--allow-empty",
            "-m",
            "sub extra",
        ],
    );

    let plan = sx
        .repo
        .plan(&Op::SubmoduleUpdate {
            path: "extras/dep".into(),
            commit: None,
        })
        .unwrap();
    assert_eq!(
        plan.steps[0].argv,
        vec!["git", "submodule", "update", "extras/dep"]
    );
    assert_eq!(plan.steps[0].risk, Risk::Mutating);

    run(
        &sx,
        Op::SubmoduleUpdate {
            path: "extras/dep".into(),
            commit: None,
        },
    )
    .await;

    let subs = sx.repo.submodules().unwrap();
    assert_eq!(subs[0].head.as_deref(), Some(sx.dep2.as_str()));
    assert!(!subs[0].differs);
    assert!(
        sx.repo.status().unwrap().changes.is_empty(),
        "the drift is gone from Files"
    );
}

#[tokio::test]
async fn submodule_update_to_a_specific_commit_checks_it_out_inside_the_submodule() {
    let sx = sub_fixture("update-commit");

    let plan = sx
        .repo
        .plan(&Op::SubmoduleUpdate {
            path: "extras/dep".into(),
            commit: Some(sx.dep1.clone()),
        })
        .unwrap();
    assert_eq!(
        plan.steps.len(),
        2,
        "update, then checkout inside the submodule"
    );
    assert_eq!(
        plan.steps[1].argv,
        vec!["git", "-C", "extras/dep", "checkout", sx.dep1.as_str()]
    );

    run(
        &sx,
        Op::SubmoduleUpdate {
            path: "extras/dep".into(),
            commit: Some(sx.dep1.clone()),
        },
    )
    .await;

    let head = testutil::git(&sx.parent.path, &["-C", "extras/dep", "rev-parse", "HEAD"]);
    assert_eq!(head, sx.dep1);
    let subs = sx.repo.submodules().unwrap();
    assert!(
        subs[0].differs,
        "now the checkout is behind the recorded gitlink"
    );
    let status = sx.repo.status().unwrap();
    assert!(
        status
            .changes
            .iter()
            .any(|c| c.path == "extras/dep" && c.submodule),
        "the pointer move shows in Files: {:?}",
        status.changes
    );
}

#[tokio::test]
async fn submodule_init_creates_the_missing_checkout_from_the_recorded_commit() {
    let sx = sub_fixture("init");
    sx.parent.git(&["submodule", "deinit", "extras/dep"]);

    let subs = sx.repo.submodules().unwrap();
    assert_eq!(subs.len(), 1);
    assert!(!subs[0].initialized, "deinit clears the local registration");
    assert_eq!(subs[0].head, None);
    assert_eq!(subs[0].recorded, sx.dep2, "the gitlink survives deinit");

    let plan = sx
        .repo
        .plan(&Op::SubmoduleInit {
            path: "extras/dep".into(),
        })
        .unwrap();
    assert_eq!(
        plan.steps[0].argv,
        vec!["git", "submodule", "update", "--init", "extras/dep"]
    );

    run(
        &sx,
        Op::SubmoduleInit {
            path: "extras/dep".into(),
        },
    )
    .await;

    let subs = sx.repo.submodules().unwrap();
    assert!(subs[0].initialized);
    assert_eq!(subs[0].head.as_deref(), Some(sx.dep2.as_str()));
    assert!(
        sx.repo
            .status()
            .unwrap()
            .changes
            .iter()
            .all(|c| c.path != "extras/dep")
    );
}

#[test]
fn submodule_ops_on_an_unknown_path_are_clean_errors() {
    let sx = sub_fixture("unknown");
    for op in [
        Op::SubmoduleInit {
            path: "nope".into(),
        },
        Op::SubmoduleUpdate {
            path: "nope".into(),
            commit: None,
        },
    ] {
        let err = sx.repo.plan(&op).expect_err("nope is not a submodule");
        assert!(
            err.to_string().contains("no such submodule"),
            "the error names the missing submodule: {err}"
        );
    }
}
