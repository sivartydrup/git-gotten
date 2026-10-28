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
//! Stage 3 adds the branching-graph shapes (SPEC §10.1): [`Graph`], [`Node`],
//! [`Edge`], [`Line`], [`LineSegment`], [`RefLabel`], [`RefKind`], and the
//! [`BRANCH_PALETTE`] color indices they reference.
//! Stage 4 adds the inspect-drawer shapes (`load_commit`): [`CommitDetail`],
//! [`FileStat`], [`FileDiff`] — and the graph context-menu ops: [`Op::BranchCreate`],
//! [`Op::Reset`] ([`ResetMode`]), [`Op::CherryPick`].
//! Stage 5 adds the per-hunk staging shapes (SPEC §10.2): [`Hunk`], the
//! working-tree side of [`FileDiff`], [`Author`], the stdin [`GitStep::input`],
//! and the staging ops: [`Op::Stage`], [`Op::Unstage`], [`Op::Discard`],
//! [`Op::StageHunks`].
//! Stage 6 adds the ref-management shapes (SPEC §10.3): [`Branch`], [`Remote`],
//! [`Tag`], the `Status.head` oid, and the ops: [`Op::BranchDelete`],
//! [`Op::BranchRename`], [`Op::BranchSetUpstream`], [`Op::RemoteAdd`],
//! [`Op::RemoteRemove`], [`Op::RemoteFetch`], [`Op::TagCreate`],
//! [`Op::TagCheckout`], [`Op::TagDelete`], [`Op::TagPush`] — plus the
//! `set_upstream` flag on [`Op::Push`].
//! Stage 7 adds the sync + merge shapes (SPEC §10.4): [`Op::Pull`]
//! (`git pull [--rebase]`), [`Op::Merge`] (`git merge --no-ff <source>`), and
//! the `conflicts` field on [`OpResult`] / [`LogEntry`] (the unmerged files a
//! merge/rebase/cherry-pick stopped on).
//! Stage 8 adds the conflict-resolution shapes (SPEC §10.5): [`ConflictFile`],
//! [`ConflictSegment`], [`ConflictHunk`], [`ConflictSide`], [`HunkChoice`],
//! [`ConflictResolution`], `Status.in_cherry_pick`, the
//! [`GitStep::program`] (external-editor hand-off), and the ops
//! [`Op::ResolveConflict`], [`Op::ContinueOp`], [`Op::AbortOp`],
//! [`Op::OpenEditor`].
//! Stage 9 adds the interactive-rebase shapes (SPEC §10.4): [`RebaseAction`],
//! [`RebaseEdit`], [`RebaseCommit`] (the range behind the RebaseDialog), the ops
//! [`Op::Rebase`], [`Op::RebaseOnto`], and the `target` on [`Op::CherryPick`]
//! (pick onto another branch).
//! Stage 10 adds the worktree shapes (SPEC §10.7): [`Worktree`], the IPC
//! `load_worktrees` read, and the ops [`Op::WorktreeAdd`] and
//! [`Op::WorktreeRemove`] (force-removing a dirty worktree is `Dangerous`).

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
    /// Full OID (hex) of HEAD (branch tip or detached); `None` on an unborn branch.
    pub head: Option<String>,
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
    /// True while a cherry-pick is in progress (`.git/CHERRY_PICK_HEAD` present).
    #[serde(default)]
    pub in_cherry_pick: bool,
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

// ── Refs: branches, remotes, tags (SPEC §10.3, computed by `gix`) ──────────

/// A branch — local or remote-tracking — for the Branches tab (SPEC §10.3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Branch {
    /// Display name: `main`, or `origin/feature` for a remote-tracking branch.
    pub name: String,
    /// `true` when HEAD is on this branch (always `false` for remote branches).
    pub is_current: bool,
    /// `true` for a remote-tracking branch (`refs/remotes/…`).
    pub is_remote: bool,
    /// The branch's upstream (e.g. `origin/main`); `None` when untracked.
    pub upstream: Option<String>,
    /// Commits on the branch that are not on its upstream (0 when untracked).
    pub ahead: u32,
    /// Commits on the upstream that are not on the branch (0 when untracked).
    pub behind: u32,
    /// The commit the branch points at.
    pub tip: Hash,
}

/// A configured remote (a `[remote "…"]` section) for the Remotes tab (SPEC §10.3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Remote {
    /// The remote's name (e.g. `origin`).
    pub name: String,
    /// The url to display: the fetch url, falling back to the push url.
    pub url: String,
    /// The fetch url (`remote.<name>.url`); `None` when only a push url is set.
    pub fetch: Option<String>,
    /// The push url (`remote.<name>.pushurl`); `None` when unset (pushes use the fetch url).
    pub push: Option<String>,
}

/// A tag for the Tags tab (SPEC §10.3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Tag {
    /// The tag name (e.g. `v1.0`).
    pub name: String,
    /// The commit the tag points at (annotated tags are peeled to their commit).
    pub target: Hash,
    /// `true` for an annotated tag (a tag object); `false` for a lightweight one.
    pub annotated: bool,
    /// The tag object's message (annotated tags); `None` for lightweight tags.
    pub message: Option<String>,
}

// ── Worktrees (SPEC §10.7) ─────────────────────────────────────────────────

