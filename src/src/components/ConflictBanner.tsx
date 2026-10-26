import { useActivityStore, useConflictStore, useRepoStore } from "../store";
import type { Status } from "../types";

/**
 * The recoverable-state banner (SPEC §10.5): shown on every tab whenever the
 * active repo is mid merge / rebase / cherry-pick. It tracks the
 * "k of N resolved" progress while files are unmerged, and offers the
 * operation's conclusions — Continue (commit the result), Abort (restore the
 * pre-operation state), and Open in editor (the external-editor escape
 * hatch) — through the transparent preview flow, so the exact git command is
 * always visible first.
 */
export function ConflictBanner() {
  const { activeId, status } = useRepoStore();
  const { files, total } = useConflictStore();
  const { requestPreview, busy, running } = useActivityStore();

  const activeStatus: Status | null = activeId ? (status[activeId] ?? null) : null;
  if (!activeStatus) {
    return null;
  }
  const unmerged = activeStatus.changes.filter((c) => c.kind === "Conflicted").length;
  const inOp = activeStatus.in_merge || activeStatus.in_rebase || activeStatus.in_cherry_pick;
  if (!inOp) {
    return null;
  }

  // The same precedence as the git-directory state markers: a rebase stop
  // (which may also carry a CHERRY_PICK_HEAD) reads first.
  const label = activeStatus.in_rebase
    ? "rebase"
    : activeStatus.in_merge
      ? "merge"
      : activeStatus.in_cherry_pick
        ? "cherry-pick"
        : "operation";

  const disabled = busy || running;
  const resolved = Math.max(0, total - unmerged);

  return (
    <div className="conflict-banner" role="status">
      <span className="conflict-banner-label">
        {label} in progress —{" "}
        {unmerged === 0 ? (
          <strong>ready to continue</strong>
        ) : total > 0 ? (
          <strong>
            {resolved} of {total} file{total === 1 ? "" : "s"} resolved
          </strong>
        ) : (
          <strong>
            {unmerged} unmerged file{unmerged === 1 ? "" : "s"}
          </strong>
        )}
      </span>
      <span className="conflict-banner-actions">
        <button
          type="button"
          className="primary"
          disabled={disabled || unmerged > 0}
          title={
            unmerged > 0 ? "Resolve all unmerged files first" : `git ${label} --continue`
          }
          onClick={() =>
            requestPreview(activeId!, { kind: "continue_op" })
          }
        >
          Continue
        </button>
        <button
          type="button"
          className="danger"
          disabled={disabled}
          title={`git ${label} --abort`}
          onClick={() => requestPreview(activeId!, { kind: "abort_op" })}
        >
          Abort
        </button>
        <button
          type="button"
          disabled={disabled || files.length === 0}
          title="Open the unmerged files in the configured editor"
          onClick={() =>
            requestPreview(activeId!, { kind: "open_editor", paths: files })
          }
        >
          Open in editor
        </button>
      </span>
    </div>
  );
}
