import { beforeEach, expect, test, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";

// `../api` is mocked wholesale: word highlighting and the panel's diffstats
// must come from the hunks Rust already delivers, with no stats IPC of their own.
const { loadDiff } = vi.hoisted(() => ({
  loadDiff: vi.fn(
    (_repo: string, path: string) =>
      Promise.resolve(
        path === "src/a.ts"
          ? {
              path,
              binary: false,
              patch: "",
              hunks: [
                { header: "@@ -1,2 +1,3 @@", lines: ["-a", "+b", "+c"], staged: true },
                { header: "@@ -5,2 +5,1 @@", lines: ["-d", "-e", "+f"], staged: false },
              ],
            }
          : {
              path,
              binary: false,
              patch: "",
              hunks: [{ header: "@@ -0,0 +1,2 @@", lines: ["+x", "+y"], staged: false }],
            },
      ),
  ),
}));

vi.mock("../api", () => ({
  workspaceGet: async () => null,
  workspaceRepos: async () => null,
  workspaceSet: async () => null,
  groupAdd: async () => null,
  groupRename: async () => null,
  groupDelete: async () => null,
  repoPin: async () => null,
  repoTouch: async () => null,
  removeRepoFromWorkspace: async () => null,
  loadRebaseCommits: async () => null,
  fetchBuildInfo: async () => null,
  openRepo: async () => null,
  loadStatus: async () => null,
  onStatusChanged: async () => null,
  planOp: async () => null,
  runOp: async () => null,
  loadLog: async () => null,
  onOpStarted: async () => null,
  onOpFinished: async () => null,
  loadGraph: async () => null,
  loadCommit: async () => null,
  loadCommitStats: async () => [],
  loadDiff,
  loadStashes: async () => [],
  loadSubmodules: async () => [],
  loadBisect: async () => ({ active: false, current: null, good: [], bad: [], remaining: 0, found: null, log: [] }),
  bisectTest: async () => ({ active: false, current: null, good: [], bad: [], remaining: 0, found: null, log: [] }),
}));

import { useRepoStore, useStagingStore } from "../store";
import { Diffstat, FileDiff, hunkStats } from "./FileDiff";
import { FilesPanel } from "./FilesPanel";
import type { FileDiff as FileDiffType, Hunk, RepoMeta, Status } from "../types";

const meta: RepoMeta = {
  id: "/repo/alpha",
  name: "alpha",
  path: "/repo/alpha",
  branch: "main",
  ahead: 0,
  behind: 0,
  dirty_count: 0,
};

const writeText = vi.fn().mockResolvedValue(undefined);

function seedRepo(changes: Status["changes"] = []): void {
  const status: Status = {
    branch: "main",
    detached_head: null,
    head: null,
    ahead: 0,
    behind: 0,
    upstream: null,
    changes,
    in_merge: false,
    in_rebase: false,
    in_cherry_pick: false,
    last_commit: null,
  };
  useRepoStore.setState({ repos: [meta], activeId: meta.id, status: { [meta.id]: status }, opening: false, error: null });
}

/** Seed a selected file whose diff is already loaded (no round-trip). */
function seedDiff(diff: FileDiffType): void {
  useStagingStore.setState({ file: diff.path, diff, loading: false, error: null });
}

function setClipboard(value: Clipboard | undefined): void {
  Object.defineProperty(navigator, "clipboard", { configurable: true, writable: true, value });
}

beforeEach(() => {
  cleanup();
  loadDiff.mockReset();
  loadDiff.mockImplementation((_repo: string, path: string) =>
    Promise.resolve(
      path === "src/a.ts"
        ? { path, binary: false, patch: "", hunks: [
            { header: "@@ -1,2 +1,3 @@", lines: ["-a", "+b", "+c"], staged: true },
            { header: "@@ -5,2 +5,1 @@", lines: ["-d", "-e", "+f"], staged: false },
          ] }
        : { path, binary: false, patch: "", hunks: [{ header: "@@ -0,0 +1,2 @@", lines: ["+x", "+y"], staged: false }] },
    ),
  );
  writeText.mockReset();
  setClipboard({ writeText } as unknown as Clipboard);
  useStagingStore.getState().reset();
  useRepoStore.setState({ repos: [], activeId: null, status: {}, opening: false, error: null });
});

/** Advance the fake clock past the panel's 60 ms settle so its reads run. */
async function settle(): Promise<void> {
  await act(async () => {
    vi.advanceTimersByTime(120);
  });
}

/** The `<span>` structure of the shown added line, as [class, text] pairs. */
function addedSpans(): [string, string][] {
  const line = document.querySelector(".diff-line.add")!;
  return Array.from(line.querySelectorAll("span")).map((el) => [el.className, el.textContent ?? ""]);
}

// ── word-level highlighting ─────────────────────────────────────────────────

test("a changed pair marks only the differing runs and keeps the rest intact", () => {
  seedRepo();
  seedDiff({
    path: "a.ts",
    binary: false,
    patch: "",
    hunks: [
      {
        header: "@@ -1,3 +1,3 @@",
        lines: [" context keeps", '-const count = compute(items, "old")', '+const count = compute(items, "new")', " tail keeps"],
        staged: false,
      },
    ],
  });

  render(<FileDiff />);

  expect(addedSpans()).toEqual([
    ["", "const"],
    ["", " "],
    ["", "count"],
    ["", " "],
    ["", "="],
    ["", " "],
    ["", "compute(items,"],
    ["", " "],
    ["wadd", '"new")'],
  ]);
  // Identical surrounding text survives, in order, unmarked.
  expect(document.querySelector(".diff-line.add")!.textContent).toBe('const count = compute(items, "new")');
  // Context lines get no word marks at all.
  expect(document.querySelectorAll(".diff-line.ctx .wadd, .diff-line.ctx .wdel")).toHaveLength(0);
});

test("a pure insertion marks the whole added line", () => {
  seedRepo();
  seedDiff({ path: "a.ts", binary: false, patch: "", hunks: [{ header: "@@ -1,0 +1,1 @@", lines: ["+brand new line"], staged: false }] });

  render(<FileDiff />);

  expect(addedSpans()).toEqual([["wadd", "brand new line"]]);
});

test("word marks never cover leading whitespace", () => {
  seedRepo();
  seedDiff({
    path: "a.ts",
    binary: false,
    patch: "",
    hunks: [{ header: "@@ -4,1 +4,1 @@", lines: ["-    let a = 1", "+    let b = 1"], staged: true }],
  });

  render(<FileDiff />);

  const spans = addedSpans();
  const marks = spans.filter(([cls]) => cls === "wadd");
  // The indentation run is plain text, not part of the mark.
  expect(marks).toEqual([["wadd", "b"]]);
  expect(spans[0][0]).toBe("");
  // The line's text — and with it its `white-space: pre` alignment — is unchanged.
  expect(document.querySelector(".diff-line.add")!.textContent).toBe("    let b = 1");
  expect(document.querySelectorAll(".diff-line.add .wadd")).toHaveLength(1);
});

test("an lopsided del/add block marks whole lines instead of guessing pairs", () => {
  seedRepo();
  seedDiff({
    path: "a.ts",
    binary: false,
    patch: "",
    hunks: [{ header: "@@ -1,4 +1,1 @@", lines: ["-one", "-two", "-three", "+replaced"], staged: false }],
  });

  render(<FileDiff />);

  expect(addedSpans()).toEqual([["wadd", "replaced"]]);
});

// ── per-file diffstat from the delivered hunks ──────────────────────────────

test("hunkStats counts delivered hunk lines only", () => {
  const hunks: Hunk[] = [
    { header: "@@ -1,2 +1,3 @@", lines: [" ctx", "-a", "+b", "+c"], staged: true },
    { header: "@@ -9,1 +9,2 @@", lines: ["-d", "+e", "+f", "+g", " ctx"], staged: false },
  ];
  expect(hunkStats(hunks)).toEqual({ added: 5, removed: 2 });
  expect(hunkStats([{ header: "@@ -1,1 +1,1 @@", lines: [" ctx", "\\ No newline at end of file"], staged: false }])).toEqual({
    added: 0,
    removed: 0,
  });
});

test("the diff header shows one five-segment stat per staging side", () => {
  seedRepo();
  seedDiff({
    path: "a.ts",
    binary: false,
    patch: "diff",
    hunks: [
      { header: "@@ -1,2 +1,3 @@", lines: ["-a", "+b", "+c"], staged: true },
      { header: "@@ -5,2 +5,1 @@", lines: ["-d", "-e", "+f"], staged: false },
    ],
  });

  render(<FileDiff />);

  const staged = screen.getByLabelText("2 added, 1 removed");
  const unstaged = screen.getByLabelText("1 added, 2 removed");
  expect(staged.textContent).toContain("+2");
  expect(staged.textContent).toContain("−1");
  expect(unstaged.textContent).toContain("+1");
  expect(unstaged.textContent).toContain("−2");
  expect(staged.querySelectorAll(".diffstat-seg")).toHaveLength(5);
  expect(staged.querySelectorAll(".diffstat-seg.add")).toHaveLength(3);
  expect(staged.querySelectorAll(".diffstat-seg.del")).toHaveLength(2);
  expect(unstaged.querySelectorAll(".diffstat-seg.add")).toHaveLength(2);
  expect(unstaged.querySelectorAll(".diffstat-seg.del")).toHaveLength(3);
});

test("an empty change fills no segment", () => {
  render(<Diffstat added={0} removed={0} />);
  const stat = screen.getByLabelText("0 added, 0 removed");
  expect(stat.textContent).toBe("+0 −0");
  expect(stat.querySelectorAll(".diffstat-seg.add")).toHaveLength(0);
  expect(stat.querySelectorAll(".diffstat-seg.del")).toHaveLength(0);
});

test("each file row carries its own staged / unstaged counts", async () => {
  vi.useFakeTimers();
  seedRepo([
    { path: "a.ts", kind: "Modified", staged: true, binary: false },
    { path: "a.ts", kind: "Modified", staged: false, binary: false },
    { path: "b.ts", kind: "Untracked", staged: false, binary: false },
  ]);
  loadDiff.mockImplementation(async (_repo: string, path: string) =>
    path === "a.ts"
      ? {
          path: "a.ts",
          binary: false,
          patch: "",
          hunks: [
            { header: "@@ -1,1 +1,2 @@", lines: ["-old", "+new", "+more"], staged: true },
            { header: "@@ -4,1 +4,1 @@", lines: ["-p", "+q"], staged: false },
          ],
        }
      : { path: "b.ts", binary: false, patch: "", hunks: [{ header: "@@ -0,0 +1,2 @@", lines: ["+x", "+y"], staged: false }] }
  );

  render(<FilesPanel />);
  expect(loadDiff).not.toHaveBeenCalled(); // the reads wait for the status to settle

  await settle();

  // One read per path serves both rows of that path.
  expect(loadDiff.mock.calls.map((c) => c[1])).toEqual(["a.ts", "b.ts"]);
  // Staged a.ts counts staged hunks only; Changes a.ts counts the worktree ones.
  expect(screen.getAllByLabelText("2 added, 1 removed")).toHaveLength(1); // Staged a.ts
  expect(screen.getAllByLabelText("1 added, 1 removed")).toHaveLength(1); // Changes a.ts
  expect(screen.getAllByLabelText("2 added, 0 removed")).toHaveLength(1); // Untracked b.ts
  vi.useRealTimers();
});

// ── copy affordances ────────────────────────────────────────────────────────

test("copy path and copy patch write the clipboard and relabel briefly", async () => {
  vi.useFakeTimers();
  seedRepo();
  seedDiff({
    path: "src/a.ts",
    binary: false,
    patch: "diff --git a/src/a.ts\n@@ -1 +1 @@\n-x\n+y",
    hunks: [{ header: "@@ -1,1 +1,1 @@", lines: ["-x", "+y"], staged: false }],
  });

  render(<FileDiff />);

  fireEvent.click(screen.getByRole("button", { name: "Copy path" }));
  expect(writeText).toHaveBeenCalledWith("src/a.ts");

  fireEvent.click(screen.getByRole("button", { name: "Copy patch" }));
  expect(writeText).toHaveBeenLastCalledWith("diff --git a/src/a.ts\n@@ -1 +1 @@\n-x\n+y");

  await act(async () => {
    await Promise.resolve(); // the clipboard write is a promise: let it resolve first
  });
  expect(screen.getAllByRole("button", { name: "Copied" })).toHaveLength(2); // path and patch
  await act(async () => {
    vi.advanceTimersByTime(1300);
  });
  expect(screen.queryByText("Copied")).toBeNull();
  expect(screen.getByRole("button", { name: "Copy path" })).toBeTruthy();
  vi.useRealTimers();
});

test("a clipboard the platform does not offer is a silent no-op", () => {
  setClipboard(undefined);
  seedRepo();
  seedDiff({ path: "a.ts", binary: false, patch: "p", hunks: [{ header: "@@ -1,1 +1,1 @@", lines: ["-x", "+y"], staged: false }] });

  render(<FileDiff />);
  fireEvent.click(screen.getByRole("button", { name: "Copy patch" }));

  expect(writeText).not.toHaveBeenCalled();
  expect(screen.queryByText("Copied")).toBeNull();
});