/// One checkout of a repository's shared object database: the main worktree or
/// a linked `git worktree` checkout. The canonical absolute `path` doubles as
/// the [`RepoId`] when the worktree is opened as the active repository.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Worktree {
    /// Canonical absolute path of the worktree root.
    pub path: String,
    /// The checked-out branch; `None` when HEAD is detached there.
    pub branch: Option<String>,
    /// Object id HEAD points at in this worktree.
    pub head: Hash,
    /// `true` for the main worktree (the one with the shared `.git` directory).
    pub is_main: bool,
    /// Manually locked (`git worktree lock`).
    pub locked: bool,
    /// git considers it prunable (its working tree is gone).
    pub prunable: bool,
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
    /// The exact argv of the spawned process — `argv[0]` is the program
    /// (the `git` binary when [`Self::program`] is `None`). Always a vector:
    /// no shell, no string interpolation (SPEC §16).
    pub argv: Vec<String>,
    /// One human-readable sentence of what this step does.
    pub summary: String,
    /// The risk of this step.
    pub risk: Risk,
    /// Structured blast radius when the step affects history
    /// (e.g. `force-push `main` — remote loses 2 commits`); `None` for most steps.
    pub effect: Option<String>,
    /// Optional text piped to the process's stdin (e.g. a patch for
    /// `git apply --cached`). `None` for steps that take no input.
    pub input: Option<String>,
    /// The program to spawn instead of the `git` binary (the external-editor
    /// hand-off, SPEC §10.5). `None` runs the resolved `git`; `Some` runs that
    /// program with `argv[1..]` as its arguments. `argv[0]` always names the
    /// program, so the `display` string is exactly what is spawned.
    #[serde(default)]
    pub program: Option<String>,
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

/// The mode of a `git reset` requested by the graph context menu (SPEC §10.1).
///
/// `Soft` moves only the branch pointer (index and worktree untouched); `Mixed`
/// also resets the index to the target (changes reappear unstaged); `Hard`
/// discards the working tree too — the only one of the three that destroys
/// uncommitted work, so it is the `Dangerous` (type-to-confirm) tier.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ResetMode {
    /// `git reset --soft <target>` — move the pointer, keep everything else.
    Soft,
    /// `git reset --mixed <target>` — move the pointer, unstage the difference.
    Mixed,
    /// `git reset --hard <target>` — discard all uncommitted changes.
    Hard,
}

/// The action an interactive-rebase todo takes on one commit (SPEC §10.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub enum RebaseAction {
    /// Re-apply the commit as-is.
    Pick,
    /// Re-apply with a new message ([`RebaseEdit::message`]).
    Reword,
    /// Fold into the previous rebased commit; a new message when given.
    Squash,
    /// Fold into the previous rebased commit, keeping its message.
    Fixup,
    /// Pause the rebase after this commit (`edit`); resume with `ContinueOp`.
    Edit,
    /// Remove the commit from the branch.
    Drop,
}

/// One entry of the edited interactive-rebase todo ([`Op::Rebase::edits`]).
///
/// The web sends edits in *UI order* — newest commit first, like `git log` —
/// and the core reverses them into git's oldest-first todo.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct RebaseEdit {
    /// The commit this edit applies to (full hex oid).
    pub hash: Hash,
    /// What the rebase does with it.
    pub action: RebaseAction,
    /// The new message for `Reword`/`Squash` (single line); `None` keeps git's own.
    #[serde(default)]
    pub message: Option<String>,
}

/// One commit in the range an interactive rebase would rewrite (the
/// `load_rebase_commits` read behind the RebaseDialog, SPEC §10.4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct RebaseCommit {
    /// Full hex oid.
    pub hash: Hash,
    /// The commit message's first line.
    pub subject: String,
}

