//! Stage 13 hardening (SPEC §10.1): `Repo::graph` must *reuse* its per-repository
//! DAG + layout cache while nothing the graph depends on has moved, and must never
//! serve a stale graph after a commit, a ref move, a checkout or a worktree change.
//!
//! The cache lives in `Repo` and is keyed by a fingerprint of every ref target,
//! `HEAD` and each linked worktree's recorded HEAD — so the check is cheap, and
//! invalidation is not tied to `Repo::refresh` (which only re-opens the `gix`
//! handle). Every test re-reads through the *same* handle: if a mutation were
//! invisible to the fingerprint, the stale cached graph would be served and the
//! assertion fails.

use std::path::PathBuf;
use std::time::Instant;

use git_gotten_protocol::RefKind;
use git_gotten_testutil::{big_history_fixture, git, Fixture};
use gitcore::Repo;

fn wt_dir(label: &str) -> PathBuf {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    std::env::temp_dir().join(format!("gitgotten-graphcache-{}-{seq}-{label}", std::process::id()))
}

/// SPEC §10.1's saving, measured on the SPEC §18 benchmark repository: browsing and
/// staging must not re-walk 10k commits.
#[test]
fn graph_is_rebuilt_only_when_the_graph_actually_moves() {
    let fx = big_history_fixture();
    let repo = Repo::open(&fx.path).unwrap();

    let start = Instant::now();
    let cold = repo.graph().expect("cold graph");
    let cold_ms = start.elapsed().as_millis();

    // A commit and a branch move: both must be visible on the *same* handle.
    fx.commit_file("new.txt", "new\n", "c-new");
    fx.git(&["branch", "moved-tip"]);
    let start = Instant::now();
    let moved = repo.graph().expect("graph after a commit + branch");
    let moved_ms = start.elapsed().as_millis();

    // Staging and editing touch the index and the worktree only — the graph is
    // unchanged, so the cache must answer it.
    fx.write_file("dirty.txt", "uncommitted\n");
    fx.git(&["add", "-A"]);
    let start = Instant::now();
    let cached = repo.graph().expect("cached graph");
    let cached_ms = start.elapsed().as_millis();

    eprintln!(
        "{} nodes: cold {cold_ms}ms, invalidated {moved_ms}ms, cache hit {cached_ms}ms",
        cold.nodes.len()
    );
    assert_ne!(moved, cold, "a new commit must recompute the graph");
    assert_eq!(
        moved.nodes.len(),
        cold.nodes.len() + 1,
        "the new commit is a new node"
    );
    assert!(
        moved
            .nodes
            .iter()
            .any(|n| n
                .refs
                .iter()
                .any(|r| r.kind == RefKind::Branch && r.name == "moved-tip")),
        "the new branch must label the new commit"
    );
    assert_eq!(cached, moved, "staging must not change the graph");
    // The saving is the point, and it must survive a faster machine: both
    // measurements come from the same build, so the ratio is profile-independent.
    assert!(
        cached_ms * 10 < cold_ms.max(1),
        "cache hit {cached_ms}ms did not beat the {cold_ms}ms rebuild"
    );
}

/// Invalidation must cover every input of the fingerprint — including the ones that
/// move no ref at all.
#[test]
fn every_graph_input_invalidates_the_cache() {
    let fx = Fixture::new("graph-cache");
    fx.commit_file("a.txt", "1\n", "c1");
    fx.commit_file("b.txt", "2\n", "c2");
    let repo = Repo::open(&fx.path).unwrap();

    let base = repo.graph().unwrap();
    assert_eq!(base.nodes.len(), 2);
    assert!(
        base.nodes.iter().any(|n| n
            .refs
            .iter()
            .any(|r| r.kind == RefKind::Branch && r.name == "main")),
        "main must label its tip: {base:?}"
    );
    // Same inputs → the same answer, straight from the cache.
    assert_eq!(repo.graph().unwrap(), base);

    // A tag: a new label on an existing commit.
    let parent = fx.git(&["rev-parse", "HEAD~1"]);
    fx.git(&["tag", "v1", &parent]);
    let tagged = repo.graph().unwrap();
    assert_ne!(tagged, base, "a tag must recompute the graph");
    assert!(
        tagged.nodes.iter().any(|n| n.hash == parent
            && n.refs.iter().any(|r| r.kind == RefKind::Tag && r.name == "v1")),
        "v1 must label HEAD~1"
    );

    // A branch move: the same commit gains a second label.
    fx.git(&["branch", "release", &parent]);
    let branched = repo.graph().unwrap();
    assert_ne!(branched, tagged, "a new branch must recompute the graph");
    assert!(
        branched.nodes.iter().any(|n| n.hash == parent
            && n.refs.iter().any(|r| r.name == "release")),
        "release must label HEAD~1"
    );

    // A commit no ref points at: `HEAD` is the only thing that reaches it, so HEAD
    // belongs in the key too.
    fx.commit_file("c.txt", "3\n", "c3");
    let tip = fx.git(&["rev-parse", "HEAD"]);
    fx.git(&["checkout", "--detach", &tip]);
    fx.git(&["branch", "-D", "main"]);
    let orphaned = repo.graph().unwrap();
    assert_ne!(orphaned, branched, "a deleted branch must recompute the graph");
    assert!(
        orphaned.nodes.iter().any(|n| n.hash == tip),
        "a HEAD-only commit must be walked"
    );
    fx.git(&["checkout", "release"]);
    let back = repo.graph().unwrap();
    assert_ne!(back, orphaned, "the walked set follows HEAD, not only the refs");
    assert!(
        !back.nodes.iter().any(|n| n.hash == tip),
        "an unreachable-by-HEAD commit must leave the graph"
    );
    // A worktree added with a detached HEAD: it moves a checkout marker and the
    // worktree's recorded HEAD, while no ref is created or moved at all.
    let wt = wt_dir("add");
    fx.git(&["worktree", "add", "--detach", wt.to_str().unwrap(), &parent]);
    let worktreed = repo.graph().unwrap();
    assert_ne!(worktreed, back, "a new worktree must recompute the graph");
    let short: String = parent.chars().take(7).collect();
    assert!(
        worktreed.nodes.iter().any(|n| n.hash == parent
            && n
                .refs
                .iter()
                .any(|r| r.kind == RefKind::Worktree && r.name == short)),
        "the worktree must mark its checked-out commit as {short}"
    );
    // The app's most frequent reads (status, `git status`) change nothing.
    let _ = repo.status().unwrap();
    let _ = git(&fx.path, &["status", "--porcelain"]);
    assert_eq!(repo.graph().unwrap(), worktreed);
}
