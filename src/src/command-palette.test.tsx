import { afterEach, expect, test, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import App from "./App";
import { useActivityStore, useCommitDraftStore, useGraphStore, useMainTab, useRepoStore, useRefsStore } from "./store";
import type { CommandPlan, Graph, RepoMeta, Status } from "./types";

const { planOp, runOp, onOpFinished, loadBranches, loadGraph } = vi.hoisted(() => ({
  planOp: vi.fn().mockResolvedValue(null),
  runOp: vi.fn().mockResolvedValue(null),
  onOpFinished: vi.fn().mockResolvedValue(null),
  loadBranches: vi.fn().mockResolvedValue(null),
  loadGraph: vi.fn().mockResolvedValue(null),
}));

const BRANCHES = [
  { name: "main", is_current: true, is_remote: false, upstream: "origin/main", ahead: 0, behind: 0, tip: "a".repeat(40) },
  { name: "side", is_current: false, is_remote: false, upstream: null, ahead: 0, behind: 0, tip: "b".repeat(40) },
];

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
  onOpFinished,
  loadGraph,
  loadCommit: vi.fn().mockResolvedValue(null),
  loadCommitStats: async () => [],
  loadDiff: vi.fn().mockResolvedValue(null),
  loadConflict: vi.fn().mockResolvedValue(null),
  loadBranches,
  loadRemotes: vi.fn().mockResolvedValue(null),
  loadTags: vi.fn().mockResolvedValue(null),
  loadWorktrees: async () => [],
  loadStashes: async () => [],
  loadSubmodules: async () => [],
  loadBisect: async () => ({ active: false, current: null, good: [], bad: [], remaining: 0, found: null, log: [] }),
  bisectTest: async () => ({ active: false, current: null, good: [], bad: [], remaining: 0, found: null, log: [] }),
  checkForUpdate: async () => null,
  installUpdate: async () => undefined,
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

const GRAPH: Graph = {
  nodes: [
    { hash: "c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1c1", subject: "fix: the graph lanes", author: "Ada", date: 1700000000, row: 0, lane: 0, is_head: true, refs: [{ name: "main", kind: "Branch", color: 0 }], lines: [] },
    { hash: "b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2", subject: "feat: stash panel", author: "Ada", date: 1699999000, row: 1, lane: 0, is_head: false, refs: [], lines: [] },
  ],
  edges: [],
  lines: [],
};

function plan(display: string, risk: "ReadOnly" | "Mutating" | "Dangerous"): CommandPlan {
  return {
    display,
    steps: [{ argv: display.split(" "), summary: "Step.", risk, effect: null, input: null, program: null }],
    preview: { headline: "Preview", lines: [], requires_type_to_confirm: risk === "Dangerous" },
  };
}

async function openPalette() {
  render(<App />);
  await act(async () => {});
  fireEvent.keyDown(document.body, { key: "k", metaKey: true });
  await act(async () => {});
  return screen.getByRole("dialog", { name: "Command palette" });
}

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  useRepoStore.setState({ repos: [], activeId: null, status: {}, opening: false, error: null });
  useActivityStore.setState({ activities: [], running: false, runningDisplay: null, runningRisk: null, preview: null, error: null, busy: false });
  useMainTab.setState({ tab: "graph" });
  useRefsStore.setState({ branches: [], remotes: [], tags: [], loading: false, error: null });
  useGraphStore.setState({ graph: null, loading: false, error: null, selected: null, hovered: null, query: "", detail: null, detailLoading: false, detailError: null });
  useCommitDraftStore.setState({ drafts: {} });
});

