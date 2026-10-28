import { useState } from "react";
import { useActivityStore, useRefsStore, useRepoStore } from "../store";
import type { Branch, Op, Remote, Tag } from "../types";

export type RefsTab = "branches" | "remotes" | "tags";

/**
 * The Branches / Remotes / Tags tabs (SPEC §10.5): lists of the active repo's
 * refs with per-row actions and a create dialog per kind. Every action goes
 * through the activity store's preview flow (`planOp` → confirm → `runOp`),
 * so the exact git command is always the Rust core's (SPEC §8).
 */
export function RefsPanel({ tab }: { tab: RefsTab }) {
  const { branches, remotes, tags, loading, error } = useRefsStore();
  const activeId = useRepoStore((s) => s.activeId);
  const status = useRepoStore((s) => (s.activeId ? s.status[s.activeId] : undefined));
  const { requestPreview, running } = useActivityStore();

  const defaultRemote = remotes[0]?.name ?? null;
  const act = (op: Op) => {
    if (activeId) {
      void requestPreview(activeId, op);
    }
  };

  if (error) {
    return (
      <div className="refs-panel">
        <div className="refs-state">Failed to load refs: {error}</div>
      </div>
    );
  }

  return (
    <div className="refs-panel">
      {tab === "branches" && (
        <BranchesTab branches={branches} loading={loading} defaultRemote={defaultRemote} head={status?.head ?? null} act={act} running={running} />
      )}
      {tab === "remotes" && <RemotesTab remotes={remotes} loading={loading} act={act} running={running} />}
      {tab === "tags" && <TagsTab tags={tags} loading={loading} defaultRemote={defaultRemote} head={status?.head ?? null} act={act} running={running} />}
    </div>
  );
}

function RowButton({ label, disabled, onClick }: { label: string; disabled: boolean; onClick: () => void }) {
  return (
    <button type="button" className="btn row-btn" disabled={disabled} onClick={onClick}>
      {label}
    </button>
  );
}

// ── Branches ─────────────────────────────────────────────────────────────────

function BranchesTab({
  branches,
  loading,
  defaultRemote,
  head,
  act,
  running,
}: {
  branches: Branch[];
  loading: boolean;
  defaultRemote: string | null;
  head: string | null;
  act: (op: Op) => void;
  running: boolean;
}) {
  const [renaming, setRenaming] = useState<Branch | null>(null);
  const [createOpen, setCreateOpen] = useState(false);

  return (
    <div className="refs-section">
      <div className="refs-head">
        <h3>Branches</h3>
        <button
          type="button"
          className="btn"
          disabled={running || !head}
          onClick={() => setCreateOpen(true)}
        >
          Create
        </button>
      </div>
      {loading ? (
        <div className="refs-state">Loading…</div>
      ) : (
        <ul className="refs-list">
          {branches.map((b) => {
            // A remote-tracking branch (`origin/main`) is deleted via its
            // remote; a local branch via `remote: null`.
            const slash = b.is_remote ? b.name.indexOf("/") : -1;
            const delName = b.is_remote && slash > 0 ? b.name.slice(slash + 1) : b.name;
            const delRemote = b.is_remote && slash > 0 ? b.name.slice(0, slash) : null;
            return (
              <li key={b.name} className={`refs-row ${b.is_current ? "current" : ""}`}>
                <span className="refs-name">{b.name}</span>
                {b.is_current && <span className="badge">current</span>}
                {b.upstream && <span className="refs-upstream">↔ {b.upstream}</span>}
                {b.ahead > 0 && <span className="badge sync">{b.ahead} ahead</span>}
                {b.behind > 0 && <span className="badge sync">{b.behind} behind</span>}
                <span className="row-actions">
                  {!b.is_remote && !b.is_current && (
                    <RowButton
                      label="Switch"
                      disabled={running}
                      onClick={() => act({ kind: "checkout", target: b.name, force: false })}
                    />
                  )}
                  {!b.is_remote && b.upstream === null && defaultRemote && (
                    <RowButton
                      label="Push"
                      disabled={running}
                      onClick={() =>
                        act({
                          kind: "push",
                          remote: defaultRemote,
                          branch: b.name,
                          force: false,
                          set_upstream: true,
                        })
                      }
                    />
                  )}
                  {!b.is_remote && (
                    <RowButton label="Rename" disabled={running} onClick={() => setRenaming(b)} />
                  )}
                  {!b.is_remote && !b.is_current && (
                    <RowButton
                      label="Rebase onto"
                      disabled={running}
                      onClick={() => act({ kind: "rebase_onto", target: b.name })}
                    />
                  )}
                  <RowButton
                    label="Delete"
                    disabled={running}
                    onClick={() =>
                      act({
                        kind: "branch_delete",
                        name: delName,
                        remote: delRemote,
                        force: false,
                      })
                    }
                  />
                </span>
              </li>
            );
          })}
        </ul>
      )}
      {renaming && <RenameBranchDialog branch={renaming} onClose={() => setRenaming(null)} act={act} running={running} />}
      {createOpen && head && <CreateBranchDialog head={head} onClose={() => setCreateOpen(false)} act={act} running={running} />}
    </div>
  );
}

