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
        // The second arm reads as: "either the second element is None
        // (covering both (Some, None) and (None, None) cases) OR the
        // first element is None and the second is Some". This nested
        // OR-pattern form is what `clippy::unnested_or_patterns` (a
        // pedantic lint) prefers; the unnested equivalent
        // `(Some(_), None) | (None, Some(_)) | (None, None)` is more
        // verbose to read but fires the same lint.
        match (self.after.vram_mb(), self.before.vram_mb()) {
            (Some(after), Some(before)) => Some(after - before),
            (Some(_) | None, None) | (None, Some(_)) => None,
        }
    }

    /// Format a one-line delta summary as an owned `String` ending in a newline.
    ///
    /// Format: `  <label>: RAM <±N> MB  |  VRAM <±M> MB [per-process|device-wide]\n`.
    /// The `VRAM` segment is omitted when [`Self::vram_delta_mb`] is `None`.
    ///
    /// Suitable for log frameworks (`tracing::info!("{}", r.format_delta("step 1"))`)
    /// or capture into a buffer. [`Self::print_delta`] delegates here.
    #[must_use]
    pub fn format_delta(&self, label: &str) -> String {
        let ram = self.ram_delta_mb();
        self.vram_delta_mb().map_or_else(
            || format!("  {label}: RAM {ram:+.0} MB\n"),
            |vram| {
                let qualifier = self.vram_qualifier();
                format!("  {label}: RAM {ram:+.0} MB  |  VRAM {vram:+.0} MB{qualifier}\n")
            },
        )
    }

    /// Format a two-line `before → after` summary as an owned `String`.
    ///
    /// Line 1: `  <label>: RAM <a> MB → <b> MB (<±delta> MB)\n`.
    /// Line 2 (only when both snapshots have `VRAM` data):
    /// `  <label>: VRAM <a> MB → <b> MB (<±delta> MB[ / <total> MB]) [qualifier][ [adapter name]]\n`.
    ///
    /// Suitable for log frameworks or file output.
    /// [`Self::print_before_after`] delegates here.
    #[must_use]
    pub fn format_before_after(&self, label: &str) -> String {
        let mut out = format!(
            "  {label}: RAM {:.0} MB → {:.0} MB ({:+.0} MB)\n",
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
            // BORROW: explicit and_then + map_or + format — gpu_device.name is
            // Option<String>; we need an owned String for the suffix.
            let gpu = self
                .after
                .gpu_device
                .as_ref()
                .and_then(|d| d.name.as_deref())
                .map_or(String::new(), |name| format!(" [{name}]"));
            let line2 = format!(
                "  {label}: VRAM {before:.0} MB → {after:.0} MB ({:+.0} MB{total}){qualifier}{gpu}\n",
                after - before,
            );
            out.push_str(&line2);
        }
        out
    }

    /// Print a one-line summary of the delta to stdout.
    ///
    /// Delegates to [`Self::format_delta`]; the printed string is
    /// byte-for-byte identical to the formatted return value.
    pub fn print_delta(&self, label: &str) {
        print!("{}", self.format_delta(label));
    }

    /// Print a two-line `before → after` summary to stdout.
    ///
    /// Delegates to [`Self::format_before_after`]; the printed string
    /// is byte-for-byte identical to the formatted return value.
    pub fn print_before_after(&self, label: &str) {
        print!("{}", self.format_before_after(label));
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

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::missing_docs_in_private_items
)]
mod tests {
    use super::*;
    use crate::{GpuDeviceInfo, GpuQuerySource, ProcessGpuInfo};

    /// Build a Snapshot for delta/qualifier/format tests.
    fn snapshot_with(
        ram: u64,
        vram_used: Option<u64>,
        is_per_process: bool,
        total: u64,
        name: Option<&str>,
    ) -> Snapshot {
        Snapshot {
            ram_bytes: ram,
            gpu: vram_used.map(|used| ProcessGpuInfo {
                used_bytes: used,
                is_per_process,
                source: if is_per_process {
                    GpuQuerySource::Nvml
                } else {
                    GpuQuerySource::NvidiaSmi
                },
            }),
            gpu_device: Some(GpuDeviceInfo {
                index: 0,
                name: name.map(str::to_owned),
                total_bytes: total,
                free_bytes: total.saturating_sub(vram_used.unwrap_or(0)),
                used_bytes: vram_used.unwrap_or(0),
                reserved_bytes: None,
                driver_version: None,
            }),
        }
    }

    /// Build a Snapshot with no GPU data at all.
    fn snapshot_no_gpu(ram: u64) -> Snapshot {
        Snapshot {
            ram_bytes: ram,
            gpu: None,
            gpu_device: None,
        }
    }

    #[test]
    fn report_delta_positive_for_allocation() {
        let before = snapshot_with(
            100 * 1_048_576,
            Some(500 * 1_048_576),
            true,
            16_384 * 1_048_576,
            None,
        );
        let after = snapshot_with(
            200 * 1_048_576,
            Some(1_000 * 1_048_576),
            true,
            16_384 * 1_048_576,
            None,
        );
        let report = MemoryReport::new(before, after);

        let ram_delta = report.ram_delta_mb();
        assert!(
            (ram_delta - 100.0).abs() < 0.01,
            "RAM delta should be ~100 MB, got {ram_delta}"
        );

        let vram_delta = report.vram_delta_mb().unwrap();
        assert!(
            (vram_delta - 500.0).abs() < 0.01,
            "VRAM delta should be ~500 MB, got {vram_delta}"
        );
    }

