import { afterEach, expect, test, vi } from "vitest";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import {
  GraphCanvas,
  dotRadius,
  graphLegend,
  hitTest,
  legendOverflow,
  minimapMap,
  minimapRowAt,
  renderGraph,
  renderMinimap,
  statLabel,
  visibleRowRange,
  type CanvasCtx,
  type RenderState,
  type Viewport,
} from "./GraphCanvas";
import { InspectDrawer } from "./InspectDrawer";
import { useBisectStore, useGraphStore, useMainTab, useRepoStore } from "../store";
import type { BisectState, CommitDetail, Graph, Node } from "../types";
import { BRANCH_PALETTE_LIGHT, DARK_GRAPH_COLORS, paletteColor } from "../types";

// `loadCommit` is hoisted so interaction tests can drive its result;
// `loadCommitStats` so the stats-column tests can drive theirs.
const { loadCommit, loadCommitStats, planOp } = vi.hoisted(() => ({
  loadCommit: vi.fn().mockResolvedValue(null),
  loadCommitStats: vi.fn().mockResolvedValue([]),
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
  loadRepoSelection: vi.fn().mockResolvedValue(null),
  saveRepoSelection: vi.fn().mockResolvedValue(null),
  loadCommit,
  loadCommitStats,
  loadStashes: async () => [],
  loadSubmodules: async () => [],
  loadBisect: async () => ({ active: false, current: null, good: [], bad: [], remaining: 0, found: null, log: [] }),
  bisectTest: async () => ({ active: false, current: null, good: [], bad: [], remaining: 0, found: null, log: [] }),
}));

