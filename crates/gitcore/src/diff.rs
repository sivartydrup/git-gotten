//! Working-tree diff access for the staging UI (SPEC §10.2): the raw git
//! invocations behind [`crate::Repo::diff_file`], the hunk parser that turns
//! unified-diff text into [`Hunk`]s (the per-hunk staging unit), and the
//! patch builder that [`crate::Repo`] uses to compile a hunk selection into
//! the stdin patch of `git apply --cached`.
//!
//! All of these are *reads* (or pure text transforms): they run no git
//! command that mutates anything, so they are not `CommandPlan`s (SPEC §8).
//! The git invocations go through the real binary with the same environment
//! hygiene as the executor (SPEC §8) — the `gix` read-model covers the live
//! status, not on-demand diffs.

use std::path::Path;

use git_gotten_protocol::{FileDiff, Hunk};

use crate::GitError;
use crate::show::run_git_sync_allow;

/// The null device, per platform, for `git diff --no-index`.
///
/// Untracked files have no index entry, so `git diff`/`git diff --cached`
/// cannot diff them; `--no-index` against the null device yields the
/// file-vs-nothing diff (every line added).
fn dev_null() -> &'static str {
    if cfg!(windows) { "NUL" } else { "/dev/null" }
}

/// `git diff -- <file>` — worktree vs index (the unstaged side).
///
/// Empty output when the file has no unstaged changes (or is untracked).
fn diff_worktree(cwd: &Path, file: &str) -> Result<String, GitError> {
    run_git_sync_allow(cwd, &["diff", "--", file], -1)
}

/// `git diff --cached -- <file>` — index vs HEAD (the staged side).
fn diff_index(cwd: &Path, file: &str) -> Result<String, GitError> {
    run_git_sync_allow(cwd, &["diff", "--cached", "--", file], -1)
}

/// The diff of an *untracked* file: `git diff --no-index -- <null> <file>`.
///
/// `--no-index` exits `1` when the files differ (its "they are not the same"
/// signal), so exit `1` is tolerated as a successful result.
fn diff_untracked(cwd: &Path, file: &str) -> Result<String, GitError> {
    run_git_sync_allow(cwd, &["diff", "--no-index", "--", dev_null(), file], 1)
}

/// Both sides of a file's diff, as raw unified-diff text:
/// `(staged, unstaged)` — index-vs-HEAD and worktree-vs-index.
///
/// For untracked files the unstaged side is the file-vs-`/dev/null` diff
/// (the file is new, so it has no staged side).
pub(crate) fn diff_sides(
    cwd: &Path,
    file: &str,
    untracked: bool,
) -> Result<(String, String), GitError> {
    let staged = if untracked {
        String::new()
    } else {
        diff_index(cwd, file)?
    };
    let unstaged = if untracked {
        diff_untracked(cwd, file)?
    } else {
        diff_worktree(cwd, file)?
    };
    Ok((staged, unstaged))
}

/// `true` when the diff is git's binary marker instead of text hunks.
pub(crate) fn is_binary(patch: &str) -> bool {
    patch.lines().any(|l| l.starts_with("Binary files "))
}

/// Whether a line is a unified-diff hunk header (`@@ -l,c +l,c @@ [section]`).
fn is_hunk_header(line: &str) -> bool {
    line.starts_with("@@") && line.matches("@@").count() >= 2
}

/// Whether a line is a hunk body line: ` ` (context), `+`, `-`, or the
/// `\ No newline at end of file` marker.
fn is_hunk_body(line: &str) -> bool {
    matches!(line.chars().next(), Some(' ' | '+' | '-' | '\\'))
}

