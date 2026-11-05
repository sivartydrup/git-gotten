import { beforeEach, expect, test, vi } from "vitest";
import { readFileSync } from "node:fs";

import {
  BRANCH_PALETTE,
  BRANCH_PALETTE_LIGHT,
  emptyWorkspace,
  paletteColor,
  resolveGraphColors,
  ROOT_GROUP_ID,
  type ChildNode,
  type Workspace,
} from "./types";

/** Mutable fake for the workspace IPC: mirrors the Rust tree semantics. */
const fake = vi.hoisted(() => ({
  tree: null as Workspace | null,
  pairs: null as { meta: unknown; status: unknown }[] | null,
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
  loadStashes: async () => [],
  loadSubmodules: async () => [],
  loadBisect: async () => ({ active: false, current: null, good: [], bad: [], remaining: 0, found: null, log: [] }),
  bisectTest: async () => ({ active: false, current: null, good: [], bad: [], remaining: 0, found: null, log: [] }),
  workspaceGet: async () => fake.clone(),
  workspaceRepos: async () => fake.pairs,
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
  groupRename: async (id: string, name: string) => {
    if (!fake.tree) return null;
    fake.tree.groups.find((g) => g.id === id)!.name = name;
    return fake.clone();
  },
  groupDelete: async (id: string) => {
    if (!fake.tree) return null;
    const subtree = [id];
    const gone = new Set<string>([id]);
    const hoisted: ChildNode[] = [];
    while (subtree.length) {
      const g = fake.tree.groups.find((x) => x.id === subtree.pop());
      if (!g) continue;
      for (const c of g.children) {
        if (c.kind === "group") {
          subtree.push(c.id);
          gone.add(c.id);
        } else {
          hoisted.push(c);
        }
      }
    }
    fake.tree.groups = fake.tree.groups.filter((g) => !gone.has(g.id));
    for (const g of fake.tree.groups)
      g.children = g.children.filter((c) => c.kind !== "group" || !gone.has(c.id));
    fake.tree.groups.find((g) => g.id === "root")!.children.push(...hoisted);
    return fake.clone();
  },
  repoPin: async (id: string, pinned: boolean) => {
    if (!fake.tree) return null;
    fake.tree.pinned = pinned ? [id] : [];
    return fake.clone();
  },
  repoTouch: async (id: string) => {
    if (!fake.tree) return null;
    fake.tree.recents = [id, ...fake.tree.recents.filter((r) => r !== id)];
    return fake.clone();
  },
  removeRepoFromWorkspace: async (id: string) => {
    if (!fake.tree) return null;
    for (const g of fake.tree.groups)
      g.children = g.children.filter((c) => !(c.kind === "repo" && c.id === id));
    fake.tree.pinned = fake.tree.pinned.filter((r) => r !== id);
    fake.tree.recents = fake.tree.recents.filter((r) => r !== id);
    return fake.clone();
  },
}));

import {
  moveNode,
  navOrder,
  repoIdsInTree,
  useRepoStore,
  useThemeStore,
  useWorkspaceStore,
} from "./store";

function sampleTree(): Workspace {
  return {
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
      {
        id: "g1",
        name: "Work",
        children: [
          { kind: "repo", id: "/r/work" },
          { kind: "group", id: "g2" },
        ],
      },
      { id: "g2", name: "Nested", children: [{ kind: "repo", id: "/r/nested" }] },
    ],
    pinned: ["/r/alpha"],
    recents: ["/r/beta"],
    last_open: null,
  };
}

beforeEach(() => {
  fake.tree = sampleTree();
  fake.pairs = null;
  useWorkspaceStore.setState({ tree: sampleTree() });
  useRepoStore.setState({ repos: [], activeId: null, status: {}, opening: false, error: null });
});

// ── tree ordering (SPEC §7) ──────────────────────────────────────────────────

