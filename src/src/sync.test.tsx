import { beforeEach, expect, test, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import App from "./App";

// `./api` is mocked wholesale; per-test behavior is configured on the mocks.
// `onOpFinished` captures its listener so tests can fire `op_finished` events.
const {
  planOp,
  runOp,
  loadStatus,
  loadBranches,
  loadRemotes,
  loadTags,
  loadGraph,
  onOpFinished,
  finishedCb,
  resetFinishedCb,
} = vi.hoisted(() => {
  let cb: ((p: LogEntry) => void) | null = null;
  return {
    planOp: vi.fn().mockResolvedValue(null),
    runOp: vi.fn().mockResolvedValue(null),
    loadStatus: vi.fn().mockResolvedValue(null),
    loadBranches: vi.fn().mockResolvedValue(null),
    loadRemotes: vi.fn().mockResolvedValue(null),
    loadTags: vi.fn().mockResolvedValue(null),
    loadGraph: vi.fn().mockResolvedValue(null),
    onOpFinished: vi.fn((c: (p: LogEntry) => void) => {
      cb = c;
      return Promise.resolve(null);
    }),
    finishedCb: () => cb,
    resetFinishedCb: () => {
      cb = null;
    },
  };
});

vi.mock("./api", () => ({
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
  loadBranches,
  loadRemotes,
  loadTags,
}));

import { useActivityStore, useGraphStore, useMainTab, useRefsStore, useRepoStore } from "./store";
import { TopBar } from "./components/TopBar";
import type { Branch, CommandPlan, LogEntry, Remote, RepoMeta, Status, Tag } from "./types";

const REPO = "/repo/alpha";

const branches: Branch[] = [
  {
    name: "main",
    is_current: true,
    is_remote: false,
    upstream: "origin/main",
    ahead: 1,
    behind: 2,
    tip: "a".repeat(40),
  },
  {
    name: "side",
    is_current: false,
    is_remote: false,
    upstream: null,
    ahead: 0,
    behind: 0,
    tip: "b".repeat(40),
  },
];

const remotes: Remote[] = [
  { name: "origin", url: "/origin.git", fetch: "/origin.git", push: null },
];

const tags: Tag[] = [];

const meta: RepoMeta = {
  id: REPO,
  name: "alpha",
  path: REPO,
  branch: "main",
  ahead: 1,
  behind: 2,
  dirty_count: 0,
};

const status: Status = {
  branch: "main",
  detached_head: null,
  head: "a".repeat(40),
  ahead: 1,
  behind: 2,
  upstream: "origin/main",
  changes: [],
  in_merge: false,
  in_rebase: false,
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
      },
    ],
    preview: {
      headline: display,
      lines: [],
      requires_type_to_confirm: false,
    },
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
  useRefsStore.setState({
    branches,
    remotes,
    tags,
    loading: false,
    error: null,
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
  useGraphStore.setState({
    query: "",
    selected: null,
    hovered: null,
    detail: null,
    detailLoading: false,
    detailError: null,
  });
  planOp.mockReset().mockResolvedValue(plan("git pull"));
  useMainTab.setState({ tab: "graph" });
  runOp.mockReset().mockResolvedValue(null);
  loadStatus.mockReset().mockResolvedValue(status);
  loadBranches.mockReset().mockResolvedValue(branches);
  loadRemotes.mockReset().mockResolvedValue(remotes);
  loadTags.mockReset().mockResolvedValue(tags);
  loadGraph.mockReset().mockResolvedValue(null);
  onOpFinished.mockClear();
  resetFinishedCb();
}

beforeEach(() => {
  cleanup();
  seed();
});

/** Flush the microtask queue deterministically (no wall-clock timers). */
async function flush(): Promise<void> {
  await act(async () => {});
}

function entry(conflicts: string[]): LogEntry {
  return {
    id: 1,
    repo: REPO,
    display: "git merge --no-ff side",
    ok: conflicts.length === 0,
    duration_ms: 5,
    ts: Date.now(),
    stdout: "",
    stderr:
      conflicts.length > 0 ? "CONFLICT (content): Merge conflict in f.txt" : "",
    conflicts,
  };
}

// ── sync controls: labels ───────────────────────────────────────────────────

test("top bar renders sync controls with ahead/behind-aware labels", async () => {
  render(<TopBar />);
  const fetch = screen.getByRole("button", { name: /Fetch/ }) as HTMLButtonElement;
  expect(fetch.disabled).toBe(false);
  const pull = screen.getByRole("button", { name: /^Pull(?!.*rebase)/i });
  expect(pull.textContent).toContain("↓2");
  expect((pull as HTMLButtonElement).disabled).toBe(false);
  const rebase = screen.getByRole("button", { name: /Pull.*rebase/i });
  expect(rebase.textContent).toContain("↓2");
  const push = screen.getByRole("button", { name: /Push/ });
  expect(push.textContent).toContain("↑1");
  const merge = screen.getByRole("button", { name: /Merge/ }) as HTMLButtonElement;
  expect(merge.disabled).toBe(false);
});

// ── sync controls: ops ──────────────────────────────────────────────────────

test("sync buttons compile the right ops", async () => {
  render(<TopBar />);

  fireEvent.click(screen.getByRole("button", { name: /Fetch/ }));
  await flush();
  expect(planOp).toHaveBeenCalledWith(REPO, {
    kind: "remote_fetch",
    remote: "origin",
    refspec: null,
  });
  useActivityStore.getState().dismissPreview();
  planOp.mockClear();

  fireEvent.click(screen.getByRole("button", { name: /^Pull(?!.*rebase)/i }));
  await flush();
  expect(planOp).toHaveBeenCalledWith(REPO, { kind: "pull", rebase: false });
  useActivityStore.getState().dismissPreview();
  planOp.mockClear();

  fireEvent.click(screen.getByRole("button", { name: /Pull.*rebase/i }));
  await flush();
  expect(planOp).toHaveBeenCalledWith(REPO, { kind: "pull", rebase: true });
  useActivityStore.getState().dismissPreview();
  planOp.mockClear();

  fireEvent.click(screen.getByRole("button", { name: /Push/ }));
  await flush();
  expect(planOp).toHaveBeenCalledWith(REPO, {
    kind: "push",
    remote: "origin",
    branch: null,
    force: false,
    set_upstream: false,
  });
});

test("pull and push are disabled without an upstream", async () => {
  useRepoStore.setState({
    status: {
      [REPO]: { ...status, upstream: null, ahead: 0, behind: 0 },
    },
  });
  render(<TopBar />);
  expect((screen.getByRole("button", { name: /^Pull(?!.*rebase)/i }) as HTMLButtonElement).disabled).toBe(true);
  expect((screen.getByRole("button", { name: /Pull.*rebase/i }) as HTMLButtonElement).disabled).toBe(true);
  expect((screen.getByRole("button", { name: /Push/ }) as HTMLButtonElement).disabled).toBe(true);
  // Fetch does not need an upstream: it targets a configured remote.
  expect((screen.getByRole("button", { name: /Fetch/ }) as HTMLButtonElement).disabled).toBe(false);
});

// ── merge picker ────────────────────────────────────────────────────────────

test("merge picker lists other local branches and compiles the op", async () => {
  render(<TopBar />);
  fireEvent.click(screen.getByRole("button", { name: /Merge/ }));
  await flush();
  // The menu offers `side`; the current branch (`main`) is not a source.
  const menu = screen.getByRole("menu");
  expect(
    within(menu)
      .getAllByRole("menuitem")
      .map((b) => b.textContent),
  ).toEqual(["side"]);
  fireEvent.click(within(menu).getByRole("menuitem", { name: "side" }));
  await flush();
  expect(planOp).toHaveBeenCalledWith(REPO, { kind: "merge", source: "side" });
});

// ── conflicted merge → conflict view (SPEC §10.4/§10.5) ────────────────────

test("a finished op with conflicts routes to the Files tab", async () => {
  render(<App />);
  await flush();
  expect(screen.getByRole("tab", { name: "Graph" }).getAttribute("aria-selected")).toBe("true");
  expect(screen.getByRole("tab", { name: "Files" }).getAttribute("aria-selected")).toBe("false");

  const cb = finishedCb();
  expect(cb).toBeTypeOf("function");
  await act(async () => {
    cb!(entry(["f.txt"]));
  });
  expect(screen.getByRole("tab", { name: "Files" }).getAttribute("aria-selected")).toBe("true");
  expect(screen.getByRole("tab", { name: "Graph" }).getAttribute("aria-selected")).toBe("false");
});

test("a finished op without conflicts stays on the current tab", async () => {
  render(<App />);
  await flush();
  const cb = finishedCb();
  expect(cb).toBeTypeOf("function");
  await act(async () => {
    cb!(entry([]));
  });
  expect(screen.getByRole("tab", { name: "Graph" }).getAttribute("aria-selected")).toBe("true");
});
