// SPDX-License-Identifier: MIT OR Apache-2.0

//! `NVML` backend (cross-platform).
//!
//! Dynamically loads `libnvidia-ml.so.1` (Linux) or `nvml.dll` (Windows)
//! via `libloading` and calls `nvmlInit_v2` / `nvmlDeviceGetMemoryInfo` /
//! `nvmlDeviceGetComputeRunningProcesses_v3` / `nvmlShutdown`.
//!
//! Phase 1 scaffolding — real implementation lands in Wave 2 (port from
//! `candle-mi/src/memory.rs` lines 419-637).
