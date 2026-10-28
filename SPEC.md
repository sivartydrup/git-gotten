# GitGotten — Visual Git Manager

A cross-platform, Rust-based visual Git manager. Simple enough that a user does not
need to know Git, yet fully transparent — every operation shows the exact Git command it
runs. Built for multiple repositories and full worktree support, with an interactive
branching graph at its core. Inspired by GitKraken.

> Status: **in build** — stages 0–10 of 15 are complete; stage 11 (stash + submodules + bisect) is next. See
> [`README.md`](./README.md) for the current
> state and [`PLAN.md`](./PLAN.md) for the stage plan. This document is the source of
> truth for the design. Anything marked **(default — confirm)** was chosen by the author
> during spec drafting and is a reasoned default, not a decision from the design
> interview.

---

## 1. Decisions (from the design interview)

These were explicitly chosen and are the foundation the rest of the spec builds on.

| Dimension | Decision |
|---|---|
| App name | `GitGotten` (package: `git-gotten`) |
| Languages | Rust (core) + TypeScript (web UI) |
| GUI stack | **Tauri v2** + **React + TypeScript** (Vite) |
| Git engine | **Hybrid** — `gix` (gitoxide) for reads/graph, **git CLI** for writes |
| Platforms (v1) | **macOS + Windows** (Linux deferred, architecture kept cross-platform-ready) |
| Git operations | Full set: basics, merge, rebase (interactive), cherry-pick, stash, conflict resolution, worktrees, submodules, tags & remotes, bisect |
| Branching graph | **Lane-based interactive DAG** (canvas, zoom/pan, select, context actions) — the main focus |
| Command display | **Layered**: preview+confirm (risky ops) + passive activity log + inline hints |
| Repo navigation | **Full workspace**: groups, pinning, per-repo quick info, cross-repo status dashboard |
| Conflict resolution | **In-app 3-way editor** + external `$EDITOR` fallback |
| Staging | **Per-hunk** diff staging |
| Theming | **Light + Dark**, system-following + manual override |
| Frontend framework | React + TypeScript (state: Zustand) |
| Distribution | Manual installer + **built-in auto-updater** |
| In-app editor | In-app **CodeMirror** (syntax highlighting) + external `$EDITOR` hand-off |

---

## 2. Goals

- **Zero Git knowledge required.** Complete everyday Git tasks (commit, push, switch
  branches, merge, rebase, resolve conflicts, manage worktrees) without typing a command.
- **Total transparency.** The user always sees *exactly* which Git command is being run,
  and can preview + confirm before risky operations.
- **Multi-repo, worktree-native.** Repos are first-class, grouped, and switchable in a
  left nav; worktrees are a first-class concept alongside branches.
- **Graph is the heart.** A fast, readable, interactive commit graph (lane DAG) is the
  primary view and the reason to exist.

## 3. Non-goals (v1)

- **Not a full IDE.** In-app editing is quick-edit only (commit messages, staged/conflict
  hunks); real editing hands off to `$EDITOR`.
- **No remote-service integration.** No GitHub/GitLab PR, issue, or CI UI in v1. (Later
  milestone; the command layer is designed so it could be added.)
- **No plugin/extension system** in v1.
- **No Linux packaging/testing** in v1 (architecture stays portable; Linux is a later
  milestone).
- **No multi-user / team features.**
- **No simultaneous editing of multiple repos** in one working set (v1 delivers a
  cross-repo *status dashboard* + single-repo active working set; true parallel
  multi-repo working sets are a later milestone — see §10).

---

## 4. Guiding principles

1. **The command is the source of trust.** Every mutating action produces a concrete
   `CommandPlan` (the literal Git invocations). The UI is a thin, safe layer over those
   commands. If it can't be expressed as real Git commands, it isn't in the UI.
2. **Safe by default, explicit for danger.** Reads are quiet; recoverable writes preview
   briefly; destructive writes require deliberate confirmation and show their blast radius.