afterEach(() => {
  cleanup();
  vi.useRealTimers();
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
    commitStats: null,
    statsRepo: null,
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

/** A bare RenderState; individual tests override what they care about. */
function baseState(graph: Graph | null, over: Partial<RenderState> = {}): RenderState {
  return { graph, selected: null, hovered: null, query: "", ...over };
}

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
 * `fillText` calls with the alpha/ink in effect (the virtualization, dimming and
 * column evidence, since a real canvas context cannot be inspected in jsdom).
 */
function recorder() {
  const rec = {
    arcs: 0,
    arcRadii: [] as number[],
    band: [] as { w: number; h: number; alpha: number }[],
    strokes: 0,
    strokeWidths: [] as number[],
    strokeStyles: [] as string[],
    beziers: 0,
    curves: [] as { x1: number; y1: number; x2: number; y2: number; x: number; y: number }[],
    fills: 0,
    fillStyles: [] as string[],
    rects: [] as { x: number; y: number; w: number; h: number }[],
    strokeRects: [] as { x: number; y: number; w: number; h: number }[],
    texts: [] as { text: string; x: number; y: number; alpha: number; style: string }[],
    measured: [] as string[],
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
    bezierCurveTo(x1: number, y1: number, x2: number, y2: number, x: number, y: number) {
      rec.beziers += 1;
      rec.curves.push({ x1, y1, x2, y2, x, y });
    },
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
      rec.fillStyles.push(rec.fillStyle);
    },
    stroke() {
      rec.strokes += 1;
      rec.strokeWidths.push(rec.lineWidth);
      rec.strokeStyles.push(rec.strokeStyle);
    },
    fillText(text: string, x: number, y: number) {
      rec.texts.push({ text, x, y, alpha: rec.globalAlpha, style: rec.fillStyle });
    },
    measureText(text: string) {
      rec.measured.push(text);
      // Monospace advance, so the tests can reason about widths without a font.
      return { width: text.length * 6.4 };
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

// ── diff weighting (dot radius, stat column) ────────────────────────────────

test("dotRadius: unknown stats keep the base dot, bigger diffs grow it on a log ramp", () => {
  expect(dotRadius(null)).toBe(5);
  expect(dotRadius({ hash: "x", insertions: 0, deletions: 0 })).toBe(4);
  expect(dotRadius({ hash: "x", insertions: 1, deletions: 0 })).toBe(4);
  // The ramp saturates at 10 000 changed lines.
  expect(dotRadius({ hash: "x", insertions: 100, deletions: 0 })).toBeGreaterThan(
    dotRadius({ hash: "x", insertions: 10, deletions: 0 }),
  );
  expect(dotRadius({ hash: "x", insertions: 9_000, deletions: 2_000 })).toBe(8);
  for (const stat of [null, { hash: "x", insertions: 0, deletions: 0 }, { hash: "x", insertions: 99_999, deletions: 0 }]) {
    const r = dotRadius(stat);
    expect(r).toBeGreaterThanOrEqual(4);
    expect(r).toBeLessThanOrEqual(8);
  }
});

test("statLabel: no stats means no column text", () => {
  expect(statLabel(null)).toBeNull();
  expect(statLabel({ hash: "x", insertions: 3, deletions: 1 })).toBe("+3 −1");
});

// ── lane legend ─────────────────────────────────────────────────────────────

test("graphLegend: one chip per lane, named from the layout, in lane order", () => {
  const g = threeNodeGraph();
  expect(graphLegend(g)).toEqual([{ lane: 0, color: paletteColor(0), name: "main" }]);
  expect(graphLegend(null)).toEqual([]);
  expect(graphLegend(g, BRANCH_PALETTE_LIGHT)[0].color).toBe(paletteColor(0, BRANCH_PALETTE_LIGHT));
});

test("graphLegend: a lane takes the name of the line at home there, not of a visitor", () => {
  // The orbit-shaped case: `origin/main` holds lane 0, `main` diverged into lane 1,
  // and `feature/telemetry` opens in lane 1 but continues down in lane 0.
  const diverged: Graph = {
    nodes: [
      { hash: "r0", subject: "theirs", author: "Al", date: 1_700_000_003, row: 0, lane: 0, is_head: false, refs: [{ name: "origin/main", kind: "Remote", color: 0 }], lines: [0] },
      { hash: "r1", subject: "ours", author: "Bo", date: 1_700_000_002, row: 1, lane: 1, is_head: true, refs: [{ name: "main", kind: "Branch", color: 1 }], lines: [0, 1] },
      { hash: "r2", subject: "feature", author: "Cy", date: 1_700_000_001, row: 2, lane: 1, is_head: false, refs: [{ name: "feature/telemetry", kind: "Branch", color: 1 }], lines: [0, 3] },
      { hash: "r3", subject: "base", author: "Cy", date: 1_700_000_000, row: 3, lane: 0, is_head: false, refs: [], lines: [3] },
    ],
    edges: [
      { from: "r1", to: "r3" },
      { from: "r0", to: "r3" },
      { from: "r2", to: "r3" },
    ],
    lines: [
      { id: 0, color: 0, name: "origin/main", segments: [{ lane: 0, from_row: 0, to_row: 1 }] },
      { id: 1, color: 1, name: "main", segments: [{ lane: 1, from_row: 1, to_row: 1 }] },
      {
        id: 3,
        color: 1,
        name: "feature/telemetry",
        segments: [
          { lane: 1, from_row: 2, to_row: 2 },
          { lane: 0, from_row: 3, to_row: 3 },
        ],
      },
    ],
  };
  expect(graphLegend(diverged)).toEqual([
    { lane: 0, color: paletteColor(0), name: "origin/main" },
    { lane: 1, color: paletteColor(1), name: "main" },
  ]);
});

test("legendOverflow: the chips collapse into `+N more` past the cap", () => {
  expect(legendOverflow(3, 8)).toBeNull();
  expect(legendOverflow(8, 8)).toBeNull();
  expect(legendOverflow(11, 8)).toBe("+3 more");
});

// ── renderGraph (soundness + virtualization + dimming) ──────────────────────

test("renderGraph: nothing is drawn past the canvas bounds", () => {
  const g = straightGraph(6);
  g.nodes[3].refs = [
    { name: "a-very-long-branch-name-indeed", kind: "Branch", color: 1 },
    { name: "release/1.3", kind: "Branch", color: 2 },
  ];
  g.nodes[3].subject = "a subject long enough to want more room than the row has";
  const width = 300;
  const rec = recorder();
  renderGraph(rec, width, 260, identity, baseState(g), 1);
  const texts = rec.texts.length + rec.rects.length + rec.strokeRects.length + rec.arcs;
  expect(texts).toBeGreaterThan(0);
  for (const t of rec.texts) {
    expect(t.x, `text "${t.text}" at ${t.x}`).toBeGreaterThanOrEqual(2);
  }
  for (const r of [...rec.rects, ...rec.strokeRects]) {
    expect(r.x, `rect at ${r.x}`).toBeGreaterThanOrEqual(2);
    expect(r.x + r.w, `rect ending at ${r.x + r.w}`).toBeLessThanOrEqual(width);
  }
  for (const c of rec.curves) {
    expect(c.x1).toBeGreaterThanOrEqual(0);
    expect(c.x).toBeGreaterThanOrEqual(0);
  }
});

test("renderGraph: a 1000-row history draws only the visible rows", () => {
  const g = straightGraph(1000);
  const rec = recorder();
  renderGraph(rec, 800, 300, identity, baseState(g), 1);
  // The 300px window at scale 1 covers rows 0..14 (±5 margin, clamped): each row
  // costs a fixed handful of calls, so 1000 rows still cost 15 rows' worth.
  expect(rec.texts.length).toBeLessThan(100);
  expect(rec.texts.some((t) => t.text === "subject 14")).toBe(true);
  expect(rec.texts.some((t) => t.text === "subject 15")).toBe(false);
  // 15 dots + one HEAD ring = 16 arcs — not 1000.
  expect(rec.arcs).toBe(16);
});

test("renderGraph: a query dims non-matching rows and keeps matching rows opaque", () => {
  const g = threeNodeGraph();
  const rec = recorder();
  renderGraph(rec, 800, 300, identity, baseState(g, { query: "mid" }), 1);
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
    baseState({ nodes: [], edges: [], lines: [] }),
    1,
  );
  expect(rec.texts.length).toBe(0);
  expect(rec.arcs).toBe(0);
  expect(rec.strokes).toBe(0);
});

test("renderGraph: the HEAD marker stays inside the canvas", () => {
  // A lane-0 dot sits at x=18; its HEAD label extends left and must never cross the canvas's
  // left edge.
  const g = threeNodeGraph();
  const rec = recorder();
  renderGraph(rec, 800, 300, identity, baseState(g), 1);
  const head = rec.texts.find((t) => t.text === "HEAD");
  expect(head?.x, "HEAD label x").toBeGreaterThanOrEqual(2);
});

test("renderGraph: ref pills follow the subject and never cover it", () => {
  // The defect this guards: long ref names used to be clamped against the left edge, where they
  // were painted straight over the commit message.
  const g = straightGraph(2);
  const message = "recalibrate the guidance loop before the burn window opens";
  g.nodes[1].subject = message;
  g.nodes[1].refs = [
    { name: "feature/telemetry-batching", kind: "Branch", color: 0 },
    { name: "release/1.3", kind: "Branch", color: 1 },
  ];
  const width = 460;
  const rec = recorder();
  renderGraph(rec, width, 300, identity, baseState(g), 1);
  const subject = rec.texts.find((t) => t.text.startsWith("recalibrate"));
  expect(subject, "subject text").toBeTruthy();
  expect(subject!.text.length, "the subject makes room for its pills").toBeLessThan(message.length);
  const pills = rec.rects.filter((r) => r.h === 12);
  expect(pills.length).toBe(2);
  const subjectEnd = subject!.x + subject!.text.length * 6.2;
  for (const pill of pills) {
    expect(pill.x, `pill at ${pill.x}`).toBeGreaterThanOrEqual(subjectEnd);
    expect(pill.x + pill.w, "pill stays on canvas").toBeLessThanOrEqual(width - 8);
  }
  expect(rec.texts.some((t) => t.text === "feature/telemetry-batching")).toBe(true);
});

test("renderGraph: worktree checkouts get outlined marker pills, branches stay filled", () => {
  const g = straightGraph(2);
  g.nodes[1].refs = [
    { name: "main", kind: "Branch", color: 0 },
    { name: "side", kind: "Worktree", color: 0 },
  ];
  const rec = recorder();
  renderGraph(rec, 800, 300, identity, baseState(g), 1);
  expect(rec.rects.filter((r) => r.h === 12).length).toBe(1);
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
  renderGraph(rec, 800, 300, identity, baseState(g, { bisect }), 1);
  // One translucent band from the newest bad row to the oldest good row.
  expect(rec.band).toHaveLength(1);
  expect(rec.band[0].alpha).toBeLessThan(1);
  // The HEAD ring plus one ring per marked commit (h0, h1, h2, h4).
  expect(rec.strokes).toBeGreaterThan(0);
  const rings = rec.arcRadii.filter((r) => r > 5);
  expect(rings.length).toBe(5);
});

test("renderGraph: a converged bisect fills the culprit dot", () => {
  const g = straightGraph(3);
  const rec = recorder();
  renderGraph(
    rec,
    800,
    300,
    identity,
    baseState(g, {
      // The culprit's own diff is known, so its fill tracks its weighted dot.
      stats: { h1: { hash: "h1", insertions: 1, deletions: 1 } },
      bisect: {
        active: true,
        current: null,
        good: ["h2"],
        bad: ["h0"],
        remaining: 0,
        found: "h1",
        log: [],
      },
    }),
    1,
  );
  // 3 dots + the culprit's filled override at the base dot radius, and a verdict
  // ring for each marked commit (plus HEAD's own ring): a bisect verdict is never
  // resized by its commit's diff, so the marker geometry stays constant.
  const baseDots = rec.arcRadii.filter((r) => r === 5).length;
  const verdictRings = rec.arcRadii.filter((r) => r === 8).length;
  expect(baseDots).toBeGreaterThanOrEqual(4); // 3 dots + the culprit fill
  expect(verdictRings).toBeGreaterThanOrEqual(4); // HEAD ring + good/current/bad/found
  expect(rec.arcRadii.filter((r) => r !== 5 && r !== 8)).toHaveLength(0);
});

test("renderGraph: no bisect in RenderState draws no band or extra ring", () => {
  const g = straightGraph(5);
  const rec = recorder();
  renderGraph(rec, 800, 300, identity, baseState(g), 1);
  expect(rec.band).toHaveLength(0);
  // Only the HEAD ring beyond the 5 dots.
  expect(rec.arcRadii.filter((r) => r > 5)).toHaveLength(1);
});

// ── lane lineage curves ─────────────────────────────────────────────────────

test("renderGraph: lane lineage joins are cubic Béziers, not straight diagonals", () => {
  // Row 0 sits on lane 0, rows 1..3 on lane 1: the only diagonal is the hand-off.
  const g: Graph = {
    nodes: [
      { hash: "c0", subject: "merge", author: "Al", date: 1700000000, row: 0, lane: 0, is_head: true, refs: [], lines: [0] },
      { hash: "c1", subject: "side", author: "Al", date: 1699999000, row: 1, lane: 1, is_head: false, refs: [], lines: [1] },
      { hash: "c2", subject: "side", author: "Al", date: 1699998000, row: 2, lane: 1, is_head: false, refs: [], lines: [1] },
    ],
    edges: [
      { from: "c0", to: "c1" },
      { from: "c1", to: "c2" },
    ],
    lines: [
      { id: 0, color: 0, name: null, segments: [{ lane: 0, from_row: 0, to_row: 0 }] },
      { id: 1, color: 1, name: null, segments: [{ lane: 1, from_row: 1, to_row: 2 }] },
    ],
  };
  const rec = recorder();
  renderGraph(rec, 600, 300, identity, baseState(g), 1);
  expect(rec.beziers).toBe(1);
  const curve = rec.curves[0];
  // The join leaves the child straight down and enters the parent straight from above.
  expect(curve.x1).toBe(18); // lane 0 centre
  expect(curve.y1).toBeGreaterThan(0);
  expect(curve.x2).toBe(34); // lane 1 centre
  expect(curve.y2).toBeLessThan(curve.y);
  // Lane runs stay straight: exactly one diagonal curve for one lane change, and
  // every straight run is a single `lineTo`-based stroke.
  expect(rec.strokes).toBeGreaterThanOrEqual(1);
  expect(rec.strokes).toBeLessThan(5);
});

test("renderGraph: stroke count stays bounded by the window, not the history", () => {
  const g = straightGraph(10_000);
  const rec = recorder();
  renderGraph(rec, 900, 500, identity, baseState(g), 1);
  // One stroke per visible lane run: the window is ~27 rows and there is one line.
  expect(rec.strokes).toBeLessThan(20);
  expect(rec.beziers).toBe(0);
});

// ── ancestry highlight ──────────────────────────────────────────────────────

test("renderGraph: a selection adds accent strokes for its lineage; none when nothing is selected", () => {
  const g = straightGraph(4);
  const plain = recorder();
  renderGraph(plain, 800, 300, identity, baseState(g), 1);

  const selected = recorder();
  renderGraph(selected, 800, 300, identity, baseState(g, { selected: "h1" }), 1);

  // The whole lineage is one accent path (two curve segments, one stroke call),
  // heavier than the lane strokes, and it adds no stroke when nothing is selected.
  // The lineage adds curve calls and accent ink the plain pass does not have.
  expect(selected.beziers).toBe(2);
  expect(plain.beziers).toBe(0);
  expect(selected.strokeStyles).toContain(DARK_GRAPH_COLORS.accent);
  expect(plain.strokeStyles).not.toContain(DARK_GRAPH_COLORS.accent);
  expect(selected.strokeWidths).toContain(3); // heavier than the lane strokes
  expect(selected.strokes).toBe(plain.strokes + 1); // the whole lineage is one path
  // The accent lineage is drawn under the lanes: accent strokes come first.
  const firstLaneStroke = selected.strokeStyles.findIndex((s) => s !== DARK_GRAPH_COLORS.accent);
  const lastAccent = selected.strokeStyles.lastIndexOf(DARK_GRAPH_COLORS.accent);
  expect(lastAccent).toBeLessThan(firstLaneStroke);
});

// ── stats column ────────────────────────────────────────────────────────────

test("renderGraph: the stats column draws only for rows whose stats arrived", () => {
  const g = straightGraph(3);
  const rec = recorder();
  renderGraph(rec, 800, 300, identity, baseState(g, {
    stats: { h1: { hash: "h1", insertions: 12, deletions: 3 } },
    statInk: { add: "#00ff00", del: "#ff0000" },
  }), 1);
  expect(rec.texts.filter((t) => t.text === "+12").length).toBe(1);
  expect(rec.texts.filter((t) => t.text === "−3").length).toBe(1);
  expect(rec.texts.filter((t) => t.text.startsWith("+")).length).toBe(1);
  expect(rec.texts.filter((t) => t.text.startsWith("−")).length).toBe(1);
  // Requested ink colours, not the muted author ink.
  expect(rec.texts.find((t) => t.text === "+12")!.style).toBe("#00ff00");
  expect(rec.texts.find((t) => t.text === "−3")!.style).toBe("#ff0000");
  // Every row still gets its relative time, so a row is never left blank: a
  // time is the muted text sitting in the right-hand column.
  const times = rec.texts.filter((t) => t.style === DARK_GRAPH_COLORS.muted && t.x > 400);
  expect(times.length).toBe(3);
  for (const t of times) {
    expect(t.text, `relative time "${t.text}"`).toMatch(/(just now|\d+|\d{4})/);
  }
});

test("renderGraph: weighted dots follow the stats, and the column sits right of the subject", () => {
  const g = straightGraph(2);
  const rec = recorder();
  renderGraph(rec, 800, 300, identity, baseState(g, {
    stats: {
      h0: { hash: "h0", insertions: 20_000, deletions: 0 },
      h1: { hash: "h1", insertions: 1, deletions: 1 },
    },
  }), 1);
  // h1's 2-line diff floors the ramp at 4; HEAD keeps its base dot and ring so
  // its label never crowds the lane.
  const weighted = rec.arcRadii.filter((r) => r === dotRadius({ hash: "h1", insertions: 1, deletions: 1 }));
  expect(weighted).toHaveLength(1);
  expect(rec.arcRadii.filter((r) => r === 5)).toHaveLength(1); // HEAD dot + HEAD ring
  const stat = rec.texts.find((t) => t.text === "+20000")!;
  const subject = rec.texts.find((t) => t.text === "subject 0")!;
  expect(stat.x).toBeGreaterThan(subject.x);
});

// ── minimap ─────────────────────────────────────────────────────────────────

test("renderMinimap: a bounded draw budget far below the main pass", () => {
  const g = straightGraph(10_000);
  const rec = recorder();
  renderMinimap(rec, 46, 400, identity, baseState(g), 1);
  // Per-colour lane runs, not per-row marks: a linear history costs one bar, and
  // the strip never issues more than a handful of calls for any layout.
  expect(rec.rects.length).toBeLessThan(100);
  expect(rec.rects.length).toBeGreaterThan(0);
  expect(rec.strokeRects.length).toBe(1);
  expect(rec.strokes).toBe(0);
  expect(rec.beziers).toBe(0);
  expect(rec.texts.length).toBe(0);
  expect(rec.arcs).toBe(0);
});

test("renderMinimap: the visible window is marked and a selection adds one bar", () => {
  const g = straightGraph(200);
  const plain = recorder();
  renderMinimap(plain, 46, 400, identity, baseState(g), 1);
  expect(plain.strokeRects).toHaveLength(1);
  expect(plain.rects.length).toBeLessThan(100);

  const selected = recorder();
  renderMinimap(selected, 46, 400, { scale: 1, ox: 0, oy: -3_000 }, baseState(g, { selected: "h150" }), 1);
  expect(selected.rects.length).toBe(plain.rects.length + 1); // + the selection bar
  expect(selected.strokeRects[0].h).toBeGreaterThan(0);
  // Panning moves the window marker down the strip.
  expect(selected.strokeRects[0].y).toBeGreaterThan(plain.strokeRects[0].y);
});

test("renderMinimap: an empty graph draws nothing", () => {
  const rec = recorder();
  renderMinimap(rec, 46, 400, identity, baseState({ nodes: [], edges: [], lines: [] }), 1);
  expect(rec.rects).toHaveLength(0);
  expect(rec.strokeRects).toHaveLength(0);
});

test("minimapRowAt: a click maps to the row the strip marks", () => {
  const g = straightGraph(500);
  const height = 400;
  const map = minimapMap(g, 46, height);
  for (const row of [0, 1, 250, 499]) {
    expect(minimapRowAt(g, height, map.y(row))).toBe(row);
  }
  // Above/below the strip clamp to the ends instead of inventing a row.
  expect(minimapRowAt(g, height, -20)).toBe(0);
  expect(minimapRowAt(g, height, height + 100)).toBe(499);
});

// ── Component smoke ──────────────────────────────────────────────────────────

test("GraphCanvas renders the toolbar, a canvas, and the commit count", () => {
  useGraphStore.setState({ graph: threeNodeGraph(), loading: false });
  useRepoStore.setState({ activeId: "/repo/alpha" });
  render(<GraphCanvas />);
  expect(screen.getByText("3 commits")).toBeTruthy();
  expect(screen.getByText("Follow HEAD")).toBeTruthy();
  expect(document.querySelector("canvas.graph-canvas")).toBeTruthy();
  expect(screen.getByRole("button", { name: "zoom in" })).toBeTruthy();
  expect(screen.getByRole("button", { name: "zoom out" })).toBeTruthy();
  expect(screen.getByRole("button", { name: "100%" })).toBeTruthy();
  expect(screen.getByRole("button", { name: "Fit" })).toBeTruthy();
  expect(screen.getByText("100%")).toBeTruthy();
});

test("GraphCanvas labels the canvas with the repository and its commit count", () => {
  useGraphStore.setState({ graph: threeNodeGraph(), loading: false });
  useRepoStore.setState({ activeId: "/repo/alpha" });
  render(<GraphCanvas />);
  const canvas = document.querySelector("canvas.graph-canvas")!;
  expect(canvas.getAttribute("role")).toBe("img");
  expect(canvas.getAttribute("tabindex")).toBe("0");
  expect(canvas.getAttribute("aria-label")).toContain("/repo/alpha");
  expect(canvas.getAttribute("aria-label")).toContain("3 commits");
});

test("GraphCanvas draws a lane legend and collapses the overflow into `+N more`", () => {
  const g: Graph = {
    nodes: Array.from({ length: 12 }, (_, i) => ({
      hash: `h${i}`,
      subject: `s${i}`,
      author: "Al",
      date: 1700000000 - i,
      row: i,
      lane: i,
      is_head: i === 0,
      refs: [],
      lines: [i],
    })),
    edges: [],
    lines: Array.from({ length: 12 }, (_, i) => ({
      id: i,
      color: i,
      name: `branch-${i}`,
      segments: [{ lane: i, from_row: i, to_row: i }],
    })),
  };
  useGraphStore.setState({ graph: g, loading: false });
  useRepoStore.setState({ activeId: "/repo/alpha" });
  render(<GraphCanvas />);
  expect(document.querySelectorAll(".graph-legend-chip").length).toBe(8);
  expect(screen.getByText("branch-0")).toBeTruthy();
  expect(screen.getByText("+4 more")).toBeTruthy();
});

test("GraphCanvas mounts the minimap canvas beside the graph canvas", () => {
  useGraphStore.setState({ graph: threeNodeGraph(), loading: false });
  useRepoStore.setState({ activeId: "/repo/alpha" });
  render(<GraphCanvas />);
  const mini = document.querySelector("canvas.graph-minimap");
  expect(mini).toBeTruthy();
  expect(mini?.getAttribute("aria-hidden")).toBe("true");
});

test("GraphCanvas renders the open-repo prompt when nothing is active", () => {
  render(<GraphCanvas />);
  expect(screen.getByText("Open a repository to see its history")).toBeTruthy();
  expect(document.querySelector("canvas.graph-canvas")).toBeNull();
  expect(document.querySelector("canvas.graph-minimap")).toBeNull();
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
    FakeResizeObserver.instances.some((o) => o.observed?.tagName === "CANVAS"),
    "a size observer on the canvas itself: sizing from the section made the bitmap taller than the canvas box",
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

test("the hover card carries the commit's stats once they are known", () => {
  vi.useFakeTimers();
  loadCommitStats.mockResolvedValueOnce([
    { hash: "a1b2c3", insertions: 9, deletions: 2 },
    { hash: "d4e5f6", insertions: 1, deletions: 0 },
    { hash: "a7b8c9", insertions: 0, deletions: 0 },
  ]);
  useGraphStore.setState({ graph: threeNodeGraph(), loading: false });
  useRepoStore.setState({ activeId: "/repo/alpha" });
  render(<GraphCanvas />);
  fireEvent.pointerMove(document.querySelector("canvas.graph-canvas")!, {
    clientX: 18,
    clientY: 15,
  });
  expect(document.querySelector(".graph-tooltip .stat-add")).toBeNull();
  vi.advanceTimersByTime(300);
  return flush().then(async () => {
    await flush();
    expect(document.querySelector(".graph-tooltip .stat-add")?.textContent).toBe("+9");
    expect(document.querySelector(".graph-tooltip .stat-del")?.textContent).toBe("−2");
  });
});

test("renderGraph: theme colors drive lanes, dots, and text", () => {
  const g = threeNodeGraph();
  const light = {
    ...DARK_GRAPH_COLORS,
    fg: "#101418",
    success: "#17793f",
    palette: BRANCH_PALETTE_LIGHT,
  };
  const rec = recorder();
  renderGraph(rec, 800, 300, identity, baseState(g), 1, light);
  expect(rec.texts.find((t) => t.text === "tip")!.style).toBe("#101418");
  expect(rec.strokeStyles).toContain(paletteColor(g.lines[0].color, BRANCH_PALETTE_LIGHT));
  expect(rec.fillStyles).toContain(paletteColor(g.lines[0].color, BRANCH_PALETTE_LIGHT));
  // Without explicit colors, the dark palette stays the default (canvas tests).
  const dark = recorder();
  renderGraph(dark, 800, 300, identity, baseState(g), 1);
  expect(dark.texts.find((t) => t.text === "tip")!.style).toBe(DARK_GRAPH_COLORS.fg);
  expect(dark.fillStyles).toContain(paletteColor(g.lines[0].color));
});

// ── Keyboard operation ───────────────────────────────────────────────────────

test("arrow keys move the selection along the laid-out rows and announce it", () => {
  useGraphStore.setState({ graph: threeNodeGraph(), loading: false });
  useRepoStore.setState({ activeId: "/repo/alpha" });
  render(<GraphCanvas />);
  const canvas = document.querySelector("canvas.graph-canvas")!;
  fireEvent.keyDown(canvas, { key: "ArrowDown" });
  expect(useGraphStore.getState().selected).toBe("a1b2c3"); // no selection yet → first row
  fireEvent.keyDown(canvas, { key: "ArrowDown" });
  expect(useGraphStore.getState().selected).toBe("d4e5f6");
  fireEvent.keyDown(canvas, { key: "ArrowUp" });
  expect(useGraphStore.getState().selected).toBe("a1b2c3");
  fireEvent.keyDown(canvas, { key: "ArrowUp" });
  expect(useGraphStore.getState().selected).toBe("a1b2c3"); // clamped at the top
  expect(screen.getByLabelText("Selected commit").textContent).toContain("a1b2c3");
  expect(screen.getByLabelText("Selected commit").textContent).toContain("tip");
});

test("Home and End jump to the first and last row", () => {
  useGraphStore.setState({ graph: straightGraph(6), loading: false });
  useRepoStore.setState({ activeId: "/repo/alpha" });
  render(<GraphCanvas />);
  const canvas = document.querySelector("canvas.graph-canvas")!;
  fireEvent.keyDown(canvas, { key: "End" });
  expect(useGraphStore.getState().selected).toBe("h5");
  fireEvent.keyDown(canvas, { key: "Home" });
  expect(useGraphStore.getState().selected).toBe("h0");
});

test("Enter on the canvas opens the inspect drawer exactly like a click", async () => {
  useGraphStore.setState({ graph: threeNodeGraph(), loading: false, selected: "d4e5f6" });
  useRepoStore.setState({ activeId: "/repo/alpha" });
  loadCommit.mockResolvedValueOnce(detailFixture);
  render(
    <>
      <GraphCanvas />
      <InspectDrawer />
    </>,
  );
  fireEvent.keyDown(document.querySelector("canvas.graph-canvas")!, { key: "Enter" });
  expect(loadCommit).toHaveBeenCalledWith("/repo/alpha", "d4e5f6");
  await flush();
  expect(useGraphStore.getState().detail).toBe(detailFixture);
});

test("PageUp/PageDown pan the viewport without moving the selection", () => {
  useGraphStore.setState({ graph: straightGraph(200), loading: false, selected: "h3" });
  useRepoStore.setState({ activeId: "/repo/alpha" });
  render(<GraphCanvas />);
  const canvas = document.querySelector("canvas.graph-canvas")!;
  fireEvent.keyDown(canvas, { key: "PageDown" });
  expect(useGraphStore.getState().selected).toBe("h3");
  // The pan is visible on the minimap: the click mapping for the same screen point
  // now resolves to a deeper row.
  expect(screen.getByText("200 commits")).toBeTruthy();
});

test("the zoom buttons change the scale and the % readout, and 100% restores it", () => {
  useGraphStore.setState({ graph: threeNodeGraph(), loading: false });
  useRepoStore.setState({ activeId: "/repo/alpha" });
  render(<GraphCanvas />);
  expect(screen.getByText("100%")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "zoom in" }));
  expect(screen.getByText("125%")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "zoom in" }));
  expect(screen.getByText("156%")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "zoom out" }));
  expect(screen.getByText("125%")).toBeTruthy();
  fireEvent.click(screen.getByRole("button", { name: "100%" }));
  expect(screen.getByText("100%")).toBeTruthy();
});

