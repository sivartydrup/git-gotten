import { useState } from "react";
import { useActivityStore, useConflictStore, useRepoStore } from "../store";
import type { ConflictSide, HunkChoice } from "../types";

/**
 * The Conflicts tab (SPEC §10.5): the conflicted files of the active repo,
 * and the selected file's 3-way view.
 *
 * Combined mode renders the file in order — unchanged spans as text, each
 * conflict region with its base/ours/theirs blocks and per-region choices
 * (our / their / both / manual). Sides mode shows the three full file
 * versions side by side (a user preference, per the SPEC). Whole-file
 * resolutions ("take our/their version") and the Continue / Abort / editor
 * actions live in the conflict banner, which stays visible on every tab.
 */
export function ConflictView() {
  const { activeId } = useRepoStore();
  const { files, file, conflict, choices, selectFile, setChoice, loading, error } =
    useConflictStore();
  const { requestPreview, busy, running } = useActivityStore();
  const [viewMode, setViewMode] = useState<"combined" | "sides">("combined");

  if (!activeId) {
    return (
      <div className="conflict-view">
        <p className="muted">Open a repository to see its conflicts.</p>
      </div>
    );
  }

  const disabled = busy || running;
  const resolveWhole = (side: ConflictSide) => {
    if (!file) {
      return;
    }
    void requestPreview(activeId, {
      kind: "resolve_conflict",
      path: file,
      resolution: { Side: side },
    });
  };
  const resolveRegions = () => {
    if (!file || !conflict) {
      return;
    }
    void requestPreview(activeId, {
      kind: "resolve_conflict",
      path: file,
      resolution: { Hunks: choices },
    });
  };

  let regionIndex = -1;

  return (
    <div className="conflict-view">
      <aside className="conflict-files">
        <h3>
          Conflicted <span className="count">({files.length})</span>
        </h3>
        {files.length === 0 ? (
          <p className="muted">No unmerged files.</p>
        ) : (
          <ul className="changes">
            {files.map((path) => (
              <li
                key={path}
                className={`change file ${file === path ? "selected" : ""}`}
                onClick={() => selectFile(file === path ? null : path)}
              >
                <span className="badge kind kind-conflicted">C</span>
                <span className="change-path">{path}</span>
              </li>
            ))}
          </ul>
        )}
      </aside>

      <div className="conflict-detail">
        {!file || !conflict ? (
          error ? (
            <p className="error">{error}</p>
          ) : loading ? (
            <p className="muted">Reading the 3-way view…</p>
          ) : (
            <p className="muted">Select a conflicted file to resolve it.</p>
          )
        ) : (
          <>
            <div className="conflict-header">
              <h3 className="conflict-path">{conflict.path}</h3>
              {!conflict.binary && (
                <div className="conflict-viewmodes">
                  <button
                    type="button"
                    className={`tab ${viewMode === "combined" ? "active" : ""}`}
                    onClick={() => setViewMode("combined")}
                  >
                    Combined
                  </button>
                  <button
                    type="button"
                    className={`tab ${viewMode === "sides" ? "active" : ""}`}
                    onClick={() => setViewMode("sides")}
                  >
                    Sides
                  </button>
                </div>
              )}
              <div className="conflict-whole-actions">
                <button
                  type="button"
                  disabled={disabled}
                  onClick={() => resolveWhole("Ours")}
                >
                  Take our version
                </button>
                <button
                  type="button"
                  disabled={disabled}
                  onClick={() => resolveWhole("Theirs")}
                >
                  Take their version
                </button>
              </div>
            </div>

            {conflict.binary ? (
              <p className="conflict-binary">
                Binary file — only a whole-file resolution is available (the
                buttons above).
              </p>
            ) : viewMode === "sides" ? (
              <div className="conflict-sides">
                <div className="conflict-side">
                  <h4>Base</h4>
                  <pre>{conflict.base || "(empty)"}</pre>
                </div>
                <div className="conflict-side">
                  <h4>Ours</h4>
                  <pre>{conflict.ours || "(empty)"}</pre>
                </div>
                <div className="conflict-side">
                  <h4>Theirs</h4>
                  <pre>{conflict.theirs || "(empty)"}</pre>
                </div>
              </div>
            ) : (
              <div className="conflict-combined">
                {conflict.segments.map((segment, i) => {
                  if ("Common" in segment) {
                    return (
                      <pre key={i} className="conflict-common">
                        {segment.Common}
                      </pre>
                    );
                  }
                  regionIndex += 1;
                  const index = regionIndex;
                  const hunk = segment.Conflict;
                  const choice: HunkChoice = choices[index] ?? "AcceptOurs";
                  const manual =
                    typeof choice === "object" && "Custom" in choice ? choice.Custom : null;
                  return (
                    <div key={i} className="conflict-region">
                      <div className="conflict-region-choices">
                        {(
                          [
                            ["Our", "AcceptOurs"],
                            ["Their", "AcceptTheirs"],
                            ["Both", "AcceptBoth"],
                          ] as const
                        ).map(([label, value]) => (
                          <button
                            key={value}
                            type="button"
                            className={`choice ${choice === value ? "active" : ""}`}
                            disabled={disabled}
                            onClick={() => setChoice(index, value)}
                          >
                            {label}
                          </button>
                        ))}
                        <button
                          type="button"
                          className={`choice ${manual !== null ? "active" : ""}`}
                          disabled={disabled}
                          onClick={() => setChoice(index, { Custom: manual ?? hunk.ours })}
                        >
                          Manual
                        </button>
                      </div>
                      <div className="conflict-region-sides">
                        <div className="conflict-side">
                          <h4>Base</h4>
                          <pre>{hunk.base || "(empty)"}</pre>
                        </div>
                        <div className="conflict-side">
                          <h4>Ours</h4>
                          <pre>{hunk.ours || "(empty)"}</pre>
                        </div>
                        <div className="conflict-side">
                          <h4>Theirs</h4>
                          <pre>{hunk.theirs || "(empty)"}</pre>
                        </div>
                      </div>
                      {manual !== null && (
                        <textarea
                          className="conflict-custom"
                          rows={5}
                          value={manual}
                          disabled={disabled}
                          onChange={(e) =>
                            setChoice(index, { Custom: e.target.value })
                          }
                        />
                      )}
                    </div>
                  );
                })}
                <div className="conflict-footer">
                  <button
                    type="button"
                    className="primary"
                    disabled={disabled}
                    onClick={resolveRegions}
                  >
                    Resolve file
                  </button>
                </div>
              </div>
            )}
          </>
        )}
      </div>
    </div>
  );
}
