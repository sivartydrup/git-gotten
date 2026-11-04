//! Stage 14 (SPEC §17): the auto-updater contract.
//!
//! These tests pin the pieces that ship *outside* the binary and therefore
//! cannot be caught by the UI tests: the hosted update manifest (fixture at
//! repo root `updater/latest.json`, the exact file that gets published), the
//! app's embedded updater configuration, and the real plugin's check decision
//! exercised against a local mock feed (a plain HTTP server on 127.0.0.1).
//!
//! The mock feed only works over http — allowed because `cargo test` is a
//! debug build; release builds keep the https-only rule.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

use tauri_plugin_updater::{Error, RemoteRelease, UpdaterExt};

/// Minimal single-file HTTP server serving one canned body per connection
/// (the last body repeats). Enough for the updater's GET requests.
struct MockFeed {
    port: u16,
    handle: thread::JoinHandle<()>,
}

impl MockFeed {
    fn start(bodies: Vec<String>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock feed");
        let port = listener.local_addr().unwrap().port();
        let counter = std::sync::atomic::AtomicUsize::new(0);
        let handle = thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut stream) = conn else { continue };
                let mut buf = [0u8; 2048];
                let _ = stream.read(&mut buf); // the request head is not inspected
                let i = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                let body = &bodies[std::cmp::min(i, bodies.len() - 1)];
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        Self { port, handle }
    }

    fn url(&self) -> String {
        format!("http://127.0.0.1:{}/latest.json", self.port)
    }
}

impl Drop for MockFeed {
    fn drop(&mut self) {
        let _ = self.handle;
    }
}

/// The static manifest shape the plugin parses (`RemoteRelease`), for the
/// given advertised version. `{{target}}` in the URL resolves to the same
/// key the platforms map uses.
fn manifest(version: &str) -> serde_json::Value {
    serde_json::json!({
        "version": version,
        "notes": format!("GitGotten {version} — mock release notes"),
        "pub_date": "2026-11-04T00:00:00Z",
        "platforms": {
            "darwin-aarch64": {
                "url": "https://mock.invalid/GitGotten_darwin-aarch64.app.tar.gz",
                "signature": "dW50cnVzdGVkIGNvbW1lbnQ6IG1vY2sgc2lnbmF0dXJlCm5vdC1hLXJlYWwtc2lnbmF0dXJlCg=="
            },
            "darwin-x86_64": {
                "url": "https://mock.invalid/GitGotten_darwin-x86_64.app.tar.gz",
                "signature": "dW50cnVzdGVkIGNvbW1lbnQ6IG1vY2sgc2lnbmF0dXJlCm5vdC1hLXJlYWwtc2lnbmF0dXJlCg=="
            },
            "windows-x86_64": {
                "url": "https://mock.invalid/GitGotten_windows-x86_64.msi.zip",
                "signature": "dW50cnVzdGVkIGNvbW1lbnQ6IG1vY2sgc2lnbmF0dXJlCm5vdC1hLXJlYWwtc2lnbmF0dXJlCg=="
            }
        }
    })
}

/// An app on the mock runtime carrying the **real** embedded configuration —
/// so these tests exercise the same `plugins.updater` config (pubkey,
/// endpoints) the shipped binary reads. Endpoints are overridden per case.
fn mock_app() -> tauri::App<tauri::test::MockRuntime> {
    tauri::test::mock_builder()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .build(tauri::generate_context!())
        .expect("mock app")
}

#[test]
fn updater_with_no_endpoints_fails_loudly() {
    let app = mock_app();
    assert!(matches!(
        app.updater_builder().endpoints(vec![]).unwrap().build(),
        Err(Error::EmptyEndpoints)
    ));
}

#[tokio::test]
async fn mock_feed_announcing_a_newer_version_is_an_update() {
    let feed = MockFeed::start(vec![manifest("9.9.9").to_string()]);
    let app = mock_app();
    let updater = app
        .updater_builder()
        .endpoints(vec![feed.url().parse().expect("mock endpoint")])
        .expect("mock endpoints")
        .build()
        .expect("updater");

    let update = updater.check().await.expect("mock feed check").expect("update");

    let host = tauri_plugin_updater::target().expect("host target");
    let (os, _) = host.split_once('-').expect("os-arch target");
    assert_eq!(update.target, os, "the release is matched to this os-arch");
    assert_eq!(update.version, "9.9.9");
    assert_eq!(
        update.body.as_deref(),
        Some("GitGotten 9.9.9 — mock release notes")
    );
    let artifact = if os == "windows" { "msi.zip" } else { "app.tar.gz" };
    assert_eq!(
        update.download_url.as_str(),
        format!("https://mock.invalid/GitGotten_{host}.{artifact}"),
        "the feed entry for this os-arch is the one served"
    );
}

#[tokio::test]
async fn mock_feed_with_current_or_older_version_is_not_an_update() {
    let current = mock_app().package_info().version.to_string();

    let same = MockFeed::start(vec![manifest(&current).to_string()]);
    let app = mock_app();
    let updater = app
        .updater_builder()
        .endpoints(vec![same.url().parse().expect("mock endpoint")])
        .expect("mock endpoints")
        .build()
        .expect("updater");
    assert!(
        updater.check().await.expect("same-version check").is_none(),
        "the current version must not be offered as an update"
    );

    let older = MockFeed::start(vec![manifest("0.0.9").to_string()]);
    let app = mock_app();
    let updater = app
        .updater_builder()
        .endpoints(vec![older.url().parse().expect("mock endpoint")])
        .expect("mock endpoints")
        .build()
        .expect("updater");
    assert!(
        updater.check().await.expect("older-version check").is_none(),
        "a downgrade must not be offered"
    );
}

