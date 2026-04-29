// SPDX-License-Identifier: MIT OR Apache-2.0

//! GPU memory measurement dispatchers and backend modules.
//!
//! Each backend (`nvml`, `dxgi`, `nvidia_smi`) is gated by a Cargo
//! feature; the dispatchers below try them in priority order and surface
//! the first success. Backend modules are crate-private — public access
//! is via the three dispatchers ([`device_count`], [`device_info`],
//! [`process_gpu_info`]).

use crate::{GpuDeviceInfo, HypomnesisError, ProcessGpuInfo, Result};

#[cfg(feature = "nvml")]
use crate::GpuQuerySource;

#[cfg(feature = "nvml")]
mod nvml;

#[cfg(all(windows, feature = "dxgi"))]
mod dxgi;

#[cfg(feature = "nvidia-smi-fallback")]
mod nvidia_smi;

/// Number of NVIDIA GPUs visible to `NVML` (`NVML`-canonical ordering).
///
/// On Windows the count uses `NVML`; if `NVML` is unavailable, the
/// `DXGI` fallback (Phase B+1) counts NVIDIA adapters with non-zero
/// dedicated `VRAM`.
///
/// # Errors
///
/// Returns [`HypomnesisError::Nvml`] if `NVML` fails to load or report a
/// count and no fallback succeeds, or [`HypomnesisError::NoGpuSource`]
/// if no measurement backend is enabled.
pub fn device_count() -> Result<u32> {
    #[cfg(feature = "nvml")]
    if let Some(count) = nvml::device_count() {
        return Ok(count);
    }

    // DXGI fallback wired in Phase B+1 (src/gpu/dxgi.rs port).

    Err(HypomnesisError::Nvml(
        "device_count: NVML unavailable and no fallback wired yet".into(),
    ))
}

/// Device-wide info for a specific GPU index (`NVML`-canonical ordering).
///
/// On Windows: `NVML` for `total` / `free` / `used` numerics, `DXGI` for
/// the adapter name (Phase B+1). `nvidia-smi` is the final fallback
/// (Phase B+1). iGPUs and the Microsoft Basic Render Driver are skipped.
///
/// # Errors
///
/// Returns [`HypomnesisError::DeviceIndexOutOfRange`] if `index` is past
/// the device count reported by `NVML`.
/// Returns [`HypomnesisError::NoGpuSource`] if no backend can satisfy
/// the query.
#[allow(unused_variables)] // `index` unused when no GPU backend feature is enabled
#[allow(clippy::missing_const_for_fn)] // const only when no features are enabled (body collapses)
pub fn device_info(index: u32) -> Result<GpuDeviceInfo> {
    #[cfg(feature = "nvml")]
    if let Some(snap) = nvml::query(index) {
        return Ok(GpuDeviceInfo {
            index,
            name: snap.device_name,
            total_bytes: snap.device_total,
            free_bytes: snap.device_free,
            used_bytes: snap.device_used,
        });
    }

    // DXGI and nvidia-smi fallbacks wired in Phase B+1.

    // If NVML can give us the count, surface a precise
    // DeviceIndexOutOfRange when `index` is past the count. Otherwise we
    // can't tell range-error from no-source-available; default to
    // NoGpuSource.
    #[cfg(feature = "nvml")]
    if let Some(count) = nvml::device_count()
        && index >= count
    {
        return Err(HypomnesisError::DeviceIndexOutOfRange { index, count });
    }

    Err(HypomnesisError::NoGpuSource)
}

/// Per-process GPU memory used by the calling process on the given device.
///
/// Tries (in order): `DXGI` on Windows (Phase B+1), `NVML`, then
/// `nvidia-smi` fallback (Phase B+1). The returned `ProcessGpuInfo`
/// carries an `is_per_process` flag and a `source` discriminator so
/// callers can distinguish a true per-process reading from a
/// device-wide fallback.
///
/// # Errors
///
/// Returns [`HypomnesisError::DeviceIndexOutOfRange`] if `device_index`
/// is past the device count reported by `NVML`.
/// Returns [`HypomnesisError::NoGpuSource`] if every available backend fails.
#[allow(unused_variables)] // `device_index` unused when no GPU backend feature is enabled
#[allow(clippy::missing_const_for_fn)] // const only when no features are enabled (body collapses)
pub fn process_gpu_info(device_index: u32) -> Result<ProcessGpuInfo> {
    // DXGI primary on Windows wired in Phase B+1.

    #[cfg(feature = "nvml")]
    if let Some(snap) = nvml::query(device_index)
        && let Some(used) = snap.process_used_bytes
    {
        return Ok(ProcessGpuInfo {
            used_bytes: used,
            is_per_process: true,
            source: GpuQuerySource::Nvml,
        });
    }

    // nvidia-smi fallback wired in Phase B+1.

    #[cfg(feature = "nvml")]
    if let Some(count) = nvml::device_count()
        && device_index >= count
    {
        return Err(HypomnesisError::DeviceIndexOutOfRange {
            index: device_index,
            count,
        });
    }

    Err(HypomnesisError::NoGpuSource)
}
