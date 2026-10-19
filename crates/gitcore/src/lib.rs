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
mod show;

/// Shared read-model types, re-exported so `gitcore::Status` etc. read naturally.
pub use git_gotten_protocol::{
    ChangeKind, CommandPlan, CommitDetail, FileChange, FileDiff, FileStat, GitStep, Graph, Op,
    OpResult, Preview, RefLabel, RepoMeta, RepoId, Risk, Status,
};
pub use exec::{GitExecutor, OpQueue};
pub use show::{numstat, patch};

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

    /// A `gix` object read failed (object database, refs, …).
    #[error("git object read failed: {0}")]
    Object(String),

    /// The requested object does not exist (or is not a commit).
    #[error("no such commit: {hash}")]
    NoCommit { hash: String },

    /// The repository has no commits yet, so an op that needs a history cannot run.
    #[error("the repository has no commits yet")]
    NoCommits,

    /// A requested branch name is empty or invalid.
    #[error("invalid branch name: {name}")]
    BadBranchName { name: String },

    /// No commits were selected for an op (e.g. cherry-pick).
    #[error("no commits selected")]
    EmptySelection,

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

    /// The `git` binary is not on the PATH, so no operation can be executed.
    #[error("git executable not found")]
    GitNotFound,

    /// Building or laying out the commit graph failed (e.g. an empty repository).
    #[error(transparent)]
    Graph(graph::GraphError),
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

    /// The laid-out commit graph of this repository (IPC `load_graph`, SPEC §10.1):
    /// every commit reachable from the refs (+ HEAD), with lanes, lines, and edges.
    ///
    /// A repository with no commits is [`GitError::Graph`]. The web renders the
    /// result on a canvas; the layout itself lives in the `graph` crate.
    pub fn graph(&self) -> Result<Graph, GitError> {
        let dag = graph::build_dag(&self.inner).map_err(GitError::Graph)?;
        let mut layout = graph::LaneLayout;
        layout.compute(&dag).map_err(GitError::Graph)
    }

    /// Full details of one commit (IPC `load_commit`, the inspect drawer, SPEC §10.1):
    /// the full message, both identities and dates, parents, the refs pointing here,
    /// and — for non-merge commits — the per-file stat and diff from `git show`.
    ///
    /// A missing (or non-commit) object is [`GitError::NoCommit`]. Merge commits
    /// (≥2 parents) have empty `stat`/`diff`: git shows no diff for them.
    pub fn commit_detail(&self, hash: &str) -> Result<CommitDetail, GitError> {
        let commit = self.commit_at(hash)?;
        let oid = commit.id().detach();

        let message = String::from_utf8_lossy(
            commit.message_raw().map_err(|e| GitError::Object(e.to_string()))?,
        )
        .trim_end()
        .to_string();
        let subject = message.lines().next().unwrap_or("").to_string();

        let sig = |s: &gix::actor::SignatureRef| -> (String, String, u64) {
            let name = String::from_utf8_lossy(s.name).into_owned();
            let email = String::from_utf8_lossy(s.email).into_owned();
            let date = s
                .time()
                .map(|t| u64::try_from(t.seconds).unwrap_or(0))
                .unwrap_or(0);
            (name, email, date)
        };
        let author = commit
            .author()
            .map_err(|e| GitError::Object(e.to_string()))?;
        let committer = commit
            .committer()
            .map_err(|e| GitError::Object(e.to_string()))?;
        let (author_name, author_email, author_date) = sig(&author);
        let (committer_name, committer_email, committer_date) = sig(&committer);

        let parents: Vec<git_gotten_protocol::Hash> = commit
            .parent_ids()
            .map(|id| id.to_hex().to_string())
            .collect();

        let is_head = self
            .inner
            .head()
            .ok()
            .and_then(|h| h.id().map(|id| id.detach()))
            == Some(oid);

        let refs = self.refs_at(oid)?;

        // Git shows no diff for merge commits without combined-diff flags.
        let (stat, diff) = if parents.len() < 2 {
            (
                show::numstat(&self.path, hash)?,
                show::patch(&self.path, hash)?,
            )
        } else {
            (Vec::new(), Vec::new())
        };

        Ok(CommitDetail {
            hash: oid.to_hex().to_string(),
            subject,
            message,
            author_name,
            author_email,
            author_date,
            committer_name,
            committer_email,
            committer_date,
            parents,
            refs,
            is_head,
            stat,
            diff,
        })
    }

    /// Find a commit by its (short or full) hex object id.
    ///
    /// Any failure to read the object (missing, not a commit, corrupt) is
    /// [`GitError::NoCommit`] with the hash as given: to the drawer these all
    /// read the same.
    fn commit_at(&self, hash: &str) -> Result<gix::Commit<'_>, GitError> {
        let hash = hash.trim();
        let oid: ObjectId =
            hash.parse().map_err(|_| GitError::NoCommit { hash: hash.to_string() })?;
        self.inner
            .find_commit(oid)
            .map_err(|_| GitError::NoCommit { hash: hash.to_string() })
    }

    /// Every ref pointing at `oid` (branches, remote-tracking, tags — annotated
    /// tags peeled to their commit), with placeholder palette colors: the graph
    /// layout is the color authority, so the web recolors these from the node's
    /// line.
    fn refs_at(&self, oid: ObjectId) -> Result<Vec<RefLabel>, GitError> {
        // The shared label table keeps the drawer and the graph in agreement.
        let mut labels = graph::collect_ref_labels(&self.inner)
            .map_err(GitError::Graph)?
            .get(&oid)
            .cloned()
            .unwrap_or_default();
        // Deterministic drawer order: branches, remotes, tags — each alphabetical.
        let rank = |kind: git_gotten_protocol::RefKind| match kind {
            git_gotten_protocol::RefKind::Branch => 0,
            git_gotten_protocol::RefKind::Remote => 1,
            git_gotten_protocol::RefKind::Tag => 2,
            git_gotten_protocol::RefKind::Worktree => 3,
        };
        labels.sort_by(|a, b| (rank(a.kind), &a.name).cmp(&(rank(b.kind), &b.name)));
        Ok(labels)
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
    use git_gotten_protocol::ResetMode;

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

    // ── Stage 4: graph + commit details + the context-menu ops ────────────

    /// Two branches, a merge, a tag, and a remote:
    ///   main:    c1 (a.txt) → c4 (d.txt) → c5 (merge of feature)   [HEAD]
    ///   feature: c2 (b.txt) → c3 (c.txt)
    ///   tag `v1` on c1; `origin/main` on c4 (pushed before the merge).
    /// Deterministic dates (commit_file_at) keep the ordering reproducible.
    fn graph_fixture() -> (testutil::Fixture, Repo) {
        let mut fx = testutil::Fixture::new("graph");
        fx.commit_file_at("a.txt", "a\n", "c1", 1_000);
        fx.git(&["checkout", "-b", "feature"]);
        fx.commit_file_at("b.txt", "b\n", "c2", 1_100);
        fx.commit_file_at("c.txt", "c\n", "c3", 1_200);
        fx.git(&["checkout", "main"]);
        fx.commit_file_at("d.txt", "d\n", "c4", 1_300);
        fx.add_origin("main"); // origin/main = c4
        let (code, _o, err) =
            fx.git_run(&["merge", "--no-ff", "feature", "-m", "merge feature"]);
        assert_eq!(code, 0, "merge should be clean: {err}");
        fx.git(&["tag", "v1", &fx.git(&["rev-parse", "main~2"])]);
        let repo = Repo::open(&fx.path).unwrap();
        (fx, repo)
    }

    #[test]
    fn graph_lays_out_a_branch_merge_tag_remote() {
        let (fx, repo) = graph_fixture();
        let g = repo.graph().unwrap();

        assert_eq!(g.nodes.len(), 5);
        let c5 = fx.git(&["rev-parse", "HEAD"]);
        assert_eq!(g.nodes[0].hash, c5, "the merge is the newest row");
        assert!(g.nodes[0].is_head);
        assert!(
            g.nodes[0].refs.iter().any(|r| {
                r.name == "main" && r.kind == git_gotten_protocol::RefKind::Branch
            }),
            "the merge carries main: {:?}",
            g.nodes[0].refs
        );
        assert_eq!(g.edges.len(), 5, "5 commits, 4 links + the merge's 2nd parent");

        let c1 = fx.git(&["rev-parse", "main~2"]);
        let n1 = g.nodes.iter().find(|n| n.hash == c1).unwrap();
        assert!(
            n1.refs.iter().any(|r| r.name == "v1" && r.kind == git_gotten_protocol::RefKind::Tag),
            "c1 carries tag v1: {:?}",
            n1.refs
        );
        let c4 = fx.git(&["rev-parse", "main~1"]);
        let n4 = g.nodes.iter().find(|n| n.hash == c4).unwrap();
        assert!(
            n4.refs.iter().any(|r| {
                r.name == "origin/main" && r.kind == git_gotten_protocol::RefKind::Remote
            }),
            "c4 carries origin/main: {:?}",
            n4.refs
        );

        // Two lines: the mainline and the feature branch. The merge absorbs
        // the feature lane - a dense layout allocates no third for it.
        assert_eq!(g.lines.len(), 2, "got {:?} lines", g.lines.len());
        for line in &g.lines {
            assert!(!line.segments.is_empty());
        }
        // Rows are unique and dense.
        let rows: std::collections::HashSet<u32> = g.nodes.iter().map(|n| n.row).collect();
        assert_eq!(rows.len(), 5);
        assert_eq!(rows.iter().copied().min().unwrap(), 0);
    }

    #[test]
    fn graph_of_an_empty_repo_errors() {
        let fx = testutil::Fixture::new("empty-graph");
        let repo = Repo::open(&fx.path).unwrap();
        let err = repo.graph().unwrap_err();
        assert!(
            matches!(err, GitError::Graph(graph::GraphError::Empty)),
            "got {err:?}"
        );
    }

    #[test]
    fn commit_detail_reads_a_regular_commit() {
        let (fx, repo) = graph_fixture();
        let c4 = fx.git(&["rev-parse", "main~1"]);
        let d = repo.commit_detail(&c4).unwrap();

        assert_eq!(d.hash, c4);
        assert_eq!(d.subject, "c4");
        assert_eq!(d.message, "c4");
        assert_eq!(d.author_name, "Test User");
        assert_eq!(d.author_email, "test@example.com");
        assert_eq!((d.author_date, d.committer_date), (1_300, 1_300));
        assert_eq!(d.parents, vec![fx.git(&["rev-parse", "main~2"])]);
        assert!(!d.is_head);

        assert_eq!(d.refs.len(), 1);
        assert_eq!(d.refs[0].name, "origin/main");
        assert_eq!(d.refs[0].kind, git_gotten_protocol::RefKind::Remote);

        assert_eq!(d.stat.len(), 1);
        assert_eq!(d.stat[0].path, "d.txt");
        assert_eq!((d.stat[0].additions, d.stat[0].deletions), (1, 0));
        assert!(!d.stat[0].binary);
        assert_eq!(d.diff.len(), 1);
        assert_eq!(d.diff[0].path, "d.txt");
        assert!(d.diff[0].patch.contains("+d"), "got {:?}", d.diff[0].patch);
    }

    #[test]
    fn commit_detail_reads_a_root_commit() {
        let (fx, repo) = graph_fixture();
        let c1 = fx.git(&["rev-parse", "main~2"]);
        let d = repo.commit_detail(&c1).unwrap();

        assert!(d.parents.is_empty());
        assert!(
            d.refs.iter().any(|r| r.name == "v1" && r.kind == git_gotten_protocol::RefKind::Tag),
            "root carries v1: {:?}",
            d.refs
        );
        // A root commit diffs against the empty tree — it has stat + diff.
        assert_eq!(d.stat.len(), 1);
        assert_eq!(d.stat[0].path, "a.txt");
        assert_eq!(d.diff.len(), 1);
        assert!(d.diff[0].patch.contains("+a"));
    }

    #[test]
    fn commit_detail_of_a_merge_has_no_diff() {
        let (fx, repo) = graph_fixture();
        let c5 = fx.git(&["rev-parse", "HEAD"]);
        let d = repo.commit_detail(&c5).unwrap();

        assert_eq!(d.parents.len(), 2);
        assert!(d.is_head);
        assert!(d.stat.is_empty(), "merges have no default diff");
        assert!(d.diff.is_empty(), "merges have no default diff");
    }

    #[test]
    fn commit_detail_reports_renames_and_binaries() {
        let fx = testutil::Fixture::new("detail-rename-bin");
        fx.commit_file_at("old.txt", "x\n", "c1", 100);
        std::fs::write(fx.path.join("img.bin"), [0u8, 1, 2]).unwrap();
        fx.git(&["mv", "old.txt", "new.txt"]);
        fx.write_file("new.txt", "x\ny\n");
        std::fs::write(fx.path.join("img.bin"), [0u8, 1, 2, 0, 3]).unwrap();
        fx.add("new.txt");
        fx.add("img.bin");
        let (code, _o, err) = fx.git_run(&["commit", "-m", "c2"]);
        assert_eq!(code, 0, "commit should succeed: {err}");
        let c2 = fx.git(&["rev-parse", "HEAD"]);
        let repo = Repo::open(&fx.path).unwrap();

        let d = repo.commit_detail(&c2).unwrap();
        let stat_paths: Vec<&str> = d.stat.iter().map(|s| s.path.as_str()).collect();
        assert!(stat_paths.contains(&"new.txt"), "rename resolves to the new name: {stat_paths:?}");
        assert!(stat_paths.contains(&"img.bin"), "{stat_paths:?}");
        let img = d.stat.iter().find(|s| s.path == "img.bin").unwrap();
        assert!(img.binary);

        let diff_paths: Vec<&str> = d.diff.iter().map(|f| f.path.as_str()).collect();
        assert!(diff_paths.contains(&"new.txt"), "{diff_paths:?}");
        assert!(diff_paths.contains(&"img.bin"), "{diff_paths:?}");
        let img = d.diff.iter().find(|f| f.path == "img.bin").unwrap();
        assert!(img.binary);
        let renamed = d.diff.iter().find(|f| f.path == "new.txt").unwrap();
        assert!(!renamed.binary);
        assert!(renamed.patch.contains("rename from old.txt"), "got {:?}", renamed.patch);
    }

    #[test]
    fn commit_detail_rejects_unknown_and_malformed_hashes() {
        let (_fx, repo) = graph_fixture();
        let missing = "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz";
        assert!(
            matches!(repo.commit_detail(missing).unwrap_err(), GitError::NoCommit { .. }),
            "got {:?}",
            repo.commit_detail(missing).unwrap_err()
        );
        assert!(
            matches!(repo.commit_detail("not-a-hash").unwrap_err(), GitError::NoCommit { .. }),
            "got {:?}",
            repo.commit_detail("not-a-hash").unwrap_err()
        );
    }

    // ── End-to-end through the executor (real git) ────────────────────────

    #[tokio::test]
    async fn run_branch_create_op_end_to_end() {
        let fx = testutil::Fixture::new("exec-branch");
        fx.commit_file("a.txt", "one\n", "c1");
        fx.commit_file("b.txt", "two\n", "c2");
        let c1 = fx.git(&["rev-parse", "HEAD~1"]);
        let repo = Repo::open(&fx.path).unwrap();
        let plan = repo
            .plan(&Op::BranchCreate {
                name: "side".into(),
                from: c1.clone(),
                checkout: true,
            })
            .unwrap();

        let executor = GitExecutor::new();
        let result = executor.run_plan(&repo.id, &repo.path, &plan).await;
        assert!(result.ok, "stderr: {}", result.stderr);
        assert_eq!(fx.git(&["rev-parse", "--abbrev-ref", "HEAD"]), "side");
        assert_eq!(fx.git(&["rev-parse", "side"]), c1);
    }

    #[tokio::test]
    async fn run_reset_mixed_op_end_to_end() {
        let fx = testutil::Fixture::new("exec-reset-mixed");
        fx.commit_file("a.txt", "one\n", "c1");
        fx.commit_file("a.txt", "one\ntwo\n", "c2");
        let c1 = fx.git(&["rev-parse", "HEAD~1"]);
        let repo = Repo::open(&fx.path).unwrap();
        let plan = repo
            .plan(&Op::Reset {
                mode: ResetMode::Mixed,
                target: c1.clone(),
            })
            .unwrap();

        let executor = GitExecutor::new();
        let result = executor.run_plan(&repo.id, &repo.path, &plan).await;
        assert!(result.ok, "stderr: {}", result.stderr);
        assert_eq!(fx.git(&["rev-parse", "main"]), c1);
        // The worktree change survives, unstaged.
        assert!(
            fx.git(&["status", "--porcelain"])
                .lines()
                .any(|l| l == " M a.txt"),
            "porcelain: {}",
            fx.git(&["status", "--porcelain"])
        );
    }

    #[tokio::test]
    async fn run_reset_hard_op_end_to_end() {
        let fx = testutil::Fixture::new("exec-reset-hard");
        fx.commit_file("a.txt", "one\n", "c1");
        fx.commit_file("a.txt", "one\ntwo\n", "c2");
        fx.write_file("a.txt", "one\ntwo\nthree\n"); // uncommitted
        let c1 = fx.git(&["rev-parse", "HEAD~1"]);
        let repo = Repo::open(&fx.path).unwrap();
        let plan = repo
            .plan(&Op::Reset {
                mode: ResetMode::Hard,
                target: c1.clone(),
            })
            .unwrap();

        let executor = GitExecutor::new();
        let result = executor.run_plan(&repo.id, &repo.path, &plan).await;
        assert!(result.ok, "stderr: {}", result.stderr);
        assert_eq!(fx.git(&["rev-parse", "main"]), c1);
        assert_eq!(fx.git(&["status", "--porcelain"]), "");
        assert_eq!(
            std::fs::read_to_string(fx.path.join("a.txt")).unwrap(),
            "one\n"
        );
    }

    #[tokio::test]
    async fn run_cherry_pick_op_end_to_end() {
        let fx = testutil::Fixture::new("exec-cherry");
        fx.commit_file("a.txt", "one\n", "c1");
        fx.git(&["checkout", "-b", "side"]);
        fx.commit_file("b.txt", "two\n", "c2");
        fx.git(&["checkout", "main"]);
        let pick = fx.git(&["rev-parse", "side"]);
        let repo = Repo::open(&fx.path).unwrap();
        let plan = repo
            .plan(&Op::CherryPick {
                hashes: vec![pick],
            })
            .unwrap();

        let executor = GitExecutor::new();
        let result = executor.run_plan(&repo.id, &repo.path, &plan).await;
        assert!(result.ok, "stderr: {}", result.stderr);
        assert_ne!(fx.git(&["rev-parse", "HEAD"]), fx.git(&["rev-parse", "main~1"]));
        assert_eq!(
            std::fs::read_to_string(fx.path.join("b.txt")).unwrap(),
            "two\n"
        );
        assert_eq!(fx.git(&["log", "-1", "--format=%s"]), "c2");
    }
}
