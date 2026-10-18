//! GitGotten git engine (the "heart" — SPEC §8).
//!
//! Hybrid design: `gix` (gitoxide) for the read model, the real `git` CLI for writes.
//! Every mutating operation compiles to a visible `CommandPlan` (the transparency
//! primitive, SPEC §8/§9) and is run by a serialized per-repo executor.
//!
//! Stage 1 implements the read model: [`Repo::open`] (validate + canonicalize a
//! repository) and [`Repo::status`] (map the `gix` status iterator to the shared
//! [`Status`] shape: ahead/behind vs the upstream ref, merge/rebase state detection,
//! and per-file change kinds with binary sniffing).
//!
//! Stage 2 implements the command layer: [`Repo::plan`] (compile an [`Op`] into a
//! visible [`CommandPlan`]) and the executor ([`GitExecutor`] runs plans through the
//! real git binary, serialized per repo by [`OpQueue`]).

use std::collections::HashSet;
use std::io::Read;
use std::path::{Path, PathBuf};

use gix::hash::ObjectId;

mod exec;
mod plan;

/// Shared read-model types, re-exported so `gitcore::Status` etc. read naturally.
pub use git_gotten_protocol::{
    ChangeKind, CommandPlan, FileChange, GitStep, Op, OpResult, Preview, RepoMeta, RepoId, Risk,
    Status,
};
pub use exec::{GitExecutor, OpQueue};

/// Errors surfaced by the git engine.
///
/// Displayed text is part of the transparency contract: the exact command and the
/// reason it failed are always available to the UI (SPEC §18).
#[derive(Debug, thiserror::Error)]
pub enum GitError {
    /// The path does not exist or is not a git repository.
    #[error("not a git repository: {path}")]
    NotARepository { path: PathBuf },

    /// A bare repository has no working tree, so a status-based UI cannot use it.
    #[error("not a working-tree repository: {path}")]
    BareRepository { path: PathBuf },

    /// A `gix` (gitoxide) read-model failure (object database, status, …).
    #[error("git error: {0}")]
    Gix(#[from] gix::Error),

    /// A git subprocess exited with a non-zero status.
    #[error("git command failed (exit {exit}): {detail}")]
    Command { exit: i32, detail: String },

    /// A low-level I/O failure while touching the working tree or the index.
    #[error("git i/o error: {0}")]
    Io(#[from] std::io::Error),

    /// No working `git` binary was found (PATH search + `git --version` failed).
    #[error("git binary not found on PATH")]
    GitNotFound,

    /// A commit was requested with an empty (or blank) message.
    #[error("commit message is empty")]
    EmptyCommitMessage,

    /// A switch/checkout was requested with an empty target.
    #[error("switch target is empty")]
    EmptyTarget,

    /// A push was requested while HEAD is detached (there is no branch to push).
    #[error("cannot push: no branch checked out (detached HEAD)")]
    DetachedHead,

    /// The named remote does not exist in the repository.
    #[error("unknown remote: {name}")]
    UnknownRemote { name: String },
}

/// Serialize an error for the web shell as its (transparency-contract) display string.
impl serde::Serialize for GitError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_string())
    }
}

/// An open repository with a working tree.
///
/// `id` and `path` are the canonical absolute path of the *worktree root*: reopening
/// the same folder yields the same id, and a `git worktree add` checkout has its own
/// id (SPEC §10.7).
#[derive(Debug)]
pub struct Repo {
    /// Canonical absolute path of the worktree root (the stable repository identity).
    pub id: RepoId,
    /// Canonical absolute path of the worktree root.
    pub path: PathBuf,
    inner: gix::Repository,
}

impl Repo {
    /// Open the repository containing `path`.
    ///
    /// `path` may be any path *inside* a repository — the worktree root is resolved.
    /// Fails with [`GitError::NotARepository`] when `path` is missing or not inside a
    /// repository, and [`GitError::BareRepository`] when the repository has no
    /// working tree.
    pub fn open(path: &Path) -> Result<Self, GitError> {
        let path = path.canonicalize().map_err(|_| GitError::NotARepository {
            path: path.to_path_buf(),
        })?;
        let inner = gix::open(&path).map_err(|_| GitError::NotARepository {
            path: path.clone(),
        })?;
        if inner.is_bare() {
            return Err(GitError::BareRepository { path: path.clone() });
        }
        // The identity is the canonical worktree root (what the user sees and opens).
        let workdir = inner
            .workdir()
            .ok_or_else(|| GitError::BareRepository { path: path.clone() })?;
        let path = workdir.canonicalize().map_err(|_| GitError::NotARepository {
            path: workdir.to_path_buf(),
        })?;
        Ok(Repo {
            id: path.to_string_lossy().into_owned(),
            path,
            inner,
        })
    }

