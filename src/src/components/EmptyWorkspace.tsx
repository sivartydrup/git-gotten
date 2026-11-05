import { Icon } from "./Icons";
import "./empty-workspace.css";

/**
 * First-run guidance (SPEC §7): the main area before any repository is open.
 * It says what the graph will show, states the transparency contract, and hands
 * the user to the nav's open field; the shortcut list mirrors App.tsx's keys.
 */
export function EmptyWorkspace() {
  const focusOpenField = () => {
    document
      .querySelector<HTMLElement>(".repo-nav .open-form .open-input")
      ?.focus();
  };

  return (
    <section className="empty-workspace" aria-label="Get started">
      <Icon name="graph" size={40} className="empty-workspace-icon" />
      <h2>No repository open</h2>
      <p>
        Open a repository and its commit history appears here as a branching
        graph — one row per commit, one lane per branch.
      </p>
      <p>
        Every action runs an exact <code>git</code> command: you always see that
        command, its steps and its risk before anything touches the repo.
      </p>
      <button type="button" className="btn primary" onClick={focusOpenField}>
        <Icon name="repo" size={14} /> <span className="btn-label">Open a repository</span>
      </button>
      <ul className="empty-workspace-keys">
        <li>
          <kbd>⌘</kbd>
          <kbd>K</kbd> command palette
        </li>
        <li>
          <kbd>1</kbd>–<kbd>7</kbd> switch tabs
        </li>
        <li>
          <kbd>/</kbd> filter the graph
        </li>
      </ul>
    </section>
  );
}
