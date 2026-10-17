//! The GitGotten Tauri application — a thin shell over `gitcore` and `graph`.
//!
//! Stage 0 opens a window, mounts the web shell, and exposes the first real IPC
//! command: `build_info`. The web `App` calls it and displays the result, proving the
//! Rust -> web bridge end to end.

use git_gotten_protocol::BuildInfo;

/// App identity, exposed to the web shell via the `build_info` command.
///
/// Kept private: Tauri v2's `#[tauri::command]` macro emits module-scope helpers keyed
/// by the function name, and a `pub` command in a library crate double-defines them
/// (E0255). The test below reaches it via `use super::*`.
#[tauri::command]
fn build_info() -> BuildInfo {
    BuildInfo {
        name: "GitGotten".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        arch: std::env::consts::ARCH.to_string(),
    }
}

/// Entry point: build the Tauri app with a single 1280x800 window.
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![build_info])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_info_reports_identity() {
        let bi = build_info();
        assert_eq!(bi.name, "GitGotten");
        assert_eq!(bi.version, env!("CARGO_PKG_VERSION"));
        assert!(!bi.arch.is_empty());
    }
}