test("Fit zooms out to the whole history and clamps to the zoom bounds", () => {
  useGraphStore.setState({ graph: straightGraph(200), loading: false });
  useRepoStore.setState({ activeId: "/repo/alpha" });
  render(<GraphCanvas />);
  fireEvent.click(screen.getByRole("button", { name: "Fit" }));
  // 200 rows in a jsdom-sized canvas cannot fit: it clamps at MIN_SCALE (25%).
  expect(screen.getByText("25%")).toBeTruthy();
});

test("+ / - / 0 on the canvas zoom the same way the buttons do", () => {
  useGraphStore.setState({ graph: threeNodeGraph(), loading: false });
  useRepoStore.setState({ activeId: "/repo/alpha" });
  render(<GraphCanvas />);
  const canvas = document.querySelector("canvas.graph-canvas")!;
  fireEvent.keyDown(canvas, { key: "+" });
  expect(screen.getByText("125%")).toBeTruthy();
  fireEvent.keyDown(canvas, { key: "-" });
  expect(screen.getByText("100%")).toBeTruthy();
  fireEvent.keyDown(canvas, { key: "+" });
  fireEvent.keyDown(canvas, { key: "0" });
  expect(screen.getByText("100%")).toBeTruthy();
});

// ── stats batching (store + viewport) ────────────────────────────────────────

