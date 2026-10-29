//! Lane layout: the GitGotten graph algorithm (SPEC §10.1).
//!
//! The layout is a faithful port of `git log --graph`, verified against git's
//! actual output (git 2.54, `graph.c` + `revision.c`'s topo walk):
//!
//! - **Row order** (`--graph` implies `--topo-order`): a commit is emitted only
//!   after *all* of its children have been; the pending commits form a LIFO
//!   stack, so the most recently freed commit goes next (a merge's last parent
//!   is emitted first, then its first-parent chain). The initial tips are
//!   pushed in ascending ref-name order, which the LIFO pop reverses into
//!   git's reverse-lexicographic ref order (unmerged branches, newest ref name
//!   first).
//! - **Columns** (lanes) are dense, left to right. A column's *frontier* is the
//!   next commit of that line. When a commit is emitted, the row is rebuilt
//!   left to right, exactly as git's `graph.c` (`graph_update_columns`): every
//!   column's frontier — or the emitted commit's parents, in parent order, in
//!   place of the commit's own column — is reinserted, and a commit that
//!   already holds a rebuilt position is deduplicated (its *first* appearance
//!   in the rebuild wins). Consequences:
//!   - a commit that is not yet a frontier (a tip) is appended as a new column
//!     on the right, opening a **new line** (a line with a fresh color);
//!   - the emitted commit's own column dies — its line ends at the commit;
//!   - a first parent in a column to the commit's *right* *moves* into the
//!     commit's slot: its line continues in the commit's lane;
//!   - a first parent in a column to the *left* stays in its own column (the
//!     commit's edge simply ends at the commit);
//!   - a first parent with no column inherits the commit's column *and line*
//!     (the first-parent chain continues);
//!   - each additional parent (a merge's 2nd+) follows the same rule in
//!     parent order — a column to the right moves into the slot, a column to
//!     the left stays put, and a parent with no column opens a **new line**
//!     in the slot, immediately right of the inherited first parent.
//!   The rebuilt list is dense, so surviving columns keep their order or shift
//!   left — never right. A line therefore never ends up left of the line it
//!   branched from, and a merged branch's lane dies where its line converges;
//!   the graph keeps the familiar "mainline straight on the left" shape.

use std::collections::{HashMap, HashSet};

use git_gotten_protocol::{BRANCH_PALETTE, Edge, Graph, Line, LineId, LineSegment, Node, RefKind};

use crate::{GraphError, RawDag};

/// A line being laid out: its color, display name, and the geometry accumulated
/// so far (segments are completed when the line dies or moves lanes).
#[derive(Debug)]
struct LineRec {
    /// Palette index (the final value comes from the interval recolor pass).
    color: u32,
    /// Branch/remote name at the line's opener, if one exists.
    name: Option<String>,
    /// Lane geometry, in row order: (lane, from_row, to_row), inclusive.
    segments: Vec<(i32, u32, u32)>,
    /// Last row the line held a column; `u32::MAX` until first placed. A
    /// line's live span is a contiguous row interval, so the stamp drives
    /// the segment bookkeeping without a per-row scan of every line.
    last_row: u32,
}

/// The lane layout engine (SPEC §10.1). Stateless; one layout per call.
#[derive(Debug, Default, Clone, Copy)]
pub struct LaneLayout;

