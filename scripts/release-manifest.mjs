#!/usr/bin/env node
// Fill `updater/latest.json` from the artifacts `tauri build` just produced, so no signature is
// ever copied by hand (a hand-copied or stale signature is the one way the updater can be poisoned).
//
// Run it once per build machine: each run fills the targets whose artifacts exist locally and
// leaves the others as they were, then refuses to call the manifest publishable while any target
// still carries a placeholder signature.
//
//   pnpm release:manifest            # fill from target/release/bundle, report what is missing
//   pnpm release:manifest --check    # only verify the current manifest is publishable
import { existsSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const repo = join(dirname(fileURLToPath(import.meta.url)), "..");
const bundleRoot = join(repo, "target/release/bundle");
const manifestPath = join(repo, "updater/latest.json");
const placeholder = "REPLACE-WITH-tauri-signer-sign-OUTPUT";

const conf = JSON.parse(readFileSync(join(repo, "crates/app/tauri.conf.json"), "utf8"));
const manifest = JSON.parse(readFileSync(manifestPath, "utf8"));
const version = conf.version;

if (version !== manifest.version) {
  fail(
    `version drift: tauri.conf.json is ${version}, the manifest is ${manifest.version}. ` +
      "Bump Cargo.toml [workspace.package] version, tauri.conf.json and the manifest together.",
  );
}

if (!process.argv.includes("--check")) {
  for (const [target, entry] of discover()) {
    const signature = readFileSync(entry.sig, "utf8").trim();
    if (!signature.startsWith("dW50cnVzdGVk")) {
      fail(`${entry.sig} is not base64-wrapped minisign armor: build with TAURI_SIGNING_PRIVATE_KEY set.`);
    }
    manifest.platforms[target] = {
      url: `https://github.com/sivartydrup/git-gotten/releases/download/v${version}/${entry.asset}`,
      signature,
    };
    manifest.notes = manifest.notes?.trim() || `${conf.productName} ${version}`;
    manifest.pub_date = new Date().toISOString().replace(/\.\d{3}Z$/, "Z");
    const size = (readFileSync(entry.artifact).byteLength / 1048576).toFixed(2);
    console.log(`filled ${target}: ${entry.asset} (${size} MiB)`);
  }
  writeFileSync(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`);
}

const unfilled = Object.entries(manifest.platforms)
  .filter(([, entry]) => isPlaceholder(entry.signature))
  .map(([target]) => target);

if (unfilled.length === 0) {
  console.log(`publishable for: ${Object.keys(manifest.platforms).join(", ")}`);
  console.log(
    `upload each artifact under the asset name its url ends with, then:\n` +
      `  gh release create v${version} --latest --notes-file NOTES.md ` +
      Object.values(manifest.platforms).map((e) => e.url.split("/").pop()).join(" ") +
      " updater/latest.json",
  );
} else {
  console.log(`not publishable yet - no signed artifact here for: ${unfilled.join(", ")}`);
  console.log("build it on that platform with TAURI_SIGNING_PRIVATE_KEY set, then run this again.");
  if (process.argv.includes("--check")) process.exit(1);
}

// The committed placeholder is base64-wrapped exactly like a real signature, so the marker is
// only visible once decoded.
function isPlaceholder(signature) {
  if (signature.includes(placeholder)) return true;
  try {
    return Buffer.from(signature, "base64").toString("utf8").includes(placeholder);
  } catch {
    return false;
  }
}

// Bundle file names carry the target: the .dmg names the arch it was built for and the .msi names
// x64. `GitGotten.app.tar.gz` (the updater artifact) is arch-blind, so it is paired with the .dmg
// from the same build, and uploaded under the arch-named asset the manifest links to.
function discover() {
  const found = new Map();
  if (!existsSync(bundleRoot)) return found;

  const macos = list(join(bundleRoot, "macos"));
  const dmgArch = list(join(bundleRoot, "dmg"))
    .map((file) => /GitGotten_.+_([a-z0-9]+)\.dmg$/.exec(file)?.[1])
    .filter(Boolean);
  const tar = macos.find((file) => file.endsWith(".app.tar.gz"));
  if (tar && dmgArch.length > 0) {
    const arch = dmgArch[0];
    found.set(`darwin-${arch}`, {
      artifact: join(bundleRoot, "macos", tar),
      sig: join(bundleRoot, "macos", `${tar}.sig`),
      asset: `GitGotten_${version}_${arch === "aarch64" ? "aarch64" : "x64"}.app.tar.gz`,
    });
  }

  const msi = list(join(bundleRoot, "msi")).find(
    (file) => file.endsWith(".msi") && file.includes(version) && file.includes("x64"),
  );
  if (msi) {
    found.set("windows-x86_64", {
      artifact: join(bundleRoot, "msi", msi),
      sig: join(bundleRoot, "msi", `${msi}.sig`),
      asset: msi,
    });
  }
  return found;
}

function list(dir) {
  try {
    return readdirSync(dir);
  } catch {
    return [];
  }
}

function fail(message) {
  console.error(message);
  process.exit(1);
}
