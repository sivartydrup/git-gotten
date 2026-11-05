# GitGotten — Verification Log

Dated verification records for each platform gate. Every entry is what was actually run on that
machine on that date; the README carries the summary. Newest entries last.

---

## macOS 25.6, arm64 — 2026-10-29

- `cargo test --workspace` — 304 passed.
- `pnpm test` — 109 passed (11 files).
- `pnpm build` — TypeScript + Vite production build passes.
- Stage-09 flows (reorder / squash / reword / fixup / drop, `edit` pause + Continue,
  mid-rebase conflict → resolve → complete, cherry-pick onto a branch (clean +
  conflicting), published-history Danger gate) run against real `git` in fixture tests.
- Stage-10 flows (list main + linked + detached, add for an existing branch and with a
  new branch, double-checkout refused by git as a clean error, clean remove, dirty
  remove refused without force, force-remove as Dangerous type-to-confirm, switching
  retargets status/graph, per-worktree graph markers) run against real `git` in
  fixture tests.

## macOS 25.6, arm64 — 2026-11-02

- `cargo test --workspace` — 349 passed (includes the new workspace-tree tests:
  `app.json` round-trip across simulated restarts, corrupt-file recovery, group
  ops, pin/remove persistence, reopen-on-startup).
- `pnpm test` — 165 passed (16 files).
- `pnpm build` — TypeScript + Vite production build passes.
- `tauri build` — `GitGotten.app` bundle produced.
- Stage-12 flows driven headlessly in Chromium against the production bundle:
  nested groups with pinned-first ordering, `+ group` add, group delete hoisting
  children, pin reorder, search filter, remove-from-workspace, the
  All-Repositories dashboard (live branch/state/ahead-behind/dirty/last-commit
  per repo, row click switches the active repo), Light/Dark recoloring the chrome
  and swapping the canvas lane palette, and theme persistence across reload.
- Windows gate was not run at that time; later Windows verification is recorded below.
  The observed workspace path is `%APPDATA%\com.sivartydrup.gitgotten\app.json`.

## macOS 25.6, arm64 — 2026-11-04 (stage 13)

- `cargo test --workspace` — 368 passed, in both debug **and** release profiles, no warnings.
- `pnpm test` — 189 passed (17 files). `pnpm build` — production build passes.
- Stage-13 performance on a 10 120-commit synthetic repository (`--release`): the app's
  graph pipeline builds DAG + lane layout cold in 116–121 ms (SPEC §18 budget 300 ms),
  replays it from the per-repo cache in 3 ms, and rebuilds it in ~90 ms after one commit
  moves the fingerprint. Layout threshold asserted in both profiles.
- Canvas culling at 10 000 graph rows: the visible window stays under 100 rows at every
  pan position and at both zoom bounds, a full `renderGraph` pass issues under 2 000
  canvas calls and finishes in well under 500 ms — a culling regression fails the test,
  it does not merely slow down. The subjective 60 fps check was not done by hand.
- Stage-13 concurrency, against real `git`: 24 concurrent mutating ops (commits, branch
  and tag creates, stash pushes) on one repository all land with exactly the implied
  history and zero lock diagnostics; 8 good ops survive 9 colliding neighbours that all
  fail with non-zero exits; 12 ops fired from a main checkout *and* a linked worktree
  serialize on one turn (same queue key = the common git dir); ops on two repositories
  measurably overlap; `run_plans` keeps a compound flow atomic against 12 interleaving
  `git reset -q` attempts.
- Stage-13 failure paths, against real `git`: a failing middle step stops the rest of its
  plan and carries git's own stderr verbatim; an interrupted mid-merge stays a
  recoverable merge with `git merge --continue` refusing until the conflict is really
  resolved; a push to a dead remote fails cleanly with no remote-tracking ref written;
  a commit message full of shell metacharacters (plus a file named `-rf`) never reaches a
  shell. Friendly git-error text renders *above* the raw command + output, and every
  panel re-reads live state when an op finishes.
