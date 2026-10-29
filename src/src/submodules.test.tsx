// Stage 11 acceptance (SPEC §10.8, web): the submodules store, the FilesPanel
// badge + section, and the init/update ops. Mirrors worktrees.test.tsx.

import { beforeEach, expect, test, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import App from "./App";

const {
  planOp,
  runOp,
  loadStatus,
  loadGraph,
  loadSubmodules,
  onOpFinished,
} = vi.hoisted(() => ({
  planOp: vi.fn().mockResolvedValue(null),
  runOp: vi.fn().mockResolvedValue(null),
  loadStatus: vi.fn().mockResolvedValue(null),
  loadGraph: vi.fn().mockResolvedValue(null),
  loadSubmodules: vi.fn().mockResolvedValue(null),
  onOpFinished: vi.fn().mockResolvedValue(null),
}));

vi.mock("./api", () => ({
  loadRebaseCommits: vi.fn().mockResolvedValue(null),
  fetchBuildInfo: vi.fn().mockResolvedValue(null),
  openRepo: vi.fn().mockResolvedValue(null),
  loadStatus,
  onStatusChanged: vi.fn().mockResolvedValue(null),
  planOp,
  runOp,
  loadLog: vi.fn().mockResolvedValue(null),
  onOpStarted: vi.fn().mockResolvedValue(null),
  onOpFinished,
  loadGraph,
  loadCommit: vi.fn().mockResolvedValue(null),
  loadDiff: vi.fn().mockResolvedValue(null),
  loadConflict: vi.fn().mockResolvedValue(null),
  loadBranches: vi.fn().mockResolvedValue([]),
  loadRemotes: vi.fn().mockResolvedValue([]),
  loadTags: vi.fn().mockResolvedValue([]),
  loadWorktrees: vi.fn().mockResolvedValue([]),
  loadStashes: async () => [],
  loadSubmodules,
  loadBisect: async () => ({ active: false, current: null, good: [], bad: [], remaining: 0, found: null, log: [] }),
  bisectTest: async () => ({ active: false, current: null, good: [], bad: [], remaining: 0, found: null, log: [] }),
}));

import {
  useActivityStore,
  useMainTab,
  useRepoStore,
  useStagingStore,
  useSubmodulesStore,
} from "./store";
import { FilesPanel } from "./components/FilesPanel";
import type { CommandPlan, RepoMeta, Status, Submodule } from "./types";

const REPO = "/repo/alpha";

const submodules: Submodule[] = [
  {
    name: "vendor/protocol",
    path: "vendor/protocol",
    url: "https://example.com/protocol.git",
    recorded: "b".repeat(40),
    head: "b".repeat(40),
    initialized: true,
    differs: false,
  },
  {
    name: "vendor/packs",
    path: "vendor/packs",
    url: "https://example.com/packs.git",
    recorded: "c".repeat(40),
    head: null,
    initialized: false,
    differs: false,
  },
];

const meta: RepoMeta = {
  id: REPO,
  name: "alpha",
  path: REPO,
  branch: "main",
  ahead: 0,
  behind: 0,
  dirty_count: 0,
};

const status: Status = {
  branch: "main",
  detached_head: null,
  head: "a".repeat(40),
  ahead: 0,
  behind: 0,
  upstream: null,
  changes: [
    { path: "vendor/protocol", kind: "Modified", staged: false, binary: false, submodule: true },
    { path: "src/a.ts", kind: "Modified", staged: false, binary: false },
  ],
  in_merge: false,
  in_rebase: false,
  in_cherry_pick: false,
};

/** A minimal `Mutating` plan (the shape the preview/confirm flow receives). */
function plan(display: string): CommandPlan {
  return {
    display,
    steps: [
      {
        argv: display.split(" "),
        summary: display,
        risk: "Mutating",
        effect: null,
        input: null,
        program: null,
      },
    ],
    preview: { headline: display, lines: [], requires_type_to_confirm: false },
  };
}

function seed(): void {
  useRepoStore.setState({
    repos: [meta],
    activeId: REPO,
    status: { [REPO]: status },
    opening: false,
    error: null,
  });
  useSubmodulesStore.setState({ submodules, loading: false, error: null });
  useStagingStore.getState().reset();
  useActivityStore.setState({
    activities: [],
    running: false,
    runningDisplay: null,
    runningRisk: null,
    preview: null,
    error: null,
    busy: false,
  });
  useMainTab.getState().setTab("files");
  planOp.mockReset().mockResolvedValue(plan("git submodule update --init"));
  runOp.mockReset().mockResolvedValue(null);
  loadStatus.mockReset().mockResolvedValue(status);
  loadGraph.mockReset().mockResolvedValue(null);
  loadSubmodules.mockReset().mockResolvedValue(submodules);
}

/** Flush the microtask queue deterministically (no wall-clock timers). */
async function flush(): Promise<void> {
  await act(async () => {});
}

beforeEach(() => {
  cleanup();
  seed();
});

// ── store ───────────────────────────────────────────────────────────────────

test("submodules store loads the active repo's submodules", async () => {
  useSubmodulesStore.setState({ submodules: [] });
  await act(async () => {
    await useSubmodulesStore.getState().load(REPO);
  });
  expect(loadSubmodules).toHaveBeenCalledWith(REPO);
  expect(useSubmodulesStore.getState().submodules).toEqual(submodules);
});

// ── FilesPanel badge + section ──────────────────────────────────────────────

test("a gitlink change gets the submodule badge", () => {
  render(<FilesPanel />);
  const badge = screen.getByText("submodule");
  expect(badge.classList.contains("submodule")).toBe(true);
  expect(screen.queryByText("src/a.ts")).toBeTruthy();
});

test("the submodules section lists path, url title, and state badges", () => {
  render(<FilesPanel />);
  const section = screen.getByText("Submodules").closest(".submodules")!;
  const rows = section.querySelectorAll("ul.refs-list > li");
  expect(rows).toHaveLength(2);
  expect(within(rows[0] as HTMLElement).getAllByText("vendor/protocol").length).toBe(2);
  expect(within(rows[0] as HTMLElement).getByTitle("https://example.com/protocol.git")).toBeTruthy();
  expect(within(rows[1] as HTMLElement).getByText("not initialized")).toBeTruthy();
  expect(within(rows[0] as HTMLElement).queryByText("not initialized")).toBeNull();
});

test("a moved pointer gets the `pointer moved` badge", () => {
  useSubmodulesStore.setState({
    submodules: [{ ...submodules[0], differs: true, head: "d".repeat(40) }],
  });
  render(<FilesPanel />);
  expect(screen.getByText("pointer moved")).toBeTruthy();
});

test("init dispatches submodule_init for an uninitialized submodule", async () => {
  render(<FilesPanel />);
  const section = screen.getByText("Submodules").closest(".submodules")!;
  const rows = section.querySelectorAll("ul.refs-list > li");
  // Only the uninitialized row offers Init.
  expect(within(rows[0] as HTMLElement).queryByRole("button", { name: "Init" })).toBeNull();
  fireEvent.click(within(rows[1] as HTMLElement).getByRole("button", { name: "Init" }));
  await flush();
  expect(planOp).toHaveBeenCalledWith(REPO, { kind: "submodule_init", path: "vendor/packs" });
  useActivityStore.getState().dismissPreview();
});

test("update dispatches submodule_update with the optional pinned commit", async () => {
  render(<FilesPanel />);
  const section = screen.getByText("Submodules").closest(".submodules")!;
  fireEvent.change(within(section as HTMLElement).getByPlaceholderText("check out a specific commit (optional)"), {
    target: { value: "abc123" },
  });
  const rows = section.querySelectorAll("ul.refs-list > li");
  fireEvent.click(within(rows[0] as HTMLElement).getByRole("button", { name: "Update" }));
  await flush();
  expect(planOp).toHaveBeenCalledWith(REPO, {
    kind: "submodule_update",
    path: "vendor/protocol",
    commit: "abc123",
  });
  useActivityStore.getState().dismissPreview();
});

test("an empty commit input updates to the recorded pointer (null)", async () => {
  render(<FilesPanel />);
  const rows = document.querySelectorAll(".submodules ul.refs-list > li");
  expect(rows).toHaveLength(2);
  fireEvent.click(within(rows[0] as HTMLElement).getByRole("button", { name: "Update" }));
  await flush();
  expect(planOp).toHaveBeenCalledWith(REPO, {
    kind: "submodule_update",
    path: "vendor/protocol",
    commit: null,
  });
  useActivityStore.getState().dismissPreview();
});

test("a repo without submodules renders no submodule section", () => {
  useSubmodulesStore.setState({ submodules: [] });
  render(<FilesPanel />);
  expect(screen.queryByText("Submodules")).toBeNull();
});

// ── App wiring ──────────────────────────────────────────────────────────────

test("App loads submodules for the active repo", async () => {
  render(<App />);
  await flush();
  expect(loadSubmodules).toHaveBeenCalledWith(REPO);
});
