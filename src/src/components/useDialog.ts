import { useCallback, useEffect, useRef } from "react";

/**
 * Focus management for the app's dialogs (SPEC §10, accessibility).
 *
 * Attach the returned ref to the element carrying `role="dialog" aria-modal="true"`
 * (a non-modal banner may use it too - see below).
 * While the dialog is open:
 *   • focus moves inside it (first focusable control, else the primary action),
 *   • Tab / Shift+Tab stay inside it,
 *   • Escape closes it — unless an op is running (`opts.blocked`),
 *   • the Escape/Tab keys never reach the shell's global shortcut handler.
 * On close, focus goes back to the element that had it before the dialog opened.
 *
 * Only elements that declare themselves modal (`aria-modal="true"`) get focus
 * moved into them and a Tab trap; a non-modal banner (the update prompt) just
 * gets its own Escape/restore, so it never interrupts what the user is doing.
 *
 * `blocked` is deliberately a live value read from the store at key-press time
 * rather than a re-registered listener, so a preview dialog mid-run can never
 * be dismissed with the keyboard.
 */
export interface DialogOptions {
  /** `true` while an op is running: Escape must not close the dialog. */
  blocked?: () => boolean;
  /** `false` when something else owns Escape (the palette: App.tsx closes it). */
  escape?: boolean;
}

const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

function focusables(el: HTMLElement): HTMLElement[] {
  return Array.from(el.querySelectorAll<HTMLElement>(FOCUSABLE)).filter(
    // `[hidden]` and `display:none` are both invisible; jsdom only knows the
    // former, so an explicit check keeps the trap honest in the webview too.
    (node) => !node.hasAttribute("hidden"),
  );
}

export function useDialog(
  open: boolean,
  close: () => void,
  opts?: DialogOptions,
): (el: HTMLElement | null) => void {
  const elRef = useRef<HTMLElement | null>(null);
  const restoreRef = useRef<HTMLElement | null>(null);
  const blockedRef = useRef(opts?.blocked);
  blockedRef.current = opts?.blocked;
  // `close` is usually an inline arrow, so it is read through a ref: keying the
  // effect on it would re-run (and re-focus) on every keystroke.
  const closeRef = useRef(close);
  closeRef.current = close;
  const optsEscape = opts?.escape;

  const setRef = useCallback((el: HTMLElement | null) => {
    elRef.current = el;
  }, []);

  useEffect(() => {
    if (!open) {
      return;
    }
    const el = elRef.current;
    if (!el) {
      return;
    }
    const modal = el.getAttribute("aria-modal") === "true";
    // Only the first dialog in a chain claims the trigger: a preview that opens
    // while another dialog is still mounted must not steal (and then restore
    // to) the wrong element.
    if (!restoreRef.current) {
      const active = document.activeElement;
      restoreRef.current =
        active instanceof HTMLElement && !active.closest('[role="dialog"]')
          ? active
          : null;
    }

    if (modal) {
      const inside = focusables(el);
      const target =
        el.querySelector<HTMLElement>("[data-dialog-primary]") ?? inside[0] ?? el;
      target.focus();
    }

    const onKeyDown = (e: KeyboardEvent) => {
      if (!elRef.current) {
        return;
      }
      const target = e.target as HTMLElement | null;
      const inThisDialog = !!target && elRef.current.contains(target);
      // Focus sat outside the dialog (a key fired at the document, or a stray
      // click): a modal dialog still owns Escape.
      const outside = modal && !!target && !target.closest('[role="dialog"]');
      if (!inThisDialog && !outside) {
        return;
      }
      if (e.key === "Escape") {
        if (optsEscape === false) {
          return;
        }
        if (blockedRef.current?.()) {
          return;
        }
        // The shell's global Escape (drawer → context menu → palette) must not
        // also fire for this keystroke.
        e.stopPropagation();
        e.preventDefault();
        closeRef.current();
        return;
      }
      if (e.key !== "Tab" || !modal) {
        return;
      }
      const nodes = focusables(elRef.current);
      if (nodes.length === 0) {
        e.preventDefault();
        elRef.current.focus();
        return;
      }
      const current = document.activeElement;
      const index = nodes.indexOf(current as HTMLElement);
      const next = e.shiftKey
        ? index <= 0 ? nodes[nodes.length - 1] : nodes[index - 1]
        : index === nodes.length - 1 || index === -1 ? nodes[0] : nodes[index + 1];
      e.preventDefault();
      next.focus();
    };

    document.addEventListener("keydown", onKeyDown, true);
    return () => {
      document.removeEventListener("keydown", onKeyDown, true);
      const restore = restoreRef.current;
      restoreRef.current = null;
      if (restore && document.contains(restore)) {
        restore.focus();
      }
    };
  }, [open]);

  return setRef;
}
