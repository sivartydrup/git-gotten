import { useEffect, useMemo, useRef, useState, type KeyboardEvent as ReactKeyboardEvent } from "react";
import { planOp } from "../api";
import {
  EMPTY_DRAFT,
  useActivityStore,
  useCommitDraftStore,
  useGraphStore,
  useMainTab,
  useRepoStore,
  useRefsStore,
  type MainTab,
} from "../store";
import type { Op, Risk } from "../types";
import { Icon, type IconName } from "./Icons";
import { RiskBadge } from "./PreviewDialog";
import { useDialog } from "./useDialog";
import "./command-palette.css";

/** A row the palette can run. `op` rows preview through the normal flow. */
interface PaletteItem {
  id: string;
  group: string;
  label: string;
  /** Secondary text (repo path, commit hash, shortcut chip). */
  meta?: string;
  icon: IconName;
  /** The `Op` this row runs — always through `requestPreview` (SPEC §9). */
  op?: Op;
  /** Non-op rows are pure UI moves (switch repo, switch tab, select a commit). */
  action?: () => void;
}

interface Planned {
  display: string;
  risk: Risk;
}

const RISK_ORDER: Risk[] = ["ReadOnly", "Mutating", "Dangerous"];

/** The tab order of the shell, i.e. the `1`..`7` shortcuts (App.tsx). */
export const TAB_ORDER: MainTab[] = [
  "graph",
  "files",
  "conflicts",
  "branches",
  "worktrees",
  "stash",
  "bisect",
];

const TAB_ICONS: Record<MainTab, IconName> = {
  graph: "graph",
  files: "files",
  conflicts: "conflict",
  branches: "branch",
  worktrees: "worktree",
  stash: "stash",
  bisect: "bisect",
};

/**
 * Row filter: every query token must appear in the row's label or meta, so one
 * query cannot match an unrelated group.
 */
function matches(query: string, label: string, ...fields: (string | undefined)[]): boolean {
  const haystack = `${label} ${fields.join(" ")}`.toLowerCase();
  return query
    .toLowerCase()
    .split(/\s+/)
    .filter(Boolean)
    .every((token) => haystack.includes(token));
}

/**
 * The ⌘K command palette (SPEC §9): a searchable list of the commands the app
 * can run, grouped into Repository actions, Repositories, Tabs and Commits.
 *
 * Transparency invariant: every action row shows the exact `CommandPlan.display`
 * the Rust core compiles for it (via `planOp`) plus its risk class, and
 * `Enter` only opens the normal preview/confirm dialog — the palette never runs
 * an op, so a `Dangerous` action still lands in the type-to-confirm dialog.
 */
