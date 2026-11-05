import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type MouseEvent as ReactMouseEvent,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
} from "react";
import {
  useActivityStore,
  useBisectStore,
  useGraphStore,
  useMainTab,
  useRepoStore,
  useThemeStore,
} from "../store";
import {
  DARK_GRAPH_COLORS,
  paletteColor,
  relativeTime,
  resolveGraphColors,
  type BisectState,
  type CommitStat,
  type Graph,
  type GraphColors,
  type Node,
  type Op,
} from "../types";
import { Icon } from "./Icons";
import "./graph.css";

// ── Geometry (world units = CSS px at zoom 1) ────────────────────────────────

/** Height of one commit row. */
const ROW_H = 30;
/** Width of one lane column. */
const LANE_W = 16;
/** Padding around the lane area. */
const PAD = 10;
/** Commit dot radius when nothing is known about the commit. */
const DOT_R = 5;
/** Dot radius bounds for stat-weighted dots (world units). */
const DOT_R_MIN = 4;
const DOT_R_MAX = 8;
/** Gap between the lane area and the text block. */
const TEXT_X_OFFSET = 14;
/** Width the `+ins −dels` meta column reserves when truncating a subject. */
const STAT_W = 66;
/** Gap between the subject band and the meta column. */
const STAT_GAP = 10;
/** Extra inset so text never sits under the minimap or its border. */
const MINIMAP_INK_INSET = 12;
/** Zoom bounds. */
const MIN_SCALE = 0.25;
const MAX_SCALE = 4;
/** Zoom steps for the toolbar / keyboard. */
const ZOOM_STEP = 1.25;
/** Rows drawn beyond the visible window on each side. */
const ROW_MARGIN = 5;
/** More refs than this collapse into a `+n` pill. */
const MAX_REF_PILLS = 4;
/** The left edge a label may not cross (keeps pills / the HEAD marker inside the canvas). */
const MIN_X = 2;
/** Minimap width in CSS px. */
const MINIMAP_W = 46;
/** How many of the graph's lanes the toolbar legend shows. */
const MAX_LEGEND_LANES = 8;

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
export function textX(lanes: number, minimapWidth = 0): number {
  return PAD + lanes * LANE_W + TEXT_X_OFFSET + minimapWidth;
}

