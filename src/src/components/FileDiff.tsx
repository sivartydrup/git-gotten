import { useEffect, useState } from "react";
import { useActivityStore, useRepoStore, useStagingStore } from "../store";
import type { Hunk, Op } from "../types";
import { Icon } from "./Icons";
import "./diff-activity.css";

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

/** Word runs of a line: whitespace, if any, is its own single-column run. */
function runs(line: string): string[] {
  return line.match(/\S+|\s/g) ?? [];
}

const isWord = (run: string) => !/^\s$/.test(run);

/**
 * The differing runs of an added line against the removed line it replaces.
 *
 * The words the two lines share are trimmed off the head and off the tail; the
 * runs between those two points are the edit, kept verbatim (including the
 * whitespace between them) so `white-space: pre` alignment is untouched and the
 * indentation before the edit is never inside a mark. A pure insertion (no
 * partner line) marks the whole line.
 *
 * Runs are compared, not characters, and each line is walked a constant number
 * of times: linear for a 500-line file, with nothing recomputed per frame or per
 * keystroke.
 */
function wordRuns(removed: string[] | null, added: string[]): { text: string; changed: boolean }[] {
  if (removed === null) {
    return [{ text: added.join(""), changed: true }];
  }
  const a = removed.filter(isWord);
  const b = added.filter(isWord);
  let head = 0;
  while (head < a.length && head < b.length && a[head] === b[head]) {
    head += 1;
  }
  let tailA = a.length - 1;
  let tailB = b.length - 1;
  while (tailB >= head && tailA >= head && a[tailA] === b[tailB]) {
    tailA -= 1;
    tailB -= 1;
  }
  // Map the trimmed word range back onto the run list of the added line.
  const from = runsIndexOf(added, head);
  const to = runsIndexOf(added, tailB);
  return added.map((text, i) => ({ text, changed: tailB >= head && from >= 0 && i >= from && i <= to }));
}

/** Index of the `wordIndex`-th word run in a run list, or -1 when there is none. */
function runsIndexOf(runsList: string[], wordIndex: number): number {
  if (wordIndex < 0) {
    return -1;
  }
  let seen = -1;
  for (let i = 0; i < runsList.length; i += 1) {
    if (!isWord(runsList[i])) {
      continue;
    }
    seen += 1;
    if (seen === wordIndex) {
      return i;
    }
  }
  return -1;
}

interface DiffRow {
  kind: string;
  text: string;
  /** Word runs for an added line; `null` on every other kind. */
  words: { text: string; changed: boolean }[] | null;
}

/**
 * Turn a hunk body into renderable rows. Each `+` line is lined up with the `-`
 * line of its replacement block so the runs can be compared; a 1:1 swap gets
 * word-level detail, anything lopsided marks the whole line.
 */
function diffRows(lines: string[]): DiffRow[] {
  const rows: DiffRow[] = [];
  for (let i = 0; i < lines.length; i += 1) {
    const line = lines[i];
    const kind = lineClass(line);
    const body = kind === "ctx" || kind === "meta" ? line : line.slice(1);
    if (kind !== "add") {
      rows.push({ kind, text: body, words: null });
      continue;
    }
    let removedStart = i;
    while (removedStart > 0 && lineClass(lines[removedStart - 1]) === "del") {
      removedStart -= 1;
    }
    let addedEnd = i;
    while (addedEnd < lines.length && lineClass(lines[addedEnd]) === "add") {
      addedEnd += 1;
    }
    // A one-into-one swap has a partner to compare runs with; a block that grows
    // or shrinks is a pure insertion for this line.
    const partner =
      addedEnd - i === 1 && i - removedStart === 1 ? runs(lines[removedStart].slice(1)) : null;
    rows.push({ kind, text: body, words: wordRuns(partner, runs(body)) });
  }
  return rows;
}

/** Added / removed line counts of hunks we already have (no extra IPC). */
export function hunkStats(hunks: Hunk[]): { added: number; removed: number } {
  let added = 0;
  let removed = 0;
  for (const hunk of hunks) {
    for (const line of hunk.lines) {
      const kind = lineClass(line);
      if (kind === "add") {
        added += 1;
      } else if (kind === "del") {
        removed += 1;
      }
    }
  }
  return { added, removed };
}

/**
 * The shared diffstat: a compact `+N −N` label plus a five-segment bar filled
 * left to right by the added share of the change.
 */
export function Diffstat({ added, removed }: { added: number; removed: number }) {
  const total = added + removed;
  // Segments owned by each side; at least one each whenever there is a change.
  const adds = total === 0 ? 0 : Math.max(1, Math.round((added / total) * 5));
  const dels = total === 0 ? 0 : Math.max(1, 5 - adds);
  return (
    <span
      className="diffstat"
      title={`${added} added, ${removed} removed`}
      aria-label={`${added} added, ${removed} removed`}
    >
      <span className="diffstat-count">
        <span className="add">+{added}</span> <span className="del">−{removed}</span>
      </span>
      <span className="diffstat-bar" aria-hidden>
        {Array.from({ length: 5 }, (_, i) => (
          <span
            key={i}
            className={`diffstat-seg ${i < adds ? "add" : i < adds + dels ? "del" : ""}`}
          />
        ))}
      </span>
    </span>
  );
}

/** Copy button whose label reads "Copied" for 1.2 s; no clipboard API means a no-op. */
function CopyButton({ text, label }: { text: string; label: string }) {
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) {
      return undefined;
    }
    const timer = setTimeout(() => setCopied(false), 1200);
    return () => clearTimeout(timer);
  }, [copied]);

  return (
    <button
      type="button"
      className={`copy-btn ${copied ? "copied" : ""}`}
      onClick={() => {
        if (!navigator.clipboard?.writeText) {
          return; // no clipboard in this context: leave everything as it is
        }
        void Promise.resolve(navigator.clipboard.writeText(text))
          .then(() => setCopied(true))
          .catch(() => {});
      }}
    >
      <Icon name="copy" size={14} />
      <span className="btn-label">{copied ? "Copied" : label}</span>
    </button>
  );
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
      {diffRows(hunk.lines).map((row, i) => (
        <div key={i} className={`diff-line ${row.kind}`}>
          {row.words
            ? row.words.map((run, w) => (
                <span
                  key={w}
                  className={run.changed ? (row.kind === "add" ? "wadd" : "wdel") : undefined}
                >
                  {run.text}
                </span>
              ))
            : row.text || " "}
        </div>
      ))}
    </div>
  );
}

/**
 * The per-file diff of the Files tab (SPEC §10.2): the working-tree diff with
 * a checkbox per unstaged hunk. Staging the chosen hunks, the whole file, or
 * the whole tree — and the `Dangerous` discard — all compile to the core's
 * `CommandPlan`s and run through the preview/confirm flow. The header carries
 * the file's diffstat per staging side, plus the copy affordances.
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

  const stagedHunks = diff.hunks.filter((h) => h.staged);
  const unstagedHunks = diff.hunks.filter((h) => !h.staged);
  const stagedCount = stagedHunks.length;
  const unstagedCount = unstagedHunks.length;

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
        {stagedCount > 0 && <Diffstat {...hunkStats(stagedHunks)} />}
        {unstagedCount > 0 && <Diffstat {...hunkStats(unstagedHunks)} />}
        <CopyButton text={file} label="Copy path" />
        <CopyButton text={diff.patch} label="Copy patch" />
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
