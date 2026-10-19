//! Plan constructors: compile user operations into transparent [`CommandPlan`]s
//! (SPEC §8 — the transparency primitive).
//!
//! Each `plan_*` reads repository state through the `gix` read-model (branch,
//! upstream, staged files) to fill in the branch names, blast-radius effects, and
//! preview lines. Planning is read-only: it runs no git command and mutates nothing.
//!
//! `display` is the single source of truth for the UI (inline hint, preview dialog,
//! activity log, SPEC §8/§9): it is the exact argv, quoted only where necessary.

use git_gotten_protocol::{ChangeKind, CommandPlan, GitStep, Op, Preview, ResetMode, Risk, Status};

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
            Op::BranchCreate {
                name,
                from,
                checkout,
            } => plan_branch_create(self, name, from, *checkout),
            Op::Reset { mode, target } => plan_reset(self, &status, *mode, target),
            Op::CherryPick { hashes } => plan_cherry_pick(self, &status, hashes),
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
/// Ref-name characters git rejects in a branch name (a leading `-` is checked
/// separately; a name may not be empty or contain whitespace/control chars).
fn invalid_branch_name(name: &str) -> bool {
    if name.is_empty()
        || name.starts_with('-')
        || name.ends_with('.')
        || name.contains("//")
        || name.chars().any(|c| {
            c.is_whitespace()
                || matches!(
                    c,
                    ':' | '~' | '^' | '?' | '*' | '[' | '\\' | '@' | '{' | '}'
                )
        })
    {
        return true;
    }
    // Path traversal or dot components are not valid refname segments.
    name.split('/').any(|component| {
        component.is_empty() || component == "." || component == ".." || component.starts_with('.')
    })
}

/// First 12 hex digits of a hash (the human display form).
fn short_hash(hash: &str) -> String {
    hash.chars().take(12).collect()
}

/// The checked-out branch, or a `detached <short-oid>` label (preview lines).
fn from_label(status: &Status) -> String {
    status
        .branch
        .clone()
        .unwrap_or_else(|| format!("detached {}", short_oid(&status.detached_head)))
}

/// `git branch <name> <from>` (+ optional `git switch <name>`) — `Mutating`.
///
/// The commit the branch points at must exist (a graph context-menu pick always
/// does; a stale hash would only fail at run time). If the branch name is taken,
/// the preview says so — git still refuses with a clear error.
fn plan_branch_create(
    repo: &Repo,
    name: &str,
    from: &str,
    checkout: bool,
) -> Result<CommandPlan, GitError> {
    if invalid_branch_name(name) {
        return Err(GitError::BadBranchName {
            name: name.to_string(),
        });
    }
    let from_commit = repo.commit_at(from)?;
    let from_subject = from_commit
        .message()
        .map_err(|e| GitError::Object(e.to_string()))?
        .summary()
        .to_string();

    let mut steps = vec![GitStep {
        argv: vec![
            "git".to_string(),
            "branch".to_string(),
            name.to_string(),
            from.to_string(),
        ],
        summary: format!("create branch {name} at {}", short_hash(from)),
        risk: Risk::Mutating,
        effect: None,
    }];
    if checkout {
        steps.push(GitStep {
            argv: vec!["git".to_string(), "switch".to_string(), name.to_string()],
            summary: format!("switch to {name}"),
            risk: Risk::Mutating,
            effect: None,
        });
    }

    let mut lines = vec![format!("at {}: {from_subject}", short_hash(from))];
    if checkout {
        lines.push("then switch to the new branch".to_string());
    }
    let exists_name = format!("refs/heads/{name}");
    let exists = repo
        .inner
        .find_reference(exists_name.as_str())
        .is_ok();
    if exists {
        lines.push(format!("branch `{name}` already exists — git will fail"));
    }
    let display = if steps.len() == 1 {
        format_argv(&steps[0].argv)
    } else {
        format!(
            "{} && {}",
            format_argv(&steps[0].argv),
            format_argv(&steps[1].argv)
        )
    };

    Ok(CommandPlan {
        steps,
        display,
        preview: Some(Preview {
            headline: format!("Create branch {name}"),
            lines,
            requires_type_to_confirm: false,
        }),
    })
}

