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
//!
//! Stage 7 completes the sync + merge surface (SPEC §10.4): a conflicted
//! `run_op` (merge/rebase/cherry-pick) carries the unmerged file list in the
//! [`OpResult`] and the activity-log entry, so the web can route to the
//! conflict view.
//!
//! Stage 11 adds the stash / submodule / bisect commands (SPEC §10.6, §10.8,
//! §10.9): the `load_stashes` / `load_submodules` / `load_bisect` reads, and
//! `bisect_test`, which runs the test command and the resulting
//! `git bisect good|bad` verdict as two fully planned, logged steps.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use git_gotten_protocol::{
    BisectState, BisectVerdict, Branch, CommandPlan, CommitDetail, ConflictFile, FileDiff, Graph,
    GroupId, LogEntry, Op, OpResult, OpStarted, RebaseCommit, Remote, RepoId, RepoMeta, Risk,
    Stash, Status, StatusChanged, Submodule, Tag, Worktree, Workspace, WorkspaceRepo,
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
    /// A malformed workspace tree, rejected before it can be persisted.
    #[error("invalid workspace: {0}")]
    Workspace(String),

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
        self.repos.lock().insert(repo.id.clone(), repo);
    }

    /// Re-derive a repository's live status, or `None` if it is not open.
    pub fn refresh_status(&self, id: &str) -> Option<Status> {
        self.repos.lock().get(id)?.status().ok()
    }
    /// The unmerged file paths of an open repository (`None` if it is not
    /// open, `empty` when it is not in a conflicted state).
    pub fn unmerged_files(&self, id: &str) -> Option<Vec<String>> {
        let repos = self.repos.lock();
        let repo = repos.get(id)?;
        Some(repo.unmerged_files().ok()?)
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
    /// The live workspace tree (SPEC §7), mirrored in memory and persisted to `app.json`.
    workspace: Mutex<Workspace>,
    /// Where the tree is persisted (`app_data_dir()/app.json`, None until the store is set).
    store_path: Mutex<Option<PathBuf>>,

}

impl Backend {
    /// Create the backend and its event channel.
    ///
    /// Returns the backend (to be managed) and the receiver end for the watcher
    /// thread. If the platform watcher cannot be created, the backend still works —
    /// just without live updates.
    pub fn new() -> (Self, mpsc::Receiver<notify::Result<Event>>) {
        let (event_tx, event_rx) = mpsc::channel();
        let watcher = notify::recommended_watcher(move |res| {
            let _ = event_tx.send(res);
        })
        .ok();
        (
            Self {
                registry: RepoRegistry::new(),
                watcher: Mutex::new(watcher),
                executor: GitExecutor::new(),
                log: LogBuffer::new(),
                workspace: Mutex::new(Workspace::new()),
                store_path: Mutex::new(None),
            },
            event_rx,
        )
    }

    /// Open a repository (validating + canonicalizing it), register it for
    /// watching, and add it to the workspace tree + recents (SPEC §7).
    pub fn open(&self, path: &str) -> Result<RepoMeta, AppError> {
        let meta = self.register(path)?;
        {
            let mut ws = self.workspace.lock();
            ws.ensure_repo(&meta.id);
            ws.touch(&meta.id);
        }
        self.persist();
        Ok(meta)
    }

    /// Open + watch + register a repository, without workspace bookkeeping
    /// (startup re-opens keep the persisted tree ordering).
    fn register(&self, path: &str) -> Result<RepoMeta, AppError> {
        let repo = gitcore::Repo::open(Path::new(path))?;
        if let Some(watcher) = self.watcher.lock().as_mut() {
            let _ = watcher.watch(&repo.path, RecursiveMode::Recursive);
        }
        let meta = repo.meta()?;
        self.registry.insert(repo);
        Ok(meta)
    }

    /// Re-open every repo in the persisted tree (startup). Paths that vanished
    /// stay in the tree; re-opening is best-effort, never fatal (SPEC §11).
    pub fn reopen_workspace_repos(&self) {
        for id in self.workspace().repo_ids() {
            let _ = self.register(&id);
        }
    }

    /// Point the workspace at its `app.json` and load the persisted tree.
    /// A corrupt or missing file yields the default tree (SPEC §11).
    pub fn set_store(&self, path: PathBuf) {
        *self.workspace.lock() = load_workspace(&path);
        // SPEC §11: every repository keeps its own state file in the same directory —
        // here, its activity log. Loading it is what makes the log survive a restart.
        if let Some(dir) = path.parent() {
            self.log.set_dir(dir.to_path_buf());
        }
        *self.store_path.lock() = Some(path);
    }

    pub fn workspace(&self) -> Workspace {
        self.workspace.lock().clone()
    }

    fn persist(&self) {
        let path = self.store_path.lock().clone();
        if let Some(path) = path {
            save_workspace(&path, &self.workspace.lock());
        }
    }

    /// Apply `f` to the tree, validate it, persist, and hand back the new tree.
    fn mutate(
        &self,
        f: impl FnOnce(&mut Workspace) -> Result<(), String>,
    ) -> Result<Workspace, AppError> {
        let mut candidate = self.workspace.lock().clone();
        f(&mut candidate).map_err(AppError::Workspace)?;
        candidate.validate().map_err(AppError::Workspace)?;
        *self.workspace.lock() = candidate;
        self.persist();
        Ok(self.workspace())
    }

    /// Replace the whole tree (the web commits DnD reorder/move through here).
    pub fn set_workspace(&self, workspace: Workspace) -> Result<Workspace, AppError> {
        self.mutate(|ws| {
            *ws = workspace;
            Ok(())
        })
    }

    pub fn group_add(&self, name: &str, parent: Option<&str>) -> Result<Workspace, AppError> {
        let name = name.to_string();
        let parent = parent.map(str::to_string);
        self.mutate(|ws| ws.add_group(&name, parent.as_deref()).map(|_| ()))
    }

    pub fn group_rename(&self, id: &str, name: &str) -> Result<Workspace, AppError> {
        let id = id.to_string();
        let name = name.to_string();
        self.mutate(|ws| ws.rename_group(&id, &name))
    }

    pub fn group_delete(&self, id: &str) -> Result<Workspace, AppError> {
        let id = id.to_string();
        self.mutate(|ws| ws.delete_group(&id))
    }

    pub fn repo_pin(&self, id: &str, pinned: bool) -> Result<Workspace, AppError> {
        let id = id.to_string();
        self.mutate(|ws| {
            if pinned {
                ws.pin(&id);
            } else {
                ws.unpin(&id);
            }
            Ok(())
        })
    }

    /// Record activity for the recents ordering (SPEC §7).
    pub fn repo_touch(&self, id: &str) -> Workspace {
        self.workspace.lock().touch(id);
        self.persist();
        self.workspace()
    }

    /// Unlink a repo from the workspace: tree + registry + watcher. The on-disk
    /// repository is never touched (SPEC §7).
    pub fn remove_repo_from_workspace(&self, id: &str) -> Result<Workspace, AppError> {
        let ws = self.mutate(|ws| {
            ws.remove_repo(id);
            Ok(())
        })?;
        if let Some(repo) = self.registry.remove(id) {
            if let Some(watcher) = self.watcher.lock().as_mut() {
                let _ = watcher.unwatch(&repo.path);
            }
        }
        Ok(ws)
    }

