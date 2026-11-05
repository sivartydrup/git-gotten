import { Risk } from "../types";

/**
 * Risk class of a logged command, inferred from its argv.
 *
 * `LogEntry` carries no risk field — only `op_started` does, and that is live
 * state for the command currently running; entries re-read from the Rust ring
 * arrive without their plans. The verb is the signal available at render time,
 * and it mirrors `gitcore::plan`: `gitcore::run` only executes plans it planned
 * itself, so a logged verb is the verb the core classified. An unrecognised
 * verb (or a line that is not a git command) stays unclassified rather than
 * guessed.
 */
const READ_ONLY: Record<string, true> = {
  status: true,
  log: true,
  show: true,
  diff: true,
  blame: true,
  "ls-files": true,
  "rev-parse": true,
  config: true,
  help: true,
  describe: true,
};

const MUTATING: Record<string, true> = {
  add: true,
  commit: true,
  checkout: true,
  switch: true,
  merge: true,
  pull: true,
  fetch: true,
  push: true,
  "cherry-pick": true,
  apply: true,
  rm: true,
  mv: true,
  tag: true,
  branch: true,
  remote: true,
  worktree: true,
  stash: true,
  submodule: true,
  bisect: true,
  rebase: true,
  reset: true,
  restore: true,
};

/**
 * The flags that make the core raise these verbs to `Dangerous` (SPEC §8 risk
 * ordering): a force-push, a hard reset, deleting a dirty worktree, aborting an
 * in-progress operation, or deleting every stash. A bare verb stays `Mutating`.
 */
function isDangerous(verb: string, argv: string[]): boolean {
  const flag = (name: string) => argv.includes(name);
  if (verb === "push" || verb === "worktree") {
    return flag("--force") || flag("-f");
  }
  if (verb === "reset") {
    return flag("--hard");
  }
  if (verb === "rebase" || verb === "merge" || verb === "cherry-pick") {
    return flag("--abort");
  }
  if (verb === "stash") {
    return flag("clear");
  }
  return verb === "clean";
}

export function riskOf(display: string): Risk | null {
  const argv = display.trim().split(/\s+/);
  if (argv[0] !== "git") {
    return null;
  }
  const verb = argv[1] ?? "";
  if (READ_ONLY[verb]) {
    return "ReadOnly";
  }
  if (isDangerous(verb, argv)) {
    return "Dangerous";
  }
  return MUTATING[verb] ? "Mutating" : null;
}
