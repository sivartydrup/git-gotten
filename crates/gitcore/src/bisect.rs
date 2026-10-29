//! Stage 11 read model (SPEC §10.9): the live bisect session — `git bisect log`
//! parsed into known good/bad commits, the culprit line when converged, the
//! candidate under test (the bisect checkout *is* HEAD), and the count of
//! candidates still untested.

use std::path::Path;

use git_gotten_protocol::BisectState;

use crate::{GitError, show};

/// A session is running when the per-worktree `BISECT_LOG` marker exists
/// (`git bisect log` itself errors when inactive).
pub(crate) fn is_running(cwd: &Path) -> Result<bool, GitError> {
    let git_dir = show::run_git_sync(cwd, &["rev-parse", "--absolute-git-dir"])?;
    Ok(Path::new(git_dir.trim()).join("BISECT_LOG").is_file())
}

/// The session state, or `active: false` outside one.
pub(crate) fn state(cwd: &Path) -> Result<BisectState, GitError> {
    if !is_running(cwd)? {
        return Ok(BisectState {
            active: false,
            current: None,
            good: Vec::new(),
            bad: Vec::new(),
            remaining: 0,
            found: None,
            log: Vec::new(),
        });
    }
    let log = show::run_git_sync(cwd, &["bisect", "log"])?;
    let mut good = Vec::new();
    let mut bad = Vec::new();
    let mut found = None;
    for line in log.lines() {
        if let Some(rest) = line.strip_prefix("# good: ") {
            push_once(&mut good, rest);
        } else if let Some(rest) = line.strip_prefix("# bad: ") {
            push_once(&mut bad, rest);
        } else if let Some(rest) = line.strip_prefix("# first bad commit: ") {
            found = bracketed_hash(rest);
        }
    }
    let current = show::run_git_sync(cwd, &["rev-parse", "HEAD"])?
        .trim()
        .to_string();
    let remaining = match &found {
        Some(_) => 0,
        None => untested_candidates(cwd, &bad, &good)?,
    };
    Ok(BisectState {
        active: true,
        current: Some(current),
        good,
        bad,
        remaining,
        found,
        log: log.lines().map(str::to_string).collect(),
    })
}

fn push_once(list: &mut Vec<String>, rest: &str) {
    if let Some(hash) = bracketed_hash(rest)
        && !list.contains(&hash)
    {
        list.push(hash);
    }
}

/// Pull the object id out of git's `# good: [<hash> <subject>]`-style record.
fn bracketed_hash(rest: &str) -> Option<String> {
    let inner = rest.strip_prefix('[')?;
    let (hash, _subject) = inner.split_once(']')?;
    hash.bytes()
        .all(|b| b.is_ascii_hexdigit())
        .then(|| hash.to_string())
}

/// Candidates git still has to test: everything reachable from the original bad
/// tip that no known-good commit reaches, minus the commits already verdicted
/// bad (git treats those as decided).
fn untested_candidates(cwd: &Path, bad: &[String], good: &[String]) -> Result<u64, GitError> {
    let (Some(tip), Some(_)) = (bad.first(), good.first()) else {
        return Ok(0);
    };
    let mut argv: Vec<&str> = vec!["rev-list", "--count", tip.as_str(), "--not"];
    argv.extend(good.iter().map(String::as_str));
    let out = show::run_git_sync(cwd, &argv)?;
    let reachable: u64 = out.trim().parse().unwrap_or(0);
    Ok(reachable.saturating_sub(bad.len() as u64))
}
