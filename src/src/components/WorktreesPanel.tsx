import { useState } from "react";
import { useActivityStore, useRepoStore, useWorktreesStore } from "../store";
import type { Op } from "../types";

/**
 * Worktrees tab (SPEC §10.7): every checkout of the active repo — branch (or
 * detached commit), path, and main/current/locked/prunable badges — with
 * switch-to-worktree (opens that worktree root as the active repo, which
 * retargets status + graph + every op), and add/remove ops. Removing a dirty
 * worktree with force is classified `Dangerous` by the core, so the preview
 * dialog gates it behind type-to-confirm. "Remove worktree" is the git-side
 * action, distinct from forgetting a repo in the nav.
 */
export function WorktreesPanel() {
  const { worktrees, loading, error } = useWorktreesStore();
  const activeId = useRepoStore((s) => s.activeId);
  const status = useRepoStore((s) => (s.activeId ? s.status[s.activeId] : undefined));
  const { requestPreview, running } = useActivityStore();
  const [adding, setAdding] = useState(false);

  const act = (op: Op) => {
    if (activeId) {
      void requestPreview(activeId, op);
    }
  };

  if (error) {
    return (
      <div className="refs-panel">
        <div className="refs-state">Failed to load worktrees: {error}</div>
      </div>
    );
  }

  return (
    <div className="refs-panel">
      <div className="refs-head">
        <h3>Worktrees</h3>
        <button type="button" className="btn" disabled={running} onClick={() => setAdding(true)}>
          Add worktree
        </button>
      </div>
      {loading && worktrees.length === 0 ? (
        <div className="refs-state">Loading…</div>
      ) : worktrees.length === 0 ? (
        <div className="refs-state">No worktrees.</div>
      ) : (
        <ul className="refs-list">
          {worktrees.map((w) => {
            const current = w.path === activeId;
            const label = w.branch ?? `detached ${w.head.slice(0, 7)}`;
            return (
              <li key={w.path} className={`refs-row ${current ? "current" : ""}`}>
                <span className="refs-name">{label}</span>
                <code className="refs-path" title={w.path}>
                  {w.path}
                </code>
                {w.is_main && <span className="badge">main worktree</span>}
                {current && <span className="badge">current</span>}
                {w.locked && <span className="badge">locked</span>}
                {w.prunable && <span className="badge">prunable</span>}
                <span className="row-actions">
                  {!current && (
                    <button
                      type="button"
                      className="btn row-btn"
                      disabled={running}
                      onClick={() => void useRepoStore.getState().open(w.path)}
                    >
                      Switch
                    </button>
                  )}
                  {!w.is_main && (
                    <>
                      <button
                        type="button"
                        className="btn row-btn"
                        disabled={running}
                        onClick={() => act({ kind: "worktree_remove", path: w.path, force: false })}
                      >
                        Remove
                      </button>
                      <button
                        type="button"
                        className="btn row-btn"
                        disabled={running}
                        onClick={() => act({ kind: "worktree_remove", path: w.path, force: true })}
                      >
                        Force
                      </button>
                    </>
                  )}
                </span>
              </li>
            );
          })}
        </ul>
      )}
      {adding && (
        <AddWorktreeDialog
          start={status?.branch ?? status?.detached_head ?? "HEAD"}
          onClose={() => setAdding(false)}
          act={act}
          running={running}
        />
      )}
    </div>
  );
}

/** The add-worktree dialog (SPEC §10.7): a new directory, a start point
 * (existing branch or commit), and an optional new branch to create there. */
function AddWorktreeDialog({
  start,
  onClose,
  act,
  running,
}: {
  start: string;
  onClose: () => void;
  act: (op: Op) => void;
  running: boolean;
}) {
  const [path, setPath] = useState("");
  const [committish, setCommittish] = useState(start);
  const [newBranch, setNewBranch] = useState("");

  return (
    <div
      className="dialog-overlay"
      role="presentation"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget && !running) {
          onClose();
        }
      }}
    >
      <div className="dialog" role="dialog" aria-modal="true" aria-label="Add worktree">
        <div className="dialog-head">
          <h2>Add worktree</h2>
        </div>
        <input
          className="dialog-input"
          placeholder="new directory path"
          value={path}
          onChange={(e) => setPath(e.target.value)}
          autoFocus
        />
        <input
          className="dialog-input"
          placeholder="branch or commit to check out"
          value={committish}
          onChange={(e) => setCommittish(e.target.value)}
        />
        <input
          className="dialog-input"
          placeholder="new branch name (optional)"
          value={newBranch}
          onChange={(e) => setNewBranch(e.target.value)}
        />
        <div className="dialog-actions">
          <button type="button" className="btn" disabled={running} onClick={onClose}>
            Cancel
          </button>
          <button
            type="button"
            className="btn primary"
            disabled={running || !path.trim() || !committish.trim()}
            onClick={() =>
              act({
                kind: "worktree_add",
                committish: committish.trim(),
                path: path.trim(),
                new_branch: newBranch.trim() || null,
              })
            }
          >
            Add
          </button>
        </div>
      </div>
    </div>
  );
}
