//! Shared helpers for the GoDark Rust SDK examples.
//!
//! Each binary in `examples/*.rs` includes this module via
//!     #[path = "common.rs"] mod common;
//! Cargo treats every `[[example]]` as its own crate, so this is the standard
//! workaround for sharing tiny utilities between sample programs without
//! introducing a workspace member.
//!
//! What lives here:
//!   * `load_dotenv()` — best-effort `.env` loader so `cargo run --example ...`
//!     picks up `GODARK_*` / `GDX_*` from a sibling `.env` file. Mirrors the
//!     C++ `<godark/env_loader.hpp>` helper, with the same OS-env-wins rule.
//!   * `env_first(&[...])` — reads the first non-empty env var from a list,
//!     letting examples prefer the canonical `GODARK_*` names while still
//!     honoring the legacy `GDX_*` aliases.
//!   * `print_godark_error(...)` — uniform error printer for `GodarkError`,
//!     surfacing the symbolic code (e.g. `PRICE_DEVIATION_TOO_LARGE`) and
//!     the human reason that the SDK now decodes from rejection ACKs.

#![allow(dead_code)]

use godark::GodarkError;

/// Best-effort load of `.env` from the current working directory or any
/// parent. The OS environment always wins, mirroring `dotenvy::dotenv()` and
/// the C++ `load_dotenv()` helper.
pub fn load_dotenv() {
    // `dotenvy::dotenv` returns `Err` when no file is found; that's a no-op,
    // not a problem.
    let _ = dotenvy::dotenv();
}

/// Return the first non-empty value among the named env vars, or `None`.
pub fn env_first(names: &[&str]) -> Option<String> {
    for name in names {
        if let Ok(v) = std::env::var(name) {
            if !v.is_empty() {
                return Some(v);
            }
        }
    }
    None
}

/// Print a `GodarkError` in a stable format suitable for the smoke logs that
/// the example runner asserts against. The SDK now folds the symbolic
/// `error_code` (e.g. `PRICE_DEVIATION_TOO_LARGE`) and the human reason into
/// `GodarkError::Order`, so this helper always yields both for rejections.
pub fn print_godark_error(tag: &str, operation: &str, err: &GodarkError) {
    match err {
        GodarkError::Order { message, error_code } => {
            eprintln!(
                "{tag} {operation} OrderError code={} reason={}",
                error_code.as_deref().unwrap_or("<none>"),
                message,
            );
        }
        GodarkError::Authentication(reason) => {
            eprintln!("{tag} {operation} AuthenticationError reason={reason}");
        }
        GodarkError::Session(reason) => {
            eprintln!("{tag} {operation} SessionError reason={reason}");
        }
        GodarkError::Connection(reason) => {
            eprintln!("{tag} {operation} ConnectionError reason={reason}");
        }
        GodarkError::Encryption(reason) => {
            eprintln!("{tag} {operation} EncryptionError reason={reason}");
        }
        GodarkError::Timeout(reason) => {
            eprintln!("{tag} {operation} TimeoutError reason={reason}");
        }
        GodarkError::Config(reason) => {
            eprintln!("{tag} {operation} ConfigError reason={reason}");
        }
        other => {
            eprintln!("{tag} {operation} GodarkError reason={other}");
        }
    }
}