/// `git reset --<mode> <target>` — `Mutating` for `soft`/`mixed`, `Dangerous`
/// (type-to-confirm) for `hard`: only the hard variant destroys uncommitted work.
fn plan_reset(
    repo: &Repo,
    status: &Status,
    mode: ResetMode,
    target: &str,
) -> Result<CommandPlan, GitError> {
    let head_id = repo.inner.head().ok().and_then(|h| h.id());
    if head_id.is_none() {
        return Err(GitError::NoCommits);
    }
    let target_commit = repo.commit_at(target)?;
    let target_subject = target_commit
        .message()
        .map_err(|e| GitError::Object(e.to_string()))?
        .summary()
        .to_string();

    let (flag, risk, mode_note) = match mode {
        ResetMode::Soft => (
            "--soft",
            Risk::Mutating,
            "moves the branch pointer; the index and working tree are untouched",
        ),
        ResetMode::Mixed => (
            "--mixed",
            Risk::Mutating,
            "moves the branch pointer; the difference reappears unstaged",
        ),
        ResetMode::Hard => (
            "--hard",
            Risk::Dangerous,
            "moves the branch pointer; the index and working tree are discarded",
        ),
    };

    let argv = vec![
        "git".to_string(),
        "reset".to_string(),
        flag.to_string(),
        target.to_string(),
    ];
    let mut lines = vec![format!(
        "from {} to {}: {target_subject}",
        from_label(status),
        short_hash(target)
    )];
    lines.push(mode_note.to_string());
    let hard = matches!(mode, ResetMode::Hard);
    let effect = hard.then(|| "uncommitted changes will be discarded".to_string());

    Ok(CommandPlan {
        steps: vec![GitStep {
            argv,
            summary: format!("reset to {target}"),
            risk,
            effect,
        }],
        display: format_argv(&[
            "git".to_string(),
            "reset".to_string(),
            flag.to_string(),
            target.to_string(),
        ]),
        preview: Some(Preview {
            headline: format!("Reset to {}", short_hash(target)),
            lines,
            requires_type_to_confirm: hard,
        }),
    })
}

