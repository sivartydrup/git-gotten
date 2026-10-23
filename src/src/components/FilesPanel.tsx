import { useRepoStore, useStagingStore } from "../store";
import type { ChangeKind, FileChange } from "../types";

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
  const staged = changes.filter((c) => c.staged);
  const untracked = changes.filter((c) => !c.staged && c.kind === "Untracked");
  const modified = changes.filter((c) => !c.staged && c.kind !== "Untracked");

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
      {renderGroup("Staged", staged)}
      {renderGroup("Changes", modified)}
      {renderGroup("Untracked", untracked)}
    </aside>
  );
}