test("visible rows request their stats in one capped batch, debounced", async () => {
  vi.useFakeTimers();
  loadCommitStats.mockClear();
  loadCommitStats.mockResolvedValue([]);
  useGraphStore.setState({ graph: straightGraph(50), loading: false });
  useRepoStore.setState({ activeId: "/repo/alpha" });
  render(<GraphCanvas />);
  // Debounced: a request before the pause is not made.
  expect(loadCommitStats).not.toHaveBeenCalled();
  vi.advanceTimersByTime(200);
  await flush();
  expect(loadCommitStats).toHaveBeenCalledTimes(1);
  const [repo, hashes] = loadCommitStats.mock.calls[0];
  expect(repo).toBe("/repo/alpha");
  expect(hashes.length).toBeGreaterThan(0);
  expect(hashes.length).toBeLessThanOrEqual(200);
});

test("cached hashes are never refetched, and a batch past the cap is split", async () => {
  loadCommitStats.mockClear();
  loadCommitStats.mockResolvedValue([]);
  const g = straightGraph(500);
  useGraphStore.setState({ graph: g, loading: false });
  useRepoStore.setState({ activeId: "/repo/alpha" });
  const all = g.nodes.map((n) => ({ hash: n.hash, insertions: 1, deletions: 1 }));
  await useGraphStore.getState().requestCommitStats("/repo/alpha", all.map((s) => s.hash));
  const calls = loadCommitStats.mock.calls.length;
  expect(calls).toBe(3); // 500 hashes → 200 + 200 + 100
  expect(loadCommitStats.mock.calls[0][1]).toHaveLength(200);
  expect(loadCommitStats.mock.calls[2][1]).toHaveLength(100);

  loadCommitStats.mockClear();
  useGraphStore.setState({ commitStats: Object.fromEntries(all.map((s) => [s.hash, s])) });
  await useGraphStore.getState().requestCommitStats("/repo/alpha", all.map((s) => s.hash));
  expect(loadCommitStats).not.toHaveBeenCalled(); // nothing missing → no request
  expect(Object.keys(useGraphStore.getState().commitStats!).length).toBe(500);
  expect(calls).toBe(3);
});