/// A user operation, which the core compiles into a [`CommandPlan`] (SPEC §13
/// `run_op`). The web sends intent, never argv: risk is always derived by the core.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Op {
    /// Commit the staged changes (`git commit`).
    Commit {
        /// Commit message (`-m`; co-authors are appended as trailers by the core).
        message: String,
        /// Rewrite HEAD instead of adding a new commit (`--amend`).
        amend: bool,
        /// Override the author identity (`--author`); `None` uses the configured identity.
        #[serde(default)]
        author: Option<Author>,
        /// Extra `Co-authored-by: Name <email>` trailer lines.
        #[serde(default)]
        co_authors: Vec<String>,
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
        /// Record the remote branch as the push upstream (`-u`).
        #[serde(default)]
        set_upstream: bool,
    },
    /// Pull from the current branch's upstream (`git pull [--rebase]`): fetch
    /// the upstream, then merge it (or rebase onto it when `rebase` is set).
    Pull {
        /// Rebase the local commits onto the upstream instead of merging it.
        rebase: bool,
    },
    /// Merge a branch (or commit-ish) into the current branch
    /// (`git merge --no-ff <source>`): always produces a merge commit, so a
    /// clean fast-forwardable merge is visible in the graph too.
    Merge {
        /// The source branch/commit to merge in.
        source: String,
    },
    /// Rewrite the commits above `target` per an edited todo (interactive
    /// rebase, SPEC §10.4). Rebasing commits already on a remote is `Dangerous`
    /// (type-to-confirm).
    Rebase {
        /// The branch/commit-ish to rebase onto.
        target: String,
        /// One edit per commit in `target..HEAD`, UI order (newest first);
        /// the core rejects any todo that does not cover exactly that range.
        edits: Vec<RebaseEdit>,
    },
    /// Move the current branch's commits above `target` onto it, keeping order
    /// and messages (`git rebase <target>`, SPEC §10.4 "rebase-onto").
    /// `Dangerous` when the rebased commits are already on a remote.
    RebaseOnto {
        /// The branch/commit-ish to rebase onto.
        target: String,
    },
    /// Create a branch at a commit (`git branch <name> <from>`), optionally
    /// switching to it (graph context menu, SPEC §10.1).
    BranchCreate {
        /// New branch name (a full ref name, not a commit-ish).
        name: String,
        /// The commit the new branch points at.
        from: Hash,
        /// Also `git switch` to the new branch after creating it.
        checkout: bool,
    },
    /// Move the current branch pointer to a commit (`git reset --<mode> <target>`;
    /// graph context menu "reset to here", SPEC §10.1).
    Reset {
        /// The reset mode (`--soft` / `--mixed` / `--hard`).
        mode: ResetMode,
        /// The commit to move to.
        target: Hash,
    },
    /// Re-apply commit(s) on top of the current branch (`git cherry-pick <hashes…>`;
    /// graph context menu, SPEC §10.1).
    CherryPick {
        /// The commits to pick, in the order they should be applied.
        hashes: Vec<Hash>,
        /// Branch to pick onto: switched to first when not the current branch;
        /// `None` picks onto the current branch.
        #[serde(default)]
        target: Option<String>,
    },
    /// Stage (add) a file, or the whole tree when `path` is `None` (`git add`).
    Stage {
        /// The file to stage; `None` stages everything (`git add -A`).
        path: Option<String>,
    },
    /// Unstage (reset from HEAD) a file, or the whole index when `path` is `None`
    /// (`git reset --`). Content in the worktree is untouched.
    Unstage {
        /// The file to unstage; `None` unstages everything.
        path: Option<String>,
    },
    /// Discard a file's uncommitted changes, restoring it to its HEAD state
    /// (`git checkout --`-equivalent). `Dangerous` — destroys uncommitted work.
    Discard {
        /// The file to restore.
        path: String,
    },
    /// Stage the selected hunks of a file (`git apply --cached` of the built patch).
    /// `selected` indexes into the file's *unstaged* hunks.
    StageHunks {
        /// The file whose hunks are staged.
        path: String,
        /// Indices (into the unstaged hunk list) to stage.
        selected: Vec<usize>,
    },
    /// Delete a branch: locally (`git branch -d/-D`), or on a remote
    /// (`git push <remote> --delete <name>`) when `remote` is set.
    BranchDelete {
        /// The branch name.
        name: String,
        /// `None` = a local branch; `Some` = delete that branch on this remote.
        remote: Option<String>,
        /// Force-delete unmerged work locally (`-D`). Ignored for remote deletes.
        force: bool,
    },
    /// Rename a branch (`git branch --move <name> <new_name>`).
    BranchRename {
        /// The current branch name.
        name: String,
        /// The new name.
        new_name: String,
    },
    /// Set (or clear) a local branch's upstream (`git branch --set-upstream-to`
    /// / `--unset-upstream`).
    BranchSetUpstream {
        /// The local branch.
        branch: String,
        /// The remote to track; `None` clears the upstream.
        remote: Option<String>,
        /// The remote branch to track (used when `remote` is set).
        name: String,
    },
    /// Add a remote (`git remote add <name> <url>`).
    RemoteAdd {
        /// The new remote's name.
        name: String,
        /// Its fetch url.
        url: String,
    },
    /// Remove a remote and its remote-tracking branches (`git remote remove`).
    RemoteRemove {
        /// The remote's name.
        name: String,
    },
    /// Fetch a remote (`git fetch <remote> [refspec]`).
    RemoteFetch {
        /// The remote to fetch from.
        remote: String,
        /// An explicit refspec; `None` = the remote's configured refspecs.
        refspec: Option<String>,
    },
    /// Create a tag (`git tag [-a] [-m <msg>] <name> <target>`).
    TagCreate {
        /// The new tag name.
        name: String,
        /// The commit the tag points at.
        target: Hash,
        /// Annotated (a tag object with a message) vs lightweight (a direct pointer).
        annotated: bool,
        /// The tag message (annotated tags; an empty one is allowed, never opens an editor).
        message: Option<String>,
    },
    /// Check out a tag, detaching HEAD (`git switch --detach <name>`).
    TagCheckout {
        /// The tag to check out.
        name: String,
        /// Discard local changes if they would be overwritten (`--force`).
        force: bool,
    },
    /// Delete a tag: locally (`git tag -d`), or on a remote
    /// (`git push <remote> --delete <name>`) when `remote` is set.
    TagDelete {
        /// The tag name.
        name: String,
        /// `None` = a local tag; `Some` = delete that tag on this remote.
        remote: Option<String>,
    },
    /// Push a tag to a remote (`git push <remote> <name>`).
    TagPush {
        /// The remote to push to.
        remote: String,
        /// The tag name.
        name: String,
        /// Overwrite the remote tag (`--force`): `Dangerous`.
        force: bool,
    },
    /// Resolve one conflicted file (SPEC §10.5): write the resolved content to
    /// the worktree and stage it, clearing the unmerged index entries.
    ResolveConflict {
        /// The conflicted file (repo-relative path).
        path: String,
        /// How the file is resolved.
        resolution: ConflictResolution,
    },
    /// Conclude the in-progress merge/rebase/cherry-pick
    /// (`git merge --continue` / `git rebase --continue` / `git cherry-pick
    /// --continue`): the in-progress operation is detected from the repository
    /// state, so one op covers all three.
    ContinueOp,
    /// Abort the in-progress merge/rebase/cherry-pick (`… --abort`): restores
    /// the pre-operation state. `Dangerous` — it discards the in-progress
    /// resolution work.
    AbortOp,
    /// Open the given files in the configured external editor (`core.editor`,
    /// falling back to `$VISUAL` / `$EDITOR`): the escape hatch for gnarly
    /// conflicts (SPEC §10.5).
    OpenEditor {
        /// The files to open (repo-relative paths).
        paths: Vec<String>,
    },
    /// Add a linked worktree (`git worktree add [-b <new_branch>] <path> <committish>`,
    /// SPEC §10.7). `Mutating`: git itself refuses to check out a branch that is
    /// already checked out in another worktree, and that refusal surfaces as a
    /// failed [`OpResult`] (clean error, not a crash).
    WorktreeAdd {
        /// Existing branch or commit to check out; the start point when `new_branch` is set.
        committish: String,
        /// Directory to create the worktree in.
        path: String,
        /// `Some` = create and check out this new branch; `None` = check out `committish`.
        new_branch: Option<String>,
    },
    /// Remove a linked worktree (`git worktree remove [--force] <path>`).
    /// A clean worktree is `Mutating`; force-removing a dirty one is
    /// `Dangerous` (type-to-confirm — it discards uncommitted work). The main
    /// worktree is never removable through this op.
    WorktreeRemove {
        /// The worktree's absolute path.
        path: String,
        /// Remove despite uncommitted changes (`--force`).
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
    /// The unmerged file paths when the op left the repository in a conflicted
    /// state (a merge/rebase/cherry-pick that stopped on conflicts); empty
    /// otherwise. The web routes a non-empty list to the conflict view.
    #[serde(default)]
    pub conflicts: Vec<String>,
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
    /// The unmerged file paths when the op stopped on conflicts (see
    /// [`OpResult::conflicts`]).
    #[serde(default)]
    pub conflicts: Vec<String>,
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

// ── Branching graph (SPEC §10.1, computed by `crates/graph`) ─────────────────

/// The default branch-lane palette: packed `0xRRGGBB` colors, in assignment
/// order. `crates/graph` assigns each line an index into this palette (a line's
/// `color` field is an index, not an RGB value), so the web can map indices to
/// theme tokens (SPEC §12) in light/dark without the layout knowing about theming.
///
/// Chosen for pairwise distinguishability: hue rotations with alternating
/// lightness so adjacent lanes read as distinct at small sizes.
pub const BRANCH_PALETTE: [u32; 20] = [
    0x4C8BF5, // 0  blue (mainline first)
    0xE05252, // 1  red
    0x58B368, // 2  green
    0xE89A3C, // 3  orange
    0x9B6BD4, // 4  purple
    0x4CC9C0, // 5  teal
    0xD65FA3, // 6  pink
    0x8E9C3C, // 7  olive
    0x5F6EF0, // 8  indigo
    0xC75C2E, // 9  vermillion
    0x2FB56B, // 10 emerald
    0x7E5BC0, // 11 violet
    0xB0893B, // 12 gold
    0x3FA8C4, // 13 cyan
    0xB4589E, // 14 fuchsia
    0x6FA332, // 15 lime
    0x9E4F4F, // 16 brick
    0x5472D6, // 17 royal blue
    0x8C6BA8, // 18 mauve
    0x4A9E8C, // 19 sea green
];

/// A line's stable identity within a [`Graph`].
pub type LineId = u32;

/// A commit's object id, as a hex string.
pub type Hash = String;

/// What kind of ref a [`RefLabel`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub enum RefKind {
    /// A local branch (`refs/heads/*`).
    Branch,
    /// A remote-tracking ref (`refs/remotes/*/*`); `name` is `remote/branch`.
    Remote,
    /// A tag (annotated tags are peeled to their commit).
    Tag,
    /// A worktree checkout (added in Stage 10).
    Worktree,
}

