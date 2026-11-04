import { beforeEach, expect, test, vi } from "vitest";

const { openRepo, loadStatus, loadGraph, loadCommit, loadRepoSelection, saveRepoSelection } =
  vi.hoisted(() => ({
    openRepo: vi.fn(),
    loadStatus: vi.fn(),
    loadGraph: vi.fn(),
    loadCommit: vi.fn(),
    loadRepoSelection: vi.fn().mockResolvedValue(null),
    saveRepoSelection: vi.fn().mockResolvedValue(null),
  }));

// Mock the IPC layer; per-test behavior is configured on the hoisted mocks.
vi.mock("./api", () => ({
  workspaceGet: async () => null,
  workspaceRepos: async () => null,
  workspaceSet: async () => null,
  groupAdd: async () => null,
  groupRename: async () => null,
  groupDelete: async () => null,
  repoPin: async () => null,
  repoTouch: async () => null,
  removeRepoFromWorkspace: async () => null,
  loadRebaseCommits: vi.fn().mockResolvedValue(null),
  fetchBuildInfo: vi.fn().mockResolvedValue(null),
  openRepo,
  loadStatus,
  onStatusChanged: vi.fn().mockResolvedValue(null),
  planOp: vi.fn().mockResolvedValue(null),
  runOp: vi.fn().mockResolvedValue(null),
  loadLog: vi.fn().mockResolvedValue(null),
  onOpStarted: vi.fn().mockResolvedValue(null),
  onOpFinished: vi.fn().mockResolvedValue(null),
  loadGraph,
  loadCommit,
  loadRepoSelection,
  saveRepoSelection,
  loadStashes: async () => [],
  loadSubmodules: async () => [],
  loadBisect: async () => ({ active: false, current: null, good: [], bad: [], remaining: 0, found: null, log: [] }),
  bisectTest: async () => ({ active: false, current: null, good: [], bad: [], remaining: 0, found: null, log: [] }),
}));

import { useActivityStore, useGraphStore, useMainTab, useRepoStore } from "./store";
import type { CommitDetail, Graph, RepoMeta, Status } from "./types";

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
  head: null,
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
  in_cherry_pick: false,
};