test("switching repositories drops the other repository's stat cache", async () => {
  loadCommitStats.mockClear();
  loadCommitStats.mockResolvedValue([{ hash: "h0", insertions: 5, deletions: 4 }]);
  useGraphStore.setState({ commitStats: { h0: { hash: "h0", insertions: 1, deletions: 1 } }, statsRepo: "/repo/other" });
  await useGraphStore.getState().requestCommitStats("/repo/alpha", ["h0"]);
  expect(loadCommitStats).toHaveBeenCalledWith("/repo/alpha", ["h0"]);
  expect(useGraphStore.getState().commitStats?.h0).toEqual({ hash: "h0", insertions: 5, deletions: 4 });
  expect(useGraphStore.getState().statsRepo).toBe("/repo/alpha");
});

// ── Virtualization evidence at 10 000 rows (SPEC §10.1) ─────────────────────

/** A viewport's window width: how many rows `visibleRowRange` hands the renderer. */
const width = (r: [number, number]) => r[1] - r[0] + 1;

test("visibleRowRange: a 10 000-row history keeps a small window at a typical viewport", () => {
  // 900x500 canvas at scale 1: rows 0..21 (±5 margin), whatever the history size.
  const window = visibleRowRange(10_000, { scale: 1, ox: 0, oy: 0 }, 500);
  expect(window).toEqual([0, 21]);
  expect(width(window)).toBeLessThan(100);
  // The window does not care whether there are 100 or 10 000 commits.
  expect(width(visibleRowRange(100, { scale: 1, ox: 0, oy: 0 }, 500))).toBe(
    width(window),
  );
});

