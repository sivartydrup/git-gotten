//! Shared Rust <-> TypeScript types for GitGotten.
//!
//! Every type that crosses the IPC boundary lives here and is generated to TypeScript
//! via `ts-rs` (see SPEC §5). Keeping them in one crate means the Rust core and the web
//! UI cannot drift apart on a shared shape.
//!
//! Stage 0 scaffolds the crate with the first real shared type: [`BuildInfo`], which the
//! app exposes to the web and the shell displays.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Build/runtime identity of the GitGotten binary, surfaced to the web shell.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct BuildInfo {
    /// Product name (`GitGotten`).
    pub name: String,
    /// Semantic version of this build (`CARGO_PKG_VERSION`).
    pub version: String,
    /// Target CPU architecture (e.g. `aarch64`).
    pub arch: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compile-time check that `BuildInfo` implements the `ts-rs` `TS` supertrait.
    fn _assert_ts<T: TS>() {}

    #[test]
    fn build_info_serializes_and_deserializes() {
        let bi = BuildInfo {
            name: "GitGotten".into(),
            version: "0.1.0".into(),
            arch: "aarch64".into(),
        };
        let json = serde_json::to_string(&bi).unwrap();
        assert!(json.contains("\"GitGotten\""));
        assert!(json.contains("\"0.1.0\""));

        let back: BuildInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(back, bi);
    }

    #[test]
    fn build_info_is_a_ts_type() {
        _assert_ts::<BuildInfo>();
    }
}
