//! The git CLI executor (SPEC §8): runs [`CommandPlan`]s through the real `git`
//! binary, serialized per repository through an [`OpQueue`].
//!
//! - **No shell**: every step is spawned from its argv vector directly.
//! - **Controlled environment**: `GIT_DIR`-family variables are removed (the cwd is
//!   the worktree, and git finds its own `.git`), and credentials/pager prompts
//!   are disabled so an op fails fast with a readable error instead of hanging.
//! - **Safe failure**: a non-zero exit is *data* — it lands in the [`OpResult`]
//!   (`ok: false` + captured stderr), never a Rust error, so the UI can always
//!   show the exact command and output (SPEC §18).

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, LazyLock};
use std::time::Instant;

use tokio::sync::Mutex as AsyncMutex;

use git_gotten_protocol::{CommandPlan, OpResult, RepoId};

use crate::GitError;

/// Environment variables that must never leak into a git child process.
///
/// A user's shell (or a parent `git` invocation) may set these; if they did, git
/// would operate on the *wrong* repository. The cwd is always the worktree root,
/// which is what we want git to target.
pub(crate) const ENV_UNSET: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_GIT_DIR",
];

/// Environment set explicitly on every git child process: fail fast on
/// credential prompts, never invoke a pager (output is piped either way),
/// and never launch an *interactive* editor. The app has no terminal, so a
/// `git merge` (which opens the editor for the merge message) must keep the
/// default message and commit: `true` exits 0 without touching the message
/// file. `GIT_EDITOR` outranks both `VISUAL`/`EDITOR` and `core.editor`, so
/// one variable pins the behavior.
pub(crate) const ENV_SET: &[(&str, &str)] = &[
    ("GIT_TERMINAL_PROMPT", "0"),
    ("GIT_PAGER", "cat"),
    ("GIT_EDITOR", "true"),
    ("GIT_SEQUENCE_EDITOR", "true"),
];

/// Locate and verify a git binary: search `PATH` for the platform executable,
/// then check that it answers `git --version`. A missing or broken git is `Err(())`.
fn find_and_verify_git() -> Result<String, ()> {
    let suffix = std::env::consts::EXE_SUFFIX;
    let mut found = None;
    if let Some(path_var) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path_var) {
            for candidate in ["git", &format!("git{suffix}")] {
                let path = dir.join(candidate);
                if path.is_file() {
                    found = Some(path.to_string_lossy().into_owned());
                    break;
                }
            }
            if found.is_some() {
                break;
            }
        }
    }

    let program = found.ok_or(())?;
    let version = std::process::Command::new(&program)
        .arg("--version")
        .output()
        .map_err(|_| ())?;
    if !version.status.success() {
        return Err(());
    }
    Ok(program)
}

/// The git binary, resolved once per process (SPEC §8): honoring the user's git
/// (their `.gitconfig`, hooks, credentials) is the point of shelling out — we only
/// require that *some* working git exists.
static GIT: LazyLock<Result<String, ()>> = LazyLock::new(find_and_verify_git);

/// The path of the working git binary, or [`GitError::GitNotFound`] when there is none.
pub(crate) fn resolve_git() -> Result<String, GitError> {
    match LazyLock::force(&GIT) {
        Ok(program) => Ok(program.clone()),
        Err(()) => Err(GitError::GitNotFound),
    }
}

/// Per-repo operation queue: ops on the same repository run **one at a time**
/// (no `.git/index.lock` or ref races, SPEC §8), while ops on *different*
/// repositories run concurrently.
#[derive(Default)]
pub struct OpQueue {
    locks: AsyncMutex<HashMap<RepoId, Arc<AsyncMutex<()>>>>,
}

impl OpQueue {
    pub fn new() -> Self {
        Self::default()
    }

    /// Run `op` serialized against every other op on `repo`.
    ///
    /// `repo` is the canonical worktree root (the repository id), so two checkouts
    /// of the same repository (worktrees) serialize against each other only when
    /// the user treats them as one repo — matching the registry's identity.
    pub async fn run<R, F: Future<Output = R>>(&self, repo: &str, op: F) -> R {
        let turn = self.turn(repo).await;
        let _held = turn.lock().await;
        op.await
    }

