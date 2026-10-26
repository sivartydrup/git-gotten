import { beforeEach, expect, test, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import App from "./App";

// `./api` is mocked wholesale; per-test behavior is configured on the mocks.
// `onStatusChanged` captures its listener so tests can fire `status_changed`
// events (the store's unmerged list follows the live status).
const {
  planOp,
  runOp,
  loadStatus,
  loadConflict,
  onStatusChanged,
  statusCb,
  resetStatusCb,
} = vi.hoisted(() => {
  let cb: ((p: unknown) => void) | null = null;
  return {
    planOp: vi.fn().mockResolvedValue(null),
    runOp: vi.fn().mockResolvedValue(null),
    loadStatus: vi.fn().mockResolvedValue(null),
    loadConflict: vi.fn().mockResolvedValue(null),
    onStatusChanged: vi.fn((c: (p: unknown) => void) => {
      cb = c;
      return Promise.resolve(null);
    }),
    statusCb: () => cb,
    resetStatusCb: () => {
      cb = null;
    },
  };
});

vi.mock("./api", () => ({
  fetchBuildInfo: vi.fn().mockResolvedValue(null),
  openRepo: vi.fn().mockResolvedValue(null),
  loadStatus,
  onStatusChanged,
  planOp,
  runOp,
  loadLog: vi.fn().mockResolvedValue(null),
  onOpStarted: vi.fn().mockResolvedValue(null),
  onOpFinished: vi.fn().mockResolvedValue(null),
  loadGraph: vi.fn().mockResolvedValue(null),
  loadCommit: vi.fn().mockResolvedValue(null),
  loadDiff: vi.fn().mockResolvedValue(null),
  loadBranches: vi.fn().mockResolvedValue([]),
  loadRemotes: vi.fn().mockResolvedValue([]),
  loadTags: vi.fn().mockResolvedValue([]),
  loadConflict,
}));

import {
  useActivityStore,
  useConflictStore,
  useGraphStore,
  useMainTab,
  useRefsStore,
  useRepoStore,
  useStagingStore,
} from "./store";
import type {
  CommandPlan,
  ConflictFile,
  ConflictSegment,
  RepoMeta,
  Status,
} from "./types";

const REPO = "/repo/alpha";

const meta: RepoMeta = {
  id: REPO,
  name: "alpha",
  path: REPO,
  branch: "main",
  ahead: 0,
  behind: 0,
  dirty_count: 1,
};

/** A merge stopped on one conflicted text file. */
function conflictedStatus(unmerged: string[]): Status {
  return {
    branch: "main",
    detached_head: null,
    head: "a".repeat(40),
    ahead: 0,
    behind: 0,
    upstream: "origin/main",
    changes: unmerged.map((path) => ({
      path,
      kind: "Conflicted" as const,
      staged: true,
      binary: false,
    })),
    in_merge: true,
    in_rebase: false,
    in_cherry_pick: false,
  };
}

/** A merged repo with a clean tree (no operation in progress). */
function cleanStatus(): Status {
  return {
    branch: "main",
    detached_head: null,
    head: "a".repeat(40),
    ahead: 0,
    behind: 0,
    upstream: "origin/main",
    changes: [],
    in_merge: false,
    in_rebase: false,
    in_cherry_pick: false,
  };
}

/** One conflict region between the three file versions. */
const segments: ConflictSegment[] = [
  { Conflict: { base: "old line\n", ours: "our line\n", theirs: "their line\n" } },
  { Common: "shared tail\n" },
];

/** The 3-way view of `f.txt` (from `load_conflict`). */
function conflictFile(path: string): ConflictFile {
  return {
    path,
    binary: false,
    base: "old line\nshared tail\n",
    ours: "our line\nshared tail\n",
    theirs: "their line\nshared tail\n",
    segments,
  };
}

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

function seed(status: Status): void {
  useRepoStore.setState({
    repos: [meta],
    activeId: REPO,
    status: { [REPO]: status },
    opening: false,
    error: null,
  });
  useConflictStore.getState().applyStatus(status);
  useActivityStore.setState({
    activities: [],
    running: false,
    runningDisplay: null,
    runningRisk: null,
    preview: null,
    error: null,
    busy: false,
  });
  useMainTab.setState({ tab: "graph" });
  useStagingStore.getState().reset();
  useRefsStore.setState({ branches: [], remotes: [], tags: [], loading: false, error: null });
  useGraphStore.setState({
    query: "",
    selected: null,
    hovered: null,
    detail: null,
    detailLoading: false,
    detailError: null,
  });
  planOp.mockReset().mockResolvedValue(plan("git merge --continue"));
  runOp.mockReset().mockResolvedValue(null);
  loadStatus.mockReset().mockResolvedValue(status);
  loadConflict.mockReset().mockResolvedValue(null);
  onStatusChanged.mockClear();
  resetStatusCb();
}

beforeEach(() => {
  cleanup();
  seed(cleanStatus());
});

/** Flush the microtask queue deterministically (no wall-clock timers). */
async function flush(): Promise<void> {
  await act(async () => {});
}

/** Select the Conflicts tab and the shown file, with the 3-way view loaded. */
async function openConflictedFile(): Promise<void> {
  fireEvent.click(screen.getByRole("tab", { name: /Conflicts/ }));
  fireEvent.click(screen.getByText("f.txt"));
  await flush();
}

// ── the recoverable-state banner ────────────────────────────────────────────

test("the banner shows merge progress while files are unmerged", async () => {
  seed(conflictedStatus(["f.txt", "g.txt"]));
  render(<App />);
  await flush();
  const banner = screen.getByRole("status");
  expect(banner.textContent).toContain("merge in progress");
  expect(banner.textContent).toContain("0 of 2 files resolved");
  // Continue is unavailable until every file is resolved.
  expect((screen.getByRole("button", { name: "Continue" }) as HTMLButtonElement).disabled).toBe(
    true
  );
});

test("the banner is hidden on a clean repo", async () => {
  seed(cleanStatus());
  render(<App />);
  await flush();
  expect(screen.queryByRole("status")).toBeNull();
});

test("the banner offers continue and abort once every file is resolved", async () => {
  seed(conflictedStatus(["f.txt"]));
  // The file gets resolved (in-app or externally): the live status drops the
  // unmerged entry while the merge still stands.
  useRepoStore.setState({
    status: { [REPO]: { ...conflictedStatus(["f.txt"]), changes: [] } },
  });
  render(<App />);
  await flush();
  expect(screen.getByRole("status").textContent).toContain("ready to continue");
  const continueBtn = screen.getByRole("button", { name: "Continue" }) as HTMLButtonElement;
  expect(continueBtn.disabled).toBe(false);
  fireEvent.click(continueBtn);
  await flush();
  expect(planOp).toHaveBeenCalledWith(REPO, { kind: "continue_op" });
  useActivityStore.getState().dismissPreview();
  planOp.mockClear();
  fireEvent.click(screen.getByRole("button", { name: "Abort" }));
  await flush();
  expect(planOp).toHaveBeenCalledWith(REPO, { kind: "abort_op" });
});

// ── the 3-way view ───────────────────────────────────────────────────────────

test("selecting a conflicted file renders its base/ours/theirs", async () => {
  seed(conflictedStatus(["f.txt"]));
  loadConflict.mockResolvedValue(conflictFile("f.txt"));
  render(<App />);
  await flush();
  await openConflictedFile();
  expect(loadConflict).toHaveBeenCalledWith(REPO, "f.txt");
  // All three versions of the conflict region are visible.
  expect(screen.getByText(/old line/)).toBeTruthy();
  expect(screen.getByText(/our line/)).toBeTruthy();
  expect(screen.getByText(/their line/)).toBeTruthy();
  // The unchanged span renders once.
  expect(screen.getByText(/shared tail/)).toBeTruthy();
});

test("a per-region choice feeds the resolve_conflict op", async () => {
  seed(conflictedStatus(["f.txt"]));
  loadConflict.mockResolvedValue(conflictFile("f.txt"));
  render(<App />);
  await flush();
  await openConflictedFile();

  // The region starts "ours"; switch it to "theirs".
  fireEvent.click(screen.getByRole("button", { name: "Their" }));
  await flush();
  expect(useConflictStore.getState().choices).toEqual(["AcceptTheirs"]);

  fireEvent.click(screen.getByRole("button", { name: "Resolve file" }));
  await flush();
  expect(planOp).toHaveBeenCalledWith(REPO, {
    kind: "resolve_conflict",
    path: "f.txt",
    resolution: { Hunks: ["AcceptTheirs"] },
  });

  useActivityStore
    .getState()
    .confirmPreview();
  await flush();
  expect(runOp).toHaveBeenCalledWith(REPO, {
    kind: "resolve_conflict",
    path: "f.txt",
    resolution: { Hunks: ["AcceptTheirs"] },
  });
});

test("take their version resolves the whole file from one side", async () => {
  seed(conflictedStatus(["f.txt"]));
  loadConflict.mockResolvedValue(conflictFile("f.txt"));
  render(<App />);
  await flush();
  await openConflictedFile();

  fireEvent.click(screen.getByRole("button", { name: "Take their version" }));
  await flush();
  expect(planOp).toHaveBeenCalledWith(REPO, {
    kind: "resolve_conflict",
    path: "f.txt",
    resolution: { Side: "Theirs" },
  });
});

test("a binary conflict offers only the whole-side resolutions", async () => {
  seed(conflictedStatus(["f.bin"]));
  loadConflict.mockResolvedValue({
    path: "f.bin",
    binary: true,
    base: "",
    ours: "",
    theirs: "",
    segments: [],
  });
  render(<App />);
  await flush();
  fireEvent.click(screen.getByRole("tab", { name: /Conflicts/ }));
  fireEvent.click(screen.getByText("f.bin"));
  await flush();
  expect(screen.getByText(/Binary file/)).toBeTruthy();
  expect(screen.getByRole("button", { name: "Take our version" })).toBeTruthy();
  expect(screen.queryByRole("button", { name: /Our$/ })).toBeNull();
});

// ── the live status follows resolutions ─────────────────────────────────────

test("a status update without the shown file clears the selection", async () => {
  seed(conflictedStatus(["f.txt", "g.txt"]));
  loadConflict.mockResolvedValue(conflictFile("f.txt"));
  render(<App />);
  await flush();
  fireEvent.click(screen.getByRole("tab", { name: /Conflicts/ }));
  fireEvent.click(screen.getByText("f.txt"));
  await flush();
  expect(useConflictStore.getState().file).toBe("f.txt");

  // f.txt gets resolved (in-app or externally): it drops out of the status.
  const cb = statusCb();
  expect(cb).toBeTypeOf("function");
  await act(async () => {
    cb!({ repo: REPO, status: conflictedStatus(["g.txt"]) });
  });
  await flush();
  expect(useConflictStore.getState().file).toBeNull();
  expect(useConflictStore.getState().files).toEqual(["g.txt"]);
});
