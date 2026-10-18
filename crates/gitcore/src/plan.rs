//! Plan constructors: compile user operations into transparent [`CommandPlan`]s
//! (SPEC §8 — the transparency primitive).
//!
//! Each `plan_*` reads repository state through the `gix` read-model (branch,
//! upstream, staged files) to fill in the branch names, blast-radius effects, and
//! preview lines. Planning is read-only: it runs no git command and mutates nothing.
//!
//! `display` is the single source of truth for the UI (inline hint, preview dialog,
//! activity log, SPEC §8/§9): it is the exact argv, quoted only where necessary.

use git_gotten_protocol::{ChangeKind, CommandPlan, GitStep, Op, Preview, Risk, Status};

use crate::{GitError, Repo};

/// Join a git argv into the `display` string shown in the UI.
///
/// The `git` program itself is never quoted (it is `argv[0]`, resolved separately).
/// Arguments are wrapped in double quotes only when they are empty or contain
/// whitespace or quotes, so simple commands read `git push origin main` while a
/// message with spaces reads `git commit -m "fix: the thing"`.
pub fn format_argv(argv: &[String]) -> String {
    let mut out = String::new();
    for (i, arg) in argv.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        if arg.is_empty() || arg.contains(' ') || arg.contains('"') {
            out.push('"');
            for c in arg.chars() {
                if c == '"' {
                    out.push_str("\\\"");
                } else {
                    out.push(c);
                }
            }
            out.push('"');
        } else {
            out.push_str(arg);
        }
    }
    out
}

impl Repo {
    /// Compile `op` into a [`CommandPlan`], reading the current repository state.
    ///
    /// Read-only: this touches only the `gix` read-model, never the git CLI. The
    /// same op compiled twice against the same state yields the same plan.
    pub fn plan(&self, op: &Op) -> Result<CommandPlan, GitError> {
        let status = self.status()?;
        match op {
            Op::Commit { message, amend } => plan_commit(&status, message, *amend),
            Op::Checkout { target, force } => plan_checkout(&status, target, *force),
            Op::Push {
                remote,
                branch,
                force,
            } => plan_push(&self.inner, &status, remote, branch.as_deref(), *force),
        }
    }
}

/// `git commit -m <message> [--amend]` — `Mutating`; previews the staged files.
fn plan_commit(status: &Status, message: &str, amend: bool) -> Result<CommandPlan, GitError> {
    if message.trim().is_empty() {
        return Err(GitError::EmptyCommitMessage);
    }

    let mut argv = vec![
        "git".to_string(),
        "commit".to_string(),
        "-m".to_string(),
        message.to_string(),
    ];
    if amend {
        argv.push("--amend".to_string());
    }

    let staged: Vec<&ChangeKind> = status
        .changes
        .iter()
        .filter(|c| c.staged)
        .map(|c| &c.kind)
        .collect();
    let staged_paths: Vec<&str> = status
        .changes
        .iter()
        .filter(|c| c.staged)
        .map(|c| c.path.as_str())
        .collect();

    let mut lines = vec![format!("message: {message}")];
    if staged_paths.is_empty() {
        lines.push("no staged changes — git will fail".to_string());
    } else {
        for (path, kind) in staged_paths.iter().zip(staged.iter()) {
            lines.push(format!("  {path} ({})", kind_label(kind)));
        }
    }
    if amend {
        lines.push("amend: rewrites the last commit".to_string());
    }

    let headline = if staged_paths.is_empty() {
        "Commit (nothing staged)".to_string()
    } else if staged_paths.len() == 1 {
        "Commit 1 file".to_string()
    } else {
        format!("Commit {} files", staged_paths.len())
    };

    let preview = Preview {
        headline,
        lines,
        requires_type_to_confirm: false,
    };
    // The display is derived from the exact argv that will run — the transparency
    // contract (SPEC §8): the UI never shows a command different from what runs.
    let display = format_argv(&argv);

    Ok(CommandPlan {
        steps: vec![GitStep {
            argv,
            summary: if amend {
                "amend the last commit".to_string()
            } else {
                "create a commit".to_string()
            },
            risk: Risk::Mutating,
            effect: amend.then(|| "rewrites the last commit".to_string()),
        }],
        display,
        preview: Some(preview),
    })
}

