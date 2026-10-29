//! Plan constructors: compile user operations into transparent [`CommandPlan`]s
//! (SPEC §8 — the transparency primitive).
//!
//! Each `plan_*` reads repository state through the `gix` read-model (branch,
//! upstream, staged files) to fill in the branch names, blast-radius effects, and
//! preview lines. Planning only runs read-only git and mutates nothing — except
//! the interactive rebase, which writes the todo stub its own argv references
//! (byte-for-byte the todo the preview shows, so the transparency contract holds).
//!
//! `display` is the single source of truth for the UI (inline hint, preview dialog,
//! activity log, SPEC §8/§9): it is the exact argv, quoted only where necessary.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use git_gotten_protocol::{
    Author, BisectState, BisectVerdict, ChangeKind, CommandPlan, ConflictResolution, ConflictSide,
    GitStep, Op, Preview, RebaseAction, RebaseEdit, ResetMode, Risk, Status,
};

use crate::{GitError, Repo, ahead_behind, conflict, upstream_of};

/// Join a git argv into the `display` string shown in the UI.
///
/// `argv[0]` is included, bare (never quoted): for normal steps it is the
/// literal `git` (the resolved binary path is never shown), for the
/// external-editor hand-off step it is the editor program. Arguments are
/// wrapped in double quotes only when they are empty or contain whitespace
/// or quotes, so simple commands read `git push origin main` while a message
/// with spaces reads `git commit -m "fix: the thing"`.
pub fn format_argv(argv: &[String]) -> String {
    join_quoted(argv, 0)
}

