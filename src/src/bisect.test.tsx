// Stage 11 acceptance (SPEC §10.9, web): the bisect store + panel — start,
// verdicts, reset (all through the preview flow), the direct `bisect_test`
// round-trip, the found banner, the log, and the graph-menu draft hand-off.
// Mirrors worktrees.test.tsx.

import { beforeEach, expect, test, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import App from "./App";

const {
  planOp,
  runOp,
  loadStatus,
  loadGraph,
  loadBisect,
  bisectTest,
  onOpFinished,
} = vi.hoisted(() => ({
  planOp: vi.fn().mockResolvedValue(null),
  runOp: vi.fn().mockResolvedValue(null),
  loadStatus: vi.fn().mockResolvedValue(null),
  loadGraph: vi.fn().mockResolvedValue(null),
  loadBisect: vi.fn().mockResolvedValue(null),
  bisectTest: vi.fn().mockResolvedValue(null),
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
  loadSubmodules: async () => [],
  loadBisect,
  bisectTest,
}));

import {
  useActivityStore,
  useBisectStore,
  useMainTab,
  useRepoStore,
} from "./store";
import { BisectPanel } from "./components/BisectPanel";
import type { BisectState, CommandPlan, RepoMeta, Status } from "./types";

const REPO = "/repo/alpha";

const INACTIVE: BisectState = {
  active: false,
  current: null,
  good: [],
  bad: [],
  remaining: 0,
  found: null,
  log: [],
};

const ACTIVE: BisectState = {
  active: true,
  current: "c".repeat(40),
  good: ["e".repeat(40)],
  bad: ["a".repeat(40)],
  remaining: 3,
  found: null,
  log: ["git bisect start", "git bisect bad aaaa", "git bisect good eeee"],
};

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

function seed(bisect: BisectState = INACTIVE): void {
  useRepoStore.setState({
    repos: [meta],
    activeId: REPO,
    status: { [REPO]: status },
    opening: false,
    error: null,
  });
  useBisectStore.setState({ bisect, loading: false, error: null, draftBad: null, testCommand: "" });
  useActivityStore.setState({
    activities: [],
    running: false,
    runningDisplay: null,
    runningRisk: null,
    preview: null,
    error: null,
    busy: false,
  });
  useMainTab.getState().setTab("bisect");
  planOp.mockReset().mockResolvedValue(plan("git bisect start"));
  runOp.mockReset().mockResolvedValue(null);
  loadStatus.mockReset().mockResolvedValue(status);
  loadGraph.mockReset().mockResolvedValue(null);
  loadBisect.mockReset().mockResolvedValue(bisect);
  bisectTest.mockReset().mockResolvedValue(bisect);
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

test("bisect store loads the active repo's state", async () => {
  useBisectStore.setState({ bisect: INACTIVE });
  loadBisect.mockResolvedValueOnce(ACTIVE);
  await act(async () => {
    await useBisectStore.getState().load(REPO);
  });
  expect(loadBisect).toHaveBeenCalledWith(REPO);
  expect(useBisectStore.getState().bisect).toEqual(ACTIVE);
});

test("outside Tauri (null bisect) reads as the inactive state", async () => {
  loadBisect.mockResolvedValueOnce(null);
  await act(async () => {
    await useBisectStore.getState().load(REPO);
  });
  expect(useBisectStore.getState().bisect).toEqual(INACTIVE);
});

// ── panel: inactive → start ─────────────────────────────────────────────────

test("the inactive panel offers good/bad/extra inputs and starts the bisect", async () => {
  render(<BisectPanel />);
  fireEvent.change(screen.getByPlaceholderText("good ref (branch or commit)"), {
    target: { value: "v1.0" },
  });
  fireEvent.change(screen.getByPlaceholderText("bad ref (branch or commit)"), {
    target: { value: "main" },
  });
  fireEvent.change(screen.getByPlaceholderText("extra refs (optional, space-separated)"), {
    target: { value: "fix/1 fix/2" },
  });
  fireEvent.click(screen.getByRole("button", { name: "Start bisect" }));
  await flush();
  expect(planOp).toHaveBeenCalledWith(REPO, {
    kind: "bisect_start",
    good: "v1.0",
    bad: "main",
    extra: ["fix/1", "fix/2"],
  });
  useActivityStore.getState().dismissPreview();
});

test("no extra refs means an empty extra list", async () => {
  render(<BisectPanel />);
  fireEvent.change(screen.getByPlaceholderText("good ref (branch or commit)"), {
    target: { value: "v1.0" },
  });
  fireEvent.change(screen.getByPlaceholderText("bad ref (branch or commit)"), {
    target: { value: "HEAD" },
  });
  fireEvent.click(screen.getByRole("button", { name: "Start bisect" }));
  await flush();
  expect(planOp).toHaveBeenCalledWith(REPO, {
    kind: "bisect_start",
    good: "v1.0",
    bad: "HEAD",
    extra: [],
  });
  useActivityStore.getState().dismissPreview();
});

test("the bad input prefills from the graph menu's draft pick", async () => {
  useBisectStore.getState().setDraftBad("deadbeef");
  render(<BisectPanel />);
  const bad = screen.getByPlaceholderText("bad ref (branch or commit)") as HTMLInputElement;
  expect(bad.value).toBe("deadbeef");
  expect(useBisectStore.getState().draftBad).toBeNull();
});

// ── panel: active ───────────────────────────────────────────────────────────

test("the active panel shows the candidate, remaining count, and both sets", () => {
  seed(ACTIVE);
  render(<BisectPanel />);
  expect(screen.getByText("c".repeat(40))).toBeTruthy();
  expect(screen.getByText("3 candidates left")).toBeTruthy();
  expect(screen.getByText(`good: ${"e".repeat(40)}`)).toBeTruthy();
  expect(screen.getByText(`bad: ${"a".repeat(40)}`)).toBeTruthy();
  expect(document.querySelector("pre.bisect-log")!.textContent).toContain("git bisect start");
});

test("a converged bisect shows the culprit banner", () => {
  seed({ ...ACTIVE, active: true, found: "f".repeat(40), current: null, remaining: 0 });
  render(<BisectPanel />);
  expect(screen.getByText("First bad commit:")).toBeTruthy();
  expect(screen.getByText("f".repeat(40))).toBeTruthy();
});

test("run test calls bisectTest directly and stores the returned state", async () => {
  seed(ACTIVE);
  const next: BisectState = { ...ACTIVE, current: "d".repeat(40), remaining: 1 };
  bisectTest.mockResolvedValueOnce(next);
  render(<BisectPanel />);
  fireEvent.change(screen.getByPlaceholderText("test command (exit 0 = good)"), {
    target: { value: "cargo test" },
  });
  fireEvent.click(screen.getByRole("button", { name: "Run test" }));
  await flush();
  expect(bisectTest).toHaveBeenCalledWith(REPO, "cargo test");
  expect(useBisectStore.getState().bisect).toEqual(next);
  // The direct test flow never touches the preview pipeline.
  expect(planOp).not.toHaveBeenCalled();
});

test("good / bad verdicts go through the preview flow", async () => {
  seed(ACTIVE);
  render(<BisectPanel />);
  fireEvent.click(screen.getByRole("button", { name: "Good" }));
  await flush();
  expect(planOp).toHaveBeenLastCalledWith(REPO, { kind: "bisect_verdict", verdict: "Good" });
  useActivityStore.getState().dismissPreview();
  fireEvent.click(screen.getByRole("button", { name: "Bad" }));
  await flush();
  expect(planOp).toHaveBeenLastCalledWith(REPO, { kind: "bisect_verdict", verdict: "Bad" });
  useActivityStore.getState().dismissPreview();
});

test("reset sends bisect_reset", async () => {
  seed(ACTIVE);
  render(<BisectPanel />);
  fireEvent.click(screen.getByRole("button", { name: "Reset" }));
  await flush();
  expect(planOp).toHaveBeenCalledWith(REPO, { kind: "bisect_reset" });
  useActivityStore.getState().dismissPreview();
});

test("the bisect log renders in a <pre>", () => {
  seed(ACTIVE);
  render(<BisectPanel />);
  const pre = document.querySelector("pre.bisect-log")!;
  expect(pre.textContent).toContain("git bisect good eeee");
});

// ── graph menu + App wiring ─────────────────────────────────────────────────

test("App loads bisect for the active repo and mounts the Bisect tab", async () => {
  render(<App />);
  await flush();
  expect(loadBisect).toHaveBeenCalledWith(REPO);
  expect(screen.getByRole("tab", { name: "Bisect" })).toBeTruthy();
});
