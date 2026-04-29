# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
