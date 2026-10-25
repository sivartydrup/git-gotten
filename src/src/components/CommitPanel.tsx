import { useEffect, useState } from "react";
import { planOp } from "../api";
import { useActivityStore, useRepoStore } from "../store";
import type { Author, Op, RepoId } from "../types";

/**
 * Inline command hint (SPEC §9 "inline hints"): the exact git command an action
 * will run, compiled by the Rust core via `plan_op` — the UI never invents
 * command strings. Re-plans (debounced) as the action's inputs change.
 */
export function usePlanHint(repo: RepoId | null, op: Op | null) {
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

export function Hint({ display, error }: { display: string | null; error: string | null }) {
  if (error) {
    return <code className="cmd-hint error">{error}</code>;
  }
  if (!display) {
    return null;
  }
  return <code className="cmd-hint">{display}</code>;
}

/** Conventional-commit types for the message editor's quick prefixes. */
const COMMIT_TYPES = ["fix", "feat", "docs", "refactor", "perf", "test", "chore"] as const;

/** Apply (or replace) a `type: ` prefix on the message. */
function withType(message: string, type: string): string {
  const stripped = message.replace(/^[a-z]+\s*:\s*/, "");
  return `${type}: ${stripped}`;
}

/**
 * The docked commit panel (SPEC §10.2): the message editor (with a
 * conventional-commit hint), an optional author override + co-authors, and the
 * amend toggle. The commit, switch, and push actions from the Stage 2 action
 * set live here too — each runs through the preview/confirm flow, never git
 * directly, and carries a live command hint.
 */
export function CommitPanel() {
  const { activeId, status } = useRepoStore();
  const { requestPreview, running } = useActivityStore();

  const activeStatus = activeId ? (status[activeId] ?? null) : null;
  const hasStaged = activeStatus?.changes.some((c) => c.staged) ?? false;

  // ── commit ────────────────────────────────────────────────────────────────
  const [message, setMessage] = useState("");
  const [amend, setAmend] = useState(false);
  const [editAuthor, setEditAuthor] = useState(false);
  const [authorName, setAuthorName] = useState("");
  const [authorEmail, setAuthorEmail] = useState("");
  const [coAuthors, setCoAuthors] = useState<string[]>([]);
  const [coAuthorDraft, setCoAuthorDraft] = useState("");

  const author: Author | null =
    editAuthor && (authorName.trim() || authorEmail.trim())
      ? { name: authorName.trim(), email: authorEmail.trim() }
      : null;

  const commitOp: Op | null = message.trim()
    ? {
        kind: "commit",
        message: message.trim(),
        amend,
        // Omit the optional fields when unset: the core's `#[serde(default)]`
        // fills them, and the default op stays a minimal `{kind,message,amend}`.
        ...(author ? { author } : {}),
        ...(coAuthors.length > 0 ? { co_authors: coAuthors } : {}),
      }
    : null;
  const commitHint = usePlanHint(activeId, commitOp);
  // A plain commit needs something staged; an amend rewrites HEAD even if the
  // index is clean (it just re-message / re-records the current tree state).
  const commitDisabled = !commitOp || running || (!hasStaged && !amend);

  // ── switch + push (the Stage 2 action set) ────────────────────────────────
  const [target, setTarget] = useState("");
  const [force, setForce] = useState(false);
  const checkoutOp: Op | null = target.trim()
    ? { kind: "checkout", target: target.trim(), force: false }
    : null;
  const pushOp: Op | null = activeStatus?.branch
    ? {
        kind: "push",
        remote: "origin",
        branch: activeStatus.branch,
        force,
        // `git push` on an untracked branch needs `-u` to have anywhere to go.
        set_upstream: !activeStatus.upstream,
      }
    : null;
  const switchHint = usePlanHint(activeId, checkoutOp);
  const pushHint = usePlanHint(activeId, pushOp);

  return (
    <section className="commit-panel">
      <div className="commit-editor">
        <textarea
          className="commit-message"
          placeholder="Commit message (e.g. fix: the thing)"
          value={message}
          onChange={(e) => setMessage(e.target.value)}
          rows={2}
        />
        <div className="commit-types">
          {COMMIT_TYPES.map((t) => (
            <button
              key={t}
              type="button"
              className="chip"
              onClick={() => setMessage((m) => withType(m, t))}
            >
              {t}:
            </button>
          ))}
        </div>
        <div className="action-row commit-actions">
          <label className="force-toggle">
            <input
              type="checkbox"
              checked={amend}
              onChange={(e) => setAmend(e.target.checked)}
            />
            amend
          </label>
          <label className="force-toggle">
            <input
              type="checkbox"
              checked={editAuthor}
              onChange={(e) => setEditAuthor(e.target.checked)}
            />
            override author
          </label>
          {editAuthor && (
            <>
              <input
                className="action-input author-name"
                placeholder="Author name"
                value={authorName}
                onChange={(e) => setAuthorName(e.target.value)}
              />
              <input
                className="action-input author-email"
                placeholder="Author email"
                value={authorEmail}
                onChange={(e) => setAuthorEmail(e.target.value)}
              />
            </>
          )}
          <input
            className="action-input co-author"
            placeholder="Co-author: Name <email>"
            value={coAuthorDraft}
            onChange={(e) => setCoAuthorDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                addCoAuthor();
              }
            }}
          />
          <button
            type="button"
            className="btn"
            disabled={!coAuthorDraft.trim()}
            onClick={addCoAuthor}
          >
            Add
          </button>
          <button
            type="button"
            className="btn primary"
            disabled={commitDisabled}
            onClick={() => commitOp && void requestPreview(activeId ?? "", commitOp)}
          >
            {amend ? "Amend" : "Commit"}
          </button>
          <Hint display={commitHint.display} error={commitHint.hintError} />
        </div>
        {coAuthors.length > 0 && (
          <div className="co-authors">
            {coAuthors.map((c, i) => (
              <span key={`${c}:${i}`} className="chip co-author-chip">
                {c}
                <button
                  type="button"
                  className="chip-x"
                  aria-label={`Remove co-author ${c}`}
                  onClick={() =>
                    setCoAuthors((prev) => prev.filter((_, j) => j !== i))
                  }
                >
                  ×
                </button>
              </span>
            ))}
          </div>
        )}
      </div>
      <div className="actions">
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
    </section>
  );

  function addCoAuthor() {
    const name = coAuthorDraft.trim();
    if (!name) {
      return;
    }
    setCoAuthors((prev) => (prev.includes(name) ? prev : [...prev, name]));
    setCoAuthorDraft("");
  }
}