- Per-repo activity log: bounded 500-entry ring mirrored to one JSON file per repository,
  restored on restart with monotonic ids, rebuilt from a deliberately corrupted file;
  `Backend` restart recovers workspace and log.
- Windows gate: not run — no Windows machine available. The new paths are plain `std::fs`
  over the OS-aware app-data dir already covered by the persistence round-trip tests.

## macOS 25.6, arm64 — 2026-11-04 (stage 14)

- `cargo test` — 375 passed, including 7 new updater integration tests in
  `crates/app/tests/updater.rs`. `pnpm test` — 198 passed (18 files). `pnpm build` —
  TypeScript + Vite production build passes.
- Update decisions are exercised against a **real local HTTP feed**, not a mock: a feed
  announcing 9.9.9 yields an update whose version, notes and download URL are the entry
  for *this* os-arch; a feed announcing the running version (or an older one) yields none;
  a feed with no endpoints fails loudly instead of silently; a manifest signed with a
  placeholder signature is rejected against the pinned pubkey. The published manifest
  template (`updater/latest.json`) is parsed by the plugin's own schema type in a test.
- `tauri build` with no signing environment set: exit 0, `GitGotten.app` (7.65 MiB) and
  `GitGotten_0.1.0_aarch64.dmg` (3.93 MiB) under `target/release/bundle`.
- The `.dmg` was mounted, `GitGotten.app` copied out and launched: the window renders, and
  the launch-time feed check fired (two `GET /latest.json` hits in the mock feed's access
  log, 8 s apart) and the prompt appeared on screen — "Update available: **9.9.9**" with
  the manifest's release notes and Install / Dismiss buttons.
- Not verified at that time: the Install path end to end (download → signature check →
  install → relaunch) needs a real signed artifact. Windows MSI verification is below.

## macOS 25.6, arm64 — 2026-11-06 (stage 15)

- `cargo test --workspace` — 399 passed (1 ignored), including 9 new tests in
  `crates/gitcore/tests/commit_stats.rs`. `pnpm test` — 282 passed (23 files); `tsc --noEmit`
  clean. `pnpm build` — TypeScript + Vite production build passes.
- The per-commit diffstat is pinned against the real `git` CLI: `git show -M --numstat` is the
  expected value for every fixture commit except the rename, which is the documented divergence
  (this read does no rename detection, so the rename reports 4 added / 2 removed where git reports
  0/0 for the moved file). A merge counts against its first parent — 5 added / 0 removed for the
  fixture, where `git show` prints nothing at all. A file whose blob carries a NUL in its first
  8000 bytes contributes 0/0, as git's `-` columns do; a mode-only commit contributes 0/0; 260
  requested hashes are truncated to the 200 cap, keeping request order; the memo answers a repeat
  request without re-diffing and is refilled by a new commit.
- `pnpm tauri:build` with no signing environment: exit 0, `GitGotten.app` (7.73 MiB) and
  `GitGotten_0.1.0_aarch64.dmg` (3.98 MiB) under `target/release/bundle`.
- Headless visual pass on the production bundle driven against the stage fixtures
  (`cargo test -p git-gotten-app --test docs_fixtures -- --ignored`) at 1720×1000, 1440×900 and
  900×560: graph canvas 467 / 349 / 107 px tall with the `.graph` floor holding 140 px at short
  viewports, the repository rail collapsing to 96 px, the inspect drawer becoming an overlay below
  1120 px, and no console errors. `docs/assets/graph.webp`, `staging.webp`, `palette.webp` and
  `app-dark.webp` were re-shot from that bundle.
- That pass found a lane-legend bug: in a repository where `main` has diverged from `origin/main`,
  both chips read `origin/main`, because a lane took the name of the first *named* line routed
  through it. A lane now takes the name of the line whose first segment is drawn in it
  (`graphLegend: a lane takes the name of the line at home there, not of a visitor`).
- The docs fixture generator now also dumps `load_commit_stats`, so the screenshot harness renders
  the diffstat column with real Rust numbers instead of falling back to an unhandled command.

## macOS 25.6, arm64 — 2026-11-06 (repository folder picker)

