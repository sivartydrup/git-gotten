import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";

import {
  ROOT_GROUP_ID,
  type RepoMeta,
  type Status,
  type Workspace,
} from "./types";

const fake = vi.hoisted(() => ({
  tree: null as unknown as Workspace | null,
  clone: () => (fake.tree ? structuredClone(fake.tree) : null),
}));

vi.mock("./api", () => ({
  loadRebaseCommits: vi.fn().mockResolvedValue(null),
  fetchBuildInfo: vi.fn().mockResolvedValue(null),
  openRepo: vi.fn().mockResolvedValue(null),
  loadStatus: vi.fn().mockResolvedValue(null),
  onStatusChanged: vi.fn().mockResolvedValue(null),
  planOp: vi.fn().mockResolvedValue(null),
  runOp: vi.fn().mockResolvedValue(null),
  loadLog: vi.fn().mockResolvedValue(null),
  onOpStarted: vi.fn().mockResolvedValue(null),
  onOpFinished: vi.fn().mockResolvedValue(null),
  loadGraph: vi.fn().mockResolvedValue(null),
  loadCommit: vi.fn().mockResolvedValue(null),
  loadBranches: async () => [],
  loadRemotes: async () => [],
  loadTags: async () => [],
  loadWorktrees: async () => [],
  loadStashes: async () => [],
  loadSubmodules: async () => [],
  loadBisect: async () => ({ active: false, current: null, good: [], bad: [], remaining: 0, found: null, log: [] }),
  bisectTest: async () => ({ active: false, current: null, good: [], bad: [], remaining: 0, found: null, log: [] }),
  workspaceGet: async () => fake.clone(),
  workspaceRepos: async () => [],
  workspaceSet: async (ws: Workspace) => {
    fake.tree = structuredClone(ws);
    return fake.clone();
  },
  groupAdd: async (name: string, parent: string | null) => {
    if (!fake.tree) return null;
    const id = `g${fake.tree.groups.length}`;
    fake.tree.groups.push({ id, name, children: [] });
    fake.tree.groups.find((g) => g.id === (parent ?? "root"))!.children.push({ kind: "group", id });
    return fake.clone();
  },
  groupRename: async () => fake.clone(),
  groupDelete: async (id: string) => {
    if (!fake.tree) return null;
    fake.tree.groups = fake.tree.groups.filter((g) => g.id !== id);
    for (const g of fake.tree.groups)
      g.children = g.children.filter((c) => c.kind !== "group" || c.id !== id);
    return fake.clone();
  },
  repoPin: async (id: string, pinned: boolean) => {
    if (!fake.tree) return null;
    fake.tree.pinned = pinned ? [id] : [];
    return fake.clone();
  },
  repoTouch: async () => fake.clone(),
  removeRepoFromWorkspace: async (id: string) => {
    if (!fake.tree) return null;
    for (const g of fake.tree.groups)
      g.children = g.children.filter((c) => !(c.kind === "repo" && c.id === id));
    fake.tree.pinned = fake.tree.pinned.filter((r) => r !== id);
    fake.tree.recents = fake.tree.recents.filter((r) => r !== id);
    return fake.clone();
  },
}));

import { Dashboard } from "./components/Dashboard";
import { RepoNav } from "./components/RepoNav";
import { useRepoStore, useViewStore, useWorkspaceStore } from "./store";

function meta(id: string, name: string, over: Partial<RepoMeta> = {}): RepoMeta {
  return { id, name, path: id, branch: "main", ahead: 0, behind: 0, dirty_count: 0, ...over };
}

function status(over: Partial<Status> = {}): Status {
  return {
    branch: "main",
    detached_head: null,
    head: "abc1234",
    ahead: 0,
    behind: 0,
    upstream: "origin/main",
    changes: [],
    in_merge: false,
    in_rebase: false,
    in_cherry_pick: false,
    last_commit: { hash: "abc1234def", subject: "fix: ship it", author_time: 1_730_000_000 },
    ...over,
  };
}

function seed(): void {
  fake.tree = {
    groups: [
      {
        id: ROOT_GROUP_ID,
        name: "Workspace",
        children: [
          { kind: "repo", id: "/r/beta" },
          { kind: "group", id: "g1" },
          { kind: "repo", id: "/r/alpha" },
        ],
      },
      { id: "g1", name: "Work", children: [{ kind: "repo", id: "/r/work" }] },
    ],
    pinned: ["/r/alpha"],
    recents: ["/r/beta"],
    last_open: null,
  };
  useRepoStore.setState(() => ({
    repos: [
      meta("/r/alpha", "alpha", { dirty_count: 3, ahead: 2, behind: 1 }),
      meta("/r/beta", "beta"),
      meta("/r/work", "work", { branch: "dev" }),
    ],
    status: {
      "/r/alpha": status(),
      "/r/beta": status({ in_rebase: true, upstream: null }),
      "/r/work": status({ branch: "dev", last_commit: null }),
    },
    activeId: null,
    opening: false,
    error: null,
  }));
  useWorkspaceStore.setState(() => ({ tree: structuredClone(fake.tree!) }));
  useViewStore.setState({ dashboard: false });
}

beforeEach(seed);
afterEach(cleanup);

// ── RepoNav: group tree, pin, search, remove (SPEC §7) ──────────────────────

