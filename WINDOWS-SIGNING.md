# Windows: signed build + updater verification

Instructions for an agent working in this repo **on the Windows PC**. Everything here is
the stage-14 distribution path (`SPEC.md` §17): unsigned builds already work; this file is
about producing *signed updater artifacts* and proving the in-app update flow.

Verified facts this file is built on:

- `tauri-cli` only signs/creates updater artifacts when
  `create_updater_artifacts != Updater::Bool(false)` (`crates/tauri-cli/src/interface/rust.rs:851`).
  `crates/app/tauri.conf.json` ships `"createUpdaterArtifacts": false`, so a plain
  `pnpm tauri:build` needs **no key**. You opt in per build.
- The pubkey pinned in `crates/app/tauri.conf.json → plugins.updater.pubkey` is
  minisign key id `87D593B9B31417`. Its private half lives **outside the repo**
  (`~/.tauri/gitgotten.key` on the macOS machine, mode `600`, generated *without* a password).
- Version guards: `crates/app/tests/updater.rs:237` requires `tauri.conf.json` version ==
  `CARGO_PKG_VERSION`; `updater.rs:172` and `:200` refuse a manifest that advertises a
  version newer than the app being built.

## 0. Get the repo current

```powershell
git pull origin main
pnpm install
pnpm --dir src install
```

## 1. The release key — rotated on macOS (2026-11-05)

One key signs every release artifact. The pubkey pinned at `crates/app/tauri.conf.json →
plugins.updater.pubkey` is the only one the app accepts, and it is now the **rotated** key: the
pair that crossed the guest share is retired and unpinned, so every build made before 2026-11-05
(the macOS `.dmg`, the Windows test MSIs) must be replaced by a build made after it.

On this Mac (`~/.tauri/`, all `0600` except the public half):

| File | What it is |
|---|---|
| `gitgotten-rotated.key` | the private key, password-protected — never committed, never copied onto a share |
| `gitgotten-rotated.key.password` | its password — move it into a password manager, then delete the file |
| `gitgotten-rotated.key.pub` | the public key; its contents are exactly what `tauri.conf.json` pins |
| `gitgotten.key.retired-20261104*` | the exposed pair, kept only until nothing still needs it — delete when you decide |

Generated with `pnpm exec tauri signer generate -w ~/.tauri/gitgotten-rotated.key --password
"$(cat ~/.tauri/gitgotten-rotated.key.password)" --ci --force` — the password comes from a `0600`
file, so it never reaches chat or shell history. Piping the password to the prompt instead does
**not** work: the prompt reads `/dev/tty`, the key silently gets no password, and `tauri build`
then fails with *"incorrect updater private key password"*.

Signing needs both variables in the shell that runs the build:

```bash
export TAURI_SIGNING_PRIVATE_KEY="$HOME/.tauri/gitgotten-rotated.key"   # key text or path
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD="$(cat "$HOME/.tauri/gitgotten-rotated.key.password")"
pnpm tauri:build --config '{"bundle":{"createUpdaterArtifacts":true}}'
```

`createUpdaterArtifacts` deliberately stays out of the committed config: with no private key in
the environment `tauri build` fails outright, so a committed `true` would break unsigned dev builds.

