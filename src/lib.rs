// SPDX-License-Identifier: MIT OR Apache-2.0

//! # hypomnesis
//!
//! External measurement of a Rust process's `RAM` and `VRAM` state, on Windows and Linux.
//!
//! `hypomnesis` reports what's currently in a process's memory — process `RSS`,
//! device-wide GPU memory, and per-process GPU `VRAM` — without depending on
//! `candle`, `cudarc`, `sysinfo`, or any inference framework.
//!
//! ## Capabilities
//!
//! | Metric | Windows | Linux |
//! |--------|---------|-------|
//! | Process `RSS` | `K32GetProcessMemoryInfo` | `/proc/self/status` |
//! | Device-wide GPU memory | `NVML` (`nvml.dll`) | `NVML` (`libnvidia-ml.so.1`) |
//! | Per-process GPU memory | `DXGI` (`IDXGIAdapter3::QueryVideoMemoryInfo`) | `NVML` (`nvmlDeviceGetComputeRunningProcesses`) |
//! | Fallback | `nvidia-smi` subprocess | `nvidia-smi` subprocess |
//!
//! ## Quick start
//!
//! ```no_run
//! let snap = hypomnesis::Snapshot::now(0)?;
//! println!("RAM: {} bytes", snap.ram_bytes);
//! if let Some(dev) = snap.gpu_device {
//!     println!("GPU 0: {:?}, free {} of {} bytes",
//!              dev.name, dev.free_bytes, dev.total_bytes);
//! }
//! # Ok::<(), hypomnesis::HypomnesisError>(())
//! ```
//!
//! ## Feature flags
//!
//! | Feature | Default | Description |
//! |---------|---------|-------------|
//! | `nvml` | yes | `NVML` dynamic load via `libloading` (Linux + Windows-`WDDM` device-wide) |
//! | `dxgi` | yes | Windows per-process `VRAM` via `IDXGIAdapter3` (no-op on non-Windows) |
//! | `nvidia-smi-fallback` | yes | Subprocess fallback when `NVML` / `DXGI` fail |
//! | `report` | no | `MemoryReport` delta + `print_delta` / `print_before_after` / `ram_mb` / `vram_mb` helpers (`candle-mi` parity) |
//! | `debug-output` | no | Print raw `NVML` / `DXGI` values to stderr (diagnostic) |

#![deny(unsafe_code)]
#![allow(unknown_lints)]

pub mod error;
pub mod gpu;
pub mod ram;
pub mod snapshot;

#[cfg(feature = "report")]
pub mod report;

pub use error::{HypomnesisError, Result};
pub use gpu::{device_count, device_info, process_gpu_info};
pub use ram::process_rss;
pub use snapshot::{GpuDeviceInfo, GpuQuerySource, ProcessGpuInfo, Snapshot};

#[cfg(feature = "report")]
pub use report::MemoryReport;
