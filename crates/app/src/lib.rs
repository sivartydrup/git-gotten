//! The GitGotten Tauri application — a thin shell over `gitcore` and `graph`.
//!
//! Stage 0 proved the Rust → web bridge with `build_info`. Stage 1 opens the git
//! read model: the `open_repo` / `load_status` commands, an in-memory
//! [`RepoRegistry`] of open [`gitcore::Repo`]s, and a `notify` file-watcher that
//! re-derives status (debounced) and emits `status_changed` so the left nav and
//! top bar track a real repository live.
//!
//! Stage 2 is the command layer (SPEC §8/§9): `plan_op` compiles an [`Op`] into a
//! visible `CommandPlan`, `run_op` executes it through the git CLI (serialized
//! per repo) and records it in the bounded in-memory activity log ([`LogBuffer`]),
//! emitting `op_started` / `op_finished` so the web can show the command stream.

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use git_gotten_protocol::{
    Branch, CommandPlan, CommitDetail, FileDiff, Graph, LogEntry, Op, OpResult, OpStarted, Remote,
    RepoId, RepoMeta, Risk, Status, StatusChanged, Tag,
};
use gitcore::{GitError, GitExecutor};
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use parking_lot::{Mutex, MutexGuard};
use tauri::{AppHandle, Emitter, Manager, State};

/// Errors surfaced by the application shell.
///
/// Serialized for the web as the (transparency-contract) display string, so the UI
/// can always show *why* an operation failed (SPEC §18).
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    /// A `gitcore` (git engine) failure, passed through.
    #[error(transparent)]
    Git(#[from] GitError),

    /// The repository id is not in the open set (e.g. it was never opened).
    #[error("repository is not open: {0}")]
    RepoNotOpen(RepoId),
}

impl serde::Serialize for AppError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

/// In-memory registry of open repositories, keyed by their canonical path id.
#[derive(Default)]
pub struct RepoRegistry {
    repos: Mutex<HashMap<RepoId, gitcore::Repo>>,
}

impl RepoRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert (or replace) a repository in the registry.
    pub fn insert(&self, repo: gitcore::Repo) {
        self.repos
            .lock()
            .insert(repo.id.clone(), repo);
    }

    /// Re-derive a repository's live status, or `None` if it is not open.
    pub fn refresh_status(&self, id: &str) -> Option<Status> {
        self.repos.lock().get(id)?.status().ok()
    }

    /// Re-open a registered repository's `gix` handle in place.
    ///
    /// The handle caches the repository's configuration (and the refs it
    /// discovered) at open time; after an op that touches `branch.*`/remote
    /// config, this is how the read model (`status`, `branches`, `remotes`, and
    /// the next [`plan`](gitcore::Repo::plan)) sees the new state. A repository
    /// whose path has vanished keeps its previous handle.
    pub fn refresh(&self, id: &str) {
        if let Some(repo) = self.repos.lock().get_mut(id) {
            let _ = repo.refresh();
        }
    }

    /// All open repository ids.
    pub fn ids(&self) -> Vec<RepoId> {
        self.repos.lock().keys().cloned().collect()
    }

    /// The number of open repositories.
    pub fn len(&self) -> usize {
        self.repos.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The open repository whose worktree contains `path`, if any.
    pub fn repo_for_path(&self, path: &Path) -> Option<RepoId> {
        // `notify` may hand back a non-canonical path (macOS `/tmp` symlink); resolve it
        // before matching against the canonical worktree roots.
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        self.repos
            .lock()
            .iter()
            .find(|(_, repo)| canonical.starts_with(&repo.path))
            .map(|(id, _)| id.clone())
    }
}

/// The application backend: the open-repo registry, the file watcher, the git
/// [`GitExecutor`] (per-repo serialized ops), and the bounded in-memory activity
/// log. Managed by Tauri (`Send + Sync`) and reached from commands.
pub struct Backend {
    registry: RepoRegistry,
    /// The `notify` watcher (must be kept alive for events to arrive).
    watcher: Mutex<Option<RecommendedWatcher>>,
    /// Runs `CommandPlan`s through the real git binary, serialized per repo (SPEC §8).
    executor: GitExecutor,
    /// The activity log (bounded ring, SPEC §11).
    log: LogBuffer,
}

