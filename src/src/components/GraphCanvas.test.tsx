import { afterEach, expect, test, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import {
  GraphCanvas,
  hitTest,
  renderGraph,
  visibleRowRange,
  type CanvasCtx,
  type Viewport,
} from "./GraphCanvas";
import { InspectDrawer } from "./InspectDrawer";
import { useBisectStore, useGraphStore, useMainTab, useRepoStore } from "../store";
import type { BisectState, CommitDetail, Graph, Node } from "../types";

// `loadCommit` is hoisted so interaction tests can drive its result.
const { loadCommit, planOp } = vi.hoisted(() => ({
  loadCommit: vi.fn().mockResolvedValue(null),
  planOp: vi.fn().mockResolvedValue(null),
}));

// jsdom has no `PointerEvent` constructor: testing-library's createEvent
// would fall back to a plain Event and silently drop clientX/clientY.
// Polyfill it over MouseEvent so pointer interactions carry their coordinates.
if (window.PointerEvent === undefined) {
  Object.assign(window, {
    PointerEvent: (class PointerEvent extends MouseEvent {
      constructor(type: string, params: PointerEventInit = {}) {
        super(type, params);
      }
    }) as typeof PointerEvent,
  });
}
// jsdom has no ResizeObserver: a minimal stand-in that records `observe()`
// calls so tests can prove the canvas gets a live size observer.
class FakeResizeObserver {
  static instances: FakeResizeObserver[] = [];
  observed: Element | null = null;
  constructor(_callback: ResizeObserverCallback) {
    FakeResizeObserver.instances.push(this);
  }
  observe(el: Element) {
    this.observed = el;
  }
  unobserve() {
    this.observed = null;
  }
  disconnect() {
    this.observed = null;
  }
}
if (typeof ResizeObserver === "undefined") {
  Object.assign(globalThis, {
    ResizeObserver: FakeResizeObserver as unknown as typeof ResizeObserver,
  });
}
vi.mock("../api", () => ({
  fetchBuildInfo: vi.fn().mockResolvedValue(null),
  openRepo: vi.fn().mockResolvedValue(null),
  loadStatus: vi.fn().mockResolvedValue(null),
  onStatusChanged: vi.fn().mockResolvedValue(null),
  planOp,
  runOp: vi.fn().mockResolvedValue(null),
  loadLog: vi.fn().mockResolvedValue(null),
  onOpStarted: vi.fn().mockResolvedValue(null),
  onOpFinished: vi.fn().mockResolvedValue(null),
  loadGraph: vi.fn().mockResolvedValue(null),
  loadCommit,
  loadStashes: async () => [],
  loadSubmodules: async () => [],
  loadBisect: async () => ({ active: false, current: null, good: [], bad: [], remaining: 0, found: null, log: [] }),
  bisectTest: async () => ({ active: false, current: null, good: [], bad: [], remaining: 0, found: null, log: [] }),
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

const identity: Viewport = { scale: 1, ox: 0, oy: 0 };

/** Three linear commits on one lane; row 0 is HEAD with a `main` ref. */
function threeNodeGraph(): Graph {
  const nodes: Node[] = [
    {
      hash: "a1b2c3",
      subject: "tip",
      author: "Al",
      date: 1700000000,
      row: 0,
      lane: 0,
      is_head: true,
      refs: [{ name: "main", kind: "Branch", color: 0 }],
      lines: [0],
    },
    {
      hash: "d4e5f6",
      subject: "mid",
      author: "Bo",
      date: 1690000000,
      row: 1,
      lane: 0,
      is_head: false,
      refs: [],
      lines: [0],
    },
    {
      hash: "a7b8c9",
      subject: "base",
      author: "Cy",
      date: 1680000000,
      row: 2,
      lane: 0,
      is_head: false,
      refs: [],
      lines: [0],
    },
  ];
  return {
    nodes,
    edges: [
      { from: "a1b2c3", to: "d4e5f6" },
      { from: "d4e5f6", to: "a7b8c9" },
    ],
    lines: [
      {
        id: 0,
        color: 0,
        name: "main",
        segments: [{ lane: 0, from_row: 0, to_row: 2 }],
      },
    ],
  };
}

/** A single-lane linear history of `n` commits. */
function straightGraph(n: number): Graph {
  const nodes: Node[] = Array.from({ length: n }, (_, i) => ({
    hash: `h${i}`,
    subject: `subject ${i}`,
    author: "Al",
    date: 1700000000 - i,
    row: i,
    lane: 0,
    is_head: i === 0,
    refs: [],
    lines: [0],
  }));
  const edges = nodes.slice(0, -1).map((n, i) => ({ from: n.hash, to: nodes[i + 1].hash }));
  return {
    nodes,
    edges,
    lines: [
      { id: 0, color: 0, name: null, segments: [{ lane: 0, from_row: 0, to_row: n - 1 }] },
    ],
  };
}

/**
 * A recording stand-in for the canvas context: counts primitives and captures
 * `fillText` calls with the alpha in effect (the virtualization + dimming
 * proof, since a real canvas context cannot be inspected in jsdom).
 */
function recorder() {
  const rec = {
    arcs: 0,
    arcRadii: [] as number[],
    band: [] as { w: number; h: number; alpha: number }[],
    strokes: 0,
    fills: 0,
    rects: [] as { x: number; y: number; w: number; h: number }[],
    strokeRects: [] as { x: number; y: number; w: number; h: number }[],
    texts: [] as { text: string; x: number; y: number; alpha: number }[],
    globalAlpha: 1,
    font: "",
    fillStyle: "",
    strokeStyle: "",
    lineWidth: 1,
    setTransform() {},
    clearRect() {},
    beginPath() {},
    moveTo() {},
    lineTo() {},
    bezierCurveTo() {},
    arc(_x: number, _y: number, r: number) {
      rec.arcs += 1;
      rec.arcRadii.push(r);
    },
    fillRect(x: number, y: number, w: number, h: number) {
      rec.rects.push({ x, y, w, h });
      if (rec.globalAlpha < 1) {
        rec.band.push({ w, h, alpha: rec.globalAlpha });
      }
    },
    strokeRect(x: number, y: number, w: number, h: number) {
      rec.strokeRects.push({ x, y, w, h });
    },
    fill() {
      rec.fills += 1;
    },
    stroke() {
      rec.strokes += 1;
    },
    fillText(text: string, x: number, y: number) {
      rec.texts.push({ text, x, y, alpha: rec.globalAlpha });
    },
  };
  return rec as CanvasCtx & typeof rec;
}

// ── visibleRowRange (the virtualization window) ─────────────────────────────

test("visibleRowRange: an identity viewport shows the top rows plus margin", () => {
  expect(visibleRowRange(100, identity, 100)).toEqual([0, 7]);
});

test("visibleRowRange: scrolling moves the window; zooming shrinks it", () => {
  // Scrolled 240px down (world top 240): rows 2..22.
  expect(visibleRowRange(100, { scale: 1, ox: 0, oy: -240 }, 300)).toEqual([2, 22]);
  // Zoomed 2× with no scroll: the canvas covers half as many world rows.
  expect(visibleRowRange(100, { scale: 2, ox: 0, oy: 0 }, 100)).toEqual([0, 6]);
});

test("visibleRowRange: a viewport with no visible content degrades to one clamped row", () => {
  // Far below the history: first clamps to the last row.
  expect(visibleRowRange(100, { scale: 1, ox: 0, oy: -100000 }, 300)).toEqual([99, 99]);
  // Far above it: the first row.
  expect(visibleRowRange(100, { scale: 1, ox: 0, oy: 100000 }, 300)).toEqual([0, 0]);
  expect(visibleRowRange(0, identity, 100)).toEqual([0, 0]);
});

// ── hitTest (screen point → node) ───────────────────────────────────────────

test("hitTest: a point on a dot selects that node", () => {
  const g = threeNodeGraph();
  // Lane 0 of 1 lane → center x = 10 + 8 = 18; row 1 → center y = 10 + 30 + 15 = 55.
  expect(hitTest(g, { x: 18, y: 55 }, identity, 1)).toBe("d4e5f6");
});

test("hitTest: a point in a row's text block selects that node", () => {
  const g = threeNodeGraph();
  expect(hitTest(g, { x: 90, y: 55 }, identity, 1)).toBe("d4e5f6");
});

test("hitTest: a zoomed viewport inverse-transforms the point", () => {
  const g = threeNodeGraph();
  // Screen (18, 55) at scale 2 → world (9, 27.5) → row 0, within the dot's radius.
  expect(hitTest(g, { x: 18, y: 55 }, { scale: 2, ox: 0, oy: 0 }, 1)).toBe("a1b2c3");
});

test("hitTest: above the first row selects nothing", () => {
  const g = threeNodeGraph();
  expect(hitTest(g, { x: 90, y: 4 }, identity, 1)).toBeNull();
});

// ── renderGraph (soundness + virtualization + dimming) ──────────────────────

test("renderGraph: a 1000-row history draws only the visible rows", () => {
  const g = straightGraph(1000);
  const rec = recorder();
  renderGraph(
    rec,
    800,
    300,
    identity,
    { graph: g, selected: null, hovered: null, query: "" },
    1,
  );
  // The 300px window at scale 1 covers rows 0..14 (±5 margin, clamped):
  // 15 rows × (subject + meta) + 1 HEAD marker = 31 text draws — not 2000.
  expect(rec.texts.length).toBe(31);
  // 15 dots + one HEAD ring = 16 arcs — not 1000.
  expect(rec.arcs).toBe(16);
});

test("renderGraph: a query dims non-matching rows and keeps matching rows opaque", () => {
  const g = threeNodeGraph();
  const rec = recorder();
  renderGraph(
    rec,
    800,
    300,
    identity,
    { graph: g, selected: null, hovered: null, query: "mid" },
    1,
  );
  const alphaOf = (text: string) => rec.texts.find((t) => t.text === text)?.alpha;
  expect(alphaOf("mid")).toBe(1);
  expect(alphaOf("tip")).toBe(0.22);
  expect(alphaOf("base")).toBe(0.22);
});

test("renderGraph: an empty graph clears and draws nothing", () => {
  const rec = recorder();
  renderGraph(
    rec,
    800,
    300,
    identity,
    { graph: { nodes: [], edges: [], lines: [] }, selected: null, hovered: null, query: "" },
    1,
  );
  expect(rec.texts.length).toBe(0);
  expect(rec.arcs).toBe(0);
});

test("renderGraph: leftmost-lane labels are clamped inside the canvas", () => {
  // A lane-0 dot sits at x=18; its ref pills and the HEAD label extend left
  // and must never cross the canvas's left edge.
  const g = threeNodeGraph();
  const rec = recorder();
  renderGraph(
    rec,
    800,
    300,
    identity,
    { graph: g, selected: null, hovered: null, query: "" },
    1,
  );
  for (const r of rec.rects) {
    expect(r.x, `rect at ${r.x}`).toBeGreaterThanOrEqual(2);
  }
  const head = rec.texts.find((t) => t.text === "HEAD");
  expect(head?.x, "HEAD label x").toBeGreaterThanOrEqual(2);
});

test("renderGraph: worktree checkouts get outlined marker pills, branches stay filled", () => {
  const g = straightGraph(2);
  g.nodes[1].refs = [
    { name: "main", kind: "Branch", color: 0 },
    { name: "side", kind: "Worktree", color: 0 },
  ];
  const rec = recorder();
  renderGraph(
    rec,
    800,
    300,
    identity,
    { graph: g, selected: null, hovered: null, query: "" },
    1,
  );
  expect(rec.rects.length).toBe(1);
  expect(rec.strokeRects.length).toBe(1);
  expect(rec.texts.some((t) => t.text === "side")).toBe(true);
  for (const r of rec.strokeRects) {
    expect(r.x).toBeGreaterThanOrEqual(2);
  }
});

test("renderGraph: an active bisect bands the suspect rows and rings its commits", () => {
  const g = straightGraph(5);
  const bisect: BisectState = {
    active: true,
    current: "h2",
    good: ["h4"],
    bad: ["h0", "h1"],
    remaining: 1,
    found: null,
    log: ["git bisect start"],
  };
  const rec = recorder();
  renderGraph(
    rec,
    800,
    300,
    identity,
    { graph: g, selected: null, hovered: null, query: "", bisect },
    1,
  );
  // One translucent band from the newest bad row to the oldest good row.
  expect(rec.band).toHaveLength(1);
  expect(rec.band[0].h).toBe(120); // rowY(4) - rowY(0) + ROW_H
  expect(rec.band[0].alpha).toBeLessThan(1);
  // 5 dots + HEAD ring + one ring per marked commit (h0, h1, h2, h4).
  expect(rec.arcRadii.filter((r) => r === 8)).toHaveLength(5);
});

test("renderGraph: a converged bisect fills the culprit dot", () => {
  const g = straightGraph(3);
  const rec = recorder();
  renderGraph(
    rec,
    800,
    300,
    identity,
    {
      graph: g,
      selected: null,
      hovered: null,
      query: "",
      bisect: {
        active: true,
        current: null,
        good: ["h2"],
        bad: ["h0"],
        remaining: 0,
        found: "h1",
        log: [],
      },
    },
    1,
  );
  const plainDots = rec.arcRadii.filter((r) => r === 5).length;
  expect(plainDots).toBe(4); // 3 dots + the filled culprit override
});

test("renderGraph: no bisect in RenderState draws no band or extra ring", () => {
  const g = straightGraph(5);
  const rec = recorder();
  renderGraph(
    rec,
    800,
    300,
    identity,
    { graph: g, selected: null, hovered: null, query: "" },
    1,
  );
  expect(rec.band).toHaveLength(0);
  // Only the HEAD ring (radius DOT_R + 3) beyond the 5 dots.
  expect(rec.arcRadii.filter((r) => r === 8)).toHaveLength(1);
});

// ── Component smoke ──────────────────────────────────────────────────────────

test("GraphCanvas renders the toolbar, a canvas, and the commit count", () => {
  useGraphStore.setState({ graph: threeNodeGraph(), loading: false });
  useRepoStore.setState({ activeId: "/repo/alpha" });
  render(<GraphCanvas />);
  expect(screen.getByText("3 commits")).toBeTruthy();
  expect(screen.getByText("Follow HEAD")).toBeTruthy();
  expect(document.querySelector("canvas.graph-canvas")).toBeTruthy();
});

test("GraphCanvas renders the open-repo prompt when nothing is active", () => {
  render(<GraphCanvas />);
  expect(screen.getByText("Open a repository to see its history")).toBeTruthy();
});
test("GraphCanvas attaches the size observer when the canvas mounts after the first graph load", async () => {
  // The component mounts with no repo (no canvas yet); when the first graph
  // arrives the canvas appears and the observer must (re)attach, or the
  // canvas bitmap is never sized beyond the 300x150 HTML default.
  useGraphStore.setState({ graph: null, loading: false });
  useRepoStore.setState({ activeId: "/repo/alpha" });
  render(<GraphCanvas />);
  expect(document.querySelector("canvas.graph-canvas")).toBeNull();
  expect(
    FakeResizeObserver.instances.some((o) => o.observed !== null),
  ).toBe(false);

  useGraphStore.setState({ graph: threeNodeGraph(), loading: false });
  await flush();
  expect(document.querySelector("canvas.graph-canvas")).toBeTruthy();
  expect(
    FakeResizeObserver.instances.some((o) => o.observed?.tagName === "SECTION"),
    "a size observer on the graph container",
  ).toBe(true);
});

// ── Component interactions (canvas event → store → drawer / menu) ──────────

const detailFixture: CommitDetail = {
  hash: "d4e5f6",
  subject: "mid",
  message: "mid",
  author_name: "Bo",
  author_email: "bo@example.com",
  author_date: 1690000000,
  committer_name: "Bo",
  committer_email: "bo@example.com",
  committer_date: 1690000000,
  parents: ["a7b8c9"],
  refs: [],
  is_head: false,
  stat: [],
  diff: [],
};

/** Deterministic microtask flush (no wall-clock timers). */
async function flush(): Promise<void> {
  await Promise.resolve();
  await Promise.resolve();
}

test("clicking a commit selects it and opens the inspect drawer", async () => {
  useGraphStore.setState({ graph: threeNodeGraph(), loading: false });
  useRepoStore.setState({ activeId: "/repo/alpha" });
  loadCommit.mockResolvedValueOnce(detailFixture);
  render(
    <>
      <GraphCanvas />
      <InspectDrawer />
    </>,
  );
  // jsdom rects are zero-sized, so screen coordinates are canvas-local:
  // lane 0 center x = 18, row 1 center y = 55 → node "d4e5f6".
  fireEvent.pointerUp(document.querySelector("canvas.graph-canvas")!, {
    clientX: 18,
    clientY: 55,
  });
  expect(useGraphStore.getState().selected).toBe("d4e5f6");
  expect(loadCommit).toHaveBeenCalledWith("/repo/alpha", "d4e5f6");
  await flush();
  expect(useGraphStore.getState().detail).toBe(detailFixture);
  expect(screen.getByText("mid")).toBeTruthy();
});

test("clicking empty space clears the selection", () => {
  useGraphStore.setState({
    graph: threeNodeGraph(),
    loading: false,
    selected: "a1b2c3",
  });
  useRepoStore.setState({ activeId: "/repo/alpha" });
  render(<GraphCanvas />);
  // y = 4 is above the first row → the hit-test finds nothing.
  fireEvent.pointerUp(document.querySelector("canvas.graph-canvas")!, {
    clientX: 90,
    clientY: 4,
  });
  expect(useGraphStore.getState().selected).toBeNull();
});

test("right-clicking a commit opens the context menu with the graph actions", () => {
  useGraphStore.setState({ graph: threeNodeGraph(), loading: false });
  useRepoStore.setState({ activeId: "/repo/alpha" });
  render(<GraphCanvas />);
  fireEvent.contextMenu(document.querySelector("canvas.graph-canvas")!, {
    clientX: 18,
    clientY: 55,
  });
  expect(screen.getByText("Create branch…")).toBeTruthy();
  expect(screen.getByText("Cherry-pick")).toBeTruthy();
  expect(screen.getByText("Reset branch to here (hard) — discards changes")).toBeTruthy();
  expect(screen.getByText("View diff")).toBeTruthy();
  expect(screen.getByText("Copy hash")).toBeTruthy();
});

test("cherry-pick asks for a target branch and sends it with the op", () => {
  planOp.mockClear();
  useGraphStore.setState({ graph: threeNodeGraph(), loading: false });
  useRepoStore.setState({ activeId: "/repo/alpha" });
  const promptSpy = vi.spyOn(window, "prompt").mockReturnValue("side");
  render(<GraphCanvas />);
  fireEvent.contextMenu(document.querySelector("canvas.graph-canvas")!, {
    clientX: 18,
    clientY: 55,
  });
  fireEvent.click(screen.getByText("Cherry-pick"));
  expect(planOp).toHaveBeenCalledWith("/repo/alpha", {
    kind: "cherry_pick",
    hashes: ["d4e5f6"],
    target: "side",
  });
  promptSpy.mockRestore();
});

test("a blank cherry-pick target picks onto the current branch", () => {
  planOp.mockClear();
  useGraphStore.setState({ graph: threeNodeGraph(), loading: false });
  useRepoStore.setState({ activeId: "/repo/alpha" });
  const promptSpy = vi.spyOn(window, "prompt").mockReturnValue("  ");
  render(<GraphCanvas />);
  fireEvent.contextMenu(document.querySelector("canvas.graph-canvas")!, {
    clientX: 18,
    clientY: 55,
  });
  fireEvent.click(screen.getByText("Cherry-pick"));
  expect(planOp).toHaveBeenCalledWith("/repo/alpha", {
    kind: "cherry_pick",
    hashes: ["d4e5f6"],
    target: null,
  });
  promptSpy.mockRestore();
});

test("canceling the cherry-pick target prompt sends no op", () => {
  planOp.mockClear();
  useGraphStore.setState({ graph: threeNodeGraph(), loading: false });
  useRepoStore.setState({ activeId: "/repo/alpha" });
  const promptSpy = vi.spyOn(window, "prompt").mockReturnValue(null);
  render(<GraphCanvas />);
  fireEvent.contextMenu(document.querySelector("canvas.graph-canvas")!, {
    clientX: 18,
    clientY: 55,
  });
  fireEvent.click(screen.getByText("Cherry-pick"));
  expect(planOp).not.toHaveBeenCalled();
  promptSpy.mockRestore();
});

test('the context menu offers "bisect from here", which drafts the bad ref and opens the tab', () => {
  useGraphStore.setState({ graph: threeNodeGraph(), loading: false });
  useRepoStore.setState({ activeId: "/repo/alpha" });
  useMainTab.getState().setTab("graph");
  render(<GraphCanvas />);
  fireEvent.contextMenu(document.querySelector("canvas.graph-canvas")!, {
    clientX: 18,
    clientY: 55,
  });
  fireEvent.click(screen.getByText("bisect from here"));
  expect(useBisectStore.getState().draftBad).toBe("d4e5f6");
  expect(useMainTab.getState().tab).toBe("bisect");
  useBisectStore.getState().setDraftBad(null);
});

test("hovering a commit shows the tooltip and hover state", () => {
  useGraphStore.setState({ graph: threeNodeGraph(), loading: false });
  useRepoStore.setState({ activeId: "/repo/alpha" });
  render(<GraphCanvas />);
  fireEvent.pointerMove(document.querySelector("canvas.graph-canvas")!, {
    clientX: 18,
    clientY: 55,
  });
  expect(useGraphStore.getState().hovered).toBe("d4e5f6");
  expect(document.querySelector(".graph-tooltip")).toBeTruthy();
});
