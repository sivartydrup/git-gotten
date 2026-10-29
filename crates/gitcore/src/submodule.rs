//! Stage 11 read model (SPEC §10.8): registered submodules and the Files-panel
//! merge. git's own view is authoritative: the name/path pairs from
//! `.gitmodules`, the recorded commit from the index gitlink (`git ls-files -s`),
//! and the checkout state from a `rev-parse` inside the submodule.

use std::path::Path;

use git_gotten_protocol::{ChangeKind, FileChange, Submodule};

use crate::{GitError, show};

/// Every submodule registered in `.gitmodules`, ordered by path. A configured
/// path whose gitlink is gone (a removed submodule) is skipped.
pub(crate) fn list(cwd: &Path) -> Result<Vec<Submodule>, GitError> {
    // `git config` exits 1 when `.gitmodules` is missing or the key is unset —
    // no submodules is the common case, and it stays one fast subprocess.
    let cfg = show::run_git_sync_allow(
        cwd,
        &["config", "-f", ".gitmodules", "--get-regexp", r"\.path$"],
        1,
    )?;
    let mut subs = Vec::new();
    for line in cfg.lines() {
        // shortcut: `submodule.<name>.path <path>` is split on the first space;
        // submodule names/paths containing spaces are not supported — upgrade to
        // a config parser if such a repo ever shows up.
        let Some((key, path)) = line.split_once(' ') else {
            continue;
        };
        let Some(key) = key.strip_suffix(".path") else {
            continue;
        };
        let name = key.strip_prefix("submodule.").unwrap_or(key).to_string();
        let Some(recorded) = recorded_commit(cwd, path)? else {
            continue;
        };
        let url = show::run_git_sync_allow(
            cwd,
            &[
                "config",
                "-f",
                ".gitmodules",
                &format!("submodule.{name}.url"),
            ],
            1,
        )?
        .trim()
        .to_string();
        // Initialized means the checkout carries its own `.git` (directory or
        // gitfile). Without it `rev-parse -C` would walk up to *this* repository
        // and report the superproject HEAD, so the guard is load-bearing.
        let initialized = cwd.join(path).join(".git").exists();
        let head = initialized
            .then(|| show::run_git_sync(cwd, &["-C", path, "rev-parse", "HEAD"]))
            .transpose()?
            .map(|h| h.trim().to_string());
        let differs = head.as_ref().is_some_and(|h| *h != recorded);
        subs.push(Submodule {
            name,
            path: path.to_string(),
            url,
            recorded,
            head,
            initialized,
            differs,
        });
    }
    subs.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(subs)
}

/// The gitlink commit recorded in the index for `path` (`git ls-files -s`
/// `160000 <sha> 0\t<path>`); `None` when the entry is not a gitlink.
fn recorded_commit(cwd: &Path, path: &str) -> Result<Option<String>, GitError> {
    let out = show::run_git_sync(cwd, &["ls-files", "-s", "--", path])?;
    Ok(out
        .lines()
        .find(|line| line.starts_with("160000 "))
        .and_then(|line| line.split('\t').next())
        .and_then(|head| head.split(' ').nth(1))
        .map(str::to_string))
}

/// Badge every change whose path is a submodule, and synthesize the change
/// `git status` reports for a pointer drift (`modified: <path> (new commits)`)
/// when the gix status skipped it (SPEC §10.8: it commits like a normal change).
pub(crate) fn tag_changes(cwd: &Path, changes: &mut Vec<FileChange>) -> Result<(), GitError> {
    for sub in list(cwd)? {
        if let Some(change) = changes.iter_mut().find(|c| c.path == sub.path) {
            change.submodule = true;
        } else if sub.differs {
            changes.push(FileChange {
                path: sub.path,
                kind: ChangeKind::Modified,
                staged: false,
                binary: false,
                submodule: true,
            });
        }
    }
    Ok(())
}