**The Windows PC must use this same key** — the MSI can only be built there. The SMB mount
`//GUEST@192.168.0.104/Users` is guest and read-only, and it is how the previous key leaked: move
the private key and its password out-of-band (USB stick or password manager), never through a
share, and place them in `%USERPROFILE%\.tauri\`. If you would rather generate a fresh key on the
PC, re-pin its pubkey in `tauri.conf.json` and rebuild *every* artifact — a bundle only ever
accepts signatures from the key it was built against.

macOS artifacts were rebuilt against this key on 2026-11-05 (`bundle/macos/GitGotten.app.tar.gz` +
`.sig`, `bundle/dmg/GitGotten_0.1.0_aarch64.dmg`, 7.67 / 3.97 / 4.13 MiB). The signature was checked
outside tauri with `minisign-verify`: the pubkey pinned in `tauri.conf.json` accepts it, the retired
key rejects it (*"created with a different key"*), and a one-byte-tampered copy of the artifact is
rejected too. Unsigned `pnpm tauri:build` (no key in the environment) still works.

## 2. Build signed updater artifacts

```powershell
$env:TAURI_SIGNING_PRIVATE_KEY="$env:USERPROFILE\.tauri\gitgotten-rotated.key"   # copied out-of-band
$env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD="<the password from gitgotten-rotated.key.password>"
[System.IO.File]::WriteAllText("$env:TEMP\gitgotten-signed.json", '{"bundle":{"createUpdaterArtifacts":true}}', [System.Text.UTF8Encoding]::new($false))
pnpm tauri:build --config "$env:TEMP\gitgotten-signed.json"
```

Expected output (from `bundle.targets = "all"`):

```
target\release\bundle\msi\GitGotten_<version>_x64_en-US.msi            # the installer
target\release\bundle\msi\GitGotten_<version>_x64_en-US.msi.sig        # updater signature (base64 armor)
```

`--config` accepts a **file path** or a JSON string; a `dotted.key=value` argument is not accepted by the installed CLI.
`pnpm tauri signer sign <file>` signs an existing artifact by hand if needed.

## 3. Fill in the manifest, then publish

`updater/latest.json` at the repo root is the exact file published as
`…/releases/latest/download/latest.json`. Never copy a signature into it by hand:
`pnpm release:manifest` fills every target whose signed artifact is present under
`target/release/bundle`, reading each `.sig` file directly.

```bash
pnpm tauri:build --config '{"bundle":{"createUpdaterArtifacts":true}}'   # signing env set
pnpm release:manifest            # fills the targets built here, reports the rest
pnpm release:manifest --check    # exits 1 while any target still holds a placeholder
```

Each run fills what that machine built and leaves the others, so the manifest is assembled by
building once per platform **with the same key** (this key). Publishing is allowed only after
`--check` passes: a leftover placeholder signature means every client rejects that update.

Upload each artifact under the exact asset name its `url` ends with — the updater artifact is
built as `GitGotten.app.tar.gz` but must be uploaded as
`GitGotten_<version>_<arch>.app.tar.gz` — then:

```bash
gh release create v<version> --latest --notes-file NOTES.md \
  GitGotten_<version>_aarch64.app.tar.gz GitGotten_<version>_x64.app.tar.gz \
  GitGotten_<version>_x64_en-US.msi updater/latest.json
```

Versions move together — `Cargo.toml [workspace.package] version`,
`crates/app/tauri.conf.json → version` and the manifest: `cargo test` fails if they drift, and
`release:manifest` refuses to run on a mismatch. Run `cargo test` before committing.

## 4. Prove the Install path (the part no test can cover)

The updater only offers what is *newer* than the running build, so you need two builds.

1. **Old build to install.** At the current version, build with a mock feed baked in so the
   installed app can reach a local server:

   `mock-feed.json` (outside the repo, e.g. `C:\gg-feed\mock-feed.json`):

   ```json
   {
     "plugins": {
       "updater": {
         "pubkey": "<the pubkey now in tauri.conf.json>",
         "endpoints": ["http://127.0.0.1:8099/latest.json"],
         "dangerousInsecureTransportProtocol": true
       }
     }
   }
   ```

   ```powershell
   pnpm tauri:build --config C:\gg-feed\mock-feed.json
   # install target\release\bundle\msi\GitGotten_<old>_x64_en-US.msi
   ```

   Unsigned: SmartScreen will warn — "More info" → "Run anyway".

2. **New build to serve.** Bump the version, rebuild with `TAURI_SIGNING_PRIVATE_KEY` and
   an override enabling `createUpdaterArtifacts`, then copy the `.msi` and a `latest.json`
   (version = the new version, `windows-x86_64` → `http://127.0.0.1:8099/GitGotten_<new>_x64_en-US.msi`,
   signature = that `.sig` file's contents) into `C:\gg-feed`, and serve it:

   ```powershell
   python -m http.server 8099 --bind 127.0.0.1 --directory C:\gg-feed
   ```

3. **Launch the installed old app.** Expect the prompt "Update available: <new>" with the
   manifest's notes, then Install → download → signature verified against the pinned pubkey
   → installer runs → the app relaunches at the new version.

Failure signatures worth reporting verbatim:

- *invalid signature / signature verification failed* → the `.sig` came from a different key
  than the pinned pubkey (key rotation mismatch, or a placeholder left in the manifest).
- *no endpoints* / feed 404 → the app records the reason and shows nothing; that is the
  designed quiet state, not a bug.
- *Updater endpoint doesn't use https* at build/run time → that build lacks
  `dangerousInsecureTransportProtocol`; release builds must keep https-only.
- no prompt although the feed is reachable → the manifest version is not greater than the
  installed app's version.

## 5. Report back

Report: the artifact paths + sizes, the `.sig` filenames (contents are not secrets, but
paste them into the manifest, not into chat), the exact command you ran, and the observed
prompt/Install result. Keep private key material out of logs, commits, and chat.

Not covered here, still blocked on infrastructure: macOS Developer ID signing +
notarization (`APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`,
`APPLE_ID`/`APPLE_PASSWORD`/`APPLE_TEAM_ID` or `APPLE_API_KEY`/`APPLE_API_ISSUER`), Windows
Authenticode signing (`TAURI_WINDOWS_SIGNTOOL_PATH`, `bundle.windows.signCommand`), and CI
that publishes the release plus `latest.json`.
