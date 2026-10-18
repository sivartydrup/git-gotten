import { useEffect, useState } from "react";
import { useActivityStore } from "../store";
import type { CommandPlan, Risk } from "../types";

const RISK_ORDER: Risk[] = ["ReadOnly", "Mutating", "Dangerous"];

const RISK_LABEL: Record<Risk, string> = {
  ReadOnly: "read-only",
  Mutating: "mutates the repo",
  Dangerous: "dangerous — may rewrite history",
};

/** A plan's effective risk is its maximum step risk (SPEC §8). */
export function planRisk(plan: CommandPlan): Risk {
  return plan.steps.reduce<Risk>(
    (max, s) => (RISK_ORDER.indexOf(s.risk) > RISK_ORDER.indexOf(max) ? s.risk : max),
    "ReadOnly",
  );
}

function RiskBadge({ risk }: { risk: Risk }) {
  return <span className={`badge risk risk-${risk.toLowerCase()}`}>{RISK_LABEL[risk]}</span>;
}

/**
 * The preview/confirm dialog (SPEC §9): before a `Mutating` or `Dangerous` op
 * runs, the user sees the exact command, its steps (with blast radius), the
 * pre-run effect summary, and the risk class — then confirms. `Dangerous`
 * plans additionally require typing a confirmation word (type-to-confirm).
 */
export function PreviewDialog() {
  const { preview, running, busy, error, confirmPreview, dismissPreview } = useActivityStore();
  const [confirmText, setConfirmText] = useState("");

  // The component stays mounted with App; clear the typed confirmation
  // whenever a different plan is presented (or the dialog closes).
  useEffect(() => {
    setConfirmText("");
  }, [preview]);
  if (!preview) {
    return null;
  }

  const plan: CommandPlan = preview.plan;
  const risk = planRisk(plan);
  const typeToConfirm = plan.preview?.requires_type_to_confirm ?? risk === "Dangerous";
  const confirmed = !typeToConfirm || confirmText === "confirm";
  const disabled = running || busy || !confirmed;

  return (
    <div
      className="dialog-overlay"
      role="presentation"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget && !running && !busy) {
          dismissPreview();
        }
      }}
    >
      <div className="dialog" role="dialog" aria-modal="true" aria-label="Confirm command">
        <div className="dialog-head">
          <h2>Confirm command</h2>
          <RiskBadge risk={risk} />
        </div>

        <code className="dialog-cmd">{plan.display}</code>

        {plan.steps.length > 1 && (
          <ol className="dialog-steps">
            {plan.steps.map((step) => (
              <li key={step.argv.join(" ")}>
                <code>
                  {step.argv[0]} {step.argv.slice(1).join(" ")}
                </code>{" "}
                <span className="step-summary">{step.summary}</span>
                {step.effect && <span className="step-effect">{step.effect}</span>}
              </li>
            ))}
          </ol>
        )}

        {plan.steps.length === 1 && (
          <p className="step-summary">{plan.steps[0].summary}</p>
        )}
        {plan.steps[0]?.effect && (
          <p className="step-effect">{plan.steps[0].effect}</p>
        )}

        {plan.preview ? (
          <>
            <p className="preview-headline">{plan.preview.headline}</p>
            {plan.preview.lines.length > 0 && (
              <ul className="preview-lines">
                {plan.preview.lines.map((line) => (
                  <li key={line}>{line}</li>
                ))}
              </ul>
            )}
          </>
        ) : (
          <p className="dialog-nopreview">No pre-run output for this command.</p>
        )}

        {typeToConfirm && (
          <label className="type-confirm">
            <span className="warning">
              This is a destructive operation. Type <code>confirm</code> to enable the button.
            </span>
            <input
              value={confirmText}
              onChange={(e) => setConfirmText(e.target.value)}
              placeholder="confirm"
              autoFocus
            />
          </label>
        )}

        {error && <p className="error">{error}</p>}

        <div className="dialog-actions">
          <button
            type="button"
            className="btn"
            onClick={() => {
              setConfirmText("");
              dismissPreview();
            }}
            disabled={running || busy}
          >
            Cancel
          </button>
          <button
            type="button"
            className={`btn primary risk-${risk.toLowerCase()}`}
            onClick={() => {
              setConfirmText("");
              void confirmPreview();
            }}
            disabled={disabled}
          >
            {running ? "Running…" : `Run ${plan.display}`}
          </button>
        </div>
      </div>
    </div>
  );
}
