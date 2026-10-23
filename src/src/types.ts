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

/** A commit identity (name + email) for the commit panel's author field. */
export interface Author {
  name: string;
  email: string;
}

/** A user operation the core compiles into a [`CommandPlan`] (Rust `Op`, SPEC §13).
 *
 * The web sends intent, never argv: risk is always derived by the core.
 * (serde `tag = "kind"`, `rename_all = "snake_case"`.
 */
export type Op =
  | {
      kind: "commit";
      message: string;
      amend: boolean;
      /** Override the author identity; omitted/`null` uses the configured identity. */
      author?: Author | null;
      /** Extra `Co-authored-by: Name <email>` trailer lines. */
      co_authors?: string[];
    }
  | { kind: "checkout"; target: string; force: boolean }
  | { kind: "push"; remote: string; branch: string | null; force: boolean }
  | { kind: "branch_create"; name: string; from: string; checkout: boolean }
  | { kind: "reset"; mode: "soft" | "mixed" | "hard"; target: string }
  | { kind: "cherry_pick"; hashes: string[] }
  | { kind: "stage"; path: string | null }
  | { kind: "unstage"; path: string | null }
  | { kind: "discard"; path: string }
  | { kind: "stage_hunks"; path: string; selected: number[] };

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

// ── Branching graph (Rust `git_gotten_protocol`, SPEC §10.1) ────────────────

/** What kind of ref a [`RefLabel`] is. */
export type RefKind = "Branch" | "Remote" | "Tag" | "Worktree";

/** A ref pinned to a graph node, rendered as a colored label. */
export interface RefLabel {
  name: string;
  kind: RefKind;
  /** Palette index of the line this ref sits on (see BRANCH_PALETTE). */
  color: number;
}

/** One span of a [`Line`] in a fixed lane; `from_row`..=`to_row` inclusive. */
export interface LineSegment {
  lane: number;
  from_row: number;
  to_row: number;
}

/** A line of history: a lane lineage with a stable color. */
export interface Line {
  id: number;
  /** Palette index into BRANCH_PALETTE. */
  color: number;
  /** Branch name at the line's opener, if one exists. */
  name: string | null;
  segments: LineSegment[];
}

/** One commit in the laid-out graph (one row, one lane). */
export interface Node {
  hash: string;
  subject: string;
  author: string;
  /** Author date, unix seconds. */
  date: number;
  /** Row (y index, 0 = newest). */
  row: number;
  /** Lane (x index, 0 = leftmost). */
  lane: number;
  /** True when HEAD points at this node. */
  is_head: boolean;
  refs: RefLabel[];
  /** The lines alive at this row, in lane order. */
  lines: number[];
}

/** A commit → parent edge (merge commits have ≥2). */
export interface Edge {
  from: string;
  to: string;
}

/** The laid-out commit graph for one repository (what the canvas draws). */
export interface Graph {
  nodes: Node[];
  edges: Edge[];
  lines: Line[];
}

/** Per-file change counts in a commit's diff. */
export interface FileStat {
  path: string;
  additions: number;
  deletions: number;
  binary: boolean;
}

/** One hunk of a unified diff (SPEC §10.2): a single `@@`-delimited block. */
export interface Hunk {
  /** The `@@ -l,c +l,c @@ [section]` header line. */
  header: string;
  /** The hunk body lines, each with its leading ` ` / `+` / `-` marker. */
  lines: string[];
  /** `true` for the index-vs-HEAD side, `false` for worktree-vs-index. */
  staged: boolean;
}

/** One file's unified diff (a commit's, or the working tree's, SPEC §10.2). */
export interface FileDiff {
  path: string;
  binary: boolean;
  patch: string;
  /** The hunks parsed from `patch` (empty for binary files). */
  hunks: Hunk[];
}

/** Full details of one commit, for the inspect drawer (Rust `CommitDetail`). */
export interface CommitDetail {
  hash: string;
  subject: string;
  message: string;
  author_name: string;
  author_email: string;
  /** Author date, unix seconds. */
  author_date: number;
  committer_name: string;
  committer_email: string;
  /** Committer date, unix seconds. */
  committer_date: number;
  parents: string[];
  refs: RefLabel[];
  is_head: boolean;
  stat: FileStat[];
  diff: FileDiff[];
}

/**
 * The default branch-lane palette (Rust `BRANCH_PALETTE`): a line's `color`
 * field is an index into this array, and the web maps indices to theme
 * tokens in light/dark (SPEC §12).
 */
export const BRANCH_PALETTE: readonly string[] = [
  "#4c8bf5", // 0  blue (mainline first)
  "#e05252", // 1  red
  "#58b368", // 2  green
  "#e89a3c", // 3  orange
  "#9b6bd4", // 4  purple
  "#4cc9c0", // 5  teal
  "#d65fa3", // 6  pink
  "#8e9c3c", // 7  olive
  "#5f6ef0", // 8  indigo
  "#c75c2e", // 9  vermillion
  "#2fb56b", // 10 emerald
  "#7e5bc0", // 11 violet
  "#b0893b", // 12 gold
  "#3fa8c4", // 13 cyan
  "#b4589e", // 14 fuchsia
  "#6fa332", // 15 lime
  "#9e4f4f", // 16 brick
  "#5472d6", // 17 royal blue
  "#8c6ba8", // 18 mauve
  "#4a9e8c", // 19 sea green
];

/** Map a palette index to a hex color (wraps past the palette's end). */
export function paletteColor(index: number): string {
  return BRANCH_PALETTE[index % BRANCH_PALETTE.length];
}
