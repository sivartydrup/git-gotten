import { beforeEach, expect, test, vi } from "vitest";
import { act, cleanup, fireEvent, render } from "@testing-library/react";
import { screen } from "@testing-library/react";

const { planOp, runOp, loadLog } = vi.hoisted(() => ({
  planOp: vi.fn(),
  runOp: vi.fn(),
  loadLog: vi.fn(),
}));

vi.mock("./api", () => ({
  fetchBuildInfo: vi.fn().mockResolvedValue(null),
  openRepo: vi.fn().mockResolvedValue(null),
  loadStatus: vi.fn().mockResolvedValue(null),
  onStatusChanged: vi.fn().mockResolvedValue(null),
  planOp,
  runOp,
  loadLog,
  onOpStarted: vi.fn().mockResolvedValue(null),
  onOpFinished: vi.fn().mockResolvedValue(null),
}));

import { useActivityStore } from "./store";
import { PreviewDialog } from "./components/PreviewDialog";
import { ActivityLog } from "./components/ActivityLog";
import type { CommandPlan } from "./types";

const commitPlan: CommandPlan = {
  display: 'git commit -m "fix: thing"',
  steps: [
    {
      argv: ["git", "commit", "-m", "fix: thing"],
      summary: "Commit the staged changes.",
      risk: "Mutating",
      effect: null,
      input: null,
    },
  ],
  preview: {
    headline: "Commit 1 staged file",
    lines: ["M src/a.ts"],
    requires_type_to_confirm: false,
  },
};

const commitResult = {
  plan: commitPlan,
  exit: 0,
  ok: true,
  stdout: "[main abc1234] fix: thing",
  stderr: "",
  duration_ms: 12,
};

