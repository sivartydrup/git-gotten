import { useEffect, useState } from "react";
import { useActivityStore, useBisectStore, useBuildInfo, useConflictStore, useGraphStore, useMainTab, useRebaseStore, useRefsStore, useRepoStore, useStashesStore, useStagingStore, useSubmodulesStore, useWorktreesStore } from "./store";
import { onOpFinished, onOpStarted, onStatusChanged } from "./api";
import { TopBar } from "./components/TopBar";
import { RepoNav } from "./components/RepoNav";
import { ActivityLog } from "./components/ActivityLog";
import { PreviewDialog } from "./components/PreviewDialog";
import { RebaseDialog } from "./components/RebaseDialog";
import { GraphCanvas } from "./components/GraphCanvas";
import { InspectDrawer } from "./components/InspectDrawer";
import { FilesPanel } from "./components/FilesPanel";
import { FileDiff } from "./components/FileDiff";
import { CommitPanel } from "./components/CommitPanel";
import { RefsPanel, type RefsTab } from "./components/RefsPanel";
import { ConflictView } from "./components/ConflictView";
import { ConflictBanner } from "./components/ConflictBanner";
import { WorktreesPanel } from "./components/WorktreesPanel";
import { StashPanel } from "./components/StashPanel";
import { BisectPanel } from "./components/BisectPanel";


/**
 * The Files tab (SPEC §10.2): the changed-files list and the selected file's
 * per-hunk diff.
 */
function FilesTab() {
  return (
    <div className="files-tab">
      <FilesPanel />
      <FileDiff />
    </div>
  );
}

/**
 * GitGotten app shell.
 *
 * Stage 4: a top bar (brand + active repo state) over the workspace — the
 * open-repo nav on the left, the branching graph (SPEC §10.1) in the main
 * area, the inspect drawer on the right, and the activity log (the command
 * stream, SPEC §11) along the bottom. Stage 5 adds the Files tab (per-hunk
 * staging, SPEC §10.2) and the docked commit panel. The Rust core keeps
 * everything fresh: `status_changed` for the read model,
 * `op_started`/`op_finished` for the command stream (SPEC §13) — a finished
 * op also reloads the graph and the staged file's diff.
 */
