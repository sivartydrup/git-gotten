import { afterEach, expect, test, vi } from "vitest";
import { cleanup, render, screen } from "@testing-library/react";
import { InspectDrawer } from "./InspectDrawer";
import { useGraphStore, useRepoStore } from "../store";
import type { CommitDetail } from "../types";

// Mock the IPC layer (the store imports it); the tests drive the store directly.
vi.mock("../api", () => ({
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
}));

afterEach(() => {
  cleanup();
  useGraphStore.setState({
    graph: null,
    loading: false,
    error: null,
    selected: null,
    hovered: null,
    query: "",
    detail: null,
    detailLoading: false,
    detailError: null,
  });
  useRepoStore.setState({
    repos: [],
    activeId: null,
    status: {},
    opening: false,
    error: null,
  });
});

const detail: CommitDetail = {
  hash: "a1b2c3d4e5f6",
  subject: "tip",
  message: "tip\n\nbody of the message",
  author_name: "Al",
  author_email: "al@example.com",
  author_date: 1700000000,
  committer_name: "Al",
  committer_email: "al@example.com",
  committer_date: 1700000000,
  parents: ["d4e5f6a7b8c9"],
  refs: [{ name: "main", kind: "Branch", color: 0 }],
  is_head: true,
  stat: [{ path: "a.txt", additions: 1, deletions: 2, binary: false }],
  diff: [{ path: "a.txt", binary: false, patch: "+one\n-two" }],
};

test("empty state: no selection shows the prompt", () => {
  render(<InspectDrawer />);
  expect(screen.getByText("Select a commit to inspect it")).toBeTruthy();
});

test("a loaded detail shows subject, refs, dates, stat, and diff", () => {
  useRepoStore.setState({ activeId: "/repo/alpha" });
  useGraphStore.setState({ selected: "a1b2c3", detail });
  render(<InspectDrawer />);
  expect(screen.getByText("tip")).toBeTruthy();
  expect(screen.getByText("main")).toBeTruthy();
  expect(screen.getByText(/al@example\.com/)).toBeTruthy();
  expect(screen.getAllByText("a.txt")).toHaveLength(2);
  // The count is `+{n}`: the literal "+" and the number are separate text
  // nodes, so match with a tolerant regex.
  expect(screen.getByText(/\+\s*1/)).toBeTruthy();
  // The node's text is whitespace-collapsed; the matcher string is not, so use a space.
  expect(screen.getByText("+one -two")).toBeTruthy();
  expect(screen.queryByText("No diff for merge commits")).toBeNull();
});

test("a commit with no stat or diff (a merge) shows the merge note", () => {
  useRepoStore.setState({ activeId: "/repo/alpha" });
  useGraphStore.setState({
    selected: "m0m1m2",
    detail: { ...detail, subject: "merge", stat: [], diff: [] },
  });
  render(<InspectDrawer />);
  expect(screen.getByText("No diff for merge commits")).toBeTruthy();
});

test("a detail load error is surfaced, not swallowed", () => {
  useRepoStore.setState({ activeId: "/repo/alpha" });
  useGraphStore.setState({ selected: "a1b2c3", detail: null, detailError: "nope" });
  render(<InspectDrawer />);
  expect(screen.getByText("Failed to load the commit")).toBeTruthy();
  expect(screen.getByText("nope")).toBeTruthy();
});
