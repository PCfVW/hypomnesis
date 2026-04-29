# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] - 2026-04-29

First functional release. Wave 2 of Phase 1 — ports the actual measurement code from [candle-mi/src/memory.rs](https://github.com/PCfVW/candle-mi/blob/main/src/memory.rs) (889 lines) into the `0.0.1` placeholder skeleton.

### Added

- **Process `RSS` measurement** — [`process_rss`](src/ram.rs) returns the per-process resident-set size in bytes. Windows: `K32GetProcessMemoryInfo` → `WorkingSetSize` via an `unsafe extern "system"` block. Linux: `/proc/self/status` → `VmRSS`, parsing logic extracted as `parse_vmrss(&str)` for unit-testability (6 inline parsing tests for various `/proc/self/status` shapes).
- **`NVML` backend** ([src/gpu/nvml.rs](src/gpu/nvml.rs)) — dynamically loads `libnvidia-ml.so.1` (Linux) or `nvml.dll` (Windows) via `libloading`. Symbols loaded: `nvmlInit_v2`, `nvmlShutdown`, `nvmlDeviceGetHandleByIndex_v2`, `nvmlDeviceGetMemoryInfo`, `nvmlDeviceGetComputeRunningProcesses_v3`, **`nvmlDeviceGetCount_v2`** (new vs candle-mi, for `device_count`), and **`nvmlDeviceGetName`** (new vs candle-mi, for the adapter name on the `NVML` path — Wave 2 decision #1). Two crate-internal entry points: `query(idx)` for combined per-process + device-wide queries in a single init/shutdown cycle, and `device_count()`. Includes the `R570` `u64::MAX` sentinel guard and `used > total` sanity check ported from candle-mi.
- **`DXGI` backend** ([src/gpu/dxgi.rs](src/gpu/dxgi.rs), Windows-only) — walks `IDXGIFactory1::EnumAdapters1` filtering by NVIDIA vendor ID (`0x10DE`) + non-zero `DedicatedVideoMemory`, casts to `IDXGIAdapter3`, calls `QueryVideoMemoryInfo(DXGI_MEMORY_SEGMENT_GROUP_LOCAL)`. Three entry points: `query` (full per-process + device + name), `adapter_name` (lightweight name-only path that skips `QueryVideoMemoryInfo`), and `device_count`. The `WDDM`-aware per-process path is the only reliable per-process VRAM source on Windows.
- **`nvidia-smi` subprocess fallback** ([src/gpu/nvidia_smi.rs](src/gpu/nvidia_smi.rs)) — runs `nvidia-smi --query-gpu=memory.used,memory.total --format=csv,noheader,nounits --id=N` and parses the single CSV line. Saturating `MiB → bytes` conversion. The `--id=N` argument is new vs candle-mi (which hardcoded device 0).
- **Dispatchers** ([src/gpu/mod.rs](src/gpu/mod.rs)) — three public functions try backends in priority order:
  - `process_gpu_info`: `DXGI` (Windows) → `NVML` (Linux primary; `NVML_VALUE_NOT_AVAILABLE` under Windows `WDDM`) → `nvidia-smi` (device-wide, sets `is_per_process = false`).
  - `device_info`: `NVML` for total/free/used + `DXGI` for friendlier adapter `name` on Windows → `DXGI`-alone fallback (loose semantics: `CurrentUsage` is per-process, used as approximate device-wide `used` per Wave 2 decision #2) → `nvidia-smi` device-wide.
  - `device_count`: `NVML` → `DXGI`.
  - Returns [`HypomnesisError::DeviceIndexOutOfRange`] when `NVML` or `DXGI` reports a count and the requested index is past it.
- **`Snapshot::ram_mb()` and `Snapshot::vram_mb()`** under `#[cfg(feature = "report")]` ([src/snapshot.rs](src/snapshot.rs)) — located on `Snapshot` (rather than on `MemoryReport`) for parity with `candle-mi`'s `MemorySnapshot::ram_mb` / `vram_mb` API location, so candle-mi v0.2 adoption is a thin adapter wrapper rather than a code rewrite (Wave 2 decision #6).
- **`MemoryReport` real bodies** ([src/report.rs](src/report.rs)) — `ram_delta_mb`, `vram_delta_mb`, `vram_qualifier` (`const fn`), plus the printing helpers `print_delta` / `print_before_after`. Output formats preserved verbatim from candle-mi for migration parity.
- **`MemoryReport::format_delta` and `format_before_after`** — `String`-returning siblings of the printing helpers (new vs candle-mi). Same byte-for-byte output as the `print_*` methods, but as an owned `String` for log frameworks (`tracing`, `log`), file output, or test assertions. The `print_*` methods now delegate here, locking the format under unit-test verification.
- **`examples/print_demo.rs`** — runnable demo (`cargo run --features report --example print_demo`) that takes two snapshots around a 50 MiB allocation and prints the delta + before→after via all four `MemoryReport` formatters. See [`examples/README.md`](examples/README.md).
- **`debug-output` feature diagnostics across all three GPU backends** (Wave 2 decision #7) — `eprintln!` traces at every `NVML` return code, every `DXGI` adapter resolved, every `nvidia-smi` parse step, plus final result summaries.
- **Inline unit tests** in [`src/snapshot.rs`](src/snapshot.rs) (6 tests for `Snapshot` construction + `ram_mb` / `vram_mb` conversion) and [`src/report.rs`](src/report.rs) (7 tests for `ram_delta_mb` / `vram_delta_mb` / `vram_qualifier`).
- **Smoke tests** ([tests/smoke.rs](tests/smoke.rs)) extended with `process_rss > 0` and end-to-end `Snapshot::now` checks (no GPU dependency required — succeed on CI without NVIDIA hardware).
- **Live-GPU tests** ([tests/live_gpu.rs](tests/live_gpu.rs)) — `#[ignore]`-gated per Wave 2 decision #5; run via `cargo test -- --ignored` on a machine with an NVIDIA GPU + driver. 5 tests covering `device_count`, `device_info`, `process_gpu_info`, `Snapshot::now`, and `DeviceIndexOutOfRange`. Verified locally on Windows (RTX 5060 Ti, native `NVML` + `DXGI`) and on Ubuntu WSL2 (`NVML` via NVIDIA's CUDA-on-WSL driver).

### Changed

- Backend module visibility flipped from `pub mod` to `mod` (Wave 2 decision #4) — the public API surface in `crate::gpu` is now exactly the three dispatchers; backend internals (`nvml`, `dxgi`, `nvidia_smi`) are crate-private.
- `device_index: u32` semantics on Windows now filter by NVIDIA vendor ID (`0x10DE`) — more precise than candle-mi's `DedicatedVideoMemory > 0` alone on multi-vendor systems (Intel iGPU + AMD dGPU + NVIDIA dGPU).

## [0.0.1] - 2026-04-29

Initial scaffold (Phase 1 of the v0.1 plan; not for production use). The
function bodies are placeholders that compile and pass clippy under
`-D warnings`; Wave 2 ports the actual measurement code from
[candle-mi/src/memory.rs](https://github.com/PCfVW/candle-mi/blob/main/src/memory.rs)
(889 lines).

### Added

- **Crate scaffold** — `Cargo.toml` (edition 2024, MSRV 1.88, MIT OR Apache-2.0), source-file skeleton at [`src/lib.rs`](src/lib.rs), [`src/error.rs`](src/error.rs), [`src/snapshot.rs`](src/snapshot.rs), [`src/ram.rs`](src/ram.rs), [`src/gpu/{mod,nvml,dxgi,nvidia_smi}.rs`](src/gpu/), and [`src/report.rs`](src/report.rs) (gated on the `report` feature).
- **Public API types** — `HypomnesisError`, `Result`, `Snapshot`, `GpuDeviceInfo`, `ProcessGpuInfo`, `GpuQuerySource`, and (with the `report` feature) `MemoryReport`. All public enums and structs are `#[non_exhaustive]` for forward compatibility — new variants and fields land in patch releases without breaking callers. The brief's settled-decisions section explains why future-proofing rests on `#[non_exhaustive]` rather than on parameter type elaboration.
- **Feature flags** — defaults `nvml` (`NVML` dynamic load via `libloading`), `dxgi` (Windows per-process `VRAM` via `IDXGIAdapter3::QueryVideoMemoryInfo`, no-op on non-Windows), `nvidia-smi-fallback` (subprocess fallback when `NVML` / `DXGI` fail). Opt-in `report` (the `candle-mi` parity suite: `MemoryReport` + `print_delta` / `print_before_after` / `ram_mb` / `vram_mb`) and `debug-output` (raw `NVML` / `DXGI` values to stderr).
- **`CONVENTIONS.md`** — Grit + Grit-HMN extensions, aligned with [`anamnesis/CONVENTIONS.md`](https://github.com/PCfVW/anamnesis/blob/main/CONVENTIONS.md) and [`candle-mi/CONVENTIONS.md`](https://github.com/PCfVW/candle-mi/blob/main/CONVENTIONS.md). Adds `NVML` / `DXGI` / `K32GetProcessMemoryInfo` SAFETY guidance and a five-step recipe for adding a new GPU backend (e.g., AMD `ROCm`, Apple Metal). Drops the SIMD-specific anamnesis sections that don't apply to a measurement crate.
- **`docs/hypomnesis-brief.md`** — design document with v0.1 scope, settled-decisions section, and three-phase roadmap (Phase 1 = extraction, Phase 2 = `hf-fetch-model` adopts, Phase 3 = `candle-mi` migrates).
- **`README.md`** — project overview with badges (CI, crates.io, docs.rs, MSRV, license, unsafe-deny, NVIDIA NVML+DXGI), install, usage, capability matrix, feature flags, license, and development conventions. Mirrors the structure used in [`anamnesis/README.md`](https://github.com/PCfVW/anamnesis/blob/main/README.md).
- **`[package.metadata.docs.rs]`** — docs.rs builds with `all-features = true` and targets both `x86_64-unknown-linux-gnu` and `x86_64-pc-windows-msvc`, exposing the Windows-only `dxgi` module on docs.rs alongside the cross-platform `nvml` path.
