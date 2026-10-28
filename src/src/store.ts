import { create } from "zustand";
import {
  fetchBuildInfo,
  loadBranches,
  loadCommit,
  loadConflict,
  loadDiff,
  loadGraph,
  loadLog,
  loadRebaseCommits,
  loadRemotes,
  loadStatus,
  loadTags,
  openRepo,
  planOp,
  runOp,
} from "./api";
import type {
  Branch,
  BuildInfo,
  CommandPlan,
  CommitDetail,
  ConflictFile,
  FileDiff,
  Graph,
  HunkChoice,
  LogEntry,
  Op,
  OpFinished,
  OpResult,
  OpStarted,
  RebaseCommit,
  Remote,
  RepoId,
  Risk,
  RepoMeta,
  Status,
  StatusChanged,
  Tag,
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
// ── Main content tab (SPEC §7) ──────────────────────────────────────────────

/** The main content tab of the shell. */
export type MainTab = "graph" | "files" | "branches" | "conflicts";

interface MainTabState {
  tab: MainTab;
  setTab: (tab: MainTab) => void;
}

/** Client state for the main content tab (SPEC §7). */
export const useMainTab = create<MainTabState>((set) => ({
  tab: "graph",
  setTab: (tab) => set({ tab }),
}));


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
      conflicts: result.conflicts,
    };
    // A conflicted op (merge/rebase/cherry-pick) routes the user to the
    // Conflicts tab, where the 3-way resolution UI lives (SPEC §10.5).
    if (result.conflicts.length > 0) {
      useMainTab.getState().setTab("conflicts");
    }
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
          ? { ...a, ok: entry.ok, duration_ms: entry.duration_ms, ts: entry.ts, stdout: entry.stdout, stderr: entry.stderr, conflicts: entry.conflicts }
          : a,
      ),
    }));
    if (entry.conflicts.length > 0) {
      useMainTab.getState().setTab("conflicts");
    }
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
// ── Refs: branches, remotes, tags (SPEC §10.5) ──────────────────────────────

interface RefsState {
  /** The active repo's branches (`null`-safe: `[]` when none / outside Tauri). */
  branches: Branch[];
  /** The active repo's remotes. */
  remotes: Remote[];
  /** The active repo's tags. */
  tags: Tag[];
  /** True while the three `load*` round-trips are in flight. */
  loading: boolean;
  /** Last ref-load failure (the Rust error string). */
  error: string | null;
  /** Load the branches, remotes, and tags of `repo` (SPEC §10.5). */
  load: (repo: RepoId) => Promise<void>;
}

/** Client state for the refs tabs (SPEC §10.5). */
export const useRefsStore = create<RefsState>((set) => ({
  branches: [],
  remotes: [],
  tags: [],
  loading: false,
  error: null,

  load: async (repo) => {
    set({ loading: true, error: null });
    try {
      const [branches, remotes, tags] = await Promise.all([
        loadBranches(repo),
        loadRemotes(repo),
        loadTags(repo),
      ]);
      if (!branches && !remotes && !tags) {
        // Outside Tauri: nothing to list, no error.
        set({ branches: [], remotes: [], tags: [], loading: false });
        return;
      }
      set({
        branches: branches ?? [],
        remotes: remotes ?? [],
        tags: tags ?? [],
        loading: false,
      });
    } catch (e) {
      set({
        branches: [],
        remotes: [],
        tags: [],
        loading: false,
        error: e instanceof Error ? e.message : String(e),
      });
    }
  },
}));

// ── Interactive rebase: commit list + RebaseDialog (SPEC §10.4) ────────────

interface RebaseState {
  /** `true` while the RebaseDialog is open. */
  open: boolean;
  /** The commits the rebase would rewrite (newest first, UI order). */
  commits: RebaseCommit[];
  /** True while a `loadRebaseCommits` round-trip is in flight. */
  loading: boolean;
  /** Last `loadRebaseCommits` failure (the Rust error string). */
  error: string | null;
  /** Open the RebaseDialog. */
  show: () => void;
  /** Load the commits `target..HEAD` (SPEC §10.4). */
  load: (repo: RepoId, target: string) => Promise<void>;
  /** Close the dialog and drop the loaded list. */
  reset: () => void;
}

/** Client state for the interactive-rebase flow (SPEC §10.4). */
export const useRebaseStore = create<RebaseState>((set) => ({
  open: false,
  commits: [],
  loading: false,
  error: null,

  show: () => set({ open: true }),

  load: async (repo, target) => {
    set({ loading: true, error: null });
    try {
      const commits = await loadRebaseCommits(repo, target);
      if (!commits) {
        // Outside Tauri: nothing to list, no error.
        set({ commits: [], loading: false });
        return;
      }
      set({ commits, loading: false });
    } catch (e) {
      set({
        commits: [],
        loading: false,
        error: e instanceof Error ? e.message : String(e),
      });
    }
  },

  reset: () => set({ open: false, commits: [], loading: false, error: null }),
}));

// ── Staging: file list + per-file diff (SPEC §10.2) ─────────────────────────

