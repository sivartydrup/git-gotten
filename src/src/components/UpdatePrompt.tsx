import { useUpdaterStore } from "../store";

/**
 * The new-version prompt (SPEC §17): the update feed announced a release
 * newer than the running build. Install hands off to the updater plugin
 * (download → signature verify → install → relaunch); Dismiss hides this
 * version for the rest of the session. A failed check never renders here:
 * an unreachable feed is not an update, and its reason stays in the store.
 */
export function UpdatePrompt() {
  const { available, version, notes, installing, error, install, dismiss } = useUpdaterStore();
  if (!available) {
    return null;
  }

  return (
    <div className="update-prompt" role="status">
      <span className="update-prompt-label">
        Update available: <strong>{version}</strong>
        {notes && <em className="update-prompt-notes">{notes}</em>}
      </span>
      <span className="update-prompt-actions">
        <button
          type="button"
          className="primary"
          disabled={installing}
          title="Download, verify and install the update, then relaunch"
          onClick={() => void install()}
        >
          {installing ? "Installing…" : "Install"}
        </button>
        <button type="button" disabled={installing} onClick={dismiss}>
          Dismiss
        </button>
      </span>
      {error && <span className="update-prompt-error">{error}</span>}
    </div>
  );
}