test("navOrder: pinned first, then groups, then unpinned repos by recents", () => {
  const tree = sampleTree();
  expect(navOrder(tree, ROOT_GROUP_ID).map((c) => c.id)).toEqual(["/r/alpha", "g1", "/r/beta"]);
  // Within `g1`: group g2 before the unpinned repo (recents has no entry for it).
  expect(navOrder(tree, "g1").map((c) => c.id)).toEqual(["g2", "/r/work"]);
});

test("repoIdsInTree walks breadth-first from the root", () => {
  expect(repoIdsInTree(sampleTree())).toEqual(["/r/beta", "/r/alpha", "/r/work", "/r/nested"]);
});

test("moveNode re-homes a repo and re-keys recents to the tree order", () => {
  const moved = moveNode(sampleTree(), { kind: "repo", id: "/r/alpha" }, "g1", 0);
  expect(moved.groups.find((g) => g.id === "g1")!.children[0]).toEqual({
    kind: "repo",
    id: "/r/alpha",
  });
  // Recents mirror the new tree order for unpinned repos (DnD reorder wins).
  expect(moved.recents).toEqual(["/r/beta", "/r/work", "/r/nested"]);
  expect(moved.pinned).toEqual(["/r/alpha"]);
  expect(navOrder(moved, "g1").map((c) => c.id)).toEqual(["/r/alpha", "g2", "/r/work"]);
});

test("moveNode reorders groups at the root", () => {
  const moved = moveNode(sampleTree(), { kind: "group", id: "g1" }, ROOT_GROUP_ID, 0);
  expect(moved.groups.find((g) => g.id === ROOT_GROUP_ID)!.children[0]).toEqual({
    kind: "group",
    id: "g1",
  });
});

test("moveNode refuses to drop a group into its own subtree", () => {
  const tree = sampleTree();
  expect(moveNode(tree, { kind: "group", id: "g1" }, "g2", 0)).toEqual(tree);
});

// ── workspace store over the IPC fake ────────────────────────────────────────

test("load hydrates the tree and the repo store from workspace_repos", async () => {
  fake.tree = sampleTree();
  fake.pairs = [
    {
      meta: { id: "/r/alpha", name: "alpha", path: "/r/alpha", branch: "main", ahead: 1, behind: 0, dirty_count: 2 },
      status: {
        branch: "main", detached_head: null, head: "h", ahead: 1, behind: 0, upstream: "origin/main",
        changes: [], in_merge: false, in_rebase: false, in_cherry_pick: false,
        last_commit: { hash: "h", subject: "c", author_time: 1 },
      },
    },
  ];
  await useWorkspaceStore.getState().load();
  expect(useWorkspaceStore.getState().tree.groups).toHaveLength(3);
  expect(useRepoStore.getState().repos.map((r) => r.id)).toEqual(["/r/alpha"]);
  expect(useRepoStore.getState().status["/r/alpha"]).toBeTruthy();
});

function pair(id: string) {
  return {
    meta: { id, name: id.split("/").pop()!, path: id, branch: "main", ahead: 0, behind: 0, dirty_count: 0 },
    status: {
      branch: "main", detached_head: null, head: "h", ahead: 0, behind: 0, upstream: null,
      changes: [], in_merge: false, in_rebase: false, in_cherry_pick: false,
      last_commit: { hash: "h", subject: "c", author_time: 1 },
    },
  };
}

test("load reopens the last-open repo so a restart lands back on it (SPEC §11)", async () => {
  fake.tree = { ...sampleTree(), last_open: "/r/alpha" };
  fake.pairs = [pair("/r/beta"), pair("/r/alpha")];
  await useWorkspaceStore.getState().load();
  expect(useRepoStore.getState().activeId).toBe("/r/alpha");
});

test("load never steals a repo the user already activated", async () => {
  fake.tree = { ...sampleTree(), last_open: "/r/alpha" };
  fake.pairs = [pair("/r/beta"), pair("/r/alpha")];
  useRepoStore.setState({ activeId: "/r/beta" });
  await useWorkspaceStore.getState().load();
  expect(useRepoStore.getState().activeId).toBe("/r/beta");
});

