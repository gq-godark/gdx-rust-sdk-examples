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
//! `load_dotenv()` defers to the well-known `dotenvy` crate so the OS
//! environment always wins over `.env`.
//!
//! `print_order_error()` formats `godark::GodarkError::Order` so the symbolic
//! reject reason (e.g. `PRICE_DEVIATION_TOO_LARGE`) is always visible — the
//! SDK folds the canonical reason code into the error so MMs can match on it
//! programmatically.

#![allow(dead_code)]

use godark::GodarkError;

pub fn load_dotenv() {
    let _ = dotenvy::dotenv();
}

pub fn print_order_error(operation: &str, err: &GodarkError) {
    match err {
        GodarkError::Order {
            message,
            error_code,
        } => eprintln!(
            "{operation}: OrderError code={} reason={}",
            error_code.as_deref().unwrap_or("<none>"),
            message,
        ),
        other => eprintln!("{operation}: {other}"),
    }
}