beforeEach(() => {
  useRepoStore.setState({
    repos: [],
    activeId: null,
    status: {},
    opening: false,
    error: null,
  });
  useGraphStore.setState({
    graph: null,
    loading: false,
    error: null,
    selected: null,
    hovered: null,
    query: "",
    detail: null,
    detailLoading: false,
    detailError: null,
  });
  useActivityStore.setState({
    activities: [],
    running: false,
    runningDisplay: null,
    runningRisk: null,
    preview: null,
    error: null,
    busy: false,
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

/** Flush the microtask queue deterministically (no wall-clock timers). */
async function flush(): Promise<void> {
  // Two hops: the mocked `loadCommit` promise resolves, then its `.then` handler runs.
  await Promise.resolve();
  await Promise.resolve();
}

const graphFixture: Graph = {
  nodes: [
    {
      hash: "a1b2c3",
      subject: "tip",
      author: "Al",
      date: 1700000000,
      row: 0,
      lane: 0,
      is_head: true,
      refs: [{ name: "main", kind: "Branch", color: 0 }],
      lines: [0],
    },
    {
      hash: "d4e5f6",
      subject: "mid",
      author: "Bo",
      date: 1690000000,
      row: 1,
      lane: 0,
      is_head: false,
      refs: [],
      lines: [0],
    },
    {
      hash: "a7b8c9",
      subject: "base",
      author: "Cy",
      date: 1680000000,
      row: 2,
      lane: 0,
      is_head: false,
      refs: [],
      lines: [0],
    },
  ],
  edges: [
    { from: "a1b2c3", to: "d4e5f6" },
    { from: "d4e5f6", to: "a7b8c9" },
  ],
  lines: [
    { id: 0, color: 0, name: "main", segments: [{ lane: 0, from_row: 0, to_row: 2 }] },
  ],
};

const detailFixture: CommitDetail = {
  hash: "a1b2c3",
  subject: "tip",
  message: "tip\n\nbody",
  author_name: "Al",
  author_email: "al@example.com",
  author_date: 1700000000,
  committer_name: "Al",
  committer_email: "al@example.com",
  committer_date: 1700000000,
  parents: ["d4e5f6"],
  refs: [{ name: "main", kind: "Branch", color: 0 }],
  is_head: true,
  stat: [{ path: "a.txt", additions: 1, deletions: 0, binary: false }],
  diff: [{ path: "a.txt", binary: false, patch: "+one", hunks: [] }],
};

test("graph load stores the laid-out graph and clears stale selections", async () => {
  loadGraph.mockResolvedValueOnce(graphFixture);
  useGraphStore.setState({ selected: "stale", error: "boom", detail: detailFixture });
  await useGraphStore.getState().load("/repo/alpha");
  const s = useGraphStore.getState();
  expect(loadGraph).toHaveBeenCalledWith("/repo/alpha");
  expect(s.graph).toBe(graphFixture);
  expect(s.loading).toBe(false);
  expect(s.selected).toBeNull();
  expect(s.error).toBeNull();
  expect(s.detail).toBeNull();
});

test("graph load keeps the query and surfaces the IPC error", async () => {
  loadGraph.mockRejectedValueOnce(new Error("nope"));
  useGraphStore.getState().setQuery("fix");
  await useGraphStore.getState().load("/repo/alpha");
  const s = useGraphStore.getState();
  expect(s.graph).toBeNull();
  expect(s.error).toBe("nope");
  expect(s.query).toBe("fix");
});

test("graph select opens the drawer with the loaded detail", async () => {
  useRepoStore.setState({ activeId: "/repo/alpha" });
  loadCommit.mockResolvedValueOnce(detailFixture);
  useGraphStore.getState().select("a1b2c3");
  await flush();
  const s = useGraphStore.getState();
  expect(loadCommit).toHaveBeenCalledWith("/repo/alpha", "a1b2c3");
  expect(s.detail).toBe(detailFixture);
  expect(s.detailLoading).toBe(false);
});

test("graph select(null) clears the selection and the drawer", () => {
  useGraphStore.setState({
    selected: "x",
    detail: detailFixture,
    detailLoading: true,
  });
  useGraphStore.getState().select(null);
  const s = useGraphStore.getState();
  expect(s.selected).toBeNull();
  expect(s.detail).toBeNull();
  expect(s.detailLoading).toBe(false);
});

test("graph select outside an open repo stores no detail", async () => {
  useRepoStore.setState({ activeId: null });
  useGraphStore.getState().select("a1b2c3");
  await flush();
  const s = useGraphStore.getState();
  expect(loadCommit).not.toHaveBeenCalled();
  expect(s.selected).toBe("a1b2c3");
  expect(s.detail).toBeNull();
  expect(s.detailLoading).toBe(false);
});

// ── Stage 15: the stored selection (SPEC §16) ──────────────────────────────

test("graph load reopens the selection stored for the repository", async () => {
  useRepoStore.setState({ activeId: "/repo/alpha" });
  loadGraph.mockResolvedValueOnce(graphFixture);
  loadRepoSelection.mockResolvedValueOnce({ commit: "d4e5f6", branch: null });
  loadCommit.mockResolvedValueOnce({ ...detailFixture, hash: "d4e5f6" });

  await useGraphStore.getState().load("/repo/alpha");
  await flush();

  const s = useGraphStore.getState();
  expect(loadRepoSelection).toHaveBeenCalledWith("/repo/alpha");
  expect(s.selected).toBe("d4e5f6");
  expect(loadCommit).toHaveBeenCalledWith("/repo/alpha", "d4e5f6");
});

test("a stored commit that no longer exists falls back to its branch tip", async () => {
  useRepoStore.setState({ activeId: "/repo/alpha" });
  loadGraph.mockResolvedValueOnce(graphFixture);
  loadRepoSelection.mockResolvedValueOnce({ commit: "rewritten", branch: "main" });
  loadCommit.mockResolvedValueOnce(detailFixture);

  await useGraphStore.getState().load("/repo/alpha");
  await flush();

  const s = useGraphStore.getState();
  expect(s.selected).toBe("a1b2c3"); // `main`'s tip in the fixture
  expect(loadCommit).toHaveBeenCalledWith("/repo/alpha", "a1b2c3");
});

test("a stored selection nothing can resolve to restores nothing", async () => {
  useRepoStore.setState({ activeId: "/repo/alpha" });
  loadGraph.mockResolvedValueOnce(graphFixture);
  loadRepoSelection.mockResolvedValueOnce({ commit: "gone", branch: "deleted-branch" });

  await useGraphStore.getState().load("/repo/alpha");
  await flush();

  const s = useGraphStore.getState();
  expect(s.selected).toBeNull();
  expect(s.detail).toBeNull();
  expect(loadCommit).not.toHaveBeenCalled();
});

test("selecting stores the commit with its branch; dismissing stores an empty one", async () => {
  useRepoStore.setState({ activeId: "/repo/alpha" });
  useGraphStore.setState({ graph: graphFixture });
  loadCommit.mockResolvedValueOnce(detailFixture);

  useGraphStore.getState().select("a1b2c3");
  expect(saveRepoSelection).toHaveBeenCalledWith("/repo/alpha", {
    commit: "a1b2c3",
    branch: "main",
  });

  useGraphStore.getState().closeDetail();
  expect(saveRepoSelection).toHaveBeenCalledWith("/repo/alpha", { commit: null, branch: null });
  expect(useGraphStore.getState().selected).toBeNull();
});

test("a selection made with no active repository is not stored", () => {
  useRepoStore.setState({ activeId: null });
  useGraphStore.getState().select("a1b2c3");
  expect(saveRepoSelection).not.toHaveBeenCalled();
});

// ── Safe failure (SPEC §18): the activity store on a failed op ──────────────

const failedRunPlan = {
  display: 'git commit -m "fix: thing"',
  steps: [
    { argv: ["git", "commit", "-m", "fix: thing"], summary: "commit", risk: "Mutating" as const, effect: null, input: null, program: null },
  ],
  preview: null,
};

function failedCommitEntry(id: number) {
  // The web's `record()` and Rust's `op_finished` payload for the same op:
  // identical data, different id source (`nextEntryId` vs. the Rust ring).
  const entry = {
    id,
    repo: "/repo/alpha",
    display: 'git commit -m "fix: thing"',
    ok: false,
    duration_ms: 3,
    ts: 20,
    stdout: "",
    stderr: "nothing to commit",
    conflicts: [],
  };
  return entry;
}

test("record() of a failed op stores the raw result without touching repo state", () => {
  const before = useRepoStore.getState();
  const firstWebId = useActivityStore.getState().activities.length;
  useActivityStore.getState().record("/repo/alpha", {
    plan: failedRunPlan,
    ok: false,
    exit: 1,
    stdout: "",
    stderr: "nothing to commit",
    duration_ms: 3,
    conflicts: [],
  });
  const s = useActivityStore.getState();
  expect(s.activities).toHaveLength(firstWebId + 1);
  expect(s.activities[0]).toMatchObject({
    repo: "/repo/alpha",
    display: 'git commit -m "fix: thing"',
    ok: false,
    stdout: "",
    stderr: "nothing to commit",
  });
  // No optimistic repo state: the read model stays exactly what Rust reported.
  expect(useRepoStore.getState()).toEqual(before);
});

test("applyFinished of a failed op clears the running flag and keeps the raw failure", () => {
  const entry = { ...failedCommitEntry(12), display: "git reset --hard HEAD~1" };
  useActivityStore.getState().applyStarted({ repo: "/repo/alpha", display: "git reset --hard HEAD~1", risk: "Dangerous" });
  expect(useActivityStore.getState().running).toBe(true);

  useActivityStore.getState().applyFinished(entry);

  const s = useActivityStore.getState();
  expect(s.running).toBe(false);
  expect(s.runningDisplay).toBeNull();
  // An op the web did not record itself (Rust ran it, e.g. `bisect_test`)
  // still lands in the log with the Rust payload.
  expect(s.activities).toHaveLength(1);
  expect(s.activities[0]).toMatchObject({ ok: false, stderr: "nothing to commit" });
});

test("a failed op is recorded once: op_finished refreshes the web-recorded entry", () => {
  useActivityStore.getState().record("/repo/alpha", {
    plan: failedRunPlan,
    ok: false,
    exit: 1,
    stdout: "",
    stderr: "nothing to commit",
    duration_ms: 3,
    conflicts: [],
  });
  const webId = useActivityStore.getState().activities[0].id;
  // `op_finished` for the same op (same repo + display) refreshes, not duplicates.
  useActivityStore.getState().applyFinished({ ...failedCommitEntry(12), duration_ms: 15 });

  const s = useActivityStore.getState();
  expect(s.activities).toHaveLength(1);
  expect(s.activities[0]).toMatchObject({
    id: webId, // the web id survives …
    ok: false,
    duration_ms: 15, // … and the authoritative Rust timing/output replaces it
  });
});

test("a failed op's record() routes a conflicted repo to the Conflicts tab", () => {
  useMainTab.getState().setTab("graph");
  useActivityStore.getState().record("/repo/alpha", {
    plan: failedRunPlan,
    ok: false,
    exit: 1,
    stdout: "",
    stderr: "CONFLICT (content): Merge conflict in f.txt",
    duration_ms: 3,
    conflicts: ["f.txt"],
  });
  expect(useMainTab.getState().tab).toBe("conflicts");
});