/// `git switch <target> [--force]` — `Mutating`; previews the from → to move.
fn plan_checkout(status: &Status, target: &str, force: bool) -> Result<CommandPlan, GitError> {
    if target.trim().is_empty() {
        return Err(GitError::EmptyTarget);
    }

    let mut argv = vec!["git".to_string(), "switch".to_string(), target.to_string()];
    if force {
        argv.push("--force".to_string());
    }

    let from = status
        .branch
        .clone()
        .unwrap_or_else(|| format!("detached {}", short_oid(&status.detached_head)));

    let mut lines = vec![format!("from {from} to {target}")];
    if force {
        lines.push("force: discards local changes that would be overwritten".to_string());
    }
    let display = format_argv(&argv);

    Ok(CommandPlan {
        steps: vec![GitStep {
            argv,
            summary: format!("switch to {target}"),
            risk: Risk::Mutating,
            effect: None,
        }],
        display,
        preview: Some(Preview {
            headline: "Switch branch".to_string(),
            lines,
            requires_type_to_confirm: false,
        }),
    })
}

/// `git push [--force] <remote> <branch>` — `Mutating`, or `Dangerous` (type-to-confirm
/// + blast-radius effect) when force-pushing.
fn plan_push(
    repo: &gix::Repository,
    status: &Status,
    remote: &str,
    branch: Option<&str>,
    force: bool,
) -> Result<CommandPlan, GitError> {
    if remote.trim().is_empty() {
        return Err(GitError::UnknownRemote {
            name: remote.to_string(),
        });
    }
    if !repo.remote_names().iter().any(|name| *name == remote) {
        return Err(GitError::UnknownRemote {
            name: remote.to_string(),
        });
    }

    // Default to the checked-out branch; a detached HEAD has no branch to push.
    let branch = branch
        .map(str::to_string)
        .or_else(|| status.branch.clone())
        .ok_or(GitError::DetachedHead)?;

    let mut argv = vec!["git".to_string(), "push".to_string()];
    if force {
        argv.push("--force".to_string());
    }
    argv.push(remote.to_string());
    argv.push(branch.clone());

    let risk = if force {
        Risk::Dangerous
    } else {
        Risk::Mutating
    };

    // Blast radius of a force-push: the remote commits that would be lost.
    let effect = force.then(|| {
        if status.upstream.is_some() && status.behind > 0 {
            format!(
                "force-push `{branch}` — remote loses {} commit{}",
                status.behind,
                if status.behind == 1 { "" } else { "s" }
            )
        } else {
            format!("force-push `{branch}` — remote history will be rewritten")
        }
    });

    let mut lines = Vec::new();
    if force {
        lines.push("remote commits will be overwritten".to_string());
    }
    if status.ahead > 0 {
        lines.push(format!(
            "pushes {} new commit{}",
            status.ahead,
            if status.ahead == 1 { "" } else { "s" }
        ));
    } else {
        lines.push("nothing ahead — no change expected".to_string());
    }
    let display = format_argv(&argv);

    Ok(CommandPlan {
        steps: vec![GitStep {
            argv,
            summary: format!("push {branch} to {remote}"),
            risk,
            effect,
        }],
        display,
        preview: Some(Preview {
            headline: format!("Push to {remote}/{branch}"),
            lines,
            requires_type_to_confirm: force,
        }),
    })
}

/// A short lowercase label for a [`ChangeKind`], for preview lines.
fn kind_label(kind: &ChangeKind) -> &'static str {
    match kind {
        ChangeKind::Modified => "modified",
        ChangeKind::Added => "added",
        ChangeKind::Deleted => "deleted",
        ChangeKind::Renamed => "renamed",
        ChangeKind::Untracked => "untracked",
        ChangeKind::Conflicted => "conflicted",
    }
}

