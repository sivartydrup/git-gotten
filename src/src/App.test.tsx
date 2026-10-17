import { afterEach, expect, test } from "vitest";
import { act, cleanup, render } from "@testing-library/react";
import App from "./App";

afterEach(cleanup);

test("renders the GitGotten shell (non-Tauri fallback)", async () => {
  render(<App />);
  // `render` runs the effect that calls the async `load()`; outside Tauri that
  // resolves `null`. Flush the microtask so the fallback branch is committed.
  await act(async () => {});
  expect(document.body.textContent).toContain("GitGotten");
  expect(document.body.textContent).toContain("Running outside Tauri");
});
