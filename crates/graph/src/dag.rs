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
pub fn classify(full_name: &str) -> Option<(RefKind, String)> {
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

/// Collect the ref labels pointing at every commit of the repository.
///
/// Branches and remote-tracking refs label their target directly, as do
/// lightweight tags. An annotated tag labels its *peeled* commit: the loose
/// ref targets the tag object (and packed refs additionally yield a
/// `^{}`-suffixed peeled entry, which dedupes to the same display name).
///
/// Shared by the DAG build and by `gitcore::Repo::commit_detail`, so the
/// graph and the inspect drawer always agree on what points where.
pub fn collect_ref_labels(
    repo: &gix::Repository,
) -> Result<HashMap<ObjectId, Vec<RefLabel>>, GraphError> {
    // Peeled: an annotated tag resolves to its commit. (The iterator must do
    // the peeling itself - it holds the packed-refs buffer, which a consumer
    // peel could not.)
    let mut labels: HashMap<ObjectId, Vec<RefLabel>> = HashMap::new();
    let refs = repo.references().map_err(map_err)?;
    let mut iter = refs.all().map_err(map_err)?.peeled().map_err(map_err)?;
    while let Some(reference) = iter.next() {
        let reference = reference.map_err(map_err)?;
        let full_name = String::from_utf8_lossy(reference.name().as_bstr()).into_owned();
        let Some((kind, name)) = classify(&full_name) else {
            continue;
        };
        // A packed ref's peeled entry is named `v1^{}`; strip the suffix so
        // it displays (and dedupes) like the `v1` entry.
        let suffix = ["^", "{}"].concat(); // git's peeled-ref suffix
        let name = name.strip_suffix(&suffix).unwrap_or(&name).to_string();
        let Some(target) = reference.try_id().map(|id| id.detach()) else {
            continue; // symbolic refs (HEAD) carry no label
        };
        // The ref's own target, or - when it is an annotated tag object -
        // the peeled commit. Refs to trees/blobs label nothing.
        let oid = if repo.find_commit(target).is_ok() {
            target
        } else if let Some(peeled) = reference.inner.peeled {
            if repo.find_commit(peeled).is_ok() {
                peeled
            } else {
                continue;
            }
        } else {
            continue;
        };
        let entry = labels.entry(oid).or_default();
        if !entry.iter().any(|l| l.name == name && l.kind == kind) {
            entry.push(RefLabel {
                name,
                kind,
                color: 0, // the layout assigns the line's color in assemble()
            });
        }
    }
    Ok(labels)
}

/// See [`crate::build_dag`].
pub fn build(repo: &gix::Repository) -> Result<RawDag, GraphError> {
    let ref_labels = collect_ref_labels(repo)?;
    let mut targets_seen = HashSet::new();
    let mut walk_targets = Vec::new();
    for oid in ref_labels.keys() {
        if targets_seen.insert(*oid) {
            walk_targets.push(*oid);
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
