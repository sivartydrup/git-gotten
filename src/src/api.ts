import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type {
  BuildInfo,
  CommandPlan,
  LogEntry,
  Op,
  OpFinished,
  OpResult,
  OpStarted,
  RepoId,
  RepoMeta,
  Status,
  StatusChanged,
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
