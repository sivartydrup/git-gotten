/**
 * Web-side mirrors of the shared Rust types in `crates/protocol` (SPEC §5).
 *
 * Hand-written to match the Rust shapes exactly; once `ts-rs` generation is wired,
 * these become the generated output in `gen/` and this file is removed.
 */

export interface BuildInfo {
  name: string;
  version: string;
  arch: string;
}

/** Stable identity of an open repository: its canonical absolute path. */
export type RepoId = string;

/** What changed for one file, as reported by the git status machinery. */
export type ChangeKind =
  | "Modified"
  | "Added"
  | "Deleted"
  | "Renamed"
  | "Untracked"
  | "Conflicted";

/** One changed file in a repository status. */
export interface FileChange {
  /** Repo-relative display path (for renames: the *new* name). */
  path: string;
  kind: ChangeKind;
  /** `true` for index-vs-HEAD (staged) changes, `false` for worktree-vs-index. */
  staged: boolean;
  /** True when the content is binary; the UI shows a "binary" placeholder. */
  binary: boolean;
}

/** Live status of an open repository (SPEC §8 read model, computed by `gix`). */
export interface Status {
  /** Short name of the checked-out branch; `null` when detached. */
  branch: string | null;
  /** Full OID (hex) of HEAD when detached; `null` when a branch is checked out. */
  detached_head: string | null;
  /** Commits on `branch` that are not on `upstream`. */
  ahead: number;
  /** Commits on `upstream` that are not on `branch`. */
  behind: number;
  /** Upstream remote branch name (e.g. `origin/main`); `null` when untracked. */
  upstream: string | null;
  /** All changed files (staged, unstaged, untracked, conflicted). */
  changes: FileChange[];
  /** True while a merge is in progress (`.git/MERGE_HEAD` present). */
  in_merge: boolean;
  /** True while a rebase is in progress (`.git/rebase-merge` or `rebase-apply`). */
  in_rebase: boolean;
}

/** Quick summary of an open repo, for the workspace nav (SPEC §7). */
export interface RepoMeta {
  /** Canonical absolute path (also the `RepoId`). */
  id: RepoId;
  /** Folder basename (display name in the nav). */
  name: string;
  /** Canonical absolute path of the repository root. */
  path: string;
  /** Checked-out branch (or `null` when detached). */
  branch: string | null;
  ahead: number;
  behind: number;
  /** Number of changed files (staged + unstaged + untracked). */
  dirty_count: number;
}

/** Payload of the `status_changed` event (Rust → web, SPEC §13). */
export interface StatusChanged {
  repo: RepoId;
  status: Status;
}

/** Risk classification of a git step or operation (Rust `Risk`, SPEC §8).
 *
 * Ordered by severity: a plan's effective risk is its *maximum* step risk, and the
 * confirmation tier (SPEC §9) is selected from it — `ReadOnly` runs silently,
 * `Mutating` previews + confirms, `Dangerous` requires type-to-confirm.
 */
export type Risk = "ReadOnly" | "Mutating" | "Dangerous";

/** One literal git invocation inside a [`CommandPlan`] (Rust `GitStep`).
 *
 * `argv` is the exact spawned-process argument list (`argv[0]` is the `git`
 * binary) — always a vector, no shell, no string interpolation (SPEC §16).
 */
export interface GitStep {
  argv: string[];
  /** One human-readable sentence of what this step does. */
  summary: string;
  risk: Risk;
  /** Structured blast radius when the step affects history; `null` for most steps. */
  effect: string | null;
}

/** What will change before a risky op runs (Rust `Preview`, SPEC §8). */
export interface Preview {
  /** One-line summary of the effect (rendered large in the dialog). */
  headline: string;
  /** Detail lines: affected files, commits to be pushed, warnings. */
  lines: string[];
  /** `true` for `Dangerous`: the user must type to confirm (SPEC §9). */
  requires_type_to_confirm: boolean;
}

/** A compiled, visible representation of an operation (Rust `CommandPlan`, SPEC §8).
 *
 * The `display` string drives the inline hint, the preview dialog, and the
 * activity-log entry — one source of truth.
 */
export interface CommandPlan {
  /** Steps to run in order; the first failing step aborts the rest. */
  steps: GitStep[];
  /** The exact command(s) as shown in the UI. */
  display: string;
  /** Present for `Mutating`/`Dangerous` plans; drives the preview dialog. */
  preview: Preview | null;
}

/** A user operation the core compiles into a [`CommandPlan`] (Rust `Op`, SPEC §13).
 *
 * The web sends intent, never argv: risk is always derived by the core.
 * (serde `tag = "kind"`, `rename_all = "snake_case"`.
 */
export type Op =
  | { kind: "commit"; message: string; amend: boolean }
  | { kind: "checkout"; target: string; force: boolean }
  | { kind: "push"; remote: string; branch: string | null; force: boolean };

/** The result of running a [`CommandPlan`] (Rust `OpResult`, safe failure, SPEC §8). */
export interface OpResult {
  /** The plan that was run (so the UI shows exactly what ran). */
  plan: CommandPlan;
  /** `true` when every step exited 0. */
  ok: boolean;
  /** Exit code of the last step that ran (`-1` when a step could not be spawned). */
  exit: number;
  /** Captured stdout of all steps that ran. */
  stdout: string;
  /** Captured stderr of all steps that ran. */
  stderr: string;
  /** Wall-clock duration of the whole plan, in milliseconds. */
  duration_ms: number;
}

/** One entry in the activity log (Rust `LogEntry`, SPEC §9). */
export interface LogEntry {
  /** Monotonic id within this app session (1-based). */
  id: number;
  repo: RepoId;
  /** The exact command (the plan's `display` string). */
  display: string;
  /** `true` when the op succeeded. */
  ok: boolean;
  /** Duration in milliseconds. */
  duration_ms: number;
  /** Unix time, in milliseconds. */
  ts: number;
  /** Captured stdout (shown in the collapsible detail view). */
  stdout: string;
  /** Captured stderr. */
  stderr: string;
}

/** Payload of the `op_started` event (Rust `OpStarted`, SPEC §13). */
export interface OpStarted {
  repo: RepoId;
  /** The exact command being started (the plan's `display` string). */
  display: string;
  /** The plan's maximum step risk. */
  risk: Risk;
}

/** Payload of the `op_finished` event: the recorded [`LogEntry`] (Rust, SPEC §13). */
export type OpFinished = LogEntry;
