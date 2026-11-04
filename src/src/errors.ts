/**
 * SPEC §18 error UX: translate the git diagnostics the app can actually produce
 * into one plain-language line. It never replaces anything — the exact command
 * and its raw output stay visible everywhere (the transparency contract); this
 * is the summary shown above them. `null` means "nothing recognized", so the
 * raw output stands on its own with no invented explanation.
 */
const FRIENDLY: [RegExp, string][] = [
  [/index\.lock|cannot lock ref|unable to create .*: file exists/i, "Another git command is working in this repository — try again once it finishes."],
  [/would be overwritten|you have unstaged changes|you have uncommitted changes|local changes to the following files/i, "You have uncommitted changes in the way — commit, stash, or discard them first."],
  [/automatic merge failed|CONFLICT|unmerged files|resolve all conflicts first/i, "The merge stopped on conflicts — resolve them in the Conflicts tab."],
  [/no rebase in progress|no merge in progress|You have no current changes|no cherry-pick or revert in progress/i, "There is nothing to continue or abort — that operation already finished."],
  [/rejected|non-fast-forward|fetch first|behind its remote/i, "The remote has commits you do not have locally — pull before pushing."],
  [/no upstream branch|has no upstream|no tracking information/i, "This branch has no upstream to push to yet."],
  [/authentication failed|could not read Username|Permission denied \(publickey\)|Invalid or expired token|Write access denied/i, "The remote refused the credentials — check the remote URL and your access."],
  [/could not resolve host|failed to connect|Connection (refused|timed out|reset)|Could not access|unable to access/i, "The remote could not be reached — check the URL and your network."],
  [/not a git repository/i, "This folder is not a git repository."],
  [/already checked out|already used by another worktree/i, "That branch is already checked out in another worktree."],
  [/nothing to commit|no changes added to commit|no changes staged/i, "Nothing is staged, so there is nothing to commit."],
  [/already exists|name already/i, "That name is already taken."],
  [/bad revision|not a valid object name|unknown revision|Reference is not a valid branchname|does not exist/i, "That branch, tag, or commit does not exist."],
  [/Please tell me who you are|empty ident name|auto-detect email address/i, "git has no user name and email configured for this repository."],
];

/** The friendly one-liner for a git failure's captured output (or an IPC error
 *  string), or `null` when the text is not one of the known failures. */
export function friendlyGitError(raw: string): string | null {
  if (!raw) {
    return null;
  }
  for (const [pattern, message] of FRIENDLY) {
    if (pattern.test(raw)) {
      return message;
    }
  }
  return null;
}
