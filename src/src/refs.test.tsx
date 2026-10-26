import { beforeEach, expect, test, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import App from "./App";

// `./api` is mocked wholesale; per-test behavior is configured on the mocks.
const {
  planOp,
  runOp,
  loadStatus,
  loadBranches,
  loadRemotes,
  loadTags,
  loadGraph,
  onOpFinished,
} = vi.hoisted(() => ({
  planOp: vi.fn().mockResolvedValue(null),
  runOp: vi.fn().mockResolvedValue(null),
  loadStatus: vi.fn().mockResolvedValue(null),
  loadBranches: vi.fn().mockResolvedValue(null),
  loadRemotes: vi.fn().mockResolvedValue(null),
  loadTags: vi.fn().mockResolvedValue(null),
  loadGraph: vi.fn().mockResolvedValue(null),
  onOpFinished: vi.fn().mockResolvedValue(null),
}));

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

import { useActivityStore, useRefsStore, useRepoStore } from "./store";
import { RefsPanel } from "./components/RefsPanel";
import type { Branch, CommandPlan, Remote, RepoMeta, Status, Tag } from "./types";

const REPO = "/repo/alpha";

const branches: Branch[] = [
  {
    name: "main",
    is_current: true,
    is_remote: false,
    upstream: "origin/main",
    ahead: 1,
    behind: 0,
    tip: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  },
  {
    name: "origin/main",
    is_current: false,
    is_remote: true,
    upstream: null,
    ahead: 0,
    behind: 0,
    tip: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
  },
  {
    name: "side",
    is_current: false,
    is_remote: false,
    upstream: null,
    ahead: 0,
    behind: 2,
    tip: "cccccccccccccccccccccccccccccccccccccccc",
  },
];

const remotes: Remote[] = [
  {
    name: "origin",
    url: "/remote/origin.git",
    fetch: "/remote/origin.git",
    push: null,
  },
];

const tags: Tag[] = [
  {
    name: "v1",
    target: "dddddddddddddddddddddddddddddddddddddd",
    annotated: true,
    message: "first release",
  },
  {
    name: "v2",
    target: "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee",
    annotated: false,
    message: null,
  },
];

const meta: RepoMeta = {
  id: REPO,
  name: "alpha",
  path: REPO,
  branch: "main",
  ahead: 1,
  behind: 0,
  dirty_count: 0,
};

const status: Status = {
  branch: "main",
  detached_head: null,
  head: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
  ahead: 1,
  behind: 0,
  upstream: "origin/main",
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
  planOp.mockReset().mockResolvedValue(plan("git branch -d side"));
  runOp.mockReset().mockResolvedValue(null);
  loadStatus.mockReset().mockResolvedValue(status);
  loadBranches.mockReset().mockResolvedValue(branches);
  loadRemotes.mockReset().mockResolvedValue(remotes);
  loadTags.mockReset().mockResolvedValue(tags);
  loadGraph.mockReset().mockResolvedValue(null);
  onOpFinished.mockReset().mockResolvedValue(null);
}

beforeEach(() => {
  cleanup();
  seed();
});

/** Flush the microtask queue deterministically (no wall-clock timers). */
async function flush(): Promise<void> {
  await act(async () => {});
}

// ── refs store ──────────────────────────────────────────────────────────────

test("refs store loads branches, remotes, and tags for the repo", async () => {
  await act(async () => {
    await useRefsStore.getState().load(REPO);
  });
  expect(loadBranches).toHaveBeenCalledWith(REPO);
  expect(loadRemotes).toHaveBeenCalledWith(REPO);
  expect(loadTags).toHaveBeenCalledWith(REPO);
  const s = useRefsStore.getState();
  expect(s.branches).toHaveLength(3);
  expect(s.remotes).toHaveLength(1);
  expect(s.tags).toHaveLength(2);
  expect(s.error).toBeNull();
});

// ── Branches tab ────────────────────────────────────────────────────────────

test("branches tab renders rows with current, upstream, and sync state", async () => {
  render(<RefsPanel tab="branches" />);
  const body = document.body.textContent ?? "";
  expect(body).toContain("main");
  expect(body).toContain("side");
  expect(body).toContain("origin/main");
  expect(body).toContain("current");
  expect(body).toContain("origin/main"); // upstream
  expect(body).toContain("1"); // ahead badge
  // The current branch offers no switch action.
  const rows = screen.getAllByRole("listitem");
  expect(rows).toHaveLength(3);
});

test("branch switch/plan hint and delete action compile the right ops", async () => {
  render(<RefsPanel tab="branches" />);
  // Switch the non-current local branch.
  fireEvent.click(screen.getByRole("button", { name: "Switch" }));
  await flush();
  expect(planOp).toHaveBeenCalledWith(REPO, {
    kind: "checkout",
    target: "side",
    force: false,
  });
  useActivityStore.getState().dismissPreview();

  // Delete the current branch (the plan will warn; the op must still be sent).
  fireEvent.click(screen.getAllByRole("button", { name: "Delete" })[0]);
  await flush();
  expect(planOp).toHaveBeenCalledWith(REPO, {
    kind: "branch_delete",
    name: "main",
    remote: null,
    force: false,
  });
});

test("branch rename and push actions compile the right ops", async () => {
  render(<RefsPanel tab="branches" />);
  const sideRow = screen
    .getAllByRole("listitem")
    .find((li) => li.textContent?.includes("side") && !li.textContent?.includes("origin/side"));

  fireEvent.click(within(sideRow!).getByRole("button", { name: "Rename" }));
  await flush();
  // The rename dialog opens, prefilled with the current name.
  const dialog = screen.getByRole("dialog", { name: "Rename branch" });
  const input = within(dialog).getByRole("textbox") as HTMLInputElement;
  expect(input.value).toBe("side");
  fireEvent.change(input, { target: { value: "side2" } });
  fireEvent.click(within(dialog).getByRole("button", { name: "Rename" }));
  await flush();
  expect(planOp).toHaveBeenCalledWith(REPO, {
    kind: "branch_rename",
    name: "side",
    new_name: "side2",
  });
  useActivityStore.getState().dismissPreview();

  // Push the branch (set upstream): it has no upstream, so it goes to `origin`.
  fireEvent.click(within(sideRow!).getByRole("button", { name: "Push" }));
  await flush();
  expect(planOp).toHaveBeenCalledWith(REPO, {
    kind: "push",
    remote: "origin",
    branch: "side",
    force: false,
    set_upstream: true,
  });
});

test("create branch dialog sends branch_create from HEAD", async () => {
  render(<RefsPanel tab="branches" />);
  fireEvent.click(screen.getByRole("button", { name: "Create" }));
  await flush();
  const dialog = screen.getByRole("dialog", { name: "Create branch" });
  const input = within(dialog).getByRole("textbox") as HTMLInputElement;
  expect(input.value).toBe("");
  fireEvent.change(input, { target: { value: "feature" } });
  fireEvent.click(within(dialog).getByRole("button", { name: "Create" }));
  await flush();
  expect(planOp).toHaveBeenCalledWith(REPO, {
    kind: "branch_create",
    name: "feature",
    from: status.head,
    checkout: false,
  });
});

// ── Remotes tab ─────────────────────────────────────────────────────────────

test("remotes tab renders rows and fetch/push/remove actions", async () => {
  render(<RefsPanel tab="remotes" />);
  const body = document.body.textContent ?? "";
  expect(body).toContain("origin");
  expect(body).toContain("/remote/origin.git");

  fireEvent.click(screen.getByRole("button", { name: "Fetch" }));
  await flush();
  expect(planOp).toHaveBeenCalledWith(REPO, {
    kind: "remote_fetch",
    remote: "origin",
    refspec: null,
  });
  useActivityStore.getState().dismissPreview();

  fireEvent.click(screen.getByRole("button", { name: "Push" }));
  await flush();
  expect(planOp).toHaveBeenCalledWith(REPO, {
    kind: "push",
    remote: "origin",
    branch: null,
    force: false,
    set_upstream: false,
  });
  useActivityStore.getState().dismissPreview();

  fireEvent.click(screen.getByRole("button", { name: "Remove" }));
  await flush();
  expect(planOp).toHaveBeenCalledWith(REPO, {
    kind: "remote_remove",
    name: "origin",
  });
});

test("add remote dialog sends remote_add", async () => {
  render(<RefsPanel tab="remotes" />);
  fireEvent.click(screen.getByRole("button", { name: "Add" }));
  await flush();
  const dialog = screen.getByRole("dialog", { name: "Add remote" });
  const inputs = within(dialog).getAllByRole("textbox");
  fireEvent.change(inputs[0], { target: { value: "upstream" } });
  fireEvent.change(inputs[1], { target: { value: "git@host:org/repo.git" } });
  fireEvent.click(within(dialog).getByRole("button", { name: "Add" }));
  await flush();
  expect(planOp).toHaveBeenCalledWith(REPO, {
    kind: "remote_add",
    name: "upstream",
    url: "git@host:org/repo.git",
  });
});

// ── Tags tab ────────────────────────────────────────────────────────────────

test("tags tab renders rows and checkout/delete/push actions", async () => {
  render(<RefsPanel tab="tags" />);
  const body = document.body.textContent ?? "";
  expect(body).toContain("v1");
  expect(body).toContain("v2");
  expect(body).toContain("annotated");
  expect(body).toContain("first release");

  fireEvent.click(screen.getAllByRole("button", { name: "Checkout" })[0]);
  await flush();
  expect(planOp).toHaveBeenCalledWith(REPO, {
    kind: "tag_checkout",
    name: "v1",
    force: false,
  });
  useActivityStore.getState().dismissPreview();

  fireEvent.click(screen.getAllByRole("button", { name: "Push" })[0]);
  await flush();
  expect(planOp).toHaveBeenCalledWith(REPO, {
    kind: "tag_push",
    remote: "origin",
    name: "v1",
    force: false,
  });
  useActivityStore.getState().dismissPreview();

  fireEvent.click(screen.getAllByRole("button", { name: "Delete" })[0]);
  await flush();
  expect(planOp).toHaveBeenCalledWith(REPO, {
    kind: "tag_delete",
    name: "v1",
    remote: null,
  });
});

test("create tag dialog (annotated) sends tag_create with a message", async () => {
  render(<RefsPanel tab="tags" />);
  fireEvent.click(screen.getByRole("button", { name: "Create" }));
  await flush();
  const dialog = screen.getByRole("dialog", { name: "Create tag" });
  const inputs = within(dialog).getAllByRole("textbox");
  fireEvent.change(inputs[0], { target: { value: "v3" } });
  // Toggle annotated and fill the message.
  fireEvent.click(within(dialog).getByRole("checkbox"));
  const msgInput = within(dialog).getAllByRole("textbox")[1] as HTMLInputElement;
  fireEvent.change(msgInput, { target: { value: "third" } });
  fireEvent.click(within(dialog).getByRole("button", { name: "Create" }));
  await flush();
  expect(planOp).toHaveBeenCalledWith(REPO, {
    kind: "tag_create",
    name: "v3",
    target: status.head,
    annotated: true,
    message: "third",
  });
});

// ── graph/refs refresh on a finished op (SPEC §10.1/§10.3) ─────────────────

test("a finished op reloads the graph and the ref lists", async () => {
  render(<App />);
  await flush();
  // App subscribed to the finished-op event; the mock captured the callback.
  expect(onOpFinished).toHaveBeenCalled();
  const cb = onOpFinished.mock.calls[0][0] as (p: { repo: string; conflicts: string[] }) => void;

  loadBranches.mockClear();
  loadGraph.mockClear();
  await act(async () => {
    cb({ repo: REPO, conflicts: [] });
  });
  await flush();

  // The graph ref labels come from the reload; the ref lists follow it.
  expect(loadGraph).toHaveBeenCalledWith(REPO);
  expect(loadBranches).toHaveBeenCalledWith(REPO);
  expect(loadRemotes).toHaveBeenCalledWith(REPO);
  expect(loadTags).toHaveBeenCalledWith(REPO);
});

test("the Branches tab is a main tab of the shell", async () => {
  render(<App />);
  await flush();
  const tab = screen.getByRole("tab", { name: "Branches" });
  fireEvent.click(tab);
  await flush();
  expect(document.body.textContent).toContain("Create");
});
