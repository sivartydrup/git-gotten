import { create } from "zustand";
import {
  fetchBuildInfo,
  loadCommit,
  loadGraph,
  loadLog,
  loadStatus,
  openRepo,
  planOp,
  runOp,
} from "./api";
import type {
  BuildInfo,
  CommandPlan,
  CommitDetail,
  Graph,
  LogEntry,
  Op,
  OpFinished,
  OpResult,
  OpStarted,
  RepoId,
  Risk,
  RepoMeta,
  Status,
  StatusChanged,
} from "./types";

interface BuildInfoState {
  info: BuildInfo | null;
  loading: boolean;
  error: string | null;
  load: () => Promise<void>;
}

/** Client state for the app shell (SPEC §5: Zustand). */
export const useBuildInfo = create<BuildInfoState>((set) => ({
  info: null,
  loading: false,
  error: null,
  load: async () => {
    set({ loading: true, error: null });
    try {
      const info = await fetchBuildInfo();
      set({ info, loading: false });
    } catch (e) {
      set({
        info: null,
        loading: false,
        error: e instanceof Error ? e.message : String(e),
      });
    }
  },
}));

interface RepoState {
  /** Open repositories, in the order they were opened. */
  repos: RepoMeta[];
  /** The repository the user is looking at (`null` = none). */
  activeId: RepoId | null;
  /** Latest known status per open repo (filled on open and on `status_changed`). */
  status: Partial<Record<RepoId, Status>>;
  /** True while an `openRepo` round-trip is in flight. */
  opening: boolean;
  /** Last open/IPC failure (transparency contract: the Rust error string). */
  error: string | null;
  /** Open a repository and make it active. */
  open: (path: string) => Promise<void>;
  /** Select a repository (or none) without re-reading its status. */
  select: (id: RepoId | null) => void;
  /** Fold a fresh status in: update the status record and sync the nav meta. */
  applyStatus: (payload: StatusChanged) => void;
  clearError: () => void;
}

/** Client state for the open-repo workspace (SPEC §7). */
export const useRepoStore = create<RepoState>((set, get) => ({
  repos: [],
  activeId: null,
  status: {},
  opening: false,
  error: null,

  open: async (path) => {
    const trimmed = path.trim();
    if (!trimmed || get().opening) {
      return;
    }
    set({ opening: true, error: null });
    try {
      const meta = await openRepo(trimmed);
      if (!meta) {
        // Not inside Tauri: nothing to open, no error to surface.
        set({ opening: false });
        return;
      }
      set((s) => ({
        opening: false,
        repos: upsertRepo(s.repos, meta),
        activeId: meta.id,
      }));
      const status = await loadStatus(meta.id);
      if (status) {
        get().applyStatus({ repo: meta.id, status });
      }
    } catch (e) {
      set({
        opening: false,
        error: e instanceof Error ? e.message : String(e),
      });
    }
  },

  select: (id) => set({ activeId: id, error: null }),

  applyStatus: ({ repo, status }) => {
    set((s) => ({
      status: { ...s.status, [repo]: status },
      repos: s.repos.map((m) =>
        m.id === repo
          ? {
              ...m,
              branch: status.branch,
              ahead: status.ahead,
              behind: status.behind,
              dirty_count: status.changes.length,
            }
          : m,
      ),
    }));
  },

  clearError: () => set({ error: null }),
}));

/** Insert or replace a repo in the nav list, preserving open order. */
function upsertRepo(repos: RepoMeta[], meta: RepoMeta): RepoMeta[] {
  const index = repos.findIndex((r) => r.id === meta.id);
  if (index === -1) {
    return [...repos, meta];
  }
  const next = [...repos];
  next[index] = meta;
  return next;
}

/** One plan awaiting user confirmation in the preview dialog (SPEC §9). */
interface PreviewState {
  repo: RepoId;
  op: Op;
  plan: CommandPlan;
}

