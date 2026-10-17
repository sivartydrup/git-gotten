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