/** A row's ref pills and the width each label needs (approximate monospace advance). */
function refPills(node: Graph["nodes"][number]) {
  return node.refs.slice(0, MAX_REF_PILLS).map((ref) => ({
    ref,
    width: Math.max(14, ref.name.length * 5.6 + 8),
  }));
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

/**
 * Dot radius for a commit, on a log ramp over the total lines it changes:
 * a 1-line fix stays near the floor, a 10k-line import reaches the ceiling.
 * Unknown stats keep the neutral base radius.
 */
export function dotRadius(stat?: CommitStat | null): number {
  if (!stat) {
    return DOT_R;
  }
  const total = Math.max(0, stat.insertions) + Math.max(0, stat.deletions);
  if (total <= 0) {
    return DOT_R_MIN;
  }
  // log10 is only exact at the decade points, so the ramp steps between them.
  // floor guard: log10(1000) can land a hair below 3 in floating point.
  const step = Math.max(0, Math.min(4, Math.floor(Math.log10(total) + 1e-9)));
  return DOT_R_MIN + ((DOT_R_MAX - DOT_R_MIN) * step) / 4;
}

/** The `+ins −dels` label for a row, or `null` when its stats have not arrived. */
export function statLabel(stat?: CommitStat | null): string | null {
  if (!stat) {
    return null;
  }
  return `+${stat.insertions} −${stat.deletions}`;
}

/** The `+n more` overflow label for the lane legend. */
export function legendOverflow(laneCount: number, shown: number): string | null {
  return laneCount > shown ? `+${laneCount - shown} more` : null;
}

/** The lanes a laid-out graph actually uses, in lane order, with a colour and a name. */
export function graphLegend(
  graph: Graph | null,
  palette: readonly string[] = DARK_GRAPH_COLORS.palette,
): { lane: number; color: string; name: string }[] {
  if (!graph || graph.nodes.length === 0) {
    return [];
  }
  const byLane = new Map<number, { lane: number; color: string; name: string }>();
  const laneCount = Math.max(1, ...graph.nodes.map((n) => n.lane + 1));
  for (let lane = 0; lane < laneCount; lane++) {
    const lineIds = new Set<number>();
    for (const node of graph.nodes) {
      if (node.lane === lane) {
        for (const id of node.lines) {
          lineIds.add(id);
        }
      }
    }
    const lines = graph.lines.filter((l) => lineIds.has(l.id));
    // A lane is named by the line that is *at home* in it: where that line's first
    // segment is drawn. A line routed through a lane (a branch continuing in another
    // lane) must not give the lane its name, or two lanes read as the same branch.
    const named =
      lines.find((line) => line.name && line.segments[0]?.lane === lane) ??
      lines.find((line) => line.name);
    byLane.set(lane, {
      lane,
      color: paletteColor((named ?? lines[0])?.color ?? 0, palette),
      name: named?.name ?? (lines[0]?.name ?? `lane ${lane + 1}`),
    });
  }
  return [...byLane.values()];
}

/**
 * The hashes on the lineage from `hash` toward the root (including `hash`),
 * computed from `graph.edges` alone so the highlight is the real ancestry path,
 * not whatever lanes happen to be drawn.
 */
export function ancestryChain(graph: Graph, hash: string): string[] {
  const parents: Record<string, string[]> = {};
  for (const e of graph.edges) {
    (parents[e.from] ??= []).push(e.to);
  }
  const seen = new Set<string>([hash]);
  const chain = [hash];
  const queue = [hash];
  while (queue.length > 0) {
    const current = queue.shift()!;
    for (const parent of parents[current] ?? []) {
      if (seen.has(parent)) {
        continue;
      }
      seen.add(parent);
      chain.push(parent);
      queue.push(parent);
    }
  }
  return chain;
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
  strokeRect(x: number, y: number, w: number, h: number): void;
  fill(): void;
  stroke(): void;
  fillText(text: string, x: number, y: number): void;
  measureText(text: string): { width: number };
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
  /** Optional live bisect overlay (SPEC §10.9); absent draws no overlay. */
  bisect?: BisectState | null;
  /** Per-commit line counts; an absent hash draws no stat column and a base dot. */
  stats?: Record<string, CommitStat> | null;
  /** Inks for the stat column (canvas cannot resolve `var()`). */
  statInk?: { add: string; del: string };
  /** CSS px reserved at the canvas's right edge for the minimap (kept clear of text). */
  minimapWidth?: number;
}

/** Ink for the stat column; canvas colours must be literal, not tokens. */
const STAT_INK_FALLBACK = { add: "#58b368", del: "#e05252" };

/**
 * Draw the virtualized slice of a graph.
 *
 * Only the visible rows (± margin) are touched, so a 10k-commit history costs
 * the same as a 100-row one (SPEC §10.1). Nodes are dots on their lane, sized by
 * the lines they change, lane lineage joins are cubic Béziers, and each row carries
 * its subject/author text plus a right-aligned relative-time and `+ins −dels` column.
 * A non-empty `query` dims non-matching rows (by subject, author, or any ref name);
 * a selection lays its ancestry path under the lanes in the accent colour.
 */
export function renderGraph(
  ctx: CanvasCtx,
  width: number,
  height: number,
  viewport: Viewport,
  state: RenderState,
  dpr: number,
  colors: GraphColors = DARK_GRAPH_COLORS,
): void {
  const C = colors;
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.clearRect(0, 0, width, height);
  const graph = state.graph;
  if (!graph || graph.nodes.length === 0) {
    return;
  }

  const minimapW = state.minimapWidth ?? 0;
  const lanes = Math.max(1, ...graph.nodes.map((n) => n.lane + 1));
  const tx = textX(lanes, minimapW);
  const inkAdd = state.statInk?.add ?? STAT_INK_FALLBACK.add;
  const inkDel = state.statInk?.del ?? STAT_INK_FALLBACK.del;
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

  // Ancestry highlight (SPEC §10.1): the selected commit's lineage toward the root.
  const ancestry = new Set<string>();
  if (state.selected && hashToNode[state.selected]) {
    for (const h of ancestryChain(graph, state.selected)) {
      ancestry.add(h);
    }
  }

  const worldW = width / viewport.scale;
  const rightPad = minimapW + MINIMAP_INK_INSET;

  ctx.setTransform(
    dpr * viewport.scale,
    0,
    0,
    dpr * viewport.scale,
    dpr * viewport.ox,
    dpr * viewport.oy,
  );

  // Bisect overlay (SPEC §10.9): the suspect span between the newest `bad`
  // row and the oldest `good` row.
  const bisect = state.bisect;
  const bisectMark = (hash: string): "good" | "bad" | "current" | "found" | null => {
    if (!bisect) {
      return null;
    }
    if (bisect.found === hash) {
      return "found";
    }
    if (bisect.current === hash) {
      return "current";
    }
    if (bisect.good.includes(hash)) {
      return "good";
    }
    if (bisect.bad.includes(hash)) {
      return "bad";
    }
    return null;
  };
  if (bisect?.active) {
    let badTop = -1;
    let goodBottom = -1;
    for (const h of bisect.bad) {
      const r = hashToNode[h]?.row ?? -1;
      if (r > badTop) {
        badTop = r;
      }
    }
    for (const h of bisect.good) {
      const r = hashToNode[h]?.row ?? -1;
      if (r > goodBottom) {
        goodBottom = r;
      }
    }
    if (badTop >= 0 && goodBottom >= badTop) {
      ctx.fillStyle = C.error;
      ctx.globalAlpha = 0.1;
      ctx.fillRect(
        MIN_X,
        rowY(badTop) - ROW_H / 2,
        worldW - MIN_X * 2,
        rowY(goodBottom) - rowY(badTop) + ROW_H,
      );
      ctx.globalAlpha = 1;
    }
  }

  const [first, last] = visibleRowRange(graph.nodes.length, viewport, height);

  // Truncation budget for the per-row text (approximate char width). A row shares its line with
  // ref pills and the right-hand meta column, so both bands are reserved before the subject is
  // truncated: neither a long branch name nor the stat column can be painted over the message.
  const textBudget = (reservedPx: number) =>
    Math.max(8, Math.floor((worldW - tx - rightPad - STAT_W - STAT_GAP - reservedPx) / 6.2));

  // The curve a lane join follows: straight down from the child, straight into the
  // parent, with the control points a row-half apart so the bend never overshoots.
  // `stroke` is false while the ancestry path is being built (it strokes once).
  const laneCurve = (x0: number, y0: number, x1: number, y1: number, stroke = true) => {
    ctx.beginPath();
    ctx.moveTo(x0, y0);
    ctx.bezierCurveTo(
      x0,
      y0 + ROW_H * 0.5,
      x1,
      y1 - ROW_H * 0.5,
      x1,
      y1,
    );
    if (stroke) {
      ctx.stroke();
    }
  };

  // ── Pass 1: the selected commit's ancestry, under every lane ───────────────
  if (ancestry.size > 0) {
    ctx.strokeStyle = C.accent;
    ctx.globalAlpha = 0.55;
    ctx.lineWidth = 3;
    // One path for the whole lineage, stroked once: a 40-row lineage costs 40
    // curve segments and a single `stroke`.
    let lineageSegments = 0;
    for (const hash of ancestry) {
      const node = hashToNode[hash];
      if (!node || node.row < first - 1 || node.row > last) {
        continue;
      }
      for (const edge of graph.edges) {
        if (edge.from !== hash || !ancestry.has(edge.to)) {
          continue;
        }
        const parent = hashToNode[edge.to];
        if (!parent || parent.row <= node.row || parent.row > last + 1) {
          continue;
        }
        laneCurve(laneX(node.lane), rowY(node.row), laneX(parent.lane), rowY(parent.row), false);
        lineageSegments += 1;
      }
    }
    if (lineageSegments > 0) {
      ctx.stroke();
    }
    ctx.globalAlpha = 1;
  }

  // ── Pass 2: lane strokes and the lineage curves that join them ─────────────
  // Straight runs stay one stroke per lane segment (a 5 000-row line costs one
  // call), and every lane change is one cubic Bézier from the child's dot to the
  // parent's dot. A hand-off is keyed by the edge it draws, so a lane change can
  // never be stroked twice.
  ctx.lineWidth = 2;
  const laneRuns = new Map<number, { from: number; to: number }[]>();
  const joins: { line: number; fromRow: number; toRow: number }[] = [];
  for (const line of graph.lines) {
    const runs: { from: number; to: number }[] = [];
    for (const seg of line.segments) {
      const from = Math.max(seg.from_row, first);
      const to = Math.min(seg.to_row, last);
      if (from < to) {
        runs.push({ from, to });
      }
      const opener = rowToNode[seg.from_row - 1];
      const opened = rowToNode[seg.from_row];
      if (
        opener &&
        opened &&
        opener.lane !== seg.lane &&
        opened.lane === seg.lane &&
        graph.edges.some((e) => e.from === opener.hash && e.to === opened.hash) &&
        seg.from_row >= first &&
        seg.from_row <= last + 1
      ) {
        joins.push({ line: line.id, fromRow: opener.row, toRow: opened.row });
      }
      const holder = rowToNode[seg.to_row];
      const handoff = rowToNode[seg.to_row + 1];
      if (
        holder &&
        handoff &&
        holder.lane === seg.lane &&
        handoff.lane !== seg.lane &&
        graph.edges.some((e) => e.from === holder.hash && e.to === handoff.hash) &&
        seg.to_row >= first - 1 &&
        seg.to_row <= last
      ) {
        joins.push({ line: line.id, fromRow: holder.row, toRow: handoff.row });
      }
    }
    if (runs.length > 0) {
      laneRuns.set(line.id, runs);
    }
  }
  // Each lane change is drawn once, in the colour of the line it joins, as a
  // cubic Bézier from the child's dot to the parent's dot.
  const seenJoins = new Set<string>();
  for (const join of joins) {
    const key = `${join.fromRow}->${join.toRow}`;
    if (seenJoins.has(key)) {
      continue;
    }
    seenJoins.add(key);
    const child = rowToNode[join.fromRow]!;
    const parent = rowToNode[join.toRow]!;
    ctx.strokeStyle = paletteColor(
      graph.lines[join.line]?.color ?? child.lines[0] ?? 0,
      C.palette,
    );
    laneCurve(
      laneX(child.lane),
      rowY(child.row),
      laneX(parent.lane),
      rowY(parent.row),
    );
  }

  for (const line of graph.lines) {
    const runs = laneRuns.get(line.id);
    if (!runs) {
      continue;
    }
    ctx.strokeStyle = paletteColor(line.color, C.palette);
    for (const run of runs) {
      ctx.beginPath();
      ctx.moveTo(laneX(rowToNode[run.from]?.lane ?? 0), rowY(run.from));
      ctx.lineTo(laneX(rowToNode[run.to]?.lane ?? 0), rowY(run.to));
      ctx.stroke();
    }
  }

  // ── Pass 3: per-row content ───────────────────────────────────────────────
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
      ctx.fillStyle = C.accent;
      ctx.globalAlpha = dimmed ? 0.08 : 0.14;
      ctx.fillRect(
        tx - TEXT_X_OFFSET,
        y - ROW_H / 2,
        worldW - tx + TEXT_X_OFFSET - rightPad,
        ROW_H,
      );
      ctx.globalAlpha = dimmed ? 0.22 : 1;
    }

    const stat = state.stats?.[node.hash] ?? null;
    // Weighting is a subtle scale change, so the markers that carry text stay
    // base-sized: HEAD's ring/label and a bisect verdict read identically.
    const mark = bisectMark(node.hash);
    const rDot = stat && !node.is_head && !mark ? dotRadius(stat) : DOT_R;

    // The commit dot, in the color of the line it sits on, weighted by its diff.
    const ownLine = node.lines
      .map((id) => graph.lines[id])
      .find(
        (l) =>
          l?.segments.some(
            (s) => s.lane === node.lane && s.from_row <= r && s.to_row >= r,
          ),
      );
    ctx.fillStyle = paletteColor(ownLine?.color ?? 0, C.palette);
    ctx.beginPath();
    ctx.arc(x, y, rDot, 0, Math.PI * 2);
    ctx.fill();

    // Bisect verdict first: it re-rings the commit it marks, so HEAD's marker
    // does not stack a second ring on top of the verdict ring.
    if (mark) {
      const color =
        mark === "good" || mark === "found"
          ? "#58b368"
          : mark === "bad"
            ? "#e05252"
            : "#4c8bf5";
      ctx.strokeStyle = color;
      ctx.lineWidth = 2;
      ctx.beginPath();
      ctx.arc(x, y, rDot + 3, 0, Math.PI * 2);
      ctx.stroke();
      if (mark === "found") {
        // The culprit reads as a filled dot, not just a ring.
        ctx.fillStyle = color;
        ctx.beginPath();
        ctx.arc(x, y, rDot, 0, Math.PI * 2);
        ctx.fill();
      }
    }

    // HEAD marker: an outer ring around the dot.
    if (node.is_head) {
      ctx.strokeStyle = paletteColor(ownLine?.color ?? 0, C.palette);
      ctx.lineWidth = 2;
      ctx.beginPath();
      ctx.arc(x, y, rDot + 3, 0, Math.PI * 2);
      ctx.stroke();
      ctx.fillStyle = C.fg;
      ctx.font = "bold 10px ui-monospace, monospace";
      ctx.fillText("HEAD", Math.max(MIN_X, x - rDot - 30), y - 10);
    }

    // Hover ring.
    if (state.hovered === node.hash) {
      ctx.strokeStyle = C.fg;
      ctx.lineWidth = 1;
      ctx.beginPath();
      ctx.arc(x, y, rDot + 2, 0, Math.PI * 2);
      ctx.stroke();
    }

    // The text block: subject + author.
    const pills = refPills(node);
    const overflow = node.refs.length > MAX_REF_PILLS ? 5 : 0;
    const pillBand = Math.min(
      pills.reduce((total, pill) => total + pill.width + 6, 0) + overflow * 26,
      240,
    );
    const maxChars = textBudget(pillBand);
    const subject = truncate(node.subject, maxChars);
    ctx.fillStyle = C.fg;
    ctx.font = "12px -apple-system, 'Segoe UI', sans-serif";
    ctx.fillText(subject, tx, y - 4);
    ctx.fillStyle = C.muted;
    ctx.font = "11px -apple-system, 'Segoe UI', sans-serif";
    ctx.fillText(truncate(node.author, maxChars), tx, y + 11);

    // The right-aligned meta column: relative time over `+ins −−dels`.
    // Nothing is drawn for a row whose stats have not arrived, so a row never
    // flickers from a placeholder into its counts. The column is inset past the
    // minimap so it is never drawn underneath it.
    const metaFont = "11px ui-monospace, monospace";
    const time = relativeTime(node.date);
    ctx.font = metaFont;
    const label = statLabel(stat);
    const addText = label ? `+${stat!.insertions}` : "";
    const delText = label ? `−${stat!.deletions}` : "";
    const labelW = label ? ctx.measureText(`${addText}  ${delText}`).width + 6 : 0;
    const timeW = ctx.measureText(time).width;
    const metaX = Math.max(tx, worldW - rightPad - Math.max(timeW, labelW));
    ctx.fillStyle = C.muted;
    ctx.fillText(time, metaX, y - 4);
    if (label) {
      ctx.fillStyle = inkAdd;
      ctx.fillText(addText, metaX, y + 11);
      ctx.fillStyle = inkDel;
      ctx.fillText(delText, metaX + ctx.measureText(`${addText} `).width, y + 11);
    }

    // Ref pills inline, right after the subject - the space they need was reserved when the
    // subject was truncated, and nothing is ever drawn past the right edge of the canvas.
    let pillX = tx + subject.length * 6.2 + 12;
    let drawn = 0;
    for (const { ref, width: pillW } of pills) {
      if (pillX + pillW > worldW - rightPad) {
        break;
      }
      const color = paletteColor(ref.color, C.palette);
      if (ref.kind === "Worktree") {
        // Worktree checkouts (SPEC §10.7) read as outlined, not filled.
        ctx.strokeStyle = color;
        ctx.lineWidth = 1;
        ctx.strokeRect(pillX, y - 15, pillW, 12);
        ctx.fillStyle = color;
      } else {
        ctx.fillStyle = color;
        ctx.fillRect(pillX, y - 15, pillW, 12);
        ctx.fillStyle = C.successInk;
      }
      ctx.font = "9px ui-monospace, monospace";
      ctx.fillText(ref.name, pillX + 4, y - 6);
      pillX += pillW + 6;
      drawn += 1;
    }
    if (node.refs.length > drawn && pillX + 26 <= worldW - rightPad) {
      ctx.fillStyle = C.muted;
      ctx.font = "9px ui-monospace, monospace";
      ctx.fillText(`+${node.refs.length - drawn}`, pillX, y - 6);
    }
  }
  ctx.globalAlpha = 1;
}

