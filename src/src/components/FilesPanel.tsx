import { useEffect, useState } from "react";
import {
  useActivityStore,
  useConflictStore,
  useMainTab,
  useRepoStore,
  useStagingStore,
  useSubmodulesStore,
} from "../store";
import { loadDiff } from "../api";
import type { ChangeKind, FileChange, Op } from "../types";
import { Diffstat, hunkStats } from "./FileDiff";
import "./diff-activity.css";

/** Git-status-style short labels for each change kind. */
const KIND_LABEL: Record<ChangeKind, string> = {
  Modified: "M",
  Added: "A",
  Deleted: "D",
  Renamed: "R",
  Untracked: "??",
  Conflicted: "C",
};

/** Per-path add/remove counts for both staging sides, straight from `loadDiff`. */
interface PathStat {
  staged: { added: number; removed: number } | null;
  unstaged: { added: number; removed: number } | null;
}

/**
 * Per-file diffstats for the panel, read from the hunks Rust already delivers
 * for each path (one call per path covers both the staged and the unstaged row).
 *
 * shortcut: `loadDiff` once per path, batched, 60 ms after the status settles,
 * so a burst of `status_changed` events does not fan out per event. Upgrade to
 * a bulk per-repo stats command if a repo ever shows hundreds of files at once.
 */
function useFileStats(repo: string, changes: FileChange[]): Record<string, PathStat> {
  const [stats, setStats] = useState<Record<string, PathStat>>({});
  // Only re-read when the set of paths actually changes (a status refresh that
  // changes nothing else must not re-fetch).
  const paths = changes.map((c) => c.path).join("\n");

  useEffect(() => {
    // Reset synchronously so a render before the debounce still sees the
    // previous repo's counts rather than dropping the rows' diffstats.
    if (!repo || paths.length === 0) {
      setStats({});
      return undefined;
    }
    let live = true;
    const timer = setTimeout(() => {
      void Promise.all(
        [...new Set(paths.split("\n"))].map((path) => loadDiff(repo, path))
      ).then((diffs) => {
        if (!live) {
          return; // the panel moved on to another repo or another file set
        }
        const next: Record<string, PathStat> = {};
        diffs.forEach((diff, i) => {
          const path = [...new Set(paths.split("\n"))][i];
          if (!diff) {
            return;
          }
          next[path] = {
            staged: hunkStats(diff.hunks.filter((h) => h.staged)),
            unstaged: hunkStats(diff.hunks.filter((h) => !h.staged)),
          };
        });
        setStats(next);
      });
    }, 60);
    return () => {
      live = false;
      clearTimeout(timer);
    };
  }, [repo, paths]);

  return stats;
}

function FileRow({
  change,
  selected,
  onSelect,
  stats,
}: {
  change: FileChange;
  selected: boolean;
  onSelect: (path: string) => void;
  stats: { added: number; removed: number } | null;
}) {
  return (
    <li
      className={`change file ${selected ? "selected" : ""}`}
      onClick={() => onSelect(change.path)}
    >
      <span className={`badge kind kind-${change.kind.toLowerCase()}`}>
        {KIND_LABEL[change.kind]}
      </span>
      <span className="change-path">{change.path}</span>
      {change.binary && <span className="badge binary">binary</span>}
      {change.submodule && <span className="badge submodule">submodule</span>}
      {stats && <Diffstat added={stats.added} removed={stats.removed} />}
    </li>
  );
}

function Group({
  title,
  count,
  children,
}: {
  title: string;
  count: number;
  children: React.ReactNode;
}) {
  return (
    <div className="files-group">
      <h3>
        {title} <span className="count">({count})</span>
      </h3>
      <ul className="changes">{children}</ul>
    </div>
  );
}

/**
 * The submodule section of the Files panel (SPEC §10.8): name, checkout path,
 * url, `not initialized` / `pointer moved` badges, plus init and update (an
 * optional pinned commit straight from the section's input). Renders nothing
 * when the repo has no submodules.
 */