impl Backend {
    /// Create the backend and its event channel.
    ///
    /// Returns the backend (to be managed) and the receiver end for the watcher
    /// thread. If the platform watcher cannot be created, the backend still works —
    /// just without live updates.
    pub fn new() -> (Self, mpsc::Receiver<notify::Result<Event>>) {
        let (event_tx, event_rx) = mpsc::channel();
        let watcher =
            notify::recommended_watcher(move |res| {
                let _ = event_tx.send(res);
            })
            .ok();
        (
            Self {
                registry: RepoRegistry::new(),
                watcher: Mutex::new(watcher),
                executor: GitExecutor::new(),
                log: LogBuffer::new(),
            },
            event_rx,
        )
    }

    /// Open a repository (validating + canonicalizing it) and register it for
    /// watching. Returns its nav summary.
    pub fn open(&self, path: &str) -> Result<RepoMeta, AppError> {
        let repo = gitcore::Repo::open(Path::new(path))?;
        if let Some(watcher) = self.watcher.lock().as_mut() {
            let _ = watcher.watch(&repo.path, RecursiveMode::Recursive);
        }
        let meta = repo.meta()?;
        self.registry.insert(repo);
        Ok(meta)
    }

    /// Compute the live status of an open repository.
    pub fn load_status(&self, id: &str) -> Result<Status, AppError> {
        let repos = self.registry.lock_repos();
        let repo = repos
            .get(id)
            .ok_or_else(|| AppError::RepoNotOpen(id.to_string()))?;
        Ok(repo.status()?)
    }

    /// The laid-out commit graph of an open repository (IPC `load_graph`,
    /// SPEC §10.1): the web renders it on a canvas.
    pub fn load_graph(&self, id: &str) -> Result<Graph, AppError> {
        let repos = self.registry.lock_repos();
        let repo = repos
            .get(id)
            .ok_or_else(|| AppError::RepoNotOpen(id.to_string()))?;
        Ok(repo.graph()?)
    }

    /// Full details of one commit in an open repository (IPC `load_commit`,
    /// the inspect drawer, SPEC §10.1).
    pub fn load_commit(&self, id: &str, hash: &str) -> Result<CommitDetail, AppError> {
        let repos = self.registry.lock_repos();
        let repo = repos
            .get(id)
            .ok_or_else(|| AppError::RepoNotOpen(id.to_string()))?;
        Ok(repo.commit_detail(hash)?)
    }

    /// The working-tree diff of one file in an open repository (IPC `load_diff`,
    /// the staging UI, SPEC §10.2): both sides parsed into hunks (staged first,
    /// then unstaged) so the web can render per-hunk checkboxes.
    pub fn load_diff(&self, id: &str, path: &str) -> Result<FileDiff, AppError> {
        let repos = self.registry.lock_repos();
        let repo = repos
            .get(id)
            .ok_or_else(|| AppError::RepoNotOpen(id.to_string()))?;
        Ok(repo.diff_file(path)?)
    }
    /// The branches of an open repository (IPC `load_branches`, SPEC §10.5).
    pub fn load_branches(&self, id: &str) -> Result<Vec<Branch>, AppError> {
        let repos = self.registry.lock_repos();
        let repo = repos
            .get(id)
            .ok_or_else(|| AppError::RepoNotOpen(id.to_string()))?;
        Ok(repo.branches()?)
    }

    /// The remotes of an open repository (IPC `load_remotes`, SPEC §10.5).
    pub fn load_remotes(&self, id: &str) -> Result<Vec<Remote>, AppError> {
        let repos = self.registry.lock_repos();
        let repo = repos
            .get(id)
            .ok_or_else(|| AppError::RepoNotOpen(id.to_string()))?;
        Ok(repo.remotes()?)
    }