/// First 12 hex digits of a full OID (the human display form).
fn short_oid(oid: &Option<String>) -> String {
    oid.as_deref()
        .map(|o| o.chars().take(12).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use git_gotten_testutil as testutil;

    // ── format_argv (the display contract) ─────────────────────────────────

    #[test]
    fn format_argv_leaves_simple_args_bare() {
        let argv: Vec<String> = ["git", "push", "origin", "main"].into_iter().map(String::from).collect();
        assert_eq!(format_argv(&argv), "git push origin main");
    }

    #[test]
    fn format_argv_quotes_args_with_spaces() {
        let argv = vec![
            "git".into(),
            "commit".into(),
            "-m".into(),
            "fix: the thing".into(),
        ];
        assert_eq!(format_argv(&argv), r#"git commit -m "fix: the thing""#);
    }

    #[test]
    fn format_argv_escapes_embedded_quotes() {
        let argv = vec!["git".into(), "commit".into(), "-m".into(), "say \"hi\"".into()];
        assert_eq!(format_argv(&argv), r#"git commit -m "say \"hi\"""#);
    }

    #[test]
    fn format_argv_quotes_empty_args() {
        let argv = vec!["git".into(), "commit".into(), "-m".into(), String::new()];
        assert_eq!(format_argv(&argv), r#"git commit -m """#);
    }

    // ── commit ─────────────────────────────────────────────────────────────

    fn committed_fixture() -> testutil::Fixture {
        // A committed file plus one *staged* file: `git commit` will succeed.
        let fx = testutil::Fixture::new("plan-commit");
        fx.commit_file("a.txt", "one\n", "c1");
        fx.write_file("b.txt", "two\n");
        fx.add("b.txt");
        fx
    }

    #[test]
    fn plan_commit_has_the_exact_display() {
        let fx = committed_fixture();
        let repo = Repo::open(&fx.path).unwrap();
        let plan = repo
            .plan(&Op::Commit {
                message: "add b".into(),
                amend: false,
            })
            .unwrap();

        assert_eq!(plan.display, r#"git commit -m "add b""#);
        assert_eq!(
            plan.steps[0].argv,
            vec!["git".to_string(), "commit".to_string(), "-m".to_string(), "add b".into()]
        );
        assert_eq!(plan.steps[0].risk, Risk::Mutating);
        assert!(plan.steps[0].effect.is_none());

        let preview = plan.preview.as_ref().unwrap();
        assert!(!preview.requires_type_to_confirm);
        assert_eq!(preview.headline, "Commit 1 file");
        assert!(preview.lines.iter().any(|l| l.contains("b.txt")));
    }

    #[test]
    fn plan_commit_amend_flags_the_history_rewrite() {
        let fx = committed_fixture();
        let repo = Repo::open(&fx.path).unwrap();
        let plan = repo
            .plan(&Op::Commit {
                message: "amend it".into(),
                amend: true,
            })
            .unwrap();

        assert_eq!(plan.display, r#"git commit -m "amend it" --amend"#);
        assert_eq!(plan.steps[0].risk, Risk::Mutating);
        assert!(plan.steps[0].effect.as_deref() == Some("rewrites the last commit"));
        let preview = plan.preview.as_ref().unwrap();
        assert!(
            preview.lines.iter().any(|l| l.contains("rewrites the last commit")),
            "preview must warn about the history rewrite: {preview:?}"
        );
    }

    #[test]
    fn plan_commit_with_nothing_staged_warns_in_the_preview() {
        let fx = testutil::Fixture::new("plan-clean");
        fx.commit_file("a.txt", "one\n", "c1");
        let repo = Repo::open(&fx.path).unwrap();
        let plan = repo
            .plan(&Op::Commit {
                message: "empty".into(),
                amend: false,
            })
            .unwrap();
        let preview = plan.preview.as_ref().unwrap();
        assert!(preview.lines.iter().any(|l| l.contains("no staged changes")));
    }

    #[test]
    fn plan_commit_rejects_an_empty_message() {
        let fx = committed_fixture();
        let repo = Repo::open(&fx.path).unwrap();
        let err = repo
            .plan(&Op::Commit {
                message: "   ".into(),
                amend: false,
            })
            .unwrap_err();
        assert!(matches!(err, GitError::EmptyCommitMessage), "got {err:?}");
    }

    // ── checkout ───────────────────────────────────────────────────────────

    #[test]
    fn plan_checkout_uses_switch_with_the_exact_display() {
        let fx = testutil::Fixture::new("plan-switch");
        fx.commit_file("a.txt", "one\n", "c1");
        let repo = Repo::open(&fx.path).unwrap();
        let plan = repo
            .plan(&Op::Checkout {
                target: "b".into(),
                force: false,
            })
            .unwrap();

        assert_eq!(plan.display, "git switch b");
        assert_eq!(
            plan.steps[0].argv,
            vec!["git".to_string(), "switch".to_string(), "b".to_string()]
        );
        assert_eq!(plan.steps[0].risk, Risk::Mutating);

        let preview = plan.preview.as_ref().unwrap();
        assert!(!preview.requires_type_to_confirm);
        // The main branch of a fresh fixture is the "from" of the move.
        assert!(preview.lines.iter().any(|l| l.contains("from main to b")));
    }

    #[test]
    fn plan_checkout_force_warns_in_the_preview() {
        let fx = testutil::Fixture::new("plan-switch-force");
        fx.commit_file("a.txt", "one\n", "c1");
        let repo = Repo::open(&fx.path).unwrap();
        let plan = repo
            .plan(&Op::Checkout {
                target: "b".into(),
                force: true,
            })
            .unwrap();

        assert_eq!(plan.display, "git switch b --force");
        let preview = plan.preview.as_ref().unwrap();
        assert!(preview.lines.iter().any(|l| l.contains("force")));
    }

    #[test]
    fn plan_checkout_rejects_an_empty_target() {
        let fx = testutil::Fixture::new("plan-switch-empty");
        fx.commit_file("a.txt", "one\n", "c1");
        let repo = Repo::open(&fx.path).unwrap();
        let err = repo
            .plan(&Op::Checkout {
                target: "".into(),
                force: false,
            })
            .unwrap_err();
        assert!(matches!(err, GitError::EmptyTarget), "got {err:?}");
    }

    // ── push ───────────────────────────────────────────────────────────────

    #[test]
    fn plan_push_defaults_to_the_current_branch() {
        // status_fixture: on `main` with upstream `origin/main`, ahead 1.
        let fx = testutil::status_fixture();
        let repo = Repo::open(&fx.path).unwrap();
        let plan = repo
            .plan(&Op::Push {
                remote: "origin".into(),
                branch: None,
                force: false,
            })
            .unwrap();

        assert_eq!(plan.display, "git push origin main");
        assert_eq!(
            plan.steps[0].argv,
            vec![
                "git".to_string(),
                "push".to_string(),
                "origin".to_string(),
                "main".to_string(),
            ]
        );
        assert_eq!(plan.steps[0].risk, Risk::Mutating);
        assert!(plan.steps[0].effect.is_none());

        let preview = plan.preview.as_ref().unwrap();
        assert!(!preview.requires_type_to_confirm);
        assert_eq!(preview.headline, "Push to origin/main");
        assert!(preview.lines.iter().any(|l| l.contains("pushes 1 new commit")));
    }

    #[test]
    fn plan_force_push_is_dangerous_with_type_to_confirm() {
        // behind_fixture: local `main` is behind `origin/main` by 1 — a
        // force-push would lose exactly that remote commit.
        let fx = testutil::behind_fixture();
        let repo = Repo::open(&fx.path).unwrap();
        let plan = repo
            .plan(&Op::Push {
                remote: "origin".into(),
                branch: None,
                force: true,
            })
            .unwrap();

        assert_eq!(plan.display, "git push --force origin main");
        assert_eq!(plan.steps[0].risk, Risk::Dangerous);

        // The blast radius names the commits the remote will lose.
        let effect = plan.steps[0].effect.as_deref().unwrap();
        assert!(effect.contains("loses 1 commit"), "effect was: {effect}");

        let preview = plan.preview.as_ref().unwrap();
        assert!(
            preview.requires_type_to_confirm,
            "force-push must require type-to-confirm"
        );
    }

    #[test]
    fn plan_push_rejects_an_unknown_remote() {
        let fx = testutil::Fixture::new("plan-push-noremote");
        fx.commit_file("a.txt", "one\n", "c1");
        let repo = Repo::open(&fx.path).unwrap();
        let err = repo
            .plan(&Op::Push {
                remote: "origin".into(),
                branch: None,
                force: false,
            })
            .unwrap_err();
        assert!(
            matches!(err, GitError::UnknownRemote { ref name } if name == "origin"),
            "got {err:?}"
        );
    }

    #[test]
    fn plan_push_rejects_a_detached_head() {
        let fx = testutil::detached_fixture();
        let repo = Repo::open(&fx.path).unwrap();
        let err = repo
            .plan(&Op::Push {
                remote: "origin".into(),
                branch: None,
                force: false,
            })
            .unwrap_err();
        assert!(matches!(err, GitError::DetachedHead), "got {err:?}");
    }

    // ── Repo::plan dispatch ────────────────────────────────────────────────

    #[test]
    fn plan_dispatches_each_op_variant() {
        let fx = testutil::status_fixture();
        let repo = Repo::open(&fx.path).unwrap();

        assert_eq!(
            repo.plan(&Op::Commit {
                message: "m".into(),
                amend: false,
            }).unwrap()
            .steps[0]
            .risk,
            Risk::Mutating
        );
        assert_eq!(
            repo.plan(&Op::Checkout {
                target: "x".into(),
                force: false,
            }).unwrap()
            .steps[0]
            .risk,
            Risk::Mutating
        );
        assert_eq!(
            repo.plan(&Op::Push {
                remote: "origin".into(),
                branch: Some("main".into()),
                force: true,
            }).unwrap()
            .steps[0]
            .risk,
            Risk::Dangerous
        );
    }
}
