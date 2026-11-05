import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { cleanup, fireEvent, render, screen, waitFor } from "@testing-library/react";
import type { ChangeEvent } from "react";

import { ROOT_GROUP_ID, type RepoMeta, type Workspace } from "./types";

/**
 * The nav's folder picker (SPEC §7 "Add repo"). `flags` is what the mocked
 * bridge records, so the assertions are about the picker's *effect on the open
 * affordance*, not about the native dialog itself.
 */
const flags = vi.hoisted(() => ({
  inTauri: true,
  /** What the fake OS dialog returns: a folder, or `null` for cancelled. */
  pick: null as string | null,
  /** The starting directory the app asked for. */
  defaultPath: "unset" as string | undefined,
  opened: [] as string[],
}));

vi.mock("@tauri-apps/api/core", () => ({
  isTauri: () => flags.inTauri,
  invoke: vi.fn().mockResolvedValue(null),
}));

vi.mock("./api", () => ({
  openRepo: async (path: string) => {
    flags.opened.push(path);
    const meta: RepoMeta = {
      id: path,
      name: path.split("/").pop() ?? path,
      path,
      branch: "main",
      ahead: 0,
      behind: 0,
      dirty_count: 0,
    };
    return meta;
  },
  loadStatus: async () => null,
  onStatusChanged: async () => () => {},
  pickRepositoryFolder: async (defaultPath?: string) => {
    flags.defaultPath = defaultPath;
    return flags.pick;
  },
  workspaceGet: async () => null,
  workspaceSet: async () => null,
  workspaceRepos: async () => [],
  groupAdd: async () => null,
  groupRename: async () => null,
  groupDelete: async () => null,
  repoPin: async () => null,
  repoTouch: async () => null,
  removeRepoFromWorkspace: async () => null,
}));

import { RepoNav } from "./components/RepoNav";
import { useRepoStore, useViewStore, useWorkspaceStore } from "./store";

const ACTIVE = "/repos/alpha";

const TREE: Workspace = {
  groups: [{ id: ROOT_GROUP_ID, name: "Workspace", children: [{ kind: "repo", id: ACTIVE }] }],
  pinned: [],
  recents: [],
  last_open: null,
};

function seed(error: string | null = null): void {
  flags.inTauri = true;
  flags.pick = null;
  flags.defaultPath = "unset";
  flags.opened = [];
  useRepoStore.setState(() => ({
    repos: [
      { id: ACTIVE, name: "alpha", path: ACTIVE, branch: "main", ahead: 0, behind: 0, dirty_count: 0 },
    ],
    status: {},
    activeId: ACTIVE,
    opening: false,
    error,
  }));
  useWorkspaceStore.setState(() => ({ tree: structuredClone(TREE) }));
  useViewStore.setState({ dashboard: false });
}

beforeEach(() => seed());
afterEach(cleanup);

function openField(): HTMLInputElement {
  return screen.getByPlaceholderText(/Open a repo/) as HTMLInputElement;
}

function browseButton(): HTMLElement {
  return screen.getByRole("button", { name: /Browse for a repository folder/ });
}

test("Browse fills the open field with the picked folder and starts the dialog in the active repo's parent", async () => {
  flags.pick = "/Users/me/code/new-repo";
  render(<RepoNav />);

  fireEvent.click(browseButton());

  await waitFor(() => expect(openField().value).toBe("/Users/me/code/new-repo"));
  expect(flags.defaultPath).toBe("/repos");
  // Picking is not opening: the path is shown, the repo is not added yet.
  expect(flags.opened).toEqual([]);
});

test("a picked path replaces an earlier open failure instead of stacking errors", async () => {
  seed("not a git repository: /repos/typo");
  flags.pick = "/Users/me/code/real-repo";
  render(<RepoNav />);

  fireEvent.click(browseButton());

  await waitFor(() => expect(openField().value).toBe("/Users/me/code/real-repo"));
  expect(screen.queryByRole("alert")).toBeNull();
});

test("the picked folder opens when the form is submitted", async () => {
  flags.pick = "/Users/me/code/new-repo";
  render(<RepoNav />);

  fireEvent.click(browseButton());
  await waitFor(() => expect(openField().value).toBe("/Users/me/code/new-repo"));
  fireEvent.click(screen.getByRole("button", { name: "Open" }));

  await waitFor(() => expect(flags.opened).toEqual(["/Users/me/code/new-repo"]));
});

test("cancelling the picker leaves the field and the open error exactly as they were", () => {
  seed("not a git repository: /repos/typo");
  flags.pick = null;
  render(<RepoNav />);

  fireEvent.click(browseButton());

  expect(openField().value).toBe("");
  expect(screen.getByRole("alert").textContent).toContain("not a git repository");
  expect(flags.opened).toEqual([]);
});

test("the picker is absent when the bundle runs outside Tauri", async () => {
  flags.inTauri = false;
  render(<RepoNav />);

  expect(screen.queryByRole("button", { name: /Browse for a repository folder/ })).toBeNull();
  // The typed path still works there.
  fireEvent.change(openField(), { target: { value: "/typed/path" } } as ChangeEvent<HTMLInputElement>);
  fireEvent.click(screen.getByRole("button", { name: "Open" }));
  await waitFor(() => expect(flags.opened).toEqual(["/typed/path"]));
});