interface ActivityState {
  /** Activity log entries, newest first (SPEC §11, bounded). */
  activities: LogEntry[];
  /** `true` while an op is in flight (`op_started` → `op_finished`). */
  running: boolean;
  /** Display line of the command currently running (live CmdHint state). */
  runningDisplay: string | null;
  /** Risk of the command currently running (from `op_started`). */
  runningRisk: Risk | null;
  /** The plan awaiting confirmation, or `null` when the dialog is closed. */
  preview: PreviewState | null;
  /** Last plan/run failure (transparency contract: the Rust error string). */
  error: string | null;
  /** `true` while a `plan_op` / `run_op` round-trip is in flight. */
  busy: boolean;
  /** Load the retained history into the ring (oldest→newest, reversed here). */
  loadHistory: (repo: RepoId | null) => Promise<void>;
  /** Compile `op` into its plan and open the preview dialog (SPEC §9). */
  requestPreview: (repo: RepoId, op: Op) => Promise<void>;
  /** Run the pending plan (confirm) and record the result. */
  confirmPreview: () => Promise<void>;
  /** Close the preview dialog without running. */
  dismissPreview: () => void;
  /** Record a finished op as a new activity entry. */
  record: (repo: RepoId, result: OpResult) => void;
  /** Fold an `op_started` event in (Rust → web, SPEC §13). */
  applyStarted: (payload: OpStarted) => void;
  /** Fold an `op_finished` event in (Rust → web, SPEC §13). */
  applyFinished: (payload: OpFinished) => void;
  clearError: () => void;
}

/** Maximum retained entries (mirrors the Rust ring, SPEC §11 "last 500"). */
const ACTIVITY_CAP = 500;

/** Monotonic id for web-recorded entries (Rust assigns ids to its own). */
let nextEntryId = 1;

/** Client state for the activity log + preview dialog (SPEC §11 / §9). */
export const useActivityStore = create<ActivityState>((set, get) => ({
  activities: [],
  running: false,
  runningDisplay: null,
  runningRisk: null,
  preview: null,
  error: null,
  busy: false,

  loadHistory: async (repo) => {
    try {
      const all = await loadLog();
      if (!all) {
        return; // not inside Tauri
      }
      const entries = repo ? all.filter((e) => e.repo === repo) : all;
      const reversed = [...entries].reverse(); // rust returns oldest first
      nextEntryId = all.reduce((max, e) => Math.max(max, e.id), 0) + 1;
      set({ activities: reversed.slice(0, ACTIVITY_CAP) });
    } catch (e) {
      set({ error: e instanceof Error ? e.message : String(e) });
    }
  },

  requestPreview: async (repo, op) => {
    if (get().busy) {
      return;
    }
    set({ busy: true, error: null, preview: null });
    try {
      const plan = await planOp(repo, op);
      if (!plan) {
        // Not inside Tauri: nothing to preview, no error to surface.
        set({ busy: false });
        return;
      }
      set({ busy: false, preview: { repo, op, plan } });
    } catch (e) {
      set({ busy: false, error: e instanceof Error ? e.message : String(e) });
    }
  },

  confirmPreview: async () => {
    const { preview, running, busy } = get();
    if (!preview || running || busy) {
      return;
    }
    set({ busy: true, error: null });
    try {
      const result = await runOp(preview.repo, preview.op);
      if (result) {
        get().record(preview.repo, result);
      }
      set({ preview: null, busy: false });
    } catch (e) {
      // Keep the dialog open: the user can retry or cancel.
      set({ busy: false, error: e instanceof Error ? e.message : String(e) });
    }
  },

  dismissPreview: () => set({ preview: null, error: null }),

  record: (repo, result) => {
    const entry: LogEntry = {
      id: nextEntryId++,
      repo,
      display: result.plan.display,
      ok: result.ok,
      duration_ms: result.duration_ms,
      ts: Date.now(),
      stdout: result.stdout,
      stderr: result.stderr,
    };
    set((s) => ({ activities: [entry, ...s.activities].slice(0, ACTIVITY_CAP) }));
  },

  applyStarted: ({ display, risk }) => {
    set({
      running: true,
      runningDisplay: display,
      runningRisk: risk,
      preview: null,
      error: null,
    });
  },

  applyFinished: (entry) => {
    // The `run_op` promise and this event carry the same Rust data; whichever
    // lands first wins, and the other is a no-op or a refresh of the newest
    // entry for that repo (keep the web-assigned id, take the authoritative
    // timing + output from the Rust record).
    set((s) => ({
      running: false,
      runningDisplay: null,
      runningRisk: null,
      activities: s.activities.map((a) =>
        a.repo === entry.repo && a.display === entry.display
          ? { ...a, ok: entry.ok, duration_ms: entry.duration_ms, ts: entry.ts, stdout: entry.stdout, stderr: entry.stderr }
          : a,
      ),
    }));
  },

  clearError: () => set({ error: null }),
}));