/// A ref pinned to a node, rendered as a colored label in the graph (SPEC §10.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct RefLabel {
    /// Display name (e.g. `main`, `origin/main`, `v1.0`).
    pub name: String,
    /// The ref kind.
    pub kind: RefKind,
    /// Palette index of the line this ref sits on (same value space as [`Line::color`]).
    pub color: u32,
}

/// One span of a [`Line`] in a fixed lane: the rows it is alive there, inclusive.
///
/// A line's lane can change over its lifetime (compaction when lanes to its left
/// die), so its geometry is a sequence of segments, not a single lane.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct LineSegment {
    /// The lane (x index, 0 = leftmost) the line occupies in this span.
    pub lane: i32,
    /// First row (y index, 0 = top/newest) of the span.
    pub from_row: u32,
    /// Last row of the span (inclusive): the row where the line dies or moves lanes.
    pub to_row: u32,
}

/// A line of history (SPEC §10.1): a lane lineage with a stable color.
///
/// A line opens when a commit is placed in a lane no live line owns (a tip, or a
/// merge's additional parent) and dies when its frontier commit is emitted
/// (converging into another line, or reaching a root). Its [`LineSegment`]s are its
/// lane over time; the web draws them as the vertical lane strokes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Line {
    /// Stable identity within the graph.
    pub id: LineId,
    /// Palette index into [`BRANCH_PALETTE`] (theme-mapped by the web).
    pub color: u32,
    /// The branch name at the line's opener, when one exists (e.g. `feature`);
    /// `None` for lines that open at an unnamed commit.
    pub name: Option<String>,
    /// The line's lane over time, in row order (contiguous spans).
    pub segments: Vec<LineSegment>,
}

