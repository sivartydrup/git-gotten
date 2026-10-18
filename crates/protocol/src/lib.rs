//! Shared Rust <-> TypeScript types for GitGotten.
//!
//! Every type that crosses the IPC boundary lives here and is generated to TypeScript
//! via `ts-rs` (see SPEC §5). Keeping them in one crate means the Rust core and the web
//! UI cannot drift apart on a shared shape.
//!
//! Stage 0 added [`BuildInfo`]; Stage 1 adds the git read-model shapes: [`Status`],
//! [`FileChange`], [`ChangeKind`], [`RepoMeta`], and the [`StatusChanged`] event payload.
//! Stage 2 adds the command-transparency shapes (SPEC §8/§9): [`Risk`], [`GitStep`],
//! [`Preview`], [`CommandPlan`], [`Op`], [`OpResult`], [`LogEntry`], and the
//! [`OpStarted`] event payload.

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

/// Risk classification of a git step or operation (SPEC §8).
///
/// Ordered by severity: a plan's effective risk is its *maximum* step risk, and the
/// UI's confirmation tier (SPEC §9) is selected from it — `ReadOnly` runs silently,
/// `Mutating` previews + confirms, `Dangerous` requires type-to-confirm.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
pub enum Risk {
    /// Leaves the repository untouched (status, log, show, …).
    ReadOnly,
    /// Changes the repository but is recoverable (commit, switch, merge, …).
    Mutating,
    /// Destructive, or rewrites published history (force-push, `reset --hard`, …).
    Dangerous,
}

/// One literal git invocation inside a [`CommandPlan`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct GitStep {
    /// The exact argv of the spawned process — `argv[0]` is the `git` binary.
    /// Always a vector: no shell, no string interpolation (SPEC §16).
    pub argv: Vec<String>,
    /// One human-readable sentence of what this step does.
    pub summary: String,
    /// The risk of this step.
    pub risk: Risk,
    /// Structured blast radius when the step affects history
    /// (e.g. `force-push `main` — remote loses 2 commits`); `None` for most steps.
    pub effect: Option<String>,
}

/// What will change before a risky op runs (SPEC §8), shown in the preview dialog.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Preview {
    /// One-line summary of the effect (rendered large in the dialog).
    pub headline: String,
    /// Detail lines: affected files, commits to be pushed, warnings.
    pub lines: Vec<String>,
    /// `true` for [`Risk::Dangerous`]: the user must type to confirm (SPEC §9).
    pub requires_type_to_confirm: bool,
}

/// A compiled, visible representation of an operation (SPEC §8 — the transparency
/// primitive). The `display` string drives the inline hint, the preview dialog, and
/// the activity-log entry — one source of truth.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct CommandPlan {
    /// Steps to run in order; the first failing step aborts the rest.
    pub steps: Vec<GitStep>,
    /// The exact command(s) as shown in the UI.
    pub display: String,
    /// Present for `Mutating`/`Dangerous` plans; drives the preview dialog.
    pub preview: Option<Preview>,
}

/// A user operation, which the core compiles into a [`CommandPlan`] (SPEC §13
/// `run_op`). The web sends intent, never argv: risk is always derived by the core.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Op {
    /// Commit the staged changes (`git commit`).
    Commit {
        /// Commit message (`-m`).
        message: String,
        /// Rewrite HEAD instead of adding a new commit (`--amend`).
        amend: bool,
    },
    /// Switch to a branch, or a commit-ish (detaches HEAD) (`git switch`).
    Checkout {
        /// Branch name or commit-ish to switch to.
        target: String,
        /// Discard local changes if they would be overwritten (`--force`).
        force: bool,
    },
    /// Push a branch to a remote (`git push`).
    Push {
        /// Remote name (e.g. `origin`).
        remote: String,
        /// Branch to push; `None` = the currently checked-out branch.
        branch: Option<String>,
        /// Overwrite the remote ref (`--force`): `Dangerous`.
        force: bool,
    },
}

/// The result of running a [`CommandPlan`] (safe failure, SPEC §8): the command and
/// its exact output are always available to the UI.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct OpResult {
    /// The plan that was run (so the UI shows exactly what ran).
    pub plan: CommandPlan,
    /// `true` when every step exited 0.
    pub ok: bool,
    /// Exit code of the last step that ran (`-1` when a step could not be spawned).
    pub exit: i32,
    /// Captured stdout of all steps that ran.
    pub stdout: String,
    /// Captured stderr of all steps that ran.
    pub stderr: String,
    /// Wall-clock duration of the whole plan, in milliseconds.
    pub duration_ms: u64,
}

