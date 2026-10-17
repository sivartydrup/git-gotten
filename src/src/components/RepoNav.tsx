import { useState } from "react";
import { useRepoStore } from "../store";

/**
 * Left navigation (SPEC §7): the open-repo list plus the open affordance.
 *
 * Stage 1 opens by pasting a path (works in the plain-web preview too); a native
 * directory picker replaces the text field in a later stage.
 */
export function RepoNav() {
  const { repos, activeId, opening, error, open, select, clearError } =
    useRepoStore();
  const [path, setPath] = useState("");

  async function submit(event: React.FormEvent) {
    event.preventDefault();
    if (!path.trim()) {
      return;
    }
    await open(path);
  }

  return (
    <nav className="repo-nav">
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
        <button className="open-btn" type="submit" disabled={opening || !path.trim()}>
          {opening ? "Opening…" : "Open"}
        </button>
      </form>

      {error && (
        <p className="open-error" role="alert">
          {error}
        </p>
      )}

      <ul className="repo-list">
        {repos.map((repo) => (
          <li key={repo.id}>
            <button
              className={`repo-item${repo.id === activeId ? " active" : ""}`}
              onClick={() => select(repo.id)}
              title={repo.path}
            >
              <span className="repo-name">{repo.name}</span>
              <span className="repo-branch">{repo.branch ?? "detached"}</span>
              {repo.dirty_count > 0 && (
                <span className="badge dirty">{repo.dirty_count}</span>
              )}
            </button>
          </li>
        ))}
      </ul>

      {repos.length === 0 && !error && (
        <p className="nav-hint">No repositories open. Paste a path to get started.</p>
      )}
    </nav>
  );
}
