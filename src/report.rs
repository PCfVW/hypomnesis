// SPDX-License-Identifier: MIT OR Apache-2.0

//! `Snapshot` delta and printing helpers (opt-in via `report` feature).
//!
//! Enable `features = ["report"]` to get [`MemoryReport`] (delta between
//! two `Snapshot`s) and the `candle-mi`-compatible printing helpers
//! (`print_delta`, `print_before_after`). Names are preserved verbatim
//! from `candle-mi`'s in-tree memory module so Phase 3 (`candle-mi`
//! adopts `hypomnesis`) is a Cargo feature flip rather than a code rewrite.
//!
//! Phase 1 scaffolding — real implementation lands in Wave 2 (port from
//! `candle-mi/src/memory.rs` lines 84-275).

use crate::Snapshot;

/// Delta between two `Snapshot`s.
///
/// Construct via [`MemoryReport::new`] from a `before` and `after`
/// snapshot. Positive deltas mean memory increased; negative means freed.
///
/// `#[non_exhaustive]`: fields may be added in future releases.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct MemoryReport {
    /// `Snapshot` taken before the operation.
    pub before: Snapshot,
    /// `Snapshot` taken after the operation.
    pub after: Snapshot,
}

impl MemoryReport {
    /// Create a report from two snapshots.
    #[must_use]
    pub const fn new(before: Snapshot, after: Snapshot) -> Self {
        Self { before, after }
    }

    /// `RAM` delta in megabytes (positive = increased; signed).
    #[must_use]
    #[allow(clippy::missing_const_for_fn)] // Wave 2 impl will determine const-ness
    pub fn ram_delta_mb(&self) -> f64 {
        // Phase 1 scaffolding — real impl lands in Wave 2.
        0.0
    }

    /// Per-process `VRAM` delta in megabytes (positive = increased; signed).
    /// Returns `None` if either snapshot lacks per-process `VRAM` data.
    #[must_use]
    #[allow(clippy::missing_const_for_fn)] // Wave 2 impl will use Option::map (not const)
    pub fn vram_delta_mb(&self) -> Option<f64> {
        // Phase 1 scaffolding — real impl lands in Wave 2.
        None
    }

    /// Print a one-line summary of the delta to stdout.
    #[allow(clippy::missing_const_for_fn)] // Wave 2 impl will use println! (not const)
    pub fn print_delta(&self, _label: &str) {
        // Phase 1 scaffolding — real impl lands in Wave 2.
    }

    /// Print a two-line `before → after` summary to stdout.
    #[allow(clippy::missing_const_for_fn)] // Wave 2 impl will use println! (not const)
    pub fn print_before_after(&self, _label: &str) {
        // Phase 1 scaffolding — real impl lands in Wave 2.
    }
}
