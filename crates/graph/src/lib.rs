//! GitGotten branching graph (SPEC §10.1 — the main focus).
//!
//! Pure, unit-testable logic: build the commit DAG from a repository and compute a
//! lane layout. No rendering, no I/O beyond reading refs/objects — so the layout can be
//! exhaustively tested (it is the riskiest pure logic in the project, SPEC §21).
//!
//! Stage 0 scaffolds the crate with [`GraphError`]. Stage 3 adds `build_dag` +
//! `LaneLayout::compute`.

/// Errors from building or laying out the commit DAG.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum GraphError {
    /// The repository has no commits to render.
    #[error("empty repository: no commits to render")]
    Empty,
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
}
