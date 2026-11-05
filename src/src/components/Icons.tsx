/**
 * The app's icon set (stage 15).
 *
 * Every icon is an inline 24x24 stroke glyph drawn with `currentColor`, so it inherits the
 * colour of whatever control hosts it and needs no theme handling. Icons are decorative by
 * default (`aria-hidden`); pass `label` when the icon *is* the control, and it becomes an
 * accessible image with that name.
 */
export type IconName =
  | "graph"
  | "files"
  | "diff"
  | "conflict"
  | "branch"
  | "commit"
  | "merge"
  | "rebase"
  | "fetch"
  | "pull"
  | "pullRebase"
  | "push"
  | "worktree"
  | "stash"
  | "bisect"
  | "search"
  | "close"
  | "star"
  | "pin"
  | "chevronDown"
  | "chevronRight"
  | "chevronUp"
  | "plus"
  | "minus"
  | "copy"
  | "trash"
  | "edit"
  | "warning"
  | "danger"
  | "check"
  | "cross"
  | "clock"
  | "repo"
  | "group"
  | "dashboard"
  | "command"
  | "filter"
  | "sparkle";

const PATHS: Record<IconName, JSX.Element> = {
  graph: (
    <>
      <circle cx="6" cy="6" r="2.2" />
      <circle cx="6" cy="18" r="2.2" />
      <circle cx="18" cy="12" r="2.2" />
      <path d="M6 8.2v7.6M8 6h4a4 4 0 0 1 4 4v0M8 18h4a4 4 0 0 0 4-4" />
    </>
  ),
  files: (
    <>
      <path d="M6 3.8h7l4 4V20a1.2 1.2 0 0 1-1.2 1.2H6A1.2 1.2 0 0 1 4.8 20V5A1.2 1.2 0 0 1 6 3.8Z" />
      <path d="M13 3.8V8h4" />
    </>
  ),
  diff: (
    <>
      <path d="M7 4v11M3.5 7.5h7" />
      <path d="M14 14h7M17.5 10.5v11" />
      <path d="M4 20h6" />
    </>
  ),
  conflict: (
    <>
      <path d="M4 12h4l2-5 3 10 2-5h5" />
    </>
  ),
  branch: (
    <>
      <circle cx="7" cy="5.5" r="2.2" />
      <circle cx="7" cy="18.5" r="2.2" />
      <circle cx="17" cy="8.5" r="2.2" />
      <path d="M7 7.7v8.6M17 10.7c0 4-4 4.2-7 4.4" />
    </>
  ),
  commit: (
    <>
      <circle cx="12" cy="12" r="3.4" />
      <path d="M3 12h5.6M15.4 12H21" />
    </>
  ),
  merge: (
    <>
      <circle cx="7" cy="6" r="2.2" />
      <circle cx="7" cy="18" r="2.2" />
      <circle cx="17.5" cy="12" r="2.2" />
      <path d="M7 8.2v7.6M9 6.6c5 .6 6.4 2.6 6.4 5.4M9 17.4c5-.6 6.4-2.6 6.4-5.4" />
    </>
  ),
  rebase: (
    <>
      <path d="M4 8h9M4 16h9" />
      <path d="M10.5 5 13.5 8l-3 3M10.5 13l3 3-3 3" />
      <path d="M17 5v14" />
    </>
  ),
  fetch: (
    <>
      <path d="M12 3.5v11" />
      <path d="M8 11l4 4 4-4" />
      <path d="M4.5 19.5h15" />
    </>
  ),
  pull: (
    <>
      <path d="M12 4v12" />
      <path d="M7.5 12 12 16.5 16.5 12" />
      <path d="M6 20h12" />
    </>
  ),
  pullRebase: (
    <>
      <path d="M12 4v11" />
      <path d="M8 11.5 12 15.5l4-4" />
      <path d="M5 19.5h6M15 19.5h4" />
      <path d="M9.5 17 12 19.5 9.5 22" />
    </>
  ),
  push: (
    <>
      <path d="M12 20V8" />
      <path d="M7.5 12 12 7.5 16.5 12" />
      <path d="M6 4h12" />
    </>
  ),
  worktree: (
    <>
      <path d="M4.5 8.5h6v6h-6z" />
      <path d="M13.5 8.5h6v6h-6z" />
      <path d="M10.5 11.5h3" />
    </>
  ),
  stash: (
    <>
      <path d="M4.5 8.5h15l-1.5 11a1.4 1.4 0 0 1-1.4 1.2H7.4A1.4 1.4 0 0 1 6 19.5Z" />
      <path d="M8 8.5 9.5 4.5h5L16 8.5" />
    </>
  ),
  bisect: (
    <>
      <path d="M3.5 12h17" />
      <path d="M8 8.5v7M16 8.5v7" />
      <circle cx="12" cy="12" r="2" />
    </>
  ),
  search: (
    <>
      <circle cx="10.5" cy="10.5" r="5.5" />
      <path d="M14.8 14.8 19 19" />
    </>
  ),
  close: <path d="M6 6l12 12M18 6 6 18" />,
  star: <path d="m12 4 2.4 5 5.6.8-4 4 1 5.6-5-2.7-5 2.7 1-5.6-4-4 5.6-.8Z" />,
  pin: (
    <>
      <path d="M12 3.5v17" />
      <path d="M8 6.5h8l-1 6h-6Z" />
    </>
  ),
  chevronDown: <path d="m6 9.5 6 6 6-6" />,
  chevronRight: <path d="m9.5 6 6 6-6 6" />,
  chevronUp: <path d="m6 14.5 6-6 6 6" />,
  plus: <path d="M12 5v14M5 12h14" />,
  minus: <path d="M5 12h14" />,
  copy: (
    <>
      <path d="M9 9h9.5a1.2 1.2 0 0 1 1.2 1.2v9.1A1.2 1.2 0 0 1 18.5 20.5H9A1.2 1.2 0 0 1 7.8 19.3V10.2A1.2 1.2 0 0 1 9 9Z" />
      <path d="M15.5 5.5H6A1.2 1.2 0 0 0 4.8 6.7v9.8" />
    </>
  ),
  trash: (
    <>
      <path d="M4.5 7.5h15" />
      <path d="M9 7.5V5h6v2.5" />
      <path d="M6.5 7.5 7.5 20h9l1-12.5" />
      <path d="M10.5 11v5.5M13.5 11v5.5" />
    </>
  ),
  edit: (
    <>
      <path d="M4.5 19.5h4L20 8a2.1 2.1 0 0 0-3-3L5.5 16.5Z" />
      <path d="m15.5 6.5 2 2" />
    </>
  ),
  warning: (
    <>
      <path d="M12 4.5 21 20H3Z" />
      <path d="M12 10v4.5" />
      <circle cx="12" cy="17" r="0.6" />
    </>
  ),
  danger: (
    <>
      <circle cx="12" cy="12" r="8" />
      <path d="M12 8v4.5M8.8 15.2h6.4" />
    </>
  ),
  check: <path d="m5 12.5 4.5 4.5L19 7" />,
  cross: <path d="M6.5 6.5l11 11M17.5 6.5l-11 11" />,
  clock: (
    <>
      <circle cx="12" cy="12" r="8" />
      <path d="M12 7.5V12l3 2.2" />
    </>
  ),
  repo: (
    <>
      <path d="M5.5 4.5h13v15h-13z" />
      <path d="M9 4.5v15M5.5 9h3.5" />
    </>
  ),
  group: (
    <>
      <path d="M4 8.5h6.5l1.5 2H20v9H4Z" />
      <path d="M4 8.5V6h5l1.5 2.5" />
    </>
  ),
  dashboard: (
    <>
      <path d="M4 5.5h7v13H4zM13 5.5h7v6h-7z" />
    </>
  ),
  command: (
    <>
      <path d="M8.5 4.5a2 2 0 1 1 2 2v11a2 2 0 1 1-2-2h7a2 2 0 1 1-2 2v-11a2 2 0 1 1 2 2h-7Z" />
    </>
  ),
  filter: <path d="M4.5 6h15l-5.5 6.5V19l-4 1.5v-8Z" />,
  sparkle: (
    <>
      <path d="M12 4.5 13.6 9 18 10.6 13.6 12.2 12 16.7 10.4 12.2 6 10.6 10.4 9Z" />
      <path d="M18 16.5l.8 2 2 .8-2 .8-.8 2-.8-2-2-.8 2-.8Z" />
    </>
  ),
};

export function Icon({
  name,
  size = 16,
  label,
  className,
}: {
  name: IconName;
  size?: number;
  /** Set when the icon is the control: it becomes an accessible image with this name. */
  label?: string;
  className?: string;
}) {
  const common = {
    viewBox: "0 0 24 24",
    width: size,
    height: size,
    fill: "none",
    stroke: "currentColor",
    strokeWidth: 1.7,
    strokeLinecap: "round" as const,
    strokeLinejoin: "round" as const,
    className,
    "aria-hidden": label ? undefined : true,
    role: label ? ("img" as const) : undefined,
    "aria-label": label,
  };
  return <svg {...common}>{PATHS[name]}</svg>;
}