impl LaneLayout {
    /// Compute the full layout for a raw DAG, newest-first.
    ///
    /// The input node order is irrelevant (the layout is deterministic under
    /// shuffles — see the tests). An empty DAG yields [`GraphError::Empty`].
    pub fn compute(&mut self, dag: &RawDag) -> Result<Graph, GraphError> {
        let n = dag.nodes.len();
        if n == 0 {
            return Err(GraphError::Empty);
        }

        // Hash → dense index (and duplicate validation).
        let mut hash_to_idx: HashMap<&str, u32> = HashMap::with_capacity(n);
        for (i, node) in dag.nodes.iter().enumerate() {
            if let Some(_first) = hash_to_idx.insert(node.hash.as_str(), i as u32) {
                return Err(GraphError::DuplicateHash(node.hash.clone()));
            }
        }

        // Parent edges as dense indices (validated against the DAG contents),
        // plus each node's child count (how many nodes list it as a parent).
        let mut parents: Vec<Vec<u32>> = Vec::with_capacity(n);
        let mut child_count = vec![0u32; n];
        for node in &dag.nodes {
            let mut ps = Vec::with_capacity(node.parents.len());
            for p in &node.parents {
                let pi = *hash_to_idx
                    .get(p.as_str())
                    .ok_or_else(|| GraphError::MissingParent(p.clone()))?;
                child_count[pi as usize] += 1;
                ps.push(pi);
            }
            parents.push(ps);
        }

        // ── Row order: git's `--topo-order` walk (what `--graph` implies). ──
        // A commit is *ready* once all of its children have been emitted.
        // Git emits initial tips in commit-date descending order; equal dates
        // keep the ref-iteration (alphabetical) position from the pending
        // list, i.e. a multi-ref commit sorts by its alphabetically smallest
        // ref name. Freed parents are then stacked LIFO (last parent freed
        // first). Since this walk pops from the back of the stack, the tips
        // are pushed in *reverse* output order: date ascending, ref name
        // descending, hash descending. Tips without refs (synthetic DAGs
        // only) sort first, most recent date last.
        let mut tips: Vec<u32> = (0..n as u32)
            .filter(|i| child_count[*i as usize] == 0)
            .collect();
        tips.sort_by(|a, b| {
            let key = |i: &u32| -> (u64, std::cmp::Reverse<&str>, std::cmp::Reverse<&str>) {
                let node = &dag.nodes[*i as usize];
                let ref_name = node
                    .refs
                    .iter()
                    .map(|r| r.name.as_str())
                    .min()
                    .unwrap_or("");
                (
                    node.sort_date,
                    std::cmp::Reverse(ref_name),
                    std::cmp::Reverse(node.hash.as_str()),
                )
            };
            key(a).cmp(&key(b))
        });
        let mut stack: Vec<u32> = tips;
        let mut emitted = vec![false; n];
        let mut row_of = vec![u32::MAX; n];
        let mut order: Vec<u32> = Vec::with_capacity(n);
        while let Some(c) = stack.pop() {
            if emitted[c as usize] {
                continue;
            }
            emitted[c as usize] = true;
            row_of[c as usize] = order.len() as u32;
            order.push(c);
            for p in &parents[c as usize] {
                // Parents are freed in order, so the last parent lands on top
                // of the stack and is emitted first.
                if child_count[*p as usize] > 0 {
                    child_count[*p as usize] -= 1;
                    if child_count[*p as usize] == 0 {
                        stack.push(*p);
                    }
                }
            }
        }
        // Every node of a ref-built DAG is a descendant of a tip; anything
        // left unemitted means the input had a cycle.
        let first_unemitted = emitted
            .iter()
            .position(|e| !e)
            .map(|i| dag.nodes[i].hash.clone());
        if let Some(hash) = first_unemitted {
            return Err(GraphError::CyclicGraph(hash));
        }

        // ── Lanes: the column model, row by row (newest first). ────────────
        let mut columns: Vec<(u32, u32)> = Vec::new(); // (frontier node, line id)
        let mut lines: Vec<LineRec> = Vec::new();
        let mut color_counter: u32 = 0;
        let open_line =
            |lines: &mut Vec<LineRec>, color_counter: &mut u32, opener: u32, dag: &RawDag| -> u32 {
                let name = dag.nodes[opener as usize]
                    .refs
                    .iter()
                    .filter(|r| matches!(r.kind, RefKind::Branch | RefKind::Remote))
                    .map(|r| r.name.clone())
                    .min();
                let id = lines.len() as u32;
                lines.push(LineRec {
                    color: *color_counter,
                    name,
                    segments: Vec::new(),
                    last_row: u32::MAX,
                });
                *color_counter += 1;
                id
            };

        let mut lane_of = vec![0i32; n];
        let mut line_of = vec![0u32; n];
        let mut node_lines: Vec<Vec<LineId>> = vec![Vec::new(); n];

        for (r, &ci) in order.iter().enumerate() {
            let r = r as u32;
            // 1. Place the commit: its column, or a new one on the right.
            let col_pos = columns.iter().position(|(f, _)| *f == ci);
            let (lane, own_line) = match col_pos {
                Some(pos) => (pos as i32, columns[pos].1),
                None => {
                    let line = open_line(&mut lines, &mut color_counter, ci, dag);
                    columns.push((ci, line));
                    (columns.len() as i32 - 1, line)
                }
            };
            lane_of[ci as usize] = lane;
            line_of[ci as usize] = own_line;
            node_lines[ci as usize] = columns.iter().map(|(_, l)| *l).collect();

            // 2. Segment bookkeeping: touch only the lines that hold a column
            // this row (O(cols), not O(lines)); the `last_row` stamp records
            // liveness, so untouched lines simply stop (a line's live span is
            // one contiguous row interval).
            for (pos, (_, l)) in columns.iter().enumerate() {
                let rec = &mut lines[*l as usize];
                let shifted = !matches!(rec.segments.last(), Some(s) if s.0 == pos as i32);
                if rec.last_row == r.saturating_sub(1) && !shifted {
                    if let Some(seg) = rec.segments.last_mut() {
                        seg.2 = r; // same lane: extend the segment
                    }
                } else {
                    // New line, or lane shifted by compaction: new segment.
                    rec.segments.push((pos as i32, r, r));
                }
                rec.last_row = r;
            }

            // 3. Build the next row's columns — git graph.c
            //    (`graph_update_columns`): the row is rebuilt left to right and
            //    every frontier — or, where the commit's column was, the
            //    commit's parents in parent order — is reinserted, deduplicated
            //    by commit (first appearance in the rebuild wins its
            //    position). The commit's own column dies; a parent in a column
            //    to the commit's right moves into the slot (its line continues
            //    in the commit's lane); a parent to the left stays in its own
            //    column; a parent with no column takes the slot (a first
            //    parent inherits the commit's line, an additional one opens a
            //    new line). The rebuilt list is dense: survivors keep their
            //    order or shift left, never right.
            let ci_pos = columns.iter().position(|(f, _)| *f == ci).unwrap();
            let p0 = parents[ci as usize].first().copied();
            let p0_pos = p0.and_then(|p0| columns.iter().position(|(f, _)| *f == p0));
            let mut next: Vec<(u32, u32)> = Vec::with_capacity(columns.len() + 4);
            let mut seen: HashSet<u32> = HashSet::with_capacity(next.capacity());
            for (pos, &(frontier, line)) in columns.iter().enumerate() {
                if pos == ci_pos {
                    match p0 {
                        None => {} // root — the column dies here
                        Some(p0) => match p0_pos {
                            // Right of the slot: the parent's column moves
                            // into the commit's slot; its line continues in
                            // the commit's lane, the commit's own line ends
                            // at the commit.
                            Some(j) if j > ci_pos => {
                                next.push(columns[j]);
                                seen.insert(frontier);
                                seen.insert(p0);
                            }
                            // Left of the slot: the parent keeps its column
                            // (carried at its own position below); the
                            // commit's edge ends at the commit.
                            Some(_) => {}
                            // No column yet: the parent inherits the commit's
                            // column and line.
                            None => {
                                next.push((p0, line));
                                seen.insert(frontier);
                            }
                        },
                    }
                    for &p in parents[ci as usize].iter().skip(1) {
                        match columns.iter().position(|(f, _)| *f == p) {
                            // Right of the slot: moves into it, in parent
                            // order, keeping its own line.
                            Some(j) if j > ci_pos => {
                                next.push(columns[j]);
                                seen.insert(p);
                            }
                            // Left of the slot: stays put (carried below).
                            Some(_) => {}
                            // No column: a new line in the slot, right of the
                            // inherited first parent.
                            None => {
                                if seen.insert(p) {
                                    let l = open_line(&mut lines, &mut color_counter, p, dag);
                                    next.push((p, l));
                                }
                            }
                        }
                    }
                } else if seen.insert(frontier) {
                    next.push((frontier, line)); // carried
                }
            }
            columns = next;
        }

        // ── Assemble the output graph. ──────────────────────────────────────
        let g = assemble(
            dag,
            &order,
            &row_of,
            &lane_of,
            &line_of,
            &node_lines,
            &mut lines,
        );
        Ok(g)
    }
}