/**
 * Draw the minimap: every laid-out row as one pixel-tall band, the visible window
 * marked, the selected row highlighted. One pass over the rows regardless of graph
 * size, and it runs only when the layout or the viewport changes.
 */
export function renderMinimap(
  ctx: CanvasCtx,
  width: number,
  height: number,
  viewport: Viewport,
  state: RenderState,
  dpr: number,
  colors: GraphColors = DARK_GRAPH_COLORS,
): void {
  const C = colors;
  ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
  ctx.clearRect(0, 0, width, height);
  const graph = state.graph;
  if (!graph || graph.nodes.length === 0) {
    return;
  }
  const map = minimapMap(graph, width, height);

  // Cost budget: one rect per (lane, contiguous colour run) plus the window rect
  // and one selection bar - far below the main pass, and it only runs when the
  // layout or the viewport changed. A linear 10 000-commit history costs 2 rects.
  ctx.globalAlpha = 0.75;
  const runs: { lane: number; color: number; from: number; to: number }[] = [];
  for (const node of graph.nodes) {
    const color = graph.lines[node.lines[0]]?.color ?? 0;
    const open = runs.find(
      (r) => r.lane === node.lane && r.color === color && r.to === node.row - 1,
    );
    if (open) {
      open.to = node.row;
    } else {
      runs.push({ lane: node.lane, color, from: node.row, to: node.row });
    }
  }
  for (const run of runs) {
    ctx.fillStyle = paletteColor(run.color, C.palette);
    ctx.fillRect(map.x(run.lane), map.y(run.from), 3, map.y(run.to) - map.y(run.from) + 1);
  }
  ctx.globalAlpha = 1;

  const [first, last] = visibleRowRange(graph.nodes.length, viewport, height);
  ctx.strokeStyle = C.accent;
  ctx.lineWidth = 1;
  ctx.strokeRect(
    2,
    Math.max(2, map.y(first) - 2),
    width - 4,
    Math.max(3, map.y(last) - map.y(first) + 4),
  );

  if (state.selected) {
    const sel = graph.nodes.find((n) => n.hash === state.selected);
    if (sel) {
      ctx.fillStyle = C.accent;
      ctx.fillRect(2, map.y(sel.row), width - 4, 2);
    }
  }
}

