//! Integration tests: `build_dag` + `LaneLayout` on real repositories built
//! with the real `git` CLI (the fixtures from `crates/testutil`).
//!
//! Covers the read-model mapping (refs → labels, peeling, HEAD, dates) and
//! the layout on real-world shapes. The row-order/lanes *faithfulness* check
//! against `git log --graph` lives in `oracle.rs`.


use git_gotten_protocol::{RefKind, RefLabel};
use git_gotten_testutil::{Fixture, RemoteFixture};
use graph::{build_dag, to_svg, GraphError, LaneLayout, RawDag};

/// Open the fixture with `gix` and build its DAG.
fn dag_of(fx: &Fixture) -> RawDag {
    let repo = gix::open(&fx.path).expect("fixture must be a valid repository");
    build_dag(&repo).expect("build_dag failed")
}

/// The classic shape: `A - B - C` on `main`, `B - D - E` on `feature`, and a
/// no-FF merge on `main` of `C` + `E`. All commits carry distinct, spaced dates.
fn classic_fixture() -> Fixture {
    let fx = Fixture::new("graph-classic");
    fx.commit_file_at("a.txt", "one\n", "A", 100);
    fx.commit_file_at("a.txt", "two\n", "B", 200);
    fx.commit_file_at("a.txt", "three\n", "C", 300);
    fx.git(&["checkout", "-b", "feature"]);
    fx.commit_file_at("b.txt", "one\n", "D", 400);
    fx.commit_file_at("b.txt", "two\n", "E", 500);
    fx.git(&["checkout", "main"]);
    fx.git(&["merge", "--no-ff", "feature", "-m", "merge feature"]);
    fx
}

fn node<'a>(d: &'a RawDag, subject: &str) -> &'a graph::RawNode {
    d.nodes
        .iter()
        .find(|n| n.subject == subject)
        .unwrap_or_else(|| panic!("no node with subject {subject:?}"))
}

#[test]
fn empty_repository_is_an_error() {
    let fx = Fixture::new("graph-empty");
    let repo = gix::open(&fx.path).unwrap();
    let err = build_dag(&repo).unwrap_err();
    assert!(matches!(err, GraphError::Empty), "got {err:?}");
}

#[test]
fn classic_fixture_maps_the_dag() {
    let fx = classic_fixture();
    let d = dag_of(&fx);

    // All six commits, nothing more.
    let subjects = d
        .nodes
        .iter()
        .map(|n| n.subject.clone())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        subjects,
        ["A", "B", "C", "D", "E", "merge feature"]
            .into_iter()
            .map(String::from)
            .collect::<std::collections::BTreeSet<_>>()
    );

    // Fields: the commit's own values (dates are the pinned unix seconds).
    let a = node(&d, "A");
    assert_eq!(a.author, "Test User");
    assert_eq!(a.date, 100);
    assert_eq!(a.sort_date, 100);
    assert!(a.parents.is_empty());
    let m = node(&d, "merge feature");
    assert_eq!(m.parents.len(), 2, "a merge has two parents");

    // Refs: main at the merge, feature at its tip; HEAD at the merge.
    assert_eq!(
        m.refs,
        vec![RefLabel {
            name: "main".into(),
            kind: RefKind::Branch,
            color: 0
        }]
    );
    assert!(m.is_head, "main is checked out at M");
    assert_eq!(
        node(&d, "E").refs,
        vec![RefLabel {
            name: "feature".into(),
            kind: RefKind::Branch,
            color: 0
        }]
    );
    assert!(node(&d, "C").refs.is_empty());
}

#[test]
fn classic_fixture_layouts_like_git_log_graph() {
    let g = LaneLayout::default().compute(&dag_of(&classic_fixture())).unwrap();

    // Row order (git's --topo-order walk): merge, E, D, C, B, A.
    assert_eq!(
        g.nodes.iter().map(|n| n.subject.as_str()).collect::<Vec<_>>(),
        vec!["merge feature", "E", "D", "C", "B", "A"]
    );
    // Lanes: mainline on the left; feature in its own lane, dead at the merge.
    let lane = |s: &str| g.nodes.iter().find(|n| n.subject == s).unwrap().lane;
    assert_eq!(lane("merge feature"), 0);
    assert_eq!(lane("E"), 1);
    assert_eq!(lane("D"), 1);
    assert_eq!(lane("C"), 0);
    assert_eq!(lane("B"), 0);
    assert_eq!(lane("A"), 0);

    // Two lines with distinct colors; the feature line is a single lane-1 run.
    assert_eq!(g.lines.len(), 2);
    let feat = g.lines.iter().find(|l| l.name.as_deref() == Some("feature")).unwrap();
    let main = g.lines.iter().find(|l| l.name.as_deref() == Some("main")).unwrap();
    assert_ne!(feat.color, main.color);
    assert_eq!(feat.segments.len(), 1);
    assert_eq!(feat.segments[0].lane, 1);
    assert_eq!(main.segments[0].lane, 0);
    assert_eq!(main.segments.len(), 1);
}