export function CommandPalette({ onClose }: { onClose: () => void }) {
  const activeId = useRepoStore((s) => s.activeId);
  const repos = useRepoStore((s) => s.repos);
  const statusMap = useRepoStore((s) => s.status);
  const { branches, remotes } = useRefsStore();
  const graph = useGraphStore((s) => s.graph);
  const setTab = useMainTab((s) => s.setTab);
  // The draft lives in the store (`CommitPanel` writes it), one per repository;
  // subscribing to it keeps the offered commit in sync with the commit box.
  const draft = useCommitDraftStore((s) => s.drafts[activeId ?? ""] ?? EMPTY_DRAFT);

  const [query, setQuery] = useState("");
  const [index, setIndex] = useState(0);
  const [planned, setPlanned] = useState<Record<string, Planned>>({});
  const listRef = useRef<HTMLUListElement | null>(null);
  const dialogRef = useDialog(true, onClose, { escape: false });

  const status = activeId ? statusMap[activeId] ?? null : null;
  const upstreamRemote = status?.upstream?.split("/")[0] ?? null;
  const fetchRemote = upstreamRemote ?? remotes[0]?.name ?? null;

  const items = useMemo<PaletteItem[]>(() => {
    const out: PaletteItem[] = [];
    const op = (id: string, label: string, icon: IconName, value: Op, meta?: string) =>
      out.push({ id, group: "Repository actions", label, icon, op: value, meta });

    if (activeId && status) {
      if (fetchRemote) {
        op("fetch", `Fetch ${fetchRemote}`, "fetch", {
          kind: "remote_fetch",
          remote: fetchRemote,
          refspec: null,
        });
      }
      if (status.upstream) {
        op("pull", "Pull", "pull", { kind: "pull", rebase: false });
        op("pull-rebase", "Pull (rebase)", "pullRebase", { kind: "pull", rebase: true });
        if (status.branch) {
          op("push", `Push ${status.branch}`, "push", {
            kind: "push",
            remote: upstreamRemote ?? fetchRemote!,
            branch: status.branch,
            force: false,
            set_upstream: false,
          });
        }
        if (status.branch) {
          op("rebase-onto", `Rebase onto ${status.upstream}`, "rebase", {
            kind: "rebase_onto",
            target: status.upstream,
          });
        }
      }
      const mergeTargets = branches.filter((b) => !b.is_remote && !b.is_current);
      if (status.branch && mergeTargets.length > 0) {
        op("merge", "Merge a branch…", "merge", { kind: "merge", source: mergeTargets[0].name }, `into ${status.branch}; first local branch listed: ${mergeTargets[0].name}`);
      }
      op("stage-all", "Stage all changes", "files", { kind: "stage", path: null });
      op("unstage-all", "Unstage all changes", "minus", { kind: "unstage", path: null });
      // The commit the palette offers is literally the one the commit panel would
      // run — same message, amend flag, author override and co-authors.
      const commitDraft = activeId
        ? useCommitDraftStore.getState().commitOp(activeId)
        : null;
      if (commitDraft) {
        const staged = status.changes.filter((c) => c.staged).length;
        if (staged > 0 || commitDraft.amend) {
          const firstLine = commitDraft.message.split("\n")[0];
          op(
            "commit",
            commitDraft.amend ? "Amend HEAD commit" : "Commit staged changes",
            "commit",
            commitDraft,
            commitDraft.amend ? firstLine : `${staged} staged · ${firstLine}`,
          );
        }
      } else {
        out.push({
          id: "commit-draft",
          group: "Repository actions",
          label: "Write a commit message",
          meta: "focuses the commit box",
          icon: "commit",
          action: () =>
            document.querySelector<HTMLTextAreaElement>(".commit-message")?.focus(),
        });
      }
      op("stash", "Stash changes", "stash", { kind: "stash_push", include_untracked: false });
      if (status.in_merge || status.in_rebase || status.in_cherry_pick) {
        op("continue", "Continue the operation", "check", { kind: "continue_op" });
        op("abort", "Abort the operation", "danger", { kind: "abort_op" });
      }
      for (const b of branches.filter((b) => !b.is_current).slice(0, 8)) {
        op(`checkout:${b.name}`, `Switch to ${b.name}`, "branch", {
          kind: "checkout",
          target: b.name,
          force: false,
        });
      }
    }

    for (const r of repos) {
      out.push({
        id: `repo:${r.id}`,
        group: "Repositories",
        label: r.name,
        meta: r.path,
        icon: "repo",
        action: () => useRepoStore.getState().select(r.id),
      });
    }

    TAB_ORDER.forEach((t, i) => {
      out.push({
        id: `tab:${t}`,
        group: "Tabs",
        label: t[0].toUpperCase() + t.slice(1),
        meta: `${i + 1}`,
        icon: TAB_ICONS[t],
        action: () => setTab(t),
      });
    });

    for (const node of graph?.nodes.slice(0, 40) ?? []) {
      out.push({
        id: `commit:${node.hash}`,
        group: "Commits",
        label: node.subject,
        meta: node.hash.slice(0, 10),
        icon: "commit",
        action: () => useGraphStore.getState().select(node.hash),
      });
    }

    return out;
  }, [activeId, status, branches, remotes, repos, graph, setTab, fetchRemote, upstreamRemote, draft]);

  const visible = useMemo(
    () =>
      items.filter((it) => matches(query, it.label, it.meta ?? "")),
    [items, query],
  );

  // Selection is an index into `visible`; clamp when the filter narrows it.
  const selected = Math.min(index, Math.max(0, visible.length - 1));

  // Exact command previews: one `plan_op` per *op* row, batched and debounced
  // (never per keystroke), cached per repo + op (SPEC §9: the UI never invents
  // a command string).
  const cacheKey = useRef<Record<string, Planned>>({});
  useEffect(() => {
    const repo = activeId;
    if (!repo) {
      return;
    }
    const missing = visible
      .filter((it) => it.op)
      .map((it) => JSON.stringify(it.op))
      .filter((key, i) => i < 12 && !cacheKey.current[key]);
    if (missing.length === 0) {
      return;
    }
    const timer = setTimeout(() => {
      void Promise.all(
        missing.map(async (key) => {
          const plan = await planOp(repo, JSON.parse(key) as Op);
          if (!plan) {
            return null;
          }
          const risk = plan.steps.reduce<Risk>(
            (max, s) => (RISK_ORDER.indexOf(s.risk) > RISK_ORDER.indexOf(max) ? s.risk : max),
            "ReadOnly",
          );
          const value: Planned = { display: plan.display, risk };
          cacheKey.current[key] = value;
          return [key, value] as const;
        }),
      ).then((pairs) => {
        const fresh = pairs.filter(Boolean) as [string, Planned][];
        if (fresh.length > 0) {
          setPlanned((prev) => {
            const next = { ...prev };
            for (const [key, value] of fresh) {
              next[key] = value;
            }
            return next;
          });
        }
      });
    }, 180);
    return () => clearTimeout(timer);
  }, [visible, activeId]);

  const activate = (item: PaletteItem) => {
    if (item.op) {
      // Plan → preview → (type-to-)confirm. The palette never runs anything.
      if (activeId) {
        void useActivityStore.getState().requestPreview(activeId, item.op);
      }
    } else {
      item.action?.();
    }
    onClose();
  };

  const onKeyDown = (e: ReactKeyboardEvent) => {
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const n = visible.length;
      if (n === 0) {
        return;
      }
      const next =
        e.key === "ArrowDown"
          ? (selected + 1) % n
          : (selected - 1 + n) % n;
      setIndex(next);
      listRef.current
        ?.querySelector<HTMLElement>(`[data-row="${next}"]`)
        ?.scrollIntoView({ block: "nearest" });
      return;
    }
    if (e.key === "Home") {
      e.preventDefault();
      setIndex(0);
      return;
    }
    if (e.key === "End") {
      e.preventDefault();
      setIndex(Math.max(0, visible.length - 1));
      return;
    }
    if (e.key === "Enter") {
      e.preventDefault();
      const item = visible[selected];
      if (item) {
        activate(item);
      }
    }
  };

  let group: string | null = null;
  const freshWorkspace = repos.length === 0 && !activeId;

  return (
    <div className="palette-overlay" role="presentation" onMouseDown={(e) => {
      if (e.target === e.currentTarget) onClose();
    }}>
      <div
        className="palette"
        ref={dialogRef}
        role="dialog"
        aria-modal="true"
        aria-label="Command palette"
        onKeyDown={onKeyDown}
      >
        <div className="palette-filter">
          <Icon name="search" size={14} />
          <input
            className="palette-input"
            role="combobox"
            aria-expanded="true"
            aria-controls="palette-list"
            aria-label="Search commands"
            placeholder="Search commands, repos, tabs, commits…"
            value={query}
            onChange={(e) => {
              setQuery(e.target.value);
              setIndex(0);
            }}
          />
          <span className="palette-count">{visible.length}</span>
        </div>

        {freshWorkspace ? (
          <div className="palette-empty">
            No repository open. Open one from the left panel: every command here
            shows the exact <code>git</code> command before anything runs.
          </div>
        ) : visible.length === 0 ? (
          <div className="palette-empty">No commands match “{query}”.</div>
        ) : (
          <ul
            className="palette-list"
            id="palette-list"
            ref={listRef}
            role="listbox"
            aria-label="Commands"
          >
            {visible.map((it, i) => {
              const showGroup = it.group !== group;
              group = it.group;
              const plan = it.op ? planned[JSON.stringify(it.op)] : undefined;
              return (
                <li key={it.id} data-row={i}>
                  {showGroup && <div className="palette-group">{it.group}</div>}
                  <button
                    type="button"
                    role="option"
                    aria-selected={i === selected}
                    className={`palette-row ${i === selected ? "active" : ""}`}
                    onMouseMove={() => setIndex(i)}
                    onClick={() => activate(it)}
                  >
                    <Icon name={it.icon} size={14} />
                    <span className="palette-label">{it.label}</span>
                    {plan ? (
                      <code className="palette-cmd">{plan.display}</code>
                    ) : it.meta ? (
                      <code className="palette-meta">{it.meta}</code>
                    ) : null}
                    {plan && <RiskBadge risk={plan.risk} />}
                  </button>
                </li>
              );
            })}
          </ul>
        )}

        <div className="palette-footer">
          <span className="palette-hint">
            <kbd>↑</kbd>
            <kbd>↓</kbd> navigate
          </span>
          <span className="palette-hint">
            <kbd>↵</kbd> preview command
          </span>
          <span className="palette-hint">
            <kbd>esc</kbd> close
          </span>
          <span className="palette-hint">
            <kbd>1</kbd>–<kbd>7</kbd> tabs
          </span>
          <span className="palette-hint">
            <kbd>/</kbd> filter graph
          </span>
          <span className="palette-hint">
            <kbd>⌘</kbd>
            <kbd>K</kbd> open
          </span>
        </div>
      </div>
    </div>
  );
}