/// One commit in the laid-out graph (SPEC §10.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Node {
    /// The commit's full object id (hex).
    pub hash: Hash,
    /// First line of the commit message.
    pub subject: String,
    /// Author name.
    pub author: String,
    /// Author date, unix seconds (for display).
    pub date: u64,
    /// Row (y index, 0 = newest; one commit per row, newest-first topological).
    pub row: u32,
    /// Lane (x index, 0 = leftmost).
    pub lane: i32,
    /// `true` for the node HEAD points at (the graph emphasizes it).
    pub is_head: bool,
    /// Refs pointing at this commit (branches, remote-tracking, tags).
    pub refs: Vec<RefLabel>,
    /// The lines alive at this node's row, in lane order — the vertical strokes
    /// the renderer draws through the row (including the line of the node itself).
    pub lines: Vec<LineId>,
}

/// A commit→parent edge. Merge commits have ≥2; the renderer draws each as a
/// curve from the child's (lane, row) to the parent's, routed around live lanes.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
pub struct Edge {
    /// The child commit (the merge commit, or the newer side of a normal edge).
    pub from: Hash,
    /// The parent commit.
    pub to: Hash,
}

/// The laid-out commit graph for one repository (SPEC §10.1): what the canvas
/// renderer draws. `nodes` are in row order (newest first); `lines` carry the
/// lane geometry + colors; `edges` connect every commit to its parents.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Graph {
    /// All commits reachable from the walked refs, laid out.
    pub nodes: Vec<Node>,
    /// Every commit→parent edge in the DAG.
    pub edges: Vec<Edge>,
    /// All lines, in creation order.
    pub lines: Vec<Line>,
}

/// Per-file change counts in a commit's diff (the inspect drawer's stat,
/// SPEC §10.1). `binary` files have unknown counts (both `0`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct FileStat {
    /// The file path in the commit (a renamed file shows its new name).
    pub path: String,
    /// Lines added.
    pub additions: u32,
    /// Lines removed.
    pub deletions: u32,
    /// `true` when either side is a binary file (no text counts).
    pub binary: bool,
}

/// One hunk of a unified diff (SPEC §10.2): a single `@@`-delimited block of
/// changed lines, the unit of per-hunk staging.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Hunk {
    /// The `@@ -l,c +l,c @@ [section]` header line.
    pub header: String,
    /// The hunk body lines, each with its leading ` ` / `+` / `-` marker.
    pub lines: Vec<String>,
    /// `true` when this hunk comes from the index-vs-HEAD (staged) side of the
    /// diff, `false` for the worktree-vs-index (unstaged) side. The per-hunk
    /// stage checkboxes only act on unstaged hunks.
    pub staged: bool,
}

/// One file's unified diff. `patch` is raw git unified-diff text for that file;
/// `hunks` is the parsed form of the same diff (the working-tree view renders
/// hunks so each can carry a stage checkbox, SPEC §10.2). Commit diffs
/// (`CommitDetail`) also carry hunks (parsed, `staged: false`) but the drawer
/// renders the raw `patch` instead.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct FileDiff {
    /// The file path in the diff (a renamed file shows its new name).
    pub path: String,
    /// `true` when the diff is a binary marker instead of text hunks.
    pub binary: bool,
    /// The unified diff text for this file (`index`/`---`/`+++`/hunks, no
    /// `diff --git` header).
    pub patch: String,
    /// The hunks parsed from `patch` (empty for binary files).
    pub hunks: Vec<Hunk>,
}

// ── Conflict resolution (SPEC §10.5, computed by `gitcore`) ────────────────

/// The three versions of one conflict region (the 3-way view's building block):
/// the common-ancestor, the current-branch, and the incoming-branch content of
/// the same span of the file (from index stages 1 / 2 / 3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ConflictHunk {
    /// The base (common ancestor, stage 1) content of the region.
    pub base: String,
    /// The ours (stage 2, HEAD) content of the region.
    pub ours: String,
    /// The theirs (stage 3, the incoming ref) content of the region.
    pub theirs: String,
}

/// One span of a file's 3-way diff: unchanged (common) text, or a [`ConflictHunk`]
/// region where the two sides diverged. The sequence of segments of a file
/// reconstructs its resolved content once each conflict region carries a choice.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub enum ConflictSegment {
    /// Unchanged text, shared by both sides.
    Common(String),
    /// A conflict region with its three versions.
    Conflict(ConflictHunk),
}

/// The full 3-way view of one conflicted file (the conflict editor's input,
/// SPEC §10.5): the three side versions plus the parsed diff3 structure.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ConflictFile {
    /// The file path (repo-relative).
    pub path: String,
    /// `true` when the file is binary (or not decodable as text): the 3-way
    /// text fields and segments are empty, and only a whole-side resolution
    /// ([`ConflictResolution::Side`]) is offered.
    pub binary: bool,
    /// The full base (common ancestor) version of the file; empty for binary.
    pub base: String,
    /// The full ours (HEAD) version of the file; empty for binary.
    pub ours: String,
    /// The full theirs (incoming) version of the file; empty for binary.
    pub theirs: String,
    /// The parsed 3-way structure in file order (empty for binary files).
    /// The web renders this as the combined view; a conflict's [`HunkChoice`]
    /// index space is the order of the `Conflict` segments.
    pub segments: Vec<ConflictSegment>,
}

/// Which side of a conflict a whole-file resolution takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub enum ConflictSide {
    /// Take the entire file from the ours (HEAD) version.
    Ours,
    /// Take the entire file from the theirs (incoming) version.
    Theirs,
}

/// How the user resolves one conflict region (per the 3-way view).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub enum HunkChoice {
    /// Keep the ours content of the region.
    AcceptOurs,
    /// Keep the theirs content of the region.
    AcceptTheirs,
    /// Keep both (ours then theirs, a union resolution).
    AcceptBoth,
    /// Use this exact text as the region's resolution (manual edit).
    Custom(String),
}

