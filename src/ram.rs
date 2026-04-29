// SPDX-License-Identifier: MIT OR Apache-2.0

//! Process `RAM` (`RSS`) measurement.
//!
//! - **Windows**: `K32GetProcessMemoryInfo` → `WorkingSetSize` (exact, per-process).
//! - **Linux**: `/proc/self/status` → `VmRSS` (exact, per-process, no `unsafe`).

use crate::{HypomnesisError, Result};

/// Query the current process's resident set size (`RSS`) in bytes.
///
/// Returns the per-process resident-set figure as reported by the
/// operating system: working-set bytes on Windows, `VmRSS` on Linux.
/// On other targets this returns an error.
///
/// # Errors
///
/// Returns [`HypomnesisError::Ram`] if the platform API call fails.
/// Returns [`HypomnesisError::Io`] if reading `/proc/self/status` fails on Linux.
pub fn process_rss() -> Result<u64> {
    // Phase 1 scaffolding — real implementation lands in Wave 2 (port from
    // `candle-mi/src/memory.rs` lines 277-417).
    Err(HypomnesisError::Ram(
        "process_rss not yet implemented (Phase 1 scaffolding)".into(),
    ))
}
