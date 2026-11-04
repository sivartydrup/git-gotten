//! Stage 13 hardening (SPEC §18): concurrent mutating ops on **one** repository
//! must never collide, and ops on **different** repositories must not serialize
//! against each other.
//!
//! The full app path is exercised: `Repo::plan(&Op)` → `GitExecutor::run_plan` /
//! `GitExecutor::run_plans` → the real `git` binary, with N ops fired from a
//! multi-threaded runtime at the same time. Requires a `git` binary on `PATH`.

use std::collections::HashSet;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use git_gotten_protocol::{Op, OpResult};
use git_gotten_testutil as testutil;
use gitcore::{CommandPlan, GitExecutor, Repo};

/// The collisions git reports when two writes hit one repository at the same
/// time: a stale `.git/index.lock`, or a ref lock it could not take. (`fatal:` is
/// deliberately NOT listed: it is git's generic error prefix — "a branch named
/// 'taken' already exists" is a normal, expected failure of a colliding op, not a
/// race.) A queued executor must never produce either of these.
const LOCK_DIAGNOSTICS: &[&str] = &["index.lock", "cannot lock ref", "could not lock", "unable to create"];

fn assert_no_lock_error(result: &OpResult) {
    for needle in LOCK_DIAGNOSTICS {
        assert!(
            !result.stderr.contains(needle),
            "lock collision ({needle}) in stderr:\n{}",
            result.stderr
        );
    }
}

fn count(fx: &testutil::Fixture, args: &[&str]) -> usize {
    fx.git(args).lines().count()
}

/// The number of commits reachable from `HEAD` (git prints it as one number).
fn commit_count(fx: &testutil::Fixture) -> usize {
    fx.git(&["rev-list", "--count", "HEAD"]).trim().parse().unwrap()
}

/// `git log`/`git show` print the branch decoration on the same line as the
/// first commit, so counting subjects needs the commit count from `rev-list`.
fn subjects(fx: &testutil::Fixture) -> Vec<String> {
    fx.git(&["log", "--all", "--format=%s"]).lines().map(str::to_string).collect()
}

fn head(fx: &testutil::Fixture) -> String {
    fx.git(&["rev-parse", "HEAD"])
}

/// `git status --porcelain` must show nothing but untracked scratch files: no
/// half-staged index, no unmerged entries.
fn assert_consistent_status(fx: &testutil::Fixture) {
    let porcelain = fx.git(&["status", "--porcelain"]);
    assert!(
        porcelain.lines().all(|line| line.is_empty() || line.starts_with("?? ")),
        "no staged or unmerged leftovers expected, porcelain:\n{porcelain}"
    );
}

/// Compile `op` with the repository's own planner (the same call the app makes).
fn plan_of(fx: &testutil::Fixture, op: &Op) -> CommandPlan {
    let repo = Repo::open(&fx.path).unwrap();
    repo.plan(op).expect("op must compile to a plan")
}

/// The queue key of a fixture (the common git dir, see `Repo::queue_key`).
fn key_of(fx: &testutil::Fixture) -> String {
    Repo::open(&fx.path).unwrap().queue_key().to_string()
}

/// One logical op to fire at a repository. `plans()` is the whole op: an optional
/// `git add` for its own file, then the op's plan — all compiled by the
/// repository's own planner, so argv and `display` are exactly what the app runs
/// (transparency contract, SPEC §8). A file-scoped commit also carries
/// `-- <file>`, so a concurrent op's staged file can never be absorbed into it.
struct OpTask {
    op: Op,
    /// The file this op stages and then uses (`None` for ref-only ops).
    file: Option<String>,
    /// Scope the commit to its own file with a pathspec.
    pathspec: bool,
}