    /// Every open repo in the tree as (meta, live status) — the startup
    /// hydration for nav + dashboard (SPEC §7).
    pub fn workspace_repos(&self) -> Vec<WorkspaceRepo> {
        self.workspace()
            .repo_ids()
            .iter()
            .filter_map(|id| self.registry.repo_pair(id))
            .map(|(meta, status)| WorkspaceRepo { meta, status })
            .collect()
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

    /// The 3-way view of one unmerged file in an open repository (IPC
    /// `load_conflict`, the conflict editor, SPEC §10.5): the three side
    /// versions (index stages 1/2/3) plus the parsed diff3 segments.
    pub fn load_conflict(&self, id: &str, path: &str) -> Result<ConflictFile, AppError> {
        let repos = self.registry.lock_repos();
        let repo = repos
            .get(id)
            .ok_or_else(|| AppError::RepoNotOpen(id.to_string()))?;
        Ok(repo.conflict_file(path)?)
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

    /// The worktrees of an open repository (IPC `load_worktrees`, SPEC §10.7).
    pub fn load_worktrees(&self, id: &str) -> Result<Vec<Worktree>, AppError> {
        let repos = self.registry.lock_repos();
        let repo = repos
            .get(id)
            .ok_or_else(|| AppError::RepoNotOpen(id.to_string()))?;
        Ok(repo.worktrees()?)
    }

    /// The stashes of an open repository (IPC `load_stashes`, SPEC §10.6).
    pub fn load_stashes(&self, id: &str) -> Result<Vec<Stash>, AppError> {
        let repos = self.registry.lock_repos();
        let repo = repos
            .get(id)
            .ok_or_else(|| AppError::RepoNotOpen(id.to_string()))?;
        Ok(repo.stashes()?)
    }

    /// The submodules of an open repository (IPC `load_submodules`, SPEC §10.8).
    pub fn load_submodules(&self, id: &str) -> Result<Vec<Submodule>, AppError> {
        let repos = self.registry.lock_repos();
        let repo = repos
            .get(id)
            .ok_or_else(|| AppError::RepoNotOpen(id.to_string()))?;
        Ok(repo.submodules()?)
    }

    /// The live bisect session of an open repository (IPC `load_bisect`, SPEC §10.9).
    pub fn load_bisect(&self, id: &str) -> Result<BisectState, AppError> {
        let repos = self.registry.lock_repos();
        let repo = repos
            .get(id)
            .ok_or_else(|| AppError::RepoNotOpen(id.to_string()))?;
        Ok(repo.bisect_state()?)
    }

    /// Commits an interactive rebase onto `target` would rewrite (IPC
    /// `load_rebase_commits`, the RebaseDialog list, SPEC §10.4).
    pub fn rebase_commits(&self, id: &str, target: &str) -> Result<Vec<RebaseCommit>, AppError> {
        let repos = self.registry.lock_repos();
        let repo = repos
            .get(id)
            .ok_or_else(|| AppError::RepoNotOpen(id.to_string()))?;
        Ok(repo.rebase_commits(target)?)
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

    /// The executor's targets for an open repository: its serialization key, its
    /// workspace id and its worktree cwd.
    ///
    /// The key is the repository's **common** git dir, not the worktree id, so linked
    /// worktrees of one object database share a turn (SPEC §18). The `parking_lot`
    /// guard is `!Send`, so it is dropped here — before anything is awaited.
    fn op_target(&self, id: &str) -> Result<(String, RepoId, PathBuf), AppError> {
        let repos = self.registry.lock_repos();
        let repo = repos
            .get(id)
            .ok_or_else(|| AppError::RepoNotOpen(id.to_string()))?;
        Ok((
            repo.queue_key().to_string(),
            repo.id.clone(),
            repo.path.clone(),
        ))
    }

    /// Run a planned op: execute it (serialized per repo) and record it in the
    /// activity log. A non-zero git exit is a *successful* return value with
    /// `ok: false` — the exact output is data for the UI (safe failure, SPEC §8).
    pub async fn run_op(&self, id: &str, plan: &CommandPlan) -> Result<OpResult, AppError> {
        let (mut results, _) = self.run_op_group(id, std::iter::once(plan.clone())).await?;
        Ok(results.pop().expect("one result is recorded for one plan"))
    }

    /// Run bisect's test command and the verdict it decides as **one** serialized
    /// flow: the verdict plan cannot exist before the test command's exit code does,
    /// and the per-repo turn must not be released between them — a concurrent op
    /// could move HEAD out from under the verdict (SPEC §18).
    ///
    /// Returns the results (test, then verdict) with their activity-log entries.
    pub async fn run_bisect_flow(
        &self,
        id: &str,
        test_plan: CommandPlan,
    ) -> Result<(Vec<OpResult>, Vec<LogEntry>), AppError> {
        let (queue_key, repo_id, repo_path) = self.op_target(id)?;
        // A shared borrow, so the flow can re-plan while `repo_id` stays owned here.
        let plan_repo = &repo_id;
        let (results, plan_error) = self
            .executor
            .run_flow(&queue_key, &repo_path, |turn| async move {
                let test = turn.run(&test_plan).await;
                let verdict = if test.ok {
                    BisectVerdict::Good
                } else {
                    BisectVerdict::Bad
                };
                match self.plan(plan_repo, &Op::BisectVerdict { verdict }) {
                    Ok(verdict_plan) => {
                        let applied = turn.run(&verdict_plan).await;
                        (vec![test, applied], None)
                    }
                    Err(err) => (vec![test], Some(err)),
                }
            })
            .await;
        self.registry.refresh(&repo_id);
        // Both git commands reach the activity log even if the verdict could not be
        // planned: the UI always gets the command that ran (SPEC §18).
        let mut entries = Vec::with_capacity(results.len());
        for result in &results {
            entries.push(self.log.record(&repo_id, result));
        }
        if let Some(err) = plan_error {
            return Err(err);
        }
        Ok((results, entries))
    }

    /// Run several plans as **one** serialized critical section and return their
    /// results with the activity-log entries they produced (in plan order).
    ///
    /// The per-repo queue turn is held across the whole group, so no other op can
    /// slip between the git commands of one logical flow (SPEC §18). Each plan is
    /// still executed, capped, refreshed and recorded exactly as a standalone op, so
    /// the activity log looks identical either way.
    pub async fn run_op_group(
        &self,
        id: &str,
        plans: impl IntoIterator<Item = CommandPlan>,
    ) -> Result<(Vec<OpResult>, Vec<LogEntry>), AppError> {
        let (queue_key, repo_id, repo_path) = self.op_target(id)?;
        let mut results = self.executor.run_plans(&queue_key, &repo_path, plans).await;
        // An op may have changed the repository's config or refs (`branch.*`,
        // remotes, tags): the registry's `gix` handle caches both at open time,
        // so refresh it to keep the read model and the next `plan` live.
        self.registry.refresh(&repo_id);
        let mut entries = Vec::with_capacity(results.len());
        for result in &mut results {
            // If the op stopped on a conflict (a merge/rebase/cherry-pick), carry
            // the unmerged file list in the result — the web routes it to the
            // conflict view (SPEC §10.5). A clean op yields an empty list.
            result.conflicts = self.registry.unmerged_files(&repo_id).unwrap_or_default();
            // Worktree paths in plans are canonicalized by the planner while
            // the directory still exists, matching the workspace repository id.
            if result.ok {
                if let [step] = result.plan.steps.as_slice() {
                    let removed = match step.argv.as_slice() {
                        [git, worktree, remove, path]
                            if git == "git" && worktree == "worktree" && remove == "remove" =>
                        {
                            Some(path)
                        }
                        [git, worktree, remove, force, path]
                            if git == "git" && worktree == "worktree" && remove == "remove"
                                && force == "--force" =>
                        {
                            Some(path)
                        }
                        _ => None,
                    };
                    if let Some(path) = removed {
                        self.remove_repo_from_workspace(path)?;
                    }
                }
            }
            entries.push(self.log.record(&repo_id, result));
        }
        Ok((results, entries))
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

    /// Unregister a repository (`remove_repo_from_workspace`).
    pub fn remove(&self, id: &str) -> Option<gitcore::Repo> {
        self.lock_repos().remove(id)
    }

    /// (meta, live status) for an open repo (`workspace_repos`).
    pub fn repo_pair(&self, id: &str) -> Option<(RepoMeta, Status)> {
        let repos = self.lock_repos();
        let repo = repos.get(id)?;
        repo.meta_and_status().ok()
    }
}

/// The activity log (SPEC §11): a bounded ring of the most recent [`LogEntry`]s
/// **per repository** — the command stream — mirrored to one JSON file per repo in
/// the app-data directory, so the log survives a restart.
///
/// Persistence is best-effort in both directions: a log file that cannot be read or
/// parsed is treated as an empty ring and rewritten by the next recorded op, so a
/// corrupt per-repo state file is *rebuilt*, never a crash (SPEC §11/§18).
#[derive(Default)]
pub struct LogBuffer {
    next_id: AtomicU64,
    /// One ring per repository id; SPEC §11 keeps the "last 500" per repo, so a
    /// busy repository cannot evict another repo's history.
    rings: Mutex<HashMap<RepoId, VecDeque<LogEntry>>>,
    /// Directory holding the per-repo files (`None` until the store is set).
    dir: Mutex<Option<PathBuf>>,
}

/// Maximum number of entries retained per repository (SPEC §11: "last 500").
const LOG_CAP: usize = 500;

/// Suffix of a per-repo activity-log file inside the app-data directory.
const LOG_SUFFIX: &str = ".log.json";

/// The file a repository's log lives in: the id hashed, so any repository path
/// becomes a short, filesystem-safe flat name.
fn log_file(dir: &Path, repo: &str) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::hash::DefaultHasher::new();
    repo.hash(&mut hasher);
    dir.join(format!("{:016x}{LOG_SUFFIX}", hasher.finish()))
}

impl LogBuffer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Point the log at the app-data directory and load every repository's ring.
    ///
    /// Never fails (SPEC §11): an unreadable or malformed file — wrong shape,
    /// truncated, not JSON — is skipped, and the next op rewrites it. Entry ids
    /// seed the id counter so ids stay monotonic across restarts.
    pub fn set_dir(&self, dir: PathBuf) {
        let mut rings: HashMap<RepoId, VecDeque<LogEntry>> = HashMap::new();
        let mut max_id = 0u64;
        if let Ok(files) = std::fs::read_dir(&dir) {
            for path in files.filter_map(|e| e.ok()).map(|e| e.path()) {
                if !path.to_string_lossy().ends_with(LOG_SUFFIX) {
                    continue;
                }
                let text = match std::fs::read_to_string(&path) {
                    Ok(text) => text,
                    Err(_) => continue, // corrupt: rebuilt by the next record
                };
                let entries: Vec<LogEntry> = serde_json::from_str(&text).unwrap_or_default();
                for entry in entries {
                    max_id = max_id.max(entry.id);
                    let ring = rings.entry(entry.repo.clone()).or_default();
                    // Two files can name one repository (a renamed path, an old
                    // hash): merge by id, never duplicate a row.
                    if !ring.iter().any(|kept| kept.id == entry.id) {
                        ring.push_back(entry);
                    }
                }
            }
        }
        for ring in rings.values_mut() {
            ring.make_contiguous().sort_by_key(|entry| entry.id);
            if ring.len() > LOG_CAP {
                ring.drain(0..ring.len() - LOG_CAP);
            }
        }
        self.next_id.store(max_id, Ordering::Relaxed);
        *self.rings.lock() = rings;
        *self.dir.lock() = Some(dir);
    }

    /// Append a finished op to its repository's ring, returning the entry with its id.
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
            conflicts: result.conflicts.clone(),
        };
        {
            let mut rings = self.rings.lock();
            let ring = rings.entry(entry.repo.clone()).or_default();
            ring.push_back(entry.clone());
            if ring.len() > LOG_CAP {
                let overflow = ring.len() - LOG_CAP;
                ring.drain(0..overflow);
            }
        }
        self.persist(&entry.repo);
        entry
    }