test("load ignores a last-open repo that is no longer in the workspace", async () => {
  fake.tree = { ...sampleTree(), last_open: "/r/gone" };
  fake.pairs = [pair("/r/alpha")];
  await useWorkspaceStore.getState().load();
  expect(useRepoStore.getState().activeId).toBeNull();
});

test("removeRepo unlinks the repo from the tree and the repo list", async () => {
  useRepoStore.setState(() => ({
    repos: [{ id: "/r/alpha", name: "alpha", path: "/r/alpha", branch: "main", ahead: 0, behind: 0, dirty_count: 0 }],
  }));
  await useWorkspaceStore.getState().removeRepo("/r/alpha");
  expect(repoIdsInTree(useWorkspaceStore.getState().tree)).not.toContain("/r/alpha");
  expect(useRepoStore.getState().repos).toEqual([]);
});

test("addGroup + renameGroup + deleteGroup round-trip through the fake", async () => {
  await useWorkspaceStore.getState().addGroup("Personal", null);
  const added = useWorkspaceStore.getState().tree.groups.find((g) => g.name === "Personal");
  expect(added).toBeTruthy();
  await useWorkspaceStore.getState().renameGroup(added!.id, "Home");
  expect(useWorkspaceStore.getState().tree.groups.find((g) => g.id === added!.id)!.name).toBe("Home");
  await useWorkspaceStore.getState().deleteGroup(added!.id);
  expect(useWorkspaceStore.getState().tree.groups.find((g) => g.id === added!.id)).toBeUndefined();
});

// ── theming (SPEC §12) ───────────────────────────────────────────────────────

test("theme mode drives data-theme and the lane palette", () => {
  useThemeStore.getState().setMode("light");
  expect(document.documentElement.dataset.theme).toBe("light");
  expect(localStorage.getItem("gitgotten.theme")).toBe("light");
  expect(resolveGraphColors().palette).toEqual(BRANCH_PALETTE_LIGHT);
  expect(resolveGraphColors().fg).toBeTruthy();

  useThemeStore.getState().setMode("dark");
  expect(document.documentElement.dataset.theme).toBe("dark");
  expect(resolveGraphColors().palette).toEqual(BRANCH_PALETTE);

  useThemeStore.getState().setMode("system");
  expect(["light", "dark"]).toContain(document.documentElement.dataset.theme);
  expect(localStorage.getItem("gitgotten.theme")).toBe("system");
});

test("paletteColor wraps indices over either palette", () => {
  expect(paletteColor(21, BRANCH_PALETTE_LIGHT)).toBe(BRANCH_PALETTE_LIGHT[1]);
  expect(paletteColor(0)).toBe(BRANCH_PALETTE[0]);
});

test("styles.css defines every token for both themes (SPEC §12)", () => {
  // vitest runs with the web package root as cwd; the stylesheet sits under src/.
  const css = readFileSync("src/styles.css", "utf8");
  const tokens = ["--bg", "--fg", "--muted", "--accent", "--success", "--warning", "--error", "--border", "--surface"];
  const darkBlock = css.match(/:root\s*\{([^}]*)\}/)?.[1] ?? "";
  const lightBlock = css.match(/:root\[data-theme="light"\]\s*\{([^}]*)\}/)?.[1] ?? "";
  for (const t of tokens) {
    expect(darkBlock, `dark ${t}`).toContain(t);
    expect(lightBlock, `light ${t}`).toContain(t);
  }
});

test("emptyWorkspace is root-only (mirrors Rust Workspace::new)", () => {
  const ws = emptyWorkspace();
  expect(ws.groups).toHaveLength(1);
  expect(ws.groups[0].id).toBe(ROOT_GROUP_ID);
  expect(ws.pinned).toEqual([]);
  expect(ws.recents).toEqual([]);
});