beforeEach(() => {
  cleanup();
  vi.resetAllMocks();
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

test("requestPreview compiles the op and opens the dialog with the plan", async () => {
  planOp.mockResolvedValueOnce(commitPlan);

  await act(async () => {
    await useActivityStore.getState().requestPreview("/repo/alpha", { kind: "commit", message: "fix: thing", amend: false });
  });

  expect(planOp).toHaveBeenCalledWith("/repo/alpha", { kind: "commit", message: "fix: thing", amend: false });
  const s = useActivityStore.getState();
  expect(s.preview).not.toBeNull();
  expect(s.preview?.plan.display).toBe('git commit -m "fix: thing"');
  expect(s.preview?.plan.steps[0].risk).toBe("Mutating");
  expect(s.busy).toBe(false);
});

test("requestPreview outside Tauri (null) leaves the dialog closed and sets no error", async () => {
  planOp.mockResolvedValueOnce(null);

  await act(async () => {
    await useActivityStore.getState().requestPreview("/repo/alpha", { kind: "commit", message: "m", amend: false });
  });

  const s = useActivityStore.getState();
  expect(s.preview).toBeNull();
  expect(s.error).toBeNull();
});

test("requestPreview surfaces the IPC error and keeps the dialog closed", async () => {
  planOp.mockRejectedValueOnce(new Error("repository is not open: /repo/x"));

  await act(async () => {
    await useActivityStore.getState().requestPreview("/repo/x", { kind: "push", remote: "origin", branch: null, force: false, set_upstream: false });
  });

  const s = useActivityStore.getState();
  expect(s.preview).toBeNull();
  expect(s.error).toBe("repository is not open: /repo/x");
});

test("confirmPreview runs the plan, records the result, and closes the dialog", async () => {
  runOp.mockResolvedValueOnce(commitResult);
  useActivityStore.setState({
    preview: { repo: "/repo/alpha", op: { kind: "commit", message: "fix: thing", amend: false }, plan: commitPlan },
  });

  await act(async () => {
    await useActivityStore.getState().confirmPreview();
  });

  expect(runOp).toHaveBeenCalledWith("/repo/alpha", { kind: "commit", message: "fix: thing", amend: false });
  const s = useActivityStore.getState();
  expect(s.preview).toBeNull();
  expect(s.busy).toBe(false);
  expect(s.activities).toHaveLength(1);
  expect(s.activities[0]).toMatchObject({
    repo: "/repo/alpha",
    display: 'git commit -m "fix: thing"',
    ok: true,
    duration_ms: 12,
  });
});

test("confirmPreview keeps the dialog open on failure and surfaces the error", async () => {
  runOp.mockRejectedValueOnce(new Error("git i/o error"));
  useActivityStore.setState({
    preview: { repo: "/repo/alpha", op: { kind: "commit", message: "m", amend: false }, plan: commitPlan },
  });

  await act(async () => {
    await useActivityStore.getState().confirmPreview();
  });

  const s = useActivityStore.getState();
  expect(s.preview).not.toBeNull(); // still open: retry or cancel
  expect(s.error).toBe("git i/o error");
  expect(s.activities).toHaveLength(0);
});

test("confirmPreview is a no-op while an op is running", async () => {
  useActivityStore.setState({
    running: true,
    preview: { repo: "/repo/alpha", op: { kind: "commit", message: "m", amend: false }, plan: commitPlan },
  });

  await act(async () => {
    await useActivityStore.getState().confirmPreview();
  });

  expect(runOp).not.toHaveBeenCalled();
});

test("op_started / op_finished events drive the running state", async () => {
  const s = useActivityStore.getState();

  await act(async () => {
    s.applyStarted({ repo: "/repo/alpha", display: "git push origin main", risk: "Mutating" });
  });
  expect(useActivityStore.getState()).toMatchObject({ running: true, runningDisplay: "git push origin main" });

  await act(async () => {
    s.applyFinished({ id: 9, repo: "/repo/alpha", display: "git push origin main", ok: true, duration_ms: 87, ts: 1_000, stdout: "", stderr: "" });
  });
  expect(useActivityStore.getState()).toMatchObject({ running: false, runningDisplay: null });
});

test("loadHistory fills the ring newest-first and seeds the id counter", async () => {
  loadLog.mockResolvedValueOnce([
    { id: 41, repo: "/r", display: "git push", ok: true, duration_ms: 10, ts: 1, stdout: "", stderr: "" },
    { id: 42, repo: "/r", display: "git commit -m b", ok: false, duration_ms: 5, ts: 2, stdout: "", stderr: "err" },
    { id: 43, repo: "/other", display: "git pull", ok: true, duration_ms: 7, ts: 3, stdout: "", stderr: "" },
  ]);

  await act(async () => {
    await useActivityStore.getState().loadHistory("/r");
  });

  const s = useActivityStore.getState();
  // Filtered to the requested repo (the `/other` entry is excluded), newest first.
  expect(s.activities.map((e) => e.id)).toEqual([42, 41]);

  // New entries are numbered after the highest id seen in history.
  await act(async () => {
    useActivityStore.getState().record("/r", commitResult);
  });
  expect(useActivityStore.getState().activities[0].id).toBe(44);
});

test("record caps the ring at 500 entries", () => {
  // Build 501 entries (ids 0..500) newest-first: highest id at the head.
  const many = Array.from({ length: 501 }, (_, i) => ({
    id: 500 - i,
    repo: "/r",
    display: `git op ${500 - i}`,
    ok: true,
    duration_ms: 1,
    ts: 1,
    stdout: "",
    stderr: "",
  }));
  useActivityStore.setState({ activities: many });
  useActivityStore.getState().record("/r", commitResult);
  const s = useActivityStore.getState();
  expect(s.activities).toHaveLength(500);
  expect(s.activities[0].display).toBe(commitPlan.display); // newest kept
  // The two oldest (ids 0 and 1) fell off the tail; the new entry pushed the ring.
  expect(s.activities.some((e) => e.id === 0)).toBe(false);
  expect(s.activities.some((e) => e.id === 1)).toBe(false);
  expect(s.activities[499].id).toBe(2); // oldest retained
});

test("PreviewDialog renders the command, risk class, and confirm action", async () => {
  useActivityStore.setState({
    preview: { repo: "/repo/alpha", op: { kind: "commit", message: "fix: thing", amend: false }, plan: commitPlan },
  });

  render(<PreviewDialog />);

  const dialog = screen.getByRole("dialog", { name: "Confirm command" });
  expect(dialog.textContent).toContain('git commit -m "fix: thing"');
  expect(dialog.textContent).toContain("mutates the repo");
  expect((screen.getByRole("button", { name: /Run git commit/ }) as HTMLButtonElement).disabled).toBe(false);
  expect(screen.queryByRole("button", { name: "Cancel" })).not.toBeNull();

  runOp.mockResolvedValueOnce(commitResult);
  await act(async () => {
    await screen.getByRole("button", { name: /Run git commit/ }).click();
  });
  expect(useActivityStore.getState().preview).toBeNull();
});

test("PreviewDialog is absent when there is no pending plan", () => {
  render(<PreviewDialog />);
  expect(screen.queryByRole("dialog")).toBeNull();
});

test("Dangerous (force-push) preview requires type-to-confirm before running", async () => {
  const forcePlan: CommandPlan = {
    display: "git push --force origin main",
    steps: [
      {
        argv: ["git", "push", "--force", "origin", "main"],
        summary: "Overwrite origin/main, discarding its history.",
        risk: "Dangerous",
        effect: "force-push main — remote loses 2 commits",
        input: null,
      },
    ],
    preview: {
      headline: "Force-push main to origin",
      lines: ["origin loses 2 commits"],
      requires_type_to_confirm: true,
    },
  };
  useActivityStore.setState({
    preview: { repo: "/repo/alpha", op: { kind: "push", remote: "origin", branch: "main", force: true, set_upstream: false }, plan: forcePlan },
  });

  render(<PreviewDialog />);

  const dialog = screen.getByRole("dialog", { name: "Confirm command" });
  expect(dialog.textContent).toContain("dangerous");
  expect(dialog.textContent).toContain("loses 2 commits"); // blast radius
  const runButton = screen.getByRole("button", { name: /Run git push --force/ }) as HTMLButtonElement;
  expect(runButton.disabled).toBe(true); // blocked until the word is typed

  fireEvent.change(screen.getByPlaceholderText("confirm"), { target: { value: "confirm" } });
  expect((screen.getByRole("button", { name: /Run git push --force/ }) as HTMLButtonElement).disabled).toBe(false);

  runOp.mockResolvedValueOnce({
    plan: forcePlan,
    exit: 0,
    ok: true,
    stdout: "To origin\n + abc..def main",
    stderr: "",
    duration_ms: 20,
  });
  await act(async () => {
    await screen.getByRole("button", { name: /Run git push --force/ }).click();
  });
  expect(runOp).toHaveBeenCalledWith(
    "/repo/alpha",
    { kind: "push", remote: "origin", branch: "main", force: true, set_upstream: false },
  );
});

test("ActivityLog lists entries newest-first with outcome marks and shows the running state", async () => {
  useActivityStore.setState({
    activities: [
      { id: 2, repo: "/r", display: "git commit -m b", ok: true, duration_ms: 30, ts: 2_000, stdout: "out", stderr: "" },
      { id: 1, repo: "/r", display: "git push --force", ok: false, duration_ms: 90, ts: 1_000, stdout: "", stderr: "err" },
    ],
    running: true,
    runningDisplay: "git rebase origin/main",
    runningRisk: "Dangerous",
  });

  render(<ActivityLog />);

  const rows = screen.getAllByRole("listitem");
  expect(rows).toHaveLength(2);
  expect(rows[0].textContent).toContain("git commit -m b"); // newest first
  expect(rows[0].textContent).toContain("30 ms");
  expect(rows[1].textContent).toContain("git push --force");
  expect(document.body.textContent).toContain("git rebase origin/main"); // live hint

  // The failed row is flagged for its evidence.
  expect(rows[1].className).toContain("fail");
});
