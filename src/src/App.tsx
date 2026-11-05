import { useEffect, useState } from "react";
import { useActivityStore, useBisectStore, useBuildInfo, useConflictStore, useGraphStore, useMainTab, useRebaseStore, useRefsStore, useRepoStore, useStashesStore, useStagingStore, useSubmodulesStore, useUpdaterStore, useViewStore, useWorktreesStore, useWorkspaceStore } from "./store";
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
import { Dashboard } from "./components/Dashboard";
import { UpdatePrompt } from "./components/UpdatePrompt";
import { CommandPalette, TAB_ORDER } from "./components/CommandPalette";
import { EmptyWorkspace } from "./components/EmptyWorkspace";
import { Icon } from "./components/Icons";
import "./components/command-palette.css";


/**
 * The Files tab (SPEC §10.2): the changed-files list and the selected file's
 * per-hunk diff.
 */
/** `true` when the keystroke belongs to a text control, not to the shell. */
function isTyping(target: EventTarget | null): boolean {
  const el = target as Element | null;
  if (!el || !("tagName" in el)) {
    return false;
  }
  const tag = el.tagName;
  return tag === "INPUT" || tag === "TEXTAREA" || tag === "SELECT";
}

/**
 * Dismisses the graph's right-click menu without owning it: GraphCanvas keeps
 * the menu in its own state and clears it whenever a contextmenu lands on a spot
 * with no commit, so Escape fires exactly that, off-canvas (a pointerdown would
 * leave a half-started pan drag behind).
 * shortcut: relies on `.graph-context-menu` / `.graph-canvas` staying GraphCanvas's
 * markup; move this into GraphCanvas if it ever owns an Escape key of its own.
 */
function closeGraphMenu(): boolean {
  if (!document.querySelector(".graph-context-menu")) {
    return false;
  }
  document
    .querySelector(".graph-canvas")
    ?.dispatchEvent(
      new MouseEvent("contextmenu", { bubbles: true, clientX: -1, clientY: -1 }),
    );
  return true;
}

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
  const openRepos = useRepoStore((s) => s.repos.length);
  const [paletteOpen, setPaletteOpen] = useState(false);
  // Screen-reader announcement of the last finished op (never a visual duplicate
  // of the activity log: the region is visually hidden).
  const [announcement, setAnnouncement] = useState("");

  useEffect(() => {
    void load();
  }, [load]);

  // Update feed (SPEC §17): once at launch, then every 6h while the app runs.
  useEffect(() => {
    void useUpdaterStore.getState().check();
    const timer = setInterval(() => void useUpdaterStore.getState().check(), 6 * 3600_000);
    return () => clearInterval(timer);
  }, []);

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
      setAnnouncement(
        payload.ok ? `Completed: ${payload.display}` : `Failed: ${payload.display}`,
      );
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

  // Hydrate the persisted workspace tree + every repo's live status (SPEC §7, §11).
  useEffect(() => {
    void useWorkspaceStore.getState().load();
  }, []);

  // Keyboard layer. Keystrokes that belong to a text control are left to that
  // control (typing a commit message must never switch tabs or grab the filter);
  // only the two command-key bindings work while typing.
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      const mod = e.metaKey || e.ctrlKey;
      if (mod && (e.key === "k" || e.key === "K")) {
        e.preventDefault();
        setPaletteOpen((open) => !open);
        return;
      }
      if (mod && e.key === "Enter") {
        // Submit the commit only from the commit box, and only through the
        // commit button — i.e. through the preview/confirm flow (SPEC §9).
        const box = document.activeElement;
        if (box instanceof HTMLElement && box.classList.contains("commit-message")) {
          const commitButton = document.querySelector<HTMLButtonElement>(
            ".commit-actions button.primary:not([disabled])",
          );
          if (commitButton) {
            e.preventDefault();
            commitButton.click();
          }
        }
        return;
      }
      if (e.key === "Escape") {
        // First match wins: drawer, then graph menu, then palette. Dialogs own
        // their own Escape (useDialog stops the event before it reaches here).
        if (useGraphStore.getState().selected !== null) {
          useGraphStore.getState().closeDetail();
          return;
        }
        if (closeGraphMenu()) {
          return;
        }
        setPaletteOpen(false);
        return;
      }
      // Everything else belongs to whatever has focus: typing a commit message
      // must not switch tabs or grab the filter.
      if (isTyping(e.target)) {
        return;
      }
      if (e.key === "/") {
        e.preventDefault();
        document.querySelector<HTMLInputElement>(".topbar-search")?.focus();
        return;
      }
      if (/^[1-7]$/.test(e.key)) {
        const next = TAB_ORDER[Number(e.key) - 1];
        if (next) {
          useMainTab.getState().setTab(next);
        }
      }
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  const dashboard = useViewStore((s) => s.dashboard);

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
      <UpdatePrompt />
      <div className="workspace">
        <RepoNav />
        {dashboard ? (
          <Dashboard />
        ) : (
        <div className="main">
          {openRepos === 0 && !activeId ? (
            <EmptyWorkspace />
          ) : (
          <>
          <div className="main-tabs">
            <button
              type="button"
              role="tab"
              aria-selected={tab === "graph"}
              className={`tab ${tab === "graph" ? "active" : ""}`}
              onClick={() => setTab("graph")}
            >
              <Icon name="graph" size={14} /> Graph
            </button>
            <button
              type="button"
              role="tab"
              aria-selected={tab === "files"}
              className={`tab ${tab === "files" ? "active" : ""}`}
              onClick={() => setTab("files")}
            >
              <Icon name="files" size={14} /> Files
            </button>
            <button
              type="button"
              role="tab"
              aria-selected={tab === "conflicts"}
              className={`tab ${tab === "conflicts" ? "active" : ""}`}
              onClick={() => setTab("conflicts")}
            >
              <Icon name="conflict" size={14} /> Conflicts{conflictedCount > 0 && (
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
              <Icon name="branch" size={14} /> Branches
            </button>
            <button
              type="button"
              role="tab"
              aria-selected={tab === "worktrees"}
              className={`tab ${tab === "worktrees" ? "active" : ""}`}
              onClick={() => setTab("worktrees")}
            >
              <Icon name="worktree" size={14} /> Worktrees
            </button>
            <button
              type="button"
              role="tab"
              aria-selected={tab === "stash"}
              className={`tab ${tab === "stash" ? "active" : ""}`}
              onClick={() => setTab("stash")}
            >
              <Icon name="stash" size={14} /> Stash
            </button>
            <button
              type="button"
              role="tab"
              aria-selected={tab === "bisect"}
              className={`tab ${tab === "bisect" ? "active" : ""}`}
              onClick={() => setTab("bisect")}
            >
              <Icon name="bisect" size={14} /> Bisect
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
                      <Icon name="branch" size={13} /> {t[0].toUpperCase() + t.slice(1)}
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
          </>
          )}
        </div>
        )}
        <InspectDrawer />
      </div>
      <ActivityLog />
      {paletteOpen && <CommandPalette onClose={() => setPaletteOpen(false)} />}
      <PreviewDialog />
      <RebaseDialog />
      <p className="gg-sr-only" aria-live="polite">
        {announcement}
      </p>
    </main>
  );
}