test("palette rows show the Rust-compiled command and its risk badge", async () => {
  planOp.mockResolvedValue(plan("git fetch origin", "Mutating"));
  useRepoStore.setState({ repos: [REPO], activeId: REPO.id, status: { [REPO.id]: STATUS }, opening: false, error: null });
  const palette = await openPalette();

  // The debounced batch of `plan_op` calls has not fired yet.
  expect(planOp).not.toHaveBeenCalled();
  await act(async () => {
    await new Promise((r) => setTimeout(r, 250));
  });

  const rows = within(palette).getAllByRole("option");
  const fetchRow = rows.find((r) => r.textContent?.includes("Fetch origin"));
  expect(fetchRow).toBeTruthy();
  expect(fetchRow?.textContent).toContain("git fetch origin");
  expect(fetchRow?.textContent).toContain("mutates the repo");
  // One call per op row, batched (not per keystroke).
  expect(planOp.mock.calls.length).toBeGreaterThan(0);
  expect(planOp.mock.calls.length).toBeLessThanOrEqual(12);
});

test("choosing a palette action previews the plan and never runs it", async () => {
  planOp.mockResolvedValue(plan("git stash push", "Mutating"));
  useRepoStore.setState({ repos: [REPO], activeId: REPO.id, status: { [REPO.id]: STATUS }, opening: false, error: null });
  const palette = await openPalette();
  const input = within(palette).getByRole("combobox") as HTMLInputElement;

  fireEvent.change(input, { target: { value: "stash changes" } });
  await act(async () => {
    await new Promise((r) => setTimeout(r, 250));
  });

  const row = within(palette).getByRole("option", { selected: true });
  expect(row.textContent).toContain("Stash changes");
  expect(row.textContent).toContain("git stash push");

  fireEvent.keyDown(row, { key: "Enter" });
  await act(async () => {});

  // The normal preview dialog took over; nothing ran.
  expect(runOp).not.toHaveBeenCalled();
  const preview = screen.getByRole("dialog", { name: "Confirm command" });
  expect(preview.textContent).toContain("git stash push");
  expect(preview.textContent).toContain("mutates the repo");
  // The palette closed, so the preview is the only thing on screen.
  expect(screen.queryByRole("dialog", { name: "Command palette" })).toBeNull();
  expect(planOp).toHaveBeenCalledWith(REPO.id, { kind: "stash_push", include_untracked: false });
});

test("a dangerous palette action lands in the type-to-confirm gate", async () => {
  planOp.mockResolvedValue(plan("git reset --hard HEAD~1", "Dangerous"));
  loadBranches.mockResolvedValue(BRANCHES);
  useRepoStore.setState({ repos: [REPO], activeId: REPO.id, status: { [REPO.id]: STATUS }, opening: false, error: null });
  const palette = await openPalette();
  await act(async () => {
    await new Promise((r) => setTimeout(r, 250));
  });

  const input = within(palette).getByRole("combobox") as HTMLInputElement;
  fireEvent.change(input, { target: { value: "switch to side" } });
  const row = within(palette).getByRole("option", { selected: true });
  expect(row.textContent).toContain("Switch to side");
  expect(row.textContent).toContain("git reset --hard HEAD~1");
  expect(row.textContent).toContain("dangerous");

  fireEvent.keyDown(row, { key: "Enter" });
  await act(async () => {});

  const preview = screen.getByRole("dialog", { name: "Confirm command" });
  const confirm = within(preview).getByRole("textbox") as HTMLInputElement;
  const run = within(preview).getByRole("button", { name: /Run git reset/ }) as HTMLButtonElement;
  expect(run.disabled).toBe(true);
  expect(runOp).not.toHaveBeenCalled();
  fireEvent.change(confirm, { target: { value: "confirm" } });
  expect(run.disabled).toBe(false);
});

