import { beforeEach, expect, test, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";

// `./api` is mocked wholesale; per-test behavior is configured on the mocks.
const { planOp, loadRebaseCommits } = vi.hoisted(() => ({
  planOp: vi.fn().mockResolvedValue(null),
  loadRebaseCommits: vi.fn().mockResolvedValue([]),
}));

vi.mock("./api", () => ({
  fetchBuildInfo: vi.fn().mockResolvedValue(null),
  openRepo: vi.fn().mockResolvedValue(null),
  loadStatus: vi.fn().mockResolvedValue(null),
  onStatusChanged: vi.fn().mockResolvedValue(null),
  planOp,
  runOp: vi.fn().mockResolvedValue(null),
  loadLog: vi.fn().mockResolvedValue(null),
  onOpStarted: vi.fn().mockResolvedValue(null),
  onOpFinished: vi.fn().mockResolvedValue(null),
  loadGraph: vi.fn().mockResolvedValue(null),
  loadCommit: vi.fn().mockResolvedValue(null),
  loadDiff: vi.fn().mockResolvedValue(null),
  loadBranches: vi.fn().mockResolvedValue([]),
  loadRemotes: vi.fn().mockResolvedValue([]),
  loadTags: vi.fn().mockResolvedValue([]),
  loadRebaseCommits,
}));

import { useActivityStore, useRebaseStore, useRefsStore, useRepoStore } from "./store";
import { TopBar } from "./components/TopBar";
import { RefsPanel } from "./components/RefsPanel";
import { RebaseDialog } from "./components/RebaseDialog";
import type { Branch, RebaseCommit, RepoMeta, Status } from "./types";

const REPO = "/repo/alpha";
const H_NEW = "1111111111111111111111111111111111111111";
const H_OLD = "2222222222222222222222222222222222222222";

/** The range `main..feature`, newest first (the dialog's UI order). */
const commits: RebaseCommit[] = [
  { hash: H_NEW, subject: "two" },
  { hash: H_OLD, subject: "one" },
];

const meta: RepoMeta = {
  id: REPO,
  name: "alpha",
  path: REPO,
  branch: "feature",
  ahead: 0,
  behind: 1,
  dirty_count: 0,
};

const status: Status = {
  branch: "feature",
  detached_head: null,
  head: H_NEW,
  ahead: 0,
  behind: 1,
  upstream: "main",
  changes: [],
  in_merge: false,
  in_rebase: false,
  in_cherry_pick: false,
};

const branches: Branch[] = [
  {
    name: "feature",
    is_current: true,
    is_remote: false,
    upstream: "main",
    ahead: 0,
    behind: 1,
    tip: H_NEW,
  },
  {
    name: "side",
    is_current: false,
    is_remote: false,
    upstream: null,
    ahead: 0,
    behind: 0,
    tip: H_OLD,
  },
  {
    name: "origin/main",
    is_remote: true,
    is_current: false,
    upstream: null,
    ahead: 0,
    behind: 0,
    tip: H_OLD,
  },
];

function seed(): void {
  useRepoStore.setState({
    repos: [meta],
    activeId: REPO,
    status: { [REPO]: status },
    opening: false,
    error: null,
  });
  useRefsStore.setState({ branches, remotes: [], tags: [], loading: false, error: null });
  useRebaseStore.setState({ open: false, commits: [], loading: false, error: null });
  useActivityStore.setState({
    activities: [],
    running: false,
    runningDisplay: null,
    runningRisk: null,
    preview: null,
    error: null,
    busy: false,
  });
  planOp.mockReset().mockResolvedValue(null);
  loadRebaseCommits.mockReset().mockResolvedValue(commits);
}

beforeEach(() => {
  cleanup();
  seed();
});

/** Flush the microtask queue deterministically (no wall-clock timers). */
async function flush(): Promise<void> {
  await act(async () => {});
}

// ── rebase store ────────────────────────────────────────────────────────────

test("rebase store loads the commits the target range would rewrite", async () => {
  await act(async () => {
    await useRebaseStore.getState().load(REPO, "main");
  });
  expect(loadRebaseCommits).toHaveBeenCalledWith(REPO, "main");
  const s = useRebaseStore.getState();
  expect(s.commits).toEqual(commits);
  expect(s.error).toBeNull();
});

test("rebase store surfaces the Rust error string", async () => {
  loadRebaseCommits.mockRejectedValueOnce(new Error("no such branch 'gone'"));
  await act(async () => {
    await useRebaseStore.getState().load(REPO, "gone");
  });
  const s = useRebaseStore.getState();
  expect(s.commits).toEqual([]);
  expect(s.error).toBe("no such branch 'gone'");
});

// ── RebaseDialog ────────────────────────────────────────────────────────────

function openDialog(): void {
  render(
    <>
      <TopBar />
      <RebaseDialog />
    </>,
  );
  fireEvent.click(screen.getByRole("button", { name: "Rebase…" }));
}

test("the dialog defaults onto the upstream and lists the range newest first", async () => {
  openDialog();
  await flush();
  const dialog = screen.getByRole("dialog", { name: "Rebase" });
  const input = within(dialog).getByRole("textbox", { name: "Rebase target" });
  expect((input as HTMLInputElement).value).toBe("main");
  expect(loadRebaseCommits).toHaveBeenCalledWith(REPO, "main");
  const rows = within(dialog).getAllByRole("listitem");
  expect(rows).toHaveLength(2);
  expect(rows[0].textContent).toContain("two");
  expect(rows[1].textContent).toContain("one");
  // Every commit starts as a plain pick.
  for (const select of within(dialog).getAllByRole("combobox")) {
    expect((select as HTMLSelectElement).value).toBe("Pick");
  }
});

test("reword needs a message; squash may carry one; the todo is sent on preview", async () => {
  openDialog();
  await flush();
  const dialog = screen.getByRole("dialog", { name: "Rebase" });
  const rows = within(dialog).getAllByRole("listitem");

  fireEvent.change(within(rows[0]).getByRole("combobox"), {
    target: { value: "Reword" },
  });
  const preview = within(dialog).getByRole("button", { name: "Preview rebase" });
  expect((preview as HTMLButtonElement).disabled).toBe(true); // a reword without a message cannot run
  fireEvent.change(within(rows[0]).getByRole("textbox", { name: `new message for ${H_NEW.slice(0, 8)}` }), {
    target: { value: "rewritten two" },
  });

  fireEvent.change(within(rows[1]).getByRole("combobox"), {
    target: { value: "Squash" },
  });
  fireEvent.change(within(rows[1]).getByRole("textbox", { name: `new message for ${H_OLD.slice(0, 8)}` }), {
    target: { value: "combined" },
  });

  expect((preview as HTMLButtonElement).disabled).toBe(false);
  fireEvent.click(preview);
  await flush();
  expect(planOp).toHaveBeenCalledWith(REPO, {
    kind: "rebase",
    target: "main",
    edits: [
      { hash: H_NEW, action: "Reword", message: "rewritten two" },
      { hash: H_OLD, action: "Squash", message: "combined" },
    ],
  });
  // The dialog closes; the preview dialog takes over (SPEC §9).
  expect(screen.queryByRole("dialog", { name: "Rebase" })).toBeNull();
});

test("rows reorder before the todo is compiled", async () => {
  openDialog();
  await flush();
  const dialog = screen.getByRole("dialog", { name: "Rebase" });
  const rows = within(dialog).getAllByRole("listitem");
  // Move "two" below "one" — the todo order flips.
  fireEvent.click(within(rows[0]).getByRole("button", { name: "Move down" }));
  await flush();
  const after = within(dialog).getAllByRole("listitem");
  expect(after[0].textContent).toContain("one");
  fireEvent.click(within(dialog).getByRole("button", { name: "Preview rebase" }));
  await flush();
  expect(planOp).toHaveBeenCalledWith(REPO, {
    kind: "rebase",
    target: "main",
    edits: [
      { hash: H_OLD, action: "Pick", message: null },
      { hash: H_NEW, action: "Pick", message: null },
    ],
  });
});

test("an empty range cannot be previewed", async () => {
  loadRebaseCommits.mockResolvedValueOnce([]);
  openDialog();
  await flush();
  const dialog = screen.getByRole("dialog", { name: "Rebase" });
  expect(dialog.textContent).toContain("Nothing to rebase onto this target");
  expect((within(dialog).getByRole("button", { name: "Preview rebase" }) as HTMLButtonElement).disabled).toBe(true);
});

// ── rebase-onto (Branches tab) ──────────────────────────────────────────────

test("branch rows rebase the current branch onto another", async () => {
  render(<RefsPanel tab="branches" />);
  const sideRow = screen
    .getAllByRole("listitem")
    .find((li) => li.textContent?.includes("side"));
  fireEvent.click(within(sideRow!).getByRole("button", { name: "Rebase onto" }));
  await flush();
  expect(planOp).toHaveBeenCalledWith(REPO, { kind: "rebase_onto", target: "side" });
  // Remote-tracking rows have no rebase action.
  expect(screen.queryAllByRole("button", { name: "Rebase onto" })).toHaveLength(1);
});