/// Parse the `@@`-delimited hunks of one file's unified-diff text.
///
/// Preamble lines (mode/index/`---`/`+++` — everything before the first
/// `@@`) are skipped; a hunk ends at the next `@@` header or at the first
/// non-body line. `staged` is recorded on every hunk: the parser cannot tell
/// which side of the index a diff came from, so the caller (who ran the diff)
/// sets it.
pub(crate) fn hunks_from(patch: &str, staged: bool) -> Vec<Hunk> {
    let mut hunks: Vec<Hunk> = Vec::new();
    let mut current: Option<Hunk> = None;

    for line in patch.lines() {
        if is_hunk_header(line) {
            if let Some(h) = current.take() {
                hunks.push(h);
            }
            current = Some(Hunk {
                header: line.to_string(),
                lines: Vec::new(),
                staged,
            });
        } else if is_hunk_body(line) {
            if let Some(h) = current.as_mut() {
                h.lines.push(line.to_string());
            }
            // Body lines before the first hunk are preamble; drop them.
        } else {
            current = None; // a non-hunk line (e.g. `Binary files`) ends a hunk
        }
    }
    if let Some(h) = current.take() {
        hunks.push(h);
    }
    hunks
}

/// Build the stdin patch for `git apply --cached`: the raw (single-file)
/// unstaged diff with **only** the selected hunks.
///
/// `raw` must be the diff of *this side* of the index, and `selected` must
/// index into [`hunks_from`(`raw`, …)] (hunk order = order of appearance).
/// The diff preamble (mode/index/`---`/`+++`) is kept verbatim — it is what
/// `git apply` needs to locate the file, and its `index` line lets git verify
/// the preimage still matches the index before applying.
///
/// Returns `None` when the selection is empty or any index is out of range
/// (the file's content changed between reading and staging; the caller
/// re-derives rather than applying a stale patch).
pub(crate) fn build_apply_patch(raw: &str, selected: &[usize]) -> Option<String> {
    if selected.is_empty() {
        return None;
    }
    let lines: Vec<&str> = raw.lines().collect();

    // Locate each hunk's span (header line .. first non-body line after it).
    let mut spans: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if is_hunk_header(lines[i]) {
            let start = i;
            i += 1;
            while i < lines.len() && is_hunk_body(lines[i]) {
                i += 1;
            }
            spans.push((start, i));
        } else {
            i += 1;
        }
    }
    if selected.iter().any(|&s| s >= spans.len()) {
        return None;
    }

    let mut out = String::new();
    let preamble_end = spans.first().map(|(s, _)| *s).unwrap_or(lines.len());
    for line in &lines[..preamble_end] {
        out.push_str(line);
        out.push('\n');
    }
    let mut sel: Vec<usize> = selected.to_vec();
    sel.sort_unstable();
    sel.dedup();
    for &s in &sel {
        for line in &lines[spans[s].0..spans[s].1] {
            out.push_str(line);
            out.push('\n');
        }
    }
    Some(out)
}

