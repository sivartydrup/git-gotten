import { useEffect, useRef, useState } from "react";
import {
  childrenOf,
  moveNode,
  navOrder,
  repoIdsInTree,
  useRepoStore,
  useViewStore,
  useWorkspaceStore,
} from "../store";
import { ROOT_GROUP_ID, type ChildNode, type RepoMeta } from "../types";
import { Icon } from "./Icons";
import { isTauri } from "@tauri-apps/api/core";
import { pickRepositoryFolder } from "../api";

/** The folder that contains `path` — the picker's starting directory. */
function parentDir(path: string | undefined): string | undefined {
  if (!path) {
    return undefined;
  }
  const trimmed = path.replace(/[\\/]+$/, "");
  const cut = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  return cut > 0 ? trimmed.slice(0, cut) : undefined;
}

/**
 * Left navigation (SPEC §7): the workspace group tree — nested groups, pinned
 * repos first, recents ordering, search, drag-and-drop reorder, pin and
 * remove-from-workspace — plus the open affordance and the dashboard entry.
 */
export function RepoNav() {
  const repos = useRepoStore((s) => s.repos);
  const activeId = useRepoStore((s) => s.activeId);
  const opening = useRepoStore((s) => s.opening);
  const error = useRepoStore((s) => s.error);
  const open = useRepoStore((s) => s.open);
  const select = useRepoStore((s) => s.select);
  const clearError = useRepoStore((s) => s.clearError);

  const tree = useWorkspaceStore((s) => s.tree);
  const addGroup = useWorkspaceStore((s) => s.addGroup);
  const renameGroup = useWorkspaceStore((s) => s.renameGroup);
  const deleteGroup = useWorkspaceStore((s) => s.deleteGroup);
  const commitTree = useWorkspaceStore((s) => s.commitTree);
  const pin = useWorkspaceStore((s) => s.pin);
  const removeRepo = useWorkspaceStore((s) => s.removeRepo);

  const dashboard = useViewStore((s) => s.dashboard);
  const setDashboard = useViewStore((s) => s.setDashboard);

  const [path, setPath] = useState("");
  const [filter, setFilter] = useState("");
  const [editing, setEditing] = useState<string | null>(null);
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({});
  const drag = useRef<ChildNode | null>(null);

  const metaOf = new Map<string, RepoMeta>(repos.map((m) => [m.id, m]));
  // Repos not (yet) in the persisted tree render at the root, so the nav never
  // hides a repo the user just opened.
  const treeIds = new Set(repoIdsInTree(tree));
  const orphans: ChildNode[] = repos
    .filter((m) => !treeIds.has(m.id))
    .map((m) => ({ kind: "repo", id: m.id }));

  const q = filter.trim().toLowerCase();
  const matchesRepo = (m: RepoMeta | undefined) =>
    !!m && (!q || m.name.toLowerCase().includes(q) || m.path.toLowerCase().includes(q));
  const groupVisible = (id: string): boolean => {
    if (!q) {
      return true;
    }
    const group = tree.groups.find((g) => g.id === id);
    if (group?.name.toLowerCase().includes(q)) {
      return true;
    }
    return navOrder(tree, id).some((c) =>
      c.kind === "repo" ? matchesRepo(metaOf.get(c.id)) : groupVisible(c.id),
    );
  };

  async function submit(event: React.FormEvent) {
    event.preventDefault();
    if (!path.trim()) {
      return;
    }
    const next = path;
    setPath("");
    await open(next);
  }

  const canBrowse = isTauri();

  /**
   * Fill the open field from the OS folder picker (SPEC §7's add-repo affordance).
   * The chosen path lands in the field instead of opening straight away: the user
   * sees the exact path about to be opened and can still edit or cancel it.
   */
  async function browse() {
    const active = repos.find((m) => m.id === activeId)?.path;
    const picked = await pickRepositoryFolder(parentDir(active));
    if (!picked) {
      return; // cancelled, or no native dialog available
    }
    setPath(picked);
    if (error) {
      clearError();
    }
  }

  function dropOn(targetParent: string, index: number) {
    const node = drag.current;
    drag.current = null;
    if (!node) {
      return;
    }
    void commitTree(moveNode(tree, node, targetParent, index));
  }

  // Below 980px the nav becomes a narrow rail: names ellipsize and the
  // secondary labels collapse, but every control keeps its accessible name.
  const [compact, setCompact] = useState(
    () =>
      typeof window !== "undefined" &&
      typeof window.matchMedia === "function" &&
      window.matchMedia("(max-width: 980px)").matches,
  );
  useEffect(() => {
    if (typeof window === "undefined" || typeof window.matchMedia !== "function") {
      return;
    }
    const mq = window.matchMedia("(max-width: 980px)");
    const onChange = (e: MediaQueryListEvent) => setCompact(e.matches);
    mq.addEventListener("change", onChange);
    return () => mq.removeEventListener("change", onChange);
  }, []);

  function repoRow(m: RepoMeta, parent: string, depth: number) {
    const pinned = tree.pinned.includes(m.id);
    return (
      <div
        key={m.id}
        className={`repo-item${m.id === activeId ? " active" : ""}`}
        style={{ paddingLeft: `${10 + depth * 14}px` }}
        title={m.path}
        draggable
        onDragStart={() => {
          drag.current = { kind: "repo", id: m.id };
        }}
        onDragOver={(e) => e.preventDefault()}
        onDrop={() =>
          dropOn(
            parent,
            childrenOf(tree, parent).findIndex((c) => c.kind === "repo" && c.id === m.id),
          )
        }
      >
        <button className="repo-link" onClick={() => select(m.id)}>
          {pinned && <span className="repo-pin-mark">★</span>}
          <span className="repo-name">{m.name}</span>
          <span className="repo-branch">{m.branch ?? "detached"}</span>
          {m.dirty_count > 0 && <span className="badge dirty">{m.dirty_count}</span>}
        </button>
        <button
          className="icon-btn"
          data-act="pin"
          aria-label={pinned ? "Unpin repo" : "Pin repo"}
          onClick={() => void pin(m.id, !pinned)}
        >
          <Icon name="star" size={13} />
        </button>
        <button
          className="icon-btn"
          data-act="remove"
          aria-label="Remove from workspace"
          title="Remove from workspace (the repo stays on disk)"
          onClick={() => void removeRepo(m.id)}
        >
          <Icon name="close" size={13} />
        </button>
      </div>
    );
  }

  function groupBlock(id: string, depth: number) {
    const group = tree.groups.find((g) => g.id === id);
    if (!group) {
      return null;
    }
    const kids = navOrder(tree, id).filter((c) =>
      c.kind === "repo" ? matchesRepo(metaOf.get(c.id)) : groupVisible(c.id),
    );
    return (
      <div className="group-block" key={`g:${id}`}>
        <div
          className="group-head"
          style={{ paddingLeft: `${6 + depth * 14}px` }}
          draggable
          onDragStart={() => {
            drag.current = { kind: "group", id };
          }}
          onDragOver={(e) => e.preventDefault()}
          onDrop={() => dropOn(id, childrenOf(tree, id).length)}
        >
          <button
            className="group-caret"
            aria-label={collapsed[id] ? "Expand group" : "Collapse group"}
            onClick={() => setCollapsed((c) => ({ ...c, [id]: !c[id] }))}
          >
            <Icon name={collapsed[id] ? "chevronRight" : "chevronDown"} size={13} />
          </button>
          {editing === id ? (
            <input
              className="group-rename open-input"
              autoFocus
              defaultValue={group.name}
              onBlur={(e) => {
                setEditing(null);
                if (e.target.value.trim() && e.target.value.trim() !== group.name) {
                  void renameGroup(id, e.target.value.trim());
                }
              }}
              onKeyDown={(e) => {
                if (e.key === "Enter") {
                  (e.target as HTMLInputElement).blur();
                }
              }}
            />
          ) : (
            <span className="group-name">{group.name}</span>
          )}
          <button className="icon-btn" data-act="rename-group" aria-label="Rename group" onClick={() => setEditing(id)}>
            <Icon name="edit" size={13} />
          </button>
          <button className="icon-btn" data-act="delete-group" aria-label="Delete group" onClick={() => void deleteGroup(id)}>
            <Icon name="trash" size={13} />
          </button>
        </div>
        {!collapsed[id] && kids.map((c) => (c.kind === "group" ? groupBlock(c.id, depth + 1) : repoRow(metaOf.get(c.id)!, id, depth + 1)))}
      </div>
    );
  }

  const rootKids = [
    ...navOrder(tree, ROOT_GROUP_ID),
    ...(tree.groups.find((g) => g.id === ROOT_GROUP_ID) ? orphans : []),
  ];

  return (
    <nav className="repo-nav" data-compact={compact ? "true" : undefined}>
      <form className="open-form" onSubmit={(e) => void submit(e)}>
        <input
          className="open-input"
          placeholder="Open a repo… (path)"
          value={path}
          onChange={(e) => {
            setPath(e.target.value);
            if (error) clearError();
          }}
        />
        {canBrowse && (
          <button
            className="icon-btn browse-btn"
            type="button"
            aria-label="Browse for a repository folder"
            title="Browse for a repository folder"
            disabled={opening}
            onClick={() => void browse()}
          >
            <Icon name="browse" size={14} />
          </button>
        )}
        <button className="open-btn" type="submit" disabled={opening || !path.trim()}>
          {opening ? "Opening…" : "Open"}
        </button>
      </form>

      {error && (
        <p className="open-error" role="alert">
          {error}
        </p>
      )}

      <div className="nav-actions">
        <input
          className="open-input nav-search"
          placeholder="Search repos"
          aria-label="Search repos"
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
        />
        <button className="icon-btn" aria-label="Add group" onClick={() => void addGroup("New group", null)}>
          <Icon name="plus" size={14} /> <span className="btn-label">group</span>
        </button>
        <button
          className={`icon-btn nav-dashboard${dashboard ? " active" : ""}`}
          aria-label="All Repositories"
          onClick={() => setDashboard(!dashboard)}
        >
          <Icon name="dashboard" size={14} /> <span className="btn-label">All Repositories</span>
        </button>
      </div>

      <div className="repo-list">
        {rootKids
          .filter((c) =>
            c.kind === "repo" ? matchesRepo(metaOf.get(c.id)) : groupVisible(c.id),
          )
          .map((c) =>
            c.kind === "group"
              ? groupBlock(c.id, 0)
              : repoRow(metaOf.get(c.id)!, ROOT_GROUP_ID, 0),
          )}
      </div>

      {repos.length === 0 && !error && (
        <p className="nav-hint">No repositories open. Paste a path to get started.</p>
      )}
    </nav>
  );
}
