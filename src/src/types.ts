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
  /** Full OID (hex) of HEAD (branch tip or detached); `null` on an unborn branch. */
  head: string | null;
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
  /** True while a cherry-pick is in progress (`.git/CHERRY_PICK_HEAD` present). */
  in_cherry_pick: boolean;
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
// ── Refs: branches, remotes, tags (Rust `git_gotten_protocol`, SPEC §10.5) ──

/** A branch (local or remote-tracking), with its sync state (SPEC §10.5). */
export interface Branch {
  /** Display name: `main`, or `origin/feature` for a remote-tracking branch. */
  name: string;
  /** `true` when HEAD is on this branch (always `false` for remote branches). */
  is_current: boolean;
  /** `true` for a remote-tracking branch (`refs/remotes/…`). */
  is_remote: boolean;
  /** The branch's upstream (e.g. `origin/main`); `null` when untracked. */
  upstream: string | null;
  /** Commits on the branch that are not on its upstream (0 when untracked). */
  ahead: number;
  /** Commits on the upstream that are not on the branch (0 when untracked). */
  behind: number;
  /** The branch's tip commit (full hex oid). */
  tip: string;
}

/** A remote with its fetch/push urls (SPEC §10.5). */
export interface Remote {
  /** The remote's name (e.g. `origin`). */
  name: string;
  /** The url to display: the fetch url, falling back to the push url. */
  url: string;
  /** The fetch url (`remote.<name>.url`); `null` when only a push url is set. */
  fetch: string | null;
  /** The push url (`remote.<name>.pushurl`); `null` when unset. */
  push: string | null;
}

/** A tag, with its resolved target (SPEC §10.5). */
export interface Tag {
  /** The tag name (e.g. `v1.0`). */
  name: string;
  /** The commit the tag points at (annotated tags are peeled to their commit). */
  target: string;
  /** `true` for an annotated tag (a tag object); `false` for a lightweight one. */
  annotated: boolean;
  /** The tag object's message (annotated tags); `null` for lightweight tags. */
  message: string | null;
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
  /** Optional text piped to the process's stdin; `null` for steps that take no input. */
  input: string | null;
  /** The program to spawn when this step is not a git invocation (the
   * external-editor hand-off); `null` for all git steps. */
  program: string | null;
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

/** The action an interactive-rebase todo takes on one commit (Rust `RebaseAction`, SPEC §10.4). */
export type RebaseAction = "Pick" | "Reword" | "Squash" | "Fixup" | "Edit" | "Drop";

/** One entry of an edited interactive-rebase todo (Rust `RebaseEdit`): UI order, newest commit first. */
export interface RebaseEdit {
  /** The commit this edit applies to (full hex oid). */
  hash: string;
  action: RebaseAction;
  /** The new message for `Reword`/`Squash` (single line); `null` keeps git's own. */
  message: string | null;
}

/** One commit an interactive rebase onto `target` would rewrite (the `load_rebase_commits` read, Rust `RebaseCommit`). */
export interface RebaseCommit {
  hash: string;
  subject: string;
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
  | {
      kind: "push";
      remote: string;
      branch: string | null;
      force: boolean;
      /** Record the remote branch as the push upstream (`-u`). */
      set_upstream: boolean;
    }
  | { kind: "pull"; rebase: boolean }
  | { kind: "merge"; source: string }
  | { kind: "branch_create"; name: string; from: string; checkout: boolean }
  | { kind: "branch_delete"; name: string; remote: string | null; force: boolean }
  | { kind: "branch_rename"; name: string; new_name: string }
  | { kind: "branch_set_upstream"; branch: string; remote: string | null; name: string }
  | { kind: "remote_add"; name: string; url: string }
  | { kind: "remote_remove"; name: string }
  | { kind: "remote_fetch"; remote: string; refspec: string | null }
  | { kind: "reset"; mode: "soft" | "mixed" | "hard"; target: string }
  | { kind: "cherry_pick"; hashes: string[]; target: string | null }
  | { kind: "rebase"; target: string; edits: RebaseEdit[] }
  | { kind: "rebase_onto"; target: string }
  | { kind: "stage"; path: string | null }
  | { kind: "unstage"; path: string | null }
  | { kind: "discard"; path: string }
  | { kind: "stage_hunks"; path: string; selected: number[] }
  | { kind: "tag_create"; name: string; target: string; annotated: boolean; message: string | null }
  | { kind: "tag_checkout"; name: string; force: boolean }
  | { kind: "tag_delete"; name: string; remote: string | null }
  | { kind: "tag_push"; remote: string; name: string; force: boolean }
  | { kind: "resolve_conflict"; path: string; resolution: ConflictResolution }
  | { kind: "continue_op" }
  | { kind: "abort_op" }
  | { kind: "open_editor"; paths: string[] };

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
  /**
   * The conflicted file paths when the plan stopped on a merge/rebase/
   * cherry-pick conflict (`[]` otherwise, SPEC §10.5).
   */
  conflicts: string[];
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
  /** The conflicted file paths (empty when the op ran clean). */
  conflicts: string[];
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

// ── Conflict resolution (Rust `git_gotten_protocol`, SPEC §10.5) ──────────

/** The three versions of one conflict region (index stages 1 / 2 / 3). */
export interface ConflictHunk {
  /** The base (common ancestor, stage 1) content of the region. */
  base: string;
  /** The ours (stage 2, HEAD) content of the region. */
  ours: string;
  /** The theirs (stage 3, the incoming ref) content of the region. */
  theirs: string;
}

/** One span of a file's 3-way diff: unchanged text, or a conflict region. */
export type ConflictSegment =
  | { Common: string }
  | { Conflict: ConflictHunk };

/** Which side a whole-file resolution takes. */
export type ConflictSide = "Ours" | "Theirs";

/** How the user resolves one conflict region. */
export type HunkChoice =
  | "AcceptOurs"
  | "AcceptTheirs"
  | "AcceptBoth"
  | { Custom: string };

/** How one conflicted file is resolved (the input of `resolve_conflict`). */
export type ConflictResolution =
  | { Hunks: HunkChoice[] }
  | { Side: ConflictSide };

/** The full 3-way view of one conflicted file (the `load_conflict` read). */
export interface ConflictFile {
  /** The file path (repo-relative). */
  path: string;
  /** `true` when the content is binary: the text fields and segments are
   * empty, and only a whole-side resolution is offered. */
  binary: boolean;
  /** The full base (common ancestor) version of the file; empty for binary. */
  base: string;
  /** The full ours (HEAD) version of the file; empty for binary. */
  ours: string;
  /** The full theirs (incoming) version of the file; empty for binary. */
  theirs: string;
  /** The parsed 3-way structure in file order; the `Conflict` segments'
   * order is the `HunkChoice` index space. */
  segments: ConflictSegment[];
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
