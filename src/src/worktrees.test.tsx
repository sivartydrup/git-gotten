// Stage 10 acceptance (SPEC §10.7, web): the worktrees store + panel, the
// TopBar worktree switcher, and the add/remove op requests — mocked IPC, real
// store/component wiring. Mirrors the refs.test.tsx harness.

import { beforeEach, expect, test, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import App from "./App";

const {
  planOp,
  runOp,
  loadStatus,
  loadGraph,
  loadWorktrees,
  openRepo,
  onOpFinished,
} = vi.hoisted(() => ({
  planOp: vi.fn().mockResolvedValue(null),
  runOp: vi.fn().mockResolvedValue(null),
  loadStatus: vi.fn().mockResolvedValue(null),
  loadGraph: vi.fn().mockResolvedValue(null),
  loadWorktrees: vi.fn().mockResolvedValue(null),
  openRepo: vi.fn().mockResolvedValue(null),
  onOpFinished: vi.fn().mockResolvedValue(null),
}));

vi.mock("./api", () => ({
  loadRebaseCommits: vi.fn().mockResolvedValue(null),
  fetchBuildInfo: vi.fn().mockResolvedValue(null),
  openRepo,
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
  loadWorktrees,
  loadStashes: async () => [],
  loadSubmodules: async () => [],
  loadBisect: async () => ({ active: false, current: null, good: [], bad: [], remaining: 0, found: null, log: [] }),
  bisectTest: async () => ({ active: false, current: null, good: [], bad: [], remaining: 0, found: null, log: [] }),
}));

import { useActivityStore, useMainTab, useRepoStore, useWorktreesStore } from "./store";
import { WorktreesPanel } from "./components/WorktreesPanel";
import type { CommandPlan, RepoMeta, Status, Worktree } from "./types";

const REPO = "/repo/alpha";
const WT = "/repo/alpha-side";

const worktrees: Worktree[] = [
  { path: REPO, branch: "main", head: "a".repeat(40), is_main: true, locked: false, prunable: false },
  { path: WT, branch: "side", head: "c".repeat(40), is_main: false, locked: false, prunable: false },
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

const wtMeta: RepoMeta = {
  id: WT,
  name: "alpha-side",
  path: WT,
  branch: "side",
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
  changes: [],
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
  useWorktreesStore.setState({ worktrees, loading: false, error: null });
  useActivityStore.setState({
    activities: [],
    running: false,
    runningDisplay: null,
    runningRisk: null,
    preview: null,
    error: null,
    busy: false,
  });
  useMainTab.getState().setTab("worktrees");
  planOp.mockReset().mockResolvedValue(plan("git worktree add /repo/x main"));
  runOp.mockReset().mockResolvedValue(null);
  loadStatus.mockReset().mockResolvedValue(status);
  loadGraph.mockReset().mockResolvedValue(null);
  loadWorktrees.mockReset().mockResolvedValue(worktrees);
  openRepo.mockReset().mockResolvedValue(meta);
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

test("worktrees store loads the active repo's checkouts", async () => {
  useWorktreesStore.setState({ worktrees: [] });
  await act(async () => {
    await useWorktreesStore.getState().load(REPO);
  });
  expect(loadWorktrees).toHaveBeenCalledWith(REPO);
  expect(useWorktreesStore.getState().worktrees).toEqual(worktrees);
});

// ── panel: list, remove, add ────────────────────────────────────────────────

test("Worktrees panel lists each checkout with branch, path, and main badge", () => {
  render(<WorktreesPanel />);
  const rows = screen.getAllByRole("listitem");
  expect(rows).toHaveLength(2);
  expect(within(rows[0]).getByText("main")).toBeTruthy();
  expect(within(rows[0]).getByText(REPO)).toBeTruthy();
  expect(within(rows[0]).getByText("main worktree")).toBeTruthy();
  expect(within(rows[1]).getByText("side")).toBeTruthy();
  expect(within(rows[1]).getByText(WT)).toBeTruthy();
});

test("remove sends worktree_remove through the preview flow", async () => {
  render(<WorktreesPanel />);
  fireEvent.click(screen.getByRole("button", { name: "Remove" }));
  await flush();
  expect(planOp).toHaveBeenCalledWith(REPO, { kind: "worktree_remove", path: WT, force: false });
  useActivityStore.getState().dismissPreview();
});

test("force remove sends force:true (the Dangerous tier gate)", async () => {
  render(<WorktreesPanel />);
  fireEvent.click(screen.getByRole("button", { name: "Force" }));
  await flush();
  expect(planOp).toHaveBeenCalledWith(REPO, { kind: "worktree_remove", path: WT, force: true });
  useActivityStore.getState().dismissPreview();
});

test("the main worktree row offers no remove", () => {
  render(<WorktreesPanel />);
  const rows = screen.getAllByRole("listitem");
  expect(within(rows[0]).queryByRole("button", { name: "Remove" })).toBeNull();
  expect(within(rows[1]).queryByRole("button", { name: "Remove" })).toBeTruthy();
});

test("switch opens the linked worktree as the active repo", async () => {
  openRepo.mockResolvedValue(wtMeta);
  render(<WorktreesPanel />);
  fireEvent.click(within(screen.getAllByRole("listitem")[1]).getByRole("button", { name: "Switch" }));
  await flush();
  expect(openRepo).toHaveBeenCalledWith(WT);
  expect(useRepoStore.getState().activeId).toBe(WT);
});

test("add dialog sends worktree_add with path and optional new branch", async () => {
  render(<WorktreesPanel />);
  fireEvent.click(screen.getByRole("button", { name: "Add worktree" }));
  const dialog = screen.getByRole("dialog", { name: "Add worktree" });
  fireEvent.change(within(dialog).getByPlaceholderText("new directory path"), {
    target: { value: "/repo/alpha-new" },
  });
  fireEvent.change(within(dialog).getByPlaceholderText("new branch name (optional)"), {
    target: { value: "feature" },
  });
  fireEvent.click(within(dialog).getByRole("button", { name: "Add" }));
  await flush();
  expect(planOp).toHaveBeenCalledWith(REPO, {
    kind: "worktree_add",
    committish: "main",
    path: "/repo/alpha-new",
    new_branch: "feature",
  });
  useActivityStore.getState().dismissPreview();
});

test("add without a new branch checks out the existing committish", async () => {
  render(<WorktreesPanel />);
  fireEvent.click(screen.getByRole("button", { name: "Add worktree" }));
  const dialog = screen.getByRole("dialog", { name: "Add worktree" });
  fireEvent.change(within(dialog).getByPlaceholderText("new directory path"), {
    target: { value: "/repo/alpha-side2" },
  });
  fireEvent.click(within(dialog).getByRole("button", { name: "Add" }));
  await flush();
  expect(planOp).toHaveBeenCalledWith(REPO, {
    kind: "worktree_add",
    committish: "main",
    path: "/repo/alpha-side2",
    new_branch: null,
  });
  useActivityStore.getState().dismissPreview();
});

// ── TopBar switcher + App wiring ────────────────────────────────────────────

test("App loads worktrees for the active repo and mounts the Worktrees tab", async () => {
  render(<App />);
  await flush();
  expect(loadWorktrees).toHaveBeenCalledWith(REPO);
  expect(screen.getByRole("tab", { name: "Worktrees" })).toBeTruthy();
});

test("TopBar worktree switcher retargets the view to the picked worktree", async () => {
  openRepo.mockResolvedValue(wtMeta);
  render(<App />);
  await flush();
  fireEvent.click(screen.getByRole("button", { name: /^Worktree/ }));
  const menu = screen.getByRole("menu");
  fireEvent.click(within(menu).getByRole("menuitem", { name: /alpha-side/ }));
  await flush();
  expect(openRepo).toHaveBeenCalledWith(WT);
  expect(useRepoStore.getState().activeId).toBe(WT);
});
