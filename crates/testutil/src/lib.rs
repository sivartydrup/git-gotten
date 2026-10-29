//! Real, temporary git repository fixtures for the GitGotten test suites.
//!
//! Every fixture is built with the real `git` CLI — the same binary the app shells
//! out to for writes (SPEC §8) — so the on-disk shape is exactly what the `gix`
//! read-model must understand. Fixtures live in unique temp directories and are
//! deleted on drop (along with any remotes they created).
//!
//! Requires a `git` binary on `PATH` (the platform gate of every stage that uses
//! these: macOS primary, Windows where marked).

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

/// Monotonic counter so two fixtures created in the same process never collide.
static SEQ: AtomicU64 = AtomicU64::new(0);

/// Run `git <args>` in `cwd`, returning `(exit_code, stdout, stderr)`.
/// Never panics — call it when a non-zero exit is expected (merge conflicts, …).
pub fn git_run(cwd: &Path, args: &[&str]) -> (i32, String, String) {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn git {:?}: {e}", args));
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// Run `git <args>` in `cwd` and return the stdout with trailing newlines
/// removed (leading whitespace is preserved - `--porcelain` lines start with a
/// space for worktree-only changes). Panics on a non-zero exit: a failed setup
/// step is a broken test, fail loudly.
pub fn git(cwd: &Path, args: &[&str]) -> String {
    let (code, stdout, stderr) = git_run(cwd, args);
    if code != 0 {
        panic!(
            "git {:?} failed (exit {code}) in {:?}\nstdout: {stdout}\nstderr: {stderr}",
            args, cwd
        );
    }
    stdout.trim_end().to_string()
}

/// A temporary git repository. The tree (and any created remotes) is deleted on drop.
#[derive(Debug)]
pub struct Fixture {
    /// Canonical absolute path of the repository root (a.k.a. the `RepoId`).
    pub path: PathBuf,
    /// Remotes created via [`add_origin`](Self::add_origin); deleted when the fixture drops.
    remotes: Vec<RemoteFixture>,
}

/// Canonicalize `dir`, stripping Windows' `\\?\` verbatim prefix.
///
/// `git` parses a `\\?\C:\…` argument as an SCP-style `host:path` remote and
/// fails with "hostname contains invalid characters", so local remotes must be
/// plain absolute paths.
fn canonical_path(dir: &Path) -> PathBuf {
    let path = dir
        .canonicalize()
        .unwrap_or_else(|e| panic!("failed to canonicalize {dir:?}: {e}"));
    #[cfg(windows)]
    {
        let text = path.to_string_lossy();
        if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{rest}"));
        }
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            return PathBuf::from(rest);
        }
    }
    path
}

impl Fixture {
    /// `git init -b main` + a local user identity, in a fresh unique temp dir.
    pub fn new(label: &str) -> Self {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "gitgotten-test-{}-{seq}-{label}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir)
            .unwrap_or_else(|e| panic!("failed to create fixture dir {dir:?}: {e}"));
        git(&dir, &["init", "-b", "main"]);
        // Isolate from the host's global config so commits are deterministic.
        git(&dir, &["config", "user.name", "Test User"]);
        git(&dir, &["config", "user.email", "test@example.com"]);
        git(&dir, &["config", "commit.gpgsign", "false"]);
        git(&dir, &["config", "tag.gpgsign", "false"]);
        // A real, portable no-op editor: `git --version <path>` exits 0 and
        // leaves the file alone. (`true` has no Windows executable.)
        git(&dir, &["config", "core.editor", "git --version"]);
        git(&dir, &["config", "core.autocrlf", "false"]);
        Self {
            path: dir
                .canonicalize()
                .unwrap_or_else(|e| panic!("failed to canonicalize fixture dir {dir:?}: {e}")),
            remotes: Vec::new(),
        }
    }

    /// Wrap an existing repository directory. Unlike [`new`], the directory must
    /// already exist and be a git repo; the fixture's drop still deletes the tree.
    pub fn from_path(path: PathBuf) -> Self {
        Self {
            path,
            remotes: Vec::new(),
        }
    }