impl OpTask {
    fn commit(file: &str, message: &str) -> Self {
        Self {
            op: Op::Commit {
                message: message.to_string(),
                amend: false,
                author: None,
                co_authors: Vec::new(),
            },
            file: Some(file.to_string()),
            pathspec: true,
        }
    }
    fn stash(file: &str) -> Self {
        Self {
            op: Op::StashPush {
                include_untracked: false,
            },
            file: Some(file.to_string()),
            pathspec: false,
        }
    }
    fn branch(name: &str, from: &str) -> Self {
        Self {
            op: Op::BranchCreate {
                name: name.to_string(),
                from: from.to_string(),
                checkout: false,
            },
            file: None,
            pathspec: false,
        }
    }
    fn tag(name: &str, target: &str) -> Self {
        Self {
            op: Op::TagCreate {
                name: name.to_string(),
                target: target.to_string(),
                annotated: false,
                message: None,
            },
            file: None,
            pathspec: false,
        }
    }
    fn plans(&self, path: &std::path::Path) -> Vec<CommandPlan> {
        let repo = Repo::open(path).unwrap();
        let mut plans = Vec::new();
        if let Some(file) = &self.file {
            plans.push(
                repo.plan(&Op::Stage {
                    path: Some(file.clone()),
                })
                .unwrap(),
            );
        }
        let mut plan = repo.plan(&self.op).expect("op must compile to a plan");
        if self.pathspec {
            let file = self.file.clone().unwrap();
            let step = &mut plan.steps[0];
            step.argv.extend(["--".to_string(), file.clone()]);
            step.summary = format!("commit {file}");
            plan.display = gitcore::format_argv(&step.argv);
        }
        plans.push(plan);
        plans
    }
}

/// Fire every task at once through one executor and return each task's final
/// [`OpResult`], in task order. Each task runs its whole op inside ONE queue turn
/// (`run_plans`), so the staging and the op that needs it staged are one critical
/// section — the same shape the app's compound flows must have (SPEC §18).
async fn fire_with(
    executor: &Arc<GitExecutor>,
    key: &str,
    path: &std::path::Path,
    tasks: &[OpTask],
) -> Vec<OpResult> {
    let mut handles = Vec::new();
    for task in tasks {
        let executor = executor.clone();
        let key = key.to_string();
        let path = path.to_path_buf();
        let plans = task.plans(&path);
        handles.push(tokio::spawn(async move {
            let results = executor.run_plans(&key, &path, plans).await;
            results.into_iter().last().unwrap()
        }));
    }
    let mut out = Vec::new();
    for handle in handles {
        out.push(handle.await.expect("task must not panic"));
    }
    out
}

/// Fire every task concurrently and assert every op succeeded without a lock error.
async fn fire(fx: &testutil::Fixture, tasks: &[OpTask]) {
    let executor = Arc::new(GitExecutor::new());
    let results = fire_with(&executor, &key_of(fx), &fx.path, tasks).await;
    for result in &results {
        assert!(
            result.ok,
            "op failed: plan `{}`\nexit {}\nstdout: {}\nstderr: {}",
            result.plan.display,
            result.exit,
            result.stdout,
            result.stderr
        );
        assert_no_lock_error(result);
        assert_eq!(result.exit, 0);
    }
}

/// The fixture the concurrency tests share: one commit on `main` plus a local
/// bare `origin`, so pushes and fetches are real (no network).
fn concurrent_fixture(label: &str) -> testutil::Fixture {
    let mut fx = testutil::Fixture::new(label);
    fx.commit_file("seed.txt", "seed\n", "seed");
    fx.add_origin("main");
    fx
}

