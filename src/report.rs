// SPDX-License-Identifier: MIT OR Apache-2.0

//! `Snapshot` delta and printing helpers (opt-in via `report` feature).
//!
//! Enable `features = ["report"]` to get [`MemoryReport`] (delta between
//! two `Snapshot`s) and the `candle-mi`-compatible printing helpers
//! (`print_delta`, `print_before_after`). Names are preserved verbatim
//! from `candle-mi`'s in-tree memory module so Phase 3 (`candle-mi`
//! adopts `hypomnesis`) is a Cargo feature flip + thin adapter rather
//! than a code rewrite.

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
    pub fn ram_delta_mb(&self) -> f64 {
        self.after.ram_mb() - self.before.ram_mb()
    }

    /// Per-process `VRAM` delta in megabytes (positive = increased; signed).
    ///
    /// Returns `None` if either snapshot lacks per-process `VRAM` data.
    #[must_use]
    pub fn vram_delta_mb(&self) -> Option<f64> {
        // EXHAUSTIVE: explicit listing of all four (Some/None × Some/None) cases
        // avoids `_ => None` (which would trigger `wildcard_match_arm`) and
        // documents that we only return Some when BOTH snapshots have data.
        match (self.after.vram_mb(), self.before.vram_mb()) {
            (Some(after), Some(before)) => Some(after - before),
            (Some(_) | None, None) | (None, Some(_)) => None,
        }
    }

    /// Print a one-line summary of the delta to stdout.
    ///
    /// Format: `  <label>: RAM <±N> MB  |  VRAM <±M> MB [per-process|device-wide]`.
    /// The `VRAM` segment is omitted when `vram_delta_mb` is `None`.
    pub fn print_delta(&self, label: &str) {
        let ram = self.ram_delta_mb();
        print!("  {label}: RAM {ram:+.0} MB");
        if let Some(vram) = self.vram_delta_mb() {
            let qualifier = self.vram_qualifier();
            print!("  |  VRAM {vram:+.0} MB{qualifier}");
        }
        println!();
    }

    /// Print a two-line `before → after` summary to stdout.
    ///
    /// First line is `RAM <a> MB → <b> MB (<±delta> MB)`.
    /// Second line (printed only when both snapshots have `VRAM` data)
    /// is `VRAM <a> MB → <b> MB (<±delta> MB[ / <total> MB]) [qualifier][ [adapter name]]`.
    pub fn print_before_after(&self, label: &str) {
        println!(
            "  {label}: RAM {:.0} MB → {:.0} MB ({:+.0} MB)",
            self.before.ram_mb(),
            self.after.ram_mb(),
            self.ram_delta_mb(),
        );
        if let (Some(before), Some(after)) = (self.before.vram_mb(), self.after.vram_mb()) {
            // CAST: u64 → f64, byte count for MiB conversion (fits in f64 mantissa).
            #[allow(clippy::cast_precision_loss, clippy::as_conversions)]
            let total = self.after.gpu_device.as_ref().map_or(String::new(), |d| {
                format!(" / {:.0} MB", d.total_bytes as f64 / 1_048_576.0)
            });
            let qualifier = self.vram_qualifier();
            // BORROW: explicit map + format — gpu_device.name is Option<String>;
            // we need an owned String for the suffix.
            let gpu = self
                .after
                .gpu_device
                .as_ref()
                .and_then(|d| d.name.as_deref())
                .map_or(String::new(), |name| format!(" [{name}]"));
            println!(
                "  {label}: VRAM {before:.0} MB → {after:.0} MB ({:+.0} MB{total}){qualifier}{gpu}",
                after - before,
            );
        }
    }

    /// Short qualifier string indicating `VRAM` measurement scope.
    ///
    /// Returns `" [per-process]"` when the after-snapshot's per-process
    /// reading is genuinely per-process (`DXGI` or `NVML`),
    /// `" [device-wide]"` when it fell back to `nvidia-smi`, or `""`
    /// when no `VRAM` data is available.
    const fn vram_qualifier(&self) -> &'static str {
        match self.after.gpu.as_ref() {
            Some(g) if g.is_per_process => " [per-process]",
            Some(_) => " [device-wide]",
            None => "",
        }
    }
}