    /// The tags of an open repository (IPC `load_tags`, SPEC §10.5).
    pub fn load_tags(&self, id: &str) -> Result<Vec<Tag>, AppError> {
        let repos = self.registry.lock_repos();
        let repo = repos
            .get(id)
            .ok_or_else(|| AppError::RepoNotOpen(id.to_string()))?;
        Ok(repo.tags()?)
    }

    /// Compile `op` into its visible `CommandPlan` for an open repository.
    ///
    /// Read-only (gix state only) — the web calls this for the inline hints and the
    /// preview dialog, so the displayed command is always the Rust-side source of truth.
    pub fn plan(&self, id: &str, op: &Op) -> Result<CommandPlan, AppError> {
        let repos = self.registry.lock_repos();
        let repo = repos
            .get(id)
            .ok_or_else(|| AppError::RepoNotOpen(id.to_string()))?;
        let plan = repo.plan(op)?;
        drop(repos);
        Ok(plan)
    }

    /// Run a planned op: execute it (serialized per repo) and record it in the
    /// activity log. A non-zero git exit is a *successful* return value with
    /// `ok: false` — the exact output is data for the UI (safe failure, SPEC §8).
    pub async fn run_op(&self, id: &str, plan: &CommandPlan) -> Result<OpResult, AppError> {
        // The registry lock is a `parking_lot` guard, which is `!Send`: extract the
        // data the executor needs (id + cwd) and drop the guard *before* awaiting.
        let (repo_id, repo_path) = {
            let repos = self.registry.lock_repos();
            let repo = repos
                .get(id)
                .ok_or_else(|| AppError::RepoNotOpen(id.to_string()))?;
            (repo.id.clone(), repo.path.clone())
        };
        let result = self.executor.run_plan(&repo_id, &repo_path, plan).await;
        // An op may have changed the repository's config or refs (`branch.*`,
        // remotes, tags): the registry's `gix` handle caches both at open time,
        // so refresh it to keep the read model and the next `plan` live.
        self.registry.refresh(&repo_id);
        self.log.record(id, &result);
        Ok(result)
    }

    /// The most recent recorded log entry for `repo` (backs the `op_finished` event).
    pub fn last_entry(&self, id: &str) -> Option<LogEntry> {
        self.log.last(id)
    }

    /// The activity log (exposed for the `load_log` command and tests).
    pub fn log(&self) -> &LogBuffer {
        &self.log
    }

    /// Re-derive an open repository's status (`None` if it is not open).
    pub fn refresh_status(&self, id: &str) -> Option<Status> {
        self.registry.refresh_status(id)
    }

    /// The open repository that contains a changed path, if any.
    pub fn repo_for_path(&self, path: &Path) -> Option<RepoId> {
        self.registry.repo_for_path(path)
    }

    /// The open-repo registry (exposed for tests and the workspace UI in later stages).
    pub fn registry(&self) -> &RepoRegistry {
        &self.registry
    }
}

impl RepoRegistry {
    /// The inner repo map (package-private accessor for `Backend`).
    fn lock_repos(&self) -> MutexGuard<'_, HashMap<RepoId, gitcore::Repo>> {
        self.repos.lock()
    }
}

/// In-memory activity log: a bounded ring of the most recent [`LogEntry`]s
/// (SPEC §11 — the command stream; JSON file persistence lands with the
/// workspace persistence in Stage 12).
#[derive(Default)]
pub struct LogBuffer {
    next_id: AtomicU64,
    entries: Mutex<VecDeque<LogEntry>>,
}

/// Maximum number of entries retained (SPEC §11: "last 500").
const LOG_CAP: usize = 500;

