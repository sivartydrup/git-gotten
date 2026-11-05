import { afterEach, expect, test, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import App from "./App";
import { useActivityStore, useGraphStore, useMainTab, useRepoStore } from "./store";
import type { RepoMeta, Status } from "./types";

const { planOp, runOp, onOpFinished, loadGraph, finished } = vi.hoisted(() => ({
  planOp: vi.fn().mockResolvedValue(null),
  runOp: vi.fn().mockResolvedValue(null),
  // Captures the callback App subscribes with, so a test can deliver an
  // `op_finished` event exactly as the Rust core would.
  finished: { handler: null as ((payload: unknown) => void) | null },
  loadGraph: vi.fn().mockResolvedValue(null),
  onOpFinished: vi.fn((cb: (payload: unknown) => void) => {
    finished.handler = cb;
    return Promise.resolve(() => {});
  }),
}));

vi.mock("./api", () => ({
  workspaceGet: async () => null,
  workspaceRepos: async () => null,
  workspaceSet: async () => null,
  groupAdd: async () => null,
  groupRename: async () => null,
  groupDelete: async () => null,
  repoPin: async () => null,
  repoTouch: async () => null,
  saveRepoSelection: async () => null,
  loadRepoSelection: async () => null,
  removeRepoFromWorkspace: async () => null,
  loadRebaseCommits: vi.fn().mockResolvedValue(null),
  fetchBuildInfo: vi.fn().mockResolvedValue(null),
  openRepo: vi.fn().mockResolvedValue(null),
  loadStatus: vi.fn().mockResolvedValue(null),
  onStatusChanged: vi.fn().mockResolvedValue(null),
  planOp,
  runOp,
  loadLog: vi.fn().mockResolvedValue(null),
  onOpStarted: vi.fn().mockResolvedValue(null),
  loadGraph,
  loadCommit: vi.fn().mockResolvedValue(null),
  loadCommitStats: async () => [],
  loadDiff: vi.fn().mockResolvedValue(null),
  loadConflict: vi.fn().mockResolvedValue(null),
  loadBranches: vi.fn().mockResolvedValue(null),
  loadRemotes: vi.fn().mockResolvedValue(null),
  loadTags: vi.fn().mockResolvedValue(null),
  loadWorktrees: async () => [],
  loadStashes: async () => [],
  loadSubmodules: async () => [],
  loadBisect: async () => ({ active: false, current: null, good: [], bad: [], remaining: 0, found: null, log: [] }),
  bisectTest: async () => ({ active: false, current: null, good: [], bad: [], remaining: 0, found: null, log: [] }),
  checkForUpdate: async () => null,
  installUpdate: async () => undefined,
  onOpFinished,
}));

const REPO: RepoMeta = {
  id: "/repo/alpha",
  name: "alpha",
  path: "/repo/alpha",
  branch: "main",
  ahead: 0,
  behind: 0,
  dirty_count: 1,
};

const STATUS: Status = {
  branch: "main",
  detached_head: null,
  head: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  ahead: 0,
  behind: 0,
  upstream: "origin/main",
  changes: [{ path: "src/a.ts", kind: "Modified", staged: true, binary: false }],
  in_merge: false,
  in_rebase: false,
  in_cherry_pick: false,
};

async function renderShell() {
  useRepoStore.setState({
    repos: [REPO],
    activeId: REPO.id,
    status: { [REPO.id]: STATUS },
    opening: false,
    error: null,
  });
  render(<App />);
  await act(async () => {});
}

afterEach(() => {
  cleanup();
  useRepoStore.setState({ repos: [], activeId: null, status: {}, opening: false, error: null });
  useActivityStore.setState({ activities: [], running: false, runningDisplay: null, runningRisk: null, preview: null, error: null, busy: false });
  useMainTab.setState({ tab: "graph" });
  useGraphStore.setState({ graph: null, loading: false, error: null, selected: null, hovered: null, query: "", detail: null, detailLoading: false, detailError: null });
});

test("⌘K opens the palette and Escape closes it", async () => {
  await renderShell();
  expect(screen.queryByRole("dialog", { name: "Command palette" })).toBeNull();

  fireEvent.keyDown(document.body, { key: "k", metaKey: true });
  await act(async () => {});

  const palette = screen.getByRole("dialog", { name: "Command palette" });
  // The filter owns focus the moment the palette opens.
  expect(palette.contains(document.activeElement)).toBe(true);

  fireEvent.keyDown(document.activeElement as Element, { key: "Escape" });
  await act(async () => {});
  expect(screen.queryByRole("dialog", { name: "Command palette" })).toBeNull();

  // Ctrl+K works too (Linux/Windows parity).
  fireEvent.keyDown(document.body, { key: "k", ctrlKey: true });
  await act(async () => {});
  expect(screen.getByRole("dialog", { name: "Command palette" })).toBeTruthy();
});

test("typing in the commit message box never switches tabs or grabs the filter", async () => {
  planOp.mockResolvedValue({
    display: 'git commit -m "fix: thing"',
    steps: [{ argv: ["git", "commit", "-m", "fix: thing"], summary: "Commit.", risk: "Mutating", effect: null, input: null, program: null }],
    preview: { headline: "Commit 1 staged file", lines: [], requires_type_to_confirm: false },
  });
  await renderShell();

  const box = screen.getByPlaceholderText(/commit message/i);
  box.focus();
  for (const key of ["1", "2", "3", "4", "5", "6", "7", "/"]) {
    fireEvent.change(box, { target: { value: `fix: ${key}` } });
    fireEvent.keyDown(box, { key });
  }
  expect(useMainTab.getState().tab).toBe("graph");
  expect(document.activeElement).toBe(box);

  // The same keystrokes from the graph filter must not steal focus either.
  const filter = document.querySelector(".topbar-search") as HTMLInputElement;
  filter.focus();
  fireEvent.keyDown(filter, { key: "7" });
  expect(useMainTab.getState().tab).toBe("graph");
  expect(document.activeElement).toBe(filter);
});

test("digit keys switch the main tabs in strip order", async () => {
  await renderShell();
  const expected = ["graph", "files", "conflicts", "branches", "worktrees", "stash", "bisect"] as const;
  for (let i = 0; i < expected.length; i += 1) {
    fireEvent.keyDown(document.body, { key: String(i + 1) });
    expect(useMainTab.getState().tab).toBe(expected[i]);
  }
  // The strip agrees with the shortcut map.
  const tabs = screen.getAllByRole("tab").map((t) => t.textContent?.replace(/[0-9]+$/, "").trim());
  expect(tabs.slice(0, 7)).toEqual(["Graph", "Files", "Conflicts", "Branches", "Worktrees", "Stash", "Bisect"]);
});

test("`/` focuses the graph filter", async () => {
  await renderShell();
  document.body.focus();
  fireEvent.keyDown(document.body, { key: "/" });
  expect(document.activeElement).toBe(document.querySelector(".topbar-search"));
});

test("⌘Enter submits the commit through the preview dialog, never directly", async () => {
  planOp.mockResolvedValue({
    display: 'git commit -m "fix: thing"',
    steps: [{ argv: ["git", "commit", "-m", "fix: thing"], summary: "Commit.", risk: "Mutating", effect: null, input: null, program: null }],
    preview: { headline: "Commit 1 staged file", lines: [], requires_type_to_confirm: false },
  });
  await renderShell();

  const box = screen.getByPlaceholderText(/commit message/i);
  box.focus();
  fireEvent.change(box, { target: { value: "fix: thing" } });
  // The commit button only enables once the Rust-compiled hint confirms the op
  // is valid: wait out the panel's 300 ms re-plan debounce.
  await act(async () => {
    await new Promise((r) => setTimeout(r, 350));
  });

  fireEvent.keyDown(box, { key: "Enter", metaKey: true });
  await act(async () => {});

  const dialog = screen.getByRole("dialog", { name: "Confirm command" });
  expect(dialog.textContent).toContain('git commit -m "fix: thing"');
  expect(runOp).not.toHaveBeenCalled();

  // Cancel the preview: plain Enter in the box is a newline, not a submit.
  fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
  await act(async () => {});
  expect(screen.queryByRole("dialog")).toBeNull();
  fireEvent.keyDown(box, { key: "Enter" });
  await act(async () => {});
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(runOp).not.toHaveBeenCalled();
});

test("Escape closes the inspect drawer first, then the graph menu, then the palette", async () => {
  loadGraph.mockResolvedValue({
    nodes: [
      { hash: "c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1", subject: "fix: lanes", author: "Ada", date: 1700000000, row: 0, lane: 0, is_head: true, refs: [], lines: [] },
      { hash: "b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2", subject: "feat: stash", author: "Ada", date: 1699999000, row: 1, lane: 0, is_head: false, refs: [], lines: [] },
    ],
    edges: [],
    lines: [],
  });
  await renderShell();

  // 1. An open inspect drawer is the first thing Escape closes.
  useGraphStore.setState({ selected: "abc123", detail: null });
  fireEvent.keyDown(document.body, { key: "Escape" });
  expect(useGraphStore.getState().selected).toBeNull();
  expect(screen.queryByRole("dialog", { name: "Command palette" })).toBeNull();

  // 2. With a real graph context menu open, Escape dismisses it.
  const canvas = document.querySelector(".graph-canvas");
  expect(canvas).toBeTruthy();
  canvas!.dispatchEvent(
    new MouseEvent("contextmenu", { bubbles: true, clientX: 60, clientY: 20 }),
  );
  await act(async () => {});
  expect(document.querySelector(".graph-context-menu")).toBeTruthy();
  fireEvent.keyDown(document.body, { key: "Escape" });
  await act(async () => {});
  expect(document.querySelector(".graph-context-menu")).toBeNull();

  // 3. Only then does Escape reach the palette.
  fireEvent.keyDown(document.body, { key: "k", metaKey: true });
  await act(async () => {});
  expect(screen.getByRole("dialog", { name: "Command palette" })).toBeTruthy();
  fireEvent.keyDown(document.body, { key: "Escape" });
  await act(async () => {});
  expect(screen.queryByRole("dialog", { name: "Command palette" })).toBeNull();
});

test("the visually hidden aria-live region announces the finished op", async () => {
  await renderShell();
  const live = document.querySelector(".gg-sr-only");
  expect(live).toBeTruthy();
  expect(live?.getAttribute("aria-live")).toBe("polite");
  expect(live?.textContent).toBe("");

  act(() => {
    finished.handler!({
      id: 1,
      repo: REPO.id,
      display: "git push origin main",
      ok: true,
      duration_ms: 5,
      ts: Date.now(),
      stdout: "",
      stderr: "",
      conflicts: [],
    });
  });
  expect(live?.textContent).toContain("Completed: git push origin main");
  // The shell's other live regions are separate: this is an announcement, not a
  // visual duplicate of the activity log.
  expect(live?.className).toContain("gg-sr-only");
});

test("a fresh workspace shows guidance with a working open action", async () => {
  render(<App />);
  await act(async () => {});

  const card = screen.getByRole("region", { name: "Get started" });
  expect(card.textContent).toContain("branching graph");
  expect(card.textContent).toContain("exact");
  // The tab strip is replaced by the card, and the shortcut list is present.
  expect(screen.queryByRole("tab", { name: "Bisect" })).toBeNull();
  expect(card.textContent).toContain("command palette");

  fireEvent.click(screen.getByRole("button", { name: /Open a repository/ }));
  expect(document.activeElement).toBe(document.querySelector(".repo-nav .open-form .open-input"));
});