#[test]
fn octopus_merge_records_all_parents() {
    let fx = Fixture::new("graph-octopus");
    fx.commit_file_at("a.txt", "1\n", "A", 100);
    fx.commit_file_at("a.txt", "2\n", "B", 200);
    for (branch, file, ts) in [("b1", "b1", 300), ("b2", "b2", 400), ("b3", "b3", 500)] {
        fx.git(&["checkout", "-b", branch, "main"]);
        fx.commit_file_at(file, &format!("{branch}\n"), branch, ts);
    }
    fx.git(&["checkout", "main"]);
    fx.git(&["merge", "--no-ff", "b1", "b2", "b3", "-m", "O"]);

    let d = dag_of(&fx);
    let o = node(&d, "O");
    // An octopus merge records HEAD plus each merged branch: four parents.
    assert_eq!(o.parents.len(), 4, "octopus merge of three branches has four parents");
    let g = LaneLayout::default().compute(&d).unwrap();
    // Every merge parent edge is present.
    assert_eq!(
        g.edges.iter().filter(|e| e.from == o.hash).count(),
        4
    );
}

#[test]
fn annotated_tags_peel_to_their_commit() {
    let fx = Fixture::new("graph-tag");
    fx.commit_file_at("a.txt", "1\n", "A", 100);
    fx.commit_file_at("a.txt", "2\n", "B", 200);
    let a = fx.git(&["rev-parse", "HEAD~1"]);
    fx.git(&["tag", "-a", "v1.0", "-m", "release", &a]);

    let d = dag_of(&fx);
    assert_eq!(d.nodes.len(), 2, "a tag is not a node; it labels its commit");
    assert_eq!(
        node(&d, "A").refs,
        vec![RefLabel {
            name: "v1.0".into(),
            kind: RefKind::Tag,
            color: 0
        }]
    );
    // The branch label rides on the tip.
    assert_eq!(node(&d, "B").refs[0].name, "main");
}

#[test]
fn detached_head_is_marked_and_walked() {
    let fx = git_gotten_testutil::detached_fixture();
    let d = dag_of(&fx);
    assert_eq!(d.nodes.len(), 2);
    let head = d.nodes.iter().find(|n| n.is_head).expect("exactly one HEAD");
    assert_eq!(head.subject, "c1", "detached HEAD points at the older commit");
    assert!(!d.nodes.iter().find(|n| n.subject == "c2").unwrap().is_head);
}

/// Local `main` and `origin/main` diverge off a common base.
fn diverged_remote_fixture() -> Fixture {
    let mut fx = Fixture::new("graph-diverged");
    fx.commit_file_at("a.txt", "base\n", "C", 100);
    let remote_path = fx.add_origin("main");
    fx.commit_file_at("a.txt", "local\n", "C2", 200);
    // A remote-side commit via a throwaway clone of the bare remote.
    let clone = std::env::temp_dir().join(format!(
        "gitgotten-remote-clone-{}-diverged",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&clone);
    std::fs::create_dir_all(&clone).unwrap();
    RemoteFixture {
        path: remote_path.clone(),
    }
    .clone_into(&clone);
    let clone_fx = Fixture::from_path(clone.canonicalize().unwrap());
    clone_fx.commit_file_at("b.txt", "remote\n", "O", 300);
    clone_fx.git(&["push", "origin", "main"]);
    drop(clone_fx);
    fx.git(&["fetch", "-q", "origin"]);
    fx
}

#[test]
fn diverged_remote_gets_two_lanes() {
    let g = LaneLayout::default().compute(&dag_of(&diverged_remote_fixture())).unwrap();

    // O (origin/main, the newer ref name) emits first, C2 (main) next, base C
    // last. O and C2 each take their own lane; C converges on the leftmost.
    let at = |s: &str| -> (u32, i32) {
        let n = g.nodes.iter().find(|n| n.subject == s).unwrap();
        (n.row, n.lane)
    };
    assert_eq!(at("O"), (0, 0));
    assert_eq!(at("C2"), (1, 1));
    assert_eq!(at("C"), (2, 0));

    assert_eq!(g.lines.len(), 2);
    let o_line = g.lines.iter().find(|l| l.name.as_deref() == Some("origin/main")).unwrap();
    let main_line = g.lines.iter().find(|l| l.name.as_deref() == Some("main")).unwrap();
    assert_ne!(o_line.color, main_line.color);
    assert_eq!(
        g.nodes
            .iter()
            .find(|n| n.subject == "O")
            .unwrap()
            .refs[0]
            .kind,
        RefKind::Remote
    );
}

#[test]
fn svg_rendering_covers_every_graph_element() {
    let g = LaneLayout::default().compute(&dag_of(&classic_fixture())).unwrap();
    let svg = to_svg(&g);
    assert!(svg.starts_with("<svg") && svg.ends_with("</svg>\n"));
    // One circle per node, one path per edge, at least one line stroke.
    let circles = svg.matches("<circle").count();
    let paths = svg.matches("<path").count();
    let lines = svg.matches("<line").count();
    assert_eq!(circles, g.nodes.len());
    assert_eq!(paths, g.edges.len());
    assert!(lines >= g.lines.iter().map(|l| l.segments.len()).sum::<usize>());
    // Refs and subjects appear as labels.
    assert!(svg.contains("main") && svg.contains("feature"));
    assert!(svg.contains("merge feature"));
}

