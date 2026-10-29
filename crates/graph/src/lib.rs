//! GitGotten branching graph (SPEC §10.1 — the main focus).
//!
//! Pure, unit-testable logic: build the commit DAG from a repository
//! ([`build_dag`], reading refs + objects via `gix`) and compute a lane layout
//! ([`LaneLayout`]). No rendering, no IPC — so the layout can be exhaustively
//! tested (it is the riskiest pure logic in the project, SPEC §21). Stage 4
//! wires the result over IPC and renders it on a canvas.
//!
//! The pipeline: `gix::Repository` → [`build_dag`] → [`RawDag`] →
//! [`LaneLayout::compute`] → protocol [`Graph`]. [`to_svg`] renders a static
//! SVG for eyeball-verification in tests and dev.

mod dag;
mod layout;
mod svg;

pub use dag::classify as classify_ref;
pub use dag::collect_ref_labels;

pub use git_gotten_protocol::Graph;
pub use layout::LaneLayout;
pub use svg::to_svg;

use git_gotten_protocol::RefLabel;

/// A commit as read from the object database, before layout: identity,
/// display fields, its parents (hex oids), and the refs that point at it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RawNode {
    /// The commit's full object id (hex).
    pub hash: String,
    /// First line of the commit message.
    pub subject: String,
    /// Author name.
    pub author: String,
    /// Author date, unix seconds.
    pub date: u64,
    /// Committer date, unix seconds — the row-order tiebreak source.
    pub sort_date: u64,
    /// Parent object ids (hex), in commit order (first parent first).
    pub parents: Vec<String>,
    /// Refs (branches, remote-tracking, tags) pointing at this commit.
    pub refs: Vec<RefLabel>,
    /// `true` when HEAD points at this commit.
    pub is_head: bool,
}

/// A commit DAG: every commit reachable from the walked refs (plus HEAD), with
/// parent links. Node order is irrelevant to the layout.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RawDag {
    pub nodes: Vec<RawNode>,
}

/// Errors from building or laying out the commit DAG.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum GraphError {
    /// The repository has no commits to render.
    #[error("empty repository: no commits to render")]
    Empty,

    /// A `gix` failure while reading the repository (open, refs, objects).
    #[error("repository read failed: {0}")]
    Repository(String),

    /// Two DAG nodes share a hash (corrupt input).
    #[error("duplicate commit {0} in DAG")]
    DuplicateHash(String),

    /// A node lists a parent that is not in the DAG (corrupt input).
    #[error("commit {0} has a parent missing from the DAG")]
    MissingParent(String),

    /// The input DAG has a cycle; layout requires a well-founded DAG.
    #[error("cycle in commit DAG at {0}")]
    CyclicGraph(String),
}

/// Build the commit DAG of a repository: every commit reachable from its
/// branches, remote-tracking refs, tags, and HEAD.
///
/// Annotated tags are peeled to their commits (and labeled on the commit);
/// tag/peel targets that are not commits (tags pointing at trees/blobs) are
/// skipped. `is_head` marks the commit HEAD points at (a detached HEAD is
/// marked the same way).
pub fn build_dag(repo: &gix::Repository) -> Result<RawDag, GraphError> {
    dag::build(repo)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_renders() {
        assert_eq!(
            GraphError::Empty.to_string(),
            "empty repository: no commits to render"
        );
    }

    #[test]
    fn layout_empty_dag_is_an_error() {
        let result = LaneLayout::default().compute(&RawDag::default());
        assert_eq!(
            result.unwrap_err().to_string(),
            GraphError::Empty.to_string()
        );
    }

    #[test]
    fn corrupt_input_is_reported() {
        let dag = RawDag {
            nodes: vec![RawNode {
                hash: "a".into(),
                subject: "s".into(),
                author: "t".into(),
                date: 0,
                sort_date: 0,
                parents: vec!["b".into()],
                refs: Vec::new(),
                is_head: false,
            }],
        };
        let err = LaneLayout::default().compute(&dag).unwrap_err();
        assert!(matches!(err, GraphError::MissingParent(h) if h == "b"));

        let cyclic = RawDag {
            nodes: vec![
                RawNode {
                    hash: "a".into(),
                    subject: "s".into(),
                    author: "t".into(),
                    date: 0,
                    sort_date: 0,
                    parents: vec!["b".into()],
                    refs: Vec::new(),
                    is_head: false,
                },
                RawNode {
                    hash: "b".into(),
                    subject: "s".into(),
                    author: "t".into(),
                    date: 0,
                    sort_date: 0,
                    parents: vec!["a".into()],
                    refs: Vec::new(),
                    is_head: false,
                },
            ],
        };
        let err = LaneLayout::default().compute(&cyclic).unwrap_err();
        assert!(matches!(err, GraphError::CyclicGraph(h) if h == "a" || h == "b"));
    }
}
