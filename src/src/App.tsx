import { useEffect } from "react";
import { useBuildInfo, useRepoStore } from "./store";
import { onStatusChanged } from "./api";
import { TopBar } from "./components/TopBar";
import { RepoNav } from "./components/RepoNav";
import { StatusView } from "./components/StatusView";

/**
 * GitGotten app shell.
 *
 * Stage 1: a top bar (brand + active repo state) over the workspace — the
 * open-repo nav on the left, the active repo's live status on the right. The
 * Rust file watcher keeps the status fresh via `status_changed` (SPEC §13).
 */
export default function App() {
  const { load } = useBuildInfo();

  useEffect(() => {
    void load();
  }, [load]);

  // Live status updates from the Rust core (SPEC §13). No-op outside Tauri.
  useEffect(() => {
    let unlisten: (() => void) | null = null;
    void onStatusChanged((payload) => {
      useRepoStore.getState().applyStatus(payload);
    }).then((u) => {
      unlisten = u;
    });
    return () => {
      unlisten?.();
    };
  }, []);

  return (
    <main className="shell">
      <TopBar />
      <div className="workspace">
        <RepoNav />
        <StatusView />
      </div>
    </main>
  );
}