impl LogBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append a finished op to the ring, returning the entry with its id.
    pub fn record(&self, repo: &str, result: &OpResult) -> LogEntry {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let entry = LogEntry {
            id: self.next_id.fetch_add(1, Ordering::Relaxed) + 1,
            repo: repo.to_string(),
            display: result.plan.display.clone(),
            ok: result.ok,
            duration_ms: result.duration_ms,
            ts,
            stdout: result.stdout.clone(),
            stderr: result.stderr.clone(),
        };
        let mut ring = self.entries.lock();
        ring.push_back(entry.clone());
        if ring.len() > LOG_CAP {
            let overflow = ring.len() - LOG_CAP;
            ring.drain(0..overflow);
        }
        entry
    }

    /// The retained entries (oldest first), optionally filtered to one repo.
    pub fn snapshot(&self, repo: Option<&str>) -> Vec<LogEntry> {
        self.entries
            .lock()
            .iter()
            .filter(|e| repo.is_none_or(|r| e.repo == *r))
            .cloned()
            .collect()
    }

    /// The most recent entry for `repo`, if any.
    pub fn last(&self, repo: &str) -> Option<LogEntry> {
        self.entries
            .lock()
            .iter()
            .rev()
            .find(|e| e.repo == *repo)
            .cloned()
    }
}

/// App identity, exposed to the web shell via the `build_info` command.
///
/// Kept private: Tauri v2's `#[tauri::command]` macro emits module-scope helpers keyed
/// by the function name, and a `pub` command in a library crate double-defines them
/// (E0255). Tests reach it via `use super::*`.
#[tauri::command]
fn build_info() -> git_gotten_protocol::BuildInfo {
    git_gotten_protocol::BuildInfo {
        name: "GitGotten".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        arch: std::env::consts::ARCH.to_string(),
    }
}

/// Open a repository and register it. Returns its workspace-nav summary.
#[tauri::command]
fn open_repo(path: String, state: State<'_, Backend>) -> Result<RepoMeta, AppError> {
    state.open(&path)
}

/// Compute the live status of an already-open repository.
#[tauri::command]
fn load_status(repo: RepoId, state: State<'_, Backend>) -> Result<Status, AppError> {
    state.load_status(&repo)
}

/// The laid-out commit graph of an open repository (IPC `load_graph`, SPEC §10.1).
#[tauri::command]
fn load_graph(repo: RepoId, state: State<'_, Backend>) -> Result<Graph, AppError> {
    state.load_graph(&repo)
}

/// Full details of one commit (IPC `load_commit`, the inspect drawer, SPEC §10.1).
#[tauri::command]
fn load_commit(
    repo: RepoId,
    hash: String,
    state: State<'_, Backend>,
) -> Result<CommitDetail, AppError> {
    state.load_commit(&repo, &hash)
}

/// The working-tree diff of one file (IPC `load_diff`, the staging UI, SPEC §10.2).
#[tauri::command]
fn load_diff(
    repo: RepoId,
    path: String,
    state: State<'_, Backend>,
) -> Result<FileDiff, AppError> {
    state.load_diff(&repo, &path)
}
/// The branches of an open repository (IPC `load_branches`, SPEC §10.5).
#[tauri::command]
fn load_branches(repo: RepoId, state: State<'_, Backend>) -> Result<Vec<Branch>, AppError> {
    state.load_branches(&repo)
}

/// The remotes of an open repository (IPC `load_remotes`, SPEC §10.5).
#[tauri::command]
fn load_remotes(repo: RepoId, state: State<'_, Backend>) -> Result<Vec<Remote>, AppError> {
    state.load_remotes(&repo)
}

/// The tags of an open repository (IPC `load_tags`, SPEC §10.5).
#[tauri::command]
fn load_tags(repo: RepoId, state: State<'_, Backend>) -> Result<Vec<Tag>, AppError> {
    state.load_tags(&repo)
}

/// Compile an operation into its visible `CommandPlan` without running it (IPC
/// `plan_op`): drives the inline command hints and the preview dialog, so the
/// displayed command is always the Rust-side source of truth (SPEC §8).
#[tauri::command]
fn plan_op(repo: RepoId, op: Op, state: State<'_, Backend>) -> Result<CommandPlan, AppError> {
    state.plan(&repo, &op)
}

