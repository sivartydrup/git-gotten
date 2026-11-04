import { afterEach, expect, test, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";

vi.mock("./api", () => ({
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
  loadGraph: async () => null,
  loadCommit: async () => null,
  loadDiff: async () => null,
  loadConflict: async () => null,
  loadBranches: async () => null,
  loadRemotes: async () => null,
  loadTags: async () => null,
  loadWorktrees: async () => null,
  onStatusChanged: async () => null,
  onOpStarted: async () => null,
  onOpFinished: async () => null,
  planOp: async () => null,
  runOp: async () => null,
  loadLog: async () => null,
  loadStashes: async () => [],
  loadSubmodules: async () => [],
  loadBisect: async () => null,
  bisectTest: async () => null,
}));

import { friendlyGitError } from "./errors";
import { ActivityLog } from "./components/ActivityLog";
import { PreviewDialog } from "./components/PreviewDialog";
import { useActivityStore } from "./store";
import type { CommandPlan, LogEntry } from "./types";

afterEach(cleanup);

/** The git diagnostics this app can actually produce, quoted as git prints them. */
const GIT_FAILURES: [string, string][] = [
  [
    "fatal: Unable to create '/repo/.git/index.lock': File exists.",
    "Another git command is working in this repository — try again once it finishes.",
  ],
  [
    "error: Your local changes to the following files would be overwritten by checkout:\n\tsrc/a.ts\nPlease commit your changes or stash them before you switch branches.",
    "You have uncommitted changes in the way — commit, stash, or discard them first.",
  ],
  [
    "Auto-merging f.txt\nCONFLICT (content): Merge conflict in f.txt\nAutomatic merge failed; fix conflicts and then commit the result.",
    "The merge stopped on conflicts — resolve them in the Conflicts tab.",
  ],
  [
    "error: cannot rebase: You have unstaged changes.\nerror: Please commit or stash them.",
    "You have uncommitted changes in the way — commit, stash, or discard them first.",
  ],
  [
    "To github.com:sivartydrup/git-gotten.git\n ! [rejected]        main -> main (non-fast-forward)\nerror: failed to push some refs to 'github.com:sivartydrup/git-gotten.git'",
    "The remote has commits you do not have locally — pull before pushing.",
  ],
  [
    "fatal: The current branch feature has no upstream branch.\nTo push the current branch and set the remote as upstream, use\n\n    git push --set-upstream origin feature",
    "This branch has no upstream to push to yet.",
  ],
  [
    "remote: Support for password authentication was removed.\nfatal: Authentication failed for 'https://github.com/sivartydrup/git-gotten.git/'",
    "The remote refused the credentials — check the remote URL and your access.",
  ],
  [
    "fatal: unable to access 'https://github.com/sivartydrup/git-gotten.git/': Failed to connect to github.com port 443 after 75003 ms: Couldn't connect to server",
    "The remote could not be reached — check the URL and your network.",
  ],
  [
    "fatal: 'feature' is already checked out at '/tmp/gitgotten-worktree'",
    "That branch is already checked out in another worktree.",
  ],
  [
    "On branch main\nnothing to commit, working tree clean",
    "Nothing is staged, so there is nothing to commit.",
  ],
  [
    "fatal: bad revision 'nope-not-here'",
    "That branch, tag, or commit does not exist.",
  ],
  [
    "fatal: There is no merge in progress (MERGE_HEAD missing).",
    "There is nothing to continue or abort — that operation already finished.",
  ],
  [
    "fatal: cannot lock ref 'refs/heads/main': is at 9f2c1ab but expected 1d4e7aa",
    "Another git command is working in this repository — try again once it finishes.",
  ],
  [
    "fatal: unable to auto-detect email address (got 'lomelo@machine.(none)')",
    "git has no user name and email configured for this repository.",
  ],
];

test("every mapped git failure gets its plain-language line", () => {
  for (const [raw, friendly] of GIT_FAILURES) {
    expect(friendlyGitError(raw), raw).toBe(friendly);
  }
});

test("unrecognized output maps to nothing, so the raw text stands alone", () => {
  expect(friendlyGitError("")).toBeNull();
  expect(friendlyGitError("git i/o error")).toBeNull();
});

test("a failed log row summarizes the failure and still keeps the exact output", () => {
  const entry: LogEntry = {
    id: 7,
    repo: "/repo/alpha",
    display: "git push origin main",
    ok: false,
    duration_ms: 412,
    ts: 1_000,
    stdout: "",
    stderr: GIT_FAILURES[4][0],
    conflicts: [],
  };
  useActivityStore.setState({ activities: [entry], running: false });

  render(<ActivityLog />);
  const row = screen.getByRole("listitem");

  expect(row.textContent).toContain("The remote has commits you do not have locally");
  // The transparency contract: the command line itself is untouched.
  expect(row.querySelector(".log-cmd")?.textContent).toBe("git push origin main");

  // …and the raw output is still one click away.
  fireEvent.click(row.querySelector(".log-cmd")!);
  expect(row.querySelector(".log-stderr")?.textContent).toContain("non-fast-forward");

  cleanup();
});

test("a failed row with unmappable output invents no hint", () => {
  useActivityStore.setState({
    activities: [
      {
        id: 8,
        repo: "/repo/alpha",
        display: "git status",
        ok: false,
        duration_ms: 4,
        ts: 2_000,
        stdout: "",
        stderr: "git i/o error",
        conflicts: [],
      },
    ],
    running: false,
  });

  render(<ActivityLog />);

  expect(document.querySelector(".log-hint")).toBeNull();
  expect(screen.getByRole("listitem").textContent).toContain("git status");
  cleanup();
});

test("the preview dialog summarizes an IPC failure above the raw error", async () => {
  const plan: CommandPlan = {
    display: "git push origin main",
    steps: [
      {
        argv: ["git", "push", "origin", "main"],
        summary: "Push main to origin.",
        risk: "Mutating",
        effect: null,
        input: null,
        program: null,
      },
    ],
    preview: null,
  };
  useActivityStore.setState({
    preview: { repo: "/repo/alpha", op: { kind: "push", remote: "origin", branch: "main", force: false, set_upstream: false }, plan },
    error: GIT_FAILURES[4][0],
    activities: [],
    running: false,
    busy: false,
  });

  render(<PreviewDialog />);

  expect(screen.getByText("The remote has commits you do not have locally — pull before pushing."));
  // The raw git output is shown next to the summary, not instead of it.
  expect(document.querySelector(".error-raw")?.textContent).toContain("non-fast-forward");

  cleanup();
});