3. **The graph must feel instant.** Virtualized, worker-laid-out, 60fps at scale.
4. **Boring, robust internals.** Prefer the real `git` binary for writes (honors the
   user's config, hooks, editor, aliases) and `gix` for the fast, pure-Rust read model.
5. **Never a silent half-state.** Every operation either succeeds or fails with the exact
   command + output surfaced. Destructive/ambiguous states (mid-rebase, mid-merge,
   conflict) are always made visible and recoverable.

---

## 5. Technology stack

**Rust core (workspace):**
- Tauri v2 — shell, IPC, file access, process, auto-updater, windowing.
- `gix` (gitoxide) — pure-Rust read model: objects, refs, status, diffs, ahead/behind,
  worktrees, stashes, submodules.
- `tokio` — async runtime; per-repo operation queue (see §8 reliability).
- `serde` + `ts-rs` — shared Rust↔TS types, code-generated.
- `notify` (or Tauri fs watcher) — working-tree change watching → live status.
- `clap` — used internally to build the `git` argv vectors safely (no shell strings).

**Web UI:**
- React 18 + TypeScript + Vite.
- Zustand — client state.
- HTML **canvas** — the branching graph renderer (see §6).
- CodeMirror 6 — commit messages + manual hunk/conflict editing.
- CSS custom properties for theming tokens (§12).

**Build:** Rust workspace + `tauri build`; frontend bundled into the Tauri binary.

---

## 6. Application shell & layout

```
+---------------------------------------------------------------+
|  top bar: repo name ▾  worktree ▾  branch ▾   [search graph]  |
+----------+----------------------------------------------------+
|  left    |   main (per selected repo)                          |
|  nav:    |  +-----------------------------------------------+  |
|  groups/ |  |  graph / tabs: [Graph][Files][Branches][Log]  |  |
|  repos   |  |                                               |  |
|  (status |  |            branch graph (canvas)             |  |
|   badges)|  |                                               |  |
|          |  +-----------------------------------------------+  |
|          |  +-----------------------------------------------+  |
|          |  |  bottom: staging (per-hunk) + commit panel     |  |
|          |  +-----------------------------------------------+  |
+----------+----------------------------------------------------+
   activity log docked or a right drawer (toggleable)
```

- **Left nav** — the multi-repo workspace (§7). Collapsible.
- **Top bar** — current repo, current worktree, current branch, and a graph search box.
- **Main area** — tabbed: **Graph** (default, the focus), **Files** (staging/diff),
  **Branches**, **Log**. Plus the **staging + commit** docked panel.
- **Activity log** — docked bottom or a right drawer, always showing the command stream.
- **Inspect drawer** — right panel that opens on selecting a commit/branch: details,
  diffs, refs, actions.

Layout is responsive-ish but optimized for desktop (min ~1024px). Window is resizable;
panels are resizable with persisted sizes.

---

## 7. Multi-repo workspace (left navigation)

### Model
- A tree of **groups (folders, nestable)** → **repos**.
- Each **repo** node: name (folder basename), path, current branch, and live status
  badges: `N changes`, `↑ahead ↓behind`, `!` if in a conflict/merge/rebase state.
- **Pinning** — pinned repos stay at the top of their group.
- **Recents** — un-pinned repos ordered by last activity.
- **Search/filter** — filter repo list by name/path.

### Behaviors
- **Add repo:** open from an existing path, or **clone** (clone UI → §11 basics).
- **Remove from list:** unlinks the repo from the workspace only (never touches the on-disk
  repo). "Forget" vs. "remove worktree" are distinct actions.
- **Select repo** → switches the main area + top bar to that repo's active worktree.
- **Status is live** — file-watcher driven, debounced (~250ms), per selected repo; the
  nav badges update without a full reload.

### Cross-repo views (the "full workspace")
- **All-repos dashboard** (a top-level nav entry): a table of every repo — current
  branch, dirty count, ahead/behind, last commit, state — sortable/filterable; click a
  row to switch into it. This is the v1 cross-repo capability.
- **(Later milestone)** true simultaneous multi-repo working sets (edit/commit across
  several repos in one view). Out of scope for v1.

---

## 8. Git engine (hybrid)

Two backends, one facade.

### Read model — `gix`
- Build a per-repo **object model**: commits, refs (branches, tags, remotes, stash refs,
  worktree refs), index, working-tree status, diffs, ahead/behind, worktrees, submodules.
- Chosen for speed, purity (no C dep), and clean object access — ideal for the graph and
  for fast status.

### Write / command layer — `git` CLI
- Mutating operations **shell out to the user's `git` binary** (resolved from `PATH` /
  user config). Rationale: exact Git behavior, honors the user's `.gitconfig` (diff
  tools, editor, aliases, hooks, signing, LFS), and — critically — **the command we display
  is the real command**.
