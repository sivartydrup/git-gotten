//! Stage 15 read path (SPEC §10.1): `Repo::commit_stats` — per-commit lines added and
//! removed against the **first parent**, the numbers the graph draws inline.
//!
//! Numbers are pinned exactly and, for every commit except the rename, cross-checked
//! against the real `git` CLI: `git show -M --numstat` is the expected value. The rename is
//! the documented divergence (this read does no rename detection), and a merge is where
//! git's own `show` prints nothing while this read shows first-parent numbers.

use std::time::Instant;

use git_gotten_testutil::{git, Fixture};
use gitcore::{CommitStat, Repo, COMMIT_STATS_MAX_HASHES};

/// The numstat of one commit (path, added, removed) as real `git` computes it.
fn numstat(fx: &Fixture, hash: &str) -> Vec<(String, u32, u32)> {
    git(&fx.path, &["show", "-M", "--numstat", "--format=", hash])
        .lines()
        .map(|line| {
            let mut it = line.split('\t');
            let add = it.next().unwrap();
            let del = it.next().unwrap();
            let path = it.next().unwrap().to_string();
            // `-` is git's "binary, counts unknown".
            let parse = |s: &str| if s == "-" { 0 } else { s.parse::<u32>().unwrap() };
            (path, parse(add), parse(del))
        })
        .collect()
}

fn totals(fx: &Fixture, hash: &str) -> (i64, i64) {
    numstat(fx, hash)
        .into_iter()
        .fold((0, 0), |acc, (_p, a, d)| (acc.0 + a as i64, acc.1 + d as i64))
}

/// Look up one hash in a batch response.
fn stat_of(stats: &[CommitStat], hash: &str) -> Option<CommitStat> {
    stats.iter().find(|s| s.hash == hash).cloned()
}

/// A linear history with known line counts: +1, +3, -2, then a rename (priced as its own
/// deletion plus addition) alongside a new file, then a new file beside a mode-only change.
fn linear_fixture() -> (Fixture, Vec<String>) {
    let fx = Fixture::new("stats-linear");
    fx.commit_file_at("a.txt", "one\n", "c1", 1_000); // root: +1
    fx.commit_file_at(
        "a.txt",
        "one\ntwo\nthree\nfour\n",
        "c2",
        1_100, // +3
    );
    fx.commit_file_at("a.txt", "one\ntwo\n", "c3", 1_200); // -2
    fx.git(&["mv", "a.txt", "b.txt"]);
    fx.commit_file_at("c.txt", "x\ny\n", "c4-rename", 1_300); // rename -2/+2, plus c.txt +2
    fx.write_file("b.txt", "one\ntwo\n");
    fx.git(&["update-index", "--chmod=+x", "b.txt"]); // mode-only: git's numstat is 0/0
    fx.commit_file_at("d.txt", "z\n", "c5-mode", 1_400); // +1
    let hashes: Vec<String> = git(&fx.path, &["rev-list", "--reverse", "HEAD"])
        .lines()
        .map(String::from)
        .collect();
    assert_eq!(hashes.len(), 5, "fixture shape changed");
    (fx, hashes)
}

#[test]
fn commit_stats_follow_the_documented_rules_in_a_linear_history() {
    let (fx, hashes) = linear_fixture();
    let repo = Repo::open(&fx.path).unwrap();
    let all: Vec<&str> = hashes.iter().map(String::as_str).collect();
    let stats = repo.commit_stats(&all);

    assert_eq!(stats.len(), 5, "one stat per hash, in order");
    assert_eq!(
        stats.iter().map(|s| s.hash.as_str()).collect::<Vec<_>>(),
        hashes.iter().map(String::as_str).collect::<Vec<_>>(),
        "results are in request order"
    );

    // Exact expected numbers, oldest first.
    assert_eq!(
        stats
            .iter()
            .map(|s| (s.insertions, s.deletions))
            .collect::<Vec<_>>(),
        vec![(1, 0), (3, 0), (0, 2), (4, 2), (1, 0)],
        "root +1, +3, -2, a rename (-2/+2) beside a new +2 file, then +1 with a 0/0 mode change"
    );

    // Git-CLI parity everywhere except the rename: `git show -M` folds a.txt -> b.txt into
    // 0/0 and counts only c.txt, while this read prices the rename as its own deletion plus
    // addition (no rename detection, by design).
    let rename = &hashes[3];
    for (hash, stat) in hashes.iter().zip(&stats) {
        if hash == rename {
            assert_eq!(
                totals(&fx, rename),
                (2, 0),
                "git -M must treat this commit as a rename plus c.txt"
            );
            continue;
        }
        assert_eq!(
            (stat.insertions, stat.deletions),
            totals(&fx, hash),
            "commit_stats must agree with `git show -M --numstat` for {hash}"
        );
    }
}

