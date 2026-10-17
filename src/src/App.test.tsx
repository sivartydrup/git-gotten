import { afterEach, expect, test, vi } from "vitest";
import { act, cleanup, render } from "@testing-library/react";
import App from "./App";
import { useRepoStore } from "./store";
import type { RepoMeta, Status } from "./types";

// Mock the IPC layer: non-Tauri behavior (all calls resolve `null`), so the app
// renders its plain-web fallback exactly as it does outside Tauri.
vi.mock("./api", () => ({
  fetchBuildInfo: vi.fn().mockResolvedValue(null),
  openRepo: vi.fn().mockResolvedValue(null),
  loadStatus: vi.fn().mockResolvedValue(null),
  onStatusChanged: vi.fn().mockResolvedValue(null),
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
});

test("renders the GitGotten shell (non-Tauri fallback)", async () => {
  render(<App />);
  // `render` runs the effects that call the async IPC; outside Tauri those
  // resolve `null`. Flush the microtasks so the fallback branch commits.
  await act(async () => {});
  expect(document.body.textContent).toContain("GitGotten");
  expect(document.body.textContent).toContain("Running outside Tauri");
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
    ahead: 1,
    behind: 0,
    upstream: "origin/main",
    changes: [
      { path: "src/a.ts", kind: "Modified", staged: false, binary: false },
      { path: "README.md", kind: "Untracked", staged: false, binary: false },
    ],
    in_merge: false,
    in_rebase: false,
  };
  useRepoStore.setState({ repos: [meta], activeId: meta.id, status: { [meta.id]: status } });

  render(<App />);
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
        ahead: 0,
        behind: 0,
        upstream: null,
        changes: [],
        in_merge: false,
        in_rebase: true,
      },
    },
  });

  render(<App />);
  await act(async () => {});

  const body = document.body.textContent ?? "";
  expect(body).toContain("Working tree clean");
  expect(body).toContain("REBASE"); // in-rebase state badge
});