- Built as **argv vectors** (never interpolated shell strings) — no injection.
- Runs in a controlled environment; captures stdout/stderr/exit code.

### `CommandPlan` (the transparency primitive)
Every operation compiles to:
```
CommandPlan {
  steps: [ GitStep { argv: Vec<String>, summary: String, risk: Risk, effect: EffectSummary } ],
  display: String          // the human-readable command shown in UI
  preview?: Preview        // for risky ops: what will change, how many commits, etc.
}
```
- `Risk` = `{ ReadOnly, Mutating, Dangerous }`.
- `EffectSummary` = structured blast-radius (e.g., "rewrites 3 commits on `main`",
  "force-pushes `feature` — remote will lose 2 commits").
- The same `display` string drives the **inline hint**, the **preview dialog**, and the
  **activity log** entry — one source of truth.

### Reliability
- **Per-repo operation queue** — operations on a repo run through a serialized async queue
  (a lock) to prevent `.git/index.lock` and ref races. Reads are uncached-fresh but cheap.
- **Safe failure** — a failed step is logged with the exact command + output and rolls the
  UI state back to the pre-op snapshot; no silent partial state. Mid-merge/rebase states
  are surfaced as a recoverable banner.
- **Status refresh** — file-watcher → debounce → recompute status → push an event to the UI.

---

## 9. Command transparency (layered)

The differentiator: *you don't need to know Git, but you always see what Git is doing.*

Three layers, selected by `Risk`:

1. **Inline hints** — a subtle label/tooltip beside each action button showing the command
   it will run (e.g., `git push origin main`). Always on for simple actions.
2. **Passive activity log** — a persistent, scrollable stream. Each entry:
   `command` (mono) · duration · exit status · collapsible stdout/stderr · timestamp ·
   repo/operation. Click → full detail. Auto-scrolls; a badge shows recent errors.
3. **Preview + confirm** — for `Mutating`/`Dangerous` ops, a dialog shows the exact
   command(s) and the `EffectSummary` before running.
   - `Mutating` (commit, checkout, merge, branch create): preview + single confirm.
   - `Dangerous` (force-push, `reset --hard`, rebase of published commits, `filter`):
     preview + strong warning + **type-to-confirm** or a second explicit control.

Reads (`log`, `status`, graph) run silently but still appear in the activity log.

---

## 10. Feature specifications

### 10.1 Branching graph *(main focus)*

**Data:** `gix` walks the commit DAG → nodes (commits) + edges (commit→parent). Merge
commits have ≥2 parents and render as merge joins.

**Layout — lane assignment algorithm:**
1. Order commits topologically (newest first).
2. Assign each commit a **lane** (x) and a **row** (y). Walk parents: a commit takes the
   leftmost free lane among its parents' lanes, or a new left lane if it starts a new line.
3. Track **lane lifetimes** (a lane is "alive" from a commit's first appearance until the
   line merges/dies) so edges route without crossing a still-alive line.
4. **Color per branch/line**; a node's lane inherits the branch color of the line it sits
   on. This is the classic GitKraken/Git-Graph look.
5. Layout is computed in a **Web Worker** (or Rust, streamed) off the render thread, and
   **cached** per repo (invalidated on fetch/new commits).

**Rendering:** HTML canvas.
- Draw only the **visible window** of rows + a margin (virtualized) → smooth at scale.
- **Zoom** (wheel/pinch) and **pan** (drag); "follow HEAD" and "reset view" controls.
- Node = commit; label = subject (truncated) + author + relative date. Hover → tooltip
  (hash, author, date). Current branch's tip and HEAD emphasized; detached HEAD flagged.
- **Minimap** (optional, later) for orientation in large histories.

**Interaction:**
- **Click** node → select → **inspect drawer** (full message, author, date, stat, diffs,
  refs pointing here, tags).
- **Right-click / context menu** → actions: create branch, cherry-pick, reset to here,
  copy hash/message, view diff, bisect from here.
- **Select ranges** (shift/cmd) for multi-commit ops (cherry-pick, squash, diff A..B).
- **Search/filter** (top bar) dims non-matching commits (by message/author/date/branch).
- **Refs** render as colored labels pinned to their nodes (branches, tags, remote refs,
  worktree checkouts).

