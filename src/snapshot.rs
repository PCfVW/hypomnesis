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
