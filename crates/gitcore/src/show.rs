//! Read-only `git show` access for the inspect drawer (SPEC §10.1): per-file
//! change counts (`--numstat`) and per-file unified diffs (`--patch`).
//!
//! Reads go through the real `git` binary (the plan's "git show stat" — the
//! `gix` read-model covers the live status, not on-demand object reads), with
//! the same environment hygiene as the executor: no leaked `GIT_DIR`-family
//! variables, no pager, no prompts. These commands mutate nothing and are not
//! `CommandPlan`s (SPEC §8: plans are for writes); a non-zero exit is a
//! [`GitError`] with the captured stderr.

use std::path::Path;
use std::process::Command;

use git_gotten_protocol::{FileDiff, FileStat};

use crate::exec::{resolve_git, ENV_SET, ENV_UNSET};
use crate::GitError;

/// Run `git <args>` synchronously in `cwd` and return its stdout.
///
/// A non-zero exit is [`GitError::Command`] (with the captured stderr); a
/// spawn failure (no git binary) is an I/O error.
pub(crate) fn run_git_sync(cwd: &Path, args: &[&str]) -> Result<String, GitError> {
    run_git_sync_allow(cwd, args, -1)
}

/// Like [`run_git_sync`], but tolerates one specific non-zero exit code.
///
/// Some git commands report "the result" through their exit code:
/// `git diff --no-index` exits `1` when the files differ, and `git config`
/// exits `1` when a key is unset. Callers that treat those as data pass the
/// code here; any other non-zero exit is still an error.
pub(crate) fn run_git_sync_allow(
    cwd: &Path,
    args: &[&str],
    allow_exit: i32,
) -> Result<String, GitError> {
    let program = resolve_git()?;
    let mut cmd = Command::new(&program);
    cmd.args(args);
    cmd.current_dir(cwd);
    for (key, value) in ENV_SET {
        cmd.env(key, value);
    }
    for key in ENV_UNSET {
        cmd.env_remove(key);
    }

    let output = cmd
        .output()
        .map_err(|e| GitError::Io(std::io::Error::new(e.kind(), e.to_string())))?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let exit = output.status.code().unwrap_or(-1);
    if !output.status.success() && exit != allow_exit {
        return Err(GitError::Command {
            exit,
            detail: stderr.trim().to_string(),
        });
    }
    Ok(stdout)
}

/// Per-file change counts of one commit: `git show -M --numstat --format=<hash>`.
///
/// `-M` forces rename detection so a rename reports one line (the new name, in
/// git's `{old => new}` brace form).
pub fn numstat(cwd: &Path, hash: &str) -> Result<Vec<FileStat>, GitError> {
    let out = run_git_sync(
        cwd,
        &["show", "-M", "--numstat", "--format=", hash],
    )?;
    Ok(parse_numstat(&out))
}

/// Per-file unified diffs of one commit: `git show -M --patch --format=<hash>`.
///
/// Root commits diff against the empty tree (git does this for `show`); the
/// caller skips merge commits (≥2 parents have no default diff).
pub fn patch(cwd: &Path, hash: &str) -> Result<Vec<FileDiff>, GitError> {
    let out = run_git_sync(
        cwd,
        &["show", "-M", "--patch", "--format=", hash],
    )?;
    Ok(split_patch(&out))
}

/// `git diff --numstat` line format: `additions<TAB>deletions<TAB>path`.
///
/// Binary files report `-` for both counts. Renames use git's brace form
/// `dir/{old => new}/file` (root renames: `old => new`); both resolve to the new name.
pub fn parse_numstat(out: &str) -> Vec<FileStat> {
    out.lines()
        .filter_map(|line| {
            let (add, rest) = line.split_once('\t')?;
            let (del, path) = rest.split_once('\t')?;
            let parse = |v: &str| v.parse::<u32>().unwrap_or(0);
            let binary = add == "-" || del == "-";
            Some(FileStat {
                path: resolve_rename(path),
                additions: parse(add),
                deletions: parse(del),
                binary,
            })
        })
        .collect()
}

/// Resolve git's rename path forms to the new name:
/// - `prefix/{old => new}/suffix` (renames inside a directory) → `prefix/new/suffix`
/// - `old => new` (root-level renames) → `new`
///
/// Paths without a rename marker pass through unchanged.
fn resolve_rename(path: &str) -> String {
    if let Some(open) = path.find('{') {
        let Some(arrow) = path[open..].find("=>") else {
            return path.to_string();
        };
        let arrow = open + arrow;
        let Some(close) = path[arrow..].find('}') else {
            return path.to_string();
        };
        let close = arrow + close;
        // Git pads the brace form with spaces around the arrow; they are not
        // part of the file name.
        let new = path[arrow + 2..close].trim();
        return format!("{}{}{}", &path[..open], new, &path[close + 1..]);
    }
    if let Some(arrow) = path.find(" => ") {
        return path[arrow + 4..].to_string();
    }
    path.to_string()
}