    #[test]
    fn report_delta_negative_for_deallocation() {
        let before = snapshot_with(
            500 * 1_048_576,
            Some(2_000 * 1_048_576),
            true,
            16_384 * 1_048_576,
            None,
        );
        let after = snapshot_with(
            300 * 1_048_576,
            Some(800 * 1_048_576),
            true,
            16_384 * 1_048_576,
            None,
        );
        let report = MemoryReport::new(before, after);

        assert!(report.ram_delta_mb() < 0.0);
        assert!(report.vram_delta_mb().unwrap() < 0.0);
    }

    #[test]
    fn report_delta_none_when_no_vram() {
        let report = MemoryReport::new(snapshot_no_gpu(100), snapshot_no_gpu(200));
        assert!(report.vram_delta_mb().is_none());
    }

    #[test]
    fn report_delta_none_when_only_one_side_has_vram() {
        let before = snapshot_no_gpu(100);
        let after = snapshot_with(200, Some(500), true, 1000, None);
        let report = MemoryReport::new(before, after);
        assert!(report.vram_delta_mb().is_none());
    }

    #[test]
    fn vram_qualifier_per_process() {
        let snap = snapshot_with(100, Some(500), true, 1000, None);
        let report = MemoryReport::new(snap.clone(), snap);
        assert_eq!(report.vram_qualifier(), " [per-process]");
    }

    #[test]
    fn vram_qualifier_device_wide() {
        let snap = snapshot_with(100, Some(500), false, 1000, None);
        let report = MemoryReport::new(snap.clone(), snap);
        assert_eq!(report.vram_qualifier(), " [device-wide]");
    }

    #[test]
    fn vram_qualifier_empty_when_no_gpu() {
        let snap = snapshot_no_gpu(100);
        let report = MemoryReport::new(snap.clone(), snap);
        assert_eq!(report.vram_qualifier(), "");
    }

    // -----------------------------------------------------------------------
    // format_delta tests
    // -----------------------------------------------------------------------

    #[test]
    fn format_delta_with_vram_per_process() {
        let before = snapshot_with(
            100 * 1_048_576,
            Some(500 * 1_048_576),
            true,
            16_384 * 1_048_576,
            None,
        );
        let after = snapshot_with(
            200 * 1_048_576,
            Some(1_000 * 1_048_576),
            true,
            16_384 * 1_048_576,
            None,
        );
        let report = MemoryReport::new(before, after);
        let s = report.format_delta("alloc");
        assert_eq!(s, "  alloc: RAM +100 MB  |  VRAM +500 MB [per-process]\n");
    }

    #[test]
    fn format_delta_with_vram_device_wide() {
        let before = snapshot_with(
            100 * 1_048_576,
            Some(500 * 1_048_576),
            false, // device-wide source (e.g. nvidia-smi)
            16_384 * 1_048_576,
            None,
        );
        let after = snapshot_with(
            150 * 1_048_576,
            Some(750 * 1_048_576),
            false,
            16_384 * 1_048_576,
            None,
        );
        let report = MemoryReport::new(before, after);
        let s = report.format_delta("step");
        assert_eq!(s, "  step: RAM +50 MB  |  VRAM +250 MB [device-wide]\n");
    }

    #[test]
    fn format_delta_without_vram() {
        let report = MemoryReport::new(
            snapshot_no_gpu(50 * 1_048_576),
            snapshot_no_gpu(80 * 1_048_576),
        );
        let s = report.format_delta("cpu");
        assert_eq!(s, "  cpu: RAM +30 MB\n");
    }

    #[test]
    fn format_delta_negative_ram() {
        let report = MemoryReport::new(
            snapshot_no_gpu(80 * 1_048_576),
            snapshot_no_gpu(50 * 1_048_576),
        );
        let s = report.format_delta("free");
        assert_eq!(s, "  free: RAM -30 MB\n");
    }

    // -----------------------------------------------------------------------
    // format_before_after tests
    // -----------------------------------------------------------------------

    #[test]
    fn format_before_after_with_vram_and_name() {
        let before = snapshot_with(
            100 * 1_048_576,
            Some(500 * 1_048_576),
            true,
            16_384 * 1_048_576,
            Some("NVIDIA Test GPU"),
        );
        let after = snapshot_with(
            200 * 1_048_576,
            Some(1_000 * 1_048_576),
            true,
            16_384 * 1_048_576,
            Some("NVIDIA Test GPU"),
        );
        let report = MemoryReport::new(before, after);
        let s = report.format_before_after("model_load");
        assert_eq!(
            s,
            "  model_load: RAM 100 MB → 200 MB (+100 MB)\n  \
             model_load: VRAM 500 MB → 1000 MB (+500 MB / 16384 MB) [per-process] [NVIDIA Test GPU]\n"
        );
    }

    #[test]
    fn format_before_after_without_vram() {
        let before = snapshot_no_gpu(100 * 1_048_576);
        let after = snapshot_no_gpu(150 * 1_048_576);
        let report = MemoryReport::new(before, after);
        let s = report.format_before_after("cpu_only");
        assert_eq!(s, "  cpu_only: RAM 100 MB → 150 MB (+50 MB)\n");
    }

    // -----------------------------------------------------------------------
    // print_* delegate to format_*: verify by checking format_* output matches
    // expected stdout. We don't capture stdout; the byte-for-byte equality
    // contract is documented and the format_* tests above lock the format.
    // -----------------------------------------------------------------------

    #[test]
    fn print_delta_does_not_panic() {
        let snap = snapshot_with(100, Some(200), true, 1000, None);
        let report = MemoryReport::new(snap.clone(), snap);
        report.print_delta("smoke"); // just verify it doesn't panic
    }

    #[test]
    fn print_before_after_does_not_panic() {
        let snap = snapshot_with(100, Some(200), true, 1000, None);
        let report = MemoryReport::new(snap.clone(), snap);
        report.print_before_after("smoke");
    }
}
