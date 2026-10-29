//! Conflict resolution engine (SPEC §10.5): the 3-way read of unmerged files
//! (index stages 1/2/3 → base/ours/theirs, plus the parsed diff3 structure),
//! the splice that turns per-hunk choices into resolved content, the
//! whole-file patch that the resolve plan applies, and the detection of the
//! in-progress merge/rebase/cherry-pick that `--continue` / `--abort` target.
//!
//! The 3-way structure is computed with the real `git` binary — the same
//! read-only-exception as [`super::show`] and [`super::diff`] (planning may
//! run read-only git; `CommandPlan`s are for writes). Nothing here mutates
//! the repository: the mutations are the plans built in [`super::plan`].

use std::collections::HashSet;
use std::path::Path;

use git_gotten_protocol::{ConflictFile, ConflictHunk, ConflictSegment, HunkChoice};

use crate::GitError;

/// A stopped, resumable git operation, from the state markers in the git
/// directory (per worktree).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InProgressOp {
    /// `.git/MERGE_HEAD` present (a `git merge` stopped on conflicts).
    Merge,
    /// `.git/rebase-merge` or `.git/rebase-apply` present (a `git rebase`
    /// stopped on conflicts).
    Rebase,
    /// `.git/CHERRY_PICK_HEAD` present (a `git cherry-pick` stopped on
    /// conflicts).
    CherryPick,
}

impl InProgressOp {
    /// The git subcommand that concludes or aborts this operation.
    pub fn label(self) -> &'static str {
        match self {
            InProgressOp::Merge => "merge",
            InProgressOp::Rebase => "rebase",
            InProgressOp::CherryPick => "cherry-pick",
        }
    }
}

/// Detect the in-progress operation from the state markers in `git_dir`.
///
/// Precedence: rebase first (an am-style rebase may also carry a
/// `CHERRY_PICK_HEAD` that belongs to the rebase, not a cherry-pick), then
/// merge, then cherry-pick.
pub fn in_progress_op(git_dir: &Path) -> Option<InProgressOp> {
    if git_dir.join("rebase-merge").is_dir() || git_dir.join("rebase-apply").is_dir() {
        return Some(InProgressOp::Rebase);
    }
    if git_dir.join("MERGE_HEAD").exists() {
        return Some(InProgressOp::Merge);
    }
    if git_dir.join("CHERRY_PICK_HEAD").exists() {
        return Some(InProgressOp::CherryPick);
    }
    None
}

/// The environment hygiene for read-only git subprocesses (mirrors the
/// executor's: no leaked `GIT_DIR`-family variables, no pager, fail fast on
/// credential prompts).
const ENV_UNSET: &[&str; 6] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_NAMESPACE",
];
const ENV_SET: &[(&str, &str)] = &[
    ("GIT_TERMINAL_PROMPT", "0"),
    ("PAGER", "cat"),
    ("GIT_PAGER", "cat"),
    ("GIT_EDITOR", "true"),
    ("GIT_MERGE_VERBOSITY", "1"),
];

/// Run `git <args>` synchronously in `cwd`, capturing **bytes** (conflict
/// content may be binary; the text/UTF-8 validation happens on the caller).
///
/// `allow_exit` tolerates one specific non-zero exit code as data (like
/// `show::run_git_sync_allow`): `git merge-file` exits 1 when a conflict is
/// present, which is the expected result, not an error.
pub(crate) fn git_bytes(cwd: &Path, args: &[&str], allow_exit: i32) -> Result<Vec<u8>, GitError> {
    let program = super::exec::resolve_git()?;
    let mut cmd = std::process::Command::new(&program);
    cmd.args(args);
    cmd.current_dir(cwd);
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    cmd.stdin(std::process::Stdio::null());
    for (key, value) in ENV_SET {
        cmd.env(key, value);
    }
    for key in ENV_UNSET {
        cmd.env_remove(key);
    }
    let output = cmd.output().map_err(|e| GitError::Command {
        exit: -1,
        detail: e.to_string(),
    })?;
    let exit = output.status.code().unwrap_or(-1);
    if exit != 0 && exit != allow_exit {
        return Err(GitError::Command {
            exit,
            detail: String::from_utf8_lossy(&output.stderr).into_owned(),
        });
    }
    Ok(output.stdout)
}