/// Fire 24 mutating ops (7 commits, 6 branch creates, 6 tag creates, 5 stash
/// pushes) at one repository at the same time: every op must succeed and the
/// resulting repository must be exactly what the N ops imply.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_mutating_ops_on_one_repo_all_succeed() {
    const COMMITS: usize = 7;
    const BRANCHES: usize = 6;
    const TAGS: usize = 6;
    const STASHES: usize = 5;

    let fx = concurrent_fixture("conc-one");

    // Each op needs its own work in the tree: a file to commit, a file to stash.
    let commit_files: Vec<String> = (0..COMMITS).map(|i| format!("c{i:02}.txt")).collect();
    let stash_files: Vec<String> = (0..STASHES).map(|i| format!("s{i:02}.txt")).collect();
    for file in commit_files.iter().chain(stash_files.iter()) {
        fx.write_file(file, &format!("content {file}\n"));
    }

    // Every op's inputs (`at`, its file) exist before its own turn, so a
    // serialized queue admits all of them: only a race could make one fail.
    let at = head(&fx);
    let mut tasks: Vec<OpTask> = Vec::new();
    for file in &commit_files {
        tasks.push(OpTask::commit(file, &format!("add {file}")));
    }
    for i in 0..BRANCHES {
        tasks.push(OpTask::branch(&format!("b{i}"), &at));
    }
    for i in 0..TAGS {
        tasks.push(OpTask::tag(&format!("t{i}"), &at));
    }
    for file in &stash_files {
        tasks.push(OpTask::stash(file));
    }
    assert_eq!(
        tasks.len(),
        COMMITS + BRANCHES + TAGS + STASHES,
        "the fired op count"
    );
    fire(&fx, &tasks).await;
    println!(
        "DIAG2 count={} head={} branches={:?} tags={:?}",
        fx.git(&["rev-list", "--count", "HEAD"]),
        fx.git(&["rev-parse", "HEAD"]),
        fx.git(&["branch"]),
        fx.git(&["tag", "-l"])
    );

    for file in &commit_files {
        assert_eq!(
            subjects(&fx).iter().filter(|s| *s == &format!("add {file}")).count(),
            1,
            "`add {file}` must exist exactly once"
        );
    }
    // `rev-list HEAD` walks the branch only: stash snapshots are not commits on
    // `main` and must not appear in the count.
    assert_eq!(
        fx.git(&["rev-list", "--count", "HEAD"]),
        (1 + COMMITS).to_string(),
        "seed commit + exactly one commit per commit op"
    );

    // Each branch/tag op created exactly its own ref, at the commit it named, and
    // nothing else changed the ref store.
    let expected: HashSet<String> = ["refs/heads/main".to_string()]
        .into_iter()
        .chain((0..BRANCHES).map(|i| format!("refs/heads/b{i}")))
        .chain((0..TAGS).map(|i| format!("refs/tags/t{i}")))
        .collect();
    let actual: HashSet<String> = fx
        .git(&["for-each-ref", "--format=%(refname)", "refs/heads", "refs/tags"])
        .lines()
        .map(str::to_string)
        .collect();
    assert_eq!(actual, expected, "the ref list is exactly what the ops imply");
    for name in expected.iter().filter(|n| *n != "refs/heads/main") {
        assert_eq!(
            fx.git(&["rev-parse", name]),
            at,
            "{name} must point at the commit the op named"
        );
    }
    // `main` moved once per commit op and no further.
    assert_eq!(
        count(&fx, &["for-each-ref", "refs/heads"]),
        1 + BRANCHES,
        "main + the created branches: {}",
        fx.git(&["branch"])
    );
    assert_eq!(count(&fx, &["for-each-ref", "refs/tags"]), TAGS);

    // Each stash snapshot is in the list; each committed file is tracked as written.
    assert_eq!(count(&fx, &["stash", "list"]), STASHES);
    for file in &commit_files {
        assert_eq!(
            std::fs::read_to_string(fx.path.join(file)).unwrap(),
            format!("content {file}\n"),
            "{file} must be committed with its content"
        );
        assert_eq!(
            fx.git(&["ls-files", file]),
            file.as_str(),
            "{file} must be tracked"
        );
    }
    assert_consistent_status(&fx);
}

