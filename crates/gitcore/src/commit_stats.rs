//! Commit-diffstat read for the graph's inline +/- numbers (SPEC §10.1).
//!
//! `gix`-only, like the rest of the read model: the commit's tree diffed against its
//! first parent's tree, with each changed file priced by the blob diff. No git subprocess,
//! so a capped batch is one pass over the object database.
//!
//! # What a number means
//!
//! These are **our** definition, not a claim to reproduce `git show --numstat`
//! byte-for-byte:
//!
//! - Insertions/deletions of the commit's tree against its **first parent** only. A root
//!   commit diffs against the empty tree; an octopus merge reports its first parent, not
//!   a combined diff (git shows nothing for a merge by default).
//! - Summed over every changed file, with **no rename detection**: a pure rename reports
//!   the full deletion plus the full addition, where `git numstat -M` would report 0/0.
//! - A file that is binary on either side contributes **0/0** (git prints `-`). Binary is
//!   decided the way git decides it: a NUL byte in the first 8000 bytes of either blob.
//! - A mode-only change (exec bit, mode swap with identical content) contributes **0/0**.
//! - A gitlink (submodule commit entry) contributes **0/0**.
//! - A final line gaining or losing its newline is counted by whatever gix's blob diff
//!   reports for that file; no attempt is made to match git's line accounting there.

use std::ops::ControlFlow;

use gix::object::tree::diff::Change;
use gix::Repository;

/// Bytes probed for the binary test, matching git's own heuristic.
const BINARY_PROBE_BYTES: usize = 8000;

/// `Ok(true)`: binary, so it contributes 0/0. `Ok(false)`: diffable. `Err(())`: the blob
/// exists in the tree but could not be read, which must not silently shorten a total.
fn is_binary(id: Option<gix::Id<'_>>) -> Result<bool, ()> {
    let Some(id) = id else {
        return Ok(false);
    };
    let blob = id
        .object()
        .and_then(|obj| obj.try_into_blob())
        .map_err(|_| ())?;
    let bytes: &[u8] = &blob.data;
    Ok(bytes.iter().take(BINARY_PROBE_BYTES).any(|byte| *byte == 0))
}

/// Line totals of a commit tree against its parent tree.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CommitLineStats {
    /// Lines added.
    pub insertions: i64,
    /// Lines removed.
    pub deletions: i64,
}

/// Sum the inserted/removed lines of `parent_tree -> commit_tree`; see the module doc for
/// the exact rules.
///
/// A `None` `commit_tree` (a commit whose tree cannot be read) yields `0`/`0` rather than
/// failing the batch: a graph row with no numbers is better than a failed request. A blob
/// named by the diff that cannot be read also yields `0`/`0` for the whole commit, because
/// a total built without it would be silently short.
pub fn tree_stats(
    repo: &Repository,
    parent_tree: Option<&gix::Tree<'_>>,
    commit_tree: Option<&gix::Tree<'_>>,
) -> CommitLineStats {
    let Some(commit_tree) = commit_tree else {
        return CommitLineStats::default();
    };
    let empty = repo.empty_tree();
    let parent_tree = parent_tree.unwrap_or(&empty);

    // Rename tracking off: it pays for a similarity metric and, per gix, does not change
    // these totals — a rename therefore reports its full deletion plus addition.
    let Ok(mut platform) = parent_tree.changes() else {
        return CommitLineStats::default();
    };
    platform.options(|options| {
        options.track_rewrites(None);
    });

    // The pricing cache is ours, not the walker's: `for_each_to_obtain_tree` builds its own
    // cache internally, so holding this one in the closure is not a borrow conflict (the
    // same shape gix's own `Platform::stats()` uses) and it is reused across the commit's
    // files instead of per commit.
    let Ok(mut cache) = repo.diff_resource_cache_for_tree_diff() else {
        return CommitLineStats::default();
    };

    let mut out = CommitLineStats::default();
    let mut unreadable = false;

    let walked = platform
        .for_each_to_obtain_tree(commit_tree, |change| {
            // `lhs` is the parent tree, so `Deletion` is the old side and `Addition` the new.
            let (old, new) = match &change {
                Change::Modification {
                    previous_entry_mode,
                    previous_id,
                    entry_mode,
                    id,
                    ..
                } => {
                    if !previous_entry_mode.is_blob_or_symlink() || !entry_mode.is_blob_or_symlink() {
                        return Ok(ControlFlow::Continue(())); // gitlink or mode we cannot diff
                    }
                    (Some(*previous_id), Some(*id))
                }
                Change::Addition { entry_mode, id, .. } => {
                    if !entry_mode.is_blob_or_symlink() {
                        return Ok(ControlFlow::Continue(()));
                    }
                    (None, Some(*id))
                }
                Change::Deletion { entry_mode, id, .. } => {
                    if !entry_mode.is_blob_or_symlink() {
                        return Ok(ControlFlow::Continue(()));
                    }
                    (Some(*id), None)
                }
                Change::Rewrite {
                    source_entry_mode,
                    source_id,
                    entry_mode,
                    id,
                    ..
                } => {
                    if !source_entry_mode.is_blob_or_symlink() || !entry_mode.is_blob_or_symlink() {
                        return Ok(ControlFlow::Continue(()));
                    }
                    (Some(*source_id), Some(*id))
                }
            };

            match (is_binary(old), is_binary(new)) {
                (Ok(false), Ok(false)) => {}
                // Binary on either side contributes 0/0, like git's `-` columns.
                (Ok(true), _) | (_, Ok(true)) => return Ok(ControlFlow::Continue(())),
                _ => {
                    unreadable = true;
                    return Ok(ControlFlow::Continue(()));
                }
            }

            match change
                .diff(&mut cache)
                .and_then(|mut blob_platform| blob_platform.line_counts())
            {
                Ok(Some(counts)) => {
                    out.insertions += i64::from(counts.insertions);
                    out.deletions += i64::from(counts.removals);
                }
                Ok(None) => {}
                Err(_) => unreadable = true,
            }
            cache.clear_resource_cache_keep_allocation();
            Ok(ControlFlow::Continue(()))
        })
        .is_ok();

    if walked && !unreadable {
        out
    } else {
        CommitLineStats::default()
    }
}
