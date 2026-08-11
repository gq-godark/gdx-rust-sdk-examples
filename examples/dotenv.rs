//! Tiny `.env` helper shared by the example binaries.
//!
//! Each binary in `examples/*.rs` includes this module via
//!
//! ```text
//! #[path = "dotenv.rs"]
//! mod dotenv;
//! ```
//!
//! Cargo treats every `[[example]]` as its own crate, so this is the standard
//! pattern for sharing tiny utilities across sample programs without making a
//! workspace.
//!
//! `load_dotenv()` snapshots the real process environment, then defers to
//! `dotenvy` so OS values are not overwritten. `env_first` then prefers OS
//! `GODARK_*` / `GDX_*` before the same names from `.env`.
//!
//! `print_order_error()` formats `godark::GodarkError::Order` so the symbolic
//! reject reason (e.g. `PRICE_DEVIATION_TOO_LARGE`) is always visible — the
//! SDK folds the canonical reason code into the error so MMs can match on it
//! programmatically.

#![allow(dead_code)]

use std::collections::HashSet;
use std::sync::OnceLock;

use godark::GodarkError;

static OS_KEYS: OnceLock<HashSet<String>> = OnceLock::new();

fn nonempty(key: &str) -> Option<String> {
    std::env::var(key).ok().and_then(|v| {
        let t = v.trim();
        if t.is_empty() {
            None
        } else {
            Some(t.to_string())
        }
    })
}

/// OS `GODARK_*` then OS `GDX_*`, then the same order from `.env`.
pub fn env_first(keys: &[&str]) -> Option<String> {
    if let Some(os_keys) = OS_KEYS.get() {
        for key in keys {
            if os_keys.contains(*key) {
                if let Some(v) = nonempty(key) {
                    return Some(v);
                }
            }
        }
        for key in keys {
            if !os_keys.contains(*key) {
                if let Some(v) = nonempty(key) {
                    return Some(v);
                }
            }
        }
        None
    } else {
        keys.iter().find_map(|k| nonempty(k))
    }
}

pub fn load_dotenv() {
    let _ = OS_KEYS.get_or_init(|| {
        std::env::vars()
            .filter(|(_, v)| !v.trim().is_empty())
            .map(|(k, _)| k)
            .collect()
    });
    // Parse `.env` ourselves so `$` in passphrases is not interpolated (dotenvy does).
    let text = std::fs::read_to_string(".env").unwrap_or_default();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') || !line.contains('=') {
            continue;
        }
        let (key, val) = line.split_once('=').unwrap();
        let key = key.trim();
        let mut val = val.trim().to_string();
        if val.len() >= 2 {
            let b = val.as_bytes();
            if (b[0] == b'"' && b[val.len() - 1] == b'"')
                || (b[0] == b'\'' && b[val.len() - 1] == b'\'')
            {
                val = val[1..val.len() - 1].to_string();
            }
        }
        if key.is_empty() {
            continue;
        }
        if std::env::var_os(key).is_none() {
            std::env::set_var(key, &val);
        }
    }
}

pub fn print_order_error(operation: &str, err: &GodarkError) {
    match err {
        GodarkError::Order { message, error_code } => eprintln!(
            "{operation}: OrderError code={} reason={}",
            error_code.as_deref().unwrap_or("<none>"),
            message,
        ),
        other => eprintln!("{operation}: {other}"),
    }
}