#[test]
fn root_commit_stats_diff_against_the_empty_tree() {
    let (fx, hashes) = linear_fixture();
    let repo = Repo::open(&fx.path).unwrap();
    let root = &hashes[0];

    let stats = repo.commit_stats(&[root.as_str()]);
    assert_eq!(
        stats,
        vec![CommitStat {
            hash: root.clone(),
            insertions: 1,
            deletions: 0,
        }]
    );
    assert_eq!(fx.git(&["rev-list", "--parents", root]), format!("{root}"), "root");
}

#[test]
fn merge_commit_stats_count_against_the_first_parent_only() {
    let fx = Fixture::new("stats-merge");
    fx.commit_file_at("a.txt", "a\n", "c1", 1_000);
    fx.git(&["checkout", "-b", "feature"]);
    fx.commit_file_at("side1.txt", "s1\ns2\ns3\n", "c2", 1_100); // +3
    fx.commit_file_at("side2.txt", "s4\ns5\n", "c3", 1_200); // +2
    fx.git(&["checkout", "main"]);
    fx.commit_file_at("main1.txt", "m1\n", "c4", 1_300); // +1
    let first_parent = fx.git(&["rev-parse", "HEAD"]);
    let second_parent = fx.git(&["rev-parse", "feature"]);
    let (code, _o, err) = fx.git_run(&["merge", "--no-ff", "feature", "-m", "merge feature"]);
    assert_eq!(code, 0, "merge should be clean: {err}");
    let merge = fx.git(&["rev-parse", "HEAD"]);
    assert_eq!(
        fx.git(&["rev-list", "--parents", "-1", &merge]),
        format!("{merge} {first_parent} {second_parent}"),
        "fixture must have the merge's first parent on main"
    );
    // `git show` says nothing about a merge; the read model must not share that gap.
    // (`git log`'s default merge format is a combined diff, which is why `show` is
    // the comparison for `CommitDetail`'s empty merge stat.)
    let (code, merge_show, _err) = fx.git_run(&["show", "--format=", "--", &merge]);
    assert_eq!(code, 0, "`git show` on a merge exits 0");
    assert!(
        merge_show.trim().is_empty(),
        "git's own default merge diff is empty: {merge_show:?}"
    );

    let repo = Repo::open(&fx.path).unwrap();
    let stats = repo.commit_stats(&[merge.as_str()]);
    assert_eq!(
        stats,
        vec![CommitStat {
            hash: merge.clone(),
            insertions: 5,
            deletions: 0,
        }],
        "the merge is diffed against its first parent: side1.txt (+3) and side2.txt (+2) \
         are new there; the branch's own commits are not counted twice"
    );
}