export default function App() {
  const { load } = useBuildInfo();
  const { tab, setTab } = useMainTab();
  const [refsTab, setRefsTab] = useState<RefsTab>("branches");
  const conflictedCount = useConflictStore((s) => s.files.length);

  useEffect(() => {
    void load();
  }, [load]);

  // Live status updates from the Rust core (SPEC §13). No-op outside Tauri.
  useEffect(() => {
    let unlisten: (() => void) | null = null;
    void onStatusChanged((payload) => {
      useRepoStore.getState().applyStatus(payload);
      // The conflicted-file list (and the 3-way selection) follows the
      // live status: resolved files drop out, new ones appear.
      useConflictStore.getState().applyStatus(payload.status);
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
      const activeId = useRepoStore.getState().activeId;
      if (payload.repo === activeId) {
        void useGraphStore.getState().load(activeId);
        // The op may have changed the index/worktree: the staged file's diff
        // (hunk states) must be re-read.
        useStagingStore.getState().reload();
        // The op may have resolved (or opened) unmerged files: re-read the
        // shown file's 3-way view.
        void useConflictStore.getState().reload();
        // The op may have changed refs (branch/remote/tag): re-read them so
        // the tabs and the graph's ref labels stay live.
        void useRefsStore.getState().load(activeId);
        // Worktree add/remove rewrites the checkout registry.
        void useWorktreesStore.getState().load(activeId);
        // Stash push/apply/pop/drop rewrites the stash list.
        void useStashesStore.getState().load(activeId);
        // Submodule init/update moves the gitlinks.
        void useSubmodulesStore.getState().load(activeId);
        // Start/verdict/test/reset all move the bisect state.
        void useBisectStore.getState().load(activeId);
      }
    }).then((u) => {
      unlistenFinished = u;
    });
    return () => {
      unlistenStarted?.();
      unlistenFinished?.();
    };
  }, []);

  const activeId = useRepoStore((s) => s.activeId);
  // The branching graph follows the active repo and reloads after a finished
  // op (the graph may have changed: branch, reset, cherry-pick, …).
  useEffect(() => {
    if (activeId) {
      void useGraphStore.getState().load(activeId);
      void useRefsStore.getState().load(activeId);
      void useWorktreesStore.getState().load(activeId);
      void useStashesStore.getState().load(activeId);
      void useSubmodulesStore.getState().load(activeId);
      void useBisectStore.getState().load(activeId);
    } else {
      useGraphStore.getState().closeDetail();
    }
    // The staging selection is per-repo: drop it when the repo changes.
    useStagingStore.getState().reset();
    // Same for the conflict selection (the unmerged list re-derives from the
    // live status of the newly active repo).
    useConflictStore.getState().reset();
    const status = activeId ? (useRepoStore.getState().status[activeId] ?? null) : null;
    if (status) {
      useConflictStore.getState().applyStatus(status);
    }
  }, [activeId]);
  // The activity log follows the active repo (its command stream).
  useEffect(() => {
    void useActivityStore.getState().loadHistory(activeId);
  }, [activeId]);
  // The rebase dialog follows the active repo (its commit list is per-repo).
  useEffect(() => {
    useRebaseStore.getState().reset();
  }, [activeId]);

  return (
    <main className="shell">
      <TopBar />
      <ConflictBanner />
      <div className="workspace">
        <RepoNav />
        <div className="main">
          <div className="main-tabs">
            <button
              type="button"
              role="tab"
              aria-selected={tab === "graph"}
              className={`tab ${tab === "graph" ? "active" : ""}`}
              onClick={() => setTab("graph")}
            >
              Graph
            </button>
            <button
              type="button"
              role="tab"
              aria-selected={tab === "files"}
              className={`tab ${tab === "files" ? "active" : ""}`}
              onClick={() => setTab("files")}
            >
              Files
            </button>
            <button
              type="button"
              role="tab"
              aria-selected={tab === "conflicts"}
              className={`tab ${tab === "conflicts" ? "active" : ""}`}
              onClick={() => setTab("conflicts")}
            >
              Conflicts{conflictedCount > 0 && (
                <span className="count">({conflictedCount})</span>
              )}
            </button>
            <button
              type="button"
              role="tab"
              aria-selected={tab === "branches"}
              className={`tab ${tab === "branches" ? "active" : ""}`}
              onClick={() => setTab("branches")}
            >
              Branches
            </button>
            <button
              type="button"
              role="tab"
              aria-selected={tab === "worktrees"}
              className={`tab ${tab === "worktrees" ? "active" : ""}`}
              onClick={() => setTab("worktrees")}
            >
              Worktrees
            </button>
            <button
              type="button"
              role="tab"
              aria-selected={tab === "stash"}
              className={`tab ${tab === "stash" ? "active" : ""}`}
              onClick={() => setTab("stash")}
            >
              Stash
            </button>
            <button
              type="button"
              role="tab"
              aria-selected={tab === "bisect"}
              className={`tab ${tab === "bisect" ? "active" : ""}`}
              onClick={() => setTab("bisect")}
            >
              Bisect
            </button>
          </div>
          <div className="main-content">
            {tab === "graph" && <GraphCanvas />}
            {tab === "files" && <FilesTab />}
            {tab === "conflicts" && <ConflictView />}
            {tab === "branches" && (
              <div className="refs-tab">
                <div className="refs-subtabs">
                  {(["branches", "remotes", "tags"] as const).map((t) => (
                    <button
                      key={t}
                      type="button"
                      role="tab"
                      aria-selected={refsTab === t}
                      className={`tab ${refsTab === t ? "active" : ""}`}
                      onClick={() => setRefsTab(t)}
                    >
                      {t[0].toUpperCase() + t.slice(1)}
                    </button>
                  ))}
                </div>
                <RefsPanel tab={refsTab} />
              </div>
            )}
            {tab === "worktrees" && <WorktreesPanel />}
            {tab === "stash" && <StashPanel />}
            {tab === "bisect" && <BisectPanel />}
          </div>
          <CommitPanel />
        </div>
        <InspectDrawer />
      </div>
      <ActivityLog />
      <PreviewDialog />
      <RebaseDialog />
    </main>
  );
}
