//! Stage 15: per-repository state (SPEC §16) — branch color assignments and the
//! last selection are stored per repo in the app-data directory, survive a
//! restart, and a corrupt state file is rebuilt rather than fatal.
//!
//! Runs against real git fixtures (the same `plan → executor` path the app uses).

use git_gotten_app::Backend;
use git_gotten_protocol::{Graph, RepoSelection};
use git_gotten_testutil as testutil;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static SEQ: AtomicU64 = AtomicU64::new(0);

fn store_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "gitgotten-state-{}-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed),
        label
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The branch name → palette index assignment the graph currently renders.
fn colors(graph: &Graph) -> BTreeMap<String, u32> {
    graph
        .lines
        .iter()
        .filter_map(|line| line.name.clone().map(|name| (name, line.color)))
        .collect()
}

/// Every line alive at one row must hold a distinct palette color (SPEC §10.1),
/// sticky assignments included.
fn assert_alive_colors_distinct(graph: &Graph) {
    for node in &graph.nodes {
        let alive: Vec<u32> = node
            .lines
            .iter()
            .map(|id| graph.lines[*id as usize].color)
            .collect();
        let unique: HashSet<u32> = alive.iter().copied().collect();
        assert_eq!(
            unique.len(),
            alive.len(),
            "row {} renders a duplicate color: {alive:?}",
            node.row
        );
    }
}

/// The one `.state.json` in `dir`, for corrupting it on purpose.
fn state_file(dir: &Path) -> PathBuf {
    std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|path| path.to_string_lossy().ends_with(".state.json"))
        .expect("a per-repo state file was written")
}

/// A main + side history with two visible branch lines.
fn two_branch_fixture(label: &str) -> testutil::Fixture {
    let fx = testutil::Fixture::new(label);
    fx.commit_file("a.txt", "1\n", "c1");
    fx.git(&["checkout", "-b", "side"]);
    fx.commit_file("b.txt", "2\n", "side-1");
    fx.git(&["checkout", "main"]);
    fx.commit_file("c.txt", "3\n", "c2");
    fx
}

#[test]
fn branch_colors_survive_a_layout_change_and_a_restart() {
    let dir = store_dir("colors");
    let fx = two_branch_fixture("state-colors");

    let (backend, _rx) = Backend::new();
    backend.set_store(dir.join("app.json"));
    backend.open(fx.id()).unwrap();
    let first = backend.load_graph(fx.id()).unwrap();
    assert_alive_colors_distinct(&first);
    let before = colors(&first);
    assert!(
        before.contains_key("main") && before.contains_key("side"),
        "both branch lines should be named: {before:?}"
    );

    // A third branch changes the layout, so the naive palette walk would shift
    // every line after it. The stored assignment must not move.
    fx.git(&["checkout", "-b", "extra", "side"]);
    fx.commit_file("d.txt", "4\n", "extra-1");
    fx.commit_file("e.txt", "5\n", "extra-2");
    let after = backend.load_graph(fx.id()).unwrap();
    assert_alive_colors_distinct(&after);
    let kept = colors(&after);
    // Every branch that still opens a line must keep the color it was assigned.
    // (`side` stops opening one here: `extra` branches from its tip, so the two refs
    // share a line whose name comes from the opener — the layout's existing rule.)
    for (name, color) in &before {
        if let Some(current) = kept.get(name) {
            assert_eq!(current, color, "branch {name} changed color");
        }
    }
    assert_eq!(kept.get("main"), before.get("main"), "main must keep its color");
    assert_ne!(
        kept.get("extra"),
        kept.get("main"),
        "a new branch must not clone the color of a line it lives beside"
    );

    // A fresh backend reading the same directory must render the same colors.
    let (restarted, _rx2) = Backend::new();
    restarted.set_store(dir.join("app.json"));
    restarted.open(fx.id()).unwrap();
    let third = restarted.load_graph(fx.id()).unwrap();
    assert_alive_colors_distinct(&third);
    assert_eq!(colors(&third), kept);
}