test("RepoNav renders nested groups with pinned repos first", () => {
  render(<RepoNav />);
  expect(screen.getByText("alpha")).toBeTruthy();
  const items = [...document.querySelectorAll(".repo-item")];
  const names = items.map((el) => el.querySelector(".repo-name")?.textContent?.trim());
  // In the root, alpha (pinned) renders before beta.
  expect(names.indexOf("alpha")).toBeLessThan(names.indexOf("beta"));
  // Nested group renders its repos.
  expect(screen.getByText("Work")).toBeTruthy();
  expect(screen.getByText("work")).toBeTruthy();
});

test("pin button pins a repo to the top of its group", async () => {
  render(<RepoNav />);
  const betaRow = [...document.querySelectorAll(".repo-item")].find((el) =>
    el.textContent?.includes("beta"),
  )!;
  fireEvent.click(betaRow.querySelector("button[data-act='pin']")!);
  await act(async () => {});
  const names = [...document.querySelectorAll(".repo-item")].map(
    (el) => el.querySelector(".repo-name")?.textContent?.trim(),
  );
  expect(names.indexOf("beta")).toBeLessThan(names.indexOf("alpha"));
  expect(useWorkspaceStore.getState().tree.pinned).toEqual(["/r/beta"]);
});

test("search filters the repo list by name and path", async () => {
  render(<RepoNav />);
  fireEvent.change(screen.getByPlaceholderText(/search/i), { target: { value: "work" } });
  await act(async () => {});
  expect(screen.queryByText("alpha")).toBeNull();
  expect(screen.queryByText("beta")).toBeNull();
  expect(screen.getByText("work")).toBeTruthy();
});

test("remove button unlinks the repo from the workspace", async () => {
  render(<RepoNav />);
  const alphaRow = [...document.querySelectorAll(".repo-item")].find((el) =>
    el.textContent?.includes("alpha"),
  )!;
  fireEvent.click(alphaRow.querySelector("button[data-act='remove']")!);
  await act(async () => {});
  expect(screen.queryByText("alpha")).toBeNull();
  expect(useRepoStore.getState().repos.map((r) => r.id)).not.toContain("/r/alpha");
});

test("group rows offer delete and add-group; deleting hoists children", async () => {
  render(<RepoNav />);
  fireEvent.click(screen.getByLabelText("Add group"));
  await act(async () => {});
  expect(screen.getByText(/New group|New/i)).toBeTruthy();
  const workHeader = [...document.querySelectorAll(".group-head")].find((el) =>
    el.textContent?.includes("Work"),
  )!;
  fireEvent.click(workHeader.querySelector("button[data-act='delete-group']")!);
  await act(async () => {});
  expect(screen.queryByText("Work")).toBeNull();
  // work hoisted to root, still listed.
  expect(screen.getByText("work")).toBeTruthy();
});

test("drag-and-drop re-homes a repo and persists via workspace_set", async () => {
  render(<RepoNav />);
  const alphaRow = [...document.querySelectorAll(".repo-item")].find((el) =>
    el.textContent?.includes("alpha"),
  )!;
  const workHeader = [...document.querySelectorAll(".group-head")].find((el) =>
    el.textContent?.includes("Work"),
  )!;
  fireEvent.dragStart(alphaRow);
  fireEvent.drop(workHeader);
  await act(async () => {});
  const tree = useWorkspaceStore.getState().tree;
  expect(tree.groups.find((g) => g.id === "g1")!.children.some((c) => c.id === "/r/alpha")).toBe(true);
  expect(fake.tree).toEqual(tree); // committed to Rust
});

test("All Repositories button opens the dashboard", async () => {
  render(<RepoNav />);
  fireEvent.click(screen.getByRole("button", { name: /all repos/i }));
  await act(async () => {});
  expect(useViewStore.getState().dashboard).toBe(true);
});

// ── Dashboard: cross-repo table (SPEC §7) ────────────────────────────────────

test("dashboard lists every open repo with branch, state, ahead/behind, last commit", () => {
  render(<Dashboard />);
  const rows = [...document.querySelectorAll("tbody tr")];
  expect(rows).toHaveLength(3);
  const alphaRow = rows.find((r) => r.textContent?.includes("alpha"))!;
  expect(alphaRow.textContent).toContain("main");
  expect(alphaRow.textContent).toContain("3"); // dirty count
  expect(alphaRow.textContent).toContain("↑2");
  expect(alphaRow.textContent).toContain("↓1");
  expect(alphaRow.textContent).toContain("fix: ship it");
  const betaRow = rows.find((r) => r.textContent?.includes("beta"))!;
  expect(betaRow.textContent).toContain("rebasing"); // live state badge
});

test("dashboard rows switch the active repo and close the dashboard", async () => {
  render(<Dashboard />);
  const row = [...document.querySelectorAll("tbody tr")].find((r) =>
    r.textContent?.includes("work"),
  )!;
  fireEvent.click(row);
  await act(async () => {});
  expect(useRepoStore.getState().activeId).toBe("/r/work");
  expect(useViewStore.getState().dashboard).toBe(false);
});

test("dashboard filter narrows rows; header click sorts", async () => {
  render(<Dashboard />);
  fireEvent.change(screen.getByPlaceholderText(/filter/i), { target: { value: "beta" } });
  await act(async () => {});
  expect(document.querySelectorAll("tbody tr")).toHaveLength(1);
  expect(document.body.textContent).toContain("beta");

  fireEvent.change(screen.getByPlaceholderText(/filter/i), { target: { value: "" } });
  fireEvent.click(screen.getByText("Dirty"));
  await act(async () => {});
  const first = document.querySelector("tbody tr")!;
  expect(first.textContent).toContain("alpha"); // 3 dirty sorts first (desc)
});