    /// The workspace-nav summary for this repository (folds [`Self::status`]).
    pub fn meta(&self) -> Result<RepoMeta, GitError> {
        let status = self.status()?;
        Ok(RepoMeta {
            id: self.id.clone(),
            name: self
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| self.id.clone()),
            path: self.path.to_string_lossy().into_owned(),
            branch: status.branch,
            ahead: status.ahead,
            behind: status.behind,
            dirty_count: status.changes.len() as u32,
        })
    }

    /// Compute the live status of this repository.
    pub fn status(&self) -> Result<Status, GitError> {
        compute_status(&self.inner)
    }
}

/// The upstream remote branch and the object it currently points at.
struct Upstream {
    /// Display name, e.g. `origin/main`.
    name: String,
    /// The commit the upstream ref resolves to.
    oid: ObjectId,
}

/// Resolve `branch.<name>.remote` / `branch.<name>.merge` in the config to the
/// remote-tracking ref (e.g. `refs/remotes/origin/main`) and its object.
/// `None` when the branch has no upstream or the remote-tracking ref is absent.
fn upstream_of(repo: &gix::Repository, branch: &str) -> Option<Upstream> {
    let config = repo.config_snapshot();
    let remote = config
        .try_value::<String>(&format!("branch.{branch}.remote"))
        .ok()
        .flatten()?;
    let merge = config
        .try_value::<String>(&format!("branch.{branch}.merge"))
        .ok()
        .flatten()?;
    let merge = merge.strip_prefix("refs/heads/").map(String::from).unwrap_or(merge);
    let name = format!("{remote}/{merge}");
    let refname = format!("refs/remotes/{name}");
    let oid = match repo.find_reference(&refname).ok()?.target() {
        gix::refs::TargetRef::Object(oid) => Some(oid.to_owned()),
        _ => None,
    }?;
    Some(Upstream { name, oid })
}

/// Count the commits reachable from each tip and set-difference them.
fn ahead_behind(
    repo: &gix::Repository,
    head: ObjectId,
    upstream: ObjectId,
) -> Result<(u32, u32), GitError> {
    let reachable = |tip: ObjectId| -> Result<HashSet<ObjectId>, GitError> {
        let mut set = HashSet::new();
        for commit in repo.rev_walk([tip]).all()? {
            set.insert(commit?.id);
        }
        Ok(set)
    };

    let from_head = reachable(head)?;
    let from_upstream = reachable(upstream)?;
    let ahead = from_head
        .iter()
        .filter(|id| !from_upstream.contains(*id))
        .count() as u32;
    let behind = from_upstream
        .iter()
        .filter(|id| !from_head.contains(*id))
        .count() as u32;
    Ok((ahead, behind))
}

/// Convert a gix byte-string path (always `/`-separated) to a display `String`.
fn path_string(b: &gix::bstr::BStr) -> String {
    String::from_utf8_lossy(b).into_owned()
}

/// The object id of the index entry backing an `IndexWorktree` modification, if any.
fn worktree_entry_blob(item: &gix::status::index_worktree::Item) -> Option<ObjectId> {
    match item {
        gix::status::index_worktree::Item::Modification { entry, .. } => Some(entry.id),
        _ => None,
    }
}

/// A file is binary when its content contains a NUL byte in the first 8 KiB (git's
/// own heuristic). Prefer the worktree file; fall back to the recorded blob for
/// files that no longer exist on disk (e.g. staged deletions).
fn sniff_binary(
    repo: &gix::Repository,
    worktree_file: Option<&Path>,
    blob: Option<ObjectId>,
) -> bool {
    const LIMIT: usize = 8 * 1024;

    if let Some(path) = worktree_file && path.is_file()
        && let Ok(file) = std::fs::File::open(path) {
            let mut buf = Vec::with_capacity(LIMIT + 1);
            let _ = file.take(LIMIT as u64).read_to_end(&mut buf);
            return buf.contains(&0);
        }

    if let Some(oid) = blob
        && let Ok(object) = repo.find_object(oid)
            && object.kind == gix::objs::Kind::Blob {
                return object.data.iter().take(LIMIT).any(|b| *b == 0);
            }

    false
}