/// The stages present for an unmerged path, from `git ls-files -u`.
///
/// Each line is `<mode> <sha> <stage>\t<path>`; the stage column is the
/// unmerged entry's stage (1 = base, 2 = ours, 3 = theirs).
fn stages_present(cwd: &Path, path: &str) -> Result<HashSet<u32>, GitError> {
    let bytes = git_bytes(cwd, &["ls-files", "-u", "--", path], 0)?;
    let out = String::from_utf8_lossy(&bytes);
    let mut stages = HashSet::new();
    for line in out.lines() {
        // `<mode> <sha> <stage>\t<path>` — the stage sits after the sha,
        // before the tab that introduces the path (paths may contain spaces).
        let meta = line.split('\t').next().unwrap_or(line);
        let mut fields = meta.split(' ');
        let _mode = fields.next();
        let _sha = fields.next();
        let stage = fields
            .next()
            .and_then(|s| s.trim().parse::<u32>().ok())
            .unwrap_or(u32::MAX);
        stages.insert(stage);
    }
    Ok(stages)
}

/// The three side versions of an unmerged path (base, ours, theirs) read from
/// index stages 1/2/3 via `git show :N:<path>`.
///
/// A missing stage (the file was deleted on that side) yields an empty
/// string; a non-UTF-8 side (binary content) marks `binary`.
fn read_sides(cwd: &Path, path: &str) -> Result<(String, String, String, bool), GitError> {
    let stages = stages_present(cwd, path)?;
    let mut binary = false;
    let mut side = |stage: u32| -> String {
        if !stages.contains(&stage) {
            return String::new();
        }
        let spec = format!(":{stage}:{path}");
        let bytes = match git_bytes(cwd, &["show", &spec], 128) {
            Ok(b) => b,
            // The stage vanished between listing and reading (the conflict was
            // resolved externally): treat the side as empty, not an error.
            Err(_) => return String::new(),
        };
        match String::from_utf8(bytes) {
            Ok(s) => s,
            Err(_) => {
                binary = true;
                String::new()
            }
        }
    };
    let base = side(1);
    let ours = side(2);
    let theirs = side(3);
    Ok((base, ours, theirs, binary))
}

/// Compute the 3-way segment structure of a file with the three side versions,
/// using `git merge-file --diff3 -p` on temp copies (the diff3 alignment is
/// git's own; the temp files are scratch, the repository is untouched).
///
/// Exit `1` is the normal "conflicts present" result; `0` (the sides agree)
/// yields a single common segment. The output is parsed into segments with
/// byte-exact content.
fn diff3_segments(
    _cwd: &Path,
    base: &str,
    ours: &str,
    theirs: &str,
) -> Result<Vec<ConflictSegment>, GitError> {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("gitgotten-conflict-{}-{seq}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| GitError::Command {
        exit: -1,
        detail: e.to_string(),
    })?;
    let result = (|| {
        for (name, content) in [("ours", ours), ("base", base), ("theirs", theirs)] {
            std::fs::write(dir.join(name), content).map_err(|e| GitError::Command {
                exit: -1,
                detail: e.to_string(),
            })?;
        }
        let out = git_bytes(
            &dir,
            &["merge-file", "--diff3", "-p", "ours", "base", "theirs"],
            1,
        )?;
        Ok(parse_diff3(&String::from_utf8_lossy(&out)))
    })();
    let _ = std::fs::remove_dir_all(&dir); // best-effort scratch cleanup
    result
}

/// The marker-delimited blocks of a diff3 output.
#[derive(Clone, Copy, PartialEq, Eq)]
enum D3Block {
    Common,
    Ours,
    Base,
    Theirs,
}

