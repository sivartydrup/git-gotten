import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type MouseEvent as ReactMouseEvent,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
} from "react";
import { useActivityStore, useGraphStore, useRepoStore } from "../store";
import { paletteColor, type Graph, type Node, type Op } from "../types";

// ── Geometry (world units = CSS px at zoom 1) ────────────────────────────────

/** Height of one commit row. */
const ROW_H = 30;
/** Width of one lane column. */
const LANE_W = 16;
/** Padding around the lane area. */
const PAD = 10;
/** Radius of a commit dot. */
const DOT_R = 5;
/** Gap between the lane area and the text block. */
const TEXT_X_OFFSET = 14;
/** Zoom bounds. */
const MIN_SCALE = 0.25;
const MAX_SCALE = 4;
/** Rows drawn beyond the visible window on each side. */
const ROW_MARGIN = 5;
/** More refs than this collapse into a `+n` pill. */
const MAX_REF_PILLS = 4;
/** The left edge a label may not cross (keeps pills / the HEAD marker inside the canvas). */
const MIN_X = 2;

/** A canvas viewport: uniform scale + translation. */
export interface Viewport {
  scale: number;
  ox: number;
  oy: number;
}

/** World x of a lane's center. */
export function laneX(lane: number): number {
  return PAD + lane * LANE_W + LANE_W / 2;
}

/** World x where the per-row text block starts. */
export function textX(lanes: number): number {
  return PAD + lanes * LANE_W + TEXT_X_OFFSET;
}

/** World y of a row's center. */
export function rowY(row: number): number {
  return PAD + row * ROW_H + ROW_H / 2;
}

/**
 * The inclusive row range visible in a `height`-px viewport, padded by
 * `margin` rows on each side (the virtualization window, SPEC §10.1).
 */
export function visibleRowRange(
  rows: number,
  viewport: Viewport,
  height: number,
  margin = ROW_MARGIN,
): [number, number] {
  if (rows <= 0) {
    return [0, 0];
  }
  const worldTop = (0 - viewport.oy) / viewport.scale;
  const worldBottom = (height - viewport.oy) / viewport.scale;
  // Clamped so a viewport with no overlap degrades to a single row instead of
  // an empty/inverted range (first <= last always).
  const rawFirst = Math.floor((worldTop - PAD) / ROW_H) - margin;
  const rawLast = Math.ceil((worldBottom - PAD) / ROW_H) - 1 + margin;
  const first = Math.max(0, Math.min(rawFirst, rows - 1));
  const last = Math.max(first, Math.min(rawLast, rows - 1));
  return [first, last];
}

/**
 * The hash of the node under a *screen-space* point, or `null`.
 *
 * A point selects a node when it is near the node's dot, or anywhere in that
 * row's text block (the wide, obvious target).
 */
export function hitTest(
  graph: Graph,
  point: { x: number; y: number },
  viewport: Viewport,
  lanes: number,
): string | null {
  const wx = (point.x - viewport.ox) / viewport.scale;
  const wy = (point.y - viewport.oy) / viewport.scale;
  const row = Math.floor((wy - PAD) / ROW_H);
  if (row < 0) {
    return null;
  }
  const node = graph.nodes.find((n) => n.row === row);
  if (!node) {
    return null;
  }
  const x = laneX(node.lane);
  const y = rowY(node.row);
  const inDot = Math.hypot(wx - x, wy - y) <= LANE_W * 1.5;
  const inText =
    wx >= textX(lanes) - 8 && wy >= y - ROW_H / 2 && wy <= y + ROW_H / 2;
  return inDot || inText ? node.hash : null;
}

// ── Rendering ────────────────────────────────────────────────────────────────

/**
 * The drawing surface `renderGraph` needs. `CanvasRenderingContext2D`
 * satisfies this structurally; tests substitute a recording mock.
 */
