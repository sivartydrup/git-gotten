/**
 * Web-side mirrors of the shared Rust types in `crates/protocol` (SPEC §5).
 *
 * Stage 0 hand-writes these to match the Rust shapes; once `ts-rs` generation is wired,
 * these become the generated output in `gen/` and this file is removed.
 */

export interface BuildInfo {
  name: string;
  version: string;
  arch: string;
}