/// Parse `git merge-file --diff3 -p` output into [`ConflictSegment`]s.
///
/// The markers (with their file-name labels) delimit the blocks: `<<<<<<<`
/// starts the ours block, `|||||||` the base block, `=======` the theirs
/// block, `>>>>>>>` the end of the conflict. Line endings are preserved
/// exactly (`split_inclusive`), so the segment text concatenates back to the
/// source content byte-for-byte.
///
/// Base section: git omits the `|||||||` marker entirely when a region's base
/// equals the ours block (that region auto-merges, so no real unmerged file
/// shows it — but a base that is *empty* does show an empty `|||||||`
/// section). The `saw_base` flag keeps the two cases distinct: the region's
/// base is the base block when the marker was seen, else the ours block.
pub(crate) fn parse_diff3(out: &str) -> Vec<ConflictSegment> {
    let mut segments = Vec::new();
    let mut common: Vec<&str> = Vec::new();
    let mut ours: Vec<&str> = Vec::new();
    let mut base: Vec<&str> = Vec::new();
    let mut theirs: Vec<&str> = Vec::new();
    let mut block = D3Block::Common;
    let mut saw_base = false;

    let flush_common = |segments: &mut Vec<ConflictSegment>, common: &mut Vec<&str>| {
        if !common.is_empty() {
            segments.push(ConflictSegment::Common(common.concat()));
            common.clear();
        }
    };

    for line in out.split_inclusive('\n') {
        if line.starts_with("<<<<<<<") {
            block = D3Block::Ours;
            saw_base = false;
        } else if line.starts_with("|||||||") {
            block = D3Block::Base;
            saw_base = true;
        } else if line.starts_with("=======") {
            block = D3Block::Theirs;
        } else if line.starts_with(">>>>>>>") {
            let base_text = if saw_base {
                base.concat()
            } else {
                ours.concat()
            };
            let ours_text = ours.concat();
            let theirs_text = theirs.concat();
            flush_common(&mut segments, &mut common);
            segments.push(ConflictSegment::Conflict(ConflictHunk {
                base: base_text,
                ours: ours_text,
                theirs: theirs_text,
            }));
            ours.clear();
            base.clear();
            theirs.clear();
            block = D3Block::Common;
        } else {
            match block {
                D3Block::Common => common.push(line),
                D3Block::Ours => ours.push(line),
                D3Block::Base => base.push(line),
                D3Block::Theirs => theirs.push(line),
            }
        }
    }
    // An unterminated block (malformed output) is not a conflict: the
    // accumulated block lines are dropped rather than mislabeled as common.
    flush_common(&mut segments, &mut common);
    segments
}

/// The resolved content of a file: each segment contributes its common text,
/// or the chosen content of its conflict region.
///
/// `choices` must index the `Conflict` segments in order (one per region);
/// a count mismatch is an error (the file's conflict changed between viewing
/// and resolving — the web re-reads rather than applying stale choices).
pub(crate) fn resolved_content(
    segments: &[ConflictSegment],
    choices: &[HunkChoice],
) -> Result<String, GitError> {
    let expected = segments
        .iter()
        .filter(|s| matches!(s, ConflictSegment::Conflict(_)))
        .count();
    if choices.len() != expected {
        return Err(GitError::ConflictMismatch {
            expected,
            given: choices.len(),
        });
    }
    let mut choice_idx = 0usize;
    let mut out = String::new();
    for segment in segments {
        match segment {
            ConflictSegment::Common(text) => out.push_str(text),
            ConflictSegment::Conflict(hunk) => {
                let choice = &choices[choice_idx];
                choice_idx += 1;
                match choice {
                    HunkChoice::AcceptOurs => out.push_str(&hunk.ours),
                    HunkChoice::AcceptTheirs => out.push_str(&hunk.theirs),
                    HunkChoice::AcceptBoth => {
                        out.push_str(&hunk.ours);
                        out.push_str(&hunk.theirs);
                    }
                    HunkChoice::Custom(text) => out.push_str(text),
                }
            }
        }
    }
    Ok(out)
}