    /// The serialization slot for `repo`, created on first use.
    async fn turn(&self, repo: &str) -> Arc<AsyncMutex<()>> {
        let mut locks = self.locks.lock().await;
        if let Some(turn) = locks.get(repo) {
            return turn.clone();
        }
        let turn = Arc::new(AsyncMutex::new(()));
        locks.insert(repo.to_string(), turn.clone());
        turn
    }
}

/// Runs [`CommandPlan`]s with the real git binary, through the per-repo queue.
///
/// The resolved git program is cached after the first use; a missing binary
/// surfaces as a failed [`OpResult`] (the UI shows the error, SPEC §18) rather
/// than a dead app.
pub struct GitExecutor {
    program: LazyLock<Result<String, ()>>,
    queue: OpQueue,
}

impl Default for GitExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl GitExecutor {
    /// Create an executor. Never fails: the git binary is resolved lazily on the
    /// first run (see [`GIT`]).
    pub fn new() -> Self {
        Self {
            program: LazyLock::new(|| resolve_git().map_err(|_| ())),
            queue: OpQueue::new(),
        }
    }

    /// The cached git program path, or `None` when no git binary was found.
    pub fn program(&self) -> Option<&str> {
        match LazyLock::force(&self.program) {
            Ok(p) => Some(p.as_str()),
            Err(()) => None,
        }
    }

    /// Run `plan` in a repository's worktree, serialized against other ops on
    /// that repo (keyed by `id`).
    ///
    /// Takes the repo id and worktree path — not `&Repo` — because the queued
    /// future must be `Send` (Tauri's multi-threaded runtime) and the
    /// `gix::Repository` inside `Repo` is `!Sync`.
    ///
    /// Steps run in order; the first non-zero exit stops the remaining steps and
    /// is reported in the result. Spawning failures (no git binary) also land in
    /// the result, not as an `Err`.
    pub async fn run_plan(&self, id: &str, cwd: &Path, plan: &CommandPlan) -> OpResult {
        self.queue
            .run(id, self.exec_plan(cwd, plan.clone()))
            .await
    }

