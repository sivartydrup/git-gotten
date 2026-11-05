import { beforeEach, expect, test, vi } from "vitest";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";

import { useActivityStore } from "../store";
import { ActivityLog } from "./ActivityLog";
import { riskOf } from "./activityRisk";
import type { LogEntry } from "../types";

const writeText = vi.fn().mockResolvedValue(undefined);

function entry(over: Partial<LogEntry>): LogEntry {
  return {
    id: 1,
    repo: "/r",
    display: "git status",
    ok: true,
    duration_ms: 10,
    ts: 1_000,
    stdout: "",
    stderr: "",
    conflicts: [],
    ...over,
  };
}

beforeEach(() => {
  cleanup();
  writeText.mockReset();
  Object.defineProperty(navigator, "clipboard", { configurable: true, writable: true, value: { writeText } });
  useActivityStore.setState({ activities: [], running: false, runningDisplay: null, runningRisk: null });
});

test("the risk of a logged command matches how the core classifies it", () => {
  expect(riskOf("git status")).toBe("ReadOnly");
  expect(riskOf("git diff src/a.ts")).toBe("ReadOnly");
  expect(riskOf("git log --oneline -20")).toBe("ReadOnly");
  expect(riskOf("git commit -m \"fix: thing\"")).toBe("Mutating");
  expect(riskOf("git push origin main")).toBe("Mutating");
  expect(riskOf("git push --force origin main")).toBe("Dangerous");
  expect(riskOf("git reset --soft HEAD~1")).toBe("Mutating");
  expect(riskOf("git reset --hard HEAD~1")).toBe("Dangerous");
  expect(riskOf("git rebase --abort")).toBe("Dangerous");
  expect(riskOf("git checkout -b feature")).toBe("Mutating");
  expect(riskOf("open the file")).toBeNull();
});

test("filter chips narrow the rendered rows locally", () => {
  useActivityStore.setState({
    activities: [
      entry({ id: 4, display: "git push --force origin main", ok: false, stderr: "rejected" }),
      entry({ id: 3, display: "git commit -m fix", stdout: "1 file changed" }),
      entry({ id: 2, display: "git status" }),
      entry({ id: 1, display: "git reset --hard HEAD~1", ok: false, stderr: "bad ref" }),
    ],
  });

  const commands = () =>
    Array.from(document.querySelectorAll(".log-cmd")).map((el) => el.textContent);

  render(<ActivityLog />);
  expect(commands()).toHaveLength(4);

  fireEvent.click(screen.getByRole("button", { name: "Failed" }));
  expect(commands()).toEqual(["git push --force origin main", "git reset --hard HEAD~1"]);

  fireEvent.click(screen.getByRole("button", { name: "Mutating" }));
  expect(commands()).toEqual(["git commit -m fix"]);

  fireEvent.click(screen.getByRole("button", { name: "Dangerous" }));
  expect(commands()).toEqual(["git push --force origin main", "git reset --hard HEAD~1"]);

  fireEvent.click(screen.getByRole("button", { name: "All" }));
  expect(commands()).toHaveLength(4);
});

test("the active chip is pressed and an empty filter says so", () => {
  useActivityStore.setState({ activities: [entry({ id: 1, display: "git status" })] });
  render(<ActivityLog />);

  expect(screen.getByRole("button", { name: "All" }).getAttribute("aria-pressed")).toBe("true");

  fireEvent.click(screen.getByRole("button", { name: "Dangerous" }));
  expect(screen.getByRole("button", { name: "Dangerous" }).getAttribute("aria-pressed")).toBe("true");
  expect(screen.getByText("No dangerous commands.")).toBeTruthy();
  // Filtering is a view: the log itself still holds the entry.
  expect(useActivityStore.getState().activities).toHaveLength(1);
});

test("a filtered row keeps its expandable output, and its risk is signalled", () => {
  useActivityStore.setState({
    activities: [entry({ id: 9, display: "git push --force origin main", ok: false, stdout: "forced", stderr: "rejected" })],
  });

  render(<ActivityLog />);
  const row = screen.getByRole("listitem");
  expect(row.className).toContain("risk-dangerous");
  expect(row.className).toContain("fail");

  fireEvent.click(screen.getByRole("button", { name: "Failed" }));
  const filtered = screen.getByRole("listitem");
  expect(filtered.querySelector(".log-cmd")?.textContent).toBe("git push --force origin main");

  fireEvent.click(filtered.querySelector(".log-cmd")!);
  expect(filtered.querySelector(".log-stderr")?.textContent).toContain("rejected");
  expect(filtered.querySelector(".log-output")?.textContent).toContain("forced");
});

test("a read verb is not dressed up as a mutation", () => {
  useActivityStore.setState({ activities: [entry({ id: 1, display: "git log --oneline -20" }), entry({ id: 2, display: "git diff src/a.ts" })] });
  render(<ActivityLog />);

  fireEvent.click(screen.getByRole("button", { name: "Mutating" }));
  expect(screen.getByText("No mutating commands.")).toBeTruthy();
});

test("the copy button writes the exact command line and relabels for a moment", async () => {
  vi.useFakeTimers();
  useActivityStore.setState({ activities: [entry({ id: 5, display: 'git commit -m "fix: thing"' })] });

  render(<ActivityLog />);
  fireEvent.click(screen.getByRole("button", { name: "Copy" }));
  expect(writeText).toHaveBeenCalledWith('git commit -m "fix: thing"');

  await act(async () => {
    await Promise.resolve(); // the clipboard write is a promise: let it resolve first
  });
  expect(screen.getByRole("button", { name: "Copied" })).toBeTruthy();

  await act(async () => {
    vi.advanceTimersByTime(1300);
  });
  expect(screen.getByRole("button", { name: "Copy" })).toBeTruthy();
  vi.useRealTimers();
});

test("no clipboard API means the copy button does nothing", () => {
  Object.defineProperty(navigator, "clipboard", { configurable: true, writable: true, value: undefined });
  useActivityStore.setState({ activities: [entry({ id: 5, display: "git status" })] });

  render(<ActivityLog />);
  fireEvent.click(screen.getByRole("button", { name: "Copy" }));

  expect(writeText).not.toHaveBeenCalled();
  expect(screen.queryByRole("button", { name: "Copied" })).toBeNull();
});
