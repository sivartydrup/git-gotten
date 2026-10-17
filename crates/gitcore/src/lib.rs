//! GitGotten git engine (the "heart" — SPEC §8).
//!
//! Hybrid design: `gix` (gitoxide) for the read model, the real `git` CLI for writes.
//! Every mutating operation compiles to a visible `CommandPlan` (the transparency
//! primitive, SPEC §8/§9) and is run by a serialized per-repo executor.
//!
//! Stage 0 scaffolds the crate with [`GitError`], the error type every `gitcore`
//! operation will return. Later stages add the `gix` read-model (`open`, `status`),
//! the `CommandPlan`/executor, and the per-repo operation queue.

use std::path::PathBuf;

/// Errors surfaced by the git engine.
///
/// Displayed text is part of the transparency contract: the exact command and the
/// reason it failed are always available to the UI (SPEC §18).
#[derive(Debug, thiserror::Error)]
pub enum GitError {
    /// The path does not exist or is not a git repository.
    #[error("not a git repository: {path}")]
    NotARepository { path: PathBuf },

    /// A git subprocess exited with a non-zero status.
    #[error("git command failed (exit {exit}): {detail}")]
    Command { exit: i32, detail: String },

    /// A low-level I/O failure while touching the working tree or the index.
    #[error("git i/o error: {0}")]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_a_repository_renders_the_path() {
        let e = GitError::NotARepository { path: "/nope".into() };
        assert_eq!(e.to_string(), "not a git repository: /nope");
    }

    #[test]
    fn command_error_renders_exit_and_detail() {
        let e = GitError::Command {
            exit: 1,
            detail: "nothing to commit".into(),
        };
        assert_eq!(e.to_string(), "git command failed (exit 1): nothing to commit");
    }

    #[test]
    fn io_error_converts_via_from() {
        let io = std::io::Error::new(std::io::ErrorKind::NotFound, "missing");
        let e: GitError = io.into();
        assert!(e.to_string().contains("git i/o error"));
    }
}