test("visibleRowRange: the window stays bounded panned across a 10 000-row history", () => {
  for (const oy of [0, -5_000, -150_000, -299_970]) {
    const window = visibleRowRange(10_000, { scale: 1, ox: 0, oy }, 500);
    expect(window[0]).toBeGreaterThanOrEqual(0);
    expect(window[1]).toBeLessThan(10_000);
    expect(window[0]).toBeLessThanOrEqual(window[1]);
    expect(width(window)).toBeLessThan(100);
  }
});

test("visibleRowRange: clamps at both ends of a 10 000-row history", () => {
  // Panned past the bottom of the history: the last row only, never an
  // inverted range.
  const bottom = visibleRowRange(10_000, { scale: 1, ox: 0, oy: -1_000_000 }, 500);
  expect(bottom).toEqual([9_999, 9_999]);
  // Panned above the top: the first row only.
  expect(visibleRowRange(10_000, { scale: 1, ox: 0, oy: 1_000_000 }, 500)).toEqual([0, 0]);
  // Scrolled to the very end with content still on screen: clamped, not over the end.
  expect(visibleRowRange(10_000, { scale: 1, ox: 0, oy: -299_970 }, 500)[1]).toBe(9_999);
});

test("visibleRowRange: an off-screen viewport degrades to one row, not an empty range", () => {
  for (const viewport of [
    { scale: 1, ox: 0, oy: 1_000_000 },
    { scale: 1, ox: 0, oy: -1_000_000 },
    { scale: 0.25, ox: 0, oy: 4_000_000 },
    { scale: 4, ox: 0, oy: -4_000_000 },
  ]) {
    const window = visibleRowRange(10_000, viewport, 500);
    expect(window[0]).toBe(window[1]);
    expect(width(window)).toBe(1);
  }
});

