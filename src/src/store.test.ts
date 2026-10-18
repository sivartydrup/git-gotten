import { beforeEach, expect, test, vi } from "vitest";

const { openRepo, loadStatus } = vi.hoisted(() => ({
  openRepo: vi.fn(),
  loadStatus: vi.fn(),
}));

// Mock the IPC layer; per-test behavior is configured on the hoisted mocks.
vi.mock("./api", () => ({
  fetchBuildInfo: vi.fn().mockResolvedValue(null),
  openRepo,
  loadStatus,
  onStatusChanged: vi.fn().mockResolvedValue(null),
  planOp: vi.fn().mockResolvedValue(null),
  runOp: vi.fn().mockResolvedValue(null),
  loadLog: vi.fn().mockResolvedValue(null),
  onOpStarted: vi.fn().mockResolvedValue(null),
  onOpFinished: vi.fn().mockResolvedValue(null),
}));

import { useRepoStore } from "./store";
import type { RepoMeta, Status } from "./types";

const meta: RepoMeta = {
  id: "/repo/alpha",
  name: "alpha",
  path: "/repo/alpha",
  branch: "main",
  ahead: 1,
  behind: 0,
  dirty_count: 3,
};

const status: Status = {
  branch: "main",
  detached_head: null,
  ahead: 1,
  behind: 0,
  upstream: "origin/main",
  changes: [
    { path: "src/a.ts", kind: "Modified", staged: false, binary: false },
    { path: "src/b.ts", kind: "Added", staged: true, binary: false },
    { path: "tmp/scratch", kind: "Untracked", staged: false, binary: false },
  ],
  in_merge: false,
  in_rebase: false,
};

beforeEach(() => {
  useRepoStore.setState({
    repos: [],
    activeId: null,
    status: {},
    opening: false,
    error: null,
  });
  vi.clearAllMocks();
});

test("open registers the repo, activates it, and folds in its status", async () => {
  openRepo.mockResolvedValueOnce(meta);
  loadStatus.mockResolvedValueOnce(status);

  await useRepoStore.getState().open("/repo/alpha");

  const state = useRepoStore.getState();
  expect(openRepo).toHaveBeenCalledWith("/repo/alpha");
  expect(state.repos).toHaveLength(1);
  expect(state.activeId).toBe("/repo/alpha");
  expect(state.status["/repo/alpha"]).toBe(status);
  // The nav meta is synced from the fresh status.
  expect(state.repos[0].dirty_count).toBe(3);
  expect(state.error).toBeNull();
});

test("open deduplicates a repo that is already open", async () => {
  openRepo.mockResolvedValue(meta);
  loadStatus.mockResolvedValue(status);

  await useRepoStore.getState().open("/repo/alpha");
  await useRepoStore.getState().open("/repo/alpha");

  expect(useRepoStore.getState().repos).toHaveLength(1);
  expect(openRepo).toHaveBeenCalledTimes(2);
});

test("open outside Tauri (null) adds nothing and sets no error", async () => {
  openRepo.mockResolvedValueOnce(null);

  await useRepoStore.getState().open("/repo/alpha");

  const state = useRepoStore.getState();
  expect(state.repos).toHaveLength(0);
  expect(state.error).toBeNull();
});

test("open surfaces the IPC error for the user", async () => {
  openRepo.mockRejectedValueOnce(new Error("not a git repository: /nope"));

  await useRepoStore.getState().open("/nope");

  expect(useRepoStore.getState().error).toBe("not a git repository: /nope");
  expect(useRepoStore.getState().repos).toHaveLength(0);
});

test("applyStatus (a status_changed event) syncs the meta and the status record", async () => {
  openRepo.mockResolvedValueOnce(meta);
  loadStatus.mockResolvedValueOnce(status);
  await useRepoStore.getState().open("/repo/alpha");

  const fresh: Status = {
    ...status,
    branch: "fix/thing",
    changes: [status.changes[0]],
  };
  useRepoStore.getState().applyStatus({ repo: meta.id, status: fresh });

  const state = useRepoStore.getState();
  expect(state.status[meta.id]).toBe(fresh);
  expect(state.repos[0].branch).toBe("fix/thing");
  expect(state.repos[0].dirty_count).toBe(1);
});

test("select activates a repo and clears a stale error", async () => {
  openRepo.mockResolvedValueOnce(meta);
  loadStatus.mockResolvedValueOnce(status);
  await useRepoStore.getState().open("/repo/alpha");

  const other = { ...meta, id: "/repo/beta", name: "beta" };
  useRepoStore.setState({ repos: [meta, other], error: "boom" });

  useRepoStore.getState().select("/repo/beta");

  const state = useRepoStore.getState();
  expect(state.activeId).toBe("/repo/beta");
  expect(state.error).toBeNull();
});
