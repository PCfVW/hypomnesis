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
//! `device_index: u32` is interpreted as the index into the *filtered*
//! list of NVIDIA adapters with non-zero dedicated `VRAM`. iGPUs (Intel
//! `0x8086`, AMD integrated `0x1002`), the Microsoft Basic Render Driver
//! (`0x1414`), and any non-NVIDIA discrete GPUs are skipped during the
//! `EnumAdapters1` walk. Filter rule: `VendorId == 0x10DE` AND
//! `DedicatedVideoMemory > 0`.

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