/// A whole-file unified-diff patch replacing `old` with `new` at `path` — the
/// stdin patch of the resolve plan's `git apply` step.
///
/// Built by hand (one hunk covering the whole file): with an exact preimage
/// there is nothing to fuzzy, and a mismatched preimage makes `git apply`
/// fail loudly (the file changed between viewing and resolving — safe
/// failure, SPEC §8). `None` when `old == new` (nothing to apply).
///
/// A missing old file (deleted in the worktree) diffs against the null
/// device, like an untracked file.
pub(crate) fn whole_file_patch(path: &str, old: Option<&str>, new: &str) -> Option<String> {
    let old = old.unwrap_or("");
    if old == new {
        return None;
    }
    let old_lines: Vec<&str> = old.split_inclusive('\n').collect();
    let new_lines: Vec<&str> = new.split_inclusive('\n').collect();
    let old_count = old_lines.len();
    let new_count = new_lines.len();
    let old_has_final_newline = old.is_empty() || old.ends_with('\n');
    let new_has_final_newline = new.is_empty() || new.ends_with('\n');

    let mut patch = String::new();
    patch.push_str(&format!(
        "--- {}\n",
        if old.is_empty() {
            "/dev/null".to_string()
        } else {
            format!("a/{path}")
        }
    ));
    patch.push_str(&format!("+++ b/{path}\n"));
    patch.push_str(&format!(
        "@@ -{},{} +{},{} @@\n",
        if old.is_empty() { 0 } else { 1 },
        old_count,
        if new.is_empty() { 0 } else { 1 },
        new_count
    ));
    for line in &old_lines {
        patch.push('-');
        patch.push_str(line.strip_suffix('\n').unwrap_or(line));
        patch.push('\n');
    }
    for line in &new_lines {
        patch.push('+');
        patch.push_str(line.strip_suffix('\n').unwrap_or(line));
        patch.push('\n');
    }
    if !old.is_empty() && !old_has_final_newline {
        patch.push_str("\\ No newline at end of file\n");
    }
    if !new.is_empty() && !new_has_final_newline {
        patch.push_str("\\ No newline at end of file\n");
    }
    Some(patch)
}

/// The external-editor argv (`argv[0]` + its arguments): the repository's
/// `core.editor` if set, else `$VISUAL`, else `$EDITOR`, else `vi` (git's own
/// fallback). A value with whitespace is split into tokens (a `core.editor`
/// of `code --wait` launches `code` with `--wait`).
pub(crate) fn editor_argv(cwd: &Path) -> Vec<String> {
    let cfg = git_bytes(cwd, &["config", "core.editor"], 1)
        .map(|b| String::from_utf8_lossy(&b).trim().to_string())
        .unwrap_or_default();
    let spec = if !cfg.is_empty() {
        cfg
    } else if let Ok(v) = std::env::var("VISUAL") {
        if v.trim().is_empty() {
            std::env::var("EDITOR").unwrap_or_default()
        } else {
            v
        }
    } else {
        std::env::var("EDITOR").unwrap_or_default()
    };
    let spec = spec.trim();
    if spec.is_empty() {
        vec!["vi".to_string()]
    } else {
        spec.split_whitespace().map(String::from).collect()
    }
}

/// The full 3-way view of one unmerged path (the `load_conflict` read): the
/// three side versions plus the parsed diff3 segments.
///
/// `binary` is the status's sniff of the worktree file; a non-UTF-8 side
/// found in the stages marks it binary too. Binary files carry no text
/// fields or segments — only a whole-side resolution is possible.
pub(crate) fn conflict_file(
    cwd: &Path,
    path: &str,
    binary: bool,
) -> Result<ConflictFile, GitError> {
    let (base, ours, theirs, sides_binary) = read_sides(cwd, path)?;
    let binary = binary || sides_binary;
    if binary {
        return Ok(ConflictFile {
            path: path.to_string(),
            binary: true,
            base: String::new(),
            ours: String::new(),
            theirs: String::new(),
            segments: Vec::new(),
        });
    }
    let segments = diff3_segments(cwd, &base, &ours, &theirs)?;
    Ok(ConflictFile {
        path: path.to_string(),
        binary: false,
        base,
        ours,
        theirs,
        segments,
    })
}