/// The full working-tree diff of one file (the return of
/// [`crate::Repo::diff_file`]): both sides parsed into hunks, staged first,
/// then unstaged — the order the web renders them.
pub(crate) fn file_diff(path: &str, staged_raw: &str, unstaged_raw: &str) -> FileDiff {
    let binary = is_binary(staged_raw) || is_binary(unstaged_raw);
    let mut hunks = hunks_from(staged_raw, true);
    hunks.extend(hunks_from(unstaged_raw, false));
    if binary {
        hunks = Vec::new();
    }
    // `patch` is the side the user acts on: the unstaged diff (what the
    // checkboxes stage), falling back to the staged diff when there is none.
    let patch = if unstaged_raw.trim().is_empty() {
        staged_raw.to_string()
    } else {
        unstaged_raw.to_string()
    };
    FileDiff {
        path: path.to_string(),
        binary,
        patch,
        hunks,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
index 1b2a1c5..0d03e65 100644
--- a/f.txt
+++ b/f.txt
@@ -1,4 +1,4 @@
-l1
+l1X
 l2
 l3
 l4
@@ -8,2 +8,3 @@
 l8
+l8X
 l9
";

    #[test]
    fn parses_multiple_hunks_with_their_headers_and_lines() {
        let hunks = hunks_from(SAMPLE, false);
        assert_eq!(hunks.len(), 2);
        assert_eq!(hunks[0].header, "@@ -1,4 +1,4 @@");
        assert_eq!(hunks[0].lines, vec!["-l1", "+l1X", " l2", " l3", " l4"]);
        assert_eq!(hunks[1].header, "@@ -8,2 +8,3 @@");
        assert_eq!(hunks[1].lines, vec![" l8", "+l8X", " l9"]);
        assert!(hunks.iter().all(|h| !h.staged));
    }

    #[test]
    fn records_the_staged_side() {
        let hunks = hunks_from(SAMPLE, true);
        assert!(hunks.iter().all(|h| h.staged));
    }

    #[test]
    fn section_suffix_stays_part_of_the_header() {
        let patch = "@@ -1,2 +1,2 @@ fn main()\n- a\n+ b\n";
        let hunks = hunks_from(patch, false);
        assert_eq!(hunks[0].header, "@@ -1,2 +1,2 @@ fn main()");
    }

    #[test]
    fn new_file_diff_has_one_all_added_hunk() {
        let patch = "\
new file mode 100644
index 0000000..3e75765
--- /dev/null
+++ b/u.txt
@@ -0,0 +1,2 @@
+new
+two
";
        let hunks = hunks_from(patch, false);
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].lines, vec!["+new", "+two"]);
    }

    #[test]
    fn no_newline_marker_is_a_body_line() {
        let patch = "@@ -1,2 +1,2 @@\n a\n-b\n+b\n\\ No newline at end of file\n";
        let hunks = hunks_from(patch, false);
        assert_eq!(hunks[0].lines.len(), 4);
        assert!(hunks[0].lines[3].starts_with('\\'));
    }

    #[test]
    fn binary_marker_is_detected_and_has_no_hunks() {
        let patch = "Binary files a/img.png and b/img.png differ\n";
        assert!(is_binary(patch));
        assert!(hunks_from(patch, false).is_empty());
    }

    #[test]
    fn apply_patch_keeps_the_preamble_and_selected_hunks_only() {
        let patch = build_apply_patch(SAMPLE, &[1]).expect("hunk 1 in range");
        assert!(patch.starts_with("index 1b2a1c5..0d03e65 100644\n--- a/f.txt\n+++ b/f.txt\n"));
        assert!(patch.contains("@@ -8,2 +8,3 @@"));
        assert!(!patch.contains("-l1"));
        assert!(!patch.contains("@@ -1,4 +1,4 @@"));
    }

    #[test]
    fn apply_patch_selects_multiple_hunks_in_order() {
        let patch = build_apply_patch(SAMPLE, &[1, 0]).expect("both in range");
        let first = patch.find("@@ -1,4 +1,4 @@").unwrap();
        let second = patch.find("@@ -8,2 +8,3 @@").unwrap();
        assert!(first < second, "selection order is sorted into file order");
    }

    #[test]
    fn apply_patch_rejects_empty_or_out_of_range_selections() {
        assert!(build_apply_patch(SAMPLE, &[]).is_none());
        assert!(build_apply_patch(SAMPLE, &[2]).is_none());
    }

    #[test]
    fn file_diff_orders_staged_hunks_first_and_chooses_the_unstaged_patch() {
        let staged = "@@ -1,2 +1,2 @@\n-a\n+a2\n";
        let unstaged = "@@ -3,2 +3,2 @@\n-c\n+c2\n";
        let d = file_diff("f.txt", staged, unstaged);
        assert_eq!(d.hunks.len(), 2);
        assert!(d.hunks[0].staged);
        assert!(!d.hunks[1].staged);
        assert_eq!(d.patch, unstaged);
    }

    #[test]
    fn file_diff_falls_back_to_the_staged_patch_when_unstaged_is_empty() {
        let staged = "@@ -1,2 +1,2 @@\n-a\n+a2\n";
        let d = file_diff("f.txt", staged, "");
        assert_eq!(d.patch, staged);
        assert!(d.hunks.iter().all(|h| h.staged));
    }
}
