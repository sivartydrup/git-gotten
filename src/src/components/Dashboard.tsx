import { useMemo, useState } from "react";
import { useRepoStore, useViewStore } from "../store";
import { formatDate, type RepoMeta, type Status } from "../types";

type SortKey = "name" | "branch" | "state" | "sync" | "dirty" | "commit";

interface Row {
  meta: RepoMeta;
  status: Status | undefined;
}

function stateLabel(status: Status | undefined): string {
  if (!status) {
    return "unknown";
  }
  if (status.in_rebase) return "rebasing";
  if (status.in_merge) return "merging";
  if (status.in_cherry_pick) return "cherry-picking";
  if (status.detached_head) return "detached";
  return "clean";
}

/**
 * All-repos dashboard (SPEC §7): a sortable/filterable table of every open
 * repo with live status; clicking a row switches to that repo.
 */
export function Dashboard() {
  const repos = useRepoStore((s) => s.repos);
  const statusMap = useRepoStore((s) => s.status);
  const select = useRepoStore((s) => s.select);
  const setDashboard = useViewStore((s) => s.setDashboard);
  const [filter, setFilter] = useState("");
  const [sort, setSort] = useState<{ key: SortKey; desc: boolean }>({ key: "name", desc: false });

  const rows = useMemo(() => {
    const q = filter.trim().toLowerCase();
    const list: Row[] = repos
      .filter((m) => !q || m.name.toLowerCase().includes(q) || m.path.toLowerCase().includes(q) || (m.branch ?? "").toLowerCase().includes(q))
      .map((m) => ({ meta: m, status: statusMap[m.id] }));
    const cmp = (a: Row, b: Row): number => {
      switch (sort.key) {
        case "branch":
          return (a.meta.branch ?? "\uffff").localeCompare(b.meta.branch ?? "\uffff");
        case "state":
          return stateLabel(a.status).localeCompare(stateLabel(b.status));
        case "sync":
          return a.meta.ahead + a.meta.behind - (b.meta.ahead + b.meta.behind);
        case "dirty":
          return a.meta.dirty_count - b.meta.dirty_count;
        case "commit":
          return (a.status?.last_commit?.author_time ?? 0) - (b.status?.last_commit?.author_time ?? 0);
        default:
          return a.meta.name.localeCompare(b.meta.name);
      }
    };
    return list.sort((a, b) => (sort.desc ? -cmp(a, b) : cmp(a, b)));
  }, [repos, statusMap, filter, sort]);

  const head = (key: SortKey, label: string) => (
    <th
      onClick={() =>
        setSort((s) => (s.key === key ? { key, desc: !s.desc } : { key, desc: key !== "name" }))
      }
    >
      {label}
      {sort.key === key && (sort.desc ? " ▾" : " ▴")}
    </th>
  );

  return (
    <section className="dashboard">
      <header className="dashboard-head">
        <h2>All Repositories</h2>
        <input
          className="open-input dashboard-filter"
          placeholder="Filter repos"
          aria-label="Filter repos"
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
        />
        <button className="icon-btn" aria-label="Close dashboard" onClick={() => setDashboard(false)}>
          ✕
        </button>
      </header>
      {rows.length === 0 ? (
        <p className="nav-hint">No repositories match.</p>
      ) : (
        <table className="dashboard-table">
          <thead>
            <tr>
              {head("name", "Repo")}
              {head("branch", "Branch")}
              {head("state", "State")}
              {head("sync", "Sync")}
              {head("dirty", "Dirty")}
              {head("commit", "Last commit")}
            </tr>
          </thead>
          <tbody>
            {rows.map(({ meta, status }) => (
              <tr
                key={meta.id}
                title={meta.path}
                onClick={() => {
                  select(meta.id);
                  setDashboard(false);
                }}
              >
                <td className="dash-name">{meta.name}</td>
                <td>{meta.branch ?? "detached"}</td>
                <td>
                  <span className={`badge dash-state${stateLabel(status) === "clean" ? "" : " dirty"}`}>
                    {stateLabel(status)}
                  </span>
                </td>
                <td className="mono">
                  {meta.ahead > 0 && <span className="ahead">↑{meta.ahead}</span>}{" "}
                  {meta.behind > 0 && <span className="behind">↓{meta.behind}</span>}
                  {meta.ahead === 0 && meta.behind === 0 && "—"}
                </td>
                <td className="mono">{meta.dirty_count > 0 ? meta.dirty_count : "—"}</td>
                <td className="dash-commit">
                  {status?.last_commit ? (
                    <>
                      <span className="mono hash">{status.last_commit.hash.slice(0, 8)}</span>{" "}
                      {status.last_commit.subject}{" "}
                      <span className="muted">{formatDate(status.last_commit.author_time)}</span>
                    </>
                  ) : (
                    "—"
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </section>
  );
}