export interface CanvasCtx {
  setTransform(a: number, b: number, c: number, d: number, e: number, f: number): void;
  clearRect(x: number, y: number, w: number, h: number): void;
  beginPath(): void;
  moveTo(x: number, y: number): void;
  lineTo(x: number, y: number): void;
  bezierCurveTo(
    x1: number,
    y1: number,
    x2: number,
    y2: number,
    x: number,
    y: number,
  ): void;
  arc(x: number, y: number, r: number, a0: number, a1: number): void;
  fillRect(x: number, y: number, w: number, h: number): void;
  fill(): void;
  stroke(): void;
  fillText(text: string, x: number, y: number): void;
  font: string;
  fillStyle: string | CanvasGradient | CanvasPattern;
  strokeStyle: string | CanvasGradient | CanvasPattern;
  globalAlpha: number;
  lineWidth: number;
}

/** What the renderer draws. */
export interface RenderState {
  graph: Graph | null;
  selected: string | null;
  hovered: string | null;
  query: string;
}

/**
 * Draw the virtualized slice of a graph.
 *
 * Only the visible rows (± margin) are touched, so a 10k-commit history costs
 * the same as a 100-row one (SPEC §10.1). Nodes are dots on their lane, lines
 * are the vertical lane strokes, edges are bezier curves, and each row carries
 * its subject/author/date text. A non-empty `query` dims non-matching rows
 * (by subject, author, or any ref name).
 */
