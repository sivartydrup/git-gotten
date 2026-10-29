import { useState } from "react";
import { useActivityStore, useRepoStore, useStashesStore } from "../store";
import type { Op, Stash } from "../types";

/**
 * Stash tab (SPEC §10.6): every `stash@{n}` of the active repo — name, date,
 * branch, message and the files it carries — with push (optionally including
 * untracked files) and apply/pop/drop per entry. All four are `Mutating`
 * (drop is `Dangerous`) and run through the preview/confirm flow.
 */
export function StashPanel() {
  const { stashes, loading, error } = useStashesStore();
  const activeId = useRepoStore((s) => s.activeId);
  const { requestPreview, running } = useActivityStore();
  const [includingUntracked, setIncludingUntracked] = useState(false);

  const act = (op: Op) => {
    if (activeId) {
      void requestPreview(activeId, op);
    }
  };

  if (error) {
    return (
      <div className="refs-panel">
        <div className="refs-state">Failed to load stashes: {error}</div>
      </div>
    );
  }

  return (
    <div className="refs-panel">
      <div className="refs-head">
        <h3>Stashes</h3>
        <span className="stash-push">
          <label className="dialog-check">
            <input
              type="checkbox"
              checked={includingUntracked}
              onChange={(e) => setIncludingUntracked(e.target.checked)}
            />{" "}
            include untracked
          </label>
          <button
            type="button"
            className="btn"
            disabled={running}
            onClick={() => act({ kind: "stash_push", include_untracked: includingUntracked })}
          >
            Push
          </button>
          {stashes.length > 0 && (
            <button
              type="button"
              className="btn"
              disabled={running}
              onClick={() => act({ kind: "stash_drop_all" })}
            >
              Clear all
            </button>
          )}
        </span>
      </div>
      {loading && stashes.length === 0 ? (
        <div className="refs-state">Loading…</div>
      ) : stashes.length === 0 ? (
        <div className="refs-state">No stashes.</div>
      ) : (
        <ul className="refs-list">
          {stashes.map((s: Stash) => (
            <li key={s.id} className="refs-row">
              <span className="refs-name">{s.name}</span>
              <span className="refs-date">{s.date}</span>
              {s.branch && <span className="refs-branch">on {s.branch}</span>}
              <span className="refs-msg">{s.message}</span>
              <span className="row-actions">
                <button
                  type="button"
                  className="btn row-btn"
                  disabled={running}
                  onClick={() => act({ kind: "stash_apply", id: s.id })}
                >
                  Apply
                </button>
                <button
                  type="button"
                  className="btn row-btn"
                  disabled={running}
                  onClick={() => act({ kind: "stash_pop", id: s.id })}
                >
                  Pop
                </button>
                <button
                  type="button"
                  className="btn row-btn"
                  disabled={running}
                  onClick={() => act({ kind: "stash_drop", id: s.id })}
                >
                  Drop
                </button>
              </span>
              {s.files.length > 0 && (
                <ul className="stash-files">
                  {s.files.map((f) => (
                    <li key={f}>
                      <code className="refs-path" title={f}>
                        {f}
                      </code>
                    </li>
                  ))}
                </ul>
              )}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