/// The manifest fixture is the exact file published to the update host, so
/// the plugin must parse it and pick the entry for every supported target.
#[test]
fn published_manifest_fixture_is_valid() {
    let raw = include_str!("../../../updater/latest.json");
    let release: RemoteRelease = serde_json::from_str(raw).expect("fixture parses as a release manifest");

    assert!(
        release.version <= semver::Version::parse(env!("CARGO_PKG_VERSION")).unwrap(),
        "a manifest must never advertise a version ahead of the shipped app"
    );
    assert!(release.notes.as_deref().map(|n| !n.is_empty()).unwrap_or(false));
    assert!(release.pub_date.is_some(), "manifests need a publish date");

    for target in ["darwin-aarch64", "darwin-x86_64", "windows-x86_64"] {
        let url = release.download_url(target).expect("fixture covers target");
        let signature = release.signature(target).expect("fixture signature");
        assert_eq!(url.scheme(), "https", "artifacts ship over https");
        assert!(
            signature.starts_with("dW50cnVzdGVk"),
            "manifest signatures are base64-wrapped minisign armor"
        );
    }
}

#[tokio::test]
async fn fixture_announces_the_current_version_and_is_not_an_update() {
    let feed = MockFeed::start(vec![include_str!("../../../updater/latest.json").to_string()]);
    let app = mock_app();
    let updater = app
        .updater_builder()
        .endpoints(vec![feed.url().parse().expect("mock endpoint")])
        .expect("mock endpoints")
        .build()
        .expect("updater");
    assert!(
        updater.check().await.expect("fixture feed check").is_none(),
        "the published fixture advertises exactly the current app version"
    );
}

#[tokio::test]
async fn unsigned_placeholder_signatures_are_rejected() {
    let feed = MockFeed::start(vec![include_str!("../../../updater/latest.json").to_string()]);
    let app = mock_app();
    let updater = app
        .updater_builder()
        .endpoints(vec![feed.url().parse().expect("mock endpoint")])
        .expect("mock endpoints")
        .build()
        .expect("updater");
    // Same manifest, version bumped so it qualifies as an update; its
    // placeholder signatures must then be rejected against any bytes.
    let mut metadata: serde_json::Value =
        serde_json::from_str(include_str!("../../../updater/latest.json")).unwrap();
    metadata["version"] = serde_json::json!("99.0.0");
    let update = updater
        .restore_update(metadata)
        .expect("restored")
        .expect("update");
    assert!(
        update.verify(b"public fixture bytes").is_err(),
        "the updater must reject a package the key never signed"
    );
}

/// The distribution configuration `tauri build` actually reads, asserted
/// from the file: the dev-mode embedded context carries bundle defaults that
/// only the bundler applies, so it is not the thing worth pinning here.
#[test]
fn distribution_config_is_wired_and_release_safe() {
    let config: tauri::utils::config::Config =
        serde_json::from_str(include_str!("../tauri.conf.json")).expect("valid tauri config");
    assert_eq!(config.version.as_deref(), Some(env!("CARGO_PKG_VERSION")));

    let updater = config
        .plugins
        .0
        .get("updater")
        .expect("plugins.updater is configured");
    let pubkey = updater["pubkey"].as_str().expect("pubkey string");
    assert!(
        pubkey.starts_with("dW50cnVzdGVk"),
        "the configured pubkey is base64-wrapped minisign armor"
    );

    let endpoints = updater["endpoints"].as_array().expect("endpoints array");
    assert!(!endpoints.is_empty());
    for endpoint in endpoints {
        let url = endpoint.as_str().expect("endpoint string");
        assert!(url.starts_with("https://"), "release builds require https endpoints");
        assert!(
            url.ends_with("/latest.json"),
            "the endpoint hosts exactly the manifest this repo publishes"
        );
    }

    // One bundle config serves both OS gates: "all" yields .dmg on macOS and
    // .msi on Windows (SPEC §17).
    assert_eq!(serde_json::to_string(&config.bundle.targets).unwrap(), "\"all\"");
    assert_eq!(
        config.bundle.macos.minimum_system_version.as_deref(),
        Some("12.0"),
        "macOS 12 is the supported floor"
    );

    // Updater artifacts need the signing key, so they stay a release-time
    // opt-in: an unsigned dev `tauri build` must still produce an installer.
    assert!(matches!(
        config.bundle.create_updater_artifacts,
        tauri::utils::config::Updater::Bool(false)
    ));

    // The web reaches the updater only through the main window's capabilities.
    let capabilities: serde_json::Value =
        serde_json::from_str(include_str!("../capabilities/default.json")).unwrap();
    let permissions = capabilities["permissions"].as_array().expect("permissions array");
    for required in ["updater:default", "process:allow-restart"] {
        assert!(
            permissions.iter().any(|p| p == required),
            "{required} missing from the main window capability"
        );
    }
}
