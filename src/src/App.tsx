import { useEffect } from "react";
import { useActivityStore, useBuildInfo, useRepoStore } from "./store";
import { onOpFinished, onOpStarted, onStatusChanged } from "./api";
import { TopBar } from "./components/TopBar";
import { RepoNav } from "./components/RepoNav";
import { StatusView } from "./components/StatusView";
import { ActivityLog } from "./components/ActivityLog";
import { PreviewDialog } from "./components/PreviewDialog";

/**
 * GitGotten app shell.
 *
 * Stage 2: a top bar (brand + active repo state) over the workspace — the
 * open-repo nav on the left, the active repo's live status (with the first
 * mutating action: commit) on the right, and the activity log (the command
 * stream, SPEC §11) along the bottom. The Rust core keeps everything fresh:
 * `status_changed` for the read model, `op_started`/`op_finished` for the
 * command stream (SPEC §13).
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

  // Command-stream events (SPEC §13): drive the running state in the activity
  // panel. No-op outside Tauri.
  useEffect(() => {
    let unlistenStarted: (() => void) | null = null;
    let unlistenFinished: (() => void) | null = null;
    void onOpStarted((payload) => {
      useActivityStore.getState().applyStarted(payload);
    }).then((u) => {
      unlistenStarted = u;
    });
    void onOpFinished((payload) => {
      useActivityStore.getState().applyFinished(payload);
    }).then((u) => {
      unlistenFinished = u;
    });
    return () => {
      unlistenStarted?.();
      unlistenFinished?.();
    };
  }, []);

  // The activity log follows the active repo (its command stream).
  const activeId = useRepoStore((s) => s.activeId);
  useEffect(() => {
    void useActivityStore.getState().loadHistory(activeId);
  }, [activeId]);

  return (
    <main className="shell">
      <TopBar />
      <div className="workspace">
        <RepoNav />
        <StatusView />
      </div>
      <ActivityLog />
      <PreviewDialog />
    </main>
  );
}