function SubmodulesSection() {
  const { submodules, loading, error } = useSubmodulesStore();
  const activeId = useRepoStore((s) => s.activeId);
  const { requestPreview, running } = useActivityStore();
  const [commit, setCommit] = useState("");

  if (submodules.length === 0 && !loading && !error) {
    return null;
  }

  const act = (op: Op) => {
    if (activeId) {
      void requestPreview(activeId, op);
    }
  };

  return (
    <div className="files-group submodules">
      <div className="refs-head">
        <h3>
          Submodules <span className="count">({submodules.length})</span>
        </h3>
        <input
          className="dialog-input submodule-commit"
          placeholder="check out a specific commit (optional)"
          value={commit}
          onChange={(e) => setCommit(e.target.value)}
        />
      </div>
      {error && <p className="muted">Failed to load submodules: {error}</p>}
      <ul className="refs-list">
        {submodules.map((m) => (
          <li key={m.path} className="refs-row">
            <span className="refs-name">{m.name}</span>
            <code className="refs-path" title={m.url}>
              {m.path}
            </code>
            {!m.initialized && <span className="badge">not initialized</span>}
            {m.differs && <span className="badge">pointer moved</span>}
            <span className="row-actions">
              {!m.initialized && (
                <button
                  type="button"
                  className="btn row-btn"
                  disabled={running}
                  onClick={() => act({ kind: "submodule_init", path: m.path })}
                >
                  Init
                </button>
              )}
              <button
                type="button"
                className="btn row-btn"
                disabled={running}
                onClick={() =>
                  act({
                    kind: "submodule_update",
                    path: m.path,
                    commit: commit.trim() || null,
                  })
                }
              >
                Update
              </button>
            </span>
          </li>
        ))}
      </ul>
    </div>
  );
}

/**
 * The changed-files list of the Files tab (SPEC §10.2): the active repo's
 * status grouped into Staged / Changes / Untracked. Clicking a file selects it
 * for the diff view. Every row carries its own diffstat, derived from the
 * hunks Rust delivers for that path and staging side.
 */
export function FilesPanel() {
  const { activeId, status, repos } = useRepoStore();
  const { file, selectFile } = useStagingStore();
  const conflictFile = useConflictStore((s) => s.file);

  const meta = repos.find((r) => r.id === activeId) ?? null;
  const activeStatus = activeId ? (status[activeId] ?? null) : null;
  const changes = activeStatus?.changes ?? [];
  const stats = useFileStats(activeId ?? "", changes);

  if (!meta) {
    return (
      <aside className="files-panel">
        <p className="muted">Open a repository to see its changes.</p>
      </aside>
    );
  }
  if (!activeStatus) {
    return (
      <aside className="files-panel">
        <p className="muted">Reading status…</p>
      </aside>
    );
  }

  const conflicted = changes.filter((c) => c.kind === "Conflicted");
  const staged = changes.filter((c) => c.staged && c.kind !== "Conflicted");
  const untracked = changes.filter((c) => !c.staged && c.kind === "Untracked");
  const modified = changes.filter(
    (c) => !c.staged && c.kind !== "Untracked" && c.kind !== "Conflicted"
  );

  const statOf = (c: FileChange) => {
    const entry = stats[c.path];
    if (!entry) {
      return null;
    }
    return c.staged ? entry.staged : entry.unstaged;
  };

  const openConflict = (path: string) => {
    useMainTab.getState().setTab("conflicts");
    useConflictStore.getState().selectFile(path);
  };

  const renderConflictedGroup = conflicted.length > 0 ? (
    <Group title="Conflicted" count={conflicted.length}>
      {conflicted.map((c) => (
        <FileRow
          key={c.path}
          change={c}
          selected={conflictFile === c.path}
          onSelect={openConflict}
          stats={statOf(c)}
        />
      ))}
    </Group>
  ) : null;

  const renderGroup = (title: string, group: FileChange[]) =>
    group.length > 0 ? (
      <Group title={title} count={group.length}>
        {group.map((c) => (
          <FileRow
            key={`${c.path}:${c.kind}:${c.staged ? 1 : 0}`}
            change={c}
            selected={file === c.path}
            onSelect={(path) => selectFile(file === path ? null : path)}
            stats={statOf(c)}
          />
        ))}
      </Group>
    ) : null;

  return (
    <aside className="files-panel">
      <p className="status-summary">
        {changes.length === 0 ? (
          <span className="clean">Working tree clean — no changes.</span>
        ) : (
          `${changes.length} changed file${changes.length === 1 ? "" : "s"}`
        )}
      </p>
      {renderConflictedGroup}
      {renderGroup("Staged", staged)}
      {renderGroup("Changes", modified)}
      {renderGroup("Untracked", untracked)}
      <SubmodulesSection />
    </aside>
  );
}
