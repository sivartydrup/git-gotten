import { useBuildInfo, useGraphStore, useRepoStore } from "../store";

/**
 * Top bar (SPEC §7): product brand + build identity, and — when a repository is
 * active — its name, branch (or detached OID), ahead/behind, change count, and
 * in-merge/in-rebase state badges.
 */
export function TopBar() {
  const { info } = useBuildInfo();
  const { repos, status, activeId } = useRepoStore();
  const { query, setQuery } = useGraphStore();

  const meta = repos.find((r) => r.id === activeId) ?? null;
  const activeStatus = activeId ? status[activeId] ?? null : null;

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
          </>
        )}
      </div>
    </header>
  );
}