/// The same mix, but interleaved with ops that must fail at git (a duplicate
/// branch name, a duplicate tag, a commit with nothing staged): a failing op must
/// not break the successful ones, and none of them may leak a lock error.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_ops_survive_failing_neighbours() {
    const GOOD: usize = 8;
    const COLLISIONS: usize = 4;

    let fx = concurrent_fixture("conc-fail-neighbours");
    let at = head(&fx);
    fx.git(&["branch", "taken"]);
    fx.git(&["tag", "dup"]);
    let good_files: Vec<String> = (0..GOOD).map(|i| format!("g{i}.txt")).collect();
    for file in &good_files {
        fx.write_file(file, &format!("{file}\n"));
    }

    let mut tasks: Vec<OpTask> = good_files
        .iter()
        .map(|file| OpTask::commit(file, &format!("good {file}")))
        .collect();
    for _ in 0..COLLISIONS {
        tasks.push(OpTask::branch("taken", &at));
        tasks.push(OpTask::tag("dup", &at));
    }
    // A commit with nothing staged fails too (exit 1, git's message on stdout).
    tasks.push(OpTask {
        op: Op::Commit {
            message: "nothing staged".to_string(),
            amend: false,
            author: None,
            co_authors: Vec::new(),
        },
        file: None,
        pathspec: false,
    });

    let executor = Arc::new(GitExecutor::new());
    let results = fire_with(&executor, &key_of(&fx), &fx.path, &tasks).await;

    let succeeded = results.iter().filter(|r| r.ok).count();
    let failed = results.iter().filter(|r| !r.ok).count();
    assert_eq!(succeeded, GOOD, "every good op must succeed");
    assert_eq!(failed, tasks.len() - GOOD, "every colliding op must fail");
    for result in &results {
        assert_no_lock_error(result);
        if !result.ok {
            assert_ne!(result.exit, 0, "a failed op must carry a non-zero exit");
        }
    }

    assert_eq!(
        commit_count(&fx),
        1 + GOOD,
        "the failing ops must not have created commits"
    );
    assert_eq!(
        fx.git_run(&["rev-parse", "--verify", "refs/heads/taken"]).0,
        0,
        "the pre-existing branch survives the collisions"
    );
    assert_eq!(
        fx.git(&["for-each-ref", "--format=%(refname)", "refs/tags"]),
        "refs/tags/dup",
        "the collided tag stays single"
    );
    assert_consistent_status(&fx);
}

/// Ops on two different repositories must be **in flight together**: a single
/// global turn (or a key shared by unrelated repos) would serialize them. Tag
/// creates are used so each op writes exactly one ref, independent of any other
/// op's work in the tree.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ops_on_two_repos_are_in_flight_together() {
    const PER_REPO: usize = 6;
    let fx_a = concurrent_fixture("conc-two-a");
    let fx_b = concurrent_fixture("conc-two-b");

    // Ref-only ops: each creates its own ref at the seed commit, so all
    // PER_REPO ops must land in the repository they were fired at, and no op
    // depends on a file another op may have staged.
    let head_a = head(&fx_a);
    let head_b = head(&fx_b);
    let tasks_a: Vec<OpTask> = (0..PER_REPO)
        .map(|i| OpTask::tag(&format!("ta{i}"), &head_a))
        .collect();
    let tasks_b: Vec<OpTask> = (0..PER_REPO)
        .map(|i| OpTask::tag(&format!("tb{i}"), &head_b))
        .collect();

    let executor = Arc::new(GitExecutor::new());
    let keys = [key_of(&fx_a), key_of(&fx_b)];
    assert_ne!(keys[0], keys[1], "two fixtures are two queue keys");

    // `in_flight` counts ops that have begun; `peak` is the largest number that
    // were running at the same instant.
    let in_flight = Arc::new(AtomicU64::new(0));
    let peak = Arc::new(AtomicU64::new(0));
    let mut handles = Vec::new();
    for (index, (fx, tasks)) in [(&fx_a, &tasks_a), (&fx_b, &tasks_b)].iter().enumerate() {
        for task in tasks.iter() {
            let executor = executor.clone();
            let key = keys[index].clone();
            let path = fx.path.clone();
            let plans = task.plans(&fx.path);
            let in_flight = in_flight.clone();
            let peak = peak.clone();
            handles.push(tokio::spawn(async move {
                let now = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                let _ = peak.fetch_max(now, Ordering::SeqCst);
                let results = executor.run_plans(&key, &path, plans).await;
                for result in &results {
                    assert!(
                        result.ok,
                        "op failed: plan `{}`\nstdout: {}\nstderr: {}",
                        result.plan.display, result.stdout, result.stderr
                    );
                    assert_no_lock_error(result);
                    assert_eq!(result.exit, 0);
                }
                // Sleep *inside* the queued turn window measured above: if the two
                // repositories shared one queue, no two ops could be in flight.
                tokio::time::sleep(Duration::from_millis(80)).await;
                in_flight.fetch_sub(1, Ordering::SeqCst);
            }));
        }
    }
    for handle in handles {
        handle.await.expect("task must not panic");
    }
    assert!(
        peak.load(Ordering::SeqCst) >= 2,
        "the two repositories serialized against each other (peak in-flight {})",
        peak.load(Ordering::SeqCst)
    );

    for (index, fx) in [&fx_a, &fx_b].iter().enumerate() {
        let prefix = if index == 0 { "ta" } else { "tb" };
        let expected: Vec<String> = (0..PER_REPO)
            .map(|i| format!("refs/tags/{prefix}{i}"))
            .collect();
        assert_eq!(
            fx.git(&["for-each-ref", "--format=%(refname)", "refs/tags"]).lines().collect::<Vec<_>>(),
            expected,
            "repo {index}: every tag op landed in its own repository"
        );
        assert_eq!(
            commit_count(fx),
            1,
            "repo {index}: tag ops create no commits"
        );
        assert_eq!(fx.git(&["status", "--porcelain"]), "");
    }
    // The other repository gained nothing from these ops.
    assert!(fx_a.git(&["tag", "-l"]).lines().all(|t| t.starts_with('t')));
    assert_eq!(fx_b.git(&["tag", "-l"]).lines().filter(|t| t.starts_with("ta")).count(), 0);
}