    /// The retained entries (oldest first), optionally filtered to one repo.
    pub fn snapshot(&self, repo: Option<&str>) -> Vec<LogEntry> {
        let mut entries: Vec<LogEntry> = self
            .rings
            .lock()
            .values()
            .flat_map(|ring| ring.iter().filter(|e| repo.is_none_or(|r| e.repo == *r)).cloned())
            .collect();
        entries.sort_by_key(|entry| entry.id);
        entries
    }

    /// The most recent entry for `repo`, if any.
    pub fn last(&self, repo: &str) -> Option<LogEntry> {
        self.rings.lock().get(repo).and_then(|r| r.back().cloned())
    }

    /// Rewrite one repository's log file (best-effort; a failure only loses history).
    fn persist(&self, repo: &str) {
        let Some(dir) = self.dir.lock().clone() else { return };
        let entries: Vec<LogEntry> = self
            .rings
            .lock()
            .get(repo)
            .map(|ring| ring.iter().cloned().collect())
            .unwrap_or_default();
        if let Ok(json) = serde_json::to_string(&entries) {
            let _ = std::fs::write(log_file(&dir, repo), json);
        }
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
fn load_diff(repo: RepoId, path: String, state: State<'_, Backend>) -> Result<FileDiff, AppError> {
    state.load_diff(&repo, &path)
}
/// The 3-way view of one unmerged file (IPC `load_conflict`, SPEC §10.5).
#[tauri::command]
fn load_conflict(
    repo: RepoId,
    path: String,
    state: State<'_, Backend>,
) -> Result<ConflictFile, AppError> {
    state.load_conflict(&repo, &path)
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

/// The worktrees of an open repository (IPC `load_worktrees`, SPEC §10.7).
#[tauri::command]
fn load_worktrees(repo: RepoId, state: State<'_, Backend>) -> Result<Vec<Worktree>, AppError> {
    state.load_worktrees(&repo)
}

/// The commits an interactive rebase onto `target` would rewrite (IPC
/// `load_rebase_commits`, the RebaseDialog list, SPEC §10.4).
#[tauri::command]
fn load_rebase_commits(
    repo: RepoId,
    target: String,
    state: State<'_, Backend>,
) -> Result<Vec<RebaseCommit>, AppError> {
    state.rebase_commits(&repo, &target)
}

/// The stashes of an open repository (IPC `load_stashes`, SPEC §10.6).
#[tauri::command]
fn load_stashes(repo: RepoId, state: State<'_, Backend>) -> Result<Vec<Stash>, AppError> {
    state.load_stashes(&repo)
}

/// The submodules of an open repository (IPC `load_submodules`, SPEC §10.8).
#[tauri::command]
fn load_submodules(repo: RepoId, state: State<'_, Backend>) -> Result<Vec<Submodule>, AppError> {
    state.load_submodules(&repo)
}

/// The live bisect session of an open repository (IPC `load_bisect`, SPEC §10.9).
#[tauri::command]
fn load_bisect(repo: RepoId, state: State<'_, Backend>) -> Result<BisectState, AppError> {
    state.load_bisect(&repo)
}

/// Run the bisect test command against the checked-out candidate and apply
/// the resulting verdict (IPC `bisect_test`, SPEC §10.9). The literal test argv
/// (`GitStep::program`, no shell) runs first, then `git bisect good|bad` decided by
/// its exit code (exit 0 → Good, git's `bisect run` convention).
///
/// The two are **one** serialized flow (SPEC §18): the repo's queue turn is held
/// across both, so no other op can slip between the test and the verdict it decides.
/// Each is still planned, announced and logged exactly like a standalone op.
#[tauri::command]
async fn bisect_test(
    repo: RepoId,
    command: String,
    app: AppHandle,
    state: State<'_, Backend>,
) -> Result<BisectState, AppError> {
    let test_plan = state.plan(&repo, &Op::BisectTest { command })?;
    emit_started(&app, &repo, &test_plan);
    let (_, entries) = state.run_bisect_flow(&repo, test_plan).await?;
    // Replay the lifecycle in order, so the command stream reads the same as two
    // independent ops: each one finishes before the next appears.
    for entry in entries {
        let _ = app.emit(OP_FINISHED, entry);
    }
    state.load_bisect(&repo)
}

/// Compile an operation into its visible `CommandPlan` without running it (IPC
/// `plan_op`): drives the inline command hints and the preview dialog, so the
/// displayed command is always the Rust-side source of truth (SPEC §8).
#[tauri::command]
fn plan_op(repo: RepoId, op: Op, state: State<'_, Backend>) -> Result<CommandPlan, AppError> {
    state.plan(&repo, &op)
}

/// plan → `op_started` → execute (serialized per repo) → record in the activity
/// log → `op_finished`. Every step of every mutating op runs through here, so
/// the UI always sees the exact command that ran (SPEC §8/§18). Shared by the
/// `run_op` and `bisect_test` commands.
async fn run_planned(
    repo: &RepoId,
    op: &Op,
    app: &AppHandle,
    state: &Backend,
) -> Result<OpResult, AppError> {
    let plan = state.plan(repo, op)?;
    emit_started(app, repo, &plan);
    let result = state.run_op(repo, &plan).await?;
    if let Some(entry) = state.last_entry(repo) {
        let _ = app.emit(OP_FINISHED, entry);
    }
    Ok(result)
}

/// Announce an op's start: the plan's exact display plus the highest risk of its
/// steps, so the UI names the command before it runs (SPEC §8/§18).
fn emit_started(app: &AppHandle, repo: &RepoId, plan: &CommandPlan) {
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
}

/// Run an operation (IPC `run_op`): compiles `op` and runs it through
/// [`run_planned`]. The result always carries the exact command and its
/// output, success or failure (SPEC §8/§18).
#[tauri::command]
async fn run_op(
    repo: RepoId,
    op: Op,
    app: AppHandle,
    state: State<'_, Backend>,
) -> Result<OpResult, AppError> {
    run_planned(&repo, &op, &app, &state).await
}

/// The session's activity log (oldest first) — backfills the web's log panel.
#[tauri::command]
fn load_log(state: State<'_, Backend>) -> Vec<LogEntry> {
    state.log().snapshot(None)
}

// ── Stage 12: workspace commands (SPEC §7, §13) ────────────────────────────

/// The persisted workspace tree (groups, pinned, recents).
#[tauri::command]
fn workspace_get(state: State<'_, Backend>) -> Workspace {
    state.workspace()
}

/// Replace the workspace tree (the web commits DnD reorder/move through here).
#[tauri::command]
fn workspace_set(
    workspace: Workspace,
    state: State<'_, Backend>,
) -> Result<Workspace, AppError> {
    state.set_workspace(workspace)
}

/// Add a group (under `parent`, or the root when null).
#[tauri::command]
fn group_add(
    name: String,
    parent: Option<GroupId>,
    state: State<'_, Backend>,
) -> Result<Workspace, AppError> {
    state.group_add(&name, parent.as_deref())
}

#[tauri::command]
fn group_rename(
    id: GroupId,
    name: String,
    state: State<'_, Backend>,
) -> Result<Workspace, AppError> {
    state.group_rename(&id, &name)
}

/// Delete a group; its repos hoist to the root, they are never unlinked.
#[tauri::command]
fn group_delete(id: GroupId, state: State<'_, Backend>) -> Result<Workspace, AppError> {
    state.group_delete(&id)
}

#[tauri::command]
fn repo_pin(
    id: RepoId,
    pinned: bool,
    state: State<'_, Backend>,
) -> Result<Workspace, AppError> {
    state.repo_pin(&id, pinned)
}

/// Record repo activity for the recents ordering.
#[tauri::command]
fn repo_touch(id: RepoId, state: State<'_, Backend>) -> Workspace {
    state.repo_touch(&id)
}

/// Unlink a repo from the workspace (the on-disk repo is never touched).
#[tauri::command]
fn remove_repo_from_workspace(
    id: RepoId,
    state: State<'_, Backend>,
) -> Result<Workspace, AppError> {
    state.remove_repo_from_workspace(&id)
}

/// Startup hydration: every open repo in the tree as (meta, live status).
#[tauri::command]
fn workspace_repos(state: State<'_, Backend>) -> Vec<WorkspaceRepo> {
    state.workspace_repos()
}

/// Load `app.json`, best-effort: corrupt or missing → default tree (SPEC §11).
fn load_workspace(path: &Path) -> Workspace {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Workspace>(&text).ok())
        .filter(|ws| ws.validate().is_ok())
        .unwrap_or_default()
}

fn save_workspace(path: &Path, ws: &Workspace) {
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(text) = serde_json::to_string_pretty(ws) {
        let _ = std::fs::write(path, text);
    }
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
                        let _ = app.emit(STATUS_CHANGED, StatusChanged { repo: id, status });
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
        // Stage 14 (SPEC §17): the auto-updater. Its `check` / `download_and_install`
        // commands are the plugin's own; the web shows the prompt, so every updater
        // action stays one the UI can name. Pointing the app at a mock feed is a
        // dev-time `--config` override, not a code path.
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .invoke_handler(tauri::generate_handler![
            build_info,
            open_repo,
            load_status,
            load_graph,
            load_commit,
            load_diff,
            load_conflict,
            load_branches,
            load_remotes,
            load_tags,
            load_worktrees,
            load_rebase_commits,
            load_stashes,
            load_submodules,
            load_bisect,
            bisect_test,
            plan_op,
            run_op,
            load_log,
            workspace_get,
            workspace_set,
            group_add,
            group_rename,
            group_delete,
            repo_pin,
            repo_touch,
            remove_repo_from_workspace,
            workspace_repos,
        ])
        .setup(move |app| {
            let handle = app.handle().clone();
            // Config dir: macOS `~/Library/Application Support/GitGotten`,
            // Windows `%APPDATA%\GitGotten` (SPEC §11; the resolver is OS-aware).
            if let Ok(dir) = handle.path().app_data_dir() {
                handle.state::<Backend>().set_store(dir.join("app.json"));
                handle.state::<Backend>().reopen_workspace_repos();
            }
            std::thread::spawn(move || watcher_loop(handle, events));
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;
    use git_gotten_protocol::{ChildNode, ConflictResolution, ConflictSide, Group, Workspace};
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
        assert_eq!(
            registry.repo_for_path(Path::new("/definitely/not/a/repo")),
            None
        );
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

    #[tokio::test]
    async fn worktree_add_remove_and_switch_retarget_the_active_repo() {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let wt = std::env::temp_dir().join(format!(
            "gitgotten-app-wt-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));

        let (backend, _rx) = Backend::new();
        let fx = testutil::Fixture::new("app-worktree");
        fx.commit_file("a.txt", "x\n", "c1");
        fx.git(&["branch", "side"]);
        backend.open(fx.id()).unwrap();

        let plan = backend
            .plan(
                fx.id(),
                &Op::WorktreeAdd {
                    committish: "side".into(),
                    path: wt.to_string_lossy().into_owned(),
                    new_branch: None,
                },
            )
            .unwrap();
        let result = backend.run_op(fx.id(), &plan).await.unwrap();
        assert!(result.ok, "stderr: {}", result.stderr);
        assert!(
            backend
                .load_worktrees(fx.id())
                .unwrap()
                .iter()
                .any(|w| w.branch.as_deref() == Some("side"))
        );

        // The switcher retargets by opening the worktree root as the active
        // repository: its status is the worktree's checkout (SPEC §10.7).
        let wt_path = std::fs::canonicalize(&wt).unwrap();
        let linked = backend.open(wt_path.to_str().unwrap()).unwrap();
        assert_eq!(linked.branch.as_deref(), Some("side"));
        let status = backend.load_status(&linked.id).unwrap();
        assert_eq!(status.branch.as_deref(), Some("side"));

        // Removing the worktree from the main repo is a clean Mutating op.
        let plan = backend
            .plan(
                fx.id(),
                &Op::WorktreeRemove {
                    path: wt_path.to_string_lossy().into_owned(),
                    force: false,
                },
            )
            .unwrap();
        let result = backend.run_op(fx.id(), &plan).await.unwrap();
        assert!(result.ok, "stderr: {}", result.stderr);
        assert_eq!(backend.load_worktrees(fx.id()).unwrap().len(), 1);
    }

    #[tokio::test]
    async fn removed_worktrees_are_unlinked_from_the_persisted_workspace() {
        for force in [false, true] {
            let dir = store_dir("worktree-remove");
            let wt = dir.join("linked");
            let fx = testutil::Fixture::new("workspace-worktree");
            fx.commit_file("a.txt", "base\n", "base");
            fx.git(&["branch", "side"]);
            let other = testutil::Fixture::new("workspace-other");
            other.commit_file("other.txt", "other\n", "other");

            let (backend, _rx) = Backend::new();
            backend.set_store(dir.join("app.json"));
            backend.open(fx.id()).unwrap();
            backend.open(other.id()).unwrap();
            let add = backend.plan(fx.id(), &Op::WorktreeAdd {
                committish: "side".into(),
                path: wt.to_string_lossy().into_owned(),
                new_branch: None,
            }).unwrap();
            assert!(backend.run_op(fx.id(), &add).await.unwrap().ok);
            let linked = backend.open(wt.to_str().unwrap()).unwrap();
            backend.repo_pin(&linked.id, true).unwrap();
            backend.repo_pin(other.id(), true).unwrap();
            let mut expected = backend.group_add("Linked", None).unwrap();
            let group = expected.groups.iter().find(|g| g.name == "Linked").unwrap().id.clone();
            expected.groups.iter_mut().find(|g| g.id == "root").unwrap().children
                .retain(|child| !matches!(child, ChildNode::Repo { id } if id == &linked.id));
            expected.groups.iter_mut().find(|g| g.id == group).unwrap().children
                .push(ChildNode::Repo { id: linked.id.clone() });
            backend.set_workspace(expected.clone()).unwrap();
            expected.remove_repo(&linked.id);
            if force {
                std::fs::write(wt.join("a.txt"), "dirty\n").unwrap();
            }
            let remove = backend.plan(fx.id(), &Op::WorktreeRemove {
                path: linked.id.clone(),
                force,
            }).unwrap();
            let result = backend.run_op(fx.id(), &remove).await.unwrap();
            assert!(result.ok, "stderr: {}", result.stderr);
            let workspace = backend.workspace();
            assert_eq!(workspace, expected);
            assert!(!workspace.has_repo(&linked.id));
            assert!(!workspace.pinned.contains(&linked.id));
            assert!(!workspace.recents.contains(&linked.id));
            assert!(workspace.has_repo(fx.id()));
            assert!(workspace.has_repo(other.id()));
            assert!(workspace.pinned.contains(&other.id().to_string()));
            assert!(backend.load_status(&linked.id).is_err());
            assert!(backend.load_status(other.id()).is_ok());

            let (restarted, _rx2) = Backend::new();
            restarted.set_store(dir.join("app.json"));
            restarted.reopen_workspace_repos();
            assert_eq!(restarted.workspace(), workspace);
            assert!(restarted.load_status(&linked.id).is_err());
            assert!(restarted.load_status(fx.id()).is_ok());
            assert!(restarted.load_status(other.id()).is_ok());
        }
    }

    #[tokio::test]
    async fn failed_worktree_removal_keeps_the_persisted_workspace() {
        for force in [false, true] {
            let dir = store_dir("worktree-remove-failed");
            let wt = dir.join("linked");
            let fx = testutil::Fixture::new("workspace-worktree-failed");
            fx.commit_file("a.txt", "base\n", "base");
            fx.git(&["branch", "side"]);
            let (backend, _rx) = Backend::new();
            backend.set_store(dir.join("app.json"));
            backend.open(fx.id()).unwrap();
            let add = backend.plan(fx.id(), &Op::WorktreeAdd {
                committish: "side".into(),
                path: wt.to_string_lossy().into_owned(),
                new_branch: None,
            }).unwrap();
            assert!(backend.run_op(fx.id(), &add).await.unwrap().ok);
            let linked = backend.open(wt.to_str().unwrap()).unwrap();
            backend.repo_pin(&linked.id, true).unwrap();
            let before = backend.workspace();
            let remove = backend.plan(fx.id(), &Op::WorktreeRemove {
                path: linked.id.clone(),
                force,
            }).unwrap();
            // A lock acquired after planning makes git refuse even --force;
            // removing a locked worktree requires --force twice.
            fx.git(&["worktree", "lock", wt.to_str().unwrap()]);
            let result = backend.run_op(fx.id(), &remove).await.unwrap();
            assert!(!result.ok);
            assert_eq!(backend.workspace(), before);
            assert!(backend.load_status(&linked.id).is_ok());

            let (restarted, _rx2) = Backend::new();
            restarted.set_store(dir.join("app.json"));
            restarted.reopen_workspace_repos();
            assert_eq!(restarted.workspace(), before);
            assert!(restarted.load_status(&linked.id).is_ok());
        }
    }

    #[tokio::test]
    async fn stash_push_apply_pop_drop_flow_through_the_backend() {
        let (backend, _rx) = Backend::new();
        let fx = testutil::Fixture::new("app-stash");
        fx.commit_file("a.txt", "x\n", "c1");
        backend.open(fx.id()).unwrap();

        async fn run(backend: &Backend, fx: &testutil::Fixture, op: Op) -> OpResult {
            let plan = backend.plan(fx.id(), &op).unwrap();
            backend.run_op(fx.id(), &plan).await.unwrap()
        }

        fx.write_file("a.txt", "x\nedited\n");
        run(
            &backend,
            &fx,
            Op::StashPush {
                include_untracked: true,
            },
        )
        .await;
        assert!(
            backend.load_status(fx.id()).unwrap().changes.is_empty(),
            "tree cleaned"
        );
        assert_eq!(backend.load_stashes(fx.id()).unwrap().len(), 1);

        // apply keeps the entry, pop consumes it.
        run(&backend, &fx, Op::StashApply { id: 0 }).await;
        assert_eq!(backend.load_stashes(fx.id()).unwrap().len(), 1);
        fx.write_file("a.txt", "x\n");
        run(&backend, &fx, Op::StashPop { id: 0 }).await;
        assert!(backend.load_stashes(fx.id()).unwrap().is_empty());
        assert!(
            backend
                .load_status(fx.id())
                .unwrap()
                .changes
                .iter()
                .any(|c| c.path == "a.txt")
        );
    }

    #[tokio::test]
    async fn bisect_test_steps_are_planned_logged_and_converge() {
        let (backend, _rx) = Backend::new();
        let fx = testutil::Fixture::new("app-bisect");
        fx.write_file("check.sh", "#!/bin/sh\ntest ! -f bug.txt\n");
        fx.git(&["add", "-A"]);
        fx.git(&["commit", "-qm", "c1"]);
        let good = testutil::git(&fx.path, &["rev-parse", "HEAD"]);
        fx.commit_file("bug.txt", "bug\n", "c2 introduce bug");
        let culprit = testutil::git(&fx.path, &["rev-parse", "HEAD"]);
        backend.open(fx.id()).unwrap();

        async fn run(backend: &Backend, fx: &testutil::Fixture, op: Op) -> OpResult {
            let plan = backend.plan(fx.id(), &op).unwrap();
            backend.run_op(fx.id(), &plan).await.unwrap()
        }

        run(
            &backend,
            &fx,
            Op::BisectStart {
                good: good.clone(),
                bad: culprit.clone(),
                extra: Vec::new(),
            },
        )
        .await;
        assert!(backend.load_bisect(fx.id()).unwrap().active);

        // The `bisect_test` command's body: the test plan and the verdict plan it
        // decides run as one serialized flow. With only one candidate (c2), one
        // round must converge.
        let test_plan = backend
            .plan(
                fx.id(),
                &Op::BisectTest {
                    command: "sh check.sh".into(),
                },
            )
            .unwrap();
        assert_eq!(
            test_plan.display, "sh check.sh",
            "the literal test argv is what runs"
        );
        let (results, entries) = backend.run_bisect_flow(fx.id(), test_plan).await.unwrap();
        assert_eq!(results.len(), 2, "test + verdict in one critical section");
        assert!(!results[0].ok, "the bug commit must fail the test");
        assert!(results[1].ok, "verdict accepted: {}", results[1].stderr);
        assert!(
            entries[1].display.contains("bisect bad"),
            "a failing test must produce `git bisect bad`, got {:?}",
            entries[1].display
        );

        let state = backend.load_bisect(fx.id()).unwrap();
        assert_eq!(
            state.found.as_deref(),
            Some(culprit.as_str()),
            "converged on the culprit"
        );
        assert_eq!(state.remaining, 0);

        // Transparency: both steps are in the activity log with their exact commands.
        let log = backend.log().snapshot(Some(fx.id()));
        assert!(
            log.iter().any(|e| e.display == "sh check.sh"),
            "test step logged: {log:?}"
        );
        assert!(
            log.iter().any(|e| e.display == "git bisect bad"),
            "verdict step logged: {log:?}"
        );
    }

    #[test]
    fn bisect_steps_without_a_session_plan_as_clean_errors() {
        let (backend, _rx) = Backend::new();
        let fx = testutil::Fixture::new("app-no-bisect");
        fx.commit_file("a.txt", "x\n", "c1");
        backend.open(fx.id()).unwrap();
        let err = backend
            .plan(
                fx.id(),
                &Op::BisectTest {
                    command: "sh check.sh".into(),
                },
            )
            .unwrap_err();
        assert!(err.to_string().contains("no bisect"), "got {err}");
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

        let err = backend.load_commit(fx.id(), "zzz").unwrap_err();
        assert!(
            matches!(err, AppError::Git(GitError::NoCommit { .. })),
            "got {err}"
        );
    }

    #[test]
    fn open_non_repository_surfaces_git_error() {
        let (backend, _rx) = Backend::new();
        let dir = std::env::temp_dir().join(format!("gitgotten-app-norepo-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let err = backend.open(dir.to_str().unwrap()).unwrap_err();
        assert!(
            matches!(err, AppError::Git(GitError::NotARepository { .. })),
            "got {err}"
        );
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
        let mut watcher = notify::recommended_watcher(move |res| {
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
            conflicts: Vec::new(),
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
        assert_eq!(
            log.snapshot(Some("repo-b"))[0].display,
            "git commit -m \"b\""
        );
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
        assert_eq!(backend.refresh_status(fx.id()).unwrap().upstream, None);
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
        assert!(
            names.contains(&"main") && names.contains(&"side"),
            "got {names:?}"
        );

        let remotes = backend.load_remotes(fx.id()).unwrap();
        assert_eq!(
            remotes.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(),
            vec!["origin"]
        );

        let tags = backend.load_tags(fx.id()).unwrap();
        assert_eq!(
            tags.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
            vec!["v1"]
        );

        let err = backend.load_branches("/nope").unwrap_err();
        assert!(matches!(err, AppError::RepoNotOpen(_)), "got {err}");
    }

    // ── Stage 7: sync + merge ─────────────────────────────────────────────

    /// A diverged repo: `main` and `side` both edit the same line of
    /// `f.txt` (a guaranteed merge conflict).
    fn diverged_conflict_fixture() -> testutil::Fixture {
        let fx = testutil::Fixture::new("app-sync");
        fx.commit_file("f.txt", "base\n", "c1");
        fx.git(&["checkout", "-b", "side"]);
        fx.commit_file("f.txt", "side\n", "c-side");
        fx.git(&["checkout", "main"]);
        fx.commit_file("f.txt", "main\n", "c-main");
        fx
    }

    /// A conflicted merge surfaces its unmerged files in the `OpResult` and
    /// the activity-log entry — the handoff to the conflict view (stage 8).
    #[tokio::test]
    async fn run_op_reports_conflicted_merge_files() {
        let (backend, _rx) = Backend::new();
        let fx = diverged_conflict_fixture();
        backend.open(fx.id()).unwrap();

        let op = Op::Merge {
            source: "side".into(),
        };
        let plan = backend.plan(fx.id(), &op).unwrap();
        let result = backend.run_op(fx.id(), &plan).await.unwrap();

        assert!(!result.ok, "the merge must stop on the conflict");
        assert_eq!(result.conflicts, vec!["f.txt".to_string()]);
        let entry = backend.last_entry(fx.id()).unwrap();
        assert_eq!(entry.conflicts, vec!["f.txt".to_string()]);
        // The read model agrees: mid-merge.
        assert!(backend.load_status(fx.id()).unwrap().in_merge);
    }

    /// A clean merge (and every other op) reports no conflicts.
    #[tokio::test]
    async fn run_op_clean_merge_has_no_conflicts() {
        let (backend, _rx) = Backend::new();
        let fx = testutil::Fixture::new("app-sync-clean");
        fx.commit_file("a.txt", "base\n", "c1");
        fx.git(&["checkout", "-b", "side"]);
        fx.commit_file("b.txt", "side\n", "c-side");
        fx.git(&["checkout", "main"]);
        fx.commit_file("c.txt", "main\n", "c-main");
        backend.open(fx.id()).unwrap();

        let op = Op::Merge {
            source: "side".into(),
        };
        let plan = backend.plan(fx.id(), &op).unwrap();
        let result = backend.run_op(fx.id(), &plan).await.unwrap();
        assert!(result.ok, "stderr: {}", result.stderr);
        assert!(result.conflicts.is_empty());
        let _ = fx;
    }

    // ── Stage 8: conflict resolution ─────────────────────────────────────

    /// The full path the web uses for conflict resolution: merge (stops on
    /// the conflict) → `load_conflict` (3-way) → ResolveConflict →
    /// ContinueOp — the merge commits with the chosen side.
    #[tokio::test]
    async fn run_op_resolves_and_continues_a_merge() {
        let (backend, _rx) = Backend::new();
        let fx = diverged_conflict_fixture();
        backend.open(fx.id()).unwrap();

        let merge = backend
            .plan(
                fx.id(),
                &Op::Merge {
                    source: "side".into(),
                },
            )
            .unwrap();
        let result = backend.run_op(fx.id(), &merge).await.unwrap();
        assert!(!result.ok, "the merge stops on the conflict");
        assert_eq!(result.conflicts, vec!["f.txt".to_string()]);

        // The 3-way read sees all three sides from the index stages.
        let cf = backend.load_conflict(fx.id(), "f.txt").unwrap();
        assert_eq!(
            (cf.base.as_str(), cf.ours.as_str(), cf.theirs.as_str()),
            ("base\n", "main\n", "side\n")
        );

        let resolve = backend
            .plan(
                fx.id(),
                &Op::ResolveConflict {
                    path: "f.txt".into(),
                    resolution: ConflictResolution::Side(ConflictSide::Ours),
                },
            )
            .unwrap();
        let result = backend.run_op(fx.id(), &resolve).await.unwrap();
        assert!(result.ok, "stderr: {}", result.stderr);
        assert!(
            result.conflicts.is_empty(),
            "resolving clears the unmerged list"
        );

        let cont = backend.plan(fx.id(), &Op::ContinueOp).unwrap();
        assert_eq!(cont.display, "git merge --continue");
        let result = backend.run_op(fx.id(), &cont).await.unwrap();
        assert!(result.ok, "stderr: {}", result.stderr);
        assert!(!backend.load_status(fx.id()).unwrap().in_merge);
        assert_eq!(
            std::fs::read_to_string(fx.path.join("f.txt")).unwrap(),
            "main\n",
            "the merge committed the chosen side"
        );
        let _ = fx;
    }

    /// Aborting a merge through the backend restores the pre-operation state.
    #[tokio::test]
    async fn run_op_aborts_a_merge() {
        let (backend, _rx) = Backend::new();
        let fx = diverged_conflict_fixture();
        backend.open(fx.id()).unwrap();

        let merge = backend
            .plan(
                fx.id(),
                &Op::Merge {
                    source: "side".into(),
                },
            )
            .unwrap();
        let result = backend.run_op(fx.id(), &merge).await.unwrap();
        assert!(!result.ok, "the merge stops on the conflict");

        let abort = backend.plan(fx.id(), &Op::AbortOp).unwrap();
        assert_eq!(abort.display, "git merge --abort");
        let result = backend.run_op(fx.id(), &abort).await.unwrap();
        assert!(result.ok, "stderr: {}", result.stderr);
        assert!(result.conflicts.is_empty());
        assert!(!backend.load_status(fx.id()).unwrap().in_merge);
        assert_eq!(
            std::fs::read_to_string(fx.path.join("f.txt")).unwrap(),
            "main\n",
            "the worktree is back to its pre-merge state"
        );
        let _ = fx;
    }

    /// `load_conflict` on a non-conflicted path surfaces the git engine's
    /// NoConflict error (serialized to its display string for the web).
    #[test]
    fn load_conflict_on_a_clean_path_errors() {
        let (backend, _rx) = Backend::new();
        let fx = testutil::Fixture::new("app-conflict-clean");
        fx.commit_file("a.txt", "one\n", "c1");
        backend.open(fx.id()).unwrap();

        let err = backend.load_conflict(fx.id(), "a.txt").unwrap_err();
        assert!(
            matches!(err, AppError::Git(GitError::NoConflict { .. })),
            "got {err}"
        );
    }

    /// `load_conflict` on an unknown repo id is a RepoNotOpen.
    #[test]
    fn load_conflict_on_unknown_repo_errors() {
        let (backend, _rx) = Backend::new();
        let err = backend
            .load_conflict("/does/not/exist", "f.txt")
            .unwrap_err();
        assert!(matches!(err, AppError::RepoNotOpen(_)), "got {err}");
    }

    #[test]
    fn rebase_commits_lists_the_range_for_an_open_repo() {
        let (backend, _rx) = Backend::new();
        let fx = testutil::Fixture::new("app-rebase-commits");
        fx.commit_file("a.txt", "base\n", "base");
        fx.git(&["checkout", "-b", "feature"]);
        fx.commit_file("b.txt", "one\n", "one");
        fx.commit_file("c.txt", "two\n", "two");
        backend.open(fx.id()).unwrap();
        let commits = backend.rebase_commits(fx.id(), "main").unwrap();
        let subjects: Vec<&str> = commits.iter().map(|c| c.subject.as_str()).collect();
        assert_eq!(subjects, ["two", "one"]);
    }

    #[test]
    fn rebase_commits_on_unknown_repo_errors() {
        let (backend, _rx) = Backend::new();
        let err = backend
            .rebase_commits("/does/not/exist", "main")
            .unwrap_err();
        assert!(matches!(err, AppError::RepoNotOpen(_)), "got {err}");
    }

    // ── Stage 12: workspace persistence ───────────────────────────────────

    fn store_dir(label: &str) -> std::path::PathBuf {
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "gitgotten-ws-{}-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed),
            label
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn open_adds_the_repo_to_the_tree_and_recents() {
        let fx = testutil::Fixture::new("ws-open");
        fx.commit_file("a.txt", "x\n", "c1");
        let (backend, _rx) = Backend::new();
        let meta = backend.open(fx.id()).unwrap();
        let ws = backend.workspace();
        assert!(ws.has_repo(&meta.id));
        assert_eq!(ws.recents, vec![fx.id().to_string()]);
        assert_eq!(ws.repo_ids(), vec![fx.id().to_string()]);
    }

    #[test]
    fn group_ops_nest_rename_and_delete_hoists_repos() {
        let fx = testutil::Fixture::new("ws-group");
        fx.commit_file("a.txt", "x\n", "c1");
        let (backend, _rx) = Backend::new();
        let id = backend.open(fx.id()).unwrap().id;

        let ws = backend.group_add("Work", None).unwrap();
        let gid = ws.groups.iter().find(|g| g.name == "Work").unwrap().id.clone();
        assert_eq!(backend.workspace().parent_of(&gid).as_deref(), Some("root"));

        let mut moved = ws;
        moved
            .groups
            .iter_mut()
            .find(|g| g.id == "root")
            .unwrap()
            .children
            .retain(|c| matches!(c, ChildNode::Group { .. }));
        moved
            .groups
            .iter_mut()
            .find(|g| g.id == gid)
            .unwrap()
            .children
            .push(ChildNode::Repo { id: id.clone() });
        backend.set_workspace(moved).unwrap();
        assert_eq!(backend.workspace().parent_of(&id).as_deref(), Some(gid.as_str()));

        let ws = backend.group_add("Side", Some(&gid)).unwrap();
        let sid = ws.groups.iter().find(|g| g.name == "Side").unwrap().id.clone();
        backend.group_rename(&sid, "Side Projects").unwrap();
        assert_eq!(backend.workspace().group(&sid).unwrap().name, "Side Projects");

        backend.group_delete(&gid).unwrap();
        let ws = backend.workspace();
        assert!(ws.group(&gid).is_none());
        assert!(ws.group(&sid).is_none());
        assert!(ws.has_repo(&id)); // hoisted to root, not unlinked
        assert_eq!(ws.parent_of(&id).as_deref(), Some("root"));
        assert!(backend.group_add("X", Some("g404")).is_err());
        assert!(backend.group_rename("root", "nope").is_err());
    }

    #[test]
    fn workspace_round_trips_through_app_json() {
        let dir = store_dir("roundtrip");
        let fx1 = testutil::Fixture::new("ws-r1");
        fx1.commit_file("a.txt", "x\n", "c1");
        let fx2 = testutil::Fixture::new("ws-r2");
        fx2.commit_file("b.txt", "y\n", "c2");

        let (backend, _rx) = Backend::new();
        backend.set_store(dir.join("app.json"));
        let m1 = backend.open(fx1.id()).unwrap();
        let m2 = backend.open(fx2.id()).unwrap();
        let ws = backend.group_add("Work", None).unwrap();
        let gid = ws.groups.iter().find(|g| g.name == "Work").unwrap().id.clone();
        let mut moved = ws;
        moved
            .groups
            .iter_mut()
            .find(|g| g.id == "root")
            .unwrap()
            .children
            .retain(|c| !matches!(c, ChildNode::Repo { id } if id == &m1.id));
        moved
            .groups
            .iter_mut()
            .find(|g| g.id == gid)
            .unwrap()
            .children
            .push(ChildNode::Repo { id: m1.id.clone() });
        backend.set_workspace(moved).unwrap();
        backend.repo_pin(&m2.id, true).unwrap();
        backend.repo_touch(&m1.id);
        let saved = backend.workspace();

        let text = std::fs::read_to_string(dir.join("app.json")).unwrap();
        assert!(text.contains("\"kind\": \"group\"")); // the tree really hit disk

        let (restarted, _rx2) = Backend::new();
        restarted.set_store(dir.join("app.json"));
        assert_eq!(restarted.workspace(), saved);
        assert_eq!(restarted.workspace().pinned, vec![m2.id.clone()]);
        assert_eq!(
            restarted.workspace().recents.first().map(String::as_str),
            Some(m1.id.as_str())
        );

        restarted.reopen_workspace_repos();
        let pairs = restarted.workspace_repos();
        assert_eq!(pairs.len(), 2);
        assert!(pairs.iter().all(|p| p.status.branch.as_deref() == Some("main")));
    }

    #[test]
    fn corrupt_app_json_rebuilds_the_default_tree() {
        let dir = store_dir("corrupt");
        std::fs::write(dir.join("app.json"), "{ not json").unwrap();
        let (backend, _rx) = Backend::new();
        backend.set_store(dir.join("app.json"));
        assert_eq!(backend.workspace(), Workspace::new());
        backend.group_add("Fixed", None).unwrap(); // the next persist repairs the file
        let (restarted, _rx2) = Backend::new();
        restarted.set_store(dir.join("app.json"));
        assert!(restarted.workspace().group("g1").is_some());
    }

    #[test]
    fn workspace_set_rejects_invalid_trees_before_persisting() {
        let dir = store_dir("cycle");
        let (backend, _rx) = Backend::new();
        backend.set_store(dir.join("app.json"));
        let ws = backend.group_add("Loop", None).unwrap();
        let gid = ws.groups.iter().find(|g| g.name == "Loop").unwrap().id.clone();
        let before = backend.workspace();
        let mut bad = before.clone();
        bad.groups
            .iter_mut()
            .find(|g| g.id == gid)
            .unwrap()
            .children
            .push(ChildNode::Group { id: "root".into() });
        assert!(backend.set_workspace(bad).is_err());
        assert_eq!(backend.workspace(), before);
        let persisted: Workspace =
            serde_json::from_str(&std::fs::read_to_string(dir.join("app.json")).unwrap()).unwrap();
        assert_eq!(persisted, before); // only valid trees ever reach the file
    }

    #[test]
    fn reopen_reopens_tree_repos_and_tolerates_vanished_paths() {
        let dir = store_dir("reopen");
        let fx = testutil::Fixture::new("ws-reopen");
        fx.commit_file("a.txt", "x\n", "c1");
        let ws = Workspace {
            groups: vec![Group {
                id: "root".into(),
                name: "Workspace".into(),
                children: vec![
                    ChildNode::Repo { id: fx.id().into() },
                    ChildNode::Repo { id: "/gitgotten/nope".into() },
                ],
            }],
            pinned: Vec::new(),
            recents: Vec::new(),
        };
        std::fs::write(dir.join("app.json"), serde_json::to_string(&ws).unwrap()).unwrap();
        let (backend, _rx) = Backend::new();
        backend.set_store(dir.join("app.json"));
        backend.reopen_workspace_repos();
        assert_eq!(backend.load_status(fx.id()).unwrap().branch.as_deref(), Some("main"));
        assert!(backend.load_status("/gitgotten/nope").is_err());
        assert!(backend.workspace().has_repo("/gitgotten/nope"));
    }

    #[test]
    fn remove_repo_unlinks_without_touching_the_disk() {
        let fx = testutil::Fixture::new("ws-remove");
        fx.commit_file("a.txt", "x\n", "c1");
        let dir = store_dir("remove");
        let (backend, _rx) = Backend::new();
        backend.set_store(dir.join("app.json"));
        let id = backend.open(fx.id()).unwrap().id;
        backend.repo_pin(&id, true).unwrap();
        let ws = backend.remove_repo_from_workspace(&id).unwrap();
        assert!(!ws.has_repo(&id));
        assert!(ws.pinned.is_empty());
        assert!(backend.load_status(&id).is_err()); // unregistered
        assert_eq!(fx.git(&["rev-parse", "HEAD"]).trim().len(), 40); // repo untouched
        let (restarted, _rx2) = Backend::new();
        restarted.set_store(dir.join("app.json"));
        assert!(!restarted.workspace().has_repo(&id)); // the unlink persisted
    }

    #[test]
    fn workspace_repos_pairs_meta_with_live_status() {
        let fx = testutil::Fixture::new("ws-pairs");
        fx.commit_file("a.txt", "x\n", "c1");
        fx.write_file("dirty.txt", "d\n");
        let (backend, _rx) = Backend::new();
        let id = backend.open(fx.id()).unwrap().id;
        let pairs = backend.workspace_repos();
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].meta.id, id);
        assert_eq!(pairs[0].status.changes.len(), 1);
        assert!(pairs[0].status.last_commit.is_some());
    }

    // ── Stage 13: per-repo state — activity log persistence + recovery ──────

    /// SPEC §11: the activity log is *per-repository* state — it survives a restart,
    /// and one busy repo cannot evict another repo's command stream.
    #[tokio::test]
    async fn activity_log_is_per_repo_state_and_survives_a_restart() {
        let dir = store_dir("log-restart");
        let fx_a = testutil::Fixture::new("log-a");
        let fx_b = testutil::Fixture::new("log-b");
        let (backend, _rx) = Backend::new();
        backend.set_store(dir.join("app.json"));
        backend.open(fx_a.id()).unwrap();
        backend.open(fx_b.id()).unwrap();
        for i in 0..LOG_CAP + 2 {
            backend
                .run_op(fx_a.id(), &noop_plan(&format!("git echo a{i}")))
                .await
                .unwrap();
        }
        backend
            .run_op(fx_b.id(), &noop_plan("git echo b1"))
            .await
            .unwrap();
        let highest = backend.log().snapshot(None).last().unwrap().id;

        // A second launch over the same store sees both streams, each capped on its
        // own: repo A kept its newest 500 (the two oldest are gone), B kept its one.
        let (restarted, _rx2) = Backend::new();
        restarted.set_store(dir.join("app.json"));
        let a = restarted.log().snapshot(Some(fx_a.id()));
        assert_eq!(a.len(), LOG_CAP);
        assert_eq!(a.first().unwrap().display, "git echo a2");
        let b = restarted.log().snapshot(Some(fx_b.id()));
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].display, "git echo b1");

        // Ids stay monotonic across the restart: the next entry continues after the
        // highest id on disk, so the web's log keys never collide with restored rows.
        restarted.open(fx_a.id()).unwrap();
        restarted
            .run_op(fx_a.id(), &noop_plan("git echo a-next"))
            .await
            .unwrap();
        assert_eq!(restarted.log().last(fx_a.id()).unwrap().id, highest + 1);
        assert_eq!(restarted.log().snapshot(Some(fx_a.id())).len(), LOG_CAP);
    }

    /// A malformed per-repo state file is rebuilt, never a crash — with every flavour
    /// of damage a half-written cache file can be (SPEC §11/§18).
    #[test]
    fn corrupt_per_repo_log_files_are_rebuilt() {
        let dir = store_dir("log-corrupt");
        std::fs::write(log_file(&dir, "repo-a"), "{ not json").unwrap();
        std::fs::write(log_file(&dir, "repo-b"), "[{\"id\": 1}]").unwrap(); // JSON, wrong shape
        std::fs::write(log_file(&dir, "repo-c"), "").unwrap();
        std::fs::write(log_file(&dir, "repo-d"), "[]").unwrap();
        std::fs::write(dir.join("app.json"), "not a log file").unwrap(); // ignored by the loader

        let log = LogBuffer::new();
        log.set_dir(dir.clone()); // must not panic
        assert!(log.snapshot(None).is_empty(), "corrupt files rebuild to empty");

        log.record("repo-a", &result_with(&noop_plan("git status"), false));
        assert_eq!(log.snapshot(Some("repo-a")).len(), 1);
        let repaired: Vec<LogEntry> =
            serde_json::from_str(&std::fs::read_to_string(log_file(&dir, "repo-a")).unwrap())
                .unwrap();
        assert_eq!(repaired.len(), 1);
        assert!(!repaired[0].ok);
    }

    /// SPEC §18's verification step, end to end: corrupt a repository's state file,
    /// restart, and the app recovers by rebuilding it.
    #[tokio::test]
    async fn corrupt_repo_state_file_recovers_through_the_backend() {
        let dir = store_dir("log-recover");
        let fx = testutil::Fixture::new("log-recover");
        let (backend, _rx) = Backend::new();
        backend.set_store(dir.join("app.json"));
        backend.open(fx.id()).unwrap();
        backend
            .run_op(fx.id(), &noop_plan("git commit -m \"first\""))
            .await
            .unwrap();

        // Damage it exactly like a crash mid-write would.
        std::fs::write(log_file(&dir, fx.id()), "[{\"id\": 1, \"repo\":").unwrap();

        let (restarted, _rx2) = Backend::new();
        restarted.set_store(dir.join("app.json"));
        restarted.open(fx.id()).unwrap();
        assert!(restarted.log().snapshot(Some(fx.id())).is_empty());
        restarted
            .run_op(fx.id(), &noop_plan("git commit -m \"second\""))
            .await
            .unwrap();

        let (again, _rx3) = Backend::new();
        again.set_store(dir.join("app.json"));
        let entries = again.log().snapshot(Some(fx.id()));
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].display, "git commit -m \"second\"");
    }

    /// The "last 500" ring is per repo (SPEC §11): a busy repository must not push
    /// another repository's history out of the log.
    #[test]
    fn log_cap_is_per_repo_not_global() {
        let log = LogBuffer::new();
        let busy = result_with(&noop_plan("git a"), true);
        let quiet = result_with(&noop_plan("git b"), true);
        for _ in 0..LOG_CAP + 2 {
            log.record("busy-repo", &busy);
        }
        log.record("quiet-repo", &quiet);
        assert_eq!(log.snapshot(Some("busy-repo")).len(), LOG_CAP);
        assert_eq!(log.snapshot(Some("quiet-repo")).len(), 1);
    }

    /// SPEC §18 stress: every mutating op the app can fire at one repository runs
    /// **serialized**. N ops launched at once must all succeed (no `index.lock` /
    /// "cannot lock ref" leaking into a result), all reach the activity log, and the
    /// repository must end in exactly the state all N of them describe.
    #[tokio::test]
    async fn concurrent_ops_on_one_repo_all_land() {
        let fx = testutil::Fixture::new("app-concurrent");
        fx.commit_file("seed.txt", "seed\n", "seed");
        let tip = testutil::git(&fx.path, &["rev-parse", "HEAD"]);
        let (backend, _rx) = Backend::new();
        let id = backend.open(fx.id()).unwrap().id;
        let backend = std::sync::Arc::new(backend);

        const N: usize = 24;
        let mut handles = Vec::new();
        for i in 0..N {
            let backend = backend.clone();
            let id = id.clone();
            let tip = tip.clone();
            handles.push(tokio::spawn(async move {
                let op = if i % 2 == 0 {
                    Op::BranchCreate {
                        name: format!("br{i}"),
                        from: tip,
                        checkout: false,
                    }
                } else {
                    Op::TagCreate {
                        name: format!("t{i}"),
                        target: tip,
                        annotated: false,
                        message: None,
                    }
                };
                let plan = backend.plan(&id, &op).unwrap();
                backend.run_op(&id, &plan).await.unwrap()
            }));
        }
        // The tasks are already racing each other; awaiting in order only collects.
        let mut results = Vec::with_capacity(N);
        for handle in handles {
            results.push(handle.await.unwrap());
        }

        let failed: Vec<&OpResult> = results.iter().filter(|r| !r.ok).collect();
        assert!(failed.is_empty(), "every serialized op must succeed: {failed:?}");
        let log = backend.log().snapshot(Some(&id));
        assert_eq!(log.len(), N, "every op is in the activity log");
        assert!(log.iter().all(|e| e.ok), "no op reported a lock failure");

        let branches = testutil::git(&fx.path, &["for-each-ref", "--format=%(refname:short)", "refs/heads"]);
        let tags = testutil::git(&fx.path, &["for-each-ref", "--format=%(refname:short)", "refs/tags"]);
        assert_eq!(branches.lines().filter(|l| l.starts_with("br")).count(), N / 2);
        assert_eq!(tags.lines().filter(|l| l.starts_with('t')).count(), N - N / 2);
    }
}
