//! `build_dag`: read the commit DAG of a repository out of `gix`.
//!
//! Walks every commit reachable from the repository's branches,
//! remote-tracking refs, tags, and HEAD, and records each commit's display
//! fields, parents, pointing refs, and the HEAD marker — the input the lane
//! layout consumes.

use std::collections::{HashMap, HashSet};

use gix::hash::ObjectId;
use git_gotten_protocol::{RefKind, RefLabel};

use crate::{GraphError, RawDag, RawNode};

/// Classify a ref's full name into a [`RefKind`] and display name.
///
/// `refs/heads/*` → branch (short name); `refs/remotes/<remote>/*` → remote
/// (`<remote>/<branch>`); `refs/tags/*` → tag (short name). Everything else
/// (notes, stash, …) is not part of the graph.
fn classify(full_name: &str) -> Option<(RefKind, String)> {
    if let Some(name) = full_name.strip_prefix("refs/heads/") {
        return Some((RefKind::Branch, name.to_string()));
    }
    if let Some(rest) = full_name.strip_prefix("refs/remotes/") {
        if rest.starts_with("refs/") {
            // `refs/remotes/refs/...` is not a real remote branch.
            return None;
        }
        return Some((RefKind::Remote, rest.to_string()));
    }
    if let Some(name) = full_name.strip_prefix("refs/tags/") {
        return Some((RefKind::Tag, name.to_string()));
    }
    None
}

fn map_err(e: impl std::fmt::Display) -> GraphError {
    GraphError::Repository(e.to_string())
}

/// See [`crate::build_dag`].
pub fn build(repo: &gix::Repository) -> Result<RawDag, GraphError> {
    let refs = repo.references().map_err(map_err)?;
    // Peeled: an annotated tag resolves to its commit. (The iterator must do
    // the peeling itself - it holds the packed-refs buffer, which a consumer
    // peel could not.) Refs to trees/blobs (a tag pointing at a blob) have no
    let mut ref_labels: HashMap<ObjectId, Vec<RefLabel>> = HashMap::new();
    let mut targets_seen = HashSet::new();
    let mut walk_targets = Vec::new();
    let mut iter = refs.all().map_err(map_err)?.peeled().map_err(map_err)?;
    while let Some(reference) = iter.next() {
        let reference = reference.map_err(map_err)?;
        let full_name = String::from_utf8_lossy(reference.name().as_bstr()).into_owned();
        let Some((kind, name)) = classify(&full_name) else {
            continue;
        };
        let Some(target) = reference.try_id().map(|id| id.detach()) else {
            continue;
        };
        if repo.find_commit(target).is_err() {
            continue;
        }
        ref_labels
            .entry(target)
            .or_default()
            .push(RefLabel {
                name,
                kind,
                color: 0, // the layout assigns the line's color in assemble()
            });
        if targets_seen.insert(target) {
            walk_targets.push(target);
        }
    }

    // HEAD (a detached HEAD is a walk target and the `is_head` marker).
    let head = repo.head().map_err(map_err)?;
    let head_id = head.id().map(|id| id.detach());
    if let Some(oid) = &head_id {
        if targets_seen.insert(*oid) {
            walk_targets.push(*oid);
        }
    }

    // ── Walk every reachable commit. ────────────────────────────────────────
    let mut nodes = Vec::new();
    for info in repo.rev_walk(walk_targets).all().map_err(map_err)? {
        let info = info.map_err(map_err)?;
        let commit = info.object().map_err(map_err)?;

        let subject = commit
            .message()
            .map_err(map_err)?
            .summary()
            .to_string();
        let author = commit.author().map_err(map_err)?.name.to_string();
        let date = u64::try_from(
            commit
                .author()
                .map_err(map_err)?
                .time()
                .map_err(map_err)?
                .seconds,
        )
        .unwrap_or(0);
        let sort_date = u64::try_from(
            commit
                .committer()
                .map_err(map_err)?
                .time()
                .map_err(map_err)?
                .seconds,
        )
        .unwrap_or(0);

        let parents = info
            .parent_ids
            .iter()
            .map(|id| id.to_hex().to_string())
            .collect();
        let refs = ref_labels.get(&info.id).cloned().unwrap_or_default();

        nodes.push(RawNode {
            hash: info.id.to_hex().to_string(),
            subject,
            author,
            date,
            sort_date,
            parents,
            refs,
            is_head: Some(info.id) == head_id,
        });
    }

    if nodes.is_empty() {
        return Err(GraphError::Empty);
    }
    Ok(RawDag { nodes })
}