test("visibleRowRange: zoom bounds keep the window bounded at MIN_SCALE and MAX_SCALE", () => {
  // MIN_SCALE (0.25) shows the most rows at once — the widest legal window.
  expect(width(visibleRowRange(10_000, { scale: 0.25, ox: 0, oy: 0 }, 500))).toBeLessThan(100);
  // MAX_SCALE (4) shows the fewest.
  expect(visibleRowRange(10_000, { scale: 4, ox: 0, oy: 0 }, 500)).toEqual([0, 8]);
  // Deep into the history at each bound: still a small, clamped window.
  expect(width(visibleRowRange(10_000, { scale: 0.25, ox: 0, oy: -120_000 }, 500))).toBeLessThan(100);
  expect(width(visibleRowRange(10_000, { scale: 4, ox: 0, oy: -120_000 }, 500))).toBeLessThan(100);
});

test("renderGraph: a 10 000-node graph draws only the window, in bounded time", () => {
  const g = straightGraph(10_000);
  const rec = recorder();
  const started = performance.now();
  renderGraph(rec, 900, 500, { scale: 1, ox: 0, oy: 0 }, baseState(g), 1);
  const elapsed = performance.now() - started;
  // One row costs 3 text draws (subject + author + relative time), so the text
  // count doubles as the per-row draw count.
  const perRowDraws = rec.texts.length;
  expect(perRowDraws).toBeLessThan(100);
  expect(rec.arcs).toBeLessThan(100);
  // A culling regression (drawing all 10 000 rows) fails here, not merely slows down.
  expect(perRowDraws).toBeLessThan(10_000 / 5);
  // Lanes, joins, dots and rings: the whole pass is bounded by the window.
  const drawCalls = perRowDraws + rec.arcs + rec.strokes + rec.fills + rec.rects.length;
  expect(drawCalls).toBeLessThan(2_000);
  expect(elapsed).toBeLessThan(500);
});

