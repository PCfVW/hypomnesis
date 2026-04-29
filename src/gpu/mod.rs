// SPDX-License-Identifier: MIT OR Apache-2.0

//! GPU memory measurement dispatchers and backend modules.
//!
//! Each backend (`nvml`, `dxgi`, `nvidia_smi`) is gated by a Cargo
//! feature; the dispatchers below try them in priority order and surface
//! the first success.

use crate::{GpuDeviceInfo, HypomnesisError, ProcessGpuInfo, Result};

#[cfg(feature = "nvml")]
pub mod nvml;

#[cfg(all(windows, feature = "dxgi"))]
pub mod dxgi;

#[cfg(feature = "nvidia-smi-fallback")]
pub mod nvidia_smi;

/// Number of NVIDIA GPUs visible to `NVML` (`NVML`-canonical ordering).
///
/// On Windows the count uses `NVML`; if `NVML` is unavailable, falls back
/// to counting `DXGI` adapters with non-zero dedicated `VRAM`.
///
/// # Errors
///
/// Returns [`HypomnesisError::Nvml`] if `NVML` fails to load or report a count,
/// or [`HypomnesisError::NoGpuSource`] if no measurement backend is enabled.
pub fn device_count() -> Result<u32> {
    // Phase 1 scaffolding — real implementation lands in Wave 2.
    Err(HypomnesisError::Nvml(
        "device_count not yet implemented (Phase 1 scaffolding)".into(),
    ))
}

/// Device-wide info for a specific GPU index (`NVML`-canonical ordering).
///
/// On Windows: `NVML` if available; otherwise `DXGI`'s first NVIDIA adapter
/// with non-zero dedicated `VRAM`. iGPUs and the Microsoft Basic Render
/// Driver are skipped.
///
/// # Errors
///
/// Returns [`HypomnesisError::DeviceIndexOutOfRange`] if `index` exceeds the device count.
/// Returns [`HypomnesisError::NoGpuSource`] if no backend can satisfy the query.
pub fn device_info(_index: u32) -> Result<GpuDeviceInfo> {
    // Phase 1 scaffolding — real implementation lands in Wave 2.
    Err(HypomnesisError::Nvml(
        "device_info not yet implemented (Phase 1 scaffolding)".into(),
    ))
}

/// Per-process GPU memory used by the calling process on the given device.
///
/// Tries (in order): `DXGI` on Windows, `NVML`, then `nvidia-smi` fallback.
/// The returned `ProcessGpuInfo` carries an `is_per_process` flag and a
/// `source` discriminator so callers can distinguish a true per-process
/// reading from a device-wide fallback.
///
/// # Errors
///
/// Returns [`HypomnesisError::NoGpuSource`] if every available backend fails.
#[allow(clippy::missing_const_for_fn)] // Wave 2 impl will do FFI dispatch (not const)
pub fn process_gpu_info(_device_index: u32) -> Result<ProcessGpuInfo> {
    // Phase 1 scaffolding — real implementation lands in Wave 2.
    Err(HypomnesisError::NoGpuSource)
}