export function renderGraph(
  ctx: CanvasCtx,
  width: number,
  height: number,
  viewport: Viewport,
  state: RenderState,
  dpr: number,
): void {
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.clearRect(0, 0, width, height);
  const graph = state.graph;
  if (!graph || graph.nodes.length === 0) {
    return;
  }

  const lanes = Math.max(1, ...graph.nodes.map((n) => n.lane + 1));
  const tx = textX(lanes);
  const query = state.query.trim().toLowerCase();
  const matches = (n: Node): boolean =>
    !query ||
    n.subject.toLowerCase().includes(query) ||
    n.author.toLowerCase().includes(query) ||
    n.refs.some((r) => r.name.toLowerCase().includes(query));

  const rowToNode: (Node | undefined)[] = new Array(graph.nodes.length);
  const hashToNode: Record<string, Node> = {};
  for (const n of graph.nodes) {
    rowToNode[n.row] = n;
    hashToNode[n.hash] = n;
  }

  ctx.setTransform(
    dpr * viewport.scale,
    0,
    0,
    dpr * viewport.scale,
    dpr * viewport.ox,
    dpr * viewport.oy,
  );

  const worldW = width / viewport.scale;
  // Truncation budget for the per-row text (approximate char width).
  const maxChars = Math.max(8, Math.floor((worldW - tx - 8) / 6.2));

  const [first, last] = visibleRowRange(graph.nodes.length, viewport, height);

  for (let r = first; r <= last; r++) {
    const node = rowToNode[r];
    if (!node) {
      continue;
    }
    const x = laneX(node.lane);
    const y = rowY(r);
    const dimmed = query !== "" && !matches(node);
    ctx.globalAlpha = dimmed ? 0.22 : 1;

    // Row highlight for the selection (drawn first, under everything).
    if (state.selected === node.hash) {
      ctx.fillStyle = "var(--accent, #4c8bf5)";
      ctx.globalAlpha = dimmed ? 0.08 : 0.14;
      ctx.fillRect(
        tx - TEXT_X_OFFSET,
        y - ROW_H / 2,
        worldW - tx + TEXT_X_OFFSET,
        ROW_H,
      );
      ctx.globalAlpha = dimmed ? 0.22 : 1;
    }

    // The vertical line strokes alive at this row (one half-row span each).
    for (const lineId of node.lines) {
      const line = graph.lines[lineId];
      if (!line) {
        continue;
      }
      const seg = line.segments.find((s) => s.from_row <= r && s.to_row >= r);
      if (!seg) {
        continue;
      }
      ctx.strokeStyle = paletteColor(line.color);
      ctx.lineWidth = 2;
      ctx.beginPath();
      ctx.moveTo(laneX(seg.lane), y - ROW_H / 2);
      ctx.lineTo(laneX(seg.lane), y + ROW_H / 2);
      ctx.stroke();
    }

    // Edges from this node down to its parents (bezier, routed vertically).
    for (const edge of graph.edges) {
      if (edge.from !== node.hash) {
        continue;
      }
      const parent = hashToNode[edge.to];
      if (!parent || parent.row <= r) {
        continue;
      }
      const px = laneX(parent.lane);
      const py = rowY(parent.row);
      const ownLine = graph.lines[node.lines[0]];
      ctx.strokeStyle = paletteColor(ownLine?.color ?? 0);
      ctx.lineWidth = 1.5;
      ctx.beginPath();
      ctx.moveTo(x, y);
      ctx.bezierCurveTo(x, y + ROW_H * 0.5, px, py - ROW_H * 0.5, px, py);
      ctx.stroke();
    }

    // Ref pills, stacked left of the dot (colored by their line's palette index).
    const shown = node.refs.slice(0, MAX_REF_PILLS);
    for (let i = 0; i < shown.length; i++) {
      const ref = shown[i];
      const pillW = Math.max(14, ref.name.length * 5.2 + 8);
      let px2 = x - DOT_R - 8 - (i % 2) * (pillW + 4);
      if (px2 - pillW < MIN_X) {
        // Leftmost lanes sit near the canvas edge: slide the pill right so
        // the label is never clipped (the dot is drawn on top of it).
        px2 += MIN_X - (px2 - pillW);
      }
      const py2 = y + (i < 2 ? -1 : 1) * (6 + Math.floor(i / 2) * 12);
      ctx.fillStyle = paletteColor(ref.color);
      ctx.fillRect(px2 - pillW, py2 - 6, pillW, 12);
      ctx.fillStyle = "#ffffff";
      ctx.font = "9px ui-monospace, monospace";
      ctx.fillText(ref.name, px2 - pillW + 4, py2 + 3);
    }
    if (node.refs.length > MAX_REF_PILLS) {
      ctx.fillStyle = "var(--muted, #888)";
      ctx.font = "9px ui-monospace, monospace";
      ctx.fillText(`+${node.refs.length - MAX_REF_PILLS}`, Math.max(MIN_X, x - DOT_R - 34), y + 18);
    }

    // The commit dot, in the color of the line it sits on.
    const ownLine = node.lines
      .map((id) => graph.lines[id])
      .find(
        (l) =>
          l?.segments.some(
            (s) => s.lane === node.lane && s.from_row <= r && s.to_row >= r,
          ),
      );
    ctx.fillStyle = paletteColor(ownLine?.color ?? 0);
    ctx.beginPath();
    ctx.arc(x, y, DOT_R, 0, Math.PI * 2);
    ctx.fill();

    // HEAD marker: an outer ring around the dot.
    if (node.is_head) {
      ctx.strokeStyle = paletteColor(ownLine?.color ?? 0);
      ctx.lineWidth = 2;
      ctx.beginPath();
      ctx.arc(x, y, DOT_R + 3, 0, Math.PI * 2);
      ctx.stroke();
      ctx.fillStyle = "var(--fg, #333)";
      ctx.font = "bold 10px ui-monospace, monospace";
      ctx.fillText("HEAD", Math.max(MIN_X, x - DOT_R - 30), y - 10);
    }

    // Hover ring.
    if (state.hovered === node.hash) {
      ctx.strokeStyle = "var(--fg, #333)";
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.arc(x, y, DOT_R + 2, 0, Math.PI * 2);
      ctx.stroke();
    }

    // The text block: subject + author · date.
    ctx.fillStyle = "var(--fg, #333)";
    ctx.font = "12px -apple-system, 'Segoe UI', sans-serif";
    ctx.fillText(truncate(node.subject, maxChars), tx, y - 4);
    ctx.fillStyle = "var(--muted, #888)";
    ctx.font = "11px -apple-system, 'Segoe UI', sans-serif";
    ctx.fillText(
      truncate(`${node.author} · ${formatDate(node.date)}`, maxChars),
      tx,
      y + 11,
    );
  }
  ctx.globalAlpha = 1;
}

/** Trim a string to `max` characters with an ellipsis. */
function truncate(s: string, max: number): string {
  return s.length <= max ? s : s.slice(0, Math.max(1, max - 1)) + "…";
}

/** Unix seconds → a short local timestamp (`Aug 12 14:03`). */
function formatDate(unixSeconds: number): string {
  const d = new Date(unixSeconds * 1000);
  const months = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
  const hh = String(d.getHours()).padStart(2, "0");
  const mm = String(d.getMinutes()).padStart(2, "0");
  return `${months[d.getMonth()]} ${d.getDate()} ${hh}:${mm}`;
}