fn join_quoted(argv: &[String], start: usize) -> String {
    let mut out = String::new();
    for (i, arg) in argv.iter().enumerate().skip(start) {
        if i > start {
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
    /// Planning never *mutates*: the git invocations it makes are read-only
    /// (the discard / stage-hunks / conflict previews — diffs, `ls-files`,
    /// `show`, `merge-file`, `config`), and the only file it writes is the
    /// interactive rebase's todo stub ([`plan_rebase`]). The same op compiled
    /// twice against the same state yields the same plan.
    pub fn plan(&self, op: &Op) -> Result<CommandPlan, GitError> {
        let status = self.status()?;
        match op {
            Op::Commit {
                message,
                amend,
                author,
                co_authors,
            } => plan_commit(&status, message, author.as_ref(), co_authors, *amend),
            Op::Checkout { target, force } => plan_checkout(&status, target, *force),
            Op::Push {
                remote,
                branch,
                force,
                set_upstream,
            } => plan_push(
                &self.inner,
                &status,
                remote,
                branch.as_deref(),
                *force,
                *set_upstream,
            ),
            Op::Pull { rebase } => plan_pull(&status, *rebase),
            Op::Merge { source } => plan_merge(self, &status, source),
            Op::BranchCreate {
                name,
                from,
                checkout,
            } => plan_branch_create(self, name, from, *checkout),
            Op::Reset { mode, target } => plan_reset(self, &status, *mode, target),
            Op::CherryPick { hashes, target } => {
                plan_cherry_pick(self, &status, hashes, target.as_deref())
            }
            Op::Rebase { target, edits } => plan_rebase(self, &status, target, edits),
            Op::RebaseOnto { target } => plan_rebase_onto(self, &status, target),
            Op::Stage { path } => plan_stage(&status, path.as_deref()),
            Op::Unstage { path } => plan_unstage(&status, path.as_deref()),
            Op::Discard { path } => plan_discard(self, path),
            Op::StageHunks { path, selected } => plan_stage_hunks(self, path, selected),
            Op::BranchDelete {
                name,
                remote,
                force,
            } => plan_branch_delete(self, name, remote.as_deref(), *force),
            Op::BranchRename { name, new_name } => plan_branch_rename(self, name, new_name),
            Op::BranchSetUpstream {
                branch,
                remote,
                name,
            } => plan_branch_set_upstream(self, branch, remote.as_deref(), name),
            Op::RemoteAdd { name, url } => plan_remote_add(self, name, url),
            Op::RemoteRemove { name } => plan_remote_remove(self, name),
            Op::RemoteFetch { remote, refspec } => {
                plan_remote_fetch(self, remote, refspec.as_deref())
            }
            Op::TagCreate {
                name,
                target,
                annotated,
                message,
            } => plan_tag_create(self, name, target, *annotated, message.as_deref()),
            Op::TagCheckout { name, force } => plan_tag_checkout(self, name, *force),
            Op::TagDelete { name, remote } => plan_tag_delete(self, name, remote.as_deref()),
            Op::TagPush {
                remote,
                name,
                force,
            } => plan_tag_push(self, remote, name, *force),
            Op::ResolveConflict { path, resolution } => {
                plan_resolve_conflict(self, &status, path, resolution)
            }
            Op::ContinueOp => plan_continue_op(self, &status),
            Op::AbortOp => plan_abort_op(self, &status),
            Op::OpenEditor { paths } => plan_open_editor(self, paths),
            Op::WorktreeAdd {
                committish,
                path,
                new_branch,
            } => plan_worktree_add(committish, path, new_branch.as_deref()),
            Op::WorktreeRemove { path, force } => plan_worktree_remove(self, path, *force),
            Op::StashPush { include_untracked } => plan_stash_push(&status, *include_untracked),
            Op::StashApply { id } => plan_stash_op(self, "apply", *id),
            Op::StashPop { id } => plan_stash_op(self, "pop", *id),
            Op::StashDrop { id } => plan_stash_op(self, "drop", *id),
            Op::StashDropAll => plan_stash_drop_all(self),
            Op::SubmoduleInit { path } => plan_submodule_init(self, path),
            Op::SubmoduleUpdate { path, commit } => {
                plan_submodule_update(self, path, commit.as_deref())
            }
            Op::BisectStart { good, bad, extra } => plan_bisect_start(self, good, bad, extra),
            Op::BisectVerdict { verdict } => plan_bisect_verdict(self, *verdict),
            Op::BisectTest { command } => plan_bisect_test(self, command),
            Op::BisectReset => plan_bisect_reset(self),
        }
    }
}

/// `git commit -m <message> [--amend] [--author "…"]` — `Mutating`; previews the
/// staged files. Co-authors are appended to the message as `Co-authored-by:`
/// trailers (git's conventional format), so the displayed command shows them.
fn plan_commit(
    status: &Status,
    message: &str,
    author: Option<&Author>,
    co_authors: &[String],
    amend: bool,
) -> Result<CommandPlan, GitError> {
    if message.trim().is_empty() {
        return Err(GitError::EmptyCommitMessage);
    }

    let message = if co_authors.is_empty() {
        message.to_string()
    } else {
        let mut full = message.to_string();
        full.push_str("\n\n");
        for co in co_authors {
            full.push_str(&format!("Co-authored-by: {co}\n"));
        }
        full.trim_end().to_string()
    };

    let mut argv = vec![
        "git".to_string(),
        "commit".to_string(),
        "-m".to_string(),
        message.clone(),
    ];
    if amend {
        argv.push("--amend".to_string());
    }
    if let Some(a) = author {
        argv.push("--author".to_string());
        argv.push(format!("{} <{}>", a.name, a.email));
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
            input: None,
            program: None,
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
            input: None,
            program: None,
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
    set_upstream: bool,
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
    if set_upstream {
        argv.push("-u".to_string());
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
    if set_upstream {
        lines.push(format!("sets `{branch}`'s upstream to {remote}/{branch}"));
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
            input: None,
            program: None,
        }],
        display,
        preview: Some(Preview {
            headline: format!("Push to {remote}/{branch}"),
            lines,
            requires_type_to_confirm: force,
        }),
    })
}
/// `git pull [--rebase | --no-rebase]` — `Mutating`: fetch the current
/// branch's upstream, then merge it (or rebase the local commits onto it
/// when `rebase` is set).
///
/// A branch without an upstream has no tracking information, so `git pull`
/// fails at run time; the preview says so instead of letting the user find
/// out in the activity log.
fn plan_pull(status: &Status, rebase: bool) -> Result<CommandPlan, GitError> {
    let Some(branch) = status.branch.as_deref() else {
        return Err(GitError::DetachedHead);
    };
    let mut argv = vec!["git".to_string(), "pull".to_string()];
    // The reconciliation strategy is explicit on *both* forms: a bare
    // `git pull` is refused by git ≥ 2.21 when the branches have diverged
    // and no `pull.rebase` default is configured.
    argv.push(if rebase { "--rebase" } else { "--no-rebase" }.to_string());
    let display = format_argv(&argv);

    let what = if rebase { "rebase onto" } else { "merge" };
    let (upstream, lines) = match status.upstream.as_deref() {
        Some(up) => (
            up.to_string(),
            vec![format!(
                "fetch `{up}`, then {what} it ({ahead} ahead, {behind} behind)",
                ahead = status.ahead,
                behind = status.behind
            )],
        ),
        None => (
            "no upstream".to_string(),
            vec![format!(
                "no upstream for `{branch}` — git will fail until one is set (e.g. `git push -u origin {branch}`)"
            )],
        ),
    };

    Ok(CommandPlan {
        steps: vec![GitStep {
            argv,
            summary: format!("pull from {upstream}"),
            risk: Risk::Mutating,
            effect: None,
            input: None,
            program: None,
        }],
        display,
        preview: Some(Preview {
            headline: format!("Pull from {upstream}"),
            lines,
            requires_type_to_confirm: false,
        }),
    })
}

/// `git merge --no-ff <source>` — `Mutating`: merge a branch/commit into the
/// current branch. `--no-ff` forces a merge commit even when the merge is
/// fast-forwardable, so a clean merge is always visible in the graph.
///
/// The preview counts what comes in: the commits on `source` that are not on
/// HEAD, and the files they touch (a read-only diff — planning may run
/// read-only git, SPEC §8). A conflict is not a planning error: the op runs,
/// git stops with the conflicted files in the result (stage-8 handoff).
fn plan_merge(repo: &Repo, status: &Status, source: &str) -> Result<CommandPlan, GitError> {
    if source.trim().is_empty() {
        return Err(GitError::EmptyTarget);
    }
    let source_commit = commit_ish(&repo.inner, source)?;
    let source_oid = source_commit.id().detach();
    let head_oid = repo
        .inner
        .head()
        .map_err(GitError::Gix)?
        .id()
        .map(|id| id.detach())
        .ok_or(GitError::NoCommits)?;

    // Commits on `source` that are not on HEAD (the set difference runs from
    // the source tip, mirroring `ahead_behind`'s `behind` side).
    let incoming = if source_oid == head_oid {
        0
    } else {
        ahead_behind(&repo.inner, head_oid, source_oid)?.1
    };

    // The files the incoming commits touch: a read-only symmetric diff from
    // the merge base (what the merge would bring in).
    let spec = format!("HEAD...{source}");
    let files = crate::show::run_git_sync(&repo.path, &["diff", "--name-only", &spec])?
        .lines()
        .filter(|l| !l.is_empty())
        .count();

    let argv = vec![
        "git".to_string(),
        "merge".to_string(),
        "--no-ff".to_string(),
        source.to_string(),
    ];
    let display = format_argv(&argv);
    let branch = from_label(status);

    let lines = if incoming == 0 {
        vec!["already up to date — nothing to merge".to_string()]
    } else {
        vec![format!(
            "brings in {incoming} commit{}, {} file{}",
            if incoming == 1 { "" } else { "s" },
            files,
            if files == 1 { "" } else { "s" }
        )]
    };

    Ok(CommandPlan {
        steps: vec![GitStep {
            argv,
            summary: format!("merge {source} into {branch}"),
            risk: Risk::Mutating,
            effect: None,
            input: None,
            program: None,
        }],
        display,
        preview: Some(Preview {
            headline: format!("Merge `{source}` into `{branch}`"),
            lines,
            requires_type_to_confirm: false,
        }),
    })
}

/// Resolve a commit-ish (branch name, tag name, or hex oid — annotated tags
fn commit_ish<'a>(repo: &'a gix::Repository, spec: &str) -> Result<gix::Commit<'a>, GitError> {
    let spec = spec.trim();
    let id = repo
        .rev_parse_single(spec)
        .map_err(|_| GitError::NoCommit {
            hash: spec.to_string(),
        })?
        .detach();
    repo.find_object(id)
        .and_then(|o| o.peel_to_commit())
        .map_err(|_| GitError::NoCommit {
            hash: spec.to_string(),
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
/// Ref-name characters git rejects (a leading `-` is checked separately; a name
/// may not be empty or contain whitespace/control chars). Shared by branch, tag
/// and remote names.
fn invalid_ref_name(name: &str) -> bool {
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
    if invalid_ref_name(name) {
        return Err(GitError::BadRefName {
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
        input: None,
        program: None,
    }];
    if checkout {
        steps.push(GitStep {
            argv: vec!["git".to_string(), "switch".to_string(), name.to_string()],
            summary: format!("switch to {name}"),
            risk: Risk::Mutating,
            effect: None,
            input: None,
            program: None,
        });
    }

    let mut lines = vec![format!("at {}: {from_subject}", short_hash(from))];
    if checkout {
        lines.push("then switch to the new branch".to_string());
    }
    let exists_name = format!("refs/heads/{name}");
    let exists = repo.inner.find_reference(exists_name.as_str()).is_ok();
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
/// `git branch -d <name>` (or `-D` to force), or `git push <remote> --delete
/// <name>` when `remote` is set — `Mutating` either way.
///
/// The branch must exist (a plan is where a stale UI pick becomes a clear
/// error). Deleting the *checked-out* branch shows an effect (git will refuse
/// until HEAD moves); a branch with a remote upstream says the remote copy is
/// kept; a remote delete says the ref leaves the remote.
fn plan_branch_delete(
    repo: &Repo,
    name: &str,
    remote: Option<&str>,
    force: bool,
) -> Result<CommandPlan, GitError> {
    if invalid_ref_name(name) {
        return Err(GitError::BadRefName {
            name: name.to_string(),
        });
    }

    let step = match remote {
        None => {
            if repo
                .inner
                .find_reference(format!("refs/heads/{name}").as_str())
                .is_err()
            {
                return Err(GitError::UnknownBranch {
                    name: name.to_string(),
                });
            }
            let argv = vec![
                "git".to_string(),
                "branch".to_string(),
                if force {
                    "-D".to_string()
                } else {
                    "-d".to_string()
                },
                name.to_string(),
            ];

            // Blast radius: what the user thinks gets deleted.
            let mut effect = String::new();
            if repo.status()?.branch.as_deref() == Some(name) {
                effect.push_str("the branch is checked out — git will refuse until HEAD moves");
            }
            if let Some(up) = upstream_of(&repo.inner, name) {
                if !effect.is_empty() {
                    effect.push_str("; ");
                }
                effect.push_str(&format!("the remote branch {} is kept", up.name));
            }
            let effect = (!effect.is_empty()).then_some(effect);

            let mut lines = Vec::new();
            if force {
                lines.push("force: deletes even unmerged work".to_string());
            } else {
                lines.push(
                    "git refuses if the branch has unmerged commits (use force to override)"
                        .to_string(),
                );
            }

            GitStep {
                argv,
                summary: format!("delete branch {name}"),
                risk: Risk::Mutating,
                effect,
                input: None,
                program: None,
            }
        }
        Some(remote) => {
            if !repo.inner.remote_names().iter().any(|n| *n == remote) {
                return Err(GitError::UnknownRemote {
                    name: remote.to_string(),
                });
            }
            let full = format!("refs/remotes/{remote}/{name}");
            if repo.inner.find_reference(full.as_str()).is_err() {
                return Err(GitError::UnknownBranch {
                    name: format!("{remote}/{name}"),
                });
            }
            GitStep {
                argv: vec![
                    "git".to_string(),
                    "push".to_string(),
                    remote.to_string(),
                    "--delete".to_string(),
                    name.to_string(),
                ],
                summary: format!("delete {name} from {remote}"),
                risk: Risk::Mutating,
                effect: Some(format!("the branch is deleted from {remote}")),
                input: None,
                program: None,
            }
        }
    };

    let display = format_argv(&step.argv);
    Ok(CommandPlan {
        steps: vec![step.clone()],
        display,
        preview: Some(Preview {
            headline: format!("Delete branch {name}"),
            lines: vec![step.effect.clone().unwrap_or_default()],
            requires_type_to_confirm: false,
        }),
    })
}

/// `git branch --move <name> <new_name>` — `Mutating`.
fn plan_branch_rename(repo: &Repo, name: &str, new_name: &str) -> Result<CommandPlan, GitError> {
    if invalid_ref_name(name) || invalid_ref_name(new_name) {
        return Err(GitError::BadRefName {
            name: if invalid_ref_name(name) {
                name
            } else {
                new_name
            }
            .to_string(),
        });
    }
    if repo
        .inner
        .find_reference(format!("refs/heads/{name}").as_str())
        .is_err()
    {
        return Err(GitError::UnknownBranch {
            name: name.to_string(),
        });
    }
    if repo
        .inner
        .find_reference(format!("refs/heads/{new_name}").as_str())
        .is_ok()
    {
        return Err(GitError::BranchExists {
            name: new_name.to_string(),
        });
    }

    let argv = vec![
        "git".to_string(),
        "branch".to_string(),
        "--move".to_string(),
        name.to_string(),
        new_name.to_string(),
    ];
    let display = format_argv(&argv);
    Ok(CommandPlan {
        steps: vec![GitStep {
            argv: argv.clone(),
            summary: format!("rename {name} to {new_name}"),
            risk: Risk::Mutating,
            effect: None,
            input: None,
            program: None,
        }],
        display,
        preview: Some(Preview {
            headline: format!("Rename {name} → {new_name}"),
            lines: vec![],
            requires_type_to_confirm: false,
        }),
    })
}

/// `git branch <branch> --set-upstream-to=<remote>/<name>` (or
/// `--unset-upstream` when `remote` is `None`) — `Mutating`.
fn plan_branch_set_upstream(
    repo: &Repo,
    branch: &str,
    remote: Option<&str>,
    name: &str,
) -> Result<CommandPlan, GitError> {
    if invalid_ref_name(branch) {
        return Err(GitError::BadRefName {
            name: branch.to_string(),
        });
    }
    if repo
        .inner
        .find_reference(format!("refs/heads/{branch}").as_str())
        .is_err()
    {
        return Err(GitError::UnknownBranch {
            name: branch.to_string(),
        });
    }

    let (argv, summary) = match remote {
        None => (
            vec![
                "git".to_string(),
                "branch".to_string(),
                branch.to_string(),
                "--unset-upstream".to_string(),
            ],
            format!("clear {branch}'s upstream"),
        ),
        Some(remote) => {
            if !repo.inner.remote_names().iter().any(|n| *n == remote) {
                return Err(GitError::UnknownRemote {
                    name: remote.to_string(),
                });
            }
            let full = format!("refs/remotes/{remote}/{name}");
            if repo.inner.find_reference(full.as_str()).is_err() {
                return Err(GitError::UnknownBranch {
                    name: format!("{remote}/{name}"),
                });
            }
            (
                vec![
                    "git".to_string(),
                    "branch".to_string(),
                    branch.to_string(),
                    format!("--set-upstream-to={remote}/{name}"),
                ],
                format!("set {branch}'s upstream to {remote}/{name}"),
            )
        }
    };
    let display = format_argv(&argv);
    Ok(CommandPlan {
        steps: vec![GitStep {
            argv: argv.clone(),
            summary,
            risk: Risk::Mutating,
            effect: None,
            input: None,
            program: None,
        }],
        display,
        preview: None,
    })
}

/// `git remote add <name> <url>` — `Mutating`.
fn plan_remote_add(repo: &Repo, name: &str, url: &str) -> Result<CommandPlan, GitError> {
    if invalid_ref_name(name) {
        return Err(GitError::BadRefName {
            name: name.to_string(),
        });
    }
    if url.trim().is_empty() {
        return Err(GitError::EmptyTarget);
    }
    if repo.inner.remote_names().iter().any(|n| *n == name) {
        return Err(GitError::RemoteExists {
            name: name.to_string(),
        });
    }

    let argv = vec![
        "git".to_string(),
        "remote".to_string(),
        "add".to_string(),
        name.to_string(),
        url.to_string(),
    ];
    let display = format_argv(&argv);
    Ok(CommandPlan {
        steps: vec![GitStep {
            argv: argv.clone(),
            summary: format!("add remote {name}"),
            risk: Risk::Mutating,
            effect: None,
            input: None,
            program: None,
        }],
        display,
        preview: None,
    })
}

/// `git remote remove <name>` — `Mutating`; the effect counts the
/// remote-tracking branches that disappear with the remote.
fn plan_remote_remove(repo: &Repo, name: &str) -> Result<CommandPlan, GitError> {
    if !repo.inner.remote_names().iter().any(|n| *n == name) {
        return Err(GitError::UnknownRemote {
            name: name.to_string(),
        });
    }

    // The refs that will be deleted: this remote's tracking branches.
    let prefix = format!("refs/remotes/{name}/");
    let count = repo
        .inner
        .references()
        .map_err(GitError::Gix)?
        .remote_branches()
        .map_err(GitError::Gix)?
        .filter_map(|r| r.ok())
        .filter(|r| String::from_utf8_lossy(r.name().as_bstr()).starts_with(&prefix))
        .count();

    let argv = vec![
        "git".to_string(),
        "remote".to_string(),
        "remove".to_string(),
        name.to_string(),
    ];
    let display = format_argv(&argv);
    Ok(CommandPlan {
        steps: vec![GitStep {
            argv: argv.clone(),
            summary: format!("remove remote {name}"),
            risk: Risk::Mutating,
            effect: Some(format!(
                "deletes {count} remote-tracking branch{} of `{name}`",
                if count == 1 { "" } else { "es" }
            )),
            input: None,
            program: None,
        }],
        display,
        preview: None,
    })
}

/// `git fetch <remote> [refspec]` — `Mutating` (it updates the remote-tracking
/// refs; it never touches the working tree).
fn plan_remote_fetch(
    repo: &Repo,
    remote: &str,
    refspec: Option<&str>,
) -> Result<CommandPlan, GitError> {
    if !repo.inner.remote_names().iter().any(|n| *n == remote) {
        return Err(GitError::UnknownRemote {
            name: remote.to_string(),
        });
    }

    let mut argv = vec!["git".to_string(), "fetch".to_string(), remote.to_string()];
    let summary = match refspec {
        Some(r) => {
            argv.push(r.to_string());
            format!("fetch {r} from {remote}")
        }
        None => format!("fetch {remote}"),
    };
    let display = format_argv(&argv);
    Ok(CommandPlan {
        steps: vec![GitStep {
            argv: argv.clone(),
            summary,
            risk: Risk::Mutating,
            effect: None,
            input: None,
            program: None,
        }],
        display,
        preview: None,
    })
}

/// `git tag [-a] [-m <message>] <name> <target>` — `Mutating`.
fn plan_tag_create(
    repo: &Repo,
    name: &str,
    target: &str,
    annotated: bool,
    message: Option<&str>,
) -> Result<CommandPlan, GitError> {
    if invalid_ref_name(name) {
        return Err(GitError::BadRefName {
            name: name.to_string(),
        });
    }
    let target_commit = repo.commit_at(target)?;
    let subject = target_commit
        .message()
        .map_err(|e| GitError::Object(e.to_string()))?
        .summary()
        .to_string();

    let mut argv = vec!["git".to_string(), "tag".to_string()];
    if annotated {
        argv.push("-a".to_string());
    }
    argv.push(name.to_string());
    if annotated {
        // `-m` never opens an editor; a `None` message becomes an explicit empty one.
        argv.push("-m".to_string());
        argv.push(message.unwrap_or_default().to_string());
    }
    argv.push(target.to_string());

    let mut lines = vec![format!("at {target}: {subject}")];
    if repo
        .inner
        .find_reference(format!("refs/tags/{name}").as_str())
        .is_ok()
    {
        lines.push(format!("tag `{name}` already exists — git will fail"));
    }

    let display = format_argv(&argv);
    Ok(CommandPlan {
        steps: vec![GitStep {
            argv: argv.clone(),
            summary: format!("create tag {name} at {}", short_hash(target)),
            risk: Risk::Mutating,
            effect: None,
            input: None,
            program: None,
        }],
        display,
        preview: Some(Preview {
            headline: format!("Create tag {name}"),
            lines,
            requires_type_to_confirm: false,
        }),
    })
}

/// `git switch --detach <name>` — `Mutating`; checks the tag out without a branch.
fn plan_tag_checkout(repo: &Repo, name: &str, force: bool) -> Result<CommandPlan, GitError> {
    if invalid_ref_name(name) {
        return Err(GitError::BadRefName {
            name: name.to_string(),
        });
    }
    if repo
        .inner
        .find_reference(format!("refs/tags/{name}").as_str())
        .is_err()
    {
        return Err(GitError::UnknownTag {
            name: name.to_string(),
        });
    }

    let mut argv = vec![
        "git".to_string(),
        "switch".to_string(),
        "--detach".to_string(),
        name.to_string(),
    ];
    if force {
        argv.push("--force".to_string());
    }
    let display = format_argv(&argv);
    Ok(CommandPlan {
        steps: vec![GitStep {
            argv: argv.clone(),
            summary: format!("check out tag {name} (detached)"),
            risk: Risk::Mutating,
            effect: None,
            input: None,
            program: None,
        }],
        display,
        preview: None,
    })
}

/// `git tag -d <name>` (or `git push <remote> --delete <name>` when `remote` is
/// set) — `Mutating` either way.
fn plan_tag_delete(repo: &Repo, name: &str, remote: Option<&str>) -> Result<CommandPlan, GitError> {
    if invalid_ref_name(name) {
        return Err(GitError::BadRefName {
            name: name.to_string(),
        });
    }

    let step = match remote {
        None => {
            // Deleting a local tag requires the tag to exist here.
            if repo
                .inner
                .find_reference(format!("refs/tags/{name}").as_str())
                .is_err()
            {
                return Err(GitError::UnknownTag {
                    name: name.to_string(),
                });
            }
            GitStep {
                argv: vec![
                    "git".to_string(),
                    "tag".to_string(),
                    "-d".to_string(),
                    name.to_string(),
                ],
                summary: format!("delete tag {name}"),
                risk: Risk::Mutating,
                effect: None,
                input: None,
                program: None,
            }
        }
        Some(remote) => {
            // Deleting on a remote does not require a local copy (the local tag may
            // already be gone); a stale remote tag fails at run time with git's error.
            if !repo.inner.remote_names().iter().any(|n| *n == remote) {
                return Err(GitError::UnknownRemote {
                    name: remote.to_string(),
                });
            }
            GitStep {
                argv: vec![
                    "git".to_string(),
                    "push".to_string(),
                    remote.to_string(),
                    "--delete".to_string(),
                    name.to_string(),
                ],
                summary: format!("delete {name} from {remote}"),
                risk: Risk::Mutating,
                effect: Some(format!("the tag is deleted from {remote}")),
                input: None,
                program: None,
            }
        }
    };
    let display = format_argv(&step.argv);
    Ok(CommandPlan {
        steps: vec![step.clone()],
        display,
        preview: Some(Preview {
            headline: format!("Delete tag {name}"),
            lines: vec![step.effect.clone().unwrap_or_default()],
            requires_type_to_confirm: false,
        }),
    })
}

/// `git push [--force] <remote> <tag>` — `Mutating`, or `Dangerous` (type-to-confirm)
/// when force-pushing (a tag is supposed to be immutable).
fn plan_tag_push(
    repo: &Repo,
    remote: &str,
    name: &str,
    force: bool,
) -> Result<CommandPlan, GitError> {
    if !repo.inner.remote_names().iter().any(|n| *n == remote) {
        return Err(GitError::UnknownRemote {
            name: remote.to_string(),
        });
    }
    if repo
        .inner
        .find_reference(format!("refs/tags/{name}").as_str())
        .is_err()
    {
        return Err(GitError::UnknownTag {
            name: name.to_string(),
        });
    }

    let mut argv = vec!["git".to_string(), "push".to_string()];
    if force {
        argv.push("--force".to_string());
    }
    argv.push(remote.to_string());
    argv.push(name.to_string());

    let effect = force.then(|| format!("force-push `{name}` — the remote tag will be replaced"));
    let display = format_argv(&argv);
    let preview_line = effect
        .clone()
        .unwrap_or_else(|| "the tag is published to the remote".to_string());
    Ok(CommandPlan {
        steps: vec![GitStep {
            argv: argv.clone(),
            summary: format!("push tag {name} to {remote}"),
            risk: if force {
                Risk::Dangerous
            } else {
                Risk::Mutating
            },
            effect,
            input: None,
            program: None,
        }],
        display,
        preview: Some(Preview {
            headline: format!("Push tag {name} to {remote}"),
            lines: vec![preview_line],
            requires_type_to_confirm: force,
        }),
    })
}

/// `git worktree add [-b <new_branch>] <path> <committish>` — `Mutating`
/// (SPEC §10.7). git itself refuses to check out a branch that is already
/// checked out in another worktree; that refusal is deliberately *not*
/// pre-checked — it surfaces through the executor as a failed `OpResult`
/// carrying git's own message (transparency contract, SPEC §18).
fn plan_worktree_add(
    committish: &str,
    path: &str,
    new_branch: Option<&str>,
) -> Result<CommandPlan, GitError> {
    if committish.trim().is_empty() || path.trim().is_empty() {
        return Err(GitError::EmptyTarget);
    }
    if let Some(name) = new_branch {
        if invalid_ref_name(name) {
            return Err(GitError::BadRefName {
                name: name.to_string(),
            });
        }
    }

    let mut argv = vec!["git".to_string(), "worktree".to_string(), "add".to_string()];
    if let Some(name) = new_branch {
        argv.push("-b".to_string());
        argv.push(name.to_string());
    }
    argv.push(path.to_string());
    argv.push(committish.to_string());

    let checkout = match new_branch {
        Some(name) => format!("new branch {name} at {committish}"),
        None => format!("check out {committish}"),
    };
    let display = format_argv(&argv);

    Ok(CommandPlan {
        steps: vec![GitStep {
            argv,
            summary: format!("add worktree at {path}"),
            risk: Risk::Mutating,
            effect: None,
            input: None,
            program: None,
        }],
        display,
        preview: Some(Preview {
            headline: "Add worktree".to_string(),
            lines: vec![checkout, format!("new directory {path}")],
            requires_type_to_confirm: false,
        }),
    })
}

/// `git worktree remove [--force] <path>` — `Mutating` for a clean worktree;
/// `Dangerous` (type-to-confirm, effect names the lost work) when `force`
/// removes a dirty one (SPEC §10.7). Without `force` a dirty worktree is
/// refused at plan time — the same rule git enforces. The main worktree is
/// never removable from inside it.
fn plan_worktree_remove(repo: &Repo, path: &str, force: bool) -> Result<CommandPlan, GitError> {
    let want = Path::new(path)
        .canonicalize()
        .unwrap_or_else(|_| Path::new(path).to_path_buf());
    let target = repo
        .worktrees()?
        .into_iter()
        .find(|w| Path::new(&w.path) == want)
        .ok_or_else(|| GitError::WorktreeNotFound {
            path: path.to_string(),
        })?;
    if target.is_main {
        return Err(GitError::MainWorktree);
    }

    // "Dirty" is exactly what `git worktree remove` refuses to discard:
    // anything that worktree's own status reports. An unreadable worktree
    // counts clean — git still has the final say.
    let dirty = Repo::open(&want)
        .and_then(|w| w.status())
        .map(|s| !s.changes.is_empty())
        .unwrap_or(false);
    if dirty && !force {
        return Err(GitError::WorktreeDirty {
            path: target.path.clone(),
        });
    }

    let mut argv = vec![
        "git".to_string(),
        "worktree".to_string(),
        "remove".to_string(),
    ];
    if force {
        argv.push("--force".to_string());
    }
    argv.push(target.path.clone());

    let checked_out = target.branch.as_deref().unwrap_or("detached HEAD");
    let effect = dirty.then(|| "uncommitted changes in the worktree will be discarded".to_string());
    let display = format_argv(&argv);

    Ok(CommandPlan {
        steps: vec![GitStep {
            argv,
            summary: format!("remove worktree {}", target.path),
            risk: if dirty {
                Risk::Dangerous
            } else {
                Risk::Mutating
            },
            effect,
            input: None,
            program: None,
        }],
        display,
        preview: Some(Preview {
            headline: "Remove worktree".to_string(),
            lines: vec![format!("removes {checked_out} at {}", target.path)],
            requires_type_to_confirm: dirty,
        }),
    })
}

// ── Stage 11: stashes (SPEC §10.6) ─────────────────────────────────────────

/// `git stash push [-u]` — `Mutating`: the worktree is cleaned, but the
/// snapshot is recoverable from the stash list.
fn plan_stash_push(status: &Status, include_untracked: bool) -> Result<CommandPlan, GitError> {
    let mut argv = vec!["git".to_string(), "stash".to_string(), "push".to_string()];
    if include_untracked {
        argv.push("-u".to_string());
    }
    let count = status.changes.len();
    let mut lines = vec![format!("stashes {count} uncommitted change(s)")];
    if include_untracked {
        lines.push("untracked files are included".to_string());
    }
    let display = format_argv(&argv);
    Ok(CommandPlan {
        steps: vec![GitStep {
            argv,
            summary: "stash the uncommitted changes".to_string(),
            risk: Risk::Mutating,
            effect: None,
            input: None,
            program: None,
        }],
        display,
        preview: Some(Preview {
            headline: "Stash changes".to_string(),
            lines,
            requires_type_to_confirm: false,
        }),
    })
}

/// `git stash apply|pop|drop stash@{id}` — apply keeps the entry, pop applies
/// and removes it (both `Mutating`), drop discards the snapshot without
/// applying it: `Dangerous` (type-to-confirm, SPEC §10.6). An unknown list
/// position is refused at plan time with the stash it names.
fn plan_stash_op(repo: &Repo, verb: &str, id: u32) -> Result<CommandPlan, GitError> {
    let stash = repo
        .stashes()?
        .into_iter()
        .find(|s| s.id == id)
        .ok_or(GitError::StashNotFound { id })?;
    let argv = vec![
        "git".to_string(),
        "stash".to_string(),
        verb.to_string(),
        stash.name.clone(),
    ];
    let dangerous = verb == "drop";
    let effect =
        dangerous.then(|| "the stash is deleted without applying it; its work is gone".to_string());
    let display = format_argv(&argv);
    let headline = format!("{} stash", verb[..1].to_uppercase() + &verb[1..]);
    Ok(CommandPlan {
        steps: vec![GitStep {
            argv,
            summary: format!("{verb} {}", stash.name),
            risk: if dangerous {
                Risk::Dangerous
            } else {
                Risk::Mutating
            },
            effect,
            input: None,
            program: None,
        }],
        display,
        preview: Some(Preview {
            headline,
            lines: vec![
                format!("{} — {}", stash.name, stash.message),
                format!("{} file(s): {}", stash.files.len(), stash.files.join(", ")),
            ],
            requires_type_to_confirm: dangerous,
        }),
    })
}

/// `git stash clear` — the SPEC §10.6 drop-all action: deletes every saved
/// snapshot. `Dangerous` (type-to-confirm); an empty stash list is refused at
/// plan time so the UI never confirms a no-op.
fn plan_stash_drop_all(repo: &Repo) -> Result<CommandPlan, GitError> {
    let stashes = repo.stashes()?;
    if stashes.is_empty() {
        return Err(GitError::NoStashes);
    }
    let argv = vec!["git".to_string(), "stash".to_string(), "clear".to_string()];
    Ok(CommandPlan {
        steps: vec![GitStep {
            argv: argv.clone(),
            summary: format!("delete {} stash(es)", stashes.len()),
            risk: Risk::Dangerous,
            effect: Some("every stash snapshot is deleted; their work is gone".to_string()),
            input: None,
            program: None,
        }],
        display: format_argv(&argv),
        preview: Some(Preview {
            headline: "Clear all stashes".to_string(),
            lines: stashes
                .iter()
                .map(|s| format!("{} — {}", s.name, s.message))
                .collect(),
            requires_type_to_confirm: true,
        }),
    })
}

// ── Stage 11: submodules (SPEC §10.8) ──────────────────────────────────────

/// Look up a registered submodule by path; unknown paths are a clean error.
fn submodule_at(repo: &Repo, path: &str) -> Result<git_gotten_protocol::Submodule, GitError> {
    repo.submodules()?
        .into_iter()
        .find(|s| s.path == path)
        .ok_or_else(|| GitError::SubmoduleNotFound {
            path: path.to_string(),
        })
}

/// `git submodule update --init <path>` — `Mutating`: creates the checkout at
/// the recorded commit.
fn plan_submodule_init(repo: &Repo, path: &str) -> Result<CommandPlan, GitError> {
    let sub = submodule_at(repo, path)?;
    let argv = vec![
        "git".to_string(),
        "submodule".to_string(),
        "update".to_string(),
        "--init".to_string(),
        sub.path.clone(),
    ];
    let display = format_argv(&argv);
    Ok(CommandPlan {
        steps: vec![GitStep {
            argv,
            summary: format!("initialize submodule {}", sub.path),
            risk: Risk::Mutating,
            effect: None,
            input: None,
            program: None,
        }],
        display,
        preview: Some(Preview {
            headline: "Initialize submodule".to_string(),
            lines: vec![
                format!("{} ← {}", sub.path, sub.url),
                format!("checked out at {}", sub.recorded),
            ],
            requires_type_to_confirm: false,
        }),
    })
}

/// `git submodule update <path>` (to the recorded gitlink) followed — when a
/// specific commit is asked for — by `git -C <path> checkout <commit>`
/// (SPEC §10.8). `Mutating`: local changes inside the submodule checkout can
/// be overwritten, so the preview names the current state.
fn plan_submodule_update(
    repo: &Repo,
    path: &str,
    commit: Option<&str>,
) -> Result<CommandPlan, GitError> {
    let sub = submodule_at(repo, path)?;
    let commit = match commit {
        Some(c) if !c.trim().is_empty() => Some(c.trim().to_string()),
        Some(_) => return Err(GitError::EmptyTarget),
        None => None,
    };
    let mut steps = vec![GitStep {
        argv: vec![
            "git".to_string(),
            "submodule".to_string(),
            "update".to_string(),
            sub.path.clone(),
        ],
        summary: format!("update submodule {} to the recorded commit", sub.path),
        risk: Risk::Mutating,
        effect: None,
        input: None,
        program: None,
    }];
    if let Some(c) = &commit {
        steps.push(GitStep {
            argv: vec![
                "git".to_string(),
                "-C".to_string(),
                sub.path.clone(),
                "checkout".to_string(),
                c.clone(),
            ],
            summary: format!("check {} out at {c}", sub.path),
            risk: Risk::Mutating,
            effect: None,
            input: None,
            program: None,
        });
    }
    let display = steps
        .iter()
        .map(|s| format_argv(&s.argv))
        .collect::<Vec<_>>()
        .join(" && ");
    let mut lines = match &sub.head {
        Some(h) if sub.differs => vec![format!("currently at {h}, recorded at {}", sub.recorded)],
        Some(h) => vec![format!("currently at {h}, already synced")],
        None => vec![format!(
            "not initialized; update will check out {}",
            sub.recorded
        )],
    };
    if let Some(c) = &commit {
        lines.push(format!("then checks {c} out inside the submodule"));
    }
    Ok(CommandPlan {
        steps,
        display,
        preview: Some(Preview {
            headline: "Update submodule".to_string(),
            lines,
            requires_type_to_confirm: false,
        }),
    })
}

// ── Stage 11: bisect (SPEC §10.9) ──────────────────────────────────────────

/// A bisect step needs a running session; verdict/test/reset without one is a
/// clean plan-time error (the same rule git enforces).
fn require_bisect(repo: &Repo) -> Result<BisectState, GitError> {
    let state = repo.bisect_state()?;
    if state.active {
        Ok(state)
    } else {
        Err(GitError::NoBisectInProgress)
    }
}

/// `git bisect start <bad> <good> [extra…]` — `Mutating`: HEAD detaches and
/// every step checks out a candidate. The preview counts the candidates git
/// will test (planning runs only read-only `rev-list`).
fn plan_bisect_start(
    repo: &Repo,
    good: &str,
    bad: &str,
    extra: &[String],
) -> Result<CommandPlan, GitError> {
    if good.trim().is_empty() || bad.trim().is_empty() {
        return Err(GitError::EmptyTarget);
    }
    let mut argv = vec![
        "git".to_string(),
        "bisect".to_string(),
        "start".to_string(),
        bad.to_string(),
        good.to_string(),
    ];
    argv.extend(extra.iter().cloned());
    let mut args: Vec<&str> = vec!["rev-list", "--count", bad, "--not", good];
    args.extend(extra.iter().map(String::as_str));
    let count_out = crate::show::run_git_sync(&repo.path, &args)?;
    // Same semantics as `bisect::state`: the bad tip itself is already decided,
    // so it is not a candidate under test.
    let candidates: u64 = count_out
        .trim()
        .parse::<u64>()
        .unwrap_or(0)
        .saturating_sub(1);
    let display = format_argv(&argv);
    Ok(CommandPlan {
        steps: vec![GitStep {
            argv,
            summary: "start a bisect session".to_string(),
            risk: Risk::Mutating,
            effect: None,
            input: None,
            program: None,
        }],
        display,
        preview: Some(Preview {
            headline: "Start bisect".to_string(),
            lines: vec![
                format!("{bad} is bad, {good} is good"),
                format!("{candidates} commit(s) under test"),
            ],
            requires_type_to_confirm: false,
        }),
    })
}

/// `git bisect good|bad` — `Mutating`: marks the checked-out candidate and
/// advances the session to the next one (or converges).
fn plan_bisect_verdict(repo: &Repo, verdict: BisectVerdict) -> Result<CommandPlan, GitError> {
    let state = require_bisect(repo)?;
    let word = match verdict {
        BisectVerdict::Good => "good",
        BisectVerdict::Bad => "bad",
    };
    let argv = vec!["git".to_string(), "bisect".to_string(), word.to_string()];
    let display = format_argv(&argv);
    let current = state.current.unwrap_or_default();
    Ok(CommandPlan {
        steps: vec![GitStep {
            argv,
            summary: format!("mark the current bisect candidate {word}"),
            risk: Risk::Mutating,
            effect: None,
            input: None,
            program: None,
        }],
        display,
        preview: Some(Preview {
            headline: format!("Mark {word}"),
            lines: vec![
                format!("marks {current} {word}"),
                format!("{} commit(s) still untested", state.remaining),
            ],
            requires_type_to_confirm: false,
        }),
    })
}

/// Run the bisect test command against the checked-out candidate (SPEC §10.9).
/// The command string is split on whitespace into argv and spawned directly —
/// no shell, no shell grammar (shortcut: quoted arguments are not supported;
/// upgrade to a shell-lexer if real-world commands need them). Exit 0 marks
/// the commit good (git's `bisect run` convention).
fn plan_bisect_test(repo: &Repo, command: &str) -> Result<CommandPlan, GitError> {
    let state = require_bisect(repo)?;
    let argv: Vec<String> = command.split_whitespace().map(str::to_string).collect();
    let Some(program) = argv.first().cloned() else {
        return Err(GitError::EmptyTarget);
    };
    let display = format_argv(&argv);
    let current = state.current.unwrap_or_default();
    Ok(CommandPlan {
        steps: vec![GitStep {
            argv,
            summary: format!("run the bisect test against {current}"),
            risk: Risk::Mutating,
            effect: None,
            input: None,
            program: Some(program),
        }],
        display,
        preview: Some(Preview {
            headline: "Run bisect test".to_string(),
            lines: vec![
                format!("runs against the checked-out commit {current}"),
                "exit 0 marks it good; any other exit marks it bad".to_string(),
            ],
            requires_type_to_confirm: false,
        }),
    })
}

/// `git bisect reset` — `Mutating`: ends the session and restores the original
/// branch/checkout.
fn plan_bisect_reset(repo: &Repo) -> Result<CommandPlan, GitError> {
    require_bisect(repo)?;
    let argv = vec!["git".to_string(), "bisect".to_string(), "reset".to_string()];
    let display = format_argv(&argv);
    Ok(CommandPlan {
        steps: vec![GitStep {
            argv,
            summary: "end the bisect session".to_string(),
            risk: Risk::Mutating,
            effect: None,
            input: None,
            program: None,
        }],
        display,
        preview: Some(Preview {
            headline: "Reset bisect".to_string(),
            lines: vec![
                "the bisect session ends".to_string(),
                "the original branch and commit are restored".to_string(),
            ],
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
            input: None,
            program: None,
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

/// `git cherry-pick <hash>…` (preceded by `git switch <target>` when the pick
/// targets another branch) — `Mutating`: re-apply commits onto the current
/// branch, or onto `target` (SPEC §10.4).
///
/// Every hash must exist. A conflicting pick fails at run time with git's
/// output (safe failure, SPEC §8); the Stage 8 flow resolves and continues it
/// on the target branch.
fn plan_cherry_pick(
    repo: &Repo,
    status: &Status,
    hashes: &[String],
    target: Option<&str>,
) -> Result<CommandPlan, GitError> {
    if hashes.is_empty() {
        return Err(GitError::EmptySelection);
    }
    let target = target.map(str::trim).filter(|name| !name.is_empty());
    // A target other than the current branch switches first: the pick lands
    // where the user pointed, and a stopped pick continues on that branch.
    let switch_to = match target {
        Some(name) if status.branch.as_deref() != Some(name) => {
            commit_ish(&repo.inner, name)?;
            Some(name)
        }
        _ => None,
    };
    let onto = target.unwrap_or_else(|| status.branch.as_deref().unwrap_or("the detached HEAD"));
    let mut lines = Vec::new();
    if let Some(name) = switch_to {
        lines.push(format!("switches to {name} first"));
    }
    let mut picked = Vec::new();
    for hash in hashes {
        let commit = repo.commit_at(hash)?;
        let subject = commit
            .message()
            .map_err(|e| GitError::Object(e.to_string()))?
            .summary()
            .to_string();
        lines.push(format!("  {}: {subject}", short_hash(hash)));
        picked.push(hash.clone());
    }
    lines.push(format!("onto {onto}"));

    let mut steps = Vec::new();
    if let Some(name) = switch_to {
        steps.push(GitStep {
            argv: vec!["git".to_string(), "switch".to_string(), name.to_string()],
            summary: format!("switch to {name}"),
            risk: Risk::Mutating,
            effect: None,
            input: None,
            program: None,
        });
    }
    let mut argv = vec!["git".to_string(), "cherry-pick".to_string()];
    argv.extend(picked);
    steps.push(GitStep {
        argv,
        summary: format!("cherry-pick onto {onto}"),
        risk: Risk::Mutating,
        effect: None,
        input: None,
        program: None,
    });
    let display = steps
        .iter()
        .map(|s| format_argv(&s.argv))
        .collect::<Vec<_>>()
        .join(" && ");
    let count = hashes.len();

    Ok(CommandPlan {
        steps,
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

/// The commits `target..HEAD`: (full oid, subject), newest-first — git log's
/// order, which is the UI order the RebaseDialog edits in. The target is
/// resolved as a commit-ish first so an unknown ref fails at plan time.
pub(crate) fn rebase_range(repo: &Repo, target: &str) -> Result<Vec<(String, String)>, GitError> {
    let target = target.trim();
    if target.is_empty() {
        return Err(GitError::EmptyTarget);
    }
    commit_ish(&repo.inner, target)?;
    let spec = format!("{target}..HEAD");
    let out = crate::show::run_git_sync(&repo.path, &["log", "--format=%H\t%s", &spec])?;
    Ok(out
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let (hash, subject) = line.split_once('\t').unwrap_or((line, ""));
            (hash.to_string(), subject.to_string())
        })
        .collect())
}

/// How many commits of the rebase range are reachable from a remote-tracking
/// tip — rebasing those rewrites published history (SPEC §8 protection).
fn published_count(repo: &Repo, hashes: &[String]) -> Result<usize, GitError> {
    let remote_refs = crate::show::run_git_sync(
        &repo.path,
        &["for-each-ref", "--format=%(refname)", "refs/remotes/"],
    )?;
    let tips: Vec<String> = remote_refs
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(str::to_string)
        .collect();
    let mut published = 0;
    for hash in hashes {
        for tip in &tips {
            match crate::show::run_git_sync(&repo.path, &["merge-base", "--is-ancestor", hash, tip])
            {
                Ok(_) => {
                    published += 1;
                    break;
                }
                // Exit 1 is git's "not an ancestor" — data, not failure.
                Err(GitError::Command { exit: 1, .. }) => {}
                Err(err) => return Err(err),
            }
        }
    }
    Ok(published)
}

/// The risk tier for rebasing `published` already-published commits:
/// `Dangerous` (type-to-confirm) when any, `Mutating` otherwise (SPEC §8).
fn rebase_risk(published: usize) -> (Risk, Option<String>) {
    if published == 0 {
        return (Risk::Mutating, None);
    }
    (
        Risk::Dangerous,
        Some(format!(
            "rewrites {published} commit{} already on a remote",
            if published == 1 { "" } else { "s" }
        )),
    )
}

fn published_warning(published: usize) -> String {
    format!(
        "rewrites {published} commit{} on a remote — published history will be rewritten",
        if published == 1 { "" } else { "s" }
    )
}

/// The todo line that rewrites a commit's message: `exec git commit --amend`.
/// The message is single-quoted for git's exec shell (`'\''` escaping);
/// `--allow-empty` lets the amend succeed on a commit with no tree change.
/// The exec mechanism is git's own (the sequencer runs it through a shell) —
/// the app's subprocesses stay argv-only (SPEC §16).
fn amend_exec_line(message: &str) -> Result<String, GitError> {
    if message.contains('\n') || message.contains('\r') {
        return Err(GitError::RebaseMessageMultiLine);
    }
    Ok(format!(
        "exec git commit --amend --allow-empty -m '{}'",
        message.replace('\'', "'\\''")
    ))
}

/// Write the prepared todo into a shell stub inside the git directory: git
/// runs the stub as its sequence editor with the real todo file as `$1`, and
/// the stub overwrites that file with the edited todo (quoted heredoc — no
/// interpolation). The content is deterministic for the op: a preview plan
/// writes the exact file the run plan then executes.
fn write_rebase_todo_script(repo: &Repo, todo: &[String]) -> Result<String, GitError> {
    let script = repo.inner.path().join("gitgotten-todo.sh");
    let body = format!(
        "#!/bin/sh\ncat > \"$1\" <<'GITGOTTEN_TODO'\n{}\nGITGOTTEN_TODO\n",
        todo.join("\n")
    );
    std::fs::write(&script, body)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))?;
    }
    // The editor runs through git's bundled shell even on Windows, so the
    // path must be in slash form (`C:/…` is what that shell understands).
    Ok(script.to_string_lossy().replace('\\', "/"))
}

/// `git -c sequence.editor=sh '<git-dir>/gitgotten-todo.sh' rebase -i <target>` —
/// `Mutating` (`Dangerous`, type-to-confirm, when rebased commits are already
/// on a remote): drive `git rebase -i` non-interactively with the edited todo
/// (SPEC §10.4). The todo runs through git's own sequencer, so a conflict or
/// an `edit` pauses the rebase and the Stage 8 Continue/Abort flow resumes it.
fn plan_rebase(
    repo: &Repo,
    status: &Status,
    target: &str,
    edits: &[RebaseEdit],
) -> Result<CommandPlan, GitError> {
    let branch = status.branch.as_ref().ok_or(GitError::DetachedHead)?;
    let range = rebase_range(repo, target)?;
    if edits.is_empty() {
        return Err(GitError::EmptySelection);
    }
    // Resolve every edit hash to a full oid; the todo must name each commit of
    // `target..HEAD` exactly once.
    let subjects: HashMap<&str, &str> = range
        .iter()
        .map(|(hash, subject)| (hash.as_str(), subject.as_str()))
        .collect();
    let mut given = HashSet::new();
    for edit in edits {
        let oid = repo.commit_at(&edit.hash).map(|c| c.id().to_string())?;
        given.insert(oid);
    }
    if given.len() != edits.len() || range.len() != given.len() {
        return Err(GitError::RebaseTodoMismatch {
            expected: range.len(),
            given: edits.len(),
        });
    }
    for (hash, _) in &range {
        if !given.contains(hash.as_str()) {
            return Err(GitError::RebaseTodoMismatch {
                expected: range.len(),
                given: edits.len(),
            });
        }
    }

    // UI order (newest-first) → git's todo (oldest-first). Reword and squash
    // messages ride as `exec git commit --amend` lines: the todo below *is*
    // what runs, and the preview lists it.
    let mut todo: Vec<String> = Vec::new();
    for (index, edit) in edits.iter().rev().enumerate() {
        let oid = repo.commit_at(&edit.hash).map(|c| c.id().to_string())?;
        match edit.action {
            RebaseAction::Pick => todo.push(format!("pick {oid}")),
            RebaseAction::Reword => {
                let message =
                    edit.message
                        .as_deref()
                        .ok_or_else(|| GitError::RebaseEditNeedsMessage {
                            hash: edit.hash.clone(),
                        })?;
                todo.push(format!("pick {oid}"));
                todo.push(amend_exec_line(message)?);
            }
            RebaseAction::Squash => {
                if index == 0 {
                    return Err(GitError::RebaseTodoOrder);
                }
                todo.push(format!("squash {oid}"));
                if let Some(message) = edit.message.as_deref() {
                    todo.push(amend_exec_line(message)?);
                }
            }
            RebaseAction::Fixup => {
                if index == 0 {
                    return Err(GitError::RebaseTodoOrder);
                }
                todo.push(format!("fixup {oid}"));
            }
            RebaseAction::Edit => todo.push(format!("edit {oid}")),
            // A drop is a todo entry that never gets written.
            RebaseAction::Drop => {}
        }
    }

    let script = write_rebase_todo_script(repo, &todo)?;
    // Single-quoted for the shell git launches the editor through.
    let config = format!("sequence.editor=sh '{}'", script.replace('\'', "'\\''"));
    let target = target.trim();
    let argv = vec![
        "git".to_string(),
        "-c".to_string(),
        config,
        "rebase".to_string(),
        "-i".to_string(),
        target.to_string(),
    ];
    let display = format_argv(&argv);

    let range_hashes: Vec<String> = range.iter().map(|(hash, _)| hash.clone()).collect();
    let published = published_count(repo, &range_hashes)?;
    let (risk, effect) = rebase_risk(published);
    let mut lines: Vec<String> = edits
        .iter()
        .map(|edit| {
            let subject = subjects
                .get(edit.hash.as_str())
                .copied()
                .unwrap_or_else(|| edit.hash.as_str());
            let action = match edit.action {
                RebaseAction::Pick => "pick".to_string(),
                RebaseAction::Reword => {
                    format!("reword → \"{}\"", edit.message.as_deref().unwrap_or(""))
                }
                RebaseAction::Squash => match edit.message.as_deref() {
                    Some(message) => format!("squash → {message}"),
                    None => "squash (git's combined message)".to_string(),
                },
                RebaseAction::Fixup => "fixup".to_string(),
                RebaseAction::Edit => "edit (pause)".to_string(),
                RebaseAction::Drop => "drop".to_string(),
            };
            format!("  {}: {action} {subject}", short_hash(&edit.hash))
        })
        .collect();
    if published > 0 {
        lines.push(published_warning(published));
    }

    Ok(CommandPlan {
        steps: vec![GitStep {
            argv,
            summary: format!("rebase {branch} onto {target}"),
            risk,
            effect,
            input: None,
            program: None,
        }],
        display,
        preview: Some(Preview {
            headline: format!("Rebase {branch} onto {target}"),
            lines,
            requires_type_to_confirm: published > 0,
        }),
    })
}

/// `git rebase <target>` — `Mutating` (`Dangerous`, type-to-confirm, when
/// rebased commits are already on a remote): non-interactive rebase of the
/// current branch onto a branch/commit-ish (SPEC §10.4 "rebase-onto").
fn plan_rebase_onto(repo: &Repo, status: &Status, target: &str) -> Result<CommandPlan, GitError> {
    let branch = status.branch.as_ref().ok_or(GitError::DetachedHead)?;
    let range = rebase_range(repo, target)?;
    let hashes: Vec<String> = range.iter().map(|(hash, _)| hash.clone()).collect();
    let published = published_count(repo, &hashes)?;
    let (risk, effect) = rebase_risk(published);
    let count = range.len();
    let mut lines = vec![format!(
        "rewrites {count} local commit{} above {target}",
        if count == 1 { "" } else { "s" }
    )];
    if published > 0 {
        lines.push(published_warning(published));
    }
    let target = target.trim();
    let argv = vec!["git".to_string(), "rebase".to_string(), target.to_string()];
    let display = format_argv(&argv);
    Ok(CommandPlan {
        steps: vec![GitStep {
            argv,
            summary: format!("rebase {branch} onto {target}"),
            risk,
            effect,
            input: None,
            program: None,
        }],
        display,
        preview: Some(Preview {
            headline: format!("Rebase {branch} onto {target}"),
            lines,
            requires_type_to_confirm: published > 0,
        }),
    })
}

/// `git add [-A | -- <path>]` — `Mutating`: stage one file (untracked →
/// newly added) or the whole tree.
fn plan_stage(status: &Status, path: Option<&str>) -> Result<CommandPlan, GitError> {
    let (argv, summary, headline) = match path {
        Some(p) => (
            vec![
                "git".to_string(),
                "add".to_string(),
                "--".to_string(),
                p.to_string(),
            ],
            format!("stage {p}"),
            format!("Stage {p}"),
        ),
        None => (
            vec!["git".to_string(), "add".to_string(), "-A".to_string()],
            "stage all changes".to_string(),
            "Stage all changes".to_string(),
        ),
    };
    let lines = vec![match path {
        Some(p) => format!("  {p}"),
        None => format!("  {} changed files", status.changes.len()),
    }];
    let preview = Preview {
        headline,
        lines,
        requires_type_to_confirm: false,
    };
    let display = format_argv(&argv);
    Ok(CommandPlan {
        steps: vec![GitStep {
            argv,
            summary,
            risk: Risk::Mutating,
            effect: None,
            input: None,
            program: None,
        }],
        display,
        preview: Some(preview),
    })
}

/// `git reset --<path | all>` — `Mutating`: the index entries go back to
/// HEAD while the worktree is left untouched (changes reappear unstaged; a
/// staged *new* file becomes untracked with its content preserved).
fn plan_unstage(status: &Status, path: Option<&str>) -> Result<CommandPlan, GitError> {
    let (argv, summary, headline) = match path {
        Some(p) => (
            vec![
                "git".to_string(),
                "reset".to_string(),
                "-q".to_string(),
                "--".to_string(),
                p.to_string(),
            ],
            format!("unstage {p} (worktree untouched)"),
            format!("Unstage {p}"),
        ),
        None => (
            vec!["git".to_string(), "reset".to_string(), "-q".to_string()],
            "unstage all (worktree untouched)".to_string(),
            "Unstage all changes".to_string(),
        ),
    };
    let lines = vec![match path {
        Some(p) => format!("  {p} (content kept in the worktree)"),
        None => format!(
            "  {} changed files (content kept in the worktree)",
            status.changes.len()
        ),
    }];
    let preview = Preview {
        headline,
        lines,
        requires_type_to_confirm: false,
    };
    let display = format_argv(&argv);
    Ok(CommandPlan {
        steps: vec![GitStep {
            argv,
            summary,
            risk: Risk::Mutating,
            effect: None,
            input: None,
            program: None,
        }],
        display,
        preview: Some(preview),
    })
}

/// Discard a file's uncommitted changes — `Dangerous` (type-to-confirm).
///
/// Tracked files: `git restore --source=HEAD --staged --worktree -- <path>`
/// restores *both* the index entry and the worktree file from HEAD (a
/// staged-new file is deleted; a deleted file comes back). Untracked files
/// have nothing to restore, so they are deleted: `git clean -f -- <path>`.
/// The preview shows the diff being discarded (SPEC §10.2).
fn plan_discard(repo: &Repo, path: &str) -> Result<CommandPlan, GitError> {
    let untracked = repo
        .status()?
        .changes
        .iter()
        .any(|c| c.path == path && c.kind == ChangeKind::Untracked);

    let diff = repo.diff_file(path)?;
    let mut lines = vec![path.to_string()];
    if diff.binary {
        lines.push("  (binary — the whole file is discarded)".to_string());
    } else {
        for h in &diff.hunks {
            lines.push(format!("  {}", h.header));
            for l in &h.lines {
                lines.push(format!("  {l}"));
            }
        }
    }
    let preview = Preview {
        headline: format!("Discard {path}"),
        lines,
        requires_type_to_confirm: true,
    };

    let (argv, summary) = if untracked {
        (
            vec![
                "git".to_string(),
                "clean".to_string(),
                "-f".to_string(),
                "--".to_string(),
                path.to_string(),
            ],
            format!("delete the untracked file {path}"),
        )
    } else {
        (
            vec![
                "git".to_string(),
                "restore".to_string(),
                "--source=HEAD".to_string(),
                "--staged".to_string(),
                "--worktree".to_string(),
                "--".to_string(),
                path.to_string(),
            ],
            format!("restore {path} from HEAD (index and worktree)"),
        )
    };
    let display = format_argv(&argv);
    Ok(CommandPlan {
        steps: vec![GitStep {
            argv,
            summary,
            risk: Risk::Dangerous,
            effect: Some(format!("destroys the uncommitted changes in {path}")),
            input: None,
            program: None,
        }],
        display,
        preview: Some(preview),
    })
}

/// `git apply --cached` with the selected hunks' patch on stdin — `Mutating`:
/// the index gains exactly the chosen hunks, the worktree is untouched.
///
/// `selected` indexes the web's view of the file (`Repo::diff_file(path).hunks`:
/// staged hunks first, then unstaged); only unstaged hunks are stageable, and
/// a selection that no longer matches the current diff is an error (the file
/// changed between viewing and staging — re-read rather than apply stale bytes).
fn plan_stage_hunks(repo: &Repo, path: &str, selected: &[usize]) -> Result<CommandPlan, GitError> {
    let untracked = repo
        .status()?
        .changes
        .iter()
        .any(|c| c.path == path && c.kind == ChangeKind::Untracked);
    let (staged_raw, unstaged_raw) = crate::diff::diff_sides(&repo.path, path, untracked)?;
    let staged_count = crate::diff::hunks_from(&staged_raw, true).len();
    let unstaged = crate::diff::hunks_from(&unstaged_raw, false);

    // Map the web's indices (into staged ++ unstaged) onto the raw unstaged
    // diff's hunks; drop anything that is staged or out of range.
    let selected: Vec<usize> = selected
        .iter()
        .filter(|&&i| i >= staged_count && i - staged_count < unstaged.len())
        .map(|&i| i - staged_count)
        .collect();
    if selected.is_empty() {
        return Err(GitError::EmptySelection);
    }
    let patch =
        crate::diff::build_apply_patch(&unstaged_raw, &selected).ok_or(GitError::EmptySelection)?;

    let total = unstaged.len();
    let count = selected.len();
    let mut lines = vec![path.to_string()];
    for &i in &selected {
        lines.push(format!("  {}", unstaged[i].header));
    }
    let preview = Preview {
        headline: format!("Stage {count} of {total} hunks"),
        lines,
        requires_type_to_confirm: false,
    };
    let argv = vec![
        "git".to_string(),
        "apply".to_string(),
        "--cached".to_string(),
    ];
    let display = format_argv(&argv);
    Ok(CommandPlan {
        steps: vec![GitStep {
            argv,
            summary: format!("stage {count} of {total} hunks in {path}"),
            risk: Risk::Mutating,
            effect: Some(format!(
                "updates the index for {path}; the worktree is untouched"
            )),
            input: Some(patch),
            program: None,
        }],
        display,
        preview: Some(preview),
    })
}

// ── Stage 8: conflict resolution (SPEC §10.5) ─────────────────────────────

/// `git checkout --ours|--theirs -- <path>` + `git add -- <path>` (whole-side
/// resolution) or `git apply` (stdin: the whole-file patch of the spliced
/// 3-way) + `git add -- <path>` (per-hunk resolution) — `Mutating`.
///
/// Resolving writes the chosen content to the worktree and stages it, which
/// clears the file's unmerged index entry. The continue/abort plans conclude
/// or roll back the operation itself.
fn plan_resolve_conflict(
    repo: &Repo,
    status: &Status,
    path: &str,
    resolution: &ConflictResolution,
) -> Result<CommandPlan, GitError> {
    if !status
        .changes
        .iter()
        .any(|c| c.path == path && c.kind == ChangeKind::Conflicted)
    {
        return Err(GitError::NoConflict {
            path: path.to_string(),
        });
    }
    let add_step = GitStep {
        argv: vec![
            "git".to_string(),
            "add".to_string(),
            "--".to_string(),
            path.to_string(),
        ],
        summary: format!("stage the resolved {path}"),
        risk: Risk::Mutating,
        effect: None,
        input: None,
        program: None,
    };
    match resolution {
        ConflictResolution::Side(side) => {
            let (flag, what) = match side {
                ConflictSide::Ours => ("--ours", "our"),
                ConflictSide::Theirs => ("--theirs", "their"),
            };
            let steps = vec![
                GitStep {
                    argv: vec![
                        "git".to_string(),
                        "checkout".to_string(),
                        flag.to_string(),
                        "--".to_string(),
                        path.to_string(),
                    ],
                    summary: format!("take the {what} version of {path}"),
                    risk: Risk::Mutating,
                    effect: None,
                    input: None,
                    program: None,
                },
                add_step,
            ];
            let display = steps
                .iter()
                .map(|s| format_argv(&s.argv))
                .collect::<Vec<_>>()
                .join(" && ");
            Ok(CommandPlan {
                display,
                steps,
                preview: Some(Preview {
                    headline: format!("Resolve {path}: take the {what} version"),
                    lines: vec![
                        "the file is written and staged; the unmerged entry is cleared".to_string(),
                    ],
                    requires_type_to_confirm: false,
                }),
            })
        }
        ConflictResolution::Hunks(choices) => {
            let binary = status
                .changes
                .iter()
                .find(|c| c.path == path)
                .map(|c| c.binary)
                .unwrap_or(false);
            if binary {
                return Err(GitError::BinaryConflict {
                    path: path.to_string(),
                });
            }
            // Re-derive the 3-way and splice the choices: the patch's preimage
            // is the current worktree file, so a file that changed in an
            // external editor between viewing and resolving makes `git apply`
            // fail loudly (safe failure, SPEC §8).
            let patch = conflict::resolve_patch(&repo.path, path, choices)?;
            let count = choices.len();
            let mut steps = Vec::new();
            if let Some(patch) = patch {
                steps.push(GitStep {
                    argv: vec!["git".to_string(), "apply".to_string()],
                    summary: format!("write the resolved {path}"),
                    risk: Risk::Mutating,
                    effect: Some(format!(
                        "replaces the conflicted file content ({count} region choice(s))"
                    )),
                    input: Some(patch),
                    program: None,
                });
            }
            steps.push(add_step);
            let display = steps
                .iter()
                .map(|s| format_argv(&s.argv))
                .collect::<Vec<_>>()
                .join(" && ");
            Ok(CommandPlan {
                display,
                steps,
                preview: Some(Preview {
                    headline: format!("Resolve {path} ({count} region choice(s))"),
                    lines: vec![
                        "the resolved content is written and staged; the unmerged entry is cleared"
                            .to_string(),
                    ],
                    requires_type_to_confirm: false,
                }),
            })
        }
    }
}

/// `git <merge|rebase|cherry-pick> --continue` — `Mutating`. One plan for the
/// three in-progress operations (detected from the repository state); git
/// itself refuses when files are still unmerged.
fn plan_continue_op(repo: &Repo, status: &Status) -> Result<CommandPlan, GitError> {
    let Some(op) = repo.in_progress_op() else {
        return Err(GitError::NoOpInProgress);
    };
    let label = op.label();
    let unmerged = status
        .changes
        .iter()
        .filter(|c| c.kind == ChangeKind::Conflicted)
        .count();
    let lines = if unmerged > 0 {
        vec![format!(
            "{unmerged} file(s) still unmerged — resolve them first; git will refuse"
        )]
    } else {
        vec!["all conflicts resolved — this commits the operation's result".to_string()]
    };
    Ok(CommandPlan {
        display: format!("git {label} --continue"),
        steps: vec![GitStep {
            argv: vec![
                "git".to_string(),
                label.to_string(),
                "--continue".to_string(),
            ],
            summary: format!("conclude the {label} in progress"),
            risk: Risk::Mutating,
            effect: None,
            input: None,
            program: None,
        }],
        preview: Some(Preview {
            headline: format!("Complete the {label} in progress"),
            lines,
            requires_type_to_confirm: false,
        }),
    })
}

/// `git <merge|rebase|cherry-pick> --abort` — `Dangerous` (type-to-confirm):
/// aborting discards the in-progress operation and any resolution work done
/// during it, restoring the pre-operation state.
fn plan_abort_op(repo: &Repo, status: &Status) -> Result<CommandPlan, GitError> {
    let Some(op) = repo.in_progress_op() else {
        return Err(GitError::NoOpInProgress);
    };
    let label = op.label();
    let unmerged = status
        .changes
        .iter()
        .filter(|c| c.kind == ChangeKind::Conflicted)
        .count();
    let mut lines =
        vec!["restores the repository to its state before the operation started".to_string()];
    if unmerged > 0 {
        lines.push(format!(
            "{unmerged} unresolved conflict file(s) and any edits made during the operation are discarded"
        ));
    }
    Ok(CommandPlan {
        display: format!("git {label} --abort"),
        steps: vec![GitStep {
            argv: vec!["git".to_string(), label.to_string(), "--abort".to_string()],
            summary: format!("abort the {label} in progress"),
            risk: Risk::Dangerous,
            effect: Some("discards the in-progress operation and its resolution work".to_string()),
            input: None,
            program: None,
        }],
        preview: Some(Preview {
            headline: format!("Abort the {label} in progress"),
            lines,
            requires_type_to_confirm: true,
        }),
    })
}

/// Launch the configured external editor (resolved `core.editor` → `$VISUAL`
/// → `$EDITOR` → `vi`) on the given paths — `Mutating` (the editor can modify
/// the files; the app cannot see those edits until the files are re-read).
/// `argv[0]` is the editor program, so the step carries an explicit
/// [`GitStep::program`] and the display includes it.
fn plan_open_editor(repo: &Repo, paths: &[String]) -> Result<CommandPlan, GitError> {
    if paths.is_empty() || paths.iter().any(|p| p.trim().is_empty()) {
        return Err(GitError::EmptySelection);
    }
    let mut argv = conflict::editor_argv(&repo.path);
    let program = argv[0].clone();
    argv.extend(paths.iter().cloned());
    let count = paths.len();
    Ok(CommandPlan {
        display: format_argv(&argv),
        steps: vec![GitStep {
            argv,
            summary: format!("open {count} file(s) in {program}"),
            risk: Risk::Mutating,
            effect: Some(format!(
                "launches {program}; edits made in it are not visible to the app until the files are re-read"
            )),
            input: None,
            program: Some(program.clone()),
        }],
        preview: Some(Preview {
            headline: format!("Open {count} file(s) in the external editor"),
            lines: vec![format!(
                "launches `{program}` — the app resumes when the editor closes"
            )],
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
        let argv: Vec<String> = ["git", "push", "origin", "main"]
            .into_iter()
            .map(String::from)
            .collect();
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
        let argv = vec![
            "git".into(),
            "commit".into(),
            "-m".into(),
            "say \"hi\"".into(),
        ];
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
                author: None,
                co_authors: vec![],
            })
            .unwrap();

        assert_eq!(plan.display, r#"git commit -m "add b""#);
        assert_eq!(
            plan.steps[0].argv,
            vec![
                "git".to_string(),
                "commit".to_string(),
                "-m".to_string(),
                "add b".into()
            ]
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
                author: None,
                co_authors: vec![],
            })
            .unwrap();

        assert_eq!(plan.display, r#"git commit -m "amend it" --amend"#);
        assert_eq!(plan.steps[0].risk, Risk::Mutating);
        assert!(plan.steps[0].effect.as_deref() == Some("rewrites the last commit"));
        let preview = plan.preview.as_ref().unwrap();
        assert!(
            preview
                .lines
                .iter()
                .any(|l| l.contains("rewrites the last commit")),
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
                author: None,
                co_authors: vec![],
            })
            .unwrap();
        let preview = plan.preview.as_ref().unwrap();
        assert!(
            preview
                .lines
                .iter()
                .any(|l| l.contains("no staged changes"))
        );
    }

    #[test]
    fn plan_commit_rejects_an_empty_message() {
        let fx = committed_fixture();
        let repo = Repo::open(&fx.path).unwrap();
        let err = repo
            .plan(&Op::Commit {
                message: "   ".into(),
                amend: false,
                author: None,
                co_authors: vec![],
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
                set_upstream: false,
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
        assert!(
            preview
                .lines
                .iter()
                .any(|l| l.contains("pushes 1 new commit"))
        );
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
                set_upstream: false,
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
                set_upstream: false,
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
                set_upstream: false,
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
                author: None,
                co_authors: vec![],
            })
            .unwrap()
            .steps[0]
                .risk,
            Risk::Mutating
        );
        assert_eq!(
            repo.plan(&Op::Checkout {
                target: "x".into(),
                force: false,
            })
            .unwrap()
            .steps[0]
                .risk,
            Risk::Mutating
        );
        assert_eq!(
            repo.plan(&Op::Push {
                remote: "origin".into(),
                branch: Some("main".into()),
                force: true,
                set_upstream: false,
            })
            .unwrap()
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
            })
            .unwrap()
            .steps[0]
                .risk,
            Risk::Mutating
        );
        assert_eq!(
            repo.plan(&Op::Reset {
                mode: ResetMode::Hard,
                target: head.clone()
            })
            .unwrap()
            .steps[0]
                .risk,
            Risk::Dangerous
        );
        assert_eq!(
            repo.plan(&Op::CherryPick {
                hashes: vec![head],
                target: None,
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
        for name in [
            "",
            "  ",
            "-leading-dash",
            "has space",
            "up/../down",
            "weird:colon",
        ] {
            let err = repo
                .plan(&Op::BranchCreate {
                    name: name.into(),
                    from: head.clone(),
                    checkout: false,
                })
                .unwrap_err();
            assert!(
                matches!(err, GitError::BadRefName { .. }),
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
                target: None,
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
            .plan(&Op::CherryPick {
                hashes: vec![],
                target: None,
            })
            .unwrap_err();
        assert!(matches!(err, GitError::EmptySelection), "got {err:?}");
    }

    #[test]
    fn plan_cherry_pick_rejects_an_unknown_hash() {
        let (_fx, repo) = branch_fixture();
        let err = repo
            .plan(&Op::CherryPick {
                hashes: vec!["zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz".into()],
                target: None,
            })
            .unwrap_err();
        assert!(matches!(err, GitError::NoCommit { .. }), "got {err:?}");
    }

    // ── stage / unstage / discard / stage-hunks (Stage 5) ─────────────────

    /// A committed 20-line file with two modified lines (1 and 15) apart
    /// enough to yield **two** hunks.
    fn two_hunk_fixture() -> testutil::Fixture {
        let fx = testutil::Fixture::new("plan-two-hunk");
        let content = (1..=20)
            .map(|i| format!("l{i}"))
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        fx.commit_file("f.txt", &content, "c1");
        let mut lines: Vec<&str> = content.lines().collect();
        lines[0] = "l1X";
        lines[14] = "l15X";
        fx.write_file("f.txt", &(lines.join("\n") + "\n"));
        fx
    }

    #[test]
    fn plan_stage_file_and_all() {
        let fx = two_hunk_fixture();
        let repo = Repo::open(&fx.path).unwrap();

        let file = repo
            .plan(&Op::Stage {
                path: Some("f.txt".into()),
            })
            .unwrap();
        assert_eq!(file.display, "git add -- f.txt");
        assert_eq!(file.steps[0].risk, Risk::Mutating);
        assert!(!file.preview.as_ref().unwrap().requires_type_to_confirm);

        let all = repo.plan(&Op::Stage { path: None }).unwrap();
        assert_eq!(all.display, "git add -A");
        assert!(
            all.preview
                .as_ref()
                .unwrap()
                .lines
                .iter()
                .any(|l| l.contains("1 changed file"))
        );
    }

    #[test]
    fn plan_unstage_file_and_all() {
        let fx = two_hunk_fixture();
        let repo = Repo::open(&fx.path).unwrap();

        let file = repo
            .plan(&Op::Unstage {
                path: Some("f.txt".into()),
            })
            .unwrap();
        assert_eq!(file.display, "git reset -q -- f.txt");
        assert_eq!(file.steps[0].risk, Risk::Mutating);

        let all = repo.plan(&Op::Unstage { path: None }).unwrap();
        assert_eq!(all.display, "git reset -q");
    }

    #[test]
    fn plan_discard_is_dangerous_and_shows_the_diff() {
        let fx = two_hunk_fixture();
        let repo = Repo::open(&fx.path).unwrap();

        let plan = repo
            .plan(&Op::Discard {
                path: "f.txt".into(),
            })
            .unwrap();
        assert_eq!(
            plan.steps[0].argv,
            vec![
                "git".to_string(),
                "restore".to_string(),
                "--source=HEAD".to_string(),
                "--staged".to_string(),
                "--worktree".to_string(),
                "--".to_string(),
                "f.txt".to_string()
            ]
        );
        assert_eq!(plan.steps[0].risk, Risk::Dangerous);
        let preview = plan.preview.as_ref().unwrap();
        assert!(preview.requires_type_to_confirm);
        // The diff being discarded is in the preview (SPEC §10.2).
        assert!(preview.lines.iter().any(|l| l.contains("l1X")));
    }

    #[test]
    fn plan_stage_hunks_compiles_the_apply_step() {
        let fx = two_hunk_fixture();
        let repo = Repo::open(&fx.path).unwrap();

        // Both hunks are unstaged, so the web's indices 0 and 1 map 1:1.
        let plan = repo
            .plan(&Op::StageHunks {
                path: "f.txt".into(),
                selected: vec![0],
            })
            .unwrap();
        assert_eq!(plan.display, "git apply --cached");
        assert_eq!(plan.steps[0].risk, Risk::Mutating);
        let input = plan.steps[0].input.as_deref().expect("patch on stdin");
        assert!(input.contains("-l1\n+l1X"), "hunk 1 selected: {input}");
        assert!(!input.contains("-l15"), "hunk 2 not selected: {input}");
        assert!(
            input.contains("--- a/f.txt"),
            "apply patch needs the file header: {input}"
        );
        let preview = plan.preview.as_ref().unwrap();
        assert_eq!(preview.headline, "Stage 1 of 2 hunks");
    }

    #[test]
    fn plan_stage_hunks_rejects_an_empty_selection() {
        let fx = two_hunk_fixture();
        let repo = Repo::open(&fx.path).unwrap();
        let err = repo
            .plan(&Op::StageHunks {
                path: "f.txt".into(),
                selected: vec![],
            })
            .unwrap_err();
        assert!(matches!(err, GitError::EmptySelection), "got {err:?}");
    }

    #[test]
    fn plan_stage_hunks_maps_over_staged_hunks() {
        let fx = two_hunk_fixture();
        fx.add("f.txt"); // stage everything: both hunks are now staged
        let repo = Repo::open(&fx.path).unwrap();
        // The web's hunk list is [staged, staged] — there is nothing to stage,
        // so any selection is rejected.
        let err = repo
            .plan(&Op::StageHunks {
                path: "f.txt".into(),
                selected: vec![0, 1],
            })
            .unwrap_err();
        assert!(matches!(err, GitError::EmptySelection), "got {err:?}");
    }

    #[test]
    fn plan_commit_carries_author_and_co_authors_in_the_display() {
        let fx = two_hunk_fixture();
        fx.add("f.txt");
        let repo = Repo::open(&fx.path).unwrap();

        let plan = repo
            .plan(&Op::Commit {
                message: "fix: the thing".into(),
                amend: false,
                author: Some(Author {
                    name: "Jane".into(),
                    email: "jane@example.com".into(),
                }),
                co_authors: vec!["Joe <joe@example.com>".into()],
            })
            .unwrap();
        // The trailer rides in the message and the identity in `--author`;
        // the displayed command is the exact argv (SPEC §8).
        assert!(plan.display.contains("-m \"fix: the thing"));
        assert!(
            plan.display
                .contains("Co-authored-by: Joe <joe@example.com>")
        );
        assert!(
            plan.display
                .contains(r#"--author "Jane <jane@example.com>""#)
        );
    }
}