/// `git cherry-pick <hash>…` — `Mutating`, applied onto the current branch.
///
/// Every hash must exist. A conflicting pick fails at run time with git's
/// output (safe failure, SPEC §8); the in-app conflict flow lands in Stage 8.
fn plan_cherry_pick(
    repo: &Repo,
    status: &Status,
    hashes: &[String],
) -> Result<CommandPlan, GitError> {
    if hashes.is_empty() {
        return Err(GitError::EmptySelection);
    }
    let mut lines = Vec::new();
    let mut argv = vec!["git".to_string(), "cherry-pick".to_string()];
    for hash in hashes {
        let commit = repo.commit_at(hash)?;
        let subject = commit
            .message()
            .map_err(|e| GitError::Object(e.to_string()))?
            .summary()
            .to_string();
        lines.push(format!("  {}: {subject}", short_hash(hash)));
        argv.push(hash.clone());
    }
    lines.push(format!(
        "onto {}",
        status
            .branch
            .clone()
            .unwrap_or_else(|| "the detached HEAD".to_string())
    ));
    let display = format_argv(&argv);
    let count = hashes.len();

    Ok(CommandPlan {
        steps: vec![GitStep {
            argv,
            summary: "cherry-pick onto the current branch".to_string(),
            risk: Risk::Mutating,
            effect: None,
        }],
        display,
        preview: Some(Preview {
            headline: format!(
                "Cherry-pick {count} commit{}",
                if count == 1 { "" } else { "s" }
            ),
            lines,
            requires_type_to_confirm: false,
        }),
    })
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

        // The Stage 4 graph context-menu ops dispatch and classify.
        let head = fx.git(&["rev-parse", "HEAD"]);
        assert_eq!(
            repo.plan(&Op::BranchCreate {
                name: "feature".into(),
                from: head.clone(),
                checkout: false
            }).unwrap()
            .steps[0]
            .risk,
            Risk::Mutating
        );
        assert_eq!(
            repo.plan(&Op::Reset {
                mode: ResetMode::Hard,
                target: head.clone()
            }).unwrap()
            .steps[0]
            .risk,
            Risk::Dangerous
        );
        assert_eq!(
            repo.plan(&Op::CherryPick {
                hashes: vec![head]
            })
            .unwrap()
            .steps[0]
            .risk,
            Risk::Mutating
        );
    }

    // ── branch_create ──────────────────────────────────────────────────────

    fn branch_fixture() -> (testutil::Fixture, Repo) {
        let fx = testutil::Fixture::new("plan-branch");
        fx.commit_file("a.txt", "one\n", "c1");
        fx.commit_file("b.txt", "two\n", "c2");
        let repo = Repo::open(&fx.path).unwrap();
        (fx, repo)
    }

    #[test]
    fn plan_branch_create_has_the_exact_display() {
        let (fx, repo) = branch_fixture();
        let head = fx.git(&["rev-parse", "HEAD"]);
        let plan = repo
            .plan(&Op::BranchCreate {
                name: "feature".into(),
                from: head.clone(),
                checkout: false,
            })
            .unwrap();

        assert_eq!(plan.display, format!("git branch feature {head}"));
        assert_eq!(
            plan.steps[0].argv,
            vec![
                "git".to_string(),
                "branch".to_string(),
                "feature".to_string(),
                head
            ]
        );
        assert_eq!(plan.steps[0].risk, Risk::Mutating);

        let preview = plan.preview.as_ref().unwrap();
        assert_eq!(preview.headline, "Create branch feature");
        assert!(!preview.requires_type_to_confirm);
        // The preview names the commit the branch will point at.
        assert!(
            preview.lines.iter().any(|l| l.contains("c2")),
            "preview should show the target commit: {preview:?}"
        );
    }

    #[test]
    fn plan_branch_create_checkout_adds_a_switch_step() {
        let (fx, repo) = branch_fixture();
        let head = fx.git(&["rev-parse", "HEAD"]);
        let plan = repo
            .plan(&Op::BranchCreate {
                name: "feature".into(),
                from: head,
                checkout: true,
            })
            .unwrap();

        assert_eq!(plan.steps.len(), 2);
        assert!(plan.display.contains(" && "));
        assert_eq!(
            plan.steps[1].argv,
            vec!["git".to_string(), "switch".to_string(), "feature".into()]
        );
        assert_eq!(plan.steps[1].risk, Risk::Mutating);
    }

    #[test]
    fn plan_branch_create_rejects_invalid_names() {
        let (fx, repo) = branch_fixture();
        let head = fx.git(&["rev-parse", "HEAD"]);
        for name in ["", "  ", "-leading-dash", "has space", "up/../down", "weird:colon"] {
            let err = repo
                .plan(&Op::BranchCreate {
                    name: name.into(),
                    from: head.clone(),
                    checkout: false,
                })
                .unwrap_err();
            assert!(
                matches!(err, GitError::BadBranchName { .. }),
                "name {name:?}: got {err:?}"
            );
        }
    }

    #[test]
    fn plan_branch_create_rejects_an_unknown_from() {
        let (_fx, repo) = branch_fixture();
        let err = repo
            .plan(&Op::BranchCreate {
                name: "x".into(),
                from: "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz".into(),
                checkout: false,
            })
            .unwrap_err();
        assert!(matches!(err, GitError::NoCommit { .. }), "got {err:?}");
    }

    // ── reset ──────────────────────────────────────────────────────────────

    #[test]
    fn plan_reset_classifies_risk_by_mode() {
        let (fx, repo) = branch_fixture();
        let target = fx.git(&["rev-parse", "HEAD~1"]);

        let soft = repo
            .plan(&Op::Reset {
                mode: ResetMode::Soft,
                target: target.clone(),
            })
            .unwrap();
        assert_eq!(soft.display, format!("git reset --soft {target}"));
        assert_eq!(soft.steps[0].risk, Risk::Mutating);
        assert!(soft.steps[0].effect.is_none());
        assert!(!soft.preview.as_ref().unwrap().requires_type_to_confirm);

        let mixed = repo
            .plan(&Op::Reset {
                mode: ResetMode::Mixed,
                target: target.clone(),
            })
            .unwrap();
        assert_eq!(mixed.display, format!("git reset --mixed {target}"));
        assert_eq!(mixed.steps[0].risk, Risk::Mutating);
        assert!(
            mixed
                .preview
                .as_ref()
                .unwrap()
                .lines
                .iter()
                .any(|l| l.contains("unstaged")),
            "mixed reset must explain the unstage: {mixed:?}"
        );

        let hard = repo
            .plan(&Op::Reset {
                mode: ResetMode::Hard,
                target: target.clone(),
            })
            .unwrap();
        assert_eq!(hard.display, format!("git reset --hard {target}"));
        assert_eq!(hard.steps[0].risk, Risk::Dangerous);
        assert!(hard.steps[0].effect.is_some());
        assert!(hard.preview.as_ref().unwrap().requires_type_to_confirm);
    }

    #[test]
    fn plan_reset_rejects_an_unknown_target() {
        let (_fx, repo) = branch_fixture();
        let err = repo
            .plan(&Op::Reset {
                mode: ResetMode::Mixed,
                target: "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz".into(),
            })
            .unwrap_err();
        assert!(matches!(err, GitError::NoCommit { .. }), "got {err:?}");
    }

    #[test]
    fn plan_reset_on_an_unborn_repo_errors() {
        let fx = testutil::Fixture::new("plan-reset-unborn");
        let repo = Repo::open(&fx.path).unwrap();
        let err = repo
            .plan(&Op::Reset {
                mode: ResetMode::Soft,
                target: "x".into(),
            })
            .unwrap_err();
        assert!(matches!(err, GitError::NoCommits), "got {err:?}");
    }

    // ── cherry_pick ────────────────────────────────────────────────────────

    #[test]
    fn plan_cherry_pick_lists_the_commits() {
        let fx = testutil::Fixture::new("plan-cherry");
        fx.commit_file("a.txt", "one\n", "c1");
        fx.git(&["checkout", "-b", "side"]);
        fx.commit_file("b.txt", "two\n", "c2");
        fx.git(&["checkout", "main"]);
        let pick = fx.git(&["rev-parse", "side"]);
        let repo = Repo::open(&fx.path).unwrap();

        let plan = repo
            .plan(&Op::CherryPick {
                hashes: vec![pick.clone()],
            })
            .unwrap();

        assert_eq!(plan.display, format!("git cherry-pick {pick}"));
        assert_eq!(plan.steps[0].risk, Risk::Mutating);
        let preview = plan.preview.as_ref().unwrap();
        assert_eq!(preview.headline, "Cherry-pick 1 commit");
        assert!(!preview.requires_type_to_confirm);
        assert!(
            preview.lines.iter().any(|l| l.contains("c2")),
            "preview should name the picked commit: {preview:?}"
        );
        assert!(
            preview.lines.iter().any(|l| l.contains("main")),
            "preview should name the target branch: {preview:?}"
        );
    }

    #[test]
    fn plan_cherry_pick_rejects_an_empty_selection() {
        let (_fx, repo) = branch_fixture();
        let err = repo
            .plan(&Op::CherryPick { hashes: vec![] })
            .unwrap_err();
        assert!(matches!(err, GitError::EmptySelection), "got {err:?}");
    }

    #[test]
    fn plan_cherry_pick_rejects_an_unknown_hash() {
        let (_fx, repo) = branch_fixture();
        let err = repo
            .plan(&Op::CherryPick {
                hashes: vec!["zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz".into()],
            })
            .unwrap_err();
        assert!(matches!(err, GitError::NoCommit { .. }), "got {err:?}");
    }
}
