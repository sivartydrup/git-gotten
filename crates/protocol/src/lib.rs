//! Shared Rust <-> TypeScript types for GitGotten.
//!
//! Every type that crosses the IPC boundary lives here and is generated to TypeScript
//! via `ts-rs` (see SPEC §5). Keeping them in one crate means the Rust core and the web
//! UI cannot drift apart on a shared shape.
//!
//! Stage 0 added [`BuildInfo`]; Stage 1 adds the git read-model shapes: [`Status`],
//! [`FileChange`], [`ChangeKind`], [`RepoMeta`], and the [`StatusChanged`] event payload.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Build/runtime identity of the GitGotten binary, surfaced to the web shell.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct BuildInfo {
    /// Product name (`GitGotten`).
    pub name: String,
    /// Semantic version of this build (`CARGO_PKG_VERSION`).
    pub version: String,
    /// Target CPU architecture (e.g. `aarch64`).
    pub arch: String,
}

/// Stable identity of an open repository: its canonical absolute path.
///
/// Path-as-id means reopening the same folder reuses the same id, and two checkouts
/// of one repo (worktrees) are naturally distinct (SPEC §10.7).
pub type RepoId = String;

/// What changed for one file, as reported by the git status machinery.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub enum ChangeKind {
    /// Worktree/index content differs from HEAD (or a different blob was recorded).
    Modified,
    /// New content recorded in the index (or a new worktree file about to be tracked).
    Added,
    /// Content recorded in the index/HEAD is gone.
    Deleted,
    /// Content moved under a new name.
    Renamed,
    /// Never tracked; only visible because untracked files are requested.
    Untracked,
    /// An unmerged (conflict) index entry for this path.
    Conflicted,
}

/// One changed file in a repository status.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct FileChange {
    /// Repo-relative display path (for renames: the *new* name).
    pub path: String,
    /// What changed.
    pub kind: ChangeKind,
    /// `true` for index-vs-HEAD (staged) changes, `false` for worktree-vs-index.
    pub staged: bool,
    /// True when the content is binary (NUL bytes in the first 8 KiB); the UI shows a
    /// "binary file" placeholder instead of a text diff.
    pub binary: bool,
}

/// Live status of an open repository (SPEC §8 read model, computed by `gix`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Status {
    /// Short name of the checked-out branch (e.g. `main`); `None` when detached.
    pub branch: Option<String>,
    /// Full OID (hex) of HEAD when detached; `None` when a branch is checked out.
    pub detached_head: Option<String>,
    /// Commits on `branch` that are not on `upstream`.
    pub ahead: u32,
    /// Commits on `upstream` that are not on `branch`.
    pub behind: u32,
    /// Upstream remote branch name (e.g. `origin/main`); `None` when untracked.
    pub upstream: Option<String>,
    /// All changed files (staged, unstaged, untracked, conflicted).
    pub changes: Vec<FileChange>,
    /// True while a merge is in progress (`.git/MERGE_HEAD` present).
    pub in_merge: bool,
    /// True while a rebase is in progress (`.git/rebase-merge` or `rebase-apply`).
    pub in_rebase: bool,
}

/// Quick summary of an open repo, for the workspace nav (SPEC §7).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct RepoMeta {
    /// Canonical absolute path (also the `RepoId`).
    pub id: RepoId,
    /// Folder basename (display name in the nav).
    pub name: String,
    /// Canonical absolute path of the repository root.
    pub path: String,
    /// Checked-out branch (or `None` when detached).
    pub branch: Option<String>,
    /// Ahead of upstream.
    pub ahead: u32,
    /// Behind upstream.
    pub behind: u32,
    /// Number of changed files (staged + unstaged + untracked).
    pub dirty_count: u32,
}

/// Payload of the `status_changed` event (Rust → web, SPEC §13).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct StatusChanged {
    /// The repository whose status changed.
    pub repo: RepoId,
    /// The new status.
    pub status: Status,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compile-time check that a type implements the `ts-rs` `TS` supertrait.
    fn _assert_ts<T: TS>() {}

    #[test]
    fn build_info_serializes_and_deserializes() {
        let bi = BuildInfo {
            name: "GitGotten".into(),
            version: "0.1.0".into(),
            arch: "aarch64".into(),
        };
        let json = serde_json::to_string(&bi).unwrap();
        assert!(json.contains("\"GitGotten\""));
        assert!(json.contains("\"0.1.0\""));

        let back: BuildInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(back, bi);
    }

    #[test]
    fn build_info_is_a_ts_type() {
        _assert_ts::<BuildInfo>();
    }

    fn sample_status() -> Status {
        Status {
            branch: Some("main".into()),
            detached_head: None,
            ahead: 1,
            behind: 0,
            upstream: Some("origin/main".into()),
            changes: vec![
                FileChange {
                    path: "staged.rs".into(),
                    kind: ChangeKind::Modified,
                    staged: true,
                    binary: false,
                },
                FileChange {
                    path: "dirty.md".into(),
                    kind: ChangeKind::Modified,
                    staged: false,
                    binary: false,
                },
                FileChange {
                    path: "new.txt".into(),
                    kind: ChangeKind::Untracked,
                    staged: false,
                    binary: false,
                },
            ],
            in_merge: false,
            in_rebase: false,
        }
    }

    #[test]
    fn status_round_trips_through_serde() {
        let s = sample_status();
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.contains("\"origin/main\""));
        assert!(json.contains("\"Modified\""));

        let back: Status = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn repo_meta_round_trips_through_serde() {
        let meta = RepoMeta {
            id: "/home/u/projects/git-gotten".into(),
            name: "git-gotten".into(),
            path: "/home/u/projects/git-gotten".into(),
            branch: Some("main".into()),
            ahead: 1,
            behind: 0,
            dirty_count: 3,
        };
        let json = serde_json::to_string(&meta).unwrap();
        let back: RepoMeta = serde_json::from_str(&json).unwrap();
        assert_eq!(back, meta);
        assert_eq!(back.name, "git-gotten");
    }

    #[test]
    fn read_model_types_are_ts_types() {
        _assert_ts::<Status>();
        _assert_ts::<FileChange>();
        _assert_ts::<ChangeKind>();
        _assert_ts::<RepoMeta>();
        _assert_ts::<StatusChanged>();
    }

    #[test]
    fn status_changed_round_trips_through_serde() {
        let s = StatusChanged {
            repo: "/tmp/repo".into(),
            status: sample_status(),
        };
        let json = serde_json::to_string(&s).unwrap();
        let back: StatusChanged = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
    }
}