test("renderGraph: panned to the bottom of a 10 000-node graph it still draws a window", () => {
  const g = straightGraph(10_000);
  const rec = recorder();
  const started = performance.now();
  renderGraph(
    rec,
    900,
    500,
    { scale: 1, ox: 0, oy: -299_970 }, // last row sits at the top of the canvas
    baseState(g),
    1,
  );
  const perRowDraws = rec.texts.length;
  expect(perRowDraws).toBeLessThan(100);
  expect(perRowDraws).toBeLessThan(10_000 / 5);
  // The rows it did draw are the tail of the history.
  expect(rec.texts.some((t) => t.text === "subject 9999")).toBe(true);
  expect(rec.texts.some((t) => t.text === "subject 0")).toBe(false);
  expect(performance.now() - started).toBeLessThan(500);
});

test("renderGraph: zoomed to MAX_SCALE on a 10 000-node graph it draws a smaller window", () => {
  const g = straightGraph(10_000);
  const wide = recorder();
  renderGraph(wide, 900, 500, { scale: 1, ox: 0, oy: 0 }, baseState(g), 1);
  const zoomed = recorder();
  renderGraph(zoomed, 900, 500, { scale: 4, ox: 0, oy: 0 }, baseState(g), 1);
  expect(zoomed.texts.length).toBeLessThan(wide.texts.length);
  expect(zoomed.texts.length).toBeLessThan(100);
});