/// Run an operation (IPC `run_op`): plan → `op_started` → execute (serialized
/// per repo) → record in the activity log → `op_finished`. The result always
/// carries the exact command and its output, success or failure (SPEC §8/§18).
#[tauri::command]
async fn run_op(
    repo: RepoId,
    op: Op,
    app: AppHandle,
    state: State<'_, Backend>,
) -> Result<OpResult, AppError> {
    let plan = state.plan(&repo, &op)?;
    let risk = plan
        .steps
        .iter()
        .map(|s| s.risk)
        .max()
        .unwrap_or(Risk::ReadOnly);
    let _ = app.emit(
        OP_STARTED,
        OpStarted {
            repo: repo.clone(),
            display: plan.display.clone(),
            risk,
        },
    );
    let result = state.run_op(&repo, &plan).await?;
    if let Some(entry) = state.last_entry(&repo) {
        let _ = app.emit(OP_FINISHED, entry);
    }
    Ok(result)
}

/// The session's activity log (oldest first) — backfills the web's log panel.
#[tauri::command]
fn load_log(state: State<'_, Backend>) -> Vec<LogEntry> {
    state.log().snapshot(None)
}

/// The `status_changed` event name (Rust → web, SPEC §13).
pub const STATUS_CHANGED: &str = "status_changed";

/// The `op_started` event name (Rust → web, SPEC §13).
pub const OP_STARTED: &str = "op_started";

/// The `op_finished` event name (Rust → web, SPEC §13).
pub const OP_FINISHED: &str = "op_finished";

/// Debounce window for coalescing bursts of file-watch events into one status refresh.
const DEBOUNCE: Duration = Duration::from_millis(250);

