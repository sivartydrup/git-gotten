import { useState } from "react";
import { useActivityStore } from "../store";
import type { LogEntry } from "../types";

function timeLabel(ts: number): string {
  const date = new Date(ts);
  const h = String(date.getHours()).padStart(2, "0");
  const m = String(date.getMinutes()).padStart(2, "0");
  const s = String(date.getSeconds()).padStart(2, "0");
  return `${h}:${m}:${s}`;
}

function LogRow({ entry }: { entry: LogEntry }) {
  const [expanded, setExpanded] = useState(false);
  const hasOutput = entry.stdout.length > 0 || entry.stderr.length > 0;

  return (
    <li className={`log-row ${entry.ok ? "ok" : "fail"}`}>
      <span className="log-time" title={new Date(entry.ts).toLocaleString()}>
        {timeLabel(entry.ts)}
      </span>
      <span className="log-status" aria-label={entry.ok ? "success" : "failed"}>
        {entry.ok ? "✓" : "✗"}
      </span>
      <button
        type="button"
        className="log-cmd"
        disabled={!hasOutput}
        onClick={() => setExpanded((v) => !v)}
        title={hasOutput ? (expanded ? "Hide output" : "Show output") : entry.display}
      >
        {entry.display}
      </button>
      {hasOutput && (
        <span className="log-toggle" aria-hidden>
          {expanded ? "▾" : "▸"}
        </span>
      )}
      <span className="log-duration">{entry.duration_ms} ms</span>
      {expanded && hasOutput && (
        <pre className="log-output">
          {entry.stdout && <div>{entry.stdout}</div>}
          {entry.stderr && (
            <div className="log-stderr">{entry.stderr}</div>
          )}
        </pre>
      )}
    </li>
  );
}

/**
 * Bottom activity panel (SPEC §9/§11): the command stream — every git command
 * the app has run, newest first, with the exact displayed line, outcome, and
 * duration. Clicking an entry expands its captured stdout/stderr (the
 * transparency contract: a failed op always keeps its exact output).
 */
export function ActivityLog() {
  const { activities, running, runningDisplay, runningRisk } = useActivityStore();

  return (
    <aside className="activity" aria-label="Activity log">
      <div className="activity-head">
        <h2>Activity</h2>
        {running && (
          <span className={`log-running risk-${runningRisk?.toLowerCase() ?? ""}`}>
            <span className="spinner" aria-hidden />
            {runningDisplay ?? "running…"}
          </span>
        )}
      </div>
      {activities.length === 0 ? (
        <p className="activity-empty">{running ? "Running…" : "No commands run yet."}</p>
      ) : (
        <ul className="log-rows">
          {activities.map((entry) => (
            <LogRow key={entry.id} entry={entry} />
          ))}
        </ul>
      )}
    </aside>
  );
}