/// Append one [`FileChange`], computing its binary flag.
fn push_change(
    changes: &mut Vec<FileChange>,
    kind: ChangeKind,
    path: String,
    staged: bool,
    repo: &gix::Repository,
    workdir: Option<&Path>,
    blob: Option<ObjectId>,
) {
    let worktree_file = workdir.map(|wd| wd.join(&path));
    let binary = sniff_binary(repo, worktree_file.as_deref(), blob);
    changes.push(FileChange { path, kind, staged, binary });
}

/// Map the `gix` repository state to the shared [`Status`] model.
fn compute_status(repo: &gix::Repository) -> Result<Status, GitError> {
    let head = repo.head()?;
    let head_id = head.id().map(|id| id.detach());

    // Branch vs detached HEAD (a symbolic ref to a non-branch counts as detached).
    let (branch, detached_head) = if head.is_detached() {
        (None, head_id.map(|oid| oid.to_string()))
    } else {
        match head
            .referent_name()
            .map(|r| r.to_string())
            .and_then(|r| r.strip_prefix("refs/heads/").map(String::from))
        {
            Some(name) => (Some(name), None),
            None => (None, head_id.map(|oid| oid.to_string())),
        }
    };

    let upstream = branch.as_deref().and_then(|b| upstream_of(repo, b));

    let (ahead, behind) = match (&head_id, upstream.as_ref()) {
        (Some(head), Some(up)) if *head != up.oid => ahead_behind(repo, *head, up.oid)?,
        _ => (0, 0),
    };

    let mut platform = repo
        .status(gix::progress::Discard)?
        .untracked_files(gix::status::UntrackedFiles::Files)
        .tree_index_track_renames(gix::status::tree_index::TrackRenames::AsConfigured);
    if head_id.is_none() {
        // Unborn HEAD (fresh `git init`): diff the index against the empty tree.
        platform =
            platform.head_tree(ObjectId::empty_tree(repo.object_hash()));
    }

    let workdir = repo.workdir();
    let mut changes = Vec::new();
    for item in platform.into_iter(Vec::new())? {
        let item = item?;
        match item {
            // HEAD-tree vs index: staged changes.
            gix::status::Item::TreeIndex(change) => {
                let (kind, path, blob) = match &change {
                    gix::diff::index::Change::Addition { location, id, .. } => (
                        ChangeKind::Added,
                        path_string(location),
                        Some(ObjectId::from(&**id)),
                    ),
                    gix::diff::index::Change::Deletion { location, id, .. } => (
                        ChangeKind::Deleted,
                        path_string(location),
                        Some(ObjectId::from(&**id)),
                    ),
                    gix::diff::index::Change::Modification { location, id, .. } => (
                        ChangeKind::Modified,
                        path_string(location),
                        Some(ObjectId::from(&**id)),
                    ),
                    gix::diff::index::Change::Rewrite { location, id, copy, .. } => (
                        if *copy {
                            ChangeKind::Modified
                        } else {
                            ChangeKind::Renamed
                        },
                        path_string(location),
                        Some(ObjectId::from(&**id)),
                    ),
                };
                push_change(&mut changes, kind, path, true, repo, workdir, blob);
            }
            // Index vs worktree: unstaged changes (plus untracked files and conflicts).
            gix::status::Item::IndexWorktree(item) => {
                let path = path_string(item.rela_path());
                let Some(summary) = item.summary() else {
                    continue; // directory bookkeeping, not a user-visible change
                };
                let (kind, staged, blob) = match summary {
                    gix::status::index_worktree::iter::Summary::Added
                    | gix::status::index_worktree::iter::Summary::IntentToAdd => {
                        (ChangeKind::Untracked, false, None)
                    }
                    gix::status::index_worktree::iter::Summary::Removed => {
                        (ChangeKind::Deleted, false, None)
                    }
                    gix::status::index_worktree::iter::Summary::Modified
                    | gix::status::index_worktree::iter::Summary::TypeChange => {
                        (ChangeKind::Modified, false, worktree_entry_blob(&item))
                    }
                    gix::status::index_worktree::iter::Summary::Renamed => {
                        (ChangeKind::Renamed, false, None)
                    }
                    gix::status::index_worktree::iter::Summary::Copied => {
                        (ChangeKind::Modified, false, None)
                    }
                    gix::status::index_worktree::iter::Summary::Conflict => {
                        (ChangeKind::Conflicted, true, worktree_entry_blob(&item))
                    }
                };
                push_change(&mut changes, kind, path, staged, repo, workdir, blob);
            }
        }
    }

    // Deterministic order: by path, staged before unstaged within a path.
    changes.sort_by(|a, b| a.path.cmp(&b.path).then(b.staged.cmp(&a.staged)));

    // In-progress operation state lives in the git directory (per worktree).
    let git_dir = repo.path();
    let in_merge = git_dir.join("MERGE_HEAD").exists();
    let in_rebase = git_dir.join("rebase-merge").is_dir() || git_dir.join("rebase-apply").is_dir();

    Ok(Status {
        branch,
        detached_head,
        ahead,
        behind,
        upstream: upstream.map(|up| up.name),
        changes,
        in_merge,
        in_rebase,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use git_gotten_testutil as testutil;

    // ── Error rendering ──────────────────────────────────────────────────────

    #[test]
    fn not_a_repository_renders_the_path() {
        let e = GitError::NotARepository { path: "/nope".into() };
        assert_eq!(e.to_string(), "not a git repository: /nope");
    }

    #[test]
    fn bare_repository_renders_the_path() {
        let e = GitError::BareRepository { path: "/nope".into() };
        assert_eq!(e.to_string(), "not a working-tree repository: /nope");
    }

    #[test]
    fn command_error_renders_exit_and_detail() {
        let e = GitError::Command {
            exit: 1,
            detail: "nothing to commit".into(),
        };
        assert_eq!(e.to_string(), "git command failed (exit 1): nothing to commit");
    }

    #[test]
    fn io_error_converts_via_from() {
        let io = std::io::Error::new(std::io::ErrorKind::NotFound, "missing");
        let e: GitError = io.into();
        assert!(e.to_string().contains("git i/o error"));
    }

    #[test]
    fn error_serializes_as_its_display_string() {
        let e = GitError::NotARepository { path: "/nope".into() };
        let json = serde_json::to_string(&e).unwrap();
        assert_eq!(json, "\"not a git repository: /nope\"");
    }

    // ── open ─────────────────────────────────────────────────────────────────

    #[test]
    fn open_resolves_the_worktree_root() {
        let fx = testutil::status_fixture();
        let repo = Repo::open(&fx.path).unwrap();
        assert_eq!(repo.id, fx.id());
        assert_eq!(repo.path, fx.path);
    }

    #[test]
    fn open_rejects_a_non_repository() {
        let seq = std::process::id();
        let dir = std::env::temp_dir().join(format!("gitgotten-norepo-{seq}-{}", {
            static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        }));
        std::fs::create_dir_all(&dir).unwrap();
        let err = Repo::open(&dir).unwrap_err();
        assert!(
            matches!(err, GitError::NotARepository { .. }),
            "expected NotARepository, got {err:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn open_rejects_a_bare_repository() {
        let mut fx = testutil::Fixture::new("bare-check");
        fx.commit_file("a.txt", "x\n", "c1");
        let remote = fx.add_origin("main");
        let err = Repo::open(&remote).unwrap_err();
        assert!(matches!(err, GitError::BareRepository { .. }), "got {err:?}");
    }

    // ── status ───────────────────────────────────────────────────────────────

    #[test]
    fn status_maps_the_stage_one_fixture() {
        let fx = testutil::status_fixture();
        let repo = Repo::open(&fx.path).unwrap();
        let st = repo.status().unwrap();

        assert_eq!(st.branch.as_deref(), Some("main"));
        assert!(st.detached_head.is_none());
        assert_eq!(st.upstream.as_deref(), Some("origin/main"));
        assert_eq!((st.ahead, st.behind), (1, 0));
        assert!(!st.in_merge);
        assert!(!st.in_rebase);

        // Exactly the three changes, in sorted path order, with the right kinds.
        let mapped: Vec<_> = st
            .changes
            .iter()
            .map(|c| (c.path.as_str(), c.kind, c.staged))
            .collect();
        assert_eq!(
            mapped,
            vec![
                ("b.txt", ChangeKind::Modified, false),
                ("c.txt", ChangeKind::Added, true),
                ("d.txt", ChangeKind::Untracked, false),
            ]
        );
        assert!(st.changes.iter().all(|c| !c.binary));
    }

    #[test]
    fn status_reports_behind_upstream() {
        let fx = testutil::behind_fixture();
        let repo = Repo::open(&fx.path).unwrap();
        let st = repo.status().unwrap();
        assert_eq!(st.upstream.as_deref(), Some("origin/main"));
        assert_eq!((st.ahead, st.behind), (0, 1));
        assert!(st.changes.is_empty());
    }

    #[test]
    fn status_reports_a_clean_repo() {
        let mut fx = testutil::Fixture::new("clean");
        fx.commit_file("a.txt", "x\n", "c1");
        fx.add_origin("main");
        let repo = Repo::open(&fx.path).unwrap();
        let st = repo.status().unwrap();
        assert_eq!(st.branch.as_deref(), Some("main"));
        assert_eq!((st.ahead, st.behind), (0, 0));
        assert!(st.changes.is_empty());
    }

    #[test]
    fn status_reports_detached_head() {
        let fx = testutil::detached_fixture();
        let repo = Repo::open(&fx.path).unwrap();
        let st = repo.status().unwrap();
        assert!(st.branch.is_none());
        let hex = st.detached_head.as_deref().unwrap();
        assert_eq!(hex.len(), 40, "full oid, got {hex}");
        assert!(st.upstream.is_none());
        assert_eq!((st.ahead, st.behind), (0, 0));
        assert!(st.changes.is_empty());
    }

    #[test]
    fn status_reports_mid_merge_conflict() {
        let fx = testutil::merge_conflict_fixture();
        let repo = Repo::open(&fx.path).unwrap();
        let st = repo.status().unwrap();
        assert!(st.in_merge);
        assert!(!st.in_rebase);
        let conflicted: Vec<_> = st
            .changes
            .iter()
            .filter(|c| c.kind == ChangeKind::Conflicted)
            .collect();
        assert_eq!(conflicted.len(), 1);
        assert_eq!(conflicted[0].path, "f.txt");
    }

    #[test]
    fn status_reports_mid_rebase() {
        let fx = testutil::rebase_conflict_fixture();
        let repo = Repo::open(&fx.path).unwrap();
        let st = repo.status().unwrap();
        assert!(st.in_rebase);
        assert!(!st.in_merge);
        assert!(
            st.changes
                .iter()
                .any(|c| c.kind == ChangeKind::Conflicted && c.path == "f.txt")
        );
    }

    #[test]
    fn status_reports_a_staged_rename() {
        let fx = testutil::Fixture::new("rename");
        fx.commit_file("old.txt", "content\n", "c1");
        fx.git(&["mv", "old.txt", "new.txt"]);
        let repo = Repo::open(&fx.path).unwrap();
        let st = repo.status().unwrap();
        assert_eq!(st.changes.len(), 1);
        assert_eq!(st.changes[0].kind, ChangeKind::Renamed);
        assert_eq!(st.changes[0].path, "new.txt");
        assert!(st.changes[0].staged);
    }

    #[test]
    fn status_sniffs_binary_content() {
        let fx = testutil::Fixture::new("binary");
        fx.commit_file("text.txt", "hello\n", "c1");
        let bin = fx.path.join("blob.bin");
        std::fs::write(&bin, [0u8, 1, 2, 0, 255]).unwrap();
        fx.write_file("staged.txt", "plain text\n");
        fx.add("staged.txt");

        let repo = Repo::open(&fx.path).unwrap();
        let st = repo.status().unwrap();
        let bin_change = st.changes.iter().find(|c| c.path == "blob.bin").unwrap();
        let staged_change = st.changes.iter().find(|c| c.path == "staged.txt").unwrap();
        assert!(bin_change.binary, "NUL bytes must mark the file binary");
        assert!(!staged_change.binary, "plain text must not be binary");
    }

    // ── meta ─────────────────────────────────────────────────────────────────

    #[test]
    fn meta_folds_the_status() {
        let fx = testutil::status_fixture();
        let repo = Repo::open(&fx.path).unwrap();
        let meta = repo.meta().unwrap();
        assert_eq!(meta.id, fx.id());
        assert_eq!(meta.name, fx.path.file_name().unwrap().to_str().unwrap());
        assert_eq!(meta.branch.as_deref(), Some("main"));
        assert_eq!((meta.ahead, meta.behind), (1, 0));
        assert_eq!(meta.dirty_count, 3);
    }
}
