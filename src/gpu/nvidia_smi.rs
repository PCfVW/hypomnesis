// SPDX-License-Identifier: MIT OR Apache-2.0

//! `nvidia-smi` subprocess fallback (device-wide).
//!
//! Spawns `nvidia-smi --query-gpu=memory.used,memory.total --format=csv,noheader,nounits --id=N`
//! and parses the single CSV line. Slower than `NVML` / `DXGI` and reports
//! device-wide totals only, but works whenever the NVIDIA driver is
//! installed even if `NVML` cannot be loaded.
//!
//! Phase 1 scaffolding — real implementation lands in Wave 2 (port from
//! `candle-mi/src/memory.rs` lines 639-685).