/// `run_plans` must produce results identical to `run_plan`: the compound-op seam
/// cannot change the transparency contract (SPEC §18) or a log entry built from
/// an `OpResult`. The control run is the same two plans on a repository whose
/// commits are timestamped identically, so only the parent hash can differ.
#[tokio::test]
async fn batched_run_matches_run_plan_output() {
    let stage_op = Op::Stage {
        path: Some("held.txt".into()),
    };
    let commit_op = Op::Commit {
        message: "held turn".to_string(),
        amend: false,
        author: None,
        co_authors: Vec::new(),
    };

    // Through the batch seam: one queue turn for both plans.
    let fx = concurrent_fixture("conc-batched-output");
    let repo = Repo::open(&fx.path).unwrap();
    fx.write_file("held.txt", "held\n");
    let batched = GitExecutor::new()
        .run_plans(
            repo.queue_key(),
            &repo.path,
            [
                plan_of(&fx, &stage_op),
                plan_of(&fx, &commit_op),
            ],
        )
        .await;
    assert_eq!(batched.len(), 2);
    assert!(batched[0].ok && batched[1].ok, "{batched:#?}");

    // The same two plans, one `run_plan` each, on an identically prepared repo.
    let fx2 = concurrent_fixture("conc-batched-output-control");
    let repo2 = Repo::open(&fx2.path).unwrap();
    fx2.write_file("held.txt", "held\n");
    let executor2 = GitExecutor::new();
    let plain_stage = executor2
        .run_plan(repo2.queue_key(), &repo2.path, &plan_of(&fx2, &stage_op))
        .await;
    let plain_commit = executor2
        .run_plan(repo2.queue_key(), &repo2.path, &plan_of(&fx2, &commit_op))
        .await;
    assert!(plain_stage.ok && plain_commit.ok, "{plain_commit:#?}");

    assert_eq!(batched[0].plan, plain_stage.plan, "identical stage plan");
    assert_eq!(batched[0].ok, plain_stage.ok);
    assert_eq!(batched[0].exit, plain_stage.exit);
    assert_eq!(batched[0].stdout, plain_stage.stdout, "identical stage output");
    // The plans are compiled *before* the staging runs, so both previews describe
    // the pre-commit state; the commands themselves must be identical.
    assert_eq!(
        batched[1].plan.steps, plain_commit.plan.steps,
        "identical commit argv"
    );
    assert_eq!(
        batched[1].plan.display, plain_commit.plan.display,
        "identical commit display"
    );
    assert_eq!(batched[1].ok, plain_commit.ok);
    assert_eq!(batched[1].exit, plain_commit.exit);
    assert_eq!(batched[1].conflicts, plain_commit.conflicts);
    assert!(
        batched[1].stderr.is_empty() && plain_commit.stderr.is_empty(),
        "a successful commit is silent"
    );
    // git's commit summary carries the new commit's own hash and its parent's, so
    // only those may differ; every other byte of the captured output must match.
    let (batch_summary, plain_summary) = (normalized(&batched[1].stdout), normalized(&plain_commit.stdout));
    assert_eq!(batch_summary, plain_summary, "identical captured output");
    assert_eq!(
        batched[1].plan.display,
        "git commit -m \"held turn\"",
        "the UI string is the command that ran"
    );

    for fixture in [&fx, &fx2] {
        assert_eq!(
            fixture.git(&["rev-list", "--count", "HEAD"]),
            "2",
            "both paths committed exactly once"
        );
        assert_eq!(fixture.git(&["log", "-1", "--format=%s"]), "held turn");
        assert_eq!(fixture.git(&["status", "--porcelain"]), "");
    }
}