    /// Create a bare `origin`, add it to this fixture, and push `branch` with an upstream.
    /// The bare remote stays alive as long as the fixture does.
    pub fn add_origin(&mut self, branch: &str) -> PathBuf {
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("gitgotten-remote-{}-{seq}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (code, _, stderr) = git_run(&dir, &["init", "--bare", "-b", branch]);
        if code != 0 {
            panic!("bare init failed: {stderr}");
        }
        let remote_path = canonical_path(&dir);
        self.git(&["remote", "add", "origin", remote_path.to_str().unwrap()]);
        self.git(&["push", "-u", "origin", branch]);
        self.remotes.push(RemoteFixture {
            path: remote_path.clone(),
        });
        remote_path
    }

    /// Run `git <args>` inside this fixture (panics on non-zero exit).
    pub fn git(&self, args: &[&str]) -> String {
        git(&self.path, args)
    }

    /// Run `git <args>` inside this fixture, allowing non-zero exits.
    pub fn git_run(&self, args: &[&str]) -> (i32, String, String) {
        git_run(&self.path, args)
    }

    /// Write a file in the working tree (creating parents as needed).
    pub fn write_file(&self, rel: &str, content: &str) {
        let full = self.path.join(rel);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).unwrap_or_else(|e| panic!("mkdir for {rel}: {e}"));
        }
        std::fs::write(&full, content).unwrap_or_else(|e| panic!("write {rel}: {e}"));
    }

    /// Write + `git add` + `git commit` a single file in one step.
    pub fn commit_file(&self, rel: &str, content: &str, message: &str) {
        self.write_file(rel, content);
        self.git(&["add", rel]);
        self.git(&["commit", "-m", message]);
    }

    /// Run `git <args>` inside this fixture with additional environment
    /// variables, allowing non-zero exits.
    pub fn git_env_run(&self, env: &[(&str, &str)], args: &[&str]) -> (i32, String, String) {
        let out = Command::new("git")
            .envs(env.iter().copied())
            .args(args)
            .current_dir(&self.path)
            .output()
            .unwrap_or_else(|e| panic!("failed to spawn git {:?}: {e}", args));
        (
            out.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    }

    /// Write + `git add` + `git commit` a file with a pinned timestamp: both
    /// the author and committer dates are set to `ts` (unix seconds, UTC).
    ///
    /// Deterministic commit dates make the `git log --graph` ordering
    /// (committer-date driven) reproducible — the graph crate's oracle tests
    /// rely on it.
    pub fn commit_file_at(&self, rel: &str, content: &str, message: &str, ts: i64) {
        self.write_file(rel, content);
        self.git(&["add", rel]);
        let date = format!("@{ts} +0000");
        let (code, stdout, stderr) = self.git_env_run(
            &[
                ("GIT_AUTHOR_DATE", date.as_str()),
                ("GIT_COMMITTER_DATE", date.as_str()),
            ],
            &["commit", "-m", message],
        );
        if code != 0 {
            panic!(
                "git commit failed (exit {code}) in {:?}\nstdout: {stdout}\nstderr: {stderr}",
                self.path
            );
        }
    }

    /// `git add` the path (staging it).
    pub fn add(&self, rel: &str) {
        self.git(&["add", rel]);
    }

    /// The canonical absolute path as a `&str` (use it as a `RepoId`).
    pub fn id(&self) -> &str {
        self.path.to_str().expect("fixture path is valid UTF-8")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for r in &self.remotes {
            let _ = std::fs::remove_dir_all(&r.path);
        }
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// A bare remote owned by a [`Fixture`].
#[derive(Debug)]
pub struct RemoteFixture {
    /// Canonical path of the bare remote.
    pub path: PathBuf,
}

impl RemoteFixture {
    /// Clone the bare remote directly into `target` (which must be an empty dir) so
    /// that `target` *is* the clone. Used to make commits on the remote side, e.g. to
    /// produce a behind-N state.
    pub fn clone_into(&self, target: &Path) {
        let (code, _, stderr) = git_run(
            target,
            &["clone", "--quiet", &self.path.to_string_lossy(), "."],
        );
        if code != 0 {
            panic!(
                "clone of {path:?} into {target:?} failed: {stderr}",
                path = self.path
            );
        }
    }
}

// ── Named fixtures (the scenarios the stage-1 tests assert on) ─────────────

/// The stage-1 status fixture:
/// committed `a.txt`; `b.txt` modified (unstaged); `c.txt` staged; `d.txt`
/// untracked; on `main` with upstream `origin/main` — local is **ahead 1 /
/// behind 0** (one local commit past the remote).
pub fn status_fixture() -> Fixture {
    let mut fx = Fixture::new("status");
    fx.commit_file("a.txt", "committed\n", "first");
    fx.add_origin("main"); // remote has commit "first"
    fx.commit_file("b.txt", "one\n", "second"); // local ahead by "second"
    fx.write_file("b.txt", "one\ntwo\n"); // unstaged modification
    fx.write_file("c.txt", "staged\n");
    fx.add("c.txt"); // staged addition
    fx.write_file("d.txt", "untracked\n"); // untracked
    fx
}

/// Two commits on `main` (with `origin`), then a detached checkout of the first.
pub fn detached_fixture() -> Fixture {
    let mut fx = Fixture::new("detached");
    fx.commit_file("a.txt", "one\n", "c1");
    fx.add_origin("main");
    fx.commit_file("b.txt", "two\n", "c2");
    let first = fx.git(&["rev-parse", "HEAD~1"]);
    fx.git(&["checkout", "--detach", &first]);
    fx
}

/// `main` and `side` diverge on the same file and are merged, stopping on a
/// conflict: `main` is mid-merge with `f.txt` unmerged.
pub fn merge_conflict_fixture() -> Fixture {
    let fx = Fixture::new("merge-conflict");
    fx.commit_file("f.txt", "base\n", "c1");
    fx.git(&["checkout", "-b", "side"]);
    fx.commit_file("f.txt", "side\n", "c-side");
    fx.git(&["checkout", "main"]);
    fx.commit_file("f.txt", "main\n", "c-main");
    let (code, _, _) = fx.git_run(&["merge", "side"]);
    assert_ne!(code, 0, "merge was expected to conflict");
    fx
}

/// The [`status_fixture`] minus the untracked/staged noise, plus a second commit
/// made *on the remote side* (via a throwaway clone of the bare remote) so local
/// `main` is **ahead 0 / behind 1**.
pub fn behind_fixture() -> Fixture {
    let mut fx = Fixture::new("behind");
    fx.commit_file("a.txt", "committed\n", "first");
    let remote_path = fx.add_origin("main");
    let clone = std::env::temp_dir().join(format!(
        "gitgotten-remote-clone-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&clone).unwrap();
    RemoteFixture {
        path: remote_path.clone(),
    }
    .clone_into(&clone);
    let clone_fx = Fixture {
        path: clone.canonicalize().unwrap(),
        remotes: Vec::new(),
    };
    clone_fx.git(&["config", "user.name", "Remote Bot"]);
    clone_fx.git(&["config", "user.email", "remote@example.com"]);
    clone_fx.commit_file("b.txt", "remote-side\n", "remote second");
    clone_fx.git(&["push", "origin", "main"]);
    std::mem::drop(clone_fx);
    // Refresh the local remote-tracking ref so `origin/main` reflects the push.
    fx.git(&["fetch", "-q", "origin"]);
    fx
}

/// Same divergence as [`merge_conflict_fixture`], but `main` is rebased onto
/// `side` and stops on a conflict: `main` is mid-rebase.
pub fn rebase_conflict_fixture() -> Fixture {
    let fx = Fixture::new("rebase-conflict");
    fx.commit_file("f.txt", "base\n", "c1");
    fx.git(&["checkout", "-b", "side"]);
    fx.commit_file("f.txt", "side\n", "c-side");
    fx.git(&["checkout", "main"]);
    fx.commit_file("f.txt", "main\n", "c-main");
    let (code, _, _) = fx.git_run(&["rebase", "side"]);
    assert_ne!(code, 0, "rebase was expected to conflict");
    fx
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_round_trips_through_git() {
        let mut fx = Fixture::new("roundtrip");
        fx.commit_file("a.txt", "hello\n", "c1");
        fx.add_origin("main");
        fx.write_file("b.txt", "world\n");
        assert_eq!(
            fx.git(&["rev-parse", "HEAD"]),
            fx.git(&["rev-parse", "main"])
        );
        let porcelain = fx.git(&["status", "--porcelain"]);
        assert!(
            porcelain.lines().any(|l| l.trim() == "?? b.txt"),
            "expected untracked b.txt: {porcelain}"
        );
        // Dropping deletes the tree and the remote.
        let path = fx.path.clone();
        let remote = fx.remotes[0].path.clone();
        drop(fx);
        assert!(!path.exists());
        assert!(!remote.exists());
    }

    #[test]
    fn status_fixture_matches_git_porcelain() {
        let fx = status_fixture();
        let porcelain = fx.git(&["status", "--porcelain=v1"]);
        let lines: Vec<&str> = porcelain.lines().collect();
        // Exact XY columns: b is worktree-modified (` M`), c staged-added (`A  `),
        // d untracked (`??`).
        assert!(lines.contains(&" M b.txt"), "b.txt: {porcelain}");
        assert!(lines.contains(&"A  c.txt"), "c.txt: {porcelain}");
        assert!(lines.contains(&"?? d.txt"), "d.txt: {porcelain}");
        assert_eq!(lines.len(), 3, "exactly three changes: {porcelain}");
    }
}
