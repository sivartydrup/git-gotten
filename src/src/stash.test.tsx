// Stage 11 acceptance (SPEC §10.6, web): the stash store + panel — the list
// with per-stash files, push (with/without untracked) and apply/pop/drop, all
// through the preview flow. Mirrors the worktrees.test.tsx harness.

import { beforeEach, expect, test, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import App from "./App";

const {
  planOp,
  runOp,
  loadStatus,
  loadGraph,
  loadStashes,
  onOpFinished,
} = vi.hoisted(() => ({
  planOp: vi.fn().mockResolvedValue(null),
  runOp: vi.fn().mockResolvedValue(null),
  loadStatus: vi.fn().mockResolvedValue(null),
  loadGraph: vi.fn().mockResolvedValue(null),
  loadStashes: vi.fn().mockResolvedValue(null),
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
  loadStashes,
  loadSubmodules: async () => [],
  loadBisect: async () => ({ active: false, current: null, good: [], bad: [], remaining: 0, found: null, log: [] }),
  bisectTest: async () => ({ active: false, current: null, good: [], bad: [], remaining: 0, found: null, log: [] }),
}));

import { useActivityStore, useMainTab, useRepoStore, useStashesStore } from "./store";
import { StashPanel } from "./components/StashPanel";
import type { CommandPlan, RepoMeta, Stash, Status } from "./types";

const REPO = "/repo/alpha";

const stashes: Stash[] = [
  {
    id: 0,
    name: "stash@{0}",
    message: "wip: parser",
    branch: "main",
    date: "2026-10-28T09:14:00Z",
    files: ["src/parser.ts", "src/lexer.ts"],
  },
  {
    id: 1,
    name: "stash@{1}",
    message: "before rebase",
    branch: null,
    date: "2026-10-20T18:02:00Z",
    files: [],
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
  useStashesStore.setState({ stashes, loading: false, error: null });
  useActivityStore.setState({
    activities: [],
    running: false,
    runningDisplay: null,
    runningRisk: null,
    preview: null,
    error: null,
    busy: false,
  });
  useMainTab.getState().setTab("stash");
  planOp.mockReset().mockResolvedValue(plan("git stash push"));
  runOp.mockReset().mockResolvedValue(null);
  loadStatus.mockReset().mockResolvedValue(status);
  loadGraph.mockReset().mockResolvedValue(null);
  loadStashes.mockReset().mockResolvedValue(stashes);
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

test("stash store loads the active repo's stashes", async () => {
  useStashesStore.setState({ stashes: [] });
  await act(async () => {
    await useStashesStore.getState().load(REPO);
  });
  expect(loadStashes).toHaveBeenCalledWith(REPO);
  expect(useStashesStore.getState().stashes).toEqual(stashes);
});

// ── panel ───────────────────────────────────────────────────────────────────

test("stash panel lists name, date, branch, message and the stashed files", () => {
  render(<StashPanel />);
  const rows = document.querySelectorAll("ul.refs-list > li");
  expect(rows).toHaveLength(2);
  const row = rows[0] as HTMLElement;
  expect(within(row).getByText("stash@{0}")).toBeTruthy();
  expect(within(row).getByText("2026-10-28T09:14:00Z")).toBeTruthy();
  expect(within(row).getByText("on main")).toBeTruthy();
  expect(within(row).getByText("wip: parser")).toBeTruthy();
  expect(within(row).getByText("src/parser.ts")).toBeTruthy();
  expect(within(row).getByText("src/lexer.ts")).toBeTruthy();
});

test("push sends stash_push without untracked by default", async () => {
  render(<StashPanel />);
  fireEvent.click(screen.getByRole("button", { name: "Push" }));
  await flush();
  expect(planOp).toHaveBeenCalledWith(REPO, { kind: "stash_push", include_untracked: false });
  useActivityStore.getState().dismissPreview();
});

test("the untracked checkbox feeds stash_push's include_untracked", async () => {
  render(<StashPanel />);
  fireEvent.click(screen.getByRole("checkbox", { name: /include untracked/i }));
  fireEvent.click(screen.getByRole("button", { name: "Push" }));
  await flush();
  expect(planOp).toHaveBeenCalledWith(REPO, { kind: "stash_push", include_untracked: true });
  useActivityStore.getState().dismissPreview();
});

test("apply / pop / drop send the exact op for that stash id", async () => {
  render(<StashPanel />);
  const rows = document.querySelectorAll("ul.refs-list > li");
  for (const kind of ["stash_apply", "stash_pop", "stash_drop"] as const) {
    const label = { stash_apply: "Apply", stash_pop: "Pop", stash_drop: "Drop" }[kind];
    fireEvent.click(within(rows[1] as HTMLElement).getByRole("button", { name: label }));
    await flush();
    expect(planOp).toHaveBeenLastCalledWith(REPO, { kind, id: 1 });
    useActivityStore.getState().dismissPreview();
  }
});

test("an empty stash list offers no row actions or Clear all", () => {
  useStashesStore.setState({ stashes: [] });
  render(<StashPanel />);
  expect(screen.getByText("No stashes.")).toBeTruthy();
  expect(screen.queryByRole("button", { name: "Drop" })).toBeNull();
  expect(screen.queryByRole("button", { name: "Clear all" })).toBeNull();
});

test("Clear all previews the fieldless stash_drop_all op", async () => {
  render(<StashPanel />);
  fireEvent.click(screen.getByRole("button", { name: "Clear all" }));
  await flush();
  expect(planOp).toHaveBeenCalledWith(REPO, { kind: "stash_drop_all" });
  useActivityStore.getState().dismissPreview();
});

// ── App wiring ──────────────────────────────────────────────────────────────

test("App loads stashes for the active repo and mounts the Stash tab", async () => {
  render(<App />);
  await flush();
  expect(loadStashes).toHaveBeenCalledWith(REPO);
  expect(screen.getByRole("tab", { name: "Stash" })).toBeTruthy();
});