// ── Branching graph + inspect drawer (SPEC §10.1) ───────────────────────────

interface GraphState {
  /** The laid-out graph of the active repo (`null` = none / outside Tauri). */
  graph: Graph | null;
  /** True while a `loadGraph` round-trip is in flight. */
  loading: boolean;
  /** Last graph-load failure (the Rust error string). */
  error: string | null;
  /** The selected node's hash (`null` = none). */
  selected: string | null;
  /** The hovered node's hash (drives the tooltip). */
  hovered: string | null;
  /** The search box text; non-matching nodes render dimmed. */
  query: string;
  /** The inspect drawer's content for `selected`. */
  detail: CommitDetail | null;
  /** True while a `loadCommit` round-trip is in flight. */
  detailLoading: boolean;
  /** Last `loadCommit` failure. */
  detailError: string | null;
  /** Load the graph of `repo`, clearing the selection (SPEC §10.1). */
  load: (repo: RepoId) => Promise<void>;
  /** Select a node (and open the inspect drawer for it), or clear the selection. */
  select: (hash: string | null) => void;
  hover: (hash: string | null) => void;
  setQuery: (query: string) => void;
  closeDetail: () => void;
}

/** Client state for the branching graph + inspect drawer (SPEC §10.1). */
export const useGraphStore = create<GraphState>((set, get) => ({
  graph: null,
  loading: false,
  error: null,
  selected: null,
  hovered: null,
  query: "",
  detail: null,
  detailLoading: false,
  detailError: null,

  load: async (repo) => {
    set({
      loading: true,
      error: null,
      selected: null,
      detail: null,
      detailError: null,
    });
    try {
      const graph = await loadGraph(repo);
      if (!graph) {
        // Outside Tauri: nothing to draw, no error.
        set({ graph: null, loading: false });
        return;
      }
      set({ graph, loading: false });
    } catch (e) {
      set({
        graph: null,
        loading: false,
        error: e instanceof Error ? e.message : String(e),
      });
    }
  },

  select: (hash) => {
    set({ selected: hash, detail: null, detailError: null, detailLoading: false });
    if (!hash) {
      return;
    }
    const repo = useRepoStore.getState().activeId;
    if (!repo) {
      return;
    }
    set({ detailLoading: true });
    void loadCommit(repo, hash)
      .then((detail) => {
        if (get().selected !== hash) {
          return; // stale response (the selection moved on)
        }
        set({ detail, detailLoading: false });
      })
      .catch((e: unknown) => {
        if (get().selected !== hash) {
          return;
        }
        set({
          detail: null,
          detailLoading: false,
          detailError: e instanceof Error ? e.message : String(e),
        });
      });
  },

  hover: (hash) => set({ hovered: hash }),
  setQuery: (query) => set({ query }),
  closeDetail: () =>
    set({ selected: null, detail: null, detailError: null, detailLoading: false }),
}));
