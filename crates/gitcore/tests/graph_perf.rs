//! Stage 13 performance gate (SPEC §18): the real `load_graph` path — `gix` DAG
//! build + lane layout + worktree markers — on a 10 000+ commit repository.
//!
//! The layout crate's own bench feeds a synthetic `RawDag`; this one measures
//! what the app actually pays per `load_graph` call, including reading every
//! commit out of the object database. Run it in the shipping profile to get the
//! numbers SPEC §18 targets (`cargo test --release -p gitcore --test graph_perf`).

use std::time::Instant;

use git_gotten_testutil::{big_history_fixture, git};
use gitcore::Repo;

#[test]
fn graph_of_a_ten_k_commit_repo_stays_under_the_spec_budget() {
    let fx = big_history_fixture();
    let commits: usize = git(&fx.path, &["rev-list", "--count", "HEAD"])
        .parse()
        .expect("rev-list --count");
    assert!(
        commits >= 10_000,
        "fixture must be a 10k-commit repository: {commits} commits"
    );

    let repo = Repo::open(&fx.path).unwrap();

    // Cold call: exactly what the app does when a repository is opened.
    let start = Instant::now();
    let graph = repo.graph().expect("graph of the 10k fixture");
    let elapsed = start.elapsed();
    let payload = serde_json::to_string(&graph).expect("graph must serialize");

    eprintln!(
        "{commits} commits -> {} nodes, {} edges, {} lines in {elapsed:?}; payload {} bytes",
        graph.nodes.len(),
        graph.edges.len(),
        graph.lines.len(),
        payload.len(),
    );
    // `debug_assertions` is the reliable profile signal (`cfg!(release)` is not
    // set by cargo); a debug test build is ~10x slower than the shipping build,
    // so the budget is profile-relative: the SPEC §18 target in release, and a
    // generous regression guard in debug.
    let limit_ms = if cfg!(debug_assertions) { 3_000 } else { 300 };
    assert!(
        elapsed.as_millis() < limit_ms,
        "graph pipeline took {elapsed:?}; budget {limit_ms}ms for this profile"
    );
}
