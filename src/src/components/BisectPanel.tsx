import { useEffect, useState } from "react";
import { bisectTest } from "../api";
import {
  useActivityStore,
  useBisectStore,
  useMainTab,
  useRepoStore,
} from "../store";
import type { Op } from "../types";

/**
 * Bisect tab (SPEC §10.9). Inactive: pick a known-good and a known-bad ref
 * (plus optional extra refs) and start — the bad input is prefilled from the
 * graph's "bisect from here" pick. Active: the candidate under test, how many
 * candidates are left, the good/bad sets, the found culprit, and the controls
 * — a test command (run through `bisect_test`, which posts the good/bad
 * verdict from its exit code), manual Good/Bad verdicts and Reset (all through
 * the preview flow), and the raw `git bisect log`.
 */
export function BisectPanel() {
  const { bisect, loading, error, draftBad, setDraftBad, testCommand, setTestCommand } =
    useBisectStore();
  const activeId = useRepoStore((s) => s.activeId);
  const { requestPreview, running } = useActivityStore();
  const [goodRef, setGoodRef] = useState("");
  const [badRef, setBadRef] = useState("");
  const [extraRefs, setExtraRefs] = useState("");
  const [testing, setTesting] = useState(false);

  // The graph menu seeds the bad side of the next bisect.
  useEffect(() => {
    if (draftBad) {
      setBadRef(draftBad);
      setDraftBad(null);
    }
  }, [draftBad, setDraftBad]);

  const act = (op: Op) => {
    if (activeId) {
      void requestPreview(activeId, op);
    }
  };

  const runTest = async () => {
    if (!activeId || !testCommand.trim() || testing) {
      return;
    }
    setTesting(true);
    try {
      const state = await bisectTest(activeId, testCommand.trim());
      if (state) {
        useBisectStore.setState({ bisect: state });
      }
    } finally {
      setTesting(false);
    }
  };

  if (error) {
    return (
      <div className="refs-panel">
        <div className="refs-state">Failed to load bisect state: {error}</div>
      </div>
    );
  }

  if (!bisect.active) {
    return (
      <div className="refs-panel">
        <div className="refs-head">
          <h3>Bisect</h3>
        </div>
        {loading ? (
          <div className="refs-state">Loading…</div>
        ) : (
          <>
            <p className="refs-state">
              No bisect in progress. Pick a ref where history was good and one
              where it was bad.
            </p>
            <input
              className="dialog-input"
              placeholder="good ref (branch or commit)"
              value={goodRef}
              onChange={(e) => setGoodRef(e.target.value)}
            />
            <input
              className="dialog-input"
              placeholder="bad ref (branch or commit)"
              value={badRef}
              onChange={(e) => setBadRef(e.target.value)}
            />
            <input
              className="dialog-input"
              placeholder="extra refs (optional, space-separated)"
              value={extraRefs}
              onChange={(e) => setExtraRefs(e.target.value)}
            />
            <div className="dialog-actions">
              <button
                type="button"
                className="btn primary"
                disabled={running || !goodRef.trim() || !badRef.trim()}
                onClick={() =>
                  act({
                    kind: "bisect_start",
                    good: goodRef.trim(),
                    bad: badRef.trim(),
                    extra: extraRefs.trim() ? extraRefs.trim().split(/\s+/) : [],
                  })
                }
              >
                Start bisect
              </button>
            </div>
          </>
        )}
      </div>
    );
  }

  return (
    <div className="refs-panel">
      <div className="refs-head">
        <h3>Bisect</h3>
        <button
          type="button"
          className="btn row-btn"
          disabled={running}
          onClick={() => act({ kind: "bisect_reset" })}
        >
          Reset
        </button>
      </div>
      {bisect.found && (
        <div className="bisect-found">First bad commit: <code>{bisect.found}</code></div>
      )}
      <div className="bisect-state">
        <span>
          Testing <code className="refs-name">{bisect.current ?? "—"}</code>
        </span>
        <span className="refs-msg">{bisect.remaining} candidates left</span>
      </div>
      <div className="bisect-sets">
        <span className="bisect-good">good: {bisect.good.join(", ") || "—"}</span>
        <span className="bisect-bad">bad: {bisect.bad.join(", ") || "—"}</span>
      </div>
      <div className="bisect-controls">
        <input
          className="dialog-input"
          placeholder="test command (exit 0 = good)"
          value={testCommand}
          onChange={(e) => setTestCommand(e.target.value)}
        />
        <div className="dialog-actions">
          <button
            type="button"
            className="btn"
            disabled={running || testing || !testCommand.trim()}
            onClick={() => void runTest()}
          >
            Run test
          </button>
          <button
            type="button"
            className="btn"
            disabled={running}
            onClick={() => act({ kind: "bisect_verdict", verdict: "Good" })}
          >
            Good
          </button>
          <button
            type="button"
            className="btn"
            disabled={running}
            onClick={() => act({ kind: "bisect_verdict", verdict: "Bad" })}
          >
            Bad
          </button>
        </div>
      </div>
      <pre className="bisect-log">{bisect.log.join("\n")}</pre>
      <button
        type="button"
        className="btn row-btn"
        disabled={running}
        onClick={() => useMainTab.getState().setTab("graph")}
      >
        Back to graph
      </button>
    </div>
  );
}