/// Mask the object ids git prints (the new commit, and the parent it names) so
/// two runs of the same plan on two repositories are comparable.
/// Replace every object id (or bracketed `[main <hash>]` decoration) so two runs
/// of the same plan on two repositories are comparable.
fn normalized(text: &str) -> String {
    let hex = |word: &str| word.len() >= 7 && word.chars().all(|c| c.is_ascii_hexdigit());
    text.split_whitespace()
        .map(|token| {
            let core = token.trim_matches(|c| c == '[' || c == ']' || c == '(');
            if core.chars().all(|c| c.is_ascii_hexdigit()) && core.len() >= 7 {
                token.replace(core, "<hash>")
            } else if hex(token) {
                "<hash>".to_string()
            } else {
                token.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// A compound flow owns the repository's queue turn for the **whole** flow, not
/// per git command — the shape of `git bisect`, where the verdict reads what the
/// test step left behind. `run_plans` is fired together with 12 interlopers, and
/// each interloper is chosen so it can only do damage *inside* the flow:
/// `git reset -q -- staged.txt` before the flow is a no-op, after the flow is a
/// no-op, but a landing between the flow's `git add` and its `git commit` would
/// leave the commit with nothing staged (exit 1). The flow must come back whole.
/// shortcut: catching an unsynchronized queue is probabilistic (12 attempts at a
/// two-command flow); the other tests here pin serialization state-by-state.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn batched_plans_block_other_ops_on_the_same_repo() {
    const INTERLOPERS: usize = 12;
    let fx = concurrent_fixture("conc-critical-section");
    fx.write_file("staged.txt", "flow content\n");

    let repo = Repo::open(&fx.path).unwrap();
    let key = repo.queue_key().to_string();
    let path = repo.path.clone();
    let executor = Arc::new(GitExecutor::new());

    let flow = vec![
        plan_of(
            &fx,
            &Op::Stage {
                path: Some("staged.txt".to_string()),
            },
        ),
        plan_of(
            &fx,
            &Op::Commit {
                message: "flow commit".to_string(),
                amend: false,
                author: None,
                co_authors: Vec::new(),
            },
        ),
    ];
    let interloper_plan = plan_of(
        &fx,
        &Op::Unstage {
            path: Some("staged.txt".to_string()),
        },
    );
    assert_eq!(flow[0].display, "git add -- staged.txt");
    assert_eq!(interloper_plan.display, "git reset -q -- staged.txt");

    let flow_task = {
        let executor = executor.clone();
        let key = key.clone();
        let path = path.clone();
        let flow = flow.clone();
        tokio::spawn(async move { executor.run_plans(&key, &path, flow).await })
    };
    let handles: Vec<_> = (0..INTERLOPERS)
        .map(|_| {
            let executor = executor.clone();
            let key = key.clone();
            let path = path.clone();
            let plan = interloper_plan.clone();
            tokio::spawn(async move { executor.run_plan(&key, &path, &plan).await })
        })
        .collect();

    let flow_results = flow_task.await.expect("the flow task must not panic");
    let mut others: Vec<OpResult> = Vec::with_capacity(INTERLOPERS);
    for handle in handles {
        others.push(handle.await.expect("an interloper task must not panic"));
    }

    for result in flow_results.iter().chain(others.iter()) {
        assert_no_lock_error(result);
        if !result.ok {
            assert_ne!(result.exit, 0, "a failure must arrive as git's exit code, not a crash");
        }
    }
    for result in &flow_results {
        assert!(
            result.ok,
            "an interloper slipped inside the flow's critical section: {}",
            result.stderr
        );
    }

    assert_eq!(commit_count(&fx), 2, "the seed commit plus exactly the flow commit");
    assert_eq!(fx.git(&["log", "-1", "--format=%s"]), "flow commit");
    assert_eq!(
        fx.git(&["show", "HEAD:staged.txt"]),
        "flow content",
        "the content the flow staged is what got committed"
    );
    assert_eq!(fx.git(&["diff", "--cached", "--name-only"]), "", "nothing left half-staged");
    assert_consistent_status(&fx);
}

/// Two linked worktrees of one object database must share one queue turn: keyed
/// by the worktree path they would race on the shared refs (SPEC §18).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn linked_worktrees_of_one_repo_share_the_queue_turn() {
    const OPS: usize = 12;
    let fx = concurrent_fixture("conc-worktrees");
    let main = Repo::open(&fx.path).unwrap();
    // Outside the repository: a worktree directory inside the worktree would
    // itself show up as an untracked entry.
    let worktree = std::env::temp_dir().join(format!("gitgotten-conc-wt-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&worktree);
    fx.git(&["worktree", "add", worktree.to_str().unwrap(), "-b", "second"]);
    let linked = Repo::open(&worktree).unwrap();

    assert_eq!(
        main.queue_key(),
        linked.queue_key(),
        "both worktrees of one object database serialize on one key"
    );
    assert_ne!(
        main.queue_key(),
        &main.id,
        "the queue key is the common git dir, not the worktree path"
    );

    // Lightweight tag creates from HEAD: shared-ref writes fired at both
    // worktrees at the same time.
    let executor = Arc::new(GitExecutor::new());
    let mut handles = Vec::new();
    for i in 0..OPS {
        let executor = executor.clone();
        let key = linked.queue_key().to_string();
        let path = if i % 2 == 0 {
            main.path.clone()
        } else {
            linked.path.clone()
        };
        let plan = plan_of(
            &fx,
            &Op::TagCreate {
                name: format!("w{i}"),
                target: head(&fx),
                annotated: false,
                message: None,
            },
        );
        handles.push(tokio::spawn(async move {
            let result = executor.run_plan(&key, &path, &plan).await;
            assert!(
                result.ok,
                "op failed: plan `{}`\nexit {}\nstderr: {}",
                plan.display, result.exit, result.stderr
            );
            assert_no_lock_error(&result);
        }));
    }
    for handle in handles {
        handle.await.expect("task must not panic");
    }

    assert_eq!(
        count(&fx, &["for-each-ref", "refs/tags"]),
        OPS,
        "all {OPS} tags exist: {}",
        fx.git(&["tag", "-l"])
    );
    assert_eq!(
        commit_count(&fx),
        1,
        "tag ops must not create commits"
    );
    assert_eq!(
        fx.git(&["for-each-ref", "--format=%(refname)", "refs/heads"]),
        testutil::git(&linked.path, &["for-each-ref", "--format=%(refname)", "refs/heads"]),
        "both worktrees see the same branch list"
    );
    assert_eq!(
        count(&fx, &["for-each-ref", "refs/heads"]),
        2,
        "main + second"
    );
    assert_eq!(fx.git(&["status", "--porcelain"]), "");
    assert_eq!(testutil::git(&linked.path, &["status", "--porcelain"]), "");
    fx.git(&["worktree", "remove", worktree.to_str().unwrap()]);
}

/// A `git add` fired through the executor is a real queued op (regression guard
/// for the queue key and the planner's stage display).
#[tokio::test]
async fn staged_op_runs_through_the_queue_key() {
    let fx = concurrent_fixture("conc-stage-op");
    fx.write_file("plain.txt", "plain\n");
    let plan = plan_of(&fx, &Op::Stage { path: Some("plain.txt".into()) });
    assert_eq!(plan.display, "git add -- plain.txt");
    let repo = Repo::open(&fx.path).unwrap();
    let result = GitExecutor::new()
        .run_plan(repo.queue_key(), &repo.path, &plan)
        .await;
    assert!(result.ok, "stderr: {}", result.stderr);
    assert_eq!(fx.git(&["diff", "--cached", "--name-only"]), "plain.txt");
}
