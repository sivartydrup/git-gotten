import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import App from "./App";
import { useActivityStore, useGraphStore, useRefsStore, useRepoStore } from "./store";
import type { CommandPlan, RepoMeta, Status } from "./types";

// `./api` is mocked wholesale. The spies a test drives are hoisted; every other
// IPC name resolves `null` (the app's "not inside Tauri" path).
const { planOp, runOp, onOpFinished, loadBranches } = vi.hoisted(() => ({
  planOp: vi.fn().mockResolvedValue(null),
  runOp: vi.fn().mockResolvedValue(null),
  onOpFinished: vi.fn().mockResolvedValue(null),
  loadBranches: vi.fn().mockResolvedValue(null),
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
  onOpFinished,
  loadGraph: vi.fn().mockResolvedValue(null),
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

function plan(display: string, risk: "ReadOnly" | "Mutating" | "Dangerous"): CommandPlan {
  return {
    display,
    steps: [{ argv: display.split(" "), summary: "Step.", risk, effect: null, input: null, program: null }],
    preview: {
      headline: "Preview",
      lines: [],
      requires_type_to_confirm: risk === "Dangerous",
    },
  };
}

async function renderShell() {
  useRepoStore.setState({ repos: [REPO], activeId: REPO.id, status: { [REPO.id]: STATUS }, opening: false, error: null });
  render(<App />);
  await act(async () => {});
}

beforeEach(() => {
  planOp.mockResolvedValue(plan("git status --porcelain=v1", "ReadOnly"));
  runOp.mockResolvedValue(null);
});

afterEach(() => {
  cleanup();
  useRepoStore.setState({ repos: [], activeId: null, status: {}, opening: false, error: null });
  useActivityStore.setState({ activities: [], running: false, runningDisplay: null, runningRisk: null, preview: null, error: null, busy: false });
  useGraphStore.setState({ graph: null, loading: false, error: null, selected: null, hovered: null, query: "", detail: null, detailLoading: false, detailError: null });
  useRefsStore.setState({ branches: [], remotes: [], tags: [], loading: false, error: null });
});

test("Escape closes the preview dialog and returns focus to the trigger", async () => {
  planOp.mockResolvedValue(plan("git fetch origin", "Mutating"));
  await renderShell();

  const trigger = screen.getByRole("button", { name: "Fetch" });
  trigger.focus();
  fireEvent.click(trigger);
  await act(async () => {});

  const dialog = screen.getByRole("dialog", { name: "Confirm command" });
  // Focus moved into the dialog (its primary action), not left on the button.
  expect(dialog.contains(document.activeElement)).toBe(true);

  fireEvent.keyDown(document.activeElement as Element, { key: "Escape" });
  await act(async () => {});

  expect(screen.queryByRole("dialog")).toBeNull();
  expect(document.activeElement).toBe(trigger);
});

test("Escape closes a RefsPanel dialog and returns focus to the trigger", async () => {
  await renderShell();
  // Seed the refs *after* mount: App reloads them for the active repo, and an
  // unmocked `load_branches` (null) would wipe the list.
  useRefsStore.setState({
    branches: [
      { name: "main", is_current: true, is_remote: false, upstream: "origin/main", ahead: 0, behind: 0, tip: "a".repeat(40) },
      { name: "side", is_current: false, is_remote: false, upstream: null, ahead: 0, behind: 0, tip: "b".repeat(40) },
    ],
    remotes: [],
    tags: [],
    loading: false,
    error: null,
  });
  fireEvent.click(screen.getByRole("tab", { name: /Branches/ }));
  await act(async () => {});

  const row = screen.getByText("side").closest(".refs-row") as HTMLElement;
  const trigger = within(row as unknown as HTMLElement).getByRole("button", { name: "Rename" });
  trigger.focus();
  fireEvent.click(trigger);

  const dialog = screen.getByRole("dialog", { name: "Rename branch" });
  const input = within(dialog).getByRole("textbox");
  expect(document.activeElement).toBe(input);

  // Escape inside the field closes only this dialog.
  fireEvent.keyDown(input, { key: "Escape" });
  expect(screen.queryByRole("dialog")).toBeNull();
  expect(document.activeElement).toBe(trigger);
});

test("Tab cannot leave an open dialog", async () => {
  planOp.mockResolvedValue(plan("git fetch origin", "Mutating"));
  await renderShell();

  fireEvent.click(screen.getByRole("button", { name: "Fetch" }));
  await act(async () => {});

  const dialog = screen.getByRole("dialog", { name: "Confirm command" });
  const cancel = within(dialog).getByRole("button", { name: "Cancel" });
  const run = within(dialog).getByRole("button", { name: /Run git fetch/ }) as HTMLButtonElement;

  // The shell's own focus target (the graph filter) sits before the dialog in
  // the DOM; a trapped Tab must never reach it.
  cancel.focus();
  fireEvent.keyDown(cancel, { key: "Tab" });
  expect(document.activeElement).toBe(run);

  fireEvent.keyDown(run, { key: "Tab" });
  expect(dialog.contains(document.activeElement)).toBe(true);

  run.focus();
  fireEvent.keyDown(run, { key: "Tab", shiftKey: true });
  expect(document.activeElement).toBe(cancel);

  cancel.focus();
  fireEvent.keyDown(cancel, { key: "Tab", shiftKey: true });
  expect(dialog.contains(document.activeElement)).toBe(true);
});

test("the dangerous preview keeps Run disabled until the confirm text matches", async () => {
  planOp.mockResolvedValue(plan("git reset --hard HEAD~1", "Dangerous"));
  await renderShell();

  fireEvent.click(screen.getByRole("button", { name: "Fetch" }));
  await act(async () => {});

  const dialog = screen.getByRole("dialog", { name: "Confirm command" });
  const confirmInput = within(dialog).getByRole("textbox");
  const run = within(dialog).getByRole("button", { name: /Run git reset/ }) as HTMLButtonElement;
  expect(run.disabled).toBe(true);

  fireEvent.change(confirmInput, { target: { value: "confir" } });
  expect(run.disabled).toBe(true);

  fireEvent.change(confirmInput, { target: { value: "confirm" } });
  expect(run.disabled).toBe(false);

  // Escape while the gate is armed still only cancels: nothing ran.
  fireEvent.keyDown(confirmInput, { key: "Escape" });
  await act(async () => {});
  expect(runOp).not.toHaveBeenCalled();
  expect(screen.queryByRole("dialog")).toBeNull();
});

test("Escape does not close a preview whose op is already running", async () => {
  planOp.mockResolvedValue(plan("git push origin main", "Mutating"));
  await renderShell();

  fireEvent.click(screen.getByRole("button", { name: "Fetch" }));
  await act(async () => {});
  useActivityStore.setState({ running: true, runningDisplay: "git push origin main" });

  const dialog = screen.getByRole("dialog", { name: "Confirm command" });
  fireEvent.keyDown(dialog, { key: "Escape" });
  expect(screen.getByRole("dialog", { name: "Confirm command" })).toBeTruthy();
});

test("the Add worktree dialog traps Tab, closes on Escape, and restores focus", async () => {
  await renderShell();
  fireEvent.click(screen.getByRole("tab", { name: /Worktrees/ }));
  await act(async () => {});

  const trigger = screen.getByRole("button", { name: "Add worktree" });
  trigger.focus();
  fireEvent.click(trigger);

  const dialog = screen.getByRole("dialog", { name: "Add worktree" });
  const first = within(dialog).getAllByRole("textbox")[0];
  expect(document.activeElement).toBe(first);

  const last = within(dialog).getByRole("button", { name: "Add" }) as HTMLButtonElement;
  expect(last.disabled).toBe(true); // no path typed yet
  fireEvent.keyDown(last, { key: "Tab" });
  expect(dialog.contains(document.activeElement)).toBe(true);

  fireEvent.keyDown(first, { key: "Escape" });
  expect(screen.queryByRole("dialog", { name: "Add worktree" })).toBeNull();
  expect(document.activeElement).toBe(trigger);
});
