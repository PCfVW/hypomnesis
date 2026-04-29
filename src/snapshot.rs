// SPDX-License-Identifier: MIT OR Apache-2.0

//! Snapshot data types — what a `hypomnesis` measurement returns.

use crate::Result;

/// Device-wide GPU information for a specific GPU index.
///
/// Reports what the device currently holds across **all** processes —
/// useful for sizing decisions ("can this model fit?"). For per-process
/// accounting, see `ProcessGpuInfo`.
///
/// `#[non_exhaustive]`: fields may be added in future releases (e.g.,
/// `temperature_celsius`, `pcie_link_gen`).
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct GpuDeviceInfo {
    /// Zero-based GPU index (`NVML`-canonical ordering on Windows).
    pub index: u32,
    /// Adapter name (e.g., `NVIDIA GeForce RTX 5060 Ti`).
    /// `None` when the source backend (e.g., `NVML` on a system where
    /// `nvmlDeviceGetName` failed) does not provide it.
    pub name: Option<String>,
    /// Total GPU memory in bytes.
    pub total_bytes: u64,
    /// Free GPU memory in bytes (device-wide).
    pub free_bytes: u64,
    /// Used GPU memory in bytes (device-wide; sum across all processes).
    pub used_bytes: u64,
}

/// Per-process GPU memory information.
///
/// `#[non_exhaustive]`: fields may be added in future releases.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct ProcessGpuInfo {
    /// GPU memory used by this process in bytes.
    ///
    /// When `is_per_process` is `false`, this is the device-wide total
    /// (the `nvidia-smi` fallback cannot break the figure down per process).
    pub used_bytes: u64,
    /// `true` when the value is genuinely per-process (`DXGI` or `NVML`);
    /// `false` when it falls back to a device-wide reading from `nvidia-smi`.
    pub is_per_process: bool,
    /// Which backend produced this measurement.
    pub source: GpuQuerySource,
}

/// The backend that produced a GPU memory measurement.
///
/// `#[non_exhaustive]`: more backends (e.g., AMD `ROCm` SMI, Apple Metal)
/// may be added.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuQuerySource {
    /// Windows `DXGI` per-process query (`IDXGIAdapter3::QueryVideoMemoryInfo`).
    Dxgi,
    /// `NVML` per-process query (`nvmlDeviceGetComputeRunningProcesses`).
    Nvml,
    /// `nvidia-smi` subprocess fallback (device-wide).
    NvidiaSmi,
}

/// Combined snapshot of process `RAM` and GPU memory state at a point in time.
///
/// Constructed via `Snapshot::now`. RAM measurement is mandatory; both GPU
/// fields are best-effort and set to `None` when no backend is usable.
///
/// `#[non_exhaustive]`: fields may be added in future releases.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct Snapshot {
    /// Process resident set size in bytes.
    pub ram_bytes: u64,
    /// Per-process GPU memory information for the requested device.
    /// `None` when no GPU source is usable.
    pub gpu: Option<ProcessGpuInfo>,
    /// Device-wide GPU information for the requested device.
    /// `None` when no GPU source is usable.
    pub gpu_device: Option<GpuDeviceInfo>,
}

impl Snapshot {
    /// Capture a fresh snapshot of process `RAM` and GPU memory for the given device index.
    ///
    /// `RAM` is always measured. GPU measurement failures are non-fatal —
    /// the corresponding fields are set to `None` rather than producing an error.
    ///
    /// # Errors
    ///
    /// Returns [`crate::HypomnesisError::Ram`] if the platform `RAM` query fails.
    /// Returns [`crate::HypomnesisError::Io`] if reading `/proc/self/status` fails on Linux.
    pub fn now(device_index: u32) -> Result<Self> {
        let ram_bytes = crate::ram::process_rss()?;
        let gpu = crate::gpu::process_gpu_info(device_index).ok();
        let gpu_device = crate::gpu::device_info(device_index).ok();
        Ok(Self {
            ram_bytes,
            gpu,
            gpu_device,
        })
    }
}

