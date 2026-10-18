//! Static SVG rendering of a laid-out [`Graph`].
//!
//! A test/dev tool, not the product renderer (Stage 4 is a canvas): it emits a
//! standalone SVG of the same data the canvas will draw — lanes as vertical
//! strokes, merge edges as curves, nodes as circles, refs + subjects as labels
//! — so a real repository's layout can be eyeballed from a file
//! (`cargo run -p graph --example dump_svg <repo> <out.svg>`).

use git_gotten_protocol::{BRANCH_PALETTE, Graph, Node};

const ROW_H: f64 = 28.0;
const COL_W: f64 = 120.0;
const MARGIN: f64 = 16.0;
const LABEL_W: f64 = 300.0;

/// Palette index → `#rrggbb`.
fn color(rgb: u32) -> String {
    format!("#{rgb:06x}")
}

/// The palette color of a node's own line (the line whose segment covers the
/// node's (lane, row)), if any.
fn node_color(g: &Graph, node: &Node) -> u32 {
    g.lines
        .iter()
        .find(|line| {
            line.segments
                .iter()
                .any(|s| s.lane == node.lane && node.row >= s.from_row && node.row <= s.to_row)
        })
        .map(|line| line.color)
        .unwrap_or(0)
}

fn cx(lane: i32) -> f64 {
    MARGIN + (lane.max(0) as f64) * COL_W + COL_W / 2.0
}

fn cy(row: u32) -> f64 {
    MARGIN + row as f64 * ROW_H + ROW_H / 2.0
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Render `g` as a standalone SVG document.
pub fn to_svg(g: &Graph) -> String {
    let rows = g.nodes.iter().map(|node| node.row).max().unwrap_or(0) as u32 + 1;
    let lanes = g.nodes.iter().map(|node| node.lane).max().unwrap_or(0) + 1;
    let max_lane = g
        .lines
        .iter()
        .flat_map(|line| line.segments.iter())
        .map(|s| s.lane)
        .max()
        .unwrap_or(-1)
        .max(lanes - 1);
    let width = MARGIN * 2.0 + (max_lane + 1) as f64 * COL_W + LABEL_W;
    let height = MARGIN * 2.0 + rows as f64 * ROW_H;

    let mut s = String::new();
    s.push_str(&format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w:.0}\" height=\"{h:.0}\" viewBox=\"0 0 {w:.0} {h:.0}\">\n",
        w = width,
        h = height
    ));
    s.push_str(&format!(
        "<rect width=\"100%\" height=\"100%\" fill=\"#1e2430\"/>\n"
    ));

    // Lane strokes (one run per segment).
    for line in &g.lines {
        let stroke = color(BRANCH_PALETTE[(line.color as usize) % BRANCH_PALETTE.len()]);
        for seg in &line.segments {
            let x = cx(seg.lane);
            let y0 = MARGIN + seg.from_row as f64 * ROW_H;
            let y1 = MARGIN + (seg.to_row + 1) as f64 * ROW_H;
            s.push_str(&format!(
                "<line x1=\"{x:.1}\" y1=\"{y0:.1}\" x2=\"{x:.1}\" y2=\"{y1:.1}\" stroke=\"{stroke}\" stroke-width=\"2\"/>\n"
            ));
        }
    }

    // Edges: cubic curve from the child's bottom to the parent's top.
    let pos = |hash: &str| -> Option<(f64, f64)> {
        g.nodes
            .iter()
            .find(|node| node.hash == hash)
            .map(|node| (cx(node.lane), cy(node.row)))
    };
    for edge in &g.edges {
        if let (Some((x1, y1)), Some((x2, y2))) = (pos(&edge.from), pos(&edge.to)) {
            let y1 = y1 + 5.0;
            let y2 = y2 - 5.0;
            let bend = (y2 - y1) / 2.0;
            s.push_str(&format!(
                "<path d=\"M {x1:.1} {y1:.1} C {x1:.1} {:.1}, {x2:.1} {:.1}, {x2:.1} {y2:.1}\" fill=\"none\" stroke=\"#5a6478\" stroke-width=\"1.5\"/>\n",
                y1 + bend,
                y2 - bend
            ));
        }
    }

    // Nodes + labels.
    for node in &g.nodes {
        let (x, y) = (cx(node.lane), cy(node.row));
        let fill = color(BRANCH_PALETTE[(node_color(g, node) as usize) % BRANCH_PALETTE.len()]);
        let r = if node.is_head { 7.0 } else { 5.0 };
        s.push_str(&format!(
            "<circle cx=\"{x:.1}\" cy=\"{y:.1}\" r=\"{r}\" fill=\"{fill}\" stroke=\"#1e2430\" stroke-width=\"2\"/>\n"
        ));
        let mut label_x = x + r + 6.0;
        for label in &node.refs {
            let fill = color(BRANCH_PALETTE[(label.color as usize) % BRANCH_PALETTE.len()]);
            s.push_str(&format!(
                "<text x=\"{label_x:.1}\" y=\"{y:.1}\" font-size=\"11\" font-family=\"monospace\" fill=\"{fill}\">{name}</text>\n",
                name = esc(&label.name)
            ));
            label_x += label.name.chars().count() as f64 * 6.5 + 8.0;
        }
        let subject: String = node.subject.chars().take(40).collect();
        s.push_str(&format!(
            "<text x=\"{label_x:.1}\" y=\"{y:.1}\" font-size=\"12\" font-family=\"sans-serif\" fill=\"#c8d0e0\">{subject}</text>\n",
            subject = esc(&subject)
        ));
    }

    s.push_str("</svg>\n");
    s
}