/// How one conflicted file is resolved (the input of [`Op::ResolveConflict`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub enum ConflictResolution {
    /// Per-region choices, in the order of the file's `Conflict` segments.
    Hunks(Vec<HunkChoice>),
    /// Take the whole file from one side.
    Side(ConflictSide),
}

/// A commit identity (name + email) for the commit panel's author field.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct Author {
    pub name: String,
    pub email: String,
}

/// Full details of one commit, for the inspect drawer (IPC `load_commit`,
/// SPEC §10.1): identity + full message + both dates + parents + the refs
/// pointing here + per-file stat and diff.
///
/// Merge commits (≥2 parents) have empty `stat`/`diff`: git shows no diff
/// for a merge without combined-diff flags, so the drawer says so instead.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct CommitDetail {
    /// The commit's full object id (hex).
    pub hash: Hash,
    /// First line of the message.
    pub subject: String,
    /// The complete commit message (blank line + body, trailing newline trimmed).
    pub message: String,
    pub author_name: String,
    pub author_email: String,
    /// Author date, unix seconds.
    pub author_date: u64,
    pub committer_name: String,
    pub committer_email: String,
    /// Committer date, unix seconds.
    pub committer_date: u64,
    /// Parent hashes, in parent order (a merge lists all of them).
    pub parents: Vec<Hash>,
    /// Refs pointing at this commit (branches, remote-tracking, tags).
    pub refs: Vec<RefLabel>,
    /// `true` when HEAD points at this commit.
    pub is_head: bool,
    /// Per-file +/- counts (empty for merge commits).
    pub stat: Vec<FileStat>,
    /// Per-file unified diffs (empty for merge commits).
    pub diff: Vec<FileDiff>,
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
            head: Some("a".repeat(40)),
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
            in_cherry_pick: true,
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
                input: None,
            program: None,
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
            author: None,
            co_authors: vec![],
        };
        let json = serde_json::to_string(&op).unwrap();
        assert_eq!(
            json,
            r#"{"kind":"commit","message":"fix: the thing","amend":false,"author":null,"co_authors":[]}"#
        );
        let back: Op = serde_json::from_str(&json).unwrap();
        assert_eq!(back, op);
    }

    #[test]
    fn op_push_round_trips_with_a_null_branch() {
        // `set_upstream` is `#[serde(default)]`: the old web payloads (without
        // the field) still deserialize, but serialization always carries it.
        let json = r#"{"kind":"push","remote":"origin","branch":null,"force":true,"set_upstream":false}"#;
        let op: Op = serde_json::from_str(json).unwrap();
        assert!(matches!(op, Op::Push { force: true, .. }));
        assert_eq!(serde_json::to_string(&op).unwrap(), json);
        // A payload from an older web build (no `set_upstream`) still decodes.
        let old: Op =
            serde_json::from_str(r#"{"kind":"push","remote":"origin","branch":null,"force":false}"#)
                .unwrap();
        let Op::Push { set_upstream, .. } = &old else {
            panic!("wrong variant");
        };
        assert!(!set_upstream);
    }

    #[test]
    fn op_pull_round_trips() {
        let op = Op::Pull { rebase: true };
        let json = serde_json::to_string(&op).unwrap();
        assert_eq!(json, r#"{"kind":"pull","rebase":true}"#);
        let back: Op = serde_json::from_str(&json).unwrap();
        assert_eq!(back, op);
    }

    #[test]
    fn op_merge_round_trips() {
        let op = Op::Merge {
            source: "side".into(),
        };
        let json = serde_json::to_string(&op).unwrap();
        assert_eq!(json, r#"{"kind":"merge","source":"side"}"#);
        let back: Op = serde_json::from_str(&json).unwrap();
        assert_eq!(back, op);
    }

    #[test]
    fn op_rebase_round_trips() {
        let op = Op::Rebase {
            target: "main".into(),
            edits: vec![
                RebaseEdit {
                    hash: "aa".into(),
                    action: RebaseAction::Pick,
                    message: None,
                },
                RebaseEdit {
                    hash: "bb".into(),
                    action: RebaseAction::Squash,
                    message: Some("B+C".into()),
                },
            ],
        };
        let json = serde_json::to_string(&op).unwrap();
        assert_eq!(
            json,
            r#"{"kind":"rebase","target":"main","edits":[{"hash":"aa","action":"Pick","message":null},{"hash":"bb","action":"Squash","message":"B+C"}]}"#
        );
        let back: Op = serde_json::from_str(&json).unwrap();
        assert_eq!(back, op);
    }

    #[test]
    fn op_rebase_onto_round_trips() {
        let op = Op::RebaseOnto {
            target: "main".into(),
        };
        let json = serde_json::to_string(&op).unwrap();
        assert_eq!(json, r#"{"kind":"rebase_onto","target":"main"}"#);
        let back: Op = serde_json::from_str(&json).unwrap();
        assert_eq!(back, op);
    }

    #[test]
    fn op_cherry_pick_target_is_optional_on_the_wire() {
        // An older payload (no `target`) still decodes.
        let op: Op =
            serde_json::from_str(r#"{"kind":"cherry_pick","hashes":["aa"]}"#).unwrap();
        let Op::CherryPick { target, .. } = &op else {
            panic!("wrong variant");
        };
        assert!(target.is_none());
        assert_eq!(
            serde_json::to_string(&op).unwrap(),
            r#"{"kind":"cherry_pick","hashes":["aa"],"target":null}"#
        );
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
            conflicts: vec!["f.txt".into()],
        };
        let json = serde_json::to_string(&result).unwrap();
        let back: OpResult = serde_json::from_str(&json).unwrap();
        assert_eq!(back, result);
        // A payload from an older build (no `conflicts` field) still decodes.
        let old: OpResult = serde_json::from_str(&json.replace(
            r#","conflicts":["f.txt"]"#,
            "",
        ))
        .unwrap();
        assert!(old.conflicts.is_empty());
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
            conflicts: vec!["f.txt".into()],
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
        _assert_ts::<RebaseAction>();
        _assert_ts::<RebaseEdit>();
        _assert_ts::<RebaseCommit>();
    }

    fn sample_graph() -> Graph {
        Graph {
            nodes: vec![
                Node {
                    hash: "a".repeat(40),
                    subject: "Merge branch 'feature'".into(),
                    author: "Test User".into(),
                    date: 1_700_000_100,
                    row: 0,
                    lane: 0,
                    is_head: true,
                    refs: vec![RefLabel {
                        name: "main".into(),
                        kind: RefKind::Branch,
                        color: 0,
                    }],
                    lines: vec![0],
                },
                Node {
                    hash: "b".repeat(40),
                    subject: "feature: work".into(),
                    author: "Test User".into(),
                    date: 1_700_000_090,
                    row: 1,
                    lane: 1,
                    is_head: false,
                    refs: vec![RefLabel {
                        name: "feature".into(),
                        kind: RefKind::Branch,
                        color: 1,
                    }],
                    lines: vec![0, 1],
                },
            ],
            edges: vec![
                Edge {
                    from: "a".repeat(40),
                    to: "b".repeat(40),
                },
                Edge {
                    from: "b".repeat(40),
                    to: "c".repeat(40),
                },
            ],
            lines: vec![
                Line {
                    id: 0,
                    color: 0,
                    name: Some("main".into()),
                    segments: vec![LineSegment { lane: 0, from_row: 0, to_row: 5 }],
                },
                Line {
                    id: 1,
                    color: 1,
                    name: Some("feature".into()),
                    segments: vec![LineSegment { lane: 1, from_row: 1, to_row: 4 }],
                },
            ],
        }
    }

    #[test]
    fn graph_round_trips_through_serde() {
        let g = sample_graph();
        let json = serde_json::to_string(&g).unwrap();
        assert!(json.contains(r#""is_head":true"#));
        assert!(json.contains(r#""from_row":1"#));
        let back: Graph = serde_json::from_str(&json).unwrap();
        assert_eq!(back, g);
    }

    #[test]
    fn graph_types_are_ts_types() {
        _assert_ts::<Graph>();
        _assert_ts::<Node>();
        _assert_ts::<Edge>();
        _assert_ts::<Line>();
        _assert_ts::<LineSegment>();
        _assert_ts::<RefLabel>();
        _assert_ts::<RefKind>();
    }

    #[test]
    fn branch_palette_is_distinguishable() {
        let mut distinct = std::collections::HashSet::new();
        for c in BRANCH_PALETTE {
            assert!(distinct.insert(c), "duplicate palette color {c:#x}");
        }
        assert_eq!(distinct.len(), 20);
    }

    #[test]
    fn reset_mode_serializes_as_plain_words() {
        assert_eq!(serde_json::to_string(&ResetMode::Soft).unwrap(), r#""soft""#);
        assert_eq!(serde_json::to_string(&ResetMode::Mixed).unwrap(), r#""mixed""#);
        assert_eq!(serde_json::to_string(&ResetMode::Hard).unwrap(), r#""hard""#);
        let back: ResetMode = serde_json::from_str(r#""hard""#).unwrap();
        assert_eq!(back, ResetMode::Hard);
    }

    #[test]
    fn op_context_menu_variants_round_trip() {
        // Each variant: exact JSON in, same JSON out.
        for json in [
            r#"{"kind":"branch_create","name":"feature","from":"abc","checkout":true}"#,
            r#"{"kind":"branch_create","name":"hotfix","from":"def","checkout":false}"#,
            r#"{"kind":"reset","mode":"hard","target":"def"}"#,
            r#"{"kind":"cherry_pick","hashes":["aa","bb"],"target":null}"#,
        ] {
            let op: Op = serde_json::from_str(json).unwrap();
            assert_eq!(serde_json::to_string(&op).unwrap(), json);
        }
        let reset: Op =
            serde_json::from_str(r#"{"kind":"reset","mode":"hard","target":"def"}"#).unwrap();
        assert!(matches!(reset, Op::Reset { mode: ResetMode::Hard, .. }));
        let pick: Op =
            serde_json::from_str(r#"{"kind":"cherry_pick","hashes":["aa","bb"]}"#).unwrap();
        let Op::CherryPick { hashes, .. } = &pick else {
            panic!("wrong variant");
        };
        assert_eq!(hashes.len(), 2);
    }

    // ── Stage 6: branches, remotes, tags ──────────────────────────────────

    #[test]
    fn branch_round_trips_through_serde() {
        let b = Branch {
            name: "origin/feature".into(),
            is_current: false,
            is_remote: true,
            upstream: None,
            ahead: 0,
            behind: 2,
            tip: "b".repeat(40),
        };
        let json = serde_json::to_string(&b).unwrap();
        assert!(json.contains(r#""origin/feature""#));
        assert!(json.contains(r#""is_remote":true"#));
        let back: Branch = serde_json::from_str(&json).unwrap();
        assert_eq!(back, b);
    }

    #[test]
    fn remote_round_trips_through_serde() {
        let r = Remote {
            name: "origin".into(),
            url: "git@host:org/repo.git".into(),
            fetch: Some("git@host:org/repo.git".into()),
            push: None,
        };
        let json = serde_json::to_string(&r).unwrap();
        let back: Remote = serde_json::from_str(&json).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn tag_round_trips_through_serde() {
        let t = Tag {
            name: "v1.0".into(),
            target: "c".repeat(40),
            annotated: true,
            message: Some("first release".into()),
        };
        let json = serde_json::to_string(&t).unwrap();
        assert!(json.contains(r#""annotated":true"#));
        let back: Tag = serde_json::from_str(&json).unwrap();
        assert_eq!(back, t);
    }

    #[test]
    fn ref_types_are_ts_types() {
        _assert_ts::<Branch>();
        _assert_ts::<Remote>();
        _assert_ts::<Tag>();
        _assert_ts::<Worktree>();
        _assert_ts::<RefLabel>();
    }
    #[test]
    fn worktree_round_trips_through_serde() {
        let w = Worktree {
            path: "/repos/demo-wt".into(),
            branch: Some("side".into()),
            head: "d".repeat(40),
            is_main: false,
            locked: true,
            prunable: false,
        };
        let json = serde_json::to_string(&w).unwrap();
        assert!(json.contains(r#""is_main":false"#));
        assert!(json.contains(r#""locked":true"#));
        let back: Worktree = serde_json::from_str(&json).unwrap();
        assert_eq!(back, w);
    }

    #[test]
    fn op_ref_variants_round_trip() {
        for json in [
            r#"{"kind":"branch_delete","name":"side","remote":null,"force":false}"#,
            r#"{"kind":"branch_delete","name":"side","remote":"origin","force":true}"#,
            r#"{"kind":"branch_rename","name":"side","new_name":"side2"}"#,
            r#"{"kind":"branch_set_upstream","branch":"side","remote":"origin","name":"main"}"#,
            r#"{"kind":"branch_set_upstream","branch":"side","remote":null,"name":""}"#,
            r#"{"kind":"remote_add","name":"upstream","url":"u"}"#,
            r#"{"kind":"remote_remove","name":"origin"}"#,
            r#"{"kind":"remote_fetch","remote":"origin","refspec":null}"#,
            r#"{"kind":"tag_create","name":"v3","target":"abc","annotated":true,"message":null}"#,
            r#"{"kind":"tag_checkout","name":"v1","force":false}"#,
            r#"{"kind":"tag_delete","name":"v1","remote":null}"#,
            r#"{"kind":"tag_push","remote":"origin","name":"v1","force":true}"#,
            r#"{"kind":"worktree_add","committish":"side","path":"/tmp/wt","new_branch":null}"#,
            r#"{"kind":"worktree_add","committish":"abc123","path":"/tmp/wt","new_branch":"feature"}"#,
            r#"{"kind":"worktree_remove","path":"/tmp/wt","force":true}"#,
        ] {
            let op: Op = serde_json::from_str(json).unwrap();
            assert_eq!(serde_json::to_string(&op).unwrap(), json, "round-trip: {json}");
        }
    }

    fn sample_commit_detail() -> CommitDetail {
        CommitDetail {
            hash: "a".repeat(40),
            subject: "fix: the thing".into(),
            message: "fix: the thing\n\nLonger body.\n".into(),
            author_name: "Test User".into(),
            author_email: "test@example.com".into(),
            author_date: 1_700_000_000,
            committer_name: "Test User".into(),
            committer_email: "test@example.com".into(),
            committer_date: 1_700_000_001,
            parents: vec!["b".repeat(40)],
            refs: vec![RefLabel {
                name: "main".into(),
                kind: RefKind::Branch,
                color: 0,
            }],
            is_head: true,
            stat: vec![FileStat {
                path: "a.txt".into(),
                additions: 2,
                deletions: 1,
                binary: false,
            }],
            diff: vec![FileDiff {
                path: "a.txt".into(),
                binary: false,
                patch: "-one\n+two\n+three\n".into(),
                hunks: vec![],
            }],
        }
    }

    #[test]
    fn commit_detail_round_trips_through_serde() {
        let d = sample_commit_detail();
        let json = serde_json::to_string(&d).unwrap();
        assert!(json.contains(r#""is_head":true"#));
        let back: CommitDetail = serde_json::from_str(&json).unwrap();
        assert_eq!(back, d);
    }

    #[test]
    fn commit_detail_types_are_ts_types() {
        _assert_ts::<CommitDetail>();
        _assert_ts::<FileStat>();
        _assert_ts::<FileDiff>();
        _assert_ts::<Hunk>();
        _assert_ts::<Author>();
        _assert_ts::<ResetMode>();
    }

    #[test]
    fn hunk_and_author_round_trip_through_serde() {
        let h = Hunk {
            header: "@@ -1,2 +1,3 @@".into(),
            lines: vec![" x".into(), "+y".into(), " z".into()],
            staged: false,
        };
        let json = serde_json::to_string(&h).unwrap();
        assert!(json.contains(r#""staged":false"#));
        let back: Hunk = serde_json::from_str(&json).unwrap();
        assert_eq!(back, h);

        let a = Author {
            name: "Jane".into(),
            email: "jane@example.com".into(),
        };
        let json = serde_json::to_string(&a).unwrap();
        let back: Author = serde_json::from_str(&json).unwrap();
        assert_eq!(back, a);
    }
}
