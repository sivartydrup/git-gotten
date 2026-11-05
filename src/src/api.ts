import { invoke, isTauri } from "@tauri-apps/api/core";
import { check as checkUpdater, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { listen } from "@tauri-apps/api/event";
import type {
  BisectState,
  Branch,
  BuildInfo,
  CommandPlan,
  CommitDetail,
  CommitStat,
  ConflictFile,
  FileDiff,
  Graph,
  LogEntry,
  Op,
  OpFinished,
  OpResult,
  OpStarted,
  RebaseCommit,
  Remote,
  RepoId,
  RepoMeta,
  RepoSelection,
  Status,
  StatusChanged,
  Stash,
  Submodule,
  Tag,
  Worktree,
  Workspace,
  WorkspaceRepo,
} from "./types";

/**
 * Thin wrapper over the Rust IPC bridge.
 *
 * Guarding on `isTauri()` keeps the same bundle runnable in a plain browser
 * (`pnpm dev`) with no Tauri webview: the call becomes a no-op that resolves
 * `null` instead of throwing. Callers treat `null` as "not inside Tauri".
 */

/** The app's build/runtime identity (Rust `build_info` command). */
export async function fetchBuildInfo(): Promise<BuildInfo | null> {
  if (!isTauri()) {
    return null;
  }
  return await invoke<BuildInfo>("build_info");
}

/**
 * Open a repository at `path` (Rust `open_repo` command).
 * Returns the nav summary, or `null` outside Tauri.
 */
export async function openRepo(path: string): Promise<RepoMeta | null> {
  if (!isTauri()) {
    return null;
  }
  return await invoke<RepoMeta>("open_repo", { path });
}

/**
 * Ask the OS for a directory through Tauri's native folder picker (the nav's Browse
 * button). `null` means the user cancelled — or that no native dialog is available
 * because this bundle is running in a plain browser.
 */
export async function pickRepositoryFolder(defaultPath?: string): Promise<string | null> {
  if (!isTauri()) {
    return null;
  }
  const picked = await openDialog({
    directory: true,
    title: "Choose a repository folder",
    defaultPath,
  });
  return typeof picked === "string" ? picked : null;
}

/**
 * Compute the live status of an open repository (Rust `load_status` command).
 * Returns the status, or `null` outside Tauri.
 */
export async function loadStatus(repo: RepoId): Promise<Status | null> {
  if (!isTauri()) {
    return null;
  }
  return await invoke<Status>("load_status", { repo });
}

/**
 * Load the laid-out commit graph of an open repository (Rust `load_graph`
 * command, SPEC §10.1). Returns the graph, or `null` outside Tauri.
 */
export async function loadGraph(repo: RepoId): Promise<Graph | null> {
  if (!isTauri()) {
    return null;
  }
  return await invoke<Graph>("load_graph", { repo });
}

/**
 * Load the full details of one commit (Rust `load_commit` command, the
 * inspect drawer, SPEC §10.1). Returns the details, or `null` outside Tauri.
 */
export async function loadCommit(
  repo: RepoId,
  hash: string,
): Promise<CommitDetail | null> {
  if (!isTauri()) {
    return null;
  }
  return await invoke<CommitDetail>("load_commit", { repo, hash });
}

/**
 * Load the working-tree diff of one file (Rust `load_diff` command, the
 * staging UI, SPEC §10.2). Returns the diff with its hunks, or `null`
 * outside Tauri.
 */
export async function loadDiff(
  repo: RepoId,
  path: string,
): Promise<FileDiff | null> {
  if (!isTauri()) {
    return null;
  }
  return await invoke<FileDiff>("load_diff", { repo, path });
}

/**
 * Lines changed for a bounded set of commits (Rust `load_commit_stats`
 * command, the graph's diff weighting and `+N/−N` column). Requests are capped
 * server-side at 200 hashes; commits the repository no longer has are simply
 * absent from the result. Returns `[]` outside Tauri.
 */
export async function loadCommitStats(
  repo: RepoId,
  hashes: string[],
): Promise<CommitStat[]> {
  if (!isTauri()) {
    return [];
  }
  return await invoke<CommitStat[]>("load_commit_stats", { repo, hashes });
}

/**
 * Load the full 3-way view of one conflicted file (Rust `load_conflict`
 * command, the conflict editor, SPEC §10.5): the three side versions plus
 * the parsed diff3 segments. Returns `null` outside Tauri.
 */
export async function loadConflict(
  repo: RepoId,
  path: string,
): Promise<ConflictFile | null> {
  if (!isTauri()) {
    return null;
  }
  return await invoke<ConflictFile>("load_conflict", { repo, path });
}
/**
 * Load the branches of an open repository (Rust `load_branches` command,
 * SPEC §10.5). Returns the list, or `null` outside Tauri.
 */
export async function loadBranches(repo: RepoId): Promise<Branch[] | null> {
  if (!isTauri()) {
    return null;
  }
  return await invoke<Branch[]>("load_branches", { repo });
}

/**
 * Load the remotes of an open repository (Rust `load_remotes` command,
 * SPEC §10.5). Returns the list, or `null` outside Tauri.
 */
export async function loadRemotes(repo: RepoId): Promise<Remote[] | null> {
  if (!isTauri()) {
    return null;
  }
  return await invoke<Remote[]>("load_remotes", { repo });
}

/**
 * Load the tags of an open repository (Rust `load_tags` command,
 * SPEC §10.5). Returns the list, or `null` outside Tauri.
 */
export async function loadTags(repo: RepoId): Promise<Tag[] | null> {
  if (!isTauri()) {
    return null;
  }
  return await invoke<Tag[]>("load_tags", { repo });
}

/**
 * Load the worktrees of an open repository — main first (Rust `load_worktrees`
 * command, SPEC §10.7). Returns the list, or `null` outside Tauri.
 */
export async function loadWorktrees(repo: RepoId): Promise<Worktree[] | null> {
  if (!isTauri()) {
    return null;
  }
  return await invoke<Worktree[]>("load_worktrees", { repo });
}

/**
 * Load the commits an interactive rebase onto `target` would rewrite (Rust
 * `load_rebase_commits` command, the RebaseDialog list, SPEC §10.4). Newest
 * first, UI order. Returns the list, or `null` outside Tauri.
 */
export async function loadRebaseCommits(
  repo: RepoId,
  target: string,
): Promise<RebaseCommit[] | null> {
  if (!isTauri()) {
    return null;
  }
  return await invoke<RebaseCommit[]>("load_rebase_commits", { repo, target });
}

/**
 * Load the stash list of an open repository, newest first (Rust
 * `load_stashes` command, the Stash panel, SPEC §10.6). Returns the list, or
 * `null` outside Tauri.
 */
export async function loadStashes(repo: RepoId): Promise<Stash[] | null> {
  if (!isTauri()) {
    return null;
  }
  return await invoke<Stash[]>("load_stashes", { repo });
}

/**
 * Load the submodules of an open repository (Rust `load_submodules` command,
 * the Files tab submodule section, SPEC §10.8). Returns the list, or `null`
 * outside Tauri.
 */
export async function loadSubmodules(repo: RepoId): Promise<Submodule[] | null> {
  if (!isTauri()) {
    return null;
  }
  return await invoke<Submodule[]>("load_submodules", { repo });
}

/**
 * Load the bisect state of an open repository (Rust `load_bisect` command,
 * the Bisect panel, SPEC §10.9). Returns the state, or `null` outside Tauri
 * (the store treats `null` as the inactive state).
 */
export async function loadBisect(repo: RepoId): Promise<BisectState | null> {
  if (!isTauri()) {
    return null;
  }
  return await invoke<BisectState>("load_bisect", { repo });
}

/**
 * Run a bisect test command and let the core post the matching
 * `git bisect good`/`bad` for its exit code (Rust `bisect_test` command,
 * SPEC §10.9). This is the one Stage 11 flow that does not go through
 * plan_op/run_op: it runs the test and the verdict in one round-trip and
 * returns the resulting [`BisectState`], or `null` outside Tauri.
 */
export async function bisectTest(
  repo: RepoId,
  command: string,
): Promise<BisectState | null> {
  if (!isTauri()) {
    return null;
  }
  return await invoke<BisectState>("bisect_test", { repo, command });
}

/**
 * Subscribe to the Rust `status_changed` event (SPEC §13) and forward each
 * payload to `cb`. Returns an unlisten function, or `null` outside Tauri.
 */
export async function onStatusChanged(
  cb: (payload: StatusChanged) => void,
): Promise<(() => void) | null> {
  if (!isTauri()) {
    return null;
  }
  const unlisten = await listen<StatusChanged>("status_changed", (event) => {
    cb(event.payload);
  });
  return () => {
    unlisten();
  };
}

/**
 * Compile `op` into its visible [`CommandPlan`] (Rust `plan_op` command) without
 * running it. Returns `null` outside Tauri.
 */
export async function planOp(
  repo: RepoId,
  op: Op,
): Promise<CommandPlan | null> {
  if (!isTauri()) {
    return null;
  }
  return await invoke<CommandPlan>("plan_op", { repo, op });
}

/**
 * Run an op (Rust `run_op` command): the core re-plans from `op`, executes the
 * exact git command (serialized per repo), and returns the result — including
 * captured output. Returns `null` outside Tauri.
 */
export async function runOp(
  repo: RepoId,
  op: Op,
): Promise<OpResult | null> {
  if (!isTauri()) {
    return null;
  }
  return await invoke<OpResult>("run_op", { repo, op });
}

/**
 * Read the session's activity log (Rust `load_log` command), oldest first.
 * Returns `null` outside Tauri.
 */
export async function loadLog(): Promise<LogEntry[] | null> {
  if (!isTauri()) {
    return null;
  }
  return await invoke<LogEntry[]>("load_log");
}

/**
 * Subscribe to the Rust `op_started` event (SPEC §13): an op is beginning.
 * Returns an unlisten function, or `null` outside Tauri.
 */
export async function onOpStarted(
  cb: (payload: OpStarted) => void,
): Promise<(() => void) | null> {
  if (!isTauri()) {
    return null;
  }
  const unlisten = await listen<OpStarted>("op_started", (event) => {
    cb(event.payload);
  });
  return () => {
    unlisten();
  };
}

/**
 * Subscribe to the Rust `op_finished` event (SPEC §13): an op completed.
 * Returns an unlisten function, or `null` outside Tauri.
 */
export async function onOpFinished(
  cb: (payload: OpFinished) => void,
): Promise<(() => void) | null> {
  if (!isTauri()) {
    return null;
  }
  const unlisten = await listen<OpFinished>("op_finished", (event) => {
    cb(event.payload);
  });
  return () => {
    unlisten();
  };
}

// ── Stage 12: workspace commands (SPEC §7, §13) ────────────────────────────

/**
 * The persisted workspace tree (Rust `workspace_get`). Returns `null` outside Tauri.
 */
export async function workspaceGet(): Promise<Workspace | null> {
  if (!isTauri()) {
    return null;
  }
  return await invoke<Workspace>("workspace_get");
}

/**
 * Every open repo in the tree as (meta, live status) (Rust `workspace_repos`) —
 * the startup hydration for nav + dashboard. Returns `null` outside Tauri.
 */
export async function workspaceRepos(): Promise<WorkspaceRepo[] | null> {
  if (!isTauri()) {
    return null;
  }
  return await invoke<WorkspaceRepo[]>("workspace_repos");
}

/**
 * Replace the workspace tree (Rust `workspace_set`): the web commits every
 * drag-and-drop reorder/move through here. Returns `null` outside Tauri.
 */
export async function workspaceSet(workspace: Workspace): Promise<Workspace | null> {
  if (!isTauri()) {
    return null;
  }
  return await invoke<Workspace>("workspace_set", { workspace });
}

/** Add a group under `parent` (root when null) (Rust `group_add`). */
export async function groupAdd(
  name: string,
  parent: string | null,
): Promise<Workspace | null> {
  if (!isTauri()) {
    return null;
  }
  return await invoke<Workspace>("group_add", { name, parent });
}

/** Rename a group (Rust `group_rename`). */
export async function groupRename(
  id: string,
  name: string,
): Promise<Workspace | null> {
  if (!isTauri()) {
    return null;
  }
  return await invoke<Workspace>("group_rename", { id, name });
}

/** Delete a group; its repos hoist to the root (Rust `group_delete`). */
export async function groupDelete(id: string): Promise<Workspace | null> {
  if (!isTauri()) {
    return null;
  }
  return await invoke<Workspace>("group_delete", { id });
}

/** Pin or unpin a repo (Rust `repo_pin`). */
export async function repoPin(
  id: string,
  pinned: boolean,
): Promise<Workspace | null> {
  if (!isTauri()) {
    return null;
  }
  return await invoke<Workspace>("repo_pin", { id, pinned });
}

/** Record repo activity for the recents ordering (Rust `repo_touch`). */
export async function repoTouch(id: string): Promise<Workspace | null> {
  if (!isTauri()) {
    return null;
  }
  return await invoke<Workspace>("repo_touch", { id });
}

/** Unlink a repo from the workspace; the on-disk repo is never touched (Rust `remove_repo_from_workspace`). */
export async function removeRepoFromWorkspace(id: string): Promise<Workspace | null> {
  if (!isTauri()) {
    return null;
  }
  return await invoke<Workspace>("remove_repo_from_workspace", { id });
}

// ── Stage 15: per-repo state (SPEC §16) ────────────────────────────────────

/** The commit/branch the user last selected in a repo (Rust `load_repo_selection`). */
export async function loadRepoSelection(repo: RepoId): Promise<RepoSelection | null> {
  if (!isTauri()) {
    return null;
  }
  return await invoke<RepoSelection>("load_repo_selection", { repo });
}

/** Store the last selection of a repo (Rust `save_repo_selection`). */
export async function saveRepoSelection(
  repo: RepoId,
  selection: RepoSelection,
): Promise<RepoSelection | null> {
  if (!isTauri()) {
    return null;
  }
  return await invoke<RepoSelection>("save_repo_selection", { repo, selection });
}

// ── Stage 14: auto-updater (SPEC §17) ──────────────────────────────────────

/**
 * The update the feed announced on the last check. `installUpdate` needs the
 * plugin's own handle (it carries the download URL + signature), so the
 * wrapper keeps it rather than handing it to the store.
 */
let announced: Update | null = null;

/**
 * Ask the update feed once (plugin `check`). Returns the announced release,
 * or `null` when the app is up to date / outside Tauri. Rejects when the
 * feed is unreachable or the manifest is unreadable.
 */
export async function checkForUpdate(): Promise<{ version: string; notes: string | null } | null> {
  if (!isTauri()) {
    return null;
  }
  announced = await checkUpdater();
  if (!announced) {
    return null;
  }
  return { version: announced.version, notes: announced.body ?? null };
}

/**
 * Download, verify and install the update announced by the last check, then
 * relaunch into it. Throws with the plugin's own message on any failure
 * (the store shows it verbatim, per the transparency contract).
 */
export async function installUpdate(): Promise<void> {
  if (!announced) {
    throw new Error("no update has been announced by the last check");
  }
  await announced.downloadAndInstall();
  announced = null;
  await relaunch();
}
