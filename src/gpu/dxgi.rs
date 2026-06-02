// SPDX-License-Identifier: MIT OR Apache-2.0

//! `DXGI` backend for per-process `VRAM` on Windows.
//!
//! Walks `IDXGIFactory1::EnumAdapters1`, filters NVIDIA adapters by
//! PCI vendor ID, casts to `IDXGIAdapter3`, and calls
//! `QueryVideoMemoryInfo(DXGI_MEMORY_SEGMENT_GROUP_LOCAL)` for the
//! per-process `CurrentUsage` figure. `WDDM`-aware: this is the only
//! reliable per-process `VRAM` source on Windows because the kernel
//! memory manager owns GPU allocations under `WDDM`, not the NVIDIA
//! driver — `NVML`'s `nvmlDeviceGetComputeRunningProcesses_v3` returns
//! `NVML_VALUE_NOT_AVAILABLE` for compute processes on `WDDM`.
//!
//! # Adapter filtering
//!
//! Two enumeration paths coexist with opposite filters:
//!
//! - **NVIDIA-only walk** ([`query`], [`adapter_name`], [`device_count`]).
//!   `device_index: u32` is the index into the *filtered* list of NVIDIA
//!   adapters with non-zero dedicated `VRAM`. iGPUs (Intel `0x8086`, AMD
//!   integrated `0x1002`), the Microsoft Basic Render Driver (`0x1414`),
//!   and any non-NVIDIA discrete GPUs are skipped during the
//!   `EnumAdapters1` walk. Filter rule:
//!   `VendorId == 0x10DE` AND `DedicatedVideoMemory > 0`.
//!
//! - **Non-NVIDIA walk** ([`enumerate_non_nvidia`], used by
//!   `Snapshot::all` on Windows). Reverses the NVIDIA filter to surface
//!   AMD / Intel iGPUs and any non-NVIDIA discrete GPUs alongside the
//!   NVIDIA dGPU(s) `NVML` enumerates. Filter rule:
//!   `VendorId != 0x10DE` AND `VendorId != 0x1414` AND
//!   (`DedicatedVideoMemory > 0` OR `SharedSystemMemory > 0`).

use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, DXGI_QUERY_VIDEO_MEMORY_INFO,
    IDXGIAdapter, IDXGIAdapter3, IDXGIFactory1,
};
use windows::core::Interface;

/// PCI vendor ID for NVIDIA Corporation.
///
/// Used to filter `EnumAdapters1` results so `device_index` counts only
/// NVIDIA adapters, matching `NVML`'s enumeration view.
const NVIDIA_VENDOR_ID: u32 = 0x10DE;

/// PCI vendor ID for the Microsoft Basic Render Driver.
///
/// `MSBR` is the synthetic adapter every Windows install ships with — it
/// has no underlying GPU memory. Skipped during the
/// [`enumerate_non_nvidia`] walk so it never surfaces in `Snapshot::all()`.
const MICROSOFT_BASIC_VENDOR_ID: u32 = 0x1414;

/// Combined result of a single `DXGI` query for a given (NVIDIA-filtered) adapter index.
///
/// Returned by [`query`].
pub(super) struct DxgiQueryResult {
    /// Per-process VRAM usage in bytes (`CurrentUsage` from `DXGI_QUERY_VIDEO_MEMORY_INFO`).
    /// This is the calling process's own GPU memory consumption — DXGI is
    /// WDDM-aware, so this number is reliable under Windows.
    pub current_usage: u64,
    /// Total dedicated VRAM on the adapter in bytes
    /// (`DedicatedVideoMemory` from `DXGI_ADAPTER_DESC`).
    pub dedicated_video_memory: u64,
    /// Adapter name parsed from the UTF-16 `DXGI_ADAPTER_DESC.Description` field.
    /// `None` when the name is empty after trimming trailing nulls.
    pub adapter_name: Option<String>,
}

/// Run a single `DXGI` query for the (NVIDIA-filtered) adapter at `idx`.
///
/// Walks `EnumAdapters1` until the `idx`-th NVIDIA adapter with non-zero
/// dedicated VRAM is found, then casts to `IDXGIAdapter3` and queries
/// `DXGI_MEMORY_SEGMENT_GROUP_LOCAL`. Returns `None` if `idx` is past
/// the count of qualifying adapters, or if any DXGI call fails.
#[allow(unsafe_code)]
pub(super) fn query(idx: u32) -> Option<DxgiQueryResult> {
    // SAFETY: CreateDXGIFactory1 is a documented COM factory function.
    // It initializes COM internally if needed; the returned IDXGIFactory1
    // is reference-counted and released when `factory` is dropped.
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }.ok()?;

    let mut raw_idx: u32 = 0;
    let mut nvidia_count: u32 = 0;

    loop {
        // SAFETY: EnumAdapters1 returns Err when raw_idx is past the
        // last enumerated adapter; the .ok()? converts that into the
        // function returning None (no more adapters).
        let adapter1 = unsafe { factory.EnumAdapters1(raw_idx) }.ok()?;
        let adapter: IDXGIAdapter = adapter1.cast().ok()?;

        // SAFETY: GetDesc fills DXGI_ADAPTER_DESC. The adapter handle is
        // valid (just acquired above with EnumAdapters1 returning S_OK).
        let desc = unsafe { adapter.GetDesc() }.ok()?;

        if desc.VendorId == NVIDIA_VENDOR_ID && desc.DedicatedVideoMemory > 0 {
            if nvidia_count == idx {
                // CAST: usize → u64, DedicatedVideoMemory bounded by GPU memory; fits in u64
                #[allow(clippy::as_conversions)]
                let total = desc.DedicatedVideoMemory as u64;

                // BORROW: explicit String::from_utf16_lossy + trim_end_matches +
                // to_owned — desc.Description is fixed-size [u16; 128]; we trim
                // trailing NULs and need an owned String to return.
                let raw_name = String::from_utf16_lossy(&desc.Description);
                let trimmed = raw_name.trim_end_matches('\0');
                let adapter_name = if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_owned())
                };

                let adapter3: IDXGIAdapter3 = adapter.cast().ok()?;
                let mut mem_info = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
                // SAFETY: QueryVideoMemoryInfo fills DXGI_QUERY_VIDEO_MEMORY_INFO.
                // Node 0 = primary GPU node. DXGI_MEMORY_SEGMENT_GROUP_LOCAL =
                // dedicated VRAM segment on discrete GPUs.
                unsafe {
                    adapter3.QueryVideoMemoryInfo(
                        0,
                        DXGI_MEMORY_SEGMENT_GROUP_LOCAL,
                        &raw mut mem_info,
                    )
                }
                .ok()?;

                #[cfg(feature = "debug-output")]
                eprintln!(
                    "[DXGI debug] adapter[nvidia#{idx} raw#{raw_idx}]: name={adapter_name:?}, \
                     dedicated_vram={total}, current_usage={}, budget={}",
                    mem_info.CurrentUsage, mem_info.Budget
                );

                return Some(DxgiQueryResult {
                    current_usage: mem_info.CurrentUsage,
                    dedicated_video_memory: total,
                    adapter_name,
                });
            }
            nvidia_count += 1;
        }

        raw_idx += 1;
    }
}

/// Lightweight adapter-name lookup that skips `QueryVideoMemoryInfo`.
///
/// Used by the `device_info` dispatcher to augment `NVML`'s name with
/// the friendlier `DXGI` `Description` string without paying for a full
/// `query`. Returns `None` if `idx` is past the NVIDIA-adapter count or
/// any DXGI call fails.
#[allow(unsafe_code)]
pub(super) fn adapter_name(idx: u32) -> Option<String> {
    // SAFETY: same justification as in `query`.
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }.ok()?;

    let mut raw_idx: u32 = 0;
    let mut nvidia_count: u32 = 0;

    loop {
        // SAFETY: EnumAdapters1 returns Err when raw_idx is out of range.
        let adapter1 = unsafe { factory.EnumAdapters1(raw_idx) }.ok()?;
        let adapter: IDXGIAdapter = adapter1.cast().ok()?;

        // SAFETY: GetDesc fills DXGI_ADAPTER_DESC. Adapter handle valid.
        let desc = unsafe { adapter.GetDesc() }.ok()?;

        if desc.VendorId == NVIDIA_VENDOR_ID && desc.DedicatedVideoMemory > 0 {
            if nvidia_count == idx {
                // BORROW: explicit String::from_utf16_lossy + trim + to_owned —
                // desc.Description is [u16; 128]; need owned String to return.
                let raw_name = String::from_utf16_lossy(&desc.Description);
                let trimmed = raw_name.trim_end_matches('\0');
                return if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_owned())
                };
            }
            nvidia_count += 1;
        }

        raw_idx += 1;
    }
}

/// `LUID` of the `idx`-th NVIDIA-filtered adapter, as `(high, low)`.
///
/// Walks `EnumAdapters1` with the same NVIDIA-filter rule as [`query`]
/// (vendor ID `0x10DE` + non-zero `DedicatedVideoMemory`) and returns
/// the `(HighPart, LowPart)` pair of the adapter's `LUID` from
/// `DXGI_ADAPTER_DESC.AdapterLuid`. Returns `None` if `idx` is past the
/// count of qualifying adapters, or if any `DXGI` call fails.
///
/// Consumed by [`crate::gpu::pdh`] to filter PDH counter instances of
/// the form `pid_NNNN_luid_0xHHHHHHHH_0xHHHHHHHH_phys_N` down to those
/// belonging to the target adapter. The `LUID` is the stable kernel-side
/// identifier of a `WDDM` adapter — preserved across reboots until
/// driver-level reconfiguration.
#[allow(unsafe_code)]
pub(super) fn adapter_luid(idx: u32) -> Option<(i32, u32)> {
    // SAFETY: CreateDXGIFactory1 is a documented COM factory function.
    // It initializes COM internally if needed; the returned IDXGIFactory1
    // is reference-counted and released when `factory` is dropped.
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }.ok()?;

    let mut raw_idx: u32 = 0;
    let mut nvidia_count: u32 = 0;

    loop {
        // SAFETY: EnumAdapters1 returns Err when raw_idx is out of range.
        let adapter1 = unsafe { factory.EnumAdapters1(raw_idx) }.ok()?;
        let adapter: IDXGIAdapter = adapter1.cast().ok()?;

        // SAFETY: GetDesc fills DXGI_ADAPTER_DESC. Adapter handle valid.
        let desc = unsafe { adapter.GetDesc() }.ok()?;

        if desc.VendorId == NVIDIA_VENDOR_ID && desc.DedicatedVideoMemory > 0 {
            if nvidia_count == idx {
                return Some((desc.AdapterLuid.HighPart, desc.AdapterLuid.LowPart));
            }
            nvidia_count += 1;
        }

        raw_idx += 1;
    }
}

/// One non-NVIDIA, non-`MSBR` `DXGI` adapter that exposes some form of GPU memory.
///
/// Returned by [`enumerate_non_nvidia`] for `Snapshot::all()` on Windows
/// to surface AMD / Intel iGPUs alongside the NVIDIA dGPU(s) `NVML`
/// already enumerates. NVIDIA adapters are intentionally excluded from
/// this enumeration — `NVML` is the authoritative source for them
/// (correct device-wide totals, driver-side `free` figure that accounts
/// for reservation / alignment).
pub(super) struct DxgiAdapterEntry {
    /// Adapter name parsed from `DXGI_ADAPTER_DESC.Description`.
    /// `None` when the description is empty after trimming trailing nulls.
    pub adapter_name: Option<String>,
    /// Per-process VRAM usage in bytes — `CurrentUsage` of the
    /// `DXGI_MEMORY_SEGMENT_GROUP_LOCAL` segment. Zero when the adapter
    /// does not expose `IDXGIAdapter3` or `QueryVideoMemoryInfo` fails;
    /// not an error condition (best-effort).
    pub current_usage: u64,
    /// `DXGI_ADAPTER_DESC.DedicatedVideoMemory` — dedicated `VRAM` in
    /// bytes. Non-zero on dGPUs and on iGPUs with BIOS-allocated UMA
    /// chunks; zero on iGPUs without UMA.
    pub dedicated_video_memory: u64,
    /// `DXGI_ADAPTER_DESC.SharedSystemMemory` — `WDDM` shared-memory
    /// budget in bytes (the OS-managed slice of system RAM the GPU may
    /// commit). Non-zero on every real adapter; useful as the
    /// `total_bytes` fallback for iGPUs without a dedicated chunk.
    pub shared_system_memory: u64,
}

