import { beforeEach, expect, test, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";

// `./api` is mocked wholesale; per-test behavior is configured on the mocks.
const { planOp, runOp, loadDiff, loadStatus } = vi.hoisted(() => ({
  planOp: vi.fn().mockResolvedValue(null),
  runOp: vi.fn().mockResolvedValue(null),
  loadDiff: vi.fn().mockResolvedValue(null),
  loadStatus: vi.fn().mockResolvedValue(null),
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
  onOpFinished: vi.fn().mockResolvedValue(null),
  loadGraph: vi.fn().mockResolvedValue(null),
  loadCommit: vi.fn().mockResolvedValue(null),
  loadDiff,
}));

import { useRepoStore, useStagingStore, useActivityStore } from "./store";
import { FilesPanel } from "./components/FilesPanel";
import { FileDiff } from "./components/FileDiff";
import { CommitPanel } from "./components/CommitPanel";
import type { FileDiff as FileDiffType, RepoMeta, Status } from "./types";

/** A file diff with one staged and two unstaged hunks. */
const diff: FileDiffType = {
  path: "src/a.ts",
  binary: false,
  patch: "",
  hunks: [
    { header: "@@ -1,2 +1,2 @@", lines: ["-old", "+new"], staged: true },
    { header: "@@ -10,2 +10,2 @@", lines: ["-x", "+y"], staged: false },
    { header: "@@ -20,2 +20,2 @@", lines: ["-p", "+q"], staged: false },
  ],
};

const meta: RepoMeta = {
  id: "/repo/alpha",
  name: "alpha",
  path: "/repo/alpha",
  branch: "main",
  ahead: 0,
  behind: 0,
  dirty_count: 0,
};

/** A status with one of each: staged, unstaged, untracked. */
function statusWithChanges(): Status {
  return {
    branch: "main",
    detached_head: null,
    head: null,
    ahead: 0,
    behind: 0,
    upstream: null,
    changes: [
      { path: "src/a.ts", kind: "Modified", staged: true, binary: false },
      { path: "src/b.ts", kind: "Modified", staged: false, binary: false },
      { path: "new.txt", kind: "Untracked", staged: false, binary: false },
    ],
    in_merge: false,
    in_rebase: false,
    in_cherry_pick: false,
  };
}

function seedRepo(): void {
  useRepoStore.setState({
    repos: [meta],
    activeId: meta.id,
    status: { [meta.id]: statusWithChanges() },
  });
}

beforeEach(() => {
  cleanup();
  vi.clearAllMocks();
  planOp.mockResolvedValue(null);
  runOp.mockResolvedValue(null);
  loadDiff.mockResolvedValue(null);
  useRepoStore.setState({ repos: [], activeId: null, status: {} });
  useStagingStore.setState({ file: null, diff: null, loading: false, error: null });
  useActivityStore.setState({
    activities: [],
    running: false,
    runningDisplay: null,
    runningRisk: null,
    preview: null,
    error: null,
    busy: false,
  });
});

/** Flush the microtask queue deterministically (no wall-clock timers). */
async function flush(): Promise<void> {
  await act(async () => {});
}

// ── staging store ──────────────────────────────────────────────────────────

test("selectFile loads the diff of the active repo", async () => {
  seedRepo();
  loadDiff.mockResolvedValueOnce(diff);

  await act(async () => {
    useStagingStore.getState().selectFile("src/a.ts");
  });
  await flush();

  expect(loadDiff).toHaveBeenCalledWith("/repo/alpha", "src/a.ts");
  const s = useStagingStore.getState();
  expect(s.file).toBe("src/a.ts");
  expect(s.diff).toBe(diff);
  expect(s.loading).toBe(false);
});

test("selectFile(null) clears the selection without loading", async () => {
  seedRepo();
  useStagingStore.setState({ file: "src/a.ts", diff });

  await act(async () => {
    useStagingStore.getState().selectFile(null);
  });

  const s = useStagingStore.getState();
  expect(s.file).toBeNull();
  expect(s.diff).toBeNull();
  expect(loadDiff).not.toHaveBeenCalled();
});

test("a stale load response (the selection moved on) is dropped", async () => {
  seedRepo();
  let resolve: (d: FileDiffType) => void;
  loadDiff.mockImplementationOnce(
    () =>
      new Promise((r) => {
        resolve = r;
      }),
  );

  await act(async () => {
    useStagingStore.getState().selectFile("src/a.ts");
  });
  await act(async () => {
    useStagingStore.getState().selectFile("src/b.ts");
  });
  expect(loadDiff).toHaveBeenLastCalledWith("/repo/alpha", "src/b.ts");

  await act(async () => {
    resolve!(diff); // the stale first response lands
  });

  // The diff of `src/a.ts` must not land on the selection of `src/b.ts`.
  expect(useStagingStore.getState().diff).toBeNull();
});

test("reset drops the selection (the active repo changed)", () => {
  useStagingStore.setState({ file: "src/a.ts", diff, loading: true, error: "x" });
  useStagingStore.getState().reset();
  const s = useStagingStore.getState();
  expect(s.file).toBeNull();
  expect(s.diff).toBeNull();
  expect(s.loading).toBe(false);
  expect(s.error).toBeNull();
});

test("reload re-reads the selected file's diff", async () => {
  seedRepo();
  loadDiff.mockResolvedValueOnce(diff);
  await act(async () => {
    useStagingStore.getState().selectFile("src/a.ts");
  });
  await flush();
  vi.clearAllMocks();
  loadDiff.mockResolvedValue(diff);

  useStagingStore.getState().reload();
  await flush();

  expect(loadDiff).toHaveBeenCalledWith("/repo/alpha", "src/a.ts");
  expect(useStagingStore.getState().diff).toBe(diff);
});

// ── FilesPanel ─────────────────────────────────────────────────────────────

test("files panel groups staged / changes / untracked and selects on click", async () => {
  seedRepo();
  loadDiff.mockResolvedValueOnce(diff);
  render(<FilesPanel />);

  expect(screen.getByText("3 changed files")).toBeTruthy();
  expect(screen.getByText("Staged")).toBeTruthy();
  expect(screen.getByText("Changes")).toBeTruthy();
  expect(screen.getByText("Untracked")).toBeTruthy();

  // Clicking a file selects it and loads its diff.
  fireEvent.click(screen.getByText("src/b.ts"));
  await flush();
  expect(loadDiff).toHaveBeenCalledWith("/repo/alpha", "src/b.ts");
  expect(useStagingStore.getState().file).toBe("src/b.ts");

  // Clicking the selected file again deselects.
  fireEvent.click(screen.getByText("src/b.ts"));
  expect(useStagingStore.getState().file).toBeNull();
});

test("files panel shows the clean-tree state when there are no changes", () => {
  seedRepo();
  useRepoStore.setState((s) => ({
    status: {
      ...s.status,
      [meta.id]: { ...statusWithChanges(), changes: [] },
    },
  }));
  render(<FilesPanel />);
  expect(screen.getByText(/working tree clean/i)).toBeTruthy();
});

// ── FileDiff ───────────────────────────────────────────────────────────────

test("file diff renders hunks with a checkbox per unstaged hunk only", async () => {
  seedRepo();
  loadDiff.mockResolvedValue(diff);
  await act(async () => {
    useStagingStore.getState().selectFile("src/a.ts");
  });
  await flush();
  render(<FileDiff />);

  expect(screen.getByText("src/a.ts")).toBeTruthy();
  // The staged hunk shows a "staged" badge; the two unstaged hunks get checkboxes.
  expect(screen.getAllByText("staged")).toHaveLength(1);
  const boxes = screen.getAllByRole("checkbox");
  expect(boxes).toHaveLength(2);
});

test("ticking hunks and staging sends a stage_hunks op with their indices", async () => {
  seedRepo();
  loadDiff.mockResolvedValue(diff);
  await act(async () => {
    useStagingStore.getState().selectFile("src/a.ts");
  });
  await flush();

  // The plan for the hunk-stage op (the preview the user confirms).
  planOp.mockResolvedValueOnce({
    display: "git apply --cached",
    steps: [
      {
        argv: ["git", "apply", "--cached"],
        summary: "stage the selected hunks",
        risk: "Mutating",
        effect: null,
        input: null,
      },
    ],
    preview: {
      headline: "Stage 1 of 2 hunks",
      lines: ["src/a.ts", "@@ -10,2 +10,2 @@"],
      requires_type_to_confirm: false,
    },
  });
  render(<FileDiff />);

  const boxes = screen.getAllByRole("checkbox");
  fireEvent.click(boxes[0]); // the first unstaged hunk (global index 1)
  fireEvent.click(screen.getByRole("button", { name: /stage 1 selected/i }));
  await flush();

  expect(planOp).toHaveBeenCalledWith("/repo/alpha", {
    kind: "stage_hunks",
    path: "src/a.ts",
    selected: [1],
  });
  expect(useActivityStore.getState().preview).not.toBeNull();
});

test("discarding a file requests a preview (dangerous, type-to-confirm)", async () => {
  seedRepo();
  loadDiff.mockResolvedValue(diff);
  await act(async () => {
    useStagingStore.getState().selectFile("src/a.ts");
  });
  await flush();
  planOp.mockResolvedValueOnce({
    display: "git restore --source=HEAD --staged --worktree -- src/a.ts",
    steps: [
      {
        argv: ["git", "restore", "--source=HEAD", "--staged", "--worktree", "--", "src/a.ts"],
        summary: "restore src/a.ts from HEAD (index and worktree)",
        risk: "Dangerous",
        effect: "destroys the uncommitted changes in src/a.ts",
        input: null,
      },
    ],
    preview: {
      headline: "Discard src/a.ts",
      lines: ["src/a.ts"],
      requires_type_to_confirm: true,
    },
  });
  render(<FileDiff />);

  fireEvent.click(screen.getByRole("button", { name: "Discard" }));
  await flush();

  expect(planOp).toHaveBeenCalledWith("/repo/alpha", { kind: "discard", path: "src/a.ts" });
  expect(useActivityStore.getState().preview).not.toBeNull();
});

// ── CommitPanel ────────────────────────────────────────────────────────────

test("commit sends message + amend; author override and co-authors ride along", async () => {
  seedRepo();
  // One staged change: a plain commit is enabled.
  render(<CommitPanel />);

  const input = screen.getByPlaceholderText(/commit message/i) as HTMLTextAreaElement;
  fireEvent.change(input, { target: { value: "fix: the thing" } });
  // The 300 ms hint debounce: the commit button's enabled state does not wait for it.
  let commit = screen.getByRole("button", { name: "Commit" }) as HTMLButtonElement;
  expect(commit.disabled).toBe(false);

  // Amend on a clean index is allowed (it rewrites HEAD).
  fireEvent.click(screen.getByRole("checkbox", { name: /amend/i }));
  // Override the author identity.
  fireEvent.click(screen.getByRole("checkbox", { name: /override author/i }));
  fireEvent.change(screen.getByPlaceholderText("Author name"), { target: { value: "Jane" } });
  fireEvent.change(screen.getByPlaceholderText("Author email"), {
    target: { value: "jane@example.com" },
  });
  // Add a co-author.
  const coAuthor = screen.getByPlaceholderText(/co-author/i) as HTMLInputElement;
  fireEvent.change(coAuthor, { target: { value: "Joe <joe@example.com>" } });
  fireEvent.click(screen.getByRole("button", { name: "Add" }));

  planOp.mockResolvedValueOnce({
    display: 'git commit -m "fix: the thing" --amend --author "Jane <jane@example.com>"',
    steps: [
      {
        argv: [
          "git",
          "commit",
          "-m",
          "fix: the thing\n\nCo-authored-by: Joe <joe@example.com>",
          "--amend",
          "--author",
          "Jane <jane@example.com>",
        ],
        summary: "amend the last commit",
        risk: "Mutating",
        effect: "rewrites the last commit",
        input: null,
      },
    ],
    preview: null,
  });

  commit = screen.getByRole("button", { name: "Amend" }) as HTMLButtonElement;
  fireEvent.click(commit);
  await flush();

  expect(planOp).toHaveBeenCalledWith("/repo/alpha", {
    kind: "commit",
    message: "fix: the thing",
    amend: true,
    author: { name: "Jane", email: "jane@example.com" },
    co_authors: ["Joe <joe@example.com>"],
  });
});

test("a conventional-commit type chip rewrites the message prefix", () => {
  seedRepo();
  render(<CommitPanel />);
  const input = screen.getByPlaceholderText(/commit message/i) as HTMLTextAreaElement;
  fireEvent.change(input, { target: { value: "fix: first" } });
  fireEvent.click(screen.getByRole("button", { name: "refactor:" }));
  expect((screen.getByPlaceholderText(/commit message/i) as HTMLTextAreaElement).value).toBe(
    "refactor: first",
  );
});
