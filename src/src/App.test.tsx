import { afterEach, expect, test, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import App from "./App";
import { useActivityStore, useGraphStore, useRepoStore } from "./store";
import type { RepoMeta, Status } from "./types";

// `planOp` / `runOp` are hoisted so tests can drive per-test IPC behavior.
const { planOp, runOp } = vi.hoisted(() => ({
  planOp: vi.fn().mockResolvedValue(null),
  runOp: vi.fn().mockResolvedValue(null),
}));

vi.mock("./api", () => ({
  loadRebaseCommits: vi.fn().mockResolvedValue(null),
  fetchBuildInfo: vi.fn().mockResolvedValue(null),
  openRepo: vi.fn().mockResolvedValue(null),
  loadStatus: vi.fn().mockResolvedValue(null),
  onStatusChanged: vi.fn().mockResolvedValue(null),
  planOp,
  runOp,
  loadLog: vi.fn().mockResolvedValue(null),
  onOpStarted: vi.fn().mockResolvedValue(null),
  onOpFinished: vi.fn().mockResolvedValue(null),
  loadGraph: vi.fn().mockResolvedValue(null),
  loadCommit: vi.fn().mockResolvedValue(null),
  loadStashes: async () => [],
  loadSubmodules: async () => [],
  loadBisect: async () => ({ active: false, current: null, good: [], bad: [], remaining: 0, found: null, log: [] }),
  bisectTest: async () => ({ active: false, current: null, good: [], bad: [], remaining: 0, found: null, log: [] }),
}));

afterEach(() => {
  cleanup();
  useRepoStore.setState({
    repos: [],
    activeId: null,
    status: {},
    opening: false,
    error: null,
  });
  useActivityStore.setState({
    activities: [],
    running: false,
    runningDisplay: null,
    preview: null,
    error: null,
    busy: false,
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
});

test("renders the GitGotten shell (non-Tauri fallback)", async () => {
  render(<App />);
  // `render` runs the effects that call the async IPC; outside Tauri those
  // resolve `null`. Flush the microtasks so the fallback branch commits.
  await act(async () => {});
  expect(document.body.textContent).toContain("GitGotten");
  expect(document.body.textContent).toContain("Files"); // the Files tab
  expect(document.body.textContent).toContain("No repositories open");
});

test("renders a seeded repo: nav entry, top-bar state, and changes", async () => {
  const meta: RepoMeta = {
    id: "/repo/alpha",
    name: "alpha",
    path: "/repo/alpha",
    branch: "main",
    ahead: 1,
    behind: 0,
    dirty_count: 2,
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
      { path: "README.md", kind: "Untracked", staged: false, binary: false },
    ],
    in_merge: false,
    in_rebase: false,
    in_cherry_pick: false,
  };
  useRepoStore.setState({ repos: [meta], activeId: meta.id, status: { [meta.id]: status } });

  render(<App />);
  await act(async () => {});

  // The changed files live in the Files tab (the graph is the default tab).
  fireEvent.click(screen.getByRole("tab", { name: "Files" }));
  await act(async () => {});

  const body = document.body.textContent ?? "";
  expect(body).toContain("alpha"); // nav + top bar
  expect(body).toContain("main"); // branch
  expect(body).toContain("2 changes");
  expect(body).toContain("src/a.ts");
  expect(body).toContain("README.md");
});

test("shows a clean tree for a repo with no changes", async () => {
  const meta: RepoMeta = {
    id: "/repo/clean",
    name: "clean",
    path: "/repo/clean",
    branch: "main",
    ahead: 0,
    behind: 0,
    dirty_count: 0,
  };
  useRepoStore.setState({
    repos: [meta],
    activeId: meta.id,
    status: {
      [meta.id]: {
        branch: "main",
        detached_head: null,
        head: null,
        ahead: 0,
        behind: 0,
        upstream: null,
        changes: [],
        in_merge: false,
        in_rebase: true,
        in_cherry_pick: false,
      },
    },
  });

  render(<App />);
  await act(async () => {});
  fireEvent.click(screen.getByRole("tab", { name: "Files" }));
  await act(async () => {});

  const body = document.body.textContent ?? "";
  expect(body).toContain("Working tree clean");
  expect(body).toContain("REBASE"); // in-rebase state badge
});

test("shows the activity panel with its empty state", async () => {
  render(<App />);
  await act(async () => {});
  const body = document.body.textContent ?? "";
  expect(body).toContain("Activity");
  expect(body).toContain("No commands run yet.");
});

test("commit flow: preview shows the exact command, confirm runs and records it", async () => {
  const meta: RepoMeta = {
    id: "/repo/alpha",
    name: "alpha",
    path: "/repo/alpha",
    branch: "main",
    ahead: 0,
    behind: 0,
    dirty_count: 1,
  };
  useRepoStore.setState({
    repos: [meta],
    activeId: meta.id,
    status: {
      [meta.id]: {
        branch: "main",
        detached_head: null,
        head: null,
        ahead: 0,
        behind: 0,
        upstream: null,
        changes: [
          { path: "src/a.ts", kind: "Modified", staged: true, binary: false },
        ],
        in_merge: false,
        in_rebase: false,
        in_cherry_pick: false,
      },
    },
  });

  const commitPlan = {
    display: 'git commit -m "fix: thing"',
    steps: [
      {
        argv: ["git", "commit", "-m", "fix: thing"],
        summary: "Commit the staged changes.",
        risk: "Mutating",
        effect: null,
      },
    ],
    preview: {
      headline: "Commit 1 staged file",
      lines: ["M src/a.ts"],
      requires_type_to_confirm: false,
    },
  };
  // Persistent: the debounced CmdHint re-plans, and confirm re-plans again.
  planOp.mockResolvedValue(commitPlan);
  runOp.mockResolvedValueOnce({
    plan: commitPlan,
    exit: 0,
    ok: true,
    stdout: "[main abc1234] fix: thing",
    stderr: "",
    conflicts: [],
    duration_ms: 12,
  });

  render(<App />);
  await act(async () => {});

  // The commit action with its live CmdHint is present; the button is
  // disabled until a message is typed.
  const input = screen.getByPlaceholderText(/commit message/i);
  expect((screen.getByRole("button", { name: "Commit" }) as HTMLButtonElement).disabled).toBe(true);

  fireEvent.change(input, { target: { value: "fix: thing" } });
  // The CmdHint is Rust-compiled: wait out the 300 ms debounce.
  const delay = new Promise((resolve) => setTimeout(resolve, 350));
  await act(async () => {
    await delay;
  });
  expect((screen.getByRole("button", { name: "Commit" }) as HTMLButtonElement).disabled).toBe(false);
  expect(document.body.textContent).toContain('git commit -m "fix: thing"');

  fireEvent.click(screen.getByRole("button", { name: "Commit" }));
  await act(async () => {});

  // The preview/confirm dialog shows the exact command and its risk class.
  const dialog = screen.getByRole("dialog", { name: "Confirm command" });
  expect(dialog.textContent).toContain('git commit -m "fix: thing"');
  expect(dialog.textContent).toContain("mutates the repo");
  expect(planOp).toHaveBeenCalledWith(
    meta.id,
    { kind: "commit", message: "fix: thing", amend: false },
  );

  fireEvent.click(screen.getByRole("button", { name: /Run git commit/ }));
  await act(async () => {});

  // Confirmed: the dialog closes, the op is recorded in the activity log.
  expect(runOp).toHaveBeenCalledWith(meta.id, {
    kind: "commit",
    message: "fix: thing",
    amend: false,
  });
  expect(screen.queryByRole("dialog")).toBeNull();
  const log = useActivityStore.getState();
  expect(log.activities).toHaveLength(1);
  expect(log.activities[0]).toMatchObject({
    repo: meta.id,
    display: 'git commit -m "fix: thing"',
    ok: true,
  });
});
