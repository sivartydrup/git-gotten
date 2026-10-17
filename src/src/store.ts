import { create } from "zustand";
import { fetchBuildInfo } from "./api";
import type { BuildInfo } from "./types";

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
