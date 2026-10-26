import { useState } from "react";
import {
  useActivityStore,
  useBuildInfo,
  useGraphStore,
  useRefsStore,
  useRepoStore,
} from "../store";
import type { Op } from "../types";

/**
 * Top bar (SPEC §7): product brand + build identity, and — when a repository
 * is active — its name, branch (or detached OID), ahead/behind, change count,
 * and in-merge/in-rebase state badges.
 *
 * Stage 7 adds the everyday-sync control set (SPEC §10.4): Fetch / Pull /
 * Pull (rebase) / Push with ahead/behind-aware labels (disabled when the
 * branch has no upstream), and a Merge action whose picker lists the local
 * branches. Every control compiles its op through the transparent preview
 * flow, so the user sees the exact command before anything runs.
 */
export function TopBar() {
  const { info } = useBuildInfo();
  const { repos, status, activeId } = useRepoStore();
  const { query, setQuery } = useGraphStore();
  const { branches, remotes, loading: refsLoading, load: loadRefs } = useRefsStore();
  const { busy, running } = useActivityStore();
  const [mergeOpen, setMergeOpen] = useState(false);

  const meta = repos.find((r) => r.id === activeId) ?? null;
  const activeStatus = activeId ? status[activeId] ?? null : null;
  const disabled = busy || running;

  // The current branch's upstream (`origin/main`) pins the sync targets:
  // `origin` is the remote, `main` the branch.
  const upstream = activeStatus?.upstream ?? null;
  const upstreamRemote = upstream?.split("/")[0] ?? null;
  // `git fetch` works without a tracking branch: it targets a configured
  // remote (the upstream's, else the first listed one).
  const fetchRemote = upstreamRemote ?? remotes[0]?.name ?? null;
  const hasUpstream = upstream !== null;

  const request = (op: Op) => {
    if (!activeId || disabled) {
      return;
    }
    setMergeOpen(false);
    void useActivityStore.getState().requestPreview(activeId, op);
  };

  return (
    <header className="topbar">
      <h1 className="brand">GitGotten</h1>
      {info && (
        <span className="version">
          {info.name} {info.version} · {info.arch}
        </span>
      )}
      {activeId && (
        <input
          className="topbar-search"
          placeholder="Filter graph: subject, author, branch…"
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
      )}

      <div className="topbar-repo">
        {meta && (
          <>
            <span className="topbar-name">{meta.name}</span>
            {activeStatus ? (
              <>
                <span className="topbar-branch">
                  {activeStatus.branch ?? `detached ${activeStatus.detached_head?.slice(0, 12)}`}
                </span>
                {activeStatus.upstream && (
                  <span className="topbar-sync" title={activeStatus.upstream}>
                    {activeStatus.ahead > 0 && <span className="ahead">↑{activeStatus.ahead}</span>}
                    {activeStatus.behind > 0 && <span className="behind">↓{activeStatus.behind}</span>}
                    {activeStatus.ahead === 0 && activeStatus.behind === 0 && (
                      <span className="synced">✓ synced</span>
                    )}
                  </span>
                )}
                {activeStatus.changes.length > 0 && (
                  <span className="badge dirty">{activeStatus.changes.length} changes</span>
                )}
                {activeStatus.in_merge && <span className="badge state">MERGE</span>}
                {activeStatus.in_rebase && <span className="badge state">REBASE</span>}
              </>
            ) : (
              <span className="topbar-branch">{meta.branch ?? "detached"}</span>
            )}

            {activeId && activeStatus && (
              <span className="topbar-actions">
                <span className="sync-controls" title={upstream ?? "no upstream"}>
                  <button
                    type="button"
                    disabled={disabled || !fetchRemote}
                    onClick={() =>
                      request({ kind: "remote_fetch", remote: fetchRemote!, refspec: null })
                    }
                  >
                    Fetch
                  </button>
                  <button
                    type="button"
                    disabled={disabled || !hasUpstream}
                    onClick={() => request({ kind: "pull", rebase: false })}
                  >
                    Pull{activeStatus.behind > 0 ? ` ↓${activeStatus.behind}` : ""}
                  </button>
                  <button
                    type="button"
                    disabled={disabled || !hasUpstream}
                    onClick={() => request({ kind: "pull", rebase: true })}
                  >
                    Pull (rebase)
                    {activeStatus.behind > 0 ? ` ↓${activeStatus.behind}` : ""}
                  </button>
                  <button
                    type="button"
                    disabled={disabled || !hasUpstream}
                    onClick={() =>
                      request({
                        kind: "push",
                        remote: upstreamRemote!,
                        branch: null,
                        force: false,
                        set_upstream: false,
                      })
                    }
                  >
                    Push{activeStatus.ahead > 0 ? ` ↑${activeStatus.ahead}` : ""}
                  </button>
                </span>
                {activeStatus.branch && (
                  <span className="merge-control">
                    <button
                      type="button"
                      disabled={disabled}
                      aria-expanded={mergeOpen}
                      onClick={() => {
                        const next = !mergeOpen;
                        setMergeOpen(next);
                        if (next && branches.length === 0 && !refsLoading) {
                          void loadRefs(activeId);
                        }
                      }}
                    >
                      Merge
                    </button>
                    {mergeOpen && (
                      <div className="merge-picker" role="menu">
                        {branches.filter((b) => !b.is_remote && !b.is_current).length === 0 ? (
                          <span className="merge-picker-empty">No other branches</span>
                        ) : (
                          branches
                            .filter((b) => !b.is_remote && !b.is_current)
                            .map((b) => (
                              <button
                                key={b.name}
                                type="button"
                                role="menuitem"
                                onClick={() => request({ kind: "merge", source: b.name })}
                              >
                                {b.name}
                              </button>
                            ))
                        )}
                      </div>
                    )}
                  </span>
                )}
              </span>
            )}
          </>
        )}
      </div>
    </header>
  );
}