**Performance targets:** 10k-commit repo pans/zooms at ~60fps (virtualized); layout < 300ms
in the worker; open-to-interactive < 1s. (These are v1 targets to be verified in M4.)

### 10.2 Staging & commit *(per-hunk)*

- **Files panel** lists changed files with status letters (`M A D R U C`), grouped into
  **Staged** / **Changes (unstaged)** / **Untracked**.
- **Per-file diff** — unified diff with **per-hunk checkboxes** → stage selected hunks
  only. Buttons: stage file / unstage file / stage all / discard.
- Staging mechanism: build the staged-index patch from chosen hunks (`git apply --cached`)
  or pathspec; the **commit** runs `git commit` (with `-a`/pathspec as appropriate).
- **Commit panel** — CodeMirror message editor with conventional-commit hint, author +
  date fields, co-authors, **amend** toggle. Commit button → `CommandPlan` (preview if
  the commit rewrites history) → run → log.
- **Discard** is `Dangerous` (type-to-confirm), shows the diff being discarded.

### 10.3 Branches, remotes, tags

- **Branches** — list local + remote; current highlighted; per-branch: last commit,
  ahead/behind, tracked upstream. Actions: create (from HEAD or a selected commit),
  checkout/switch, delete, rename, push (create on remote), merge, rebase-onto,
  set upstream, view in graph.
- **Remotes** — list, add, remove, fetch, push, set default. Clone UI lives here too.
- **Tags** — list, create (annotated + lightweight), checkout, delete, push.
- All of the above render as refs in the graph.

### 10.4 Merge / rebase / cherry-pick

- **Merge** — pick source → preview (commits + files that will come in) → confirm → run →
  conflict flow (§10.5).
- **Rebase** — pick target → **interactive rebase list** (reorder / squash / edit /
  fixup / drop / pick) → run; on conflict, per-stop conflict flow, then continue.
  Rebasing **published** commits is `Dangerous` (type-to-confirm + warning).
- **Cherry-pick** — select commit(s) → target branch → run → conflict flow.
- All route through `CommandPlan` + preview + activity log + the recoverable-state banner.

### 10.5 Conflict resolution

- When an op stops on conflict, a **3-way editor** opens for each conflicted file:
  - **Base / Ours / Theirs** — side-by-side or combined (user preference).
  - **Per-conflict-hunk**: accept ours / accept theirs / accept both / **manual edit**.
  - Manual edit in CodeMirror (syntax-highlighted); the resolved file is written and
    staged.
- Then **continue** (`merge --continue` / `rebase --continue` / `cherry-pick --continue`)
  or **abort**. An **"open in $EDITOR"** button hands the raw file(s) to the external
  editor as an escape hatch for gnarly conflicts.
- Progress shows which of N conflicted files remain.

### 10.6 Stash

- **List** stashes (message, date, branch). Actions: **apply** (keep), **pop** (apply +
  drop), **drop**, **create** (optionally include untracked). Per-stash shows its files.

### 10.7 Worktrees *(fully supported — differentiator)*

- A repo's **worktrees** are first-class. The top bar shows a **worktree switcher** next
  to the branch switcher; selecting one retargets all operations + the graph to that
  worktree.
- **Actions:** add worktree (new branch, existing branch, or a specific commit → into a
  chosen path), remove worktree (with force if dirty), list. Each worktree shows its
  path + checked-out branch/commit.
- The **graph** shows, per worktree, which branch/commit is checked out where.
- "Remove worktree" is distinct from "remove repo from workspace" and from "delete
  branch." Removing a worktree whose branch is elsewhere-safe is `Mutating`; force-removing
  a dirty one is `Dangerous`.

### 10.8 Submodules

- List submodules with status (checked-out commit vs. recorded). Actions: **init**,
  **update** (to recorded / to HEAD / specific commit). Submodules appear in the Files
  panel with a badge; a submodule pointer change shows as a normal file change and commits
  like one. Deeper submodule flows (synchronizing, nested) are power-user / later.

### 10.9 Bisect

- Guided flow: pick **good** and **bad** commits (from graph/history or HEAD/known) →
  optionally add extra range refs → the app runs the `git bisect` steps.