/// Interval recolor: lines that are simultaneously alive get distinct palette
/// colors (greedy by creation order). `color_last_end[c]` is the last row
/// (inclusive) a line colored `c` is alive; a color is free when that row
/// precedes `from_row`. When the palette is exhausted, the color whose holder
/// ended earliest is reused (documented best-effort beyond the palette size).
fn color_for(from: u32, to: u32, color_last_end: &[i64], preferred: u32) -> (u32, i64) {
    let size = color_last_end.len();
    for c in 0..size {
        if color_last_end[c] < from as i64 {
            return (c as u32, to as i64);
        }
    }
    let mut best = preferred.min(size as u32 - 1);
    let mut best_end = i64::MAX;
    for c in 0..size {
        if color_last_end[c] < best_end {
            best_end = color_last_end[c];
            best = c as u32;
        }
    }
    (best, to as i64)
}

fn assemble(
    dag: &RawDag,
    order: &[u32],
    row_of: &[u32],
    lane_of: &[i32],
    line_of: &[u32],
    node_lines: &[Vec<LineId>],
    lines: &mut Vec<LineRec>,
) -> Graph {
    // Final colors: greedy interval coloring over each line's row span.
    let mut color_last_end = vec![-1i64; BRANCH_PALETTE.len()];
    let mut final_colors = vec![0u32; lines.len()];
    for (l, rec) in lines.iter().enumerate() {
        if let (Some(first), Some(last)) = (rec.segments.first(), rec.segments.last()) {
            let (color, end) = color_for(first.1, last.2, &color_last_end, rec.color);
            color_last_end[color as usize] = end;
            final_colors[l] = color;
        } else {
            final_colors[l] = rec.color;
        }
    }
    for (l, rec) in lines.iter_mut().enumerate() {
        rec.color = final_colors[l];
    }

    let mut out_nodes = Vec::with_capacity(dag.nodes.len());
    for (i, node) in dag.nodes.iter().enumerate() {
        let own = line_of[i];
        let mut refs = node.refs.clone();
        // Ref labels take the color of the line their node sits on.
        for ref_label in refs.iter_mut() {
            ref_label.color = lines[own as usize].color;
        }
        out_nodes.push(Node {
            hash: node.hash.clone(),
            subject: node.subject.clone(),
            author: node.author.clone(),
            date: node.date,
            row: row_of[i],
            lane: lane_of[i],
            is_head: node.is_head,
            refs,
            lines: node_lines[i].clone(),
        });
    }
    out_nodes.sort_by_key(|node| node.row);

    let mut edges = Vec::new();
    for node in &dag.nodes {
        for p in &node.parents {
            edges.push(Edge {
                from: node.hash.clone(),
                to: p.clone(),
            });
        }
    }
    edges.sort();

    let out_lines: Vec<Line> = lines
        .iter()
        .enumerate()
        .map(|(i, rec)| Line {
            id: i as LineId,
            color: rec.color,
            name: rec.name.clone(),
            segments: rec
                .segments
                .iter()
                .map(|(lane, from, to)| LineSegment {
                    lane: *lane,
                    from_row: *from,
                    to_row: *to,
                })
                .collect(),
        })
        .collect();

    let _ = order; // row indices live on the nodes
    Graph {
        nodes: out_nodes,
        edges,
        lines: out_lines,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::RawNode;
    use git_gotten_protocol::{RefKind, RefLabel};

    // ── Test DSL: small DAGs with explicit dates ────────────────────────────

    fn n(hash: &str, sort: u64, parents: &[&str]) -> RawNode {
        RawNode {
            hash: hash.to_string(),
            subject: hash.to_string(),
            author: "Test".into(),
            date: sort,
            sort_date: sort,
            parents: parents.iter().map(|s| s.to_string()).collect(),
            refs: Vec::new(),
            is_head: false,
        }
    }

    fn rref(name: &str, kind: RefKind) -> RefLabel {
        RefLabel {
            name: name.into(),
            kind,
            color: 0,
        }
    }

    fn tag(node: &mut RawNode, name: &str, kind: RefKind) {
        node.refs.push(rref(name, kind));
    }

    fn dag(ns: &[RawNode]) -> RawDag {
        RawDag { nodes: ns.to_vec() }
    }

    fn node<'a>(g: &'a Graph, hash: &str) -> &'a Node {
        g.nodes
            .iter()
            .find(|node| node.hash == hash)
            .unwrap_or_else(|| panic!("node {hash} not in graph"))
    }

    fn row_lane(g: &Graph, hash: &str) -> (u32, i32) {
        let node = node(g, hash);
        (node.row, node.lane)
    }

    fn line_named(g: &Graph, name: &str) -> usize {
        g.lines
            .iter()
            .position(|l| l.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("no line named {name:?} in graph"))
    }

    fn max_lane(g: &Graph) -> i32 {
        g.nodes.iter().map(|node| node.lane).max().unwrap_or(0)
    }

    fn seg(lane: i32, from_row: u32, to_row: u32) -> LineSegment {
        LineSegment {
            lane,
            from_row,
            to_row,
        }
    }

    /// The core layout invariants: no two nodes share a (lane, row) cell,
    /// and no two *lines* occupy the same (lane, row) cell (a line's segments
    /// legitimately pass through its own nodes).
    fn assert_no_overlap(g: &Graph) {
        let mut node_cells = HashSet::new();
        for node in &g.nodes {
            assert!(
                node_cells.insert((node.lane, node.row)),
                "two nodes at (lane {}, row {})",
                node.lane,
                node.row
            );
        }
        let mut line_cells: HashMap<(i32, u32), u32> = HashMap::new();
        for line in &g.lines {
            for seg in &line.segments {
                for row in seg.from_row..=seg.to_row {
                    assert!(
                        line_cells.insert((seg.lane, row), line.id).is_none(),
                        "lines {} and {} overlap at (lane {}, row {})",
                        line_cells[&(seg.lane, row)],
                        line.id,
                        seg.lane,
                        row
                    );
                }
            }
        }
    }

    /// Edge completeness: every raw-DAG parent link appears as a graph edge.
    fn assert_edges_complete(g: &Graph, d: &RawDag) {
        for raw in &d.nodes {
            for p in &raw.parents {
                assert!(
                    g.edges.iter().any(|e| e.from == raw.hash && e.to == *p),
                    "missing edge {} -> {}",
                    raw.hash,
                    p
                );
            }
        }
    }

    /// Structural invariants every layout must satisfy: row coverage,
    /// topological row order, per-row lane uniqueness of live lines, and
    /// live-line color distinctness within a row.
    fn assert_invariants(g: &Graph) {
        assert_no_overlap(g);

        let n = g.nodes.len();
        let mut rows = HashSet::new();
        for node in &g.nodes {
            assert!(rows.insert(node.row), "duplicate row {}", node.row);
        }
        assert_eq!(rows.len(), n);

        for node in &g.nodes {
            for e in g.edges.iter().filter(|e| e.from == node.hash) {
                let p = g.nodes.iter().find(|x| x.hash == e.to).unwrap();
                assert!(
                    p.row > node.row,
                    "parent {} (row {}) not below child {} (row {})",
                    e.to,
                    p.row,
                    node.hash,
                    node.row
                );
            }
        }

        let mut by_row: HashMap<u32, Vec<(i32, u32)>> = HashMap::new();
        for line in &g.lines {
            for seg in &line.segments {
                for row in seg.from_row..=seg.to_row {
                    by_row.entry(row).or_default().push((seg.lane, line.color));
                }
            }
        }
        for (row, live) in &by_row {
            let mut lanes = live.iter().map(|(l, _)| *l).collect::<Vec<_>>();
            lanes.sort();
            for w in lanes.windows(2) {
                assert_ne!(w[0], w[1], "row {row}: two lines share lane {}", w[0]);
            }
            let mut colors = live.iter().map(|(_, c)| *c).collect::<Vec<_>>();
            colors.sort();
            for w in colors.windows(2) {
                assert_ne!(w[0], w[1], "row {row}: two live lines share color {}", w[0]);
            }
        }
    }

    // ── The plan's layout tests ─────────────────────────────────────────────

    #[test]
    fn linear_history_is_a_single_lane_in_row_order() {
        let d = dag(&[n("A", 1, &[]), n("B", 2, &["A"]), n("C", 3, &["B"])]);
        let g = LaneLayout::default().compute(&d).unwrap();
        assert_eq!(row_lane(&g, "C"), (0, 0));
        assert_eq!(row_lane(&g, "B"), (1, 0));
        assert_eq!(row_lane(&g, "A"), (2, 0));
        assert_eq!(max_lane(&g), 0);
        assert_eq!(g.lines.len(), 1);
        assert_eq!(
            g.lines[0].segments,
            vec![LineSegment {
                lane: 0,
                from_row: 0,
                to_row: 2
            }]
        );
        assert_invariants(&g);
    }

    #[test]
    fn branch_and_merge_join_lanes_and_branch_lane_dies() {
        // A ← B ← C (main); B ← D ← E (feature); M merges C and E.
        let mut nodes = vec![
            n("A", 1, &[]),
            n("B", 2, &["A"]),
            n("C", 3, &["B"]),
            n("D", 4, &["B"]),
            n("E", 5, &["D"]),
            n("M", 6, &["C", "E"]),
        ];
        tag(&mut nodes[5], "main", RefKind::Branch); // M
        tag(&mut nodes[4], "feature", RefKind::Branch); // E
        let d = RawDag { nodes };
        let g = LaneLayout::default().compute(&d).unwrap();

        // Row order (git's date-priority DFS): M, E, D, C, B, A.
        assert_eq!(
            g.nodes
                .iter()
                .map(|node| node.hash.as_str())
                .collect::<Vec<_>>(),
            vec!["M", "E", "D", "C", "B", "A"]
        );
        // Lanes: mainline straight on the left, feature in its own lane.
        assert_eq!(row_lane(&g, "M"), (0, 0));
        assert_eq!(row_lane(&g, "E"), (1, 1));
        assert_eq!(row_lane(&g, "D"), (2, 1));
        assert_eq!(row_lane(&g, "C"), (3, 0));
        assert_eq!(row_lane(&g, "B"), (4, 0));
        assert_eq!(row_lane(&g, "A"), (5, 0));

        // Git's edge discipline at the merge: C (row 3, lane 0) takes its
        // first parent B, which holds the feature line in lane 1 — B's column
        // *moves* into C's slot, so the feature line rides the trunk (B, A)
        // down to the root in lane 0, and the mainline line ends at C's row.
        // The branch lane (lane 1) dies at the merge: left-packing closes it.
        let feat = line_named(&g, "feature");
        let main = line_named(&g, "main");
        assert_eq!(g.lines[feat].segments, vec![seg(1, 1, 3), seg(0, 4, 5)]);
        assert_eq!(g.lines[main].segments, vec![seg(0, 0, 3)]);

        assert_eq!(g.edges.iter().filter(|e| e.from == "M").count(), 2);
        assert_edges_complete(&g, &d);
        assert_invariants(&g);
    }

    #[test]
    fn octopus_merge_opens_a_lane_per_additional_parent() {
        // Base A ← B; three branches off B; M merges F (main) + all three tips.
        let d = dag(&[
            n("A", 1, &[]),
            n("B", 2, &["A"]),
            n("C1", 3, &["B"]),
            n("C2", 4, &["C1"]),
            n("D1", 5, &["B"]),
            n("D2", 6, &["D1"]),
            n("E1", 7, &["B"]),
            n("E2", 8, &["E1"]),
            n("F", 9, &["B"]),
            n("M", 10, &["F", "C2", "D2", "E2"]),
        ]);
        let g = LaneLayout::default().compute(&d).unwrap();

        assert_eq!(row_lane(&g, "M"), (0, 0));
        // First parent stays in the merge's lane; additional parents open new
        // lanes to the right, in parent order.
        assert_eq!(node(&g, "F").lane, 0);
        assert_eq!(node(&g, "C2").lane, 1);
        assert_eq!(node(&g, "D2").lane, 2);
        assert_eq!(node(&g, "E2").lane, 3);
        assert_eq!(g.edges.iter().filter(|e| e.from == "M").count(), 4);
        // All branch lines die at the base B, which stays on the mainline lane.
        assert_eq!(node(&g, "B").lane, 0);
        assert_edges_complete(&g, &d);
        assert_invariants(&g);
    }

    #[test]
    fn long_lane_routes_around_interleaved_merges_without_overlapping() {
        // Main: A..G. Long-lived `feat` off B. Two unrelated branches (s1 off
        // C, s2 off D) merge into main while feat is still live.
        let mut nodes = vec![
            n("A", 1, &[]),
            n("B", 2, &["A"]),
            n("C", 3, &["B"]),
            n("D", 4, &["C"]),
            n("E", 5, &["D"]),
            n("F", 6, &["E"]),
            n("G", 7, &["F"]),
            n("f1", 8, &["B"]),
            n("f2", 9, &["f1"]),
            n("f3", 10, &["f2"]),
            n("f4", 11, &["f3"]),
            n("s1a", 12, &["C"]),
            n("s1b", 13, &["s1a"]),
            n("s2a", 14, &["D"]),
            n("M1", 15, &["G", "s1b"]),
            n("M2", 16, &["M1", "s2a"]),
            n("M3", 17, &["M2", "f4"]),
        ];
        tag(
            nodes.iter_mut().find(|x| x.hash == "M3").unwrap(),
            "main",
            RefKind::Branch,
        );
        let feat_ref = nodes.iter_mut().find(|x| x.hash == "f4").unwrap();
        tag(feat_ref, "feat", RefKind::Branch);
        let s1_ref = nodes.iter_mut().find(|x| x.hash == "s1b").unwrap();
        tag(s1_ref, "s1", RefKind::Branch);
        let s2_ref = nodes.iter_mut().find(|x| x.hash == "s2a").unwrap();
        tag(s2_ref, "s2", RefKind::Branch);
        let d = RawDag { nodes };
        let g = LaneLayout::default().compute(&d).unwrap();

        let main = line_named(&g, "main");
        let feat = line_named(&g, "feat");
        let s1 = line_named(&g, "s1");
        let s2 = line_named(&g, "s2");
        // The mainline is one constant lane-0 segment until E's row, where
        // the s2 edge moves in and replaces it. The long-lived feat line is
        // a single line that shifts lanes as lines to its left die
        // (left-packing) — it routes around the interleaved branches and
        // ends back on the mainline lane. No two live lines may ever share a
        // (lane, row) — that is the "route around" requirement, checked by
        // `assert_invariants` below.
        assert_eq!(g.lines[main].segments, vec![seg(0, 0, 12)]);
        assert_eq!(
            g.lines[feat].segments,
            vec![
                seg(1, 1, 5),
                seg(2, 6, 7),
                seg(3, 8, 12),
                seg(2, 13, 13),
                seg(1, 14, 14),
                seg(0, 15, 16)
            ]
        );
        assert_eq!(g.lines[s1].segments, vec![seg(1, 8, 13), seg(0, 14, 14)]);
        assert_eq!(
            g.lines[s2].segments,
            vec![seg(1, 6, 7), seg(2, 8, 12), seg(0, 13, 13)]
        );
        // No live line ever overlaps another (the "route around" requirement).
        assert_edges_complete(&g, &d);
        assert_invariants(&g);
    }

    #[test]
    fn diverged_remote_gets_its_own_lane_and_labels() {
        // Local main: A B C D; origin/main: A B C O (diverged at C). HEAD
        // detached at O.
        let mut nodes = vec![
            n("A", 1, &[]),
            n("B", 2, &["A"]),
            n("C", 3, &["B"]),
            n("D", 4, &["C"]),
            n("O", 5, &["C"]),
        ];
        tag(&mut nodes[3], "main", RefKind::Branch); // D
        nodes[3].is_head = true;
        tag(&mut nodes[4], "origin/main", RefKind::Remote); // O
        let d = RawDag { nodes };
        let g = LaneLayout::default().compute(&d).unwrap();

        // Two tips → two lanes. Git emits tips in reverse ref order, so
        // `origin/main` (O) comes before `main` (D); O takes the leftmost lane.
        assert_eq!(row_lane(&g, "O"), (0, 0));
        // They converge at C, which takes the leftmost lane.
        assert_eq!(node(&g, "C").lane, 0);
        assert!(node(&g, "O").is_head == false);
        assert!(node(&g, "D").is_head);

        // The two lines are distinct; ref labels carry their line's color.
        let o_line = line_named(&g, "origin/main");
        let d_line = line_named(&g, "main");
        assert_ne!(g.lines[o_line].color, g.lines[d_line].color);
        let o = node(&g, "O");
        assert_eq!(o.refs[0].name, "origin/main");
        assert_eq!(o.refs[0].color, g.lines[o_line].color);
        assert_edges_complete(&g, &d);
        assert_invariants(&g);
    }

    /// The plan's "worst case": a 10-commit trunk with five branches of
    /// differing lengths branching at different points — two merged back in
    /// sequence, one more merged, one short-lived, and one unmerged tip
    /// (HEAD) — plus a tag mid-trunk. This is the shape a naive layout
    /// overlaps; the lifetime routing must keep every (lane, row) unique.
    fn kitchen_sink() -> RawDag {
        let mut nodes = Vec::new();
        // Trunk t0..t9, dates 100..900.
        for i in 0..10 {
            let mut node = n(&format!("t{i}"), (i + 1) * 100, &[]);
            if i > 0 {
                node.parents = vec![format!("t{}", i - 1)];
            }
            nodes.push(node);
        }
        let mut add_branch = |base: usize, prefix: &str, len: usize, start: u64| {
            let mut prev = nodes[base].hash.clone();
            for k in 0..len {
                let mut node = n(&format!("{prefix}{k}"), start + k as u64, &[]);
                node.parents = vec![prev.clone()];
                prev = node.hash.clone();
                nodes.push(node);
            }
            prev
        };
        let b1 = add_branch(2, "b1", 3, 250); // off t2
        let b2 = add_branch(4, "b2", 4, 450); // off t4
        let b3_tip = add_branch(6, "b3", 2, 750); // off t6, unmerged (HEAD)
        let b4 = add_branch(3, "b4", 3, 350); // off t3
        // Merges back into the trunk chain: M1 = merge(t5, b1) ... M3.
        let m1 = n("M1", 1000, &["t5", b1.as_str()]);
        nodes.push(m1.clone());
        let m2 = n("M2", 1100, &["M1", b2.as_str()]);
        nodes.push(m2.clone());
        let m3 = n("M3", 1200, &["M2", b4.as_str()]);
        nodes.push(m3.clone());
        // Refs: main at M3; b3 at its tip (HEAD, newest); a tag at t5.
        for node in nodes.iter_mut() {
            if node.hash == "M3" {
                tag(node, "main", RefKind::Branch);
            }
            if node.hash == b3_tip {
                tag(node, "b3", RefKind::Branch);
                node.is_head = true;
            }
            if node.hash == "t5" {
                tag(node, "v1.0", RefKind::Tag);
            }
        }
        RawDag { nodes }
    }

    #[test]
    fn kitchen_sink_keeps_all_lanes_separated() {
        let d = kitchen_sink();
        let g = LaneLayout::default().compute(&d).unwrap();
        assert_invariants(&g);
        assert_edges_complete(&g, &d);
        // The tag label is present, colored by its line, and HEAD is the tip.
        let t5 = node(&g, "t5");
        assert!(
            t5.refs
                .iter()
                .any(|r| r.name == "v1.0" && r.kind == RefKind::Tag)
        );
        let head = g.nodes.iter().find(|x| x.is_head).unwrap();
        assert_eq!(
            head.refs
                .iter()
                .map(|r| r.name.as_str())
                .collect::<Vec<_>>(),
            vec!["b3"]
        );
    }

    #[test]
    fn layout_is_deterministic_under_node_order_shuffle() {
        let d = kitchen_sink();
        let mut layout = LaneLayout::default();
        let baseline = layout.compute(&d).unwrap();
        for seed in [1u64, 7, 42, 1234] {
            let mut perm: Vec<u32> = (0..d.nodes.len() as u32).collect();
            let mut state = seed;
            for i in (1..perm.len()).rev() {
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                let j = (state >> 33) % (i as u64 + 1);
                perm.swap(i, j as usize);
            }
            let shuffled = RawDag {
                nodes: perm.iter().map(|&i| d.nodes[i as usize].clone()).collect(),
            };
            assert_eq!(
                layout.compute(&shuffled).unwrap(),
                baseline,
                "layout depends on input order (seed {seed})"
            );
        }
    }

    #[test]
    fn palette_overflow_is_bounded_and_deterministic() {
        // 25 disjoint two-commit chains (root + child) under one 25-parent
        // octopus merge: the fan leaves all 25 lines alive at once (25 > the
        // 20-color palette). A merged-branch shape cannot produce this — a
        // branch line dies where it meets the mainline — so the fan is the
        // canonical palette-overflow input. The interval recolor must stay
        // within the palette (reusing the soonest-ending holder's color,
        // best-effort) and the layout must be deterministic.
        let mut nodes = Vec::new();
        for i in 0..25 {
            let root = format!("r{i}");
            nodes.push(n(&root, 1 + i, &[]));
            nodes.push(n(&format!("p{i}"), 100 + i, &[root.as_str()]));
        }
        let mut m = n("M", 10_000, &[]);
        m.parents = (0..25).map(|i| format!("p{i}")).collect();
        nodes.push(m);
        let d = RawDag { nodes };
        let g = LaneLayout::default().compute(&d).unwrap();
        assert_no_overlap(&g);
        let mut rows = HashSet::new();
        for node in &g.nodes {
            assert!(rows.insert(node.row), "duplicate row");
        }
        assert_eq!(rows.len(), g.nodes.len());
        // At no row may more than the palette size of live lines share a color.
        let mut by_row: HashMap<u32, Vec<u32>> = HashMap::new();
        for line in &g.lines {
            for seg in &line.segments {
                for row in seg.from_row..=seg.to_row {
                    by_row.entry(row).or_default().push(line.color);
                }
            }
        }
        let max_live = by_row.values().map(|c| c.len()).max().unwrap();
        assert!(
            max_live > BRANCH_PALETTE.len(),
            "fixture must exceed the palette (max live {max_live})"
        );
        for (row, colors) in &by_row {
            let distinct = colors.iter().collect::<HashSet<_>>();
            assert!(
                distinct.len() <= BRANCH_PALETTE.len(),
                "row {row} uses {} colors > palette {}",
                distinct.len(),
                BRANCH_PALETTE.len()
            );
        }
        // Deterministic under a reversed input (colors must cycle the same way).
        let again = LaneLayout::default()
            .compute(&RawDag {
                nodes: d.nodes.iter().rev().cloned().collect(),
            })
            .unwrap();
        assert_eq!(g, again);
    }

    // `release` is a profile cfg that is only set for `--release` builds; the
    // dev-profile test build does not list it, so allow the lint.
    #[allow(unexpected_cfgs)]
    #[test]
    fn ten_k_commit_layout_is_fast() {
        // 5000-commit trunk + 500 branches of 5–20 commits, mostly merged.
        let mut nodes = Vec::new();
        for i in 0..5000 {
            let mut node = n(&format!("t{i}"), 10_000 * (i + 1), &[]);
            if i > 0 {
                node.parents = vec![format!("t{}", i - 1)];
            }
            nodes.push(node);
        }
        let mut trunk: u64 = 5000;
        for b in 0..500u64 {
            let len = 5u64 + (b % 16);
            let base = (b * 9) % 5000;
            let base_date = 10_000 * (base + 1);
            let mut prev = nodes[base as usize].hash.clone();
            for k in 0..len {
                let hash = format!("br{b}x{k}");
                let mut node = n(&hash, base_date + 100 * (k + 1), &[]);
                node.parents = vec![prev.clone()];
                prev = node.hash.clone();
                nodes.push(node);
            }
            // Merge most branches back into the trunk; leave ~1 in 10 unmerged.
            if b % 10 != 0 {
                let merge_hash = format!("merge{b}");
                let mut m = n(&merge_hash, 10_000 * (trunk + 1) + 500, &[]);
                m.parents = vec![format!("t{}", trunk - 1), prev];
                nodes.push(m);
                let tip_hash = format!("t{trunk}");
                let mut t = n(&tip_hash, 10_000 * (trunk + 1), &[]);
                t.parents = vec![merge_hash];
                nodes.push(t);
                trunk += 1;
            }
        }
        let d = RawDag { nodes };
        assert!(
            d.nodes.len() >= 10_000,
            "fixture must be ≥10k commits: {}",
            d.nodes.len()
        );
        let start = std::time::Instant::now();
        let g = LaneLayout::default().compute(&d).unwrap();
        let elapsed = start.elapsed();
        eprintln!(
            "layout of {} commits: {elapsed:?} ({} lines)",
            d.nodes.len(),
            g.lines.len()
        );
        // SPEC §10.1 targets <300ms for the shipping (release) build; debug
        // builds run ~10x slower, so the guard is profile-relative to keep
        // `cargo test` non-failing (the plan's acceptance criterion) while
        // still catching algorithmic regressions.
        let limit_ms = if cfg!(release) { 300 } else { 5_000 };
        assert!(
            elapsed.as_millis() < limit_ms,
            "layout took {elapsed:?}; limit {limit_ms}ms for this profile"
        );
    }
}
