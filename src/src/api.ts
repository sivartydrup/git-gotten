import { invoke, isTauri } from "@tauri-apps/api/core";
import type { BuildInfo } from "./types";

/**
 * Thin wrapper over the Rust IPC bridge.
 *
 * Guarding on `isTauri()` keeps the same bundle runnable in a plain browser
 * (`pnpm dev`) with no Tauri webview: the call becomes a no-op that resolves
 * `null` instead of throwing. Callers treat `null` as "not inside Tauri".
 */
export async function fetchBuildInfo(): Promise<BuildInfo | null> {
  if (!isTauri()) {
    return null;
  }
  return await invoke<BuildInfo>("build_info");
}