#[test]
fn the_last_selection_survives_a_restart() {
    let dir = store_dir("selection");
    let fx = two_branch_fixture("state-selection");
    let (backend, _rx) = Backend::new();
    backend.set_store(dir.join("app.json"));
    backend.open(fx.id()).unwrap();

    let graph = backend.load_graph(fx.id()).unwrap();
    let side = graph
        .nodes
        .iter()
        .find(|node| node.refs.iter().any(|r| r.name == "side"))
        .expect("a node labeled `side`");
    let saved = backend
        .save_repo_selection(
            fx.id(),
            RepoSelection {
                commit: Some(side.hash.clone()),
                branch: Some("side".into()),
            },
        )
        .unwrap();
    assert_eq!(saved.commit.as_deref(), Some(side.hash.as_str()));

    let (restarted, _rx2) = Backend::new();
    restarted.set_store(dir.join("app.json"));
    restarted.open(fx.id()).unwrap();
    let restored = restarted.load_repo_selection(fx.id()).unwrap();
    assert_eq!(restored, saved);

    // Clearing the selection is state too: the drawer must not reappear.
    restarted
        .save_repo_selection(
            fx.id(),
            RepoSelection {
                commit: None,
                branch: None,
            },
        )
        .unwrap();
    let (again, _rx3) = Backend::new();
    again.set_store(dir.join("app.json"));
    again.open(fx.id()).unwrap();
    assert_eq!(
        again.load_repo_selection(fx.id()).unwrap(),
        RepoSelection::default()
    );
}

#[test]
fn a_corrupt_state_file_is_rebuilt_not_fatal() {
    let dir = store_dir("corrupt-state");
    let fx = two_branch_fixture("state-corrupt");
    let (backend, _rx) = Backend::new();
    backend.set_store(dir.join("app.json"));
    backend.open(fx.id()).unwrap();
    backend
        .save_repo_selection(
            fx.id(),
            RepoSelection {
                commit: Some("deadbeef".into()),
                branch: Some("main".into()),
            },
        )
        .unwrap();
    std::fs::write(state_file(&dir), "{ not json").unwrap();

    // Load must survive the garbage, and the next write repairs the file.
    let (restarted, _rx2) = Backend::new();
    restarted.set_store(dir.join("app.json")); // must not panic
    restarted.open(fx.id()).unwrap();
    assert_eq!(
        restarted.load_repo_selection(fx.id()).unwrap(),
        RepoSelection::default()
    );
    restarted
        .save_repo_selection(
            fx.id(),
            RepoSelection {
                commit: Some("cafe1234".into()),
                branch: Some("side".into()),
            },
        )
        .unwrap();

    let (again, _rx3) = Backend::new();
    again.set_store(dir.join("app.json"));
    again.open(fx.id()).unwrap();
    assert_eq!(again.load_repo_selection(fx.id()).unwrap().commit.as_deref(), Some("cafe1234"));
    assert!(colors(&again.load_graph(fx.id()).unwrap()).contains_key("main"));
}

#[test]
fn state_is_per_repository() {
    let dir = store_dir("per-repo");
    let fx_a = two_branch_fixture("state-a");
    let fx_b = two_branch_fixture("state-b");
    let (backend, _rx) = Backend::new();
    backend.set_store(dir.join("app.json"));
    backend.open(fx_a.id()).unwrap();
    backend.open(fx_b.id()).unwrap();

    backend
        .save_repo_selection(
            fx_a.id(),
            RepoSelection {
                commit: Some("aaaaaaaa".into()),
                branch: Some("side".into()),
            },
        )
        .unwrap();
    let b_colors = colors(&backend.load_graph(fx_b.id()).unwrap());

    let (restarted, _rx2) = Backend::new();
    restarted.set_store(dir.join("app.json"));
    restarted.open(fx_a.id()).unwrap();
    restarted.open(fx_b.id()).unwrap();
    assert_eq!(
        restarted.load_repo_selection(fx_a.id()).unwrap().commit.as_deref(),
        Some("aaaaaaaa")
    );
    assert_eq!(
        restarted.load_repo_selection(fx_b.id()).unwrap(),
        RepoSelection::default(),
        "repo B never had a selection"
    );
    assert_eq!(colors(&restarted.load_graph(fx_b.id()).unwrap()), b_colors);
}