function RenameBranchDialog({
  branch,
  onClose,
  act,
  running,
}: {
  branch: Branch;
  onClose: () => void;
  act: (op: Op) => void;
  running: boolean;
}) {
  const [name, setName] = useState(branch.name);
  return (
    <div className="dialog-overlay" role="presentation" onMouseDown={(e) => {
      if (e.target === e.currentTarget && !running) onClose();
    }}>
      <div className="dialog" role="dialog" aria-modal="true" aria-label="Rename branch">
        <div className="dialog-head">
          <h2>Rename branch</h2>
        </div>
        <input
          className="dialog-input"
          value={name}
          onChange={(e) => setName(e.target.value)}
          autoFocus
        />
        <div className="dialog-actions">
          <button type="button" className="btn" disabled={running} onClick={onClose}>
            Cancel
          </button>
          <button
            type="button"
            className="btn primary"
            disabled={running || !name.trim() || name.trim() === branch.name}
            onClick={() => act({ kind: "branch_rename", name: branch.name, new_name: name.trim() })}
          >
            Rename
          </button>
        </div>
      </div>
    </div>
  );
}

function CreateBranchDialog({
  head,
  onClose,
  act,
  running,
}: {
  head: string;
  onClose: () => void;
  act: (op: Op) => void;
  running: boolean;
}) {
  const [name, setName] = useState("");
  return (
    <div className="dialog-overlay" role="presentation" onMouseDown={(e) => {
      if (e.target === e.currentTarget && !running) onClose();
    }}>
      <div className="dialog" role="dialog" aria-modal="true" aria-label="Create branch">
        <div className="dialog-head">
          <h2>Create branch</h2>
        </div>
        <p className="refs-state">
          from <code>{head.slice(0, 12)}</code>
        </p>
        <input
          className="dialog-input"
          placeholder="branch name"
          value={name}
          onChange={(e) => setName(e.target.value)}
          autoFocus
        />
        <div className="dialog-actions">
          <button type="button" className="btn" disabled={running} onClick={onClose}>
            Cancel
          </button>
          <button
            type="button"
            className="btn primary"
            disabled={running || !name.trim()}
            onClick={() => act({ kind: "branch_create", name: name.trim(), from: head, checkout: false })}
          >
            Create
          </button>
        </div>
      </div>
    </div>
  );
}

// ── Remotes ──────────────────────────────────────────────────────────────────

function RemotesTab({
  remotes,
  loading,
  act,
  running,
}: {
  remotes: Remote[];
  loading: boolean;
  act: (op: Op) => void;
  running: boolean;
}) {
  const [addOpen, setAddOpen] = useState(false);
  return (
    <div className="refs-section">
      <div className="refs-head">
        <h3>Remotes</h3>
        <button type="button" className="btn" disabled={running} onClick={() => setAddOpen(true)}>
          Add
        </button>
      </div>
      {loading ? (
        <div className="refs-state">Loading…</div>
      ) : (
        <ul className="refs-list">
          {remotes.map((r) => (
            <li key={r.name} className="refs-row">
              <span className="refs-name">{r.name}</span>
              <code className="refs-url">{r.url}</code>
              <span className="row-actions">
                <RowButton
                  label="Fetch"
                  disabled={running}
                  onClick={() => act({ kind: "remote_fetch", remote: r.name, refspec: null })}
                />
                <RowButton
                  label="Push"
                  disabled={running}
                  onClick={() =>
                    act({ kind: "push", remote: r.name, branch: null, force: false, set_upstream: false })
                  }
                />
                <RowButton
                  label="Remove"
                  disabled={running}
                  onClick={() => act({ kind: "remote_remove", name: r.name })}
                />
              </span>
            </li>
          ))}
        </ul>
      )}
      {addOpen && <AddRemoteDialog onClose={() => setAddOpen(false)} act={act} running={running} />}
    </div>
  );
}

