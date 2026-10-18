import { useEffect, useState } from "react";
import { planOp } from "../api";
import { useActivityStore, useBuildInfo, useRepoStore } from "../store";
import type { ChangeKind, FileChange, Op, RepoId, Status } from "../types";

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
 * Inline command hint (SPEC §9 "inline hints"): the exact git command an action
 * will run, compiled by the Rust core via `plan_op` — the UI never invents
 * command strings. Re-plans (debounced) as the action's inputs change.
 */
function usePlanHint(repo: RepoId | null, op: Op | null) {
  const [display, setDisplay] = useState<string | null>(null);
  const [hintError, setHintError] = useState<string | null>(null);
  const key = op ? JSON.stringify(op) : null;

  useEffect(() => {
    if (!repo || !op) {
      setDisplay(null);
      setHintError(null);
      return;
    }
    const timer = setTimeout(() => {
      void planOp(repo, op)
        .then((plan) => {
          setDisplay(plan ? plan.display : null);
          setHintError(null);
        })
        .catch((e) => setHintError(e instanceof Error ? e.message : String(e)));
    }, 300);
    return () => clearTimeout(timer);
    // `op` is tracked by `key`: the object is rebuilt on every render.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [repo, key]);

  return { display, hintError };
}

function Hint({ display, error }: { display: string | null; error: string | null }) {
  if (error) {
    return <code className="cmd-hint error">{error}</code>;
  }
  if (!display) {
    return null;
  }
  return <code className="cmd-hint">{display}</code>;
}

/**
 * The Stage 2 action set: commit, switch (checkout), and push (with force).
 * Each carries a live CmdHint (the Rust-compiled command) and runs through
 * the preview/confirm flow — never git directly.
 */
function ActionsRow({ status }: { status: Status }) {
  const { activeId } = useRepoStore();
  const { requestPreview, running } = useActivityStore();
  const [message, setMessage] = useState("");
  const [target, setTarget] = useState("");
  const [force, setForce] = useState(false);

  const commitOp: Op | null = message.trim()
    ? { kind: "commit", message: message.trim(), amend: false }
    : null;
  const checkoutOp: Op | null = target.trim()
    ? { kind: "checkout", target: target.trim(), force: false }
    : null;
  const pushOp: Op | null = status.branch
    ? { kind: "push", remote: "origin", branch: status.branch, force }
    : null;

  const commitHint = usePlanHint(activeId, commitOp);
  const switchHint = usePlanHint(activeId, checkoutOp);
  const pushHint = usePlanHint(activeId, pushOp);

  const hasStaged = status.changes.some((c) => c.staged);

  return (
    <div className="actions">
      <div className="action-row">
        <input
          className="action-input"
          placeholder="Commit message"
          value={message}
          onChange={(e) => setMessage(e.target.value)}
        />
        <button
          type="button"
          className="btn primary"
          disabled={!hasStaged || !commitOp || running}
          onClick={() => commitOp && void requestPreview(activeId ?? "", commitOp)}
        >
          Commit
        </button>
        <Hint display={commitHint.display} error={commitHint.hintError} />
      </div>
      <div className="action-row">
        <input
          className="action-input"
          placeholder="Switch to branch…"
          value={target}
          onChange={(e) => setTarget(e.target.value)}
        />
        <button
          type="button"
          className="btn"
          disabled={!checkoutOp || running}
          onClick={() => checkoutOp && void requestPreview(activeId ?? "", checkoutOp)}
        >
          Switch
        </button>
        <Hint display={switchHint.display} error={switchHint.hintError} />
      </div>
      <div className="action-row">
        <label className="force-toggle">
          <input
            type="checkbox"
            checked={force}
            onChange={(e) => setForce(e.target.checked)}
          />
          force
        </label>
        <button
          type="button"
          className={`btn ${force ? "danger" : ""}`}
          disabled={!pushOp || running}
          onClick={() => pushOp && void requestPreview(activeId ?? "", pushOp)}
        >
          {force ? "Force push" : "Push"}
        </button>
        <Hint display={pushHint.display} error={pushHint.hintError} />
      </div>
    </div>
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
      <ActionsRow status={activeStatus} />
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
