//! The GitGotten Tauri application — a thin shell over `gitcore` and `graph`.
//!
//! Stage 0 proved the Rust → web bridge with `build_info`. Stage 1 opens the git
//! read model: the `open_repo` / `load_status` commands, an in-memory
//! [`RepoRegistry`] of open [`gitcore::Repo`]s, and a `notify` file-watcher that
//! re-derives status (debounced) and emits `status_changed` so the left nav and
//! top bar track a real repository live.

use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use git_gotten_protocol::{RepoId, RepoMeta, Status, StatusChanged};
use gitcore::GitError;
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

/// The application backend: the open-repo registry plus the file watcher that keeps
/// the UI live. Managed by Tauri (`Send + Sync`) and reached from commands.
pub struct Backend {
    registry: RepoRegistry,
    /// The `notify` watcher (must be kept alive for events to arrive).
    watcher: Mutex<Option<RecommendedWatcher>>,
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

/// The `status_changed` event name (Rust → web, SPEC §13).
pub const STATUS_CHANGED: &str = "status_changed";

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
        .invoke_handler(tauri::generate_handler![build_info, open_repo, load_status])
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
}