/// One entry in the activity log (Rust → web, SPEC §9).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct LogEntry {
    /// Monotonic id within this app session (1-based).
    pub id: u64,
    /// The repository the op ran in.
    pub repo: RepoId,
    /// The exact command (the plan's `display` string).
    pub display: String,
    /// `true` when the op succeeded.
    pub ok: bool,
    /// Duration in milliseconds.
    pub duration_ms: u64,
    /// Unix time, in milliseconds.
    pub ts: u64,
    /// Captured stdout (shown in the collapsible detail view).
    pub stdout: String,
    /// Captured stderr.
    pub stderr: String,
}

/// Payload of the `op_started` event (Rust → web, SPEC §13).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct OpStarted {
    /// The repository the op is starting in.
    pub repo: RepoId,
    /// The exact command being started (the plan's `display` string).
    pub display: String,
    /// The plan's maximum step risk.
    pub risk: Risk,
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
    fn sample_plan() -> CommandPlan {
        CommandPlan {
            steps: vec![GitStep {
                argv: vec![
                    "git".into(),
                    "push".into(),
                    "--force".into(),
                    "origin".into(),
                    "main".into(),
                ],
                summary: "force-push main to origin".into(),
                risk: Risk::Dangerous,
                effect: Some("force-push `main` — remote loses 1 commit".into()),
            }],
            display: "git push --force origin main".into(),
            preview: Some(Preview {
                headline: "Push to origin/main".into(),
                lines: vec!["pushes 1 local commit".into(), "remote will lose 1 commit".into()],
                requires_type_to_confirm: true,
            }),
        }
    }

    #[test]
    fn risk_serializes_as_plain_words_and_is_ordered() {
        assert_eq!(serde_json::to_string(&Risk::ReadOnly).unwrap(), r#""ReadOnly""#);
        assert_eq!(serde_json::to_string(&Risk::Mutating).unwrap(), r#""Mutating""#);
        assert_eq!(serde_json::to_string(&Risk::Dangerous).unwrap(), r#""Dangerous""#);
        // The ordering drives the UI's confirmation tier (SPEC §9).
        assert!(Risk::ReadOnly < Risk::Mutating);
        assert!(Risk::Mutating < Risk::Dangerous);
        let back: Risk = serde_json::from_str(r#""Dangerous""#).unwrap();
        assert_eq!(back, Risk::Dangerous);
    }

    #[test]
    fn op_is_a_tagged_union() {
        let op = Op::Commit {
            message: "fix: the thing".into(),
            amend: false,
        };
        let json = serde_json::to_string(&op).unwrap();
        assert_eq!(
            json,
            r#"{"kind":"commit","message":"fix: the thing","amend":false}"#
        );
        let back: Op = serde_json::from_str(&json).unwrap();
        assert_eq!(back, op);
    }

    #[test]
    fn op_push_round_trips_with_a_null_branch() {
        let json = r#"{"kind":"push","remote":"origin","branch":null,"force":true}"#;
        let op: Op = serde_json::from_str(json).unwrap();
        assert!(matches!(op, Op::Push { force: true, .. }));
        assert_eq!(serde_json::to_string(&op).unwrap(), json);
    }

    #[test]
    fn command_plan_round_trips() {
        let plan = sample_plan();
        let json = serde_json::to_string(&plan).unwrap();
        assert!(json.contains(r#""git push --force origin main""#));
        assert!(json.contains(r#""requires_type_to_confirm":true"#));
        let back: CommandPlan = serde_json::from_str(&json).unwrap();
        assert_eq!(back, plan);
    }

    #[test]
    fn op_result_round_trips() {
        let result = OpResult {
            plan: sample_plan(),
            ok: false,
            exit: 1,
            stdout: "ok part\n".into(),
            stderr: "failed part\n".into(),
            duration_ms: 42,
        };
        let json = serde_json::to_string(&result).unwrap();
        let back: OpResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back, result);
    }

    #[test]
    fn log_entry_round_trips() {
        let entry = LogEntry {
            id: 7,
            repo: "/home/u/repo".into(),
            display: "git commit -m \"hi\"".into(),
            ok: true,
            duration_ms: 15,
            ts: 1_700_000_000_000,
            stdout: "done\n".into(),
            stderr: String::new(),
        };
        let json = serde_json::to_string(&entry).unwrap();
        let back: LogEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(back, entry);
    }

    #[test]
    fn op_started_round_trips() {
        let event = OpStarted {
            repo: "/home/u/repo".into(),
            display: "git push --force origin main".into(),
            risk: Risk::Dangerous,
        };
        let json = serde_json::to_string(&event).unwrap();
        let back: OpStarted = serde_json::from_str(&json).unwrap();
        assert_eq!(back, event);
    }

    #[test]
    fn command_types_are_ts_types() {
        _assert_ts::<Risk>();
        _assert_ts::<GitStep>();
        _assert_ts::<Preview>();
        _assert_ts::<CommandPlan>();
        _assert_ts::<Op>();
        _assert_ts::<OpResult>();
        _assert_ts::<LogEntry>();
        _assert_ts::<OpStarted>();
    }

}