interface StagingState {
  /** The file whose diff is shown in the Files tab (`null` = none selected). */
  file: string | null;
  /** The working-tree diff of `file` (staged hunks first, then unstaged). */
  diff: FileDiff | null;
  /** True while a `loadDiff` round-trip is in flight. */
  loading: boolean;
  /** Last `loadDiff` failure (the Rust error string). */
  error: string | null;
  /** Select a file and load its diff; `null` clears the selection. */
  selectFile: (path: string | null) => void;
  /** Re-read the diff of the selected file (after an op mutates the tree). */
  reload: () => void;
  /** Drop the selection and diff (the active repo changed). */
  reset: () => void;
  /** One `loadDiff` round-trip; stale responses are dropped. */
  load: (path: string) => Promise<void>;
}

/** Client state for the staging UI (the Files tab, SPEC §10.2). */
export const useStagingStore = create<StagingState>((set, get) => ({
  file: null,
  diff: null,
  loading: false,
  error: null,

  load: async (path) => {
    set({ loading: true, error: null });
    try {
      const repo = useRepoStore.getState().activeId;
      const diff = repo ? await loadDiff(repo, path) : null;
      if (get().file !== path) {
        return; // stale response: the selection moved on
      }
      set({ diff, loading: false });
    } catch (e) {
      if (get().file !== path) {
        return;
      }
      set({
        diff: null,
        loading: false,
        error: e instanceof Error ? e.message : String(e),
      });
    }
  },

  selectFile: (path) => {
    set({ file: path, diff: null, error: null, loading: false });
    if (path) {
      void get().load(path);
    }
  },

  reload: () => {
    const { file } = get();
    if (file) {
      void get().load(file);
    }
  },

  reset: () => set({ file: null, diff: null, loading: false, error: null }),
}));

// ── Conflict resolution: 3-way view + per-region choices (SPEC §10.5) ─────

interface ConflictState {
  /** The unmerged paths of the active repo (live, from the status). */
  files: string[];
  /** The unmerged count at the start of the current operation — the
   * denominator of the banner's "k of N resolved" progress. */
  total: number;
  /** The path whose 3-way view is shown (`null` = none selected). */
  file: string | null;
  /** The 3-way view of `file` (from `load_conflict`). */
  conflict: ConflictFile | null;
  /** The per-region choices for the shown file, in the order of its
   * `Conflict` segments. Every region pre-selects "ours" (the UI default)
   * until the user picks otherwise. */
  choices: HunkChoice[];
  /** `true` while a `load_conflict` round-trip is in flight. */
  loading: boolean;
  /** Last `load_conflict` failure (the Rust error string). */
  error: string | null;
  /** Sync the unmerged list (and the "k of N" baseline) from a live status. */
  applyStatus: (status: Status) => void;
  /** Select a file and load its 3-way view; `null` clears the selection. */
  selectFile: (path: string | null) => void;
  /** Record one region's choice (region index into the `Conflict` segments). */
  setChoice: (index: number, choice: HunkChoice) => void;
  /** Read `path`'s 3-way view into the shown-file state. */
  load: (path: string) => Promise<void>;
  /** Re-read the shown file's 3-way view (after an op mutates the tree). */
  reload: () => Promise<void>;
  /** Drop the selection and view (the active repo changed). */
  reset: () => void;
}

/** Client state for the conflict-resolution UI (the Conflicts tab, SPEC §10.5). */
export const useConflictStore = create<ConflictState>((set, get) => ({
  files: [],
  total: 0,
  file: null,
  conflict: null,
  choices: [],
  loading: false,
  error: null,

  applyStatus: (status) => {
    const unmerged = status.changes
      .filter((c) => c.kind === "Conflicted")
      .map((c) => c.path);
    const { file, total } = get();
    // Baseline for "k of N": the first non-empty observation (and every
    // later growth) is what the banner counts down from.
    const newTotal = Math.max(total, unmerged.length);
    if (file && !unmerged.includes(file)) {
      // The shown file was resolved (in-app or externally): clear it.
      set({ files: unmerged, total: newTotal, file: null, conflict: null, choices: [] });
      return;
    }
    set({ files: unmerged, total: newTotal });
  },

  selectFile: (path) => {
    set({ file: path, conflict: null, choices: [], error: null, loading: false });
    if (path) {
      void get().load(path);
    }
  },

  setChoice: (index, choice) => {
    const { choices } = get();
    if (index < 0 || index >= choices.length) {
      return;
    }
    const next = choices.slice();
    next[index] = choice;
    set({ choices: next });
  },

  load: async (path) => {
    set({ loading: true, error: null });
    try {
      const repo = useRepoStore.getState().activeId;
      const conflict = repo ? await loadConflict(repo, path) : null;
      if (get().file !== path) {
        return; // stale response: the selection moved on
      }
      const regions = conflict
        ? conflict.segments.filter((s) => "Conflict" in s).length
        : 0;
      // Every region starts "ours" until the user decides per region.
      const choices: HunkChoice[] = Array.from(
        { length: regions },
        () => "AcceptOurs"
      );
      set({ conflict, choices, loading: false });
    } catch (e) {
      if (get().file !== path) {
        return;
      }
      set({
        conflict: null,
        choices: [],
        loading: false,
        error: e instanceof Error ? e.message : String(e),
      });
    }
  },

  reload: async () => {
    const { file } = get();
    if (file) {
      await get().load(file);
    }
  },

  reset: () =>
    set({ files: [], total: 0, file: null, conflict: null, choices: [], loading: false, error: null }),
}));
