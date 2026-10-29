//! The git oracle: my layout must match what `git log --graph` actually
//! produces.
//!
//! `git log --graph` implies `--topo-order`, whose row order and column
//! (lane) assignment are the very things this stage ports. For each fixture
//! shape we run the real `git log --graph --all`, parse each commit's (row,
//! lane) from the output glyphs, and require an exact match with
//! `LaneLayout::compute` on the `gix`-built DAG.
//!
//! The row order is deliberately *not* commit-date order in two of the three
//! fixtures: git walks in topological ready-stack order with tips in
//! reverse-ref order, so these fixtures pin that discipline down end to end.

use git_gotten_testutil::Fixture;
use graph::{LaneLayout, build_dag};

/// Parse `git log --graph --all --format=%H%x00%s` output into
/// `(subject, lane, row)` in emission order.
///
/// A commit's row is its position in the output; its lane is the byte column
/// of the `*` glyph divided by 2 (git's graph cell width). Gutter-only
/// connector lines carry no `*` and are skipped.
fn parse_git_graph(out: &str) -> Vec<(String, i32, u32)> {
    let mut result = Vec::new();
    for line in out.lines() {
        let Some(star) = line.find('*') else {
            continue;
        };
        let lane = star as i32 / 2;
        // After the `*`: one space, then `<hash>\0<subject>`.
        let rest = &line[star + 2..];
        let subject = rest.split('\0').nth(1).unwrap_or("").to_string();
        result.push((subject, lane, result.len() as u32));
    }
    result
}

/// Assert my layout matches git's graph for every commit in the fixture.
fn assert_matches_git(fx: &Fixture) {
    let git_out = fx.git(&["log", "--graph", "--all", "--format=%H%x00%s"]);
    let git = parse_git_graph(&git_out);

    let repo = gix::open(&fx.path).expect("fixture must be a valid repository");
    let dag = build_dag(&repo).expect("build_dag failed");
    let g = LaneLayout::default().compute(&dag).expect("layout failed");

    let mine: Vec<(String, i32, u32)> = g
        .nodes
        .iter()
        .map(|n| (n.subject.clone(), n.lane, n.row))
        .collect();

    assert_eq!(mine.len(), git.len(), "commit counts differ");
    for (i, ((gs, gl, gr), (ms, ml, mr))) in git.iter().zip(mine.iter()).enumerate() {
        assert_eq!(
            (gs, gl, gr),
            (ms, ml, mr),
            "commit {i}: git says ({gs}, lane {gl}, row {gr}) but layout says ({ms}, lane {ml}, row {mr})"
        );
    }
}

#[test]
fn classic_merge_matches_git_graph() {
    let fx = Fixture::new("oracle-merge");
    fx.commit_file_at("a.txt", "1\n", "A", 1_000);
    fx.commit_file_at("a.txt", "2\n", "B", 1_001);
    fx.commit_file_at("a.txt", "3\n", "C", 1_002);
    fx.git(&["checkout", "-b", "feature"]);
    fx.commit_file_at("b.txt", "1\n", "D", 1_003);
    fx.commit_file_at("b.txt", "2\n", "E", 1_004);
    fx.git(&["checkout", "main"]);
    fx.git(&["merge", "--no-ff", "feature", "-m", "M"]);
    assert_matches_git(&fx);
}

#[test]
fn independent_branches_follow_ref_order_not_dates() {
    // Two unrelated lines. Branch `a`'s tip is NEWER than branch `b`'s, yet
    // git emits the `b` line first (tips come out in reverse lexicographic
    // ref order under --topo-order). A date-priority walk would get this
    // backwards - this fixture pins the git-faithful discipline.
    let fx = Fixture::new("oracle-lines");
    fx.git(&["checkout", "-b", "a"]);
    fx.commit_file_at("a1", "1\n", "A1", 80);
    fx.commit_file_at("a1", "2\n", "A2", 90);
    fx.commit_file_at("a1", "3\n", "A3", 100);
    fx.git(&["checkout", "-b", "b"]);
    fx.commit_file_at("b1", "1\n", "B1", 85);
    fx.commit_file_at("b1", "2\n", "B2", 95);
    assert_matches_git(&fx);
}

#[test]
fn octopus_and_unmerged_tip_match_git_graph() {
    let fx = Fixture::new("oracle-octopus");
    fx.commit_file_at("t", "1\n", "T0", 100);
    fx.commit_file_at("t", "2\n", "T1", 200);
    let mut ts = 300;
    for branch in ["f1", "f2", "f3"] {
        fx.git(&["checkout", "-b", branch, "main"]);
        fx.commit_file_at(&format!("{branch}"), "1\n", &format!("{branch}A"), ts);
        ts += 1;
        fx.commit_file_at(&format!("{branch}"), "2\n", &format!("{branch}B"), ts);
        ts += 1;
    }
    // One unmerged side branch (its tip is a second graph tip).
    fx.git(&["checkout", "-b", "solo", "main"]);
    fx.commit_file_at("solo", "1\n", "SOLO", ts);
    // Octopus-merge f1..f3 back into main.
    fx.git(&["checkout", "main"]);
    fx.git(&["merge", "--no-ff", "f1", "f2", "f3", "-m", "O"]);
    assert_matches_git(&fx);
}