#[test]
fn a_deleted_file_counts_as_deletions_and_a_pure_mode_change_as_nothing() {
    let fx = Fixture::new("stats-delete-mode");
    fx.commit_file_at("keep.txt", "k1\nk2\nk3\n", "c1", 1_000);
    fx.commit_file_at("gone.txt", "g1\ng2\n", "c2", 1_100);
    let before_delete = fx.git(&["rev-parse", "HEAD"]);
    fx.git(&["rm", "gone.txt"]);
    let (code, _o, err) = fx.git_run(&["commit", "-m", "c3-delete"]);
    assert_eq!(code, 0, "commit should succeed: {err}");
    let delete = fx.git(&["rev-parse", "HEAD"]);

    // A mode-only change carries no content change: git's numstat line for it is
    // `0\t0\tpath`, so it contributes 0/0. The chmod goes through the index and the
    // commit takes the whole index — committing a pathspec would re-stage the file from
    // the worktree and undo the staged mode change (`git commit` then exits 1).
    fx.write_file("keep.txt", "k1\nk2\nk3\n");
    fx.git(&["update-index", "--chmod=+x", "keep.txt"]);
    let (code, out, err) = fx.git_run(&["commit", "-m", "c4-mode"]);
    assert_eq!(code, 0, "commit should succeed: {err}{out}");
    let mode_only = fx.git(&["rev-parse", "HEAD"]);
    assert!(
        out.contains("mode change 100644 => 100755 keep.txt"),
        "the fixture must be a pure mode change: {out}"
    );
    assert_eq!(
        totals(&fx, &mode_only),
        (0, 0),
        "git's own numstat for a mode-only commit is 0/0"
    );

    let repo = Repo::open(&fx.path).unwrap();
    let stats = repo.commit_stats(&[before_delete.as_str(), delete.as_str(), mode_only.as_str()]);
    assert_eq!(
        stats.iter().map(|s| (s.insertions, s.deletions)).collect::<Vec<_>>(),
        vec![(2, 0), (0, 2), (0, 0)]
    );
}

#[test]
fn a_binary_file_contributes_zero_and_not_one() {
    let fx = Fixture::new("stats-binary");
    fx.commit_file_at("a.txt", "a\n", "c1", 1_000);
    std::fs::write(fx.path.join("img.bin"), [0u8, 1, 2, 0, 3, 'x' as u8, 0, 4]).unwrap();
    fx.add("img.bin");
    fx.commit_file_at("note.txt", "n1\nn2\n", "c2", 1_100);
    let merge_bin = fx.git(&["rev-parse", "HEAD"]);

    // The binary blob must be the only thing git cannot count.
    let stats_git = numstat(&fx, &merge_bin);
    assert!(
        stats_git.iter().any(|(_, a, d)| *a == 0 && *d == 0),
        "fixture must contain an undiffable file: {stats_git:?}"
    );

    let repo = Repo::open(&fx.path).unwrap();
    let got = stat_of(&repo.commit_stats(&[merge_bin.as_str()]), &merge_bin).unwrap();
    assert_eq!(
        (got.insertions, got.deletions),
        totals(&fx, &merge_bin),
        "the binary file contributes 0/0, exactly as git's `-` columns do"
    );
    assert_eq!((got.insertions, got.deletions), (2, 0), "only note.txt counts");
}

#[test]
fn out_of_order_hashes_return_stats_in_request_order() {
    let (fx, hashes) = linear_fixture();
    let repo = Repo::open(&fx.path).unwrap();
    let oldest = &hashes[0];
    let newest = &hashes[hashes.len() - 1];
    let middle = &hashes[2];

    let requested = [newest.as_str(), oldest.as_str(), middle.as_str()];
    let stats = repo.commit_stats(&requested);
    assert_eq!(
        stats.iter().map(|s| s.hash.as_str()).collect::<Vec<_>>(),
        vec![newest.as_str(), oldest.as_str(), middle.as_str()]
    );
    assert_eq!(
        stats
            .iter()
            .map(|s| (s.insertions, s.deletions))
            .collect::<Vec<_>>(),
        vec![(1, 0), (1, 0), (0, 2)],
        "each stat travels with its own hash"
    );
}

#[test]
fn unknown_and_malformed_hashes_are_skipped_not_an_error() {
    let (fx, hashes) = linear_fixture();
    let repo = Repo::open(&fx.path).unwrap();
    let unknown = "0000000000000000000000000000000000000000";

    let stats = repo.commit_stats(&[
        hashes[0].as_str(),
        unknown,
        "not-a-hash",
        "",
        hashes[1].as_str(),
    ]);
    assert_eq!(
        stats.iter().map(|s| s.hash.as_str()).collect::<Vec<_>>(),
        vec![hashes[0].as_str(), hashes[1].as_str()],
        "unknown hashes are absent, the rest keeps its order"
    );

    // A whole batch of unknowns is an empty list, not a failure.
    assert!(repo.commit_stats(&[unknown, "nope"]).is_empty());
    assert!(repo.commit_stats(&[]).is_empty());
}