- The nav's open affordance gained `tauri-plugin-dialog`'s native folder picker: **Browse** fills
  the path field — starting the dialog in the active repository's parent folder — and the user still
  confirms with Open, so the exact path about to be opened stays visible and editable. Cancelling
  leaves the field and any open error exactly as they were; outside Tauri the button is absent and a
  typed path is the only route. `dialog:default` on the main window is the only new permission.
- `pnpm test` — 287 passed (24 files), five of them new in `src/src/folder-picker.test.tsx`;
  `tsc --noEmit` clean; `cargo test --workspace` — 399 passed (1 ignored) with the plugin registered.
- Headless pass on the production bundle at 1720×1000 and 900×560: the Browse button measures 28×29
  px (icon-only, which is why the typed field keeps 138 px at 1720 and the full 83 px in the 96 px
  rail), zero console errors, and `plugin:dialog|open` reaching the harness shim instead of raising
  an unhandled command. `graph.webp`, `staging.webp`, `palette.webp`, `dashboard.webp`,
  `update.webp`, `workspace.webp` and `app-dark.webp` under `docs/assets/` were re-shot from that
  bundle at their previous pixel sizes; the panel crops (`branches`, `worktrees`, `bisect`, `stash`,
  `conflict`, `preview`, `danger`, `hero`) show no nav and were left alone.

## Windows 11, x64 — 2026-10-27

- `cargo test --workspace` — 267 passed.
- `pnpm test` — 88 passed (9 files).
- `pnpm build` — TypeScript + Vite production build passes.
- `tauri build` — release binary built; unsigned MSI produced.
- App launched and was driven end-to-end: opened a repo, live status, per-file staging,
  `git add`/`git commit` through the preview dialog, external merge conflict detected via
  the watcher, conflict resolved in-app, `git merge --continue`, and
  `git push -u origin main` to a local bare remote.

## Windows 11, x64 — 2026-11-04

- `cargo test --workspace --no-fail-fast` — 380 passed, including updater integration
  tests, bisect, interactive rebase, submodules, and worktrees.
- Node 26.13.0 / pnpm 10.34.9: `pnpm test` — 205 passed; `pnpm build` — passed.
- The unsigned MSI was built, installed with administrator elevation, and launched.
  In the installed app, worktrees with spaces in their paths were switched to and
  removed, including a dirty force-removal. Successful removals disappear from
  `app.json` and the nav after restart; failed removals preserve workspace entries.
  Two repositories, a group and the dark theme survived restart. Workspace data was
  observed at `%APPDATA%\com.sivartydrup.gitgotten\app.json`; the theme is stored by
  WebView2.
- A 10,120-commit synthetic repository opened and rendered; automated drag and zoom
  each yielded about 60 `requestAnimationFrame` callbacks per second, p95 16.8 ms,
  with no interval over 33.4 ms (4 s drag; 3 s zoom). This measures callback
  scheduling, not presented-frame timing. A rejected double checkout showed Git's
  error in the activity log, and the graph remained usable.
  The installed UI bisected a six-commit history to its known first bad commit, reset
  the session, initialized a submodule, and restored a drifted submodule checkout.
- A local feed offered a real updater-signed Windows MSI to an installed 0.1.0 test
  build. Install downloaded and verified the signed payload, upgraded it to 0.1.1,
  and relaunched the app at 0.1.1. The test versions and feed were local only; no
  signed release is published, and the configured GitHub release feed returns 404.
  The test key was exposed on a guest-readable share and must be rotated before release.
  Windows Authenticode signing and macOS signing/notarization remain unverified.
- Git 2.56's bisect result format, Windows rebase editor paths, inherited sequence-editor
  overrides, submodule fixture paths, Node's storage globals, and the updater test
  executable's Windows manifest were corrected during this verification. A real
  interactive rebase through the app dropped the chosen commit.
- Successful worktree removals now unlink registered checkouts from the persisted
  workspace; failed removals retain them. Files-panel hooks now remain stable when
  status loads asynchronously, avoiding a blank window when opening the tab early.
