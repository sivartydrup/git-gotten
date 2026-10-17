import { create } from "zustand";
import { fetchBuildInfo, loadStatus, openRepo } from "./api";
import type { BuildInfo, RepoId, RepoMeta, Status, StatusChanged } from "./types";

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
