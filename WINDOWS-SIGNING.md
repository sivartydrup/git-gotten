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

## 1. Choose a key — rotate (recommended) or move

No signed release has been published. Existing local builds still pin the old public
key and must be reinstalled after rotation. The key transferred through the guest-readable
share must be considered exposed and rotated before release.

**Option A — generate the key on this PC (recommended).** The release machine keeps its
private key locally, password-protected:

```powershell
mkdir "$env:USERPROFILE\.tauri" -Force
pnpm exec tauri signer generate -w "$env:USERPROFILE\.tauri\gitgotten-rotated.key"
```

Choose a password when prompted. `-p` requires a password argument; avoid passing secrets
on the command line, where process listings or shell history can expose them. The command
prints the **public key** — a single base64 line. Copy that line into
`crates/app/tauri.conf.json → plugins.updater.pubkey` (replacing the current value), then:

```powershell
cargo test -p git-gotten-app --test updater   # config + manifest tests must stay green
git add -A
git commit -m "stage-14: rotate updater keypair to the release machine"
git push origin main
```

Never `git add` the private key: it lives in `%USERPROFILE%\.tauri\`, outside the repo.
If macOS builds were already made with the old pubkey, they must be rebuilt after rotation
— the pubkey is baked into every bundle, and an old bundle rejects a new key's signatures.

**Option B — reuse the existing macOS key.** Only if a *writable* path to the PC exists
(the current SMB mount `//GUEST@192.168.0.104/Users` is guest, read-only). Copy
`~/.tauri/gitgotten.key` (Mac) to `%USERPROFILE%\.tauri\gitgotten.key`, then set an empty
password — the key has none:

```powershell
$env:TAURI_SIGNING_PRIVATE_KEY="$env:USERPROFILE\.tauri\gitgotten.key"
$env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD=""
```

If the key is later stored with a password, put that password in
`TAURI_SIGNING_PRIVATE_KEY_PASSWORD` instead. `.env` files are not read by the CLI — real
environment variables only, in the same shell that runs the build.

## 2. Build signed updater artifacts

```powershell
$env:TAURI_SIGNING_PRIVATE_KEY="$env:USERPROFILE\.tauri\gitgotten.key"
$env:TAURI_SIGNING_PRIVATE_KEY_PASSWORD="<password or empty string>"
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

## 3. Fill in the manifest

`updater/latest.json` at the repo root is the exact file published as
`…/releases/latest/download/latest.json`. Its `signature` fields are placeholders
(`REPLACE-WITH-tauri-signer-sign-OUTPUT` — the fixture test only checks the shape).

For each platform entry:

- `url` → the published artifact for that os-arch:
  `windows-x86_64` → the `.msi`, `darwin-aarch64` → `GitGotten_<version>_aarch64.app.tar.gz`.
- `signature` → the **entire contents** of that artifact's `.sig` file (already base64 armor).
- `version` → the version you just built, `notes` → the release notes the app will show.

Then bump versions **together** — `Cargo.toml [workspace.package] version` and
`crates/app/tauri.conf.json → version` — and keep `updater/latest.json` describing the
newest published release. Run `cargo test` before committing: the guards listed above fail
if the three drift apart.

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
