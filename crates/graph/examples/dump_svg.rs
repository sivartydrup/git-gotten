//! Dev/test helper: dump a real repository's commit graph as a static SVG.
//!
//! Usage: `cargo run -p graph --example dump_svg <repo-path> <out.svg>`

use std::path::Path;

use graph::{build_dag, to_svg, LaneLayout};

fn main() {
    let mut args = std::env::args().skip(1);
    let (Some(path), Some(out)) = (args.next(), args.next()) else {
        eprintln!("usage: dump_svg <repo-path> <out.svg>");
        std::process::exit(2);
    };
    let repo = gix::open(Path::new(&path))
        .unwrap_or_else(|e| panic!("failed to open {path}: {e}"));
    let dag = build_dag(&repo).unwrap_or_else(|e| panic!("failed to build DAG: {e}"));
    let graph = LaneLayout::default()
        .compute(&dag)
        .unwrap_or_else(|e| panic!("failed to lay out graph: {e}"));
    std::fs::write(&out, to_svg(&graph))
        .unwrap_or_else(|e| panic!("failed to write {out}: {e}"));
    println!(
        "{path}: {} commits, {} edges, {} lines -> {out}",
        graph.nodes.len(),
        graph.edges.len(),
        graph.lines.len()
    );
}