/// The stdin patch of the resolve plan for `path` with `choices`: re-derive
/// the 3-way segments, splice the choices, and diff the result against the
/// current worktree file.
///
/// `None` when the worktree file already equals the resolved content (the
/// plan then stages it without an apply step).
pub(crate) fn resolve_patch(
    cwd: &Path,
    path: &str,
    choices: &[HunkChoice],
) -> Result<Option<String>, GitError> {
    let (base, ours, theirs, _) = read_sides(cwd, path)?;
    let segments = diff3_segments(cwd, &base, &ours, &theirs)?;
    let resolved = resolved_content(&segments, choices)?;
    let old = std::fs::read_to_string(cwd.join(path)).ok();
    Ok(whole_file_patch(path, old.as_deref(), &resolved))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_diff3_splits_common_and_conflict_blocks() {
        let out = "keep one\n\
<<<<<<< ours\n\
ours line\n\
||||||| base\n\
base line\n\
=======\n\
theirs line\n\
>>>>>>> theirs\n\
keep two\n";
        let segments = parse_diff3(out);
        assert_eq!(segments.len(), 3);
        assert!(matches!(&segments[0], ConflictSegment::Common(t) if t == "keep one\n"));
        match &segments[1] {
            ConflictSegment::Conflict(h) => {
                assert_eq!(h.ours, "ours line\n");
                assert_eq!(h.base, "base line\n");
                assert_eq!(h.theirs, "theirs line\n");
            }
            other => panic!("expected a conflict region, got {other:?}"),
        }
        assert!(matches!(&segments[2], ConflictSegment::Common(t) if t == "keep two\n"));
    }

    #[test]
    fn parse_diff3_treats_missing_base_as_ours() {
        // git omits the `|||||||` section when base == ours on a region.
        let out = "<<<<<<<\n\
same\n\
=======\n\
other\n\
>>>>>>>\n";
        let segments = parse_diff3(out);
        assert_eq!(segments.len(), 1);
        match &segments[0] {
            ConflictSegment::Conflict(h) => {
                assert_eq!(h.ours, "same\n");
                assert_eq!(h.base, "same\n");
                assert_eq!(h.theirs, "other\n");
            }
            other => panic!("expected a conflict region, got {other:?}"),
        }
    }

    #[test]
    fn parse_diff3_plain_file_is_one_common_segment() {
        let segments = parse_diff3("a\nb\n");
        assert_eq!(segments.len(), 1);
        assert!(matches!(&segments[0], ConflictSegment::Common(t) if t == "a\nb\n"));
    }

    #[test]
    fn resolved_content_splices_the_choices() {
        let segments = parse_diff3("pre\n<<<<<<<\nO\n|||||||\nB\n=======\nT\n>>>>>>>\npost\n");
        let ok = resolved_content(&segments, &[HunkChoice::AcceptBoth]).unwrap();
        assert_eq!(ok, "pre\nO\nT\npost\n");

        let ok = resolved_content(&segments, &[HunkChoice::Custom("X\n".into())]).unwrap();
        assert_eq!(ok, "pre\nX\npost\n");

        let err = resolved_content(&segments, &[]).unwrap_err();
        assert!(matches!(
            err,
            GitError::ConflictMismatch {
                expected: 1,
                given: 0
            }
        ));
    }

    #[test]
    fn whole_file_patch_round_trips_through_git_apply() {
        // A real repository: the patch must apply cleanly to the preimage.
        let fx = git_gotten_testutil::Fixture::new("patch");
        fx.write_file("f.txt", "old line\nkept\n");
        let patch =
            whole_file_patch("f.txt", Some("old line\nkept\n"), "new line\nkept\n").unwrap();
        let patch_path = fx.path.join("p.patch");
        std::fs::write(&patch_path, &patch).unwrap();
        // The patch path stays relative: git on Windows parses a verbatim
        // `\\?\…` argument as an SCP-style remote instead of a local file.
        let (code, _, stderr) = fx.git_run(&["apply", "-p1", "p.patch"]);
        assert_eq!(code, 0, "the whole-file patch must apply: {stderr}");
        assert_eq!(
            std::fs::read_to_string(fx.path.join("f.txt")).unwrap(),
            "new line\nkept\n"
        );
    }

    #[test]
    fn whole_file_patch_flags_missing_final_newline() {
        let patch = whole_file_patch("f.txt", Some("a\nb"), "x\ny").unwrap();
        assert!(patch.contains(r"\ No newline at end of file"), "{patch}");
    }

    #[test]
    fn whole_file_patch_is_none_when_unchanged() {
        assert!(whole_file_patch("f.txt", Some("a\n"), "a\n").is_none());
    }

    #[test]
    fn in_progress_op_reads_the_state_markers() {
        let dir = std::env::temp_dir().join(format!("gitgotten-opstate-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(in_progress_op(&dir), None);
        std::fs::write(dir.join("CHERRY_PICK_HEAD"), "abc\n").unwrap();
        assert_eq!(in_progress_op(&dir), Some(InProgressOp::CherryPick));
        std::fs::write(dir.join("MERGE_HEAD"), "abc\n").unwrap();
        assert_eq!(in_progress_op(&dir), Some(InProgressOp::Merge));
        std::fs::create_dir_all(dir.join("rebase-merge")).unwrap();
        // The rebase markers win: an am-style rebase may also carry a
        // CHERRY_PICK_HEAD that belongs to the rebase.
        assert_eq!(in_progress_op(&dir), Some(InProgressOp::Rebase));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
