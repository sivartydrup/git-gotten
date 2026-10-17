import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { BuildInfo, RepoId, RepoMeta, Status, StatusChanged } from "./types";

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