/// Convenience formatting helpers, available with `features = ["report"]`.
///
/// Located on `Snapshot` (rather than `MemoryReport`) for parity with
/// `candle-mi`'s `MemorySnapshot::ram_mb` / `vram_mb` API surface, so
/// candle-mi v0.2 can adopt `hypomnesis` with a thin adapter wrapper
/// rather than relocating the methods.
#[cfg(feature = "report")]
impl Snapshot {
    /// `RAM` (`RSS`) usage as megabytes (`bytes / 1_048_576`).
    #[must_use]
    pub fn ram_mb(&self) -> f64 {
        // CAST: u64 → f64, value is memory in bytes — fits in f64 mantissa
        // for any realistic process size (< 2^53 bytes ≈ 8 PiB).
        #[allow(clippy::cast_precision_loss, clippy::as_conversions)]
        let mb = self.ram_bytes as f64 / 1_048_576.0;
        mb
    }

    /// Per-process `VRAM` usage as megabytes, if available.
    ///
    /// Returns `None` when `gpu` is `None` (no GPU source succeeded).
    /// Reflects the dispatcher's mixed semantics: per-process when
    /// `DXGI` / `NVML` produced the value, device-wide when the
    /// `nvidia-smi` fallback was used (check `gpu.is_per_process`).
    #[must_use]
    pub fn vram_mb(&self) -> Option<f64> {
        // CAST: u64 → f64, same justification as ram_mb.
        #[allow(clippy::cast_precision_loss, clippy::as_conversions)]
        let mb = self.gpu.as_ref().map(|p| p.used_bytes as f64 / 1_048_576.0);
        mb
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

    /// Build a `Snapshot` for tests. Sets `gpu` only when `vram_used` is
    /// `Some`; sets `gpu_device` only when `total` is non-zero.
    fn make_snapshot(
        ram: u64,
        vram_used: Option<u64>,
        is_per_process: bool,
        total: u64,
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
            gpu_device: if total > 0 {
                Some(GpuDeviceInfo {
                    index: 0,
                    name: None,
                    total_bytes: total,
                    free_bytes: total.saturating_sub(vram_used.unwrap_or(0)),
                    used_bytes: vram_used.unwrap_or(0),
                })
            } else {
                None
            },
        }
    }

    #[test]
    fn snapshot_constructs_with_no_gpu() {
        let snap = make_snapshot(0, None, false, 0);
        assert_eq!(snap.ram_bytes, 0);
        assert!(snap.gpu.is_none());
        assert!(snap.gpu_device.is_none());
    }

    #[test]
    fn snapshot_constructs_with_full_gpu() {
        let snap = make_snapshot(1_048_576, Some(500 * 1_048_576), true, 16_384 * 1_048_576);
        assert_eq!(snap.ram_bytes, 1_048_576);
        assert_eq!(snap.gpu.as_ref().unwrap().used_bytes, 500 * 1_048_576);
        assert!(snap.gpu.as_ref().unwrap().is_per_process);
        assert_eq!(
            snap.gpu_device.as_ref().unwrap().total_bytes,
            16_384 * 1_048_576
        );
    }

    #[cfg(feature = "report")]
    #[test]
    fn ram_mb_conversion() {
        // exactly 1 MB
        let snap = make_snapshot(1_048_576, None, false, 0);
        assert!((snap.ram_mb() - 1.0).abs() < 0.001);
    }

    #[cfg(feature = "report")]
    #[test]
    fn ram_mb_zero() {
        let snap = make_snapshot(0, None, false, 0);
        assert!(snap.ram_mb().abs() < 0.001);
    }

    #[cfg(feature = "report")]
    #[test]
    fn vram_mb_none_when_no_gpu() {
        let snap = make_snapshot(100, None, false, 0);
        assert!(snap.vram_mb().is_none());
    }

    #[cfg(feature = "report")]
    #[test]
    fn vram_mb_some_when_gpu_present() {
        let snap = make_snapshot(100, Some(2 * 1_048_576), true, 16 * 1_048_576);
        assert!((snap.vram_mb().unwrap() - 2.0).abs() < 0.001);
    }
}