    async fn exec_plan(&self, cwd: &Path, plan: CommandPlan) -> OpResult {
        let start = Instant::now();
        let mut stdout = String::new();
        let mut stderr = String::new();
        let mut exit: i32 = -1;
        let mut ok = true;

        let program = match self.program() {
            Some(program) => program.to_string(),
            None => {
                ok = false;
                stderr.push_str("git binary not found on PATH\n");
                return OpResult {
                    plan,
                    ok,
                    exit,
                    stdout,
                    stderr,
                    duration_ms: start.elapsed().as_millis() as u64,
                    conflicts: Vec::new(),
                };
            }
        };

        for step in &plan.steps {
            // Steps normally run the resolved git binary; a step with an
            // explicit `program` (the external-editor hand-off) runs that
            // instead (resolved via PATH, like git).
            let program = step.program.as_deref().unwrap_or(&program);
            let mut cmd = tokio::process::Command::new(program);
            cmd.args(&step.argv[1..]);
            cmd.current_dir(cwd);
            // `spawn()` (unlike `output()`) inherits stdio by default, so the
            // capture streams must be piped explicitly or nothing is recorded.
            cmd.stdout(std::process::Stdio::piped());
            cmd.stderr(std::process::Stdio::piped());
            if step.input.is_some() {
                cmd.stdin(std::process::Stdio::piped());
            } else {
                cmd.stdin(std::process::Stdio::null());
            }
            for (key, value) in ENV_SET {
                cmd.env(key, value);
            }
            for key in ENV_UNSET {
                cmd.env_remove(key);
            }

            match cmd.spawn() {
                Ok(mut child) => {
                    // Pipe the step's stdin (if any) before waiting: `git
                    // apply --cached` reads its patch from stdin. Closing the
                    // write half sends EOF so git proceeds.
                    if let Some(input) = &step.input {
                        if let Some(mut stdin) = child.stdin.take() {
                            use tokio::io::AsyncWriteExt;
                            let _ = stdin.write_all(input.as_bytes()).await;
                            let _ = stdin.shutdown().await;
                        }
                    }
                    match child.wait_with_output().await {
                        Ok(output) => {
                            stdout.push_str(&String::from_utf8_lossy(&output.stdout));
                            stderr.push_str(&String::from_utf8_lossy(&output.stderr));
                            exit = output.status.code().unwrap_or(-1);
                            if exit != 0 {
                                ok = false;
                                break; // safe failure: the remaining steps must not run
                            }
                        }
                        Err(e) => {
                            exit = -1;
                            ok = false;
                            stderr.push_str(&format!("failed to run {program}: {e}\n"));
                            break;
                        }
                    }
                }
                Err(e) => {
                    exit = -1;
                    ok = false;
                    stderr.push_str(&format!("failed to spawn {program}: {e}\n"));
                    break;
                }
            }
        }

        OpResult {
            plan,
            ok,
            exit,
            stdout,
            stderr,
            duration_ms: start.elapsed().as_millis() as u64,
            conflicts: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Repo;
    use git_gotten_protocol::{GitStep, Risk};
    use git_gotten_testutil as testutil;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    // ── git binary resolution ──────────────────────────────────────────────

    #[test]
    fn resolves_a_working_git_binary() {
        let program = resolve_git().expect("the test suite requires git on PATH");
        assert!(!program.is_empty());
        assert!(
            program.ends_with("git") || program.ends_with("git.exe"),
            "resolved {program:?}"
        );
        // A second resolution returns the same (cached) program.
        assert_eq!(resolve_git().unwrap(), program);
    }

    // ── OpQueue serialization ──────────────────────────────────────────────

    /// A throwaway repository fixture to key the queue by.
    fn queue_fixture() -> testutil::Fixture {
        let fx = testutil::Fixture::new("queue");
        fx.commit_file("a.txt", "one\n", "c1");
        fx
    }

    #[tokio::test]
    async fn queue_serializes_ops_on_the_same_repo() {
        let fx = queue_fixture();
        let repo_id = fx.id().to_string();
        let queue: Arc<OpQueue> = Arc::new(OpQueue::new());

        // Two concurrent ops on the same repo: while the first holds the turn
        // for 200ms, the second must not start; afterwards it must complete.
        let running = Arc::new(AtomicU64::new(0));
        let starts = Arc::new(std::sync::Mutex::new(Vec::<u64>::new()));
        let ends = Arc::new(std::sync::Mutex::new(Vec::<u64>::new()));
        fn now() -> u64 {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos() as u64
        }

        let mut tasks = Vec::new();
        for _ in 0..2 {
            let running = running.clone();
            let starts = starts.clone();
            let ends = ends.clone();
            let slot = Arc::clone(&queue);
            let id = repo_id.clone();
            tasks.push(tokio::spawn(async move {
                slot.run(&id, async move {
                    running.fetch_add(1, Ordering::SeqCst);
                    let t = now();
                    starts.lock().unwrap().push(t);
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    running.fetch_sub(1, Ordering::SeqCst);
                    ends.lock().unwrap().push(now());
                    "done".to_string()
                })
                .await
            }));
        }

        let results: Vec<String> = futures_join_all(&mut tasks).await;
        assert_eq!(results, vec!["done".to_string(); 2], "no lock error: both ops complete");

        let starts: Vec<u64> = starts.lock().unwrap().clone();
        let ends: Vec<u64> = ends.lock().unwrap().clone();
        assert_eq!(starts.len(), 2);
        assert_eq!(ends.len(), 2);

        let min_start = starts.iter().copied().min().unwrap();
        let max_start = starts.iter().copied().max().unwrap();
        // The second op only starts ~200ms after the first (it waited its turn).
        let (_, s2) = if starts[0] <= starts[1] {
            (starts[0], starts[1])
        } else {
            (starts[1], starts[0])
        };
        let first_end = ends
            .iter()
            .copied()
            .min()
            .unwrap(); // the first op's end is the earliest end
        assert!(
            s2 >= first_end - 5_000_000, // 5ms tolerance for scheduler latency
            "second op (start {s2}) began before the first ended ({first_end})"
        );
        assert!(
            max_start - min_start >= 100_000_000,
            "the ops overlapped (gap {}ns) — the queue must serialize",
            max_start - min_start
        );
    }

    #[tokio::test]
    async fn queue_runs_different_repos_concurrently() {
        let queue: Arc<OpQueue> = Arc::new(OpQueue::new());
        let running = Arc::new(AtomicU64::new(0));
        let peak = Arc::new(AtomicU64::new(0));

        let mut tasks = Vec::new();
        for repo in ["repo-a", "repo-b"] {
            let running = running.clone();
            let peak = peak.clone();
            let slot = Arc::clone(&queue);
            let id = repo.to_string();
            tasks.push(tokio::spawn(async move {
                slot.run(&id, async move {
                    let current = running.fetch_add(1, Ordering::SeqCst) + 1;
                    let _ = peak.fetch_max(current, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(150)).await;
                    running.fetch_sub(1, Ordering::SeqCst);
                })
                .await
            }));
        }
        futures_join_all::<()>(&mut tasks).await;
        assert_eq!(
            peak.load(Ordering::SeqCst),
            2,
            "different repos must run concurrently, not serially"
        );
    }

    /// Await every spawned task in order.
    async fn futures_join_all<T>(tasks: &mut Vec<tokio::task::JoinHandle<T>>) -> Vec<T> {
        let mut out = Vec::with_capacity(tasks.len());
        for t in tasks.drain(..) {
            out.push(t.await.expect("task panicked"));
        }
        out
    }

    // ── GitExecutor on a real fixture ──────────────────────────────────────

    /// A fixture with one *staged* file, so `git commit` succeeds.
    fn staged_fixture() -> testutil::Fixture {
        let fx = testutil::Fixture::new("exec");
        fx.commit_file("a.txt", "one\n", "c1");
        fx.write_file("b.txt", "two\n");
        fx.add("b.txt");
        fx
    }

    fn commit_plan(message: &str) -> CommandPlan {
        CommandPlan {
            steps: vec![GitStep {
                argv: vec![
                    "git".to_string(),
                    "commit".to_string(),
                    "-m".to_string(),
                    message.to_string(),
                ],
                summary: "create a commit".into(),
                risk: Risk::Mutating,
                effect: None,
                input: None,
            program: None,
            }],
            display: format!("git commit -m \"{message}\""),
            preview: None,
        }
    }

    #[tokio::test]
    async fn executor_runs_a_real_commit() {
        let fx = staged_fixture();
        let repo = Repo::open(&fx.path).unwrap();
        let executor = GitExecutor::new();

        let result = executor.run_plan(&repo.id, &repo.path, &commit_plan("add b")).await;

        assert!(result.ok, "stderr: {}", result.stderr);
        assert_eq!(result.exit, 0);
        assert!(
            !result.stdout.is_empty(),
            "git commit must produce captured output"
        );

        // The commit actually happened: b.txt is no longer a staged change.
        let status = repo.status().unwrap();
        assert!(
            !status.changes.iter().any(|c| c.path == "b.txt" && c.staged),
            "b.txt should be committed, got {status:?}"
        );
    }

    #[tokio::test]
    async fn executor_runs_a_real_switch() {
        let fx = testutil::Fixture::new("exec-switch");
        fx.commit_file("a.txt", "one\n", "c1");
        fx.git(&["branch", "side"]);
        let repo = Repo::open(&fx.path).unwrap();
        let executor = GitExecutor::new();

        let plan = CommandPlan {
            steps: vec![GitStep {
                argv: vec![
                    "git".to_string(),
                    "switch".to_string(),
                    "side".to_string(),
                ],
                summary: "switch to side".into(),
                risk: Risk::Mutating,
                effect: None,
                input: None,
            program: None,
            }],
            display: "git switch side".into(),
            preview: None,
        };
        let result = executor.run_plan(&repo.id, &repo.path, &plan).await;

        assert!(result.ok, "stderr: {}", result.stderr);
        let status = repo.status().unwrap();
        assert_eq!(status.branch.as_deref(), Some("side"));
    }

    #[tokio::test]
    async fn executor_reports_failure_as_data_not_error() {
        let fx = testutil::Fixture::new("exec-fail");
        fx.commit_file("a.txt", "one\n", "c1"); // clean tree: `git commit` will fail
        let repo = Repo::open(&fx.path).unwrap();
        let executor = GitExecutor::new();

        let result = executor.run_plan(&repo.id, &repo.path, &commit_plan("nope")).await;

        assert!(!result.ok);
        assert_eq!(result.exit, 1);
        // `git commit` reports its failure on *stdout* (not stderr) with exit 1.
        assert!(
            result.stdout.contains("nothing to commit"),
            "git's exact output must be captured, stdout: {}",
            result.stdout
        );
    }

    #[tokio::test]
    async fn multi_step_plan_stops_at_the_first_failure() {
        // A failing `git commit` (nothing staged) followed by a `git switch`
        // that must NOT run: the branch must be unchanged afterwards.
        let fx = testutil::Fixture::new("exec-multistep");
        fx.commit_file("a.txt", "one\n", "c1");
        fx.git(&["branch", "side"]);
        let repo = Repo::open(&fx.path).unwrap();
        let executor = GitExecutor::new();

        let plan = CommandPlan {
            steps: vec![
                GitStep {
                    argv: vec![
                        "git".to_string(),
                        "commit".to_string(),
                        "-m".to_string(),
                        "never".to_string(),
                    ],
                    summary: "fails".into(),
                    risk: Risk::Mutating,
                    effect: None,
                    input: None,
                program: None,
                },
                GitStep {
                    argv: vec![
                        "git".to_string(),
                        "switch".to_string(),
                        "side".to_string(),
                    ],
                    summary: "must not run".into(),
                    risk: Risk::Mutating,
                    effect: None,
                    input: None,
                program: None,
                },
            ],
            display: "git commit -m \"never\" && git switch side".into(),
            preview: None,
        };

        let result = executor.run_plan(&repo.id, &repo.path, &plan).await;

        assert!(!result.ok);
        assert_eq!(result.exit, 1);
        // The second step never ran: HEAD is still on main.
        let status = repo.status().unwrap();
        assert_eq!(status.branch.as_deref(), Some("main"));
    }

    // ── stdin ───────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn executor_pipes_step_input_to_stdin() {
        let fx = testutil::Fixture::new("exec-stdin");
        let repo = Repo::open(&fx.path).unwrap();
        let executor = GitExecutor::new();

        // `git hash-object -t blob --stdin` is read-only and hashes exactly
        // what it receives on stdin — the SHA of the input is the proof.
        let plan = CommandPlan {
            steps: vec![GitStep {
                argv: vec![
                    "git".to_string(),
                    "hash-object".to_string(),
                    "-t".to_string(),
                    "blob".to_string(),
                    "--stdin".to_string(),
                ],
                summary: "hash the stdin blob".to_string(),
                risk: Risk::ReadOnly,
                effect: None,
                input: Some("hello\n".to_string()),
            program: None,
            }],
            display: "git hash-object -t blob --stdin".to_string(),
            preview: None,
        };
        let result = executor.run_plan(&repo.id, &repo.path, &plan).await;

        assert!(result.ok, "stderr: {}", result.stderr);
        assert_eq!(
            result.stdout.trim(),
            "ce013625030ba8dba906f756967f9e9ca394464a"
        );
    }
}
