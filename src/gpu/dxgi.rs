// SPDX-License-Identifier: MIT OR Apache-2.0

//! `DXGI` backend for per-process `VRAM` on Windows.
//!
//! Walks `IDXGIFactory1::EnumAdapters1`, casts to `IDXGIAdapter3`, and
//! calls `QueryVideoMemoryInfo(DXGI_MEMORY_SEGMENT_GROUP_LOCAL)` for the
//! per-process current-usage figure. `WDDM`-aware: this is the only
//! reliable per-process `VRAM` source on Windows because the kernel memory
//! manager owns GPU allocations, not the NVIDIA driver.
//!
//! Phase 1 scaffolding — real implementation lands in Wave 2 (port from
//! `candle-mi/src/memory.rs` lines 687-777).