/// Watcher thread body: coalesce `notify` events per repo, and after a quiet
/// [`DEBOUNCE`] window re-derive the status and emit `status_changed`.
fn watcher_loop(app: AppHandle, rx: mpsc::Receiver<notify::Result<Event>>) {
    let mut last_change: HashMap<RepoId, Instant> = HashMap::new();

    loop {
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(Ok(event)) => {
                let backend = app.state::<Backend>();
                for path in &event.paths {
                    if let Some(id) = backend.repo_for_path(path) {
                        last_change.insert(id, Instant::now());
                    }
                }
            }
            Ok(Err(_)) => {
                // A platform-specific watch error; keep the loop alive.
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                let backend = app.state::<Backend>();
                let now = Instant::now();
                let due: Vec<RepoId> = last_change
                    .iter()
                    .filter(|(_, ts)| now.duration_since(**ts) >= DEBOUNCE)
                    .map(|(id, _)| id.clone())
                    .collect();
                for id in &due {
                    last_change.remove(id);
                }
                for id in due {
                    if let Some(status) = backend.refresh_status(&id) {
                        let _ = app.emit(
                            STATUS_CHANGED,
                            StatusChanged {
                                repo: id,
                                status,
                            },
                        );
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

/// Entry point: build the Tauri app with a single 1280x800 window, manage the
/// backend, and run the file-watcher thread.
pub fn run() {
    let (backend, events) = Backend::new();
    tauri::Builder::default()
        .manage(backend)
        .invoke_handler(tauri::generate_handler![
            build_info,
            open_repo,
            load_status,
            load_graph,
            load_commit,
            load_diff,
            load_branches,
            load_remotes,
            load_tags,
            plan_op,
            run_op,
            load_log
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            std::thread::spawn(move || watcher_loop(handle, events));
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;
    use git_gotten_testutil as testutil;

    #[test]
    fn build_info_reports_identity() {
        let bi = build_info();
        assert_eq!(bi.name, "GitGotten");
        assert_eq!(bi.version, env!("CARGO_PKG_VERSION"));
        assert!(!bi.arch.is_empty());
    }

    #[test]
    fn registry_insert_and_ids_roundtrip() {
        let registry = RepoRegistry::new();
        assert!(registry.is_empty());

        let fx = testutil::Fixture::new("reg");
        fx.commit_file("a.txt", "x\n", "c1");
        let repo = gitcore::Repo::open(&fx.path).unwrap();
        registry.insert(repo);

        assert_eq!(registry.len(), 1);
        assert!(registry.ids().iter().any(|id| id == fx.id()));
        assert!(registry.refresh_status(fx.id()).is_some());
    }

    #[test]
    fn registry_repo_for_path_matches_nested_paths() {
        let registry = RepoRegistry::new();
        let fx = testutil::Fixture::new("nested");
        fx.commit_file("a.txt", "x\n", "c1");
        let repo = gitcore::Repo::open(&fx.path).unwrap();
        registry.insert(repo);

        let nested = fx.path.join("src").join("deep.rs");
        assert_eq!(registry.repo_for_path(&nested).as_deref(), Some(fx.id()));
        assert_eq!(registry.repo_for_path(Path::new("/definitely/not/a/repo")), None);
    }

    #[test]
    fn open_then_load_status_roundtrip() {
        let (backend, _rx) = Backend::new();
        let fx = testutil::status_fixture();

        let meta = backend.open(fx.id()).unwrap();
        assert_eq!(meta.id, fx.id());
        assert_eq!(meta.dirty_count, 3);

        let status = backend.load_status(fx.id()).unwrap();
        assert_eq!(status.changes.len(), 3);
        assert_eq!(status.branch.as_deref(), Some("main"));
    }

    #[test]
    fn load_status_on_unknown_id_errors() {
        let (backend, _rx) = Backend::new();
        let err = backend.load_status("/does/not/exist").unwrap_err();
        assert!(matches!(err, AppError::RepoNotOpen(_)), "got {err}");
    }

    #[test]
    fn load_graph_returns_the_laid_out_graph() {
        let (backend, _rx) = Backend::new();
        let fx = testutil::Fixture::new("app-graph");
        fx.commit_file("a.txt", "x\n", "c1");
        backend.open(fx.id()).unwrap();

        let g = backend.load_graph(fx.id()).unwrap();
        assert_eq!(g.nodes.len(), 1);
        assert_eq!(g.nodes[0].subject, "c1");
        assert!(g.nodes[0].is_head);
        assert!(
            g.nodes[0].refs.iter().any(|r| r.name == "main"),
            "HEAD's branch ref must be pinned: {:?}",
            g.nodes[0].refs
        );
    }

    #[test]
    fn load_graph_on_unknown_repo_errors() {
        let (backend, _rx) = Backend::new();
        let err = backend.load_graph("/does/not/exist").unwrap_err();
        assert!(matches!(err, AppError::RepoNotOpen(_)), "got {err}");
    }

    #[test]
    fn load_commit_reads_details_and_rejects_unknown_hashes() {
        let (backend, _rx) = Backend::new();
        let fx = testutil::Fixture::new("app-commit");
        fx.commit_file("a.txt", "x\n", "c1");
        backend.open(fx.id()).unwrap();

        let hash = fx.git(&["rev-parse", "HEAD"]);
        let d = backend.load_commit(fx.id(), &hash).unwrap();
        assert_eq!(d.hash, hash);
        assert_eq!(d.subject, "c1");
        assert!(d.parents.is_empty(), "root commit");
        assert!(d.is_head);
        assert_eq!(d.stat.len(), 1);
        assert_eq!(d.stat[0].path, "a.txt");

        let err = backend
            .load_commit(fx.id(), "zzz")
            .unwrap_err();
        assert!(
            matches!(err, AppError::Git(GitError::NoCommit { .. })),
            "got {err}"
        );
    }

    #[test]
    fn open_non_repository_surfaces_git_error() {
        let (backend, _rx) = Backend::new();
        let dir = std::env::temp_dir().join(format!(
            "gitgotten-app-norepo-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let err = backend.open(dir.to_str().unwrap()).unwrap_err();
        assert!(matches!(err, AppError::Git(GitError::NotARepository { .. })), "got {err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn refresh_status_reports_live_changes() {
        let (backend, _rx) = Backend::new();
        let fx = testutil::Fixture::new("live");
        fx.commit_file("a.txt", "v1\n", "c1");
        backend.open(fx.id()).unwrap();

        // A clean repo first.
        assert!(backend.refresh_status(fx.id()).unwrap().changes.is_empty());

        // Touch a tracked file; a refresh must see it as a (binary=false) modification.
        std::fs::write(fx.path.join("a.txt"), "v2\n").unwrap();
        let status = backend.refresh_status(fx.id()).unwrap();
        let changed = status
            .changes
            .iter()
            .find(|c| c.path == "a.txt")
            .expect("a.txt should be reported");
        assert!(!changed.binary);
        assert!(!changed.staged);
    }

    #[test]
    fn watcher_receives_events_for_watched_file() {
        let fx = testutil::Fixture::new("watch");
        fx.commit_file("a.txt", "v1\n", "c1");
        let (tx, rx) = mpsc::channel::<notify::Result<Event>>();
        let mut watcher =
            notify::recommended_watcher(move |res| {
                let _ = tx.send(res);
            })
            .expect("recommended watcher should work on this platform");
        watcher
            .watch(&fx.path, RecursiveMode::Recursive)
            .expect("watching the fixture root should work");

        // Let the platform watcher arm, then change the file.
        std::thread::sleep(Duration::from_millis(100));
        std::fs::write(fx.path.join("a.txt"), "v2\n").unwrap();

        let mut seen = false;
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && !seen {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(Ok(event)) => {
                    seen = event.paths.iter().any(|p| p.ends_with("a.txt"));
                }
                Ok(Err(_)) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
        assert!(seen, "expected a notify event for a.txt");
    }
    // ── Stage 2: activity log + run_op ─────────────────────────────────────

    fn noop_plan(display: &str) -> CommandPlan {
        CommandPlan {
            steps: Vec::new(),
            display: display.to_string(),
            preview: None,
        }
    }

    fn result_with(plan: &CommandPlan, ok: bool) -> OpResult {
        OpResult {
            plan: plan.clone(),
            ok,
            exit: i32::from(ok),
            stdout: String::new(),
            stderr: String::new(),
            duration_ms: 1,
        }
    }

    #[test]
    fn log_buffer_records_and_caps_at_500() {
        let log = LogBuffer::new();
        let plan = noop_plan("git commit -m \"x\"");
        let result = result_with(&plan, true);

        for _ in 0..LOG_CAP + 2 {
            log.record("repo", &result);
        }

        let entries = log.snapshot(None);
        assert_eq!(entries.len(), LOG_CAP);
        // The two oldest entries were evicted; ids stay monotonic.
        assert_eq!(entries.first().unwrap().id, 3);
        assert_eq!(entries.last().unwrap().id, (LOG_CAP + 2) as u64);
    }

    #[test]
    fn log_snapshot_filters_by_repo() {
        let log = LogBuffer::new();
        let plan_a = noop_plan("git commit -m \"a\"");
        let plan_b = noop_plan("git commit -m \"b\"");
        log.record("repo-a", &result_with(&plan_a, true));
        log.record("repo-b", &result_with(&plan_b, false));

        let a = log.snapshot(Some("repo-a"));
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].display, "git commit -m \"a\"");
        assert_eq!(log.snapshot(Some("repo-b"))[0].display, "git commit -m \"b\"");
        assert_eq!(log.snapshot(None).len(), 2);
    }

    #[test]
    fn plan_op_on_unknown_repo_errors() {
        let (backend, _rx) = Backend::new();
        let err = backend
            .plan(
                "/does/not/exist",
                &Op::Checkout {
                    target: "x".into(),
                    force: false,
                },
            )
            .unwrap_err();
        assert!(matches!(err, AppError::RepoNotOpen(_)), "got {err}");
    }

    #[test]
    fn plan_op_surfaces_git_errors() {
        let (backend, _rx) = Backend::new();
        let fx = testutil::Fixture::new("plan-err");
        fx.commit_file("a.txt", "x\n", "c1");
        backend.open(fx.id()).unwrap();

        let err = backend
            .plan(
                fx.id(),
                &Op::Commit {
                    message: "   ".into(),
                    amend: false,
                    author: None,
                    co_authors: vec![],
                },
            )
            .unwrap_err();
        assert!(
            matches!(err, AppError::Git(GitError::EmptyCommitMessage)),
            "got {err}"
        );
    }

    #[tokio::test]
    async fn run_op_commits_and_records_the_activity_log() {
        let (backend, _rx) = Backend::new();
        let fx = testutil::status_fixture(); // has a staged c.txt
        backend.open(fx.id()).unwrap();

        let op = Op::Commit {
            message: "add c".into(),
            amend: false,
            author: None,
            co_authors: vec![],
        };
        let plan = backend.plan(fx.id(), &op).unwrap();
        assert_eq!(plan.display, r#"git commit -m "add c""#);

        let result = backend.run_op(fx.id(), &plan).await.unwrap();
        assert!(result.ok, "stderr: {}", result.stderr);

        let entry = backend.last_entry(fx.id()).unwrap();
        assert!(entry.ok);
        assert_eq!(entry.display, plan.display);
        assert!(entry.id >= 1);
        assert_eq!(backend.log().snapshot(Some(fx.id())).len(), 1);

        // The commit actually landed: the staged c.txt is gone from the status.
        let status = backend.refresh_status(fx.id()).unwrap();
        assert!(!status.changes.iter().any(|c| c.path == "c.txt"));
    }

    #[tokio::test]
    async fn run_op_unknown_repo_errors() {
        let (backend, _rx) = Backend::new();
        let plan = noop_plan("git noop");
        let err = backend.run_op("/does/not/exist", &plan).await.unwrap_err();
        assert!(matches!(err, AppError::RepoNotOpen(_)), "got {err}");
    }
    /// Regression: an op that rewrites git config (`branch.*`) must be visible
    /// through the registry's `gix` handle — it caches the config at open time,
    /// so `run_op` refreshes the handle before the next read.
    #[tokio::test]
    async fn run_op_refreshes_the_cached_config_handle() {
        let (backend, _rx) = Backend::new();
        let mut fx = testutil::Fixture::new("config-refresh");
        fx.commit_file("a.txt", "one\n", "c1");
        let _ = fx.add_origin("main"); // main tracks origin/main in config
        backend.open(fx.id()).unwrap();
        assert_eq!(
            backend.refresh_status(fx.id()).unwrap().upstream.as_deref(),
            Some("origin/main")
        );

        let op = Op::BranchSetUpstream {
            branch: "main".into(),
            remote: None,
            name: String::new(),
        };
        let plan = backend.plan(fx.id(), &op).unwrap();
        let result = backend.run_op(fx.id(), &plan).await.unwrap();
        assert!(result.ok, "stderr: {}", result.stderr);

        // The unset must be visible: without the post-op refresh the stale
        // handle would still report the pre-op upstream.
        assert_eq!(
            backend.refresh_status(fx.id()).unwrap().upstream,
            None
        );
    }
    /// The ref loaders see the fixture's branches, remote, and tags.
    #[test]
    fn ref_loaders_read_live_refs() {
        let (backend, _rx) = Backend::new();
        let mut fx = testutil::Fixture::new("ref-loaders");
        fx.commit_file("a.txt", "one\n", "c1");
        let _ = fx.add_origin("main");
        let _ = fx.git(&["branch", "side"]);
        let head = fx.git(&["rev-parse", "HEAD"]);
        let _ = fx.git(&["tag", "v1", &head]);
        backend.open(fx.id()).unwrap();

        let branches = backend.load_branches(fx.id()).unwrap();
        let names: Vec<&str> = branches.iter().map(|b| b.name.as_str()).collect();
        assert!(names.contains(&"main") && names.contains(&"side"), "got {names:?}");

        let remotes = backend.load_remotes(fx.id()).unwrap();
        assert_eq!(remotes.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), vec!["origin"]);

        let tags = backend.load_tags(fx.id()).unwrap();
        assert_eq!(tags.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), vec!["v1"]);

        let err = backend.load_branches("/nope").unwrap_err();
        assert!(matches!(err, AppError::RepoNotOpen(_)), "got {err}");
    }

}
