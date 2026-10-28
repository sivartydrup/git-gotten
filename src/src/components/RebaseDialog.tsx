import { useEffect, useRef, useState } from "react";
import { useActivityStore, useRebaseStore, useRepoStore } from "../store";
import type { RebaseAction, RebaseEdit } from "../types";

const ACTIONS: RebaseAction[] = ["Pick", "Reword", "Squash", "Fixup", "Edit", "Drop"];

/**
 * The RebaseDialog (SPEC §10.4): choose the target branch/commit (default:
 * the current branch's upstream), then edit the todo for the commits
 * `target..HEAD` the rebase would rewrite — drag rows (or ↑/↓) to reorder,
 * set each commit's action (pick/reword/squash/fixup/edit/drop), and give
 * reword/squash messages. Submitting compiles the edited todo through the
 * transparent preview flow (SPEC §9), including the type-to-confirm gate a
 * rebase of already-published commits gets (SPEC §8).
 */
export function RebaseDialog() {
  const { open, commits, loading, error, load, reset } = useRebaseStore();
  const activeId = useRepoStore((s) => s.activeId);
  const upstream = useRepoStore((s) =>
    s.activeId ? s.status[s.activeId]?.upstream ?? null : null,
  );
  const { requestPreview, running, busy } = useActivityStore();
  const [target, setTarget] = useState("");
  const [edits, setEdits] = useState<RebaseEdit[]>([]);
  const dragFrom = useRef(-1);

  // A fresh dialog rebases onto the current branch's upstream and lists its
  // range right away; a manual target loads on Enter.
  useEffect(() => {
    if (!open) {
      return;
    }
    setTarget(upstream ?? "");
    if (upstream && activeId) {
      void load(activeId, upstream);
    }
    // Deliberately keyed on `open` only: `target` reloads via Enter.
  }, [open]);

  // A fresh commit list starts as a plain pick todo (UI order, newest first).
  useEffect(() => {
    setEdits(
      commits.map((c) => ({ hash: c.hash, action: "Pick" as const, message: null })),
    );
  }, [commits]);

  if (!open) {
    return null;
  }

  const setAction = (i: number, action: RebaseAction) =>
    setEdits((es) => es.map((e, j) => (j === i ? { ...e, action } : e)));
  const setMessage = (i: number, message: string) =>
    setEdits((es) => es.map((e, j) => (j === i ? { ...e, message } : e)));
  const move = (from: number, to: number) =>
    setEdits((es) => {
      if (from < 0 || to < 0 || to >= es.length || from === to) {
        return es;
      }
      const next = [...es];
      const [row] = next.splice(from, 1);
      next.splice(to, 0, row);
      return next;
    });

  const needsMessage = edits.some((e) => e.action === "Reword" && !e.message?.trim());
  const disabled =
    running || busy || loading || !target.trim() || edits.length === 0 || needsMessage;

  const submit = () => {
    if (activeId) {
      void requestPreview(activeId, {
        kind: "rebase",
        target: target.trim(),
        edits,
      });
    }
    reset();
  };

  return (
    <div
      className="dialog-overlay"
      role="presentation"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget && !running) reset();
      }}
    >
      <div className="dialog" role="dialog" aria-modal="true" aria-label="Rebase">
        <div className="dialog-head">
          <h2>Interactive rebase</h2>
        </div>
        <input
          className="dialog-input"
          aria-label="Rebase target"
          value={target}
          placeholder="onto branch or commit (e.g. main)"
          onChange={(e) => setTarget(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && activeId && target.trim()) {
              void load(activeId, target.trim());
            }
          }}
        />
        {loading && <div className="rebase-state">Loading commits…</div>}
        {!loading && error && (
          <div className="rebase-state error">Failed to load commits: {error}</div>
        )}
        {!loading && !error && commits.length === 0 && (
          <div className="rebase-state">
            {target.trim()
              ? "Nothing to rebase onto this target"
              : "Enter a target branch or commit, then press Enter"}
          </div>
        )}
        {edits.length > 0 && (
          <ul className="rebase-list">
            {edits.map((e, i) => (
              <li
                key={e.hash}
                className="rebase-row"
                draggable
                onDragStart={() => {
                  dragFrom.current = i;
                }}
                onDragOver={(ev) => ev.preventDefault()}
                onDrop={(ev) => {
                  ev.preventDefault();
                  move(dragFrom.current, i);
                  dragFrom.current = -1;
                }}
              >
                <code className="rebase-hash">{e.hash.slice(0, 8)}</code>
                <span className="rebase-subject">
                  {commits.find((c) => c.hash === e.hash)?.subject}
                </span>
                <select
                  className="rebase-action"
                  aria-label={`action for ${e.hash.slice(0, 8)}`}
                  value={e.action}
                  onChange={(ev) => setAction(i, ev.target.value as RebaseAction)}
                >
                  {ACTIONS.map((a) => (
                    <option key={a} value={a}>
                      {a}
                    </option>
                  ))}
                </select>
                {(e.action === "Reword" || e.action === "Squash") && (
                  <input
                    className="rebase-message"
                    aria-label={`new message for ${e.hash.slice(0, 8)}`}
                    placeholder={e.action === "Reword" ? "new message" : "squash message (optional)"}
                    value={e.message ?? ""}
                    onChange={(ev) => setMessage(i, ev.target.value)}
                  />
                )}
                <button
                  type="button"
                  className="btn"
                  aria-label="Move down"
                  disabled={i === edits.length - 1}
                  onClick={() => move(i, i + 1)}
                >
                  ↓
                </button>
                <button
                  type="button"
                  className="btn"
                  aria-label="Move up"
                  disabled={i === 0}
                  onClick={() => move(i, i - 1)}
                >
                  ↑
                </button>
              </li>
            ))}
          </ul>
        )}
        {needsMessage && (
          <div className="rebase-hint error">A reword needs a new message.</div>
        )}
        <div className="dialog-actions">
          <button type="button" className="btn" disabled={running} onClick={reset}>
            Cancel
          </button>
          <button
            type="button"
            className="btn primary"
            disabled={disabled}
            onClick={submit}
          >
            Preview rebase
          </button>
        </div>
      </div>
    </div>
  );
}
