import { beforeEach, expect, test, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { UpdatePrompt } from "./components/UpdatePrompt";
import { useUpdaterStore } from "./store";

const { checkForUpdate, installUpdate } = vi.hoisted(() => ({
  checkForUpdate: vi.fn(),
  installUpdate: vi.fn(),
}));

// Mock the IPC layer; per-test behavior is configured on the hoisted mocks.
vi.mock("./api", () => ({
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
  checkForUpdate,
  installUpdate,
}));

beforeEach(() => {
  cleanup();
  checkForUpdate.mockReset();
  installUpdate.mockReset();
  useUpdaterStore.setState({
    available: false,
    version: null,
    notes: null,
    dismissed: false,
    installing: false,
    error: null,
  });
});

test("check() surfaces the version and notes the feed announces", async () => {
  checkForUpdate.mockResolvedValue({ version: "0.9.0", notes: "New graph filters." });
  await useUpdaterStore.getState().check();
  const s = useUpdaterStore.getState();
  expect(s.available).toBe(true);
  expect(s.version).toBe("0.9.0");
  expect(s.notes).toBe("New graph filters.");
  expect(s.error).toBeNull();
});

test("check() with an up-to-date feed stays quiet", async () => {
  checkForUpdate.mockResolvedValue(null);
  await useUpdaterStore.getState().check();
  expect(useUpdaterStore.getState().available).toBe(false);
});

test("check() on an unreachable feed records the error and never prompts", async () => {
  checkForUpdate.mockRejectedValue(new Error("no endpoint returned a release manifest"));
  await useUpdaterStore.getState().check();
  const s = useUpdaterStore.getState();
  expect(s.available).toBe(false);
  expect(s.error).toContain("no endpoint returned a release manifest");
});

test("dismiss hides the prompt for the rest of the session", async () => {
  checkForUpdate.mockResolvedValue({ version: "0.9.0", notes: "Notes." });
  await useUpdaterStore.getState().check();
  useUpdaterStore.getState().dismiss();
  expect(useUpdaterStore.getState().available).toBe(false);
  expect(useUpdaterStore.getState().dismissed).toBe(true);
});

test("install runs the download-and-install flow and reports it", async () => {
  checkForUpdate.mockResolvedValue({ version: "0.9.0", notes: "Notes." });
  installUpdate.mockResolvedValue(undefined);
  await useUpdaterStore.getState().check();
  await useUpdaterStore.getState().install();
  expect(installUpdate).toHaveBeenCalledTimes(1);
  const s = useUpdaterStore.getState();
  expect(s.installing).toBe(true);
  expect(s.available).toBe(true);
});

test("a failed install keeps the prompt and shows the reason", async () => {
  checkForUpdate.mockResolvedValue({ version: "0.9.0", notes: "Notes." });
  installUpdate.mockRejectedValue(new Error("signature verification failed"));
  await useUpdaterStore.getState().check();
  await useUpdaterStore.getState().install();
  const s = useUpdaterStore.getState();
  expect(s.installing).toBe(false);
  expect(s.available).toBe(true);
  expect(s.error).toContain("signature verification failed");
});

test("UpdatePrompt shows the announced version and dismisses", () => {
  useUpdaterStore.setState({ available: true, version: "0.9.0", notes: "Notes.", dismissed: false });
  render(<UpdatePrompt />);
  expect(screen.getByText(/update available/i)).toBeTruthy();
  expect(screen.getByText(/0\.9\.0/)).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: /dismiss/i }));
  expect(useUpdaterStore.getState().available).toBe(false);
});

test("UpdatePrompt's install button triggers the store flow", () => {
  useUpdaterStore.setState({ available: true, version: "0.9.0", notes: "Notes.", dismissed: false });
  render(<UpdatePrompt />);
  fireEvent.click(screen.getByRole("button", { name: /install/i }));
  expect(installUpdate).toHaveBeenCalledTimes(1);
});

test("UpdatePrompt renders nothing when there is no update", () => {
  const { container } = render(<UpdatePrompt />);
  expect(container.innerHTML).toBe("");
});