// ── Component ────────────────────────────────────────────────────────────────

/** A right-click menu pinned to a node. */
interface ContextMenuState {
  x: number;
  y: number;
  hash: string;
}

/**
 * The branching graph (SPEC §10.1, the main view): a canvas rendering of the
 * Rust-computed layout with zoom/pan, virtualized drawing, hit-testing
 * (select → inspect drawer), hover tooltips, a context menu (the graph
 * operations), and search-driven dimming.
 */
export function GraphCanvas() {
  const activeId = useRepoStore((s) => s.activeId);
  const {
    graph,
    loading,
    error,
    selected,
    hovered,
    query,
    select,
    hover,
  } = useGraphStore();
  const { requestPreview, running } = useActivityStore();

  const containerRef = useRef<HTMLDivElement | null>(null);
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const viewportRef = useRef<Viewport>({ scale: 1, ox: 0, oy: 0 });
  const dirtyRef = useRef(true);
  const sizeRef = useRef({ width: 0, height: 0 });
  const pointerPos = useRef({ x: 0, y: 0 });
  const dragRef = useRef<{ x: number; y: number; ox: number; oy: number; moved: boolean } | null>(null);
  const [menu, setMenu] = useState<ContextMenuState | null>(null);

  const lanes = useMemo(
    () => (graph ? Math.max(1, ...graph.nodes.map((n) => n.lane + 1)) : 1),
    [graph],
  );
  const nodesCount = graph?.nodes.length ?? 0;
  const head = useMemo(() => graph?.nodes.find((n) => n.is_head) ?? null, [graph]);
  // The canvas element only exists in the drawn body state. Effects that
  // capture it must re-run when it appears/disappears (the component mounts
  // with no repo, so `canvasRef` is `null` until the first graph loads).
  const hasCanvas = !error && !loading && !!graph && graph.nodes.length > 0;

  // Keep the latest render inputs in a ref for the rAF loop.
  const stateRef = useRef({ graph, selected, hovered, query, lanes });
  stateRef.current = { graph, selected, hovered, query, lanes };

  // Redraw when anything renderable changes.
  useEffect(() => {
    dirtyRef.current = true;
  }, [graph, selected, hovered, query, nodesCount, menu]);

  // Size tracking (device pixels) via ResizeObserver.
  useEffect(() => {
    const container = containerRef.current;
    const canvas = canvasRef.current;
    // jsdom has no ResizeObserver; skip sizing in non-browser test envs.
    if (!container || !canvas || typeof ResizeObserver === "undefined") {
      return;
    }
    const ro = new ResizeObserver(() => {
      const rect = container.getBoundingClientRect();
      const dpr = window.devicePixelRatio || 1;
      sizeRef.current = { width: rect.width, height: rect.height };
      canvas.width = Math.max(1, Math.round(rect.width * dpr));
      canvas.height = Math.max(1, Math.round(rect.height * dpr));
      dirtyRef.current = true;
    });
    ro.observe(container);
    return () => ro.disconnect();
  }, [hasCanvas]);

  // The rAF render loop: draw only when dirty.
  useEffect(() => {
    let raf = 0;
    const draw = () => {
      raf = requestAnimationFrame(draw);
      if (!dirtyRef.current) {
        return;
      }
      dirtyRef.current = false;
      const canvas = canvasRef.current;
      const ctx = canvas?.getContext("2d");
      if (!ctx) {
        return;
      }
      const dpr = window.devicePixelRatio || 1;
      renderGraph(
        ctx,
        sizeRef.current.width,
        sizeRef.current.height,
        viewportRef.current,
        stateRef.current,
        dpr,
      );
    };
    raf = requestAnimationFrame(draw);
    return () => cancelAnimationFrame(raf);
  }, []);

  const mark = useCallback(() => {
    dirtyRef.current = true;
  }, []);

  const toLocal = (e: { clientX: number; clientY: number }) => {
    const rect = canvasRef.current?.getBoundingClientRect();
    return { x: e.clientX - (rect?.left ?? 0), y: e.clientY - (rect?.top ?? 0) };
  };

  // Wheel zoom is anchored at the cursor (non-passive: prevent the page scroll).
  useEffect(() => {
    const canvas = canvasRef.current;
    if (!canvas) {
      return;
    }
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      const v = viewportRef.current;
      const factor = e.deltaY < 0 ? 1.1 : 1 / 1.1;
      const scale = Math.min(MAX_SCALE, Math.max(MIN_SCALE, v.scale * factor));
      const rect = canvas.getBoundingClientRect();
      const x = e.clientX - rect.left;
      const y = e.clientY - rect.top;
      // Keep the world point under the cursor fixed.
      v.ox = x - ((x - v.ox) / v.scale) * scale;
      v.oy = y - ((y - v.oy) / v.scale) * scale;
      v.scale = scale;
      dirtyRef.current = true;
    };
    canvas.addEventListener("wheel", onWheel, { passive: false });
    return () => canvas.removeEventListener("wheel", onWheel);
  }, [hasCanvas]);

  const onPointerDown = (e: ReactPointerEvent) => {
    if (e.button !== 0) {
      return;
    }
    const p = toLocal(e);
    pointerPos.current = p;
    dragRef.current = {
      x: p.x,
      y: p.y,
      ox: viewportRef.current.ox,
      oy: viewportRef.current.oy,
      moved: false,
    };
    (e.target as Element).setPointerCapture?.(e.pointerId);
    setMenu(null);
  };

  const onPointerMove = (e: ReactPointerEvent) => {
    const p = toLocal(e);
    pointerPos.current = p;
    const drag = dragRef.current;
    if (drag && e.buttons === 1) {
      const dx = p.x - drag.x;
      const dy = p.y - drag.y;
      if (Math.abs(dx) + Math.abs(dy) > 3) {
        drag.moved = true;
      }
      viewportRef.current.ox = drag.ox + dx;
      viewportRef.current.oy = drag.oy + dy;
      dirtyRef.current = true;
      return;
    }
    // Hover (no drag): tooltip + hover ring.
    if (!graph) {
      return;
    }
    const hash = hitTest(graph, p, viewportRef.current, lanes);
    if (hash !== hovered) {
      hover(hash);
      mark();
    }
  };

  const onPointerUp = (e: ReactPointerEvent) => {
    const drag = dragRef.current;
    dragRef.current = null;
    if (drag?.moved) {
      return;
    }
    const p = toLocal(e);
    const hash = graph ? hitTest(graph, p, viewportRef.current, lanes) : null;
    select(hash);
  };

  const onPointerLeave = () => {
    if (hovered !== null) {
      hover(null);
      mark();
    }
  };

  const onContextMenu = (e: ReactMouseEvent) => {
    e.preventDefault();
    if (!graph) {
      return;
    }
    const p = toLocal(e);
    const hash = hitTest(graph, p, viewportRef.current, lanes);
    if (!hash) {
      setMenu(null);
      return;
    }
    setMenu({ x: p.x, y: p.y, hash });
  };

  const runMenuOp = (op: Op) => {
    setMenu(null);
    if (activeId) {
      void requestPreview(activeId, op);
    }
  };

  const copyText = (text: string) => {
    void navigator.clipboard?.writeText(text).catch(() => {});
  };

  const menuNode: Node | null = menu
    ? graph?.nodes.find((n) => n.hash === menu.hash) ?? null
    : null;

  const hoveredNode: Node | null = hovered
    ? graph?.nodes.find((n) => n.hash === hovered) ?? null
    : null;

  // ── Body states ────────────────────────────────────────────────────────────

  let body: ReactNode;
  if (error) {
    body = (
      <div className="graph-state error">
        <div>Failed to load the graph</div>
        <code>{error}</code>
      </div>
    );
  } else if (loading) {
    body = <div className="graph-state">Loading graph…</div>;
  } else if (!graph || graph.nodes.length === 0) {
    body = (
      <div className="graph-state">
        {activeId ? "No commits" : "Open a repository to see its history"}
      </div>
    );
  } else {
    body = (
      <>
        <canvas
          ref={canvasRef}
          className="graph-canvas"
          onPointerDown={onPointerDown}
          onPointerMove={onPointerMove}
          onPointerUp={onPointerUp}
          onPointerLeave={onPointerLeave}
          onDoubleClick={() => {
            viewportRef.current = { scale: 1, ox: 0, oy: 0 };
            mark();
          }}
          onContextMenu={onContextMenu}
        />
        {hoveredNode && (
          <div
            className="graph-tooltip"
            style={{
              left: pointerPos.current.x + 12,
              top: pointerPos.current.y + 12,
            }}
          >
            <code>{hoveredNode.hash.slice(0, 12)}</code> {hoveredNode.subject}
            <span className="muted">
              {" "}
              {hoveredNode.author} · {formatDate(hoveredNode.date)}
            </span>
          </div>
        )}
      </>
    );
  }

  return (
    <section className="graph" ref={containerRef}>
      <div className="graph-toolbar">
        <button
          type="button"
          className="btn"
          disabled={!graph || graph.nodes.length === 0}
          onClick={() => {
            viewportRef.current.oy = head
              ? -rowY(head.row) * viewportRef.current.scale + 12
              : 0;
            mark();
          }}
        >
          Follow HEAD
        </button>
        <button
          type="button"
          className="btn"
          disabled={!graph || graph.nodes.length === 0}
          onClick={() => {
            viewportRef.current = { scale: 1, ox: 0, oy: 0 };
            mark();
          }}
        >
          Reset view
        </button>
        {graph && graph.nodes.length > 0 && (
          <span className="graph-count">{graph.nodes.length} commits</span>
        )}
      </div>
      {body}
      {menu && menuNode && (
        <div
          className="graph-context-menu"
          style={{ left: menu.x, top: menu.y }}
          onContextMenu={(e) => e.preventDefault()}
        >
          <MenuButton
            label="Create branch…"
            disabled={running}
            onClick={() => {
              const name = window.prompt(
                `Branch name (from ${menuNode.hash.slice(0, 12)})`,
              );
              if (name && name.trim()) {
                runMenuOp({
                  kind: "branch_create",
                  name: name.trim(),
                  from: menuNode.hash,
                  checkout: false,
                });
              }
              setMenu(null);
            }}
          />
          <MenuButton
            label="Create branch & switch"
            disabled={running}
            onClick={() => {
              const name = window.prompt(
                `Branch name (from ${menuNode.hash.slice(0, 12)})`,
              );
              if (name && name.trim()) {
                runMenuOp({
                  kind: "branch_create",
                  name: name.trim(),
                  from: menuNode.hash,
                  checkout: true,
                });
              }
              setMenu(null);
            }}
          />
          <MenuButton
            label="Cherry-pick"
            disabled={running}
            onClick={() => {
              const name = window.prompt(
                "Cherry-pick onto branch (blank = current branch)",
              );
              if (name !== null) {
                runMenuOp({
                  kind: "cherry_pick",
                  hashes: [menuNode.hash],
                  target: name.trim() || null,
                });
              }
            }}
          />
          <MenuButton
            label="Reset branch to here (soft)"
            disabled={running}
            onClick={() => runMenuOp({ kind: "reset", mode: "soft", target: menuNode.hash })}
          />
          <MenuButton
            label="Reset branch to here (mixed)"
            disabled={running}
            onClick={() => runMenuOp({ kind: "reset", mode: "mixed", target: menuNode.hash })}
          />
          <MenuButton
            label="Reset branch to here (hard) — discards changes"
            disabled={running}
            onClick={() => runMenuOp({ kind: "reset", mode: "hard", target: menuNode.hash })}
          />
          <div className="graph-context-sep" />
          <MenuButton
            label="View diff"
            onClick={() => {
              select(menuNode.hash);
              setMenu(null);
            }}
          />
          <MenuButton
            label="Copy hash"
            onClick={() => {
              copyText(menuNode.hash);
              setMenu(null);
            }}
          />
          <MenuButton
            label="Copy message"
            onClick={() => {
              copyText(menuNode.subject);
              setMenu(null);
            }}
          />
        </div>
      )}
    </section>
  );
}

/** One context-menu row. */
function MenuButton({
  label,
  onClick,
  disabled,
}: {
  label: string;
  onClick: () => void;
  disabled?: boolean;
}) {
  return (
    <button
      type="button"
      className="graph-context-item"
      disabled={disabled}
      onClick={onClick}
    >
      {label}
    </button>
  );
}
