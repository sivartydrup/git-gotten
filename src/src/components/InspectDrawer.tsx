import type { ReactNode } from "react";
import { useActivityStore, useGraphStore, useRepoStore } from "../store";
import type { Op, RefLabel } from "../types";

/** Format unix seconds as a readable local timestamp. */
function fmt(unixSeconds: number): string {
  return new Date(unixSeconds * 1000).toLocaleString();
}

/** A ref badge (colored dot + name) for the commit's refs. */
function RefBadges({ refs }: { refs: RefLabel[] }) {
  if (refs.length === 0) {
    return null;
  }
  return (
    <div className="drawer-refs">
      {refs.map((r) => (
        <span key={r.name} className="badge kind kind-branch">
          {r.name}
        </span>
      ))}
    </div>
  );
}

/**
 * The inspect drawer (SPEC §10.1): the right-hand panel with the full details
 * of the selected commit — message, authors, dates, refs, stat table, and the
 * per-file diffs (scrollable monospace). Actions: switch-to and reset-to.
 *
 * Merge commits carry no diff of their own; the drawer says so.
 */
export function InspectDrawer() {
  const activeId = useRepoStore((s) => s.activeId);
  const { detail, detailLoading, detailError, selected, closeDetail } = useGraphStore();
  const { requestPreview, running } = useActivityStore();

  if (!selected) {
    return (
      <aside className="drawer">
        <div className="drawer-state">Select a commit to inspect it</div>
      </aside>
    );
  }

  const run = (op: Op) => {
    if (activeId && detail) {
      void requestPreview(activeId, op);
    }
  };

  let body: ReactNode;
  if (detailError) {
    body = (
      <div className="drawer-state error">
        <div>Failed to load the commit</div>
        <code>{detailError}</code>
      </div>
    );
  } else if (detailLoading || !detail) {
    body = <div className="drawer-state">Loading…</div>;
  } else {
    const noDiff = detail.stat.length === 0 && detail.diff.length === 0;
    body = (
      <>
        <header className="drawer-head">
          <code className="drawer-hash" title={detail.hash}>
            {detail.hash.slice(0, 12)}
          </code>
          <button type="button" className="btn" onClick={closeDetail}>
            Close
          </button>
        </header>
        <h2 className="drawer-subject">{detail.subject}</h2>
        <RefBadges refs={detail.refs} />
        {detail.is_head && <span className="badge state">HEAD</span>}
        {detail.message.trim() !== detail.subject.trim() && (
          <pre className="drawer-message">{detail.message}</pre>
        )}
        <dl className="drawer-dates">
          <div>
            <dt>
              {detail.author_name} &lt;{detail.author_email}&gt;
            </dt>
            <dd>{fmt(detail.author_date)}</dd>
          </div>
          {detail.committer_name !== detail.author_name ||
          detail.committer_email !== detail.author_email ? (
            <div>
              <dt>
                {detail.committer_name} &lt;{detail.committer_email}&gt;
              </dt>
              <dd>{fmt(detail.committer_date)}</dd>
            </div>
          ) : null}
        </dl>
        {detail.parents.length > 0 && (
          <div className="drawer-parents">
            {detail.parents.map((p) => (
              <code key={p} title={p}>
                {p.slice(0, 12)}
              </code>
            ))}
          </div>
        )}
        {noDiff ? (
          <div className="drawer-state">No diff for merge commits</div>
        ) : (
          <>
            <table className="drawer-stat">
              <tbody>
                {detail.stat.map((f) => (
                  <tr key={f.path}>
                    <td className="stat-path">{f.path}</td>
                    <td className="stat-add">+{f.additions}</td>
                    <td className="stat-del">−{f.deletions}</td>
                    {f.binary && <td className="stat-bin">bin</td>}
                  </tr>
                ))}
              </tbody>
            </table>
            {detail.diff.map((f) => (
              <div key={f.path} className="drawer-diff">
                <div className="drawer-diff-head">{f.path}</div>
                {f.binary ? (
                  <div className="drawer-state">Binary file</div>
                ) : (
                  <pre className="drawer-diff-body">{f.patch}</pre>
                )}
              </div>
            ))}
          </>
        )}
        <div className="drawer-actions">
          <button
            type="button"
            className="btn"
            disabled={running}
            onClick={() => run({ kind: "checkout", target: detail.hash, force: false })}
          >
            Switch to this commit
          </button>
          <button
            type="button"
            className="btn"
            disabled={running}
            onClick={() => run({ kind: "reset", mode: "mixed", target: detail.hash })}
          >
            Reset branch (mixed)
          </button>
        </div>
      </>
    );
  }

  return <aside className="drawer">{body}</aside>;
}