/**
 * The minimap's row/lane → pixel mapping. Shared by the painter and the click
 * handler so a click lands on exactly the row the dot marks.
 */
export function minimapMap(graph: Graph, width: number, height: number) {
  const rows = graph.nodes.length;
  const worldH = rowY(rows - 1) + ROW_H / 2;
  const scale = Math.min(1, (height - 4) / worldH);
  const laneCount = Math.max(1, ...graph.nodes.map((n) => n.lane + 1));
  return {
    scale,
    y: (row: number) => 2 + rowY(row) * scale,
    x: (lane: number) =>
      laneCount > 1 ? 5 + lane * ((width - 10) / (laneCount - 1)) : (width - 6) / 2,
  };
}

/** The laid-out row a minimap point refers to. */
export function minimapRowAt(graph: Graph, height: number, y: number): number {
  if (graph.nodes.length === 0) {
    return 0;
  }
  const { scale } = minimapMap(graph, 1, height);
  const row = Math.round(((y - 2) / scale - PAD) / ROW_H - 0.5);
  return Math.max(0, Math.min(graph.nodes.length - 1, row));
}

/** Trim a string to `max` characters with an ellipsis. */
function truncate(s: string, max: number): string {
  return s.length <= max ? s : s.slice(0, Math.max(1, max - 1)) + "…";
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
 * operations), search-driven dimming, a diff-weighted meta column, an ancestry
 * highlight, a minimap, and full keyboard operation.
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
    commitStats,
    statsRepo,
    requestCommitStats,
  } = useGraphStore();
  const bisect = useBisectStore((s) => s.bisect);
  const theme = useThemeStore((s) => s.resolved);
  const { requestPreview, running } = useActivityStore();

  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const minimapRef = useRef<HTMLCanvasElement | null>(null);
  const viewportRef = useRef<Viewport>({ scale: 1, ox: 0, oy: 0 });
  const dirtyRef = useRef(true);
  const minimapDirtyRef = useRef(true);
  const sizeRef = useRef({ width: 0, height: 0 });
  const minimapSizeRef = useRef({ width: 0, height: 0 });
  const pointerPos = useRef({ x: 0, y: 0 });
  const dragRef = useRef<{ x: number; y: number; ox: number; oy: number; moved: boolean } | null>(null);
  const minimapDragRef = useRef(false);
  const [zoom, setZoom] = useState(1);
  const [announce, setAnnounce] = useState("");
  const [menu, setMenu] = useState<ContextMenuState | null>(null);

  const lanes = useMemo(
    () => (graph ? Math.max(1, ...graph.nodes.map((n) => n.lane + 1)) : 1),
    [graph],
  );
  const nodesCount = graph?.nodes.length ?? 0;
  const head = useMemo(() => graph?.nodes.find((n) => n.is_head) ?? null, [graph]);
  const colors = useMemo(() => resolveGraphColors(), [theme]);
  const legend = useMemo(() => graphLegend(graph, colors.palette), [graph, colors.palette]);
  // The canvas element only exists in the drawn body state. Effects that
  // capture it must re-run when it appears/disappears (the component mounts
  // with no repo, so `canvasRef` is `null` until the first graph loads).
  const hasCanvas = !error && !loading && !!graph && graph.nodes.length > 0;

  // Keep the latest render inputs in a ref for the rAF loop.
  const statsVisible = statsRepo === activeId ? commitStats : null;
  const stateRef = useRef<RenderState>({
    graph,
    selected,
    hovered,
    query,
    bisect,
    stats: statsVisible,
    statInk: { add: colors.success, del: colors.error },
    minimapWidth: hasCanvas ? MINIMAP_W : 0,
  });
  stateRef.current = {
    graph,
    selected,
    hovered,
    query,
    bisect,
    stats: statsVisible,
    statInk: { add: colors.success, del: colors.error },
    minimapWidth: hasCanvas ? MINIMAP_W : 0,
  };

  // Redraw when anything renderable changes.
  useEffect(() => {
    dirtyRef.current = true;
    minimapDirtyRef.current = true;
  }, [graph, selected, hovered, query, bisect, nodesCount, menu, theme, statsVisible]);

  // Size tracking (device pixels) via ResizeObserver.
  useEffect(() => {
    const canvas = canvasRef.current;
    // jsdom has no ResizeObserver; skip sizing in non-browser test envs.
    if (!canvas || typeof ResizeObserver === "undefined") {
      return;
    }
    // Size the bitmap to the canvas' own box - the slot the flex layout gives it, not the
    // section's height. Measuring the section made the bitmap taller than the canvas, which
    // stretched the drawing over the toolbar and mapped pointer coordinates (measured against
    // the canvas box) onto the wrong rows.
    const ro = new ResizeObserver(() => {
      const rect = canvas.getBoundingClientRect();
      const dpr = window.devicePixelRatio || 1;
      sizeRef.current = { width: rect.width, height: rect.height };
      canvas.width = Math.max(1, Math.round(rect.width * dpr));
      canvas.height = Math.max(1, Math.round(rect.height * dpr));
      const mini = minimapRef.current;
      if (mini) {
        const miniRect = mini.getBoundingClientRect();
        minimapSizeRef.current = { width: miniRect.width, height: miniRect.height };
        mini.width = Math.max(1, Math.round(miniRect.width * dpr));
        mini.height = Math.max(1, Math.round(miniRect.height * dpr));
      }
      dirtyRef.current = true;
      minimapDirtyRef.current = true;
    });
    ro.observe(canvas);
    return () => ro.disconnect();
  }, [hasCanvas]);

  // The rAF render loop: draw only when dirty. The minimap is a separate, much
  // cheaper pass and only re-runs when the layout or the viewport moved.
  useEffect(() => {
    let raf = 0;
    const draw = () => {
      raf = requestAnimationFrame(draw);
      const canvas = canvasRef.current;
      const ctx = canvas?.getContext("2d");
      if (!ctx) {
        return;
      }
      const dpr = window.devicePixelRatio || 1;
      if (dirtyRef.current) {
        dirtyRef.current = false;
        renderGraph(
          ctx,
          sizeRef.current.width,
          sizeRef.current.height,
          viewportRef.current,
          stateRef.current,
          dpr,
          resolveGraphColors(),
        );
      }
      const mini = minimapRef.current;
      const miniCtx = mini?.getContext("2d");
      if (minimapDirtyRef.current && mini && miniCtx) {
        minimapDirtyRef.current = false;
        renderMinimap(
          miniCtx,
          minimapSizeRef.current.width,
          minimapSizeRef.current.height,
          viewportRef.current,
          stateRef.current,
          dpr,
          resolveGraphColors(),
        );
      }
    };
    raf = requestAnimationFrame(draw);
    return () => cancelAnimationFrame(raf);
  }, []);

  const mark = useCallback(() => {
    dirtyRef.current = true;
    minimapDirtyRef.current = true;
  }, []);

  const setScale = useCallback(
    (next: number, anchor?: { x: number; y: number }) => {
      const v = viewportRef.current;
      const scale = Math.min(MAX_SCALE, Math.max(MIN_SCALE, next));
      if (scale === v.scale) {
        return;
      }
      const ax = anchor?.x ?? 0;
      const ay = anchor?.y ?? 0;
      // Keep the world point at the anchor fixed while the scale changes.
      v.ox = ax - ((ax - v.ox) / v.scale) * scale;
      v.oy = ay - ((ay - v.oy) / v.scale) * scale;
      v.scale = scale;
      setZoom(scale);
      mark();
    },
    [mark],
  );

  const toLocal = (e: { clientX: number; clientY: number }) => {
    const rect = canvasRef.current?.getBoundingClientRect();
    return { x: e.clientX - (rect?.left ?? 0), y: e.clientY - (rect?.top ?? 0) };
  };

  /** Scroll so `row` sits near the top of the canvas. */
  const scrollRowIntoView = useCallback(
    (row: number, keepSelectionVisible = false) => {
      const v = viewportRef.current;
      const y = rowY(row) * v.scale;
      const h = sizeRef.current.height;
      const top = PAD * v.scale;
      const currentTop = -v.oy;
      const currentBottom = currentTop + h;
      if (keepSelectionVisible && y - top >= currentTop && y - top <= currentBottom - ROW_H * v.scale) {
        return;
      }
      v.oy = -(y - top);
      mark();
    },
    [mark],
  );

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
      const rect = canvas.getBoundingClientRect();
      setScale(v.scale * factor, {
        x: e.clientX - rect.left,
        y: e.clientY - rect.top,
      });
    };
    canvas.addEventListener("wheel", onWheel, { passive: false });
    return () => canvas.removeEventListener("wheel", onWheel);
  }, [hasCanvas, setScale]);

  // Stats for the visible rows only (SPEC §10.1): debounced so a pan/scroll burst
  // costs one batch, capped at 200 hashes per request, cached in the graph store
  // keyed by repo + oid so panning back never refetches.
  useEffect(() => {
    if (!activeId || !graph || graph.nodes.length === 0) {
      return;
    }
    const timer = setTimeout(() => {
      const v = viewportRef.current;
      const [first, last] = visibleRowRange(graph.nodes.length, v, sizeRef.current.height);
      const cached =
        statsRepo === activeId ? useGraphStore.getState().commitStats : null;
      const missing: string[] = [];
      for (let r = first; r <= last; r++) {
        const node = graph.nodes[r];
        if (!node || (cached && node.hash in cached)) {
          continue;
        }
        missing.push(node.hash);
      }
      if (missing.length > 0) {
        requestCommitStats(activeId, missing);
      }
    }, 180);
    return () => clearTimeout(timer);
  }, [activeId, graph, zoom, selected, statsRepo, requestCommitStats]);

  const moveSelection = useCallback(
    (delta: number) => {
      if (!graph || graph.nodes.length === 0) {
        return;
      }
      const current = graph.nodes.find((n) => n.hash === selected)?.row;
      const row =
        current === undefined
          ? delta > 0
            ? 0
            : graph.nodes.length - 1
          : Math.max(0, Math.min(graph.nodes.length - 1, current + delta));
      const node = graph.nodes.find((n) => n.row === row);
      if (!node) {
        return;
      }
      select(node.hash);
      setAnnounce(`${node.hash.slice(0, 12)} ${node.subject}`);
      scrollRowIntoView(row, true);
    },
    [graph, selected, select, scrollRowIntoView],
  );

  const jumpToRow = useCallback(
    (row: number) => {
      if (!graph || graph.nodes.length === 0) {
        return;
      }
      const clamped = Math.max(0, Math.min(graph.nodes.length - 1, row));
      const node = graph.nodes.find((n) => n.row === clamped);
      if (!node) {
        return;
      }
      select(node.hash);
      setAnnounce(`${node.hash.slice(0, 12)} ${node.subject}`);
      scrollRowIntoView(clamped, false);
    },
    [graph, select, scrollRowIntoView],
  );

  const panRows = useCallback(
    (rows: number) => {
      const v = viewportRef.current;
      v.oy -= rows * ROW_H * v.scale;
      mark();
    },
    [mark],
  );

  const onKeyDown = (e: ReactKeyboardEvent) => {
    if (e.key === "ArrowDown") {
      e.preventDefault();
      moveSelection(1);
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      moveSelection(-1);
    } else if (e.key === "Home") {
      e.preventDefault();
      jumpToRow(0);
    } else if (e.key === "End") {
      e.preventDefault();
      jumpToRow(graph ? graph.nodes.length - 1 : 0);
    } else if (e.key === "PageDown") {
      e.preventDefault();
      panRows(Math.max(1, Math.floor(sizeRef.current.height / (ROW_H * viewportRef.current.scale))));
    } else if (e.key === "PageUp") {
      e.preventDefault();
      panRows(-Math.max(1, Math.floor(sizeRef.current.height / (ROW_H * viewportRef.current.scale))));
    } else if (e.key === "+" || e.key === "=") {
      e.preventDefault();
      setScale(viewportRef.current.scale * ZOOM_STEP, {
        x: sizeRef.current.width / 2,
        y: sizeRef.current.height / 2,
      });
    } else if (e.key === "-" || e.key === "_") {
      e.preventDefault();
      setScale(viewportRef.current.scale / ZOOM_STEP, {
        x: sizeRef.current.width / 2,
        y: sizeRef.current.height / 2,
      });
    } else if (e.key === "0") {
      e.preventDefault();
      viewportRef.current = { scale: 1, ox: 0, oy: 0 };
      setZoom(1);
      mark();
    } else if (e.key === "Enter") {
      // Exactly what a click on the selected row does: select → inspect drawer.
      e.preventDefault();
      if (selected) {
        select(selected);
      }
    }
  };

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
      minimapDirtyRef.current = true;
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
    if (hash) {
      const node = graph?.nodes.find((n) => n.hash === hash);
      if (node) {
        setAnnounce(`${node.hash.slice(0, 12)} ${node.subject}`);
      }
    }
  };

  const onPointerLeave = () => {
    if (hovered !== null) {
      hover(null);
      mark();
    }
  };

  // The app-level Escape binding closes an open menu by dispatching an off-canvas
  // `contextmenu` event on the canvas: `hitTest` then finds nothing and the menu
  // clears. Do not "optimise" that path away.
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

  // Minimap: click or drag to move the visible window.
  const minimapToPoint = (e: { clientX: number; clientY: number }) => {
    const rect = minimapRef.current?.getBoundingClientRect();
    return { x: e.clientX - (rect?.left ?? 0), y: e.clientY - (rect?.top ?? 0) };
  };
  const minimapScrollTo = (point: { x: number; y: number }) => {
    if (!graph) {
      return;
    }
    const row = minimapRowAt(graph, minimapSizeRef.current.height, point.y);
    const v = viewportRef.current;
    v.oy = -(rowY(row) * v.scale - PAD * v.scale);
    mark();
  };
  const onMinimapPointerDown = (e: ReactPointerEvent) => {
    if (e.button !== 0) {
      return;
    }
    minimapDragRef.current = true;
    (e.target as Element).setPointerCapture?.(e.pointerId);
    minimapScrollTo(minimapToPoint(e));
  };
  const onMinimapPointerMove = (e: ReactPointerEvent) => {
    if (minimapDragRef.current) {
      minimapScrollTo(minimapToPoint(e));
    }
  };
  const onMinimapPointerUp = () => {
    minimapDragRef.current = false;
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
  const hoveredStat = hoveredNode ? statsVisible?.[hoveredNode.hash] ?? null : null;

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
          tabIndex={0}
          role="img"
          aria-label={`${activeId ?? "Repository"}: ${graph.nodes.length} commits shown as a commit graph. Use arrow keys to move the selection, Enter to open the commit.`}
          onPointerDown={onPointerDown}
          onPointerMove={onPointerMove}
          onPointerUp={onPointerUp}
          onPointerLeave={onPointerLeave}
          onKeyDown={onKeyDown}
          onDoubleClick={() => {
            viewportRef.current = { scale: 1, ox: 0, oy: 0 };
            setZoom(1);
            mark();
          }}
          onContextMenu={onContextMenu}
        />
        <canvas
          ref={minimapRef}
          className="graph-minimap"
          aria-hidden="true"
          onPointerDown={onMinimapPointerDown}
          onPointerMove={onMinimapPointerMove}
          onPointerUp={onMinimapPointerUp}
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
              {hoveredNode.author} · {relativeTime(hoveredNode.date)}
            </span>
            {hoveredStat && (
              <>
                {" "}
                <span className="stat-add">+{hoveredStat.insertions}</span>{" "}
                <span className="stat-del">−{hoveredStat.deletions}</span>
              </>
            )}
          </div>
        )}
      </>
    );
  }

  return (
    <section className="graph">
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
        <span className="graph-zoom-controls">
          <button
            type="button"
            className="graph-zoom-btn"
            aria-label="zoom out"
            title="Zoom out"
            disabled={!graph || graph.nodes.length === 0}
            onClick={() =>
              setScale(viewportRef.current.scale / ZOOM_STEP, {
                x: sizeRef.current.width / 2,
                y: sizeRef.current.height / 2,
              })
            }
          >
            <Icon name="minus" size={14} label="zoom out" />
          </button>
          <span className="graph-zoom-readout" aria-live="off">
            {Math.round(zoom * 100)}%
          </span>
          <button
            type="button"
            className="graph-zoom-btn"
            aria-label="zoom in"
            title="Zoom in"
            disabled={!graph || graph.nodes.length === 0}
            onClick={() =>
              setScale(viewportRef.current.scale * ZOOM_STEP, {
                x: sizeRef.current.width / 2,
                y: sizeRef.current.height / 2,
              })
            }
          >
            <Icon name="plus" size={14} label="zoom in" />
          </button>
          <button
            type="button"
            className="graph-zoom-btn"
            aria-label="100%"
            title="Reset zoom to 100%"
            disabled={!graph || graph.nodes.length === 0}
            onClick={() => {
              const v = viewportRef.current;
              const centre = { x: sizeRef.current.width / 2, y: sizeRef.current.height / 2 };
              setScale(1, centre);
              if (v.scale === 1) {
                mark();
              }
            }}
          >
            <Icon name="check" size={14} label="100%" />
          </button>
          <button
            type="button"
            className="graph-zoom-btn"
            aria-label="Fit"
            title="Fit the whole history"
            disabled={!graph || graph.nodes.length === 0}
            onClick={() => {
              const h = sizeRef.current.height || 1;
              const worldH = rowY(nodesCount - 1) + ROW_H / 2;
              const scale = Math.max(
                MIN_SCALE,
                Math.min(MAX_SCALE, (h - 8) / worldH),
              );
              viewportRef.current = { scale, ox: 0, oy: 0 };
              setZoom(scale);
              mark();
            }}
          >
            <Icon name="search" size={14} label="Fit" />
          </button>
        </span>
        {graph && graph.nodes.length > 0 && (
          <span className="graph-count">{graph.nodes.length} commits</span>
        )}
        {legend.length > 0 && (
          <span className="graph-legend" aria-label="Branch lanes">
            {legend.slice(0, MAX_LEGEND_LANES).map((item) => (
              <span key={item.lane} className="graph-legend-chip" title={item.name}>
                <span
                  className="graph-legend-swatch"
                  style={{ background: item.color }}
                />
                {truncate(item.name, 14)}
              </span>
            ))}
            {legendOverflow(legend.length, MAX_LEGEND_LANES) && (
              <span className="graph-legend-more">
                {legendOverflow(legend.length, MAX_LEGEND_LANES)}
              </span>
            )}
          </span>
        )}
      </div>
      {body}
      {/* Announcement-only live region: `aria-live` alone, so the app keeps exactly
          one `role="status"` region (the conflict banner). */}
      <span className="graph-sr-only" aria-live="polite" aria-label="Selected commit">
        {announce}
      </span>
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
            label="bisect from here"
            onClick={() => {
              useBisectStore.getState().setDraftBad(menuNode.hash);
              useMainTab.getState().setTab("bisect");
              setMenu(null);
            }}
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