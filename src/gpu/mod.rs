// SPDX-License-Identifier: MIT OR Apache-2.0

//! GPU memory measurement dispatchers and backend modules.
//!
//! Each backend (`nvml`, `dxgi`, `nvidia_smi`) is gated by a Cargo
//! feature; the dispatchers below try them in priority order and surface
//! the first success. Backend modules are crate-private — public access
//! is via the four dispatchers ([`device_count`], [`device_info`],
//! [`process_gpu_info`], [`gpu_processes`]).

use crate::{GpuDeviceInfo, GpuProcessEntry, HypomnesisError, ProcessGpuInfo, Result};

#[cfg(any(
    feature = "nvml",
    all(windows, feature = "dxgi"),
    feature = "nvidia-smi-fallback"
))]
use crate::GpuQuerySource;

#[cfg(feature = "nvml")]
mod nvml;

#[cfg(all(windows, feature = "dxgi"))]
mod dxgi;

#[cfg(feature = "nvidia-smi-fallback")]
mod nvidia_smi;

#[cfg(all(target_os = "macos", feature = "metal"))]
mod metal;

/// Number of NVIDIA GPUs visible to `NVML` (`NVML`-canonical ordering).
///
/// On Windows the count uses `NVML`; if `NVML` is unavailable, the
/// `DXGI` fallback counts NVIDIA adapters with non-zero dedicated `VRAM`.
///
/// # Errors
///
/// Returns [`HypomnesisError::NoGpuSource`] if no enumeration backend
/// is enabled, or if every enabled backend failed to report a count.
#[allow(clippy::missing_const_for_fn)] // const only when no features are enabled (body collapses)
pub fn device_count() -> Result<u32> {
    #[cfg(feature = "nvml")]
    if let Some(count) = nvml::device_count() {
        return Ok(count);
    }

    #[cfg(all(windows, feature = "dxgi"))]
    if let Some(count) = dxgi::device_count() {
        return Ok(count);
    }

    // nvidia-smi fallback for device_count is intentionally not wired —
    // counting via `nvidia-smi -L` is more brittle than NVML/DXGI and
    // adds a subprocess invocation for what's typically a metadata call.

    Err(HypomnesisError::NoGpuSource)
}

/// Device-wide info for a specific GPU index (`NVML`-canonical ordering).
///
/// Source priority:
/// 1. `NVML` for `total` / `free` / `used` numerics, augmented with
///    `DXGI`'s `Description`-derived `name` on Windows when available.
/// 2. `DXGI` alone (Windows only): falls back when `NVML` is
///    unavailable. **Imprecision note:** in this path `used_bytes`
///    is set to DXGI's `CurrentUsage`, which is per-process — not the
///    device-wide sum. Treat it as a lower bound. This path is rare
///    (it requires `NVML` to fail while `DXGI` works, e.g. partial
///    driver installs).
/// 3. `nvidia-smi` subprocess fallback (Phase B+1) — device-wide proper.
///
/// iGPUs and the Microsoft Basic Render Driver are skipped during the
/// `DXGI` adapter walk (filtered by NVIDIA vendor ID `0x10DE` and
/// non-zero dedicated `VRAM`).
///
/// # Errors
///
/// Returns [`HypomnesisError::DeviceIndexOutOfRange`] if `index` is past
/// the device count reported by `NVML` or `DXGI`.
/// Returns [`HypomnesisError::NoGpuSource`] if no backend can satisfy
/// the query.
#[allow(unused_variables)] // `index` unused when no GPU backend feature is enabled
#[allow(clippy::missing_const_for_fn)] // const only when no features are enabled (body collapses)
pub fn device_info(index: u32) -> Result<GpuDeviceInfo> {
    #[cfg(feature = "nvml")]
    if let Some(snap) = nvml::query(index) {
        #[cfg(all(windows, feature = "dxgi"))]
        let name = dxgi::adapter_name(index).or(snap.device_name);
        #[cfg(not(all(windows, feature = "dxgi")))]
        let name = snap.device_name;

        return Ok(GpuDeviceInfo {
            index,
            name,
            total_bytes: snap.device_total,
            free_bytes: snap.device_free,
            used_bytes: snap.device_used,
        });
    }

    // DXGI-alone fallback (Windows only). Loose semantics: CurrentUsage
    // is per-process; treated here as a lower bound on device-wide used.
    #[cfg(all(windows, feature = "dxgi"))]
    if let Some(d) = dxgi::query(index) {
        return Ok(GpuDeviceInfo {
            index,
            name: d.adapter_name,
            total_bytes: d.dedicated_video_memory,
            free_bytes: d.dedicated_video_memory.saturating_sub(d.current_usage),
            used_bytes: d.current_usage,
        });
    }

    // nvidia-smi fallback (device-wide proper, no name).
    #[cfg(feature = "nvidia-smi-fallback")]
    if let Some(result) = nvidia_smi::query(index) {
        return Ok(GpuDeviceInfo {
            index,
            name: None,
            total_bytes: result.total_bytes,
            free_bytes: result.total_bytes.saturating_sub(result.used_bytes),
            used_bytes: result.used_bytes,
        });
    }

    bounds_check(index)?;
    Err(HypomnesisError::NoGpuSource)
}

/// Per-process GPU memory used by the calling process on the given device.
///
/// Source priority:
/// 1. `DXGI` on Windows — the only WDDM-aware per-process source.
/// 2. `NVML` (Linux primary; on Windows it returns `NVML_VALUE_NOT_AVAILABLE`
///    for compute processes under WDDM, so this path is effectively Linux-only).
/// 3. `nvidia-smi` device-wide fallback (Phase B+1) — sets
///    `is_per_process = false` because `nvidia-smi` cannot break the
///    figure down per process.
///
/// # Errors
///
/// Returns [`HypomnesisError::DeviceIndexOutOfRange`] if `device_index`
/// is past the device count reported by `NVML` or `DXGI`.
/// Returns [`HypomnesisError::NoGpuSource`] if every available backend fails.
#[allow(unused_variables)] // `device_index` unused when no GPU backend feature is enabled
#[allow(clippy::missing_const_for_fn)] // const only when no features are enabled (body collapses)
pub fn process_gpu_info(device_index: u32) -> Result<ProcessGpuInfo> {
    #[cfg(all(windows, feature = "dxgi"))]
    if let Some(d) = dxgi::query(device_index) {
        return Ok(ProcessGpuInfo {
            used_bytes: d.current_usage,
            is_per_process: true,
            source: GpuQuerySource::Dxgi,
        });
    }

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

    // nvidia-smi fallback — device-wide reading (`is_per_process = false`).
    #[cfg(feature = "nvidia-smi-fallback")]
    if let Some(result) = nvidia_smi::query(device_index) {
        return Ok(ProcessGpuInfo {
            used_bytes: result.used_bytes,
            is_per_process: false,
            source: GpuQuerySource::NvidiaSmi,
        });
    }

    bounds_check(device_index)?;
    Err(HypomnesisError::NoGpuSource)
}

/// Convert each non-NVIDIA `DXGI` adapter into a `(GpuDeviceInfo, ProcessGpuInfo)`
/// pair, ready to be wrapped in a `Snapshot` by [`crate::Snapshot::all`].
///
/// Indices are assigned sequentially starting at `starting_index` so that
/// the NVIDIA portion of `Snapshot::all()` (`NVML`-canonical 0..N-1) and
/// the non-NVIDIA portion (N, N+1, …) form a contiguous index space.
///
/// `total_bytes` is the adapter's `DedicatedVideoMemory` when non-zero
/// (matches what dGPUs and UMA-allocated iGPUs expose), otherwise
/// `SharedSystemMemory` (`WDDM` shared budget — the right number for
/// iGPUs without UMA). The semantics of `total_bytes` therefore differ
/// subtly between dGPUs and iGPUs; the `Snapshot::all` rustdoc flags
/// this for callers.
///
/// `is_per_process` is `true` because `DXGI`'s `CurrentUsage` is
/// `WDDM`-aware and reports the calling process's own usage, not a
/// device-wide sum.
#[cfg(all(windows, feature = "dxgi"))]
#[must_use]
pub(crate) fn dxgi_non_nvidia_devices(starting_index: u32) -> Vec<(GpuDeviceInfo, ProcessGpuInfo)> {
    dxgi::enumerate_non_nvidia()
        .into_iter()
        .enumerate()
        .map(|(offset, entry)| {
            // CAST: usize → u32, offset is bounded by the DXGI adapter
            // count (handfuls in practice); never approaches u32::MAX.
            #[allow(clippy::as_conversions, clippy::cast_possible_truncation)]
            let index = starting_index.saturating_add(offset as u32);

            let total_bytes = if entry.dedicated_video_memory > 0 {
                entry.dedicated_video_memory
            } else {
                entry.shared_system_memory
            };
            let used_bytes = entry.current_usage;
            let free_bytes = total_bytes.saturating_sub(used_bytes);

            (
                GpuDeviceInfo {
                    index,
                    name: entry.adapter_name,
                    total_bytes,
                    free_bytes,
                    used_bytes,
                },
                ProcessGpuInfo {
                    used_bytes,
                    is_per_process: true,
                    source: GpuQuerySource::Dxgi,
                },
            )
        })
        .collect()
}

/// List every compute process holding GPU memory on the given device.
///
/// Returns one [`GpuProcessEntry`] per running process that has an
/// active `CUDA` context. Empty `Vec` when the device exists but no
/// compute processes are using it.
///
/// # Source priority
///
/// 1. `NVML` (Linux primary). `nvmlDeviceGetComputeRunningProcesses_v3`
///    yields `(pid, used_bytes)`; `/proc/<pid>/comm` supplies names on
///    Linux. Capped at 64 processes per device — the existing `NVML`
///    stack-buffer size. Per-row sentinel and `used > total` checks
///    mirror the library's other `NVML` consumers; offending rows are
///    dropped rather than reported as garbage.
/// 2. `nvidia-smi` (Windows primary, Linux fallback) — subprocess
///    `nvidia-smi --query-compute-apps=pid,process_name,used_memory --format=csv,noheader,nounits --id=N`.
///    On Windows under `WDDM`, `NVML`'s per-process call returns
///    `NVML_VALUE_NOT_AVAILABLE`, so `nvidia-smi` is the only path that
///    surfaces other-PID names.
/// 3. `DXGI` is **not** used — `IDXGIAdapter3::QueryVideoMemoryInfo`
///    only answers for the calling process and cannot enumerate other
///    PIDs.
///
/// # Limitations
///
/// **Compute-only.** Both backends only see processes with an active
/// `CUDA` context. Browsers using GPU compositing, games, and
/// pure-graphics apps do not appear here.
///
/// **Windows process names may be `?`.** `nvidia-smi` writes a literal
/// `?` for processes whose image name it cannot read (protected
/// processes); preserved as `Some("?")` in the returned entry.
///
/// # Errors
///
/// Returns [`HypomnesisError::DeviceIndexOutOfRange`] if `device_index`
/// is past the device count reported by `NVML` or `DXGI`.
/// Returns [`HypomnesisError::NoGpuSource`] if every available backend
/// fails (or no backend is enabled by features).
#[allow(unused_variables)] // `device_index` unused when no GPU backend feature is enabled
#[allow(clippy::missing_const_for_fn)] // const only when no features are enabled
pub fn gpu_processes(device_index: u32) -> Result<Vec<GpuProcessEntry>> {
    #[cfg(feature = "nvml")]
    if let Some(rows) = nvml::list_compute_processes(device_index) {
        let entries: Vec<GpuProcessEntry> = rows
            .into_iter()
            .map(|(pid, used_bytes)| {
                #[cfg(target_os = "linux")]
                let name = read_proc_comm(pid);
                #[cfg(not(target_os = "linux"))]
                let name = None;
                GpuProcessEntry {
                    pid,
                    name,
                    used_bytes,
                    source: GpuQuerySource::Nvml,
                }
            })
            .collect();
        return Ok(entries);
    }

    #[cfg(feature = "nvidia-smi-fallback")]
    if let Some(rows) = nvidia_smi::query_compute_apps(device_index) {
        let entries: Vec<GpuProcessEntry> = rows
            .into_iter()
            .map(|app| GpuProcessEntry {
                pid: app.pid,
                name: app.name,
                used_bytes: app.used_bytes,
                source: GpuQuerySource::NvidiaSmi,
            })
            .collect();
        return Ok(entries);
    }

    bounds_check(device_index)?;
    Err(HypomnesisError::NoGpuSource)
}

/// Read `/proc/<pid>/comm` (Linux only), returning the trimmed
/// executable name if available.
///
/// `/proc/<pid>/comm` is a one-line file containing the process's
/// `comm` (executable name, truncated to 15 characters by the kernel),
/// terminated by a newline. World-readable on standard kernels;
/// returns `None` on any read or trim failure (process exited, perms
/// stripped, etc.) since name resolution is best-effort.
#[cfg(all(target_os = "linux", feature = "nvml"))]
fn read_proc_comm(pid: u32) -> Option<String> {
    let path = format!("/proc/{pid}/comm");
    let content = std::fs::read_to_string(&path).ok()?;
    // BORROW: trim_end_matches + to_owned — kernel writes a trailing
    // newline; we want an owned String without it.
    let trimmed = content.trim_end_matches('\n').trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

/// Bounds-check `index` against whatever count source is available.
///
/// Tries `NVML` first; on Windows, falls back to `DXGI` if `NVML` is
/// unavailable. Returns `Ok(())` when no count source is available
/// (caller will surface its own error, typically `NoGpuSource`).
///
/// # Errors
///
/// Returns [`HypomnesisError::DeviceIndexOutOfRange`] when a count
/// source reports a count and `index >= count`.
#[allow(unused_variables)] // unused when no backend feature is enabled
#[allow(clippy::missing_const_for_fn)] // const only when no features are enabled
#[allow(clippy::unnecessary_wraps)] // Result is necessary only when nvml or dxgi feature returns Err
fn bounds_check(index: u32) -> Result<()> {
    #[cfg(feature = "nvml")]
    if let Some(count) = nvml::device_count() {
        return if index >= count {
            Err(HypomnesisError::DeviceIndexOutOfRange { index, count })
        } else {
            Ok(())
        };
    }

    #[cfg(all(windows, feature = "dxgi"))]
    if let Some(count) = dxgi::device_count() {
        return if index >= count {
            Err(HypomnesisError::DeviceIndexOutOfRange { index, count })
        } else {
            Ok(())
        };
    }

    Ok(())
}