function AddRemoteDialog({
  onClose,
  act,
  running,
}: {
  onClose: () => void;
  act: (op: Op) => void;
  running: boolean;
}) {
  const [name, setName] = useState("");
  const [url, setUrl] = useState("");
  return (
    <div className="dialog-overlay" role="presentation" onMouseDown={(e) => {
      if (e.target === e.currentTarget && !running) onClose();
    }}>
      <div className="dialog" role="dialog" aria-modal="true" aria-label="Add remote">
        <div className="dialog-head">
          <h2>Add remote</h2>
        </div>
        <input
          className="dialog-input"
          placeholder="name (e.g. upstream)"
          value={name}
          onChange={(e) => setName(e.target.value)}
          autoFocus
        />
        <input
          className="dialog-input"
          placeholder="url"
          value={url}
          onChange={(e) => setUrl(e.target.value)}
        />
        <div className="dialog-actions">
          <button type="button" className="btn" disabled={running} onClick={onClose}>
            Cancel
          </button>
          <button
            type="button"
            className="btn primary"
            disabled={running || !name.trim() || !url.trim()}
            onClick={() => act({ kind: "remote_add", name: name.trim(), url: url.trim() })}
          >
            Add
          </button>
        </div>
      </div>
    </div>
  );
}

// ── Tags ─────────────────────────────────────────────────────────────────────

function TagsTab({
  tags,
  loading,
  defaultRemote,
  head,
  act,
  running,
}: {
  tags: Tag[];
  loading: boolean;
  defaultRemote: string | null;
  head: string | null;
  act: (op: Op) => void;
  running: boolean;
}) {
  const [createOpen, setCreateOpen] = useState(false);
  return (
    <div className="refs-section">
      <div className="refs-head">
        <h3>Tags</h3>
        <button
          type="button"
          className="btn"
          disabled={running || !head}
          onClick={() => setCreateOpen(true)}
        >
          Create
        </button>
      </div>
      {loading ? (
        <div className="refs-state">Loading…</div>
      ) : (
        <ul className="refs-list">
          {tags.map((t) => (
            <li key={t.name} className="refs-row">
              <span className="refs-name">{t.name}</span>
              {t.annotated && <span className="badge">annotated</span>}
              {t.message && <span className="refs-msg">{t.message}</span>}
              <code className="refs-url">{t.target.slice(0, 12)}</code>
              <span className="row-actions">
                <RowButton
                  label="Checkout"
                  disabled={running}
                  onClick={() => act({ kind: "tag_checkout", name: t.name, force: false })}
                />
                {defaultRemote && (
                  <RowButton
                    label="Push"
                    disabled={running}
                    onClick={() => act({ kind: "tag_push", remote: defaultRemote, name: t.name, force: false })}
                  />
                )}
                <RowButton
                  label="Delete"
                  disabled={running}
                  onClick={() => act({ kind: "tag_delete", name: t.name, remote: null })}
                />
              </span>
            </li>
          ))}
        </ul>
      )}
      {createOpen && head && <CreateTagDialog head={head} onClose={() => setCreateOpen(false)} act={act} running={running} />}
    </div>
  );
}

function CreateTagDialog({
  head,
  onClose,
  act,
  running,
}: {
  head: string;
  onClose: () => void;
  act: (op: Op) => void;
  running: boolean;
}) {
  const [name, setName] = useState("");
  const [annotated, setAnnotated] = useState(false);
  const [message, setMessage] = useState("");
  return (
    <div className="dialog-overlay" role="presentation" onMouseDown={(e) => {
      if (e.target === e.currentTarget && !running) onClose();
    }}>
      <div className="dialog" role="dialog" aria-modal="true" aria-label="Create tag">
        <div className="dialog-head">
          <h2>Create tag</h2>
        </div>
        <p className="refs-state">
          on <code>{head.slice(0, 12)}</code>
        </p>
        <input
          className="dialog-input"
          placeholder="tag name (e.g. v1.0)"
          value={name}
          onChange={(e) => setName(e.target.value)}
          autoFocus
        />
        <label className="dialog-check">
          <input
            type="checkbox"
            checked={annotated}
            onChange={(e) => setAnnotated(e.target.checked)}
          />{" "}
          annotated
        </label>
        {annotated && (
          <input
            className="dialog-input"
            placeholder="message"
            value={message}
            onChange={(e) => setMessage(e.target.value)}
          />
        )}
        <div className="dialog-actions">
          <button type="button" className="btn" disabled={running} onClick={onClose}>
            Cancel
          </button>
          <button
            type="button"
            className="btn primary"
            disabled={running || !name.trim()}
            onClick={() =>
              act({
                kind: "tag_create",
                name: name.trim(),
                target: head,
                annotated,
                message: annotated ? message : null,
              })
            }
          >
            Create
          </button>
        </div>
      </div>
    </div>
  );
}
