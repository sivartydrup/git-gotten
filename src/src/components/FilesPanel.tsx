import { useState } from "react";
import {
  useActivityStore,
  useConflictStore,
  useMainTab,
  useRepoStore,
  useStagingStore,
  useSubmodulesStore,
} from "../store";
import type { ChangeKind, FileChange, Op } from "../types";

/** Git-status-style short labels for each change kind. */
const KIND_LABEL: Record<ChangeKind, string> = {
  Modified: "M",
  Added: "A",
  Deleted: "D",
  Renamed: "R",
  Untracked: "??",
  Conflicted: "C",
};

function FileRow({
  change,
  selected,
  onSelect,
}: {
  change: FileChange;
  selected: boolean;
  onSelect: (path: string) => void;
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
 * for the diff view.
 */
export function FilesPanel() {
  const { activeId, status, repos } = useRepoStore();
  const { file, selectFile } = useStagingStore();

  const meta = repos.find((r) => r.id === activeId) ?? null;
  const activeStatus = activeId ? (status[activeId] ?? null) : null;

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

  const { changes } = activeStatus;
  const conflicted = changes.filter((c) => c.kind === "Conflicted");
  const staged = changes.filter((c) => c.staged && c.kind !== "Conflicted");
  const untracked = changes.filter((c) => !c.staged && c.kind === "Untracked");
  const modified = changes.filter(
    (c) => !c.staged && c.kind !== "Untracked" && c.kind !== "Conflicted"
  );

  const conflictFile = useConflictStore((s) => s.file);
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