/// Walk every `DXGI` adapter, returning one entry per non-NVIDIA, non-`MSBR`
/// adapter that exposes any GPU-accessible memory.
///
/// Filter rule: `VendorId != 0x10DE` (NVIDIA, handled by `NVML`) AND
/// `VendorId != 0x1414` (`MSBR`) AND
/// (`DedicatedVideoMemory > 0` OR `SharedSystemMemory > 0`).
///
/// Used by `Snapshot::all()`. Returns an empty `Vec` if `DXGI` itself
/// fails to load, or the system has no qualifying non-NVIDIA adapters.
#[allow(unsafe_code)]
#[must_use]
pub(super) fn enumerate_non_nvidia() -> Vec<DxgiAdapterEntry> {
    // SAFETY: same justification as in `query`.
    let Ok(factory) = (unsafe { CreateDXGIFactory1::<IDXGIFactory1>() }) else {
        return Vec::new();
    };

    let mut out: Vec<DxgiAdapterEntry> = Vec::new();
    let mut raw_idx: u32 = 0;

    loop {
        // SAFETY: EnumAdapters1 returns Err past the last enumerated
        // adapter; we treat that as the natural end-of-walk and break.
        let Ok(adapter1) = (unsafe { factory.EnumAdapters1(raw_idx) }) else {
            break;
        };
        let Ok(adapter) = adapter1.cast::<IDXGIAdapter>() else {
            break;
        };
        // SAFETY: GetDesc fills DXGI_ADAPTER_DESC; adapter handle valid.
        let Ok(desc) = (unsafe { adapter.GetDesc() }) else {
            break;
        };

        // CAST: usize → u64, DedicatedVideoMemory and SharedSystemMemory
        // are usize in DXGI_ADAPTER_DESC; on 64-bit Windows they fit in u64.
        #[allow(clippy::as_conversions)]
        let dedicated = desc.DedicatedVideoMemory as u64;
        // CAST: usize → u64, same justification.
        #[allow(clippy::as_conversions)]
        let shared = desc.SharedSystemMemory as u64;

        let qualifies = desc.VendorId != NVIDIA_VENDOR_ID
            && desc.VendorId != MICROSOFT_BASIC_VENDOR_ID
            && (dedicated > 0 || shared > 0);

        if qualifies {
            // BORROW: explicit String::from_utf16_lossy + trim_end_matches
            // + to_owned — desc.Description is fixed-size [u16; 128] and
            // we need an owned String to return.
            let raw_name = String::from_utf16_lossy(&desc.Description);
            let trimmed = raw_name.trim_end_matches('\0');
            let adapter_name = if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_owned())
            };

            // Per-process LOCAL CurrentUsage. Best-effort: an adapter
            // that doesn't implement IDXGIAdapter3, or where the query
            // fails, contributes 0 here. The dispatcher records the
            // 0 as a per-process reading, not as an error.
            let current_usage = adapter
                .cast::<IDXGIAdapter3>()
                .ok()
                .and_then(|a3| {
                    let mut info = DXGI_QUERY_VIDEO_MEMORY_INFO::default();
                    // SAFETY: QueryVideoMemoryInfo fills the caller-provided
                    // DXGI_QUERY_VIDEO_MEMORY_INFO. Node 0 = primary GPU
                    // node; LOCAL = dedicated VRAM segment.
                    unsafe {
                        a3.QueryVideoMemoryInfo(0, DXGI_MEMORY_SEGMENT_GROUP_LOCAL, &raw mut info)
                    }
                    .ok()
                    .map(|()| info.CurrentUsage)
                })
                .unwrap_or(0);

            #[cfg(feature = "debug-output")]
            eprintln!(
                "[DXGI debug] non-nvidia adapter[raw#{raw_idx}]: vendor={:#x}, \
                 name={adapter_name:?}, dedicated={dedicated}, shared={shared}, \
                 current_usage={current_usage}",
                desc.VendorId
            );

            out.push(DxgiAdapterEntry {
                adapter_name,
                current_usage,
                dedicated_video_memory: dedicated,
                shared_system_memory: shared,
            });
        }

        raw_idx += 1;
    }

    out
}

/// Number of NVIDIA adapters with non-zero dedicated VRAM visible to `DXGI`.
///
/// Used as the `device_count` fallback when `NVML` is unavailable, and
/// for bounds-checking `idx` on Windows in `device_info` /
/// `process_gpu_info` when both `NVML` and `nvidia-smi` are absent.
#[allow(unsafe_code)]
pub(super) fn device_count() -> Option<u32> {
    // SAFETY: same justification as in `query`.
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }.ok()?;

    let mut raw_idx: u32 = 0;
    let mut count: u32 = 0;

    loop {
        // SAFETY: EnumAdapters1 returns Err when raw_idx is past the last
        // adapter; we treat that as the natural end of the walk.
        let Ok(adapter1) = (unsafe { factory.EnumAdapters1(raw_idx) }) else {
            break;
        };
        let Ok(adapter) = adapter1.cast::<IDXGIAdapter>() else {
            break;
        };
        // SAFETY: GetDesc fills DXGI_ADAPTER_DESC. Adapter handle valid.
        let Ok(desc) = (unsafe { adapter.GetDesc() }) else {
            break;
        };
        if desc.VendorId == NVIDIA_VENDOR_ID && desc.DedicatedVideoMemory > 0 {
            count += 1;
        }
        raw_idx += 1;
    }

    #[cfg(feature = "debug-output")]
    eprintln!("[DXGI debug] device_count = {count} (NVIDIA adapters with non-zero VRAM)");

    Some(count)
}