/// Split a `git show --patch` stream into per-file diffs.
///
/// Files are delimited by `diff --git a/... b/...` lines. Each file's patch
/// keeps everything after that header (mode/index lines, `---`/`+++`, hunks).
/// A rename takes the `rename to` path; a deletion (whose `b/` side is
/// `/dev/null`) keeps the `a/` side. A `Binary files ... differ` marker sets
/// `binary` instead of a text patch.
pub fn split_patch(out: &str) -> Vec<FileDiff> {
    let mut files: Vec<FileDiff> = Vec::new();
    let mut current: Option<FileDiff> = None;

    for line in out.lines() {
        if let Some(header) = line.strip_prefix("diff --git ") {
            if let Some(file) = current.take() {
                files.push(finish(file));
            }
            // `a/x /dev/null` (a deletion) has the path on the *first* token;
            // everything else reads it from the `b/` (final) side.
            let mut tokens = header.rsplit(' ');
            let last = tokens.next().unwrap_or("");
            let first = header.split(' ').next().unwrap_or("");
            let path = if last == "/dev/null" {
                strip_side_prefix(first)
            } else {
                strip_side_prefix(last)
            };
            current = Some(FileDiff {
                path,
                binary: false,
                patch: String::new(),
                hunks: Vec::new(),
            });
            continue;
        }
        let Some(file) = current.as_mut() else {
            continue; // preamble before the first file (should not happen)
        };
        if let Some(to) = line.strip_prefix("rename to ") {
            file.path = strip_side_prefix(to.trim());
        } else if let Some(_marker) = line.strip_prefix("Binary files ") {
            file.binary = true;
            file.patch = line.trim().to_string();
        } else {
            file.patch.push_str(line);
            file.patch.push('\n');
        }
    }
    if let Some(file) = current.take() {
        files.push(finish(file));
    }
    files
}

/// Fill in the parsed hunks of a finished file's diff (the `hunks` field).
/// Binary diffs have no text hunks (`hunks_from` yields none for the marker).
fn finish(file: FileDiff) -> FileDiff {
    FileDiff {
        hunks: crate::diff::hunks_from(&file.patch, false),
        ..file
    }
}

/// Strip the `a/` / `b/` side prefix from a diff header path.
fn strip_side_prefix(path: &str) -> String {
    if let Some(rest) = path.strip_prefix("b/") {
        return rest.to_string();
    }
    if let Some(rest) = path.strip_prefix("a/") {
        return rest.to_string();
    }
    path.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_numstat_counts_and_binary() {
        let out = "2\t1\ta.txt\n0\t3\tb.md\n-\t-\timg.png\n";
        let stats = parse_numstat(out);
        assert_eq!(stats.len(), 3);
        assert_eq!(stats[0].path, "a.txt");
        assert_eq!((stats[0].additions, stats[0].deletions), (2, 1));
        assert!(!stats[0].binary);
        assert_eq!((stats[1].additions, stats[1].deletions), (0, 3));
        assert!(!stats[1].binary);
        // Binary files report `-` for both counts.
        assert_eq!((stats[2].additions, stats[2].deletions), (0, 0));
        assert!(stats[2].binary);
        assert_eq!(stats[2].path, "img.png");
    }

    #[test]
    fn parse_numstat_resolves_renames() {
        let out = "1\t1\tdir/{old => new}/file.txt\n0\t0\told.txt => new.txt\n";
        let stats = parse_numstat(out);
        assert_eq!(stats[0].path, "dir/new/file.txt");
        assert_eq!(stats[1].path, "new.txt");
    }

    #[test]
    fn parse_numstat_ignores_non_tab_lines() {
        let stats = parse_numstat("no tabs here\n2\t1\tx\n");
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].path, "x");
    }

    #[test]
    fn split_patch_splits_files_and_renames_and_binaries() {
        let out = "\
diff --git a/a.txt b/a.txt
index 111..222 100644
--- a/a.txt
+++ b/a.txt
@@ -1 +1,2 @@
-one
+two
+three
diff --git a/old.txt b/new.txt
similarity index 70%
rename from old.txt
rename to new.txt
index 333..444 100644
--- a/old.txt
+++ b/new.txt
@@ -1 +1 @@
-x
+y
diff --git a/img.png b/img.png
Binary files a/img.png and b/img.png differ
";
        let files = split_patch(out);
        assert_eq!(files.len(), 3);

        assert_eq!(files[0].path, "a.txt");
        assert!(!files[0].binary);
        assert!(files[0].patch.contains("-one"));
        assert!(files[0].patch.contains("+three"));
        assert!(!files[0].patch.contains("diff --git"));

        assert_eq!(files[1].path, "new.txt");
        assert!(!files[1].binary);
        assert!(files[1].patch.contains("rename from old.txt"));

        assert_eq!(files[2].path, "img.png");
        assert!(files[2].binary);
    }

    #[test]
    fn split_patch_keeps_the_a_side_for_deletions() {
        let out = "\
diff --git a/gone.txt /dev/null
deleted file mode 100644
index 555..000
--- a/gone.txt
+++ /dev/null
@@ -1 +0,0 @@
-gone
";
        let files = split_patch(out);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, "gone.txt");
        assert!(files[0].patch.contains("-gone"));
    }

    #[test]
    fn strip_side_prefers_b_over_a() {
        assert_eq!(strip_side_prefix("b/x.txt"), "x.txt");
        assert_eq!(strip_side_prefix("a/x.txt"), "x.txt");
        assert_eq!(strip_side_prefix("no-prefix"), "no-prefix");
    }
}
