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
import { useGraphStore, useRepoStore } from "../store";
import type { CommitDetail, Graph, Node } from "../types";

// `loadCommit` is hoisted so interaction tests can drive its result.
const { loadCommit } = vi.hoisted(() => ({
  loadCommit: vi.fn().mockResolvedValue(null),
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
  loadCommit,
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
    strokes: 0,
    fills: 0,
    texts: [] as { text: string; alpha: number }[],
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
    arc() {
      rec.arcs += 1;
    },
    fillRect() {},
    fill() {
      rec.fills += 1;
    },
    stroke() {
      rec.strokes += 1;
    },
    fillText(text: string, _x: number, _y: number) {
      rec.texts.push({ text, alpha: rec.globalAlpha });
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
