//! Stage 11 read model (SPEC §10.6): saved stashes, listed through git's own
//! `stash list` porcelain (the `worktree list` precedent — the registry, not a
//! gix object walk), with each snapshot's files resolved to tracked changes
//! plus stashed untracked files.

use std::path::Path;

use git_gotten_protocol::Stash;

use crate::{GitError, show};

/// Every stash, newest first; the list position is the stash's `id`.
pub(crate) fn list(cwd: &Path) -> Result<Vec<Stash>, GitError> {
    let out = show::run_git_sync(cwd, &["stash", "list", "--format=%gd%x1f%s%x1f%cI"])?;
    let mut stashes = Vec::new();
    for (id, line) in out.lines().enumerate() {
        let mut fields = line.split('\u{1f}');
        let (Some(name), Some(subject), Some(date)) = (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        let (branch, message) = subject_parts(subject);
        stashes.push(Stash {
            id: id as u32,
            name: name.to_string(),
            message,
            branch,
            date: date.to_string(),
            files: files(cwd, name)?,
        });
    }
    Ok(stashes)
}

/// Split a `git stash list` subject into (branch, message).
///
/// git's own forms name the branch: `WIP on <branch>: <hash> <subject>` keeps
/// the full subject as the message; a custom `git stash push -m` message
/// arrives as `On <branch>: <message>` and the bare message is kept. Anything
/// else keeps the subject verbatim with no branch.
fn subject_parts(subject: &str) -> (Option<String>, String) {
    for prefix in ["WIP on ", "On "] {
        if let Some(rest) = subject.strip_prefix(prefix)
            && let Some((_branch, tail)) = rest.split_once(": ")
        {
            let message = if prefix == "WIP on " { subject } else { tail };
            return (
                Some(rest[..rest.len() - tail.len() - 2].to_string()),
                message.to_string(),
            );
        }
    }
    (None, subject.to_string())
}

/// The files one stash carries: the tracked change diff (`git stash show`),
/// plus — when the stash has a third parent (stashed untracked files) — the
/// files of that commit.
fn files(cwd: &Path, name: &str) -> Result<Vec<String>, GitError> {
    // `git stash show` exits 1 on stashes whose diff it cannot render (merge stashes).
    let mut files: Vec<String> =
        show::run_git_sync_allow(cwd, &["stash", "show", "--name-only", name], 1)?
            .lines()
            .map(str::to_string)
            .collect();
    let third = show::run_git_sync_allow(
        cwd,
        &["rev-parse", "--verify", "--quiet", &format!("{name}^3")],
        1,
    )?;
    if !third.trim().is_empty() {
        files.extend(
            show::run_git_sync(
                cwd,
                &[
                    "diff-tree",
                    "--root",
                    "--no-commit-id",
                    "--name-only",
                    "-r",
                    &format!("{name}^3"),
                ],
            )?
            .lines()
            .map(str::to_string),
        );
    }
    files.sort();
    files.dedup();
    Ok(files)
}