- Each step shows the **commit under test** + a **good / bad** verdict button; an optional
  **test command** runs automatically per step and auto-verdicts on exit code.
- The **shrinking range is highlighted in the graph**; a bisect log is kept for replay /
  reset.

---

## 11. Data model & persistence

Stored in the OS app-config dir (macOS `~/Library/Application Support/GitGotten`,
Windows `%APPDATA%\GitGotten`).

- **app.json** (user-level): workspace tree (groups, repos, pinned, recents), last-open
  repo, theme preference, per-repo last view state, editor preference (`$EDITOR`), diff
  tool preference, updater settings, panel sizes.
- **per-repo state** (one file each): graph layout cache, branch color assignments,
  last selected commit/branch.
- **activity log** (per repo, bounded ring, e.g. last 500 entries): the command stream.

v1 uses **plain JSON files** (simple, inspectable, no DB). **SQLite** is the fallback if
the activity log or graph cache outgrows files at scale — a later optimization, not a
v1 requirement. All persistence is best-effort; a corrupt cache is rebuilt, never fatal.

---

## 12. Theming & design system

- **Light + Dark**, system-following by default + a manual toggle (persisted).
- **Tokens** as CSS custom properties: `--surface`, `--text`, `--muted`, `--border`,
  `--accent`, `--success`, `--warning`, `--error`, plus a **branch-lane palette**
  (a distinguishable, theme-aware color set for lanes/refs).
- **Aesthetic** — GitKraken-inspired: calm, spacious, low-contrast chrome so the graph
  and colors carry the meaning. System UI font for text; **monospace** for commands,
  hashes, and diffs.
- Destructive actions use the `--error` accent and consistent "danger" affordances
  (matches the command-transparency risk model).

---

## 13. IPC surface (Rust ⇄ web)

**Commands (web → Rust):** `open_repo`, `close_repo`, `load_graph(repo, worktree, filter)`,
`load_status`, `load_branches/remotes/tags/stashes/worktrees/submodules`,
`run_operation(plan)` (returns result + updated model), `stage_hunks`, `commit`,
`resolve_conflict(file, hunks)`, `continue/abort_op`, `bisect_step(verdict)`,
`config.get/set`, `resolve_git_path`.

**Events (Rust → web):** `status_changed`, `op_started/finished` (command, status, output),
`graph_updated`, `repo_state_changed` (e.g., entered/exited a merge or rebase).

All payloads use **shared types** (serde ↔ `ts-rs` codegen) — no hand-maintained
duplication between the two languages.

---

## 14. Rust crate workspace (proposed)

```
git-gotten/
  Cargo.toml                 # workspace
  crates/
    protocol/                # shared serde types (→ ts-rs)
    gitcore/                 # gix read model + git CLI executor + CommandPlan + op queue
    graph/                   # commit DAG + lane layout (pure, unit-testable)
    app/                     # Tauri app: commands, events, config, updater, fs watch
  src/                       # React + TS web UI
  ui/                        # (optional) Vite/React build config
```
`gitcore` is the heart; `graph` is pure logic (easily tested); `app` is the thin Tauri
shell. The web UI is the only "view" layer.

---

## 15. Testing strategy

- **`graph` (pure Rust):** unit tests on known DAGs — lane assignment correctness,
  merge joins, lane lifetimes, color/line mapping. The layout is the riskiest pure logic,
  so it's the most tested.
- **`gitcore` CommandPlan:** tests asserting **exact command strings** for every operation
  (transparency depends on this being right). Integration tests against a **temp git
  repo fixture** covering each op end-to-end, including conflict/rebase-stop paths.
- **Web:** component tests for staging (hunk selection), commit, and graph rendering
  sanity (a fixture DAG renders the expected lanes). 
- **Smoke/e2e (M4):** a sample multi-branch repo with worktrees exercised through the
  real Tauri app (Playwright) — open → commit → branch → merge (with a seeded conflict) →
  resolve → push.

---

## 16. Safety

- Reads are read-only; writes are gated by `Risk` (§9); **dangerous ops require
  type-to-confirm** and show blast radius.
