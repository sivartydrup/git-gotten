import { useBuildInfo, useRepoStore } from "../store";
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

function ChangeRow({ change }: { change: FileChange }) {
  return (
    <li className="change">
      <span className={`badge kind kind-${change.kind.toLowerCase()}`}>
        {KIND_LABEL[change.kind]}
      </span>
      {change.staged && <span className="badge staged">staged</span>}
      <span className="change-path">{change.path}</span>
      {change.binary && <span className="badge binary">binary</span>}
    </li>
  );
}

/**
 * Content area: the active repository's live status as a changed-files list
 * (the diff viewer and lane graph land in later stages).
 */
export function StatusView() {
  const { info, error } = useBuildInfo();
  const { repos, status, activeId } = useRepoStore();

  const meta = repos.find((r) => r.id === activeId) ?? null;
  const activeStatus = activeId ? status[activeId] ?? null : null;

  if (!meta) {
    return (
      <section className="content">
        {error ? (
          <p className="error">{error}</p>
        ) : info ? (
          <p>Open a repository from the left to see its live status.</p>
        ) : (
          <p>
            Running outside Tauri (plain web preview). Start with{" "}
            <code>pnpm tauri dev</code> to open a real repository.
          </p>
        )}
      </section>
    );
  }

  if (!activeStatus) {
    return (
      <section className="content">
        <p>Reading status…</p>
      </section>
    );
  }

  const { changes } = activeStatus;

  return (
    <section className="content status-view">
      <p className="status-summary">
        {changes.length === 0 ? (
          <span className="clean">Working tree clean — no changes.</span>
        ) : (
          `${changes.length} changed file${changes.length === 1 ? "" : "s"}`
        )}
      </p>
      {changes.length > 0 && (
        <ul className="changes">
          {changes.map((change) => (
            <ChangeRow key={`${change.path}:${change.kind}:${change.staged ? 1 : 0}`} change={change} />
          ))}
        </ul>
      )}
    </section>
  );
}
