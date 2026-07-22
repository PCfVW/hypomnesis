// SPDX-License-Identifier: MIT OR Apache-2.0

//! # hypomnesis
//!
//! External measurement of a Rust process's `RAM` and `VRAM` state, on Windows, Linux, and macOS.
//!
//! `hypomnesis` reports what's currently in a process's memory — process `RSS`,
//! device-wide GPU memory, and per-process GPU `VRAM` — without depending on
//! `candle`, `cudarc`, `sysinfo`, or any inference framework.
//!
//! ## Capabilities
//!
//! | Metric | Windows | Linux | macOS |
//! |--------|---------|-------|-------|
//! | Process `RSS` | `K32GetProcessMemoryInfo` | `/proc/self/status` | `task_info(TASK_VM_INFO_PURGEABLE).phys_footprint` |
//! | Device-wide GPU memory | `NVML` (`nvml.dll`) | `NVML` (`libnvidia-ml.so.1`) | `sysctl hw.memsize` (total) + `MTLDevice.recommendedMaxWorkingSetSize` (free) |
//! | Device reserved memory | `NVML` v2 (`nvmlDeviceGetMemoryInfo_v2`, R510+) | `NVML` v2 (R510+) | n/a (`None` — UMA has no carve-out) |
//! | Per-process GPU memory | `DXGI` (`IDXGIAdapter3::QueryVideoMemoryInfo`) | `NVML` (`nvmlDeviceGetComputeRunningProcesses`) | `ledger(LEDGER_ENTRY_INFO_V2).graphics_footprint` |
//! | GPU-process listing (other PIDs) | `PDH` (`\GPU Process Memory(*)\Dedicated Usage` + `Shared Usage`) + `OpenProcess`/`QueryFullProcessImageNameW`; `nvidia-smi --query-compute-apps` fallback (NB: Windows `PDH` is **not** compute-only — it surfaces every GPU memory holder, including the compositor and browsers) | `NVML` + `/proc/<pid>/comm` (compute-only) | `proc_listpids` + per-PID `ledger` + `proc_pidpath` (same-user PIDs only; cross-user requires `sudo`) |
//! | Spill detection (`SpillTracker`) | `PDH` `\GPU Adapter Memory(*)\Dedicated Usage` + `Shared Usage` (`WDDM 2.0`+) | n/a (`is_spill_measurable()` returns `false` — normal `CUDA` OOMs rather than silently paging) | n/a (`false` — `UMA` has nothing to spill *into*) |
//! | Fallback | `nvidia-smi` subprocess | `nvidia-smi` subprocess | none (libSystem syscalls always succeed on Apple Silicon) |
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
//! | `pdh` | yes | Windows foreign-process `VRAM` listing via `PDH` `\GPU Process Memory(*)\Dedicated Usage` + `Shared Usage`, and the `\GPU Adapter Memory(*)` counters backing `SpillTracker`'s live path, under `WDDM 2.0`+ (no-op on non-Windows; depends on `dxgi` for adapter `LUID` / capacity lookup). `SpillTracker` itself compiles on every platform regardless — without this feature it is simply never measurable |
//! | `metal` | yes | macOS device-wide GPU budget via `objc2-metal` (`MTLDevice.recommendedMaxWorkingSetSize`) (no-op on non-macOS) |
//! | `nvidia-smi-fallback` | yes | Subprocess fallback when `NVML` / `DXGI` / `PDH` fail or are otherwise unavailable (e.g. pre-`WDDM 2.0` Windows) |
//! | `report` | no | `MemoryReport` delta + `print_delta` / `print_before_after` / `ram_mb` / `vram_mb` helpers (`candle-mi` parity); `format_free` / `print_free` / `format_total` / `format_used` formatting helpers on `GpuDeviceInfo` |
//! | `debug-output` | no | Print raw values from the `NVML` / `DXGI` / `PDH` / `nvidia-smi` / spill paths to stderr (diagnostic) |
//! | `cli` | no | Build the `hmn` CLI binary (pulls `clap` 4 as a dep). Library users do not need this; install via `cargo install hypomnesis --features cli` |
//! | `test-helpers` | no | Expose `GpuDeviceInfoBuilder` and `SpillReportBuilder` for downstream tests that need synthetic fixtures. Default-off, additive — production code must never enable it. |

#![deny(unsafe_code)]
#![allow(unknown_lints)]

pub mod error;
pub mod gpu;
pub mod ram;
pub mod snapshot;
pub mod spill;

#[cfg(feature = "report")]
pub mod report;

pub use error::{HypomnesisError, Result};
pub use gpu::{device_count, device_info, gpu_processes, process_gpu_info};
pub use ram::process_rss;
pub use snapshot::{GpuDeviceInfo, GpuProcessEntry, GpuQuerySource, ProcessGpuInfo, Snapshot};
pub use spill::{SpillEpisode, SpillReport, SpillTracker, is_spill_measurable};

#[cfg(feature = "report")]
pub use report::MemoryReport;

#[cfg(feature = "test-helpers")]
pub use snapshot::GpuDeviceInfoBuilder;

#[cfg(feature = "test-helpers")]
pub use spill::SpillReportBuilder;