- **Published-history protection:** detect rebase/force-push/reset that would rewrite
  commits already on a remote → strong warning (the #1 way to lose work).
- **Subprocess safety:** argv vectors only; controlled environment; no shell string
  interpolation.
- **Path safety:** only operate on repos the user explicitly opened (validated paths).
- **No destructive action without a preview** showing exactly what will be affected.

---

## 17. Distribution & packaging

- **Tauri v2 bundler** → `.dmg` (macOS) + `.msi` (Windows). (NSIS for Windows .exe
  optional.)
- **Code signing** — required for distribution: macOS Developer ID + **notarization**;
  Windows code-signing certificate. (Cert provisioning is a build-infra prerequisite.)
- **Auto-updater** — Tauri updater + a **version manifest** (can start as a statically
  hosted JSON). Checks on launch + periodically.
- **Minimum OS:** macOS 12+, Windows 10+.
- Linux: portable code, **no** packaging/testing in v1.

---

## 18. Non-functional requirements

- **Performance:** open-to-interactive < 1s; graph 60fps (virtualized) at 10k commits;
  status refresh debounced and live.
- **Reliability:** per-repo serialized op queue; safe failure + recoverable-state
  banner; corrupt caches rebuild, never crash.
- **Error UX:** parse common Git failures into friendly messages while always keeping the
  raw command + output visible.
- **Security:** see §16.

---

## 19. Milestones (phased delivery)

- **M0 — Spine.** Tauri+React+Vite shell; `gix` opens a repo; left nav (single repo);
  status; **activity log** + `CommandPlan`/preview working. *Proves the backbone:*
  repo → status → transparent commands.
- **M1 — The heart.** **Branching graph** (lane DAG, canvas, virtualized, select/context,
  refs) + **per-hunk staging** + commit + branch switch/create. *(Main focus + daily use.)*
- **M2 — Sync & integration.** fetch/push/pull, remotes, tags, **merge**, **in-app 3-way
  conflict resolution**, **rebase** (incl. interactive), cherry-pick.
- **M3 — Full operation set.** **worktrees**, stash, submodules, bisect, groups +
  cross-repo dashboard, Light+Dark polish, **auto-updater**.
- **M4 — Hardening & ship.** Perf (10k commits), reliability (op queue, error UX),
  **code signing + notarization + distribution**, full test suite (graph layout,
  CommandPlan strings, e2e smoke).

**Suggested order rationale:** get the spine and the *main focus* (graph) working early
and correct, then expand operations, then harden and package.

---

## 20. Spec-level defaults (author's choices — confirm or override)

These were **not** in the interview; I chose reasonable defaults. Flag any to change.

1. **Frontend state:** Zustand (lightweight). *(confirm)*
2. **Graph rendering:** HTML canvas + custom 2D lane renderer; layout in a Web Worker.
   *(a Rust-side layout streamed over IPC is the alternative.)*
3. **In-app editor:** CodeMirror 6 (lighter than Monaco, adequate for the quick-edit scope).
   Monaco is the alternative if you want LSP-grade editing. *(confirm)*
4. **Persistence:** plain JSON files for v1; SQLite only if it outgrows them. *(confirm)*
5. **Auto-update feed:** statically hosted version manifest. *(confirm)*
6. **Min OS:** macOS 12+, Windows 10+. *(confirm)*
7. **v1 cross-repo:** status dashboard + single-repo active working set (no simultaneous
   multi-repo editing). *(confirm the boundary.)*
8. **App icon / name lock-in:** `GitGotten`, a git-themed mark. *(art later.)*

---

## 21. Open questions / risks

- **Graph layout at scale** is the hardest pure-logic part — lane-lifetime routing in
  deep, heavily-merged histories. Mitigated by the worker + virtualization + a clear
  algorithm in `crates/graph` with strong tests. This is the top technical risk.
- **Worktree + graph interplay** — showing per-worktree checkout state in one graph is
  novel; needs a clear visual encoding (defined above, to be validated in M3).
- **Code signing / notarization** is a real-world prerequisite for distribution — needs
  certificates (infra) even though it's not a code task.
- **Conflict-editor complexity** (3-way, per-hunk, manual) is a lot of UI; scoped to a
  solid in-app editor + external fallback, not a full merge-IDE.
- **`gix` ↔ CLI parity for writes** — a few operations may behave slightly differently
  between the `gix` model and the CLI; the hybrid design keeps writes on the CLI to
  avoid this, but edge cases (e.g., partial-staging index surgery) need care.
