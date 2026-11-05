import { useEffect, useState } from "react";
import { useActivityStore } from "../store";
import type { LogEntry, Risk } from "../types";
import { friendlyGitError } from "../errors";
import { Icon } from "./Icons";
import { riskOf } from "./activityRisk";
import "./diff-activity.css";

function timeLabel(ts: number): string {
  const date = new Date(ts);
  const h = String(date.getHours()).padStart(2, "0");
  const m = String(date.getMinutes()).padStart(2, "0");
  const s = String(date.getSeconds()).padStart(2, "0");
  return `${h}:${m}:${s}`;
}

const FILTERS = [
  { key: "all", label: "All" },
  { key: "failed", label: "Failed" },
  { key: "mutating", label: "Mutating", risk: "Mutating" as Risk },
  { key: "dangerous", label: "Dangerous", risk: "Dangerous" as Risk },
] as const;

type Filter = (typeof FILTERS)[number]["key"];

function LogRow({ entry }: { entry: LogEntry }) {
  const [expanded, setExpanded] = useState(false);
  const [copied, setCopied] = useState(false);
  const hasOutput = entry.stdout.length > 0 || entry.stderr.length > 0;
  // SPEC §18: the plain-language reading of the failure, above the raw output
  // (which stays one click away — the transparency contract).
  const hint = entry.ok ? null : friendlyGitError(`${entry.stderr}\n${entry.stdout}`);
  const risk = riskOf(entry.display);

  useEffect(() => {
    if (!copied) {
      return undefined;
    }
    const timer = setTimeout(() => setCopied(false), 1200);
    return () => clearTimeout(timer);
  }, [copied]);

  return (
    <li className={`log-row ${entry.ok ? "ok" : "fail"} ${risk ? `risk-${risk.toLowerCase()}` : ""}`}>
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
      <button
        type="button"
        className={`log-copy ${copied ? "copied" : ""}`}
        onClick={() => {
          if (!navigator.clipboard?.writeText) {
            return; // no clipboard in this context: leave everything as it is
          }
          void Promise.resolve(navigator.clipboard.writeText(entry.display))
            .then(() => setCopied(true))
            .catch(() => {});
        }}
      >
        <Icon name="copy" size={13} />
        <span className="btn-label">{copied ? "Copied" : "Copy"}</span>
      </button>
      {hint && (
        <p className="log-hint" title={entry.display}>
          {hint}
        </p>
      )}
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
 * transparency contract: a failed op always keeps its exact output). The filter
 * chips narrow the already-loaded entries locally; a filtered row keeps its own
 * output one click away, and nothing leaves the log.
 */
export function ActivityLog() {
  const { activities, running, runningDisplay, runningRisk } = useActivityStore();
  const [filter, setFilter] = useState<Filter>("all");

  const rows = activities.filter((entry) => {
    if (filter === "all") {
      return true;
    }
    if (filter === "failed") {
      return !entry.ok;
    }
    const hit = FILTERS.find((f) => f.key === filter);
    return hit && "risk" in hit ? riskOf(entry.display) === hit.risk : false;
  });

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
        <span className="log-filters" role="group" aria-label="Filter commands">
          {FILTERS.map((f) => (
            <button
              key={f.key}
              type="button"
              className="log-chip"
              aria-pressed={filter === f.key}
              onClick={() => setFilter(f.key)}
            >
              {f.label}
            </button>
          ))}
        </span>
      </div>
      {activities.length === 0 ? (
        <p className="activity-empty">{running ? "Running…" : "No commands run yet."}</p>
      ) : rows.length === 0 ? (
        <p className="activity-empty">No {filter} commands.</p>
      ) : (
        <ul className="log-rows">
          {rows.map((entry) => (
            <LogRow key={entry.id} entry={entry} />
          ))}
        </ul>
      )}
    </aside>
  );
}