#[test]
fn a_request_is_capped_at_two_hundred_hashes() {
    let fx = Fixture::new("stats-cap");
    let mut script = String::new();
    let mut ts: u64 = 1_700_000_000;
    for i in 0..260 {
        ts += 1;
        script.push_str(&format!(
            "commit refs/heads/main\n\
             author Test User <test@example.com> {ts} +0000\n\
             committer Test User <test@example.com> {ts} +0000\n\
             data <<END\nc{i}\nEND\n\
             M 644 inline f.txt\ndata <<END\nc{i}\nEND\n"
        ));
    }
    fx.fast_import(&script);
    fx.git(&["reset", "--hard", "main"]);
    let count: usize = git(&fx.path, &["rev-list", "--count", "HEAD"]).parse().unwrap();
    assert_eq!(count, 260, "fixture shape changed");

    let repo = Repo::open(&fx.path).unwrap();
    // Newest first: the cap must cut the END of the request, not its front.
    let hashes: Vec<String> = git(&fx.path, &["rev-list", "HEAD"]).lines().map(String::from).collect();
    let all: Vec<&str> = hashes.iter().map(String::as_str).collect();

    let capped = repo.commit_stats(&all);
    assert_eq!(
        capped.len(),
        COMMIT_STATS_MAX_HASHES,
        "a longer request is truncated to the cap"
    );
    assert_eq!(
        capped.iter().map(|s| s.hash.as_str()).collect::<Vec<_>>(),
        all[..COMMIT_STATS_MAX_HASHES],
        "the first {COMMIT_STATS_MAX_HASHES} hashes of the request are answered"
    );
    assert_eq!(
        capped.iter().map(|s| (s.insertions, s.deletions)).collect::<Vec<_>>(),
        vec![(1, 1); COMMIT_STATS_MAX_HASHES],
        "each of these commits replaced one line with the next"
    );

    // Exactly at the cap, and one below it: no truncation.
    assert_eq!(repo.commit_stats(&all[..COMMIT_STATS_MAX_HASHES]).len(), COMMIT_STATS_MAX_HASHES);
    assert_eq!(repo.commit_stats(&all[..COMMIT_STATS_MAX_HASHES - 1]).len(), COMMIT_STATS_MAX_HASHES - 1);
}

/// SPEC §10.1's memoization: a repeat request must not re-diff the object database.
#[test]
fn repeated_requests_are_served_from_the_per_commit_cache() {
    let (fx, hashes) = linear_fixture();
    let repo = Repo::open(&fx.path).unwrap();
    let all: Vec<&str> = hashes.iter().map(String::as_str).collect();

    let start = Instant::now();
    let first = repo.commit_stats(&all);
    let cold = start.elapsed();
    let start = Instant::now();
    let second = repo.commit_stats(&all);
    let warm = start.elapsed();
    assert_eq!(first, second, "a repeat request answers identically");
    assert!(
        second.iter().all(|s| !s.hash.is_empty()),
        "cached stats keep their hash"
    );
    eprintln!("commit_stats cold {cold:?} warm {warm:?}");
    assert!(
        warm <= cold,
        "the cached pass must not cost more than the diffing pass: cold {cold:?} warm {warm:?}"
    );

    // A new commit is the event that invalidates the memo: the same hash must still
    // report its own numbers after the cache is dropped (and a fresh commit must
    // appear, which a stale memo would hide).
    fx.commit_file_at("e.txt", "e1\ne2\n", "c6", 1_500);
    let new_hash = fx.git(&["rev-parse", "HEAD"]);
    let after = repo.commit_stats(&[new_hash.as_str(), hashes[0].as_str()]);
    assert_eq!(
        after.iter().map(|s| (s.insertions, s.deletions)).collect::<Vec<_>>(),
        vec![(2, 0), (1, 0)],
        "the memo is refilled after a commit, not served stale or empty"
    );
}