test("palette commits select in the graph; repos switch; tabs switch", async () => {
  loadGraph.mockResolvedValue(GRAPH);
  useRepoStore.setState({
    repos: [REPO, { ...REPO, id: "/repo/beta", name: "beta", path: "/repo/beta" }],
    activeId: REPO.id,
    status: { [REPO.id]: STATUS },
    opening: false,
    error: null,
  });
  render(<App />);
  await act(async () => {});

  /** Open the palette, filter it, and press Enter on the live selected row. */
  const pick = async (filter: string) => {
    fireEvent.keyDown(document.body, { key: "k", metaKey: true });
    await act(async () => {});
    const palette = screen.getByRole("dialog", { name: "Command palette" });
    fireEvent.change(within(palette).getByRole("combobox"), { target: { value: filter } });
    const row = within(palette).getByRole("option", { selected: true });
    fireEvent.keyDown(row, { key: "Enter" });
    await act(async () => {});
    expect(screen.queryByRole("dialog", { name: "Command palette" })).toBeNull();
    return row.textContent ?? "";
  };

  // Commits group: search by subject, then by hash prefix.
  expect(await pick("feat: stash")).toContain("feat: stash panel");
  expect(useGraphStore.getState().selected).toBe("b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2");
  expect(await pick("b2b2b2b2")).toContain("feat: stash panel");

  // Repositories group switches the active repo (no op, no preview dialog).
  expect(await pick("beta")).toContain("beta");
  expect(useRepoStore.getState().activeId).toBe("/repo/beta");
  expect(screen.queryByRole("dialog", { name: "Confirm command" })).toBeNull();

  // Tabs group.
  expect(await pick("bisect")).toContain("Bisect");
  expect(useMainTab.getState().tab).toBe("bisect");
});

test("a fresh workspace gets an empty state instead of repo commands", async () => {
  const palette = await openPalette();
  expect(palette.textContent).toContain("No repository open");
  expect(palette.textContent).toContain("exact");
  // No repository actions to plan: the palette offers no op rows, no risk badges.
  expect(palette.textContent).not.toContain("Repository actions");
  expect(within(palette).queryAllByRole("option").every((row) => !row.querySelector(".badge"))).toBe(true);
  // The shortcut map is in the footer.
  expect(palette.textContent).toContain("preview command");
});

test("the palette offers exactly the commit the commit panel holds", async () => {
  planOp.mockResolvedValue(plan('git commit -m "fix: the graph lanes"', "Mutating"));
  useRepoStore.setState({ repos: [REPO], activeId: REPO.id, status: { [REPO.id]: STATUS }, opening: false, error: null });
  useCommitDraftStore.getState().patch(REPO.id, {
    message: "fix: the graph lanes",
    amend: false,
    coAuthors: ["Ada <ada@example.com>"],
  });

  const palette = await openPalette();
  fireEvent.change(within(palette).getByRole("combobox"), { target: { value: "commit staged" } });
  await act(async () => {
    await new Promise((r) => setTimeout(r, 250));
  });

  const row = within(palette).getByRole("option", { selected: true });
  expect(row.textContent).toContain("Commit staged changes");
  expect(row.textContent).toContain('git commit -m "fix: the graph lanes"');
  expect(row.textContent).toContain("mutates the repo");

  fireEvent.keyDown(row, { key: "Enter" });
  await act(async () => {});

  // Same op the panel would run, planned and previewed, never executed here.
  expect(runOp).not.toHaveBeenCalled();
  expect(planOp).toHaveBeenCalledWith(REPO.id, {
    kind: "commit",
    message: "fix: the graph lanes",
    amend: false,
    co_authors: ["Ada <ada@example.com>"],
  });
  const preview = screen.getByRole("dialog", { name: "Confirm command" });
  expect(preview.textContent).toContain("fix: the graph lanes");
});

test("a draft written in one repository is not offered in another", async () => {
  useRepoStore.setState({
    repos: [REPO, { ...REPO, id: "/repo/beta", name: "beta", path: "/repo/beta" }],
    activeId: REPO.id,
    status: { [REPO.id]: STATUS, "/repo/beta": STATUS },
    opening: false,
    error: null,
  });
  useCommitDraftStore.getState().patch("/repo/beta", { message: "docs: beta only", amend: false });

  const palette = await openPalette();
  fireEvent.change(within(palette).getByRole("combobox"), { target: { value: "commit" } });
  await act(async () => {});

  const labels = within(palette)
    .getAllByRole("option")
    .map((row) => row.textContent ?? "");
  expect(labels.some((t) => t.includes("Commit staged changes"))).toBe(false);
  // Alpha has no draft: the palette points at the commit box instead.
  expect(labels.some((t) => t.includes("Write a commit message"))).toBe(true);
});
