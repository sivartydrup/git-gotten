import { useEffect, useState } from "react";
import { useActivityStore, useRepoStore, useStagingStore } from "../store";
import type { Hunk, Op } from "../types";

/** Classify one hunk body line for rendering. */
function lineClass(line: string): string {
  switch (line[0]) {
    case "+":
      return "add";
    case "-":
      return "del";
    case " ":
      return "ctx";
    default:
      return "meta"; // `\ No newline at end of file` etc.
  }
}

function HunkBlock({
  hunk,
  index,
  checked,
  onToggle,
}: {
  hunk: Hunk;
  index: number;
  checked: boolean;
  onToggle: (index: number) => void;
}) {
  return (
    <div className="hunk">
      <div className={`hunk-head ${hunk.staged ? "staged" : ""}`}>
        {hunk.staged ? (
          <span className="badge staged">staged</span>
        ) : (
          <input
            type="checkbox"
            checked={checked}
            onChange={() => onToggle(index)}
            aria-label={`Stage hunk at ${hunk.header}`}
          />
        )}
        <code>{hunk.header}</code>
      </div>
      {hunk.lines.map((line, i) => (
        <div key={i} className={`diff-line ${lineClass(line)}`}>
          {line || " "}
        </div>
      ))}
    </div>
  );
}

/**
 * The per-file diff of the Files tab (SPEC §10.2): the working-tree diff with
 * a checkbox per unstaged hunk. Staging the chosen hunks, the whole file, or
 * the whole tree — and the `Dangerous` discard — all compile to the core's
 * `CommandPlan`s and run through the preview/confirm flow.
 */
export function FileDiff() {
  const { activeId } = useRepoStore();
  const { file, diff, loading, error } = useStagingStore();
  const { requestPreview, running } = useActivityStore();

  // The ticked (unstaged) hunk indices; re-derived whenever the file or its
  // diff changes (a file can appear in both the staged and unstaged groups).
  const [ticked, setTicked] = useState<ReadonlySet<number>>(new Set());
  useEffect(() => {
    setTicked(new Set());
  }, [file, diff]);

  if (!file) {
    return (
      <section className="file-diff">
        <p className="muted">Select a file to see its diff.</p>
      </section>
    );
  }
  if (loading) {
    return (
      <section className="file-diff">
        <p className="muted">Loading diff…</p>
      </section>
    );
  }
  if (error) {
    return (
      <section className="file-diff">
        <p className="error">{error}</p>
      </section>
    );
  }
  if (!diff) {
    return (
      <section className="file-diff">
        <p className="muted">No diff available.</p>
      </section>
    );
  }

  const stagedCount = diff.hunks.filter((h) => h.staged).length;
  const unstagedCount = diff.hunks.length - stagedCount;

  const toggle = (index: number) =>
    setTicked((prev) => {
      const next = new Set(prev);
      if (next.has(index)) {
        next.delete(index);
      } else {
        next.add(index);
      }
      return next;
    });

  const selected = [...ticked];
  const stageHunksOp: Op | null =
    unstagedCount > 0 && selected.length > 0
      ? { kind: "stage_hunks", path: file, selected }
      : null;
  const stageFileOp: Op | null = { kind: "stage", path: file };
  const stageAllOp: Op | null = { kind: "stage", path: null };
  const unstageFileOp: Op | null =
    stagedCount > 0 || unstagedCount === 0 ? { kind: "unstage", path: file } : null;
  const discardOp: Op | null = { kind: "discard", path: file };

  const run = (op: Op) => void requestPreview(activeId ?? "", op);

  return (
    <section className="file-diff">
      <div className="file-diff-head">
        <code className="file-diff-path">{file}</code>
        {diff.binary && <span className="badge binary">binary</span>}
      </div>

      {diff.binary ? (
        <p className="muted">Binary files have no text hunks.</p>
      ) : diff.hunks.length === 0 ? (
        <p className="muted">No differences.</p>
      ) : (
        diff.hunks.map((hunk, i) => (
          <HunkBlock
            key={i}
            hunk={hunk}
            index={i}
            checked={ticked.has(i)}
            onToggle={toggle}
          />
        ))
      )}

      <div className="file-diff-actions">
        <button
          type="button"
          className="btn"
          disabled={!stageHunksOp || running}
          onClick={() => stageHunksOp && run(stageHunksOp)}
        >
          Stage {selected.length > 0 ? `${selected.length} selected` : "hunks"}
        </button>
        <button
          type="button"
          className="btn"
          disabled={running}
          onClick={() => run(stageFileOp)}
        >
          Stage file
        </button>
        <button type="button" className="btn" disabled={running} onClick={() => run(stageAllOp)}>
          Stage all
        </button>
        {unstageFileOp && (
          <button
            type="button"
            className="btn"
            disabled={running}
            onClick={() => run(unstageFileOp)}
          >
            Unstage file
          </button>
        )}
        <button
          type="button"
          className="btn danger"
          disabled={running}
          onClick={() => run(discardOp)}
        >
          Discard
        </button>
      </div>
    </section>
  );
}
