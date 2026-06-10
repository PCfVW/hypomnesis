# `hypomnesis` — a brief

> *External measurement of a Rust process's RAM and VRAM state, on Windows, Linux, and macOS. The counterpart to `anamnesis`.*

---

## Why this crate

candle-mi's v0.1.1–v0.1.3 VRAM saga produced ~889 lines of battle-tested Rust covering capabilities that, as far as I can determine from crates.io, **do not exist elsewhere in the ecosystem**:

| Capability | Who has it in Rust today |
|-----------|--------------------------|
| Device-wide NVML memory info (Linux + Windows) | `nvml-wrapper`, `all-smi`, `hardware-query` |
| Per-process NVML compute processes (Linux) | `nvml-wrapper` |
| **Per-process VRAM on Windows for the calling process via `IDXGIAdapter3::QueryVideoMemoryInfo`** | **No one — candle-mi was first** |
| **Per-process VRAM on Windows for foreign processes via `PDH` `\GPU Process Memory(*)\Dedicated Usage` (consumer `WDDM 2.0`+)** | **No one — hypomnesis v0.2.2 was first** |
| **Per-process Metal VRAM on macOS via `ledger(LEDGER_ENTRY_INFO_V2).graphics_footprint`** | **No one — hypomnesis v0.2.3 is first** |
| NVML `u64::MAX` sentinel handling (R570 driver bug on RTX 5060 Ti) | No one |
| `nvidia-smi` subprocess fallback with sanity checks | No one (generally) |
| Process RSS via `K32GetProcessMemoryInfo` / `/proc/self/status` / `task_info(TASK_VM_INFO_PURGEABLE)` | `memory-stats` does RSS, but not combined with VRAM and not on macOS |

The Windows DXGI implementation is the **hard part** — the multi-day deep dive through WDDM architecture, COM pointer manipulation (`IDXGIFactory1` → `IDXGIAdapter` → `IDXGIAdapter3` cast chain), and adapter enumeration (skipping Microsoft Basic Render Driver, handling dedicated vs shared memory segments). It belongs in the ecosystem as a reusable crate, not buried inside a single application's source tree.

## Why the name

Plato's *Phaedrus* distinguishes two kinds of memory:

- **Anamnesis** (ἀνάμνησις) — the soul's inward recollection of truth it always possessed. Used in candle-mi: looking *inside* a model's weights.
- **Hypomnesis** (ὑπόμνησις) — *external* memory aids, reminders, written records. Plato was skeptical of them because they replace true internal memory with external notation.

A crate that queries and records what's currently in a process's memory is **literally hypomnesis**: external snapshots of memory state. The two names form a deliberate pair.

Shorthand: **`hmn`** (rule: first letter of the Greek prefix + the `mn` root — mirrors `amn` for `anamnesis`).

Availability confirmed on crates.io and GitHub (2026-04-29). The v0.0.1 placeholder (skeleton + stubs) was [published to crates.io](https://crates.io/crates/hypomnesis) on `2026-04-29T08:32:49Z` in Phase 1 Wave 1 — see Roadmap. The first functional release will be `0.1.0`, planned in Phase 1 Wave 2.

## Scope (v0.1)

**Included:**
- Process RAM (RSS): Windows, Linux, macOS
- Device-wide GPU memory (total/free/used): NVML on Linux + Windows, `sysctl hw.memsize` + `MTLDevice.recommendedMaxWorkingSetSize` on macOS
- Per-process GPU memory: NVML on Linux, DXGI on Windows, `ledger(LEDGER_ENTRY_INFO_V2).graphics_footprint` on macOS
- Compute-process listing: NVML + `/proc/<pid>/comm` on Linux, `nvidia-smi` on Windows, `proc_listpids` + per-PID `ledger` + `proc_pidpath` on macOS
- `nvidia-smi` subprocess fallback (device-wide, Windows + Linux)
- Robust sentinel handling (NVML `u64::MAX`, used > total sanity check)
- GPU adapter name (DXGI description field on Windows, NVML on Linux, CPU brand string on macOS)

**Explicitly out of scope:**
- Inference tracking (candle-mi's `sync_and_trim_gpu` stays in candle-mi — it's candle/cudarc-specific)
- CPU counters, thermal monitoring, power draw (that's `sysinfo`, `all-smi`, `hardware-query` territory)
- AMD ROCm (future consideration)
- Intel Macs (the macOS path detects Apple Silicon via `machdep.cpu.brand_string`; Intel Macs fall through to `NoGpuSource`)

## Proposed public API

```rust
// Device-wide query — what hf-fm --check-gpu needs
pub struct GpuDeviceInfo {
    pub index: u32,
    pub name: Option<String>,      // from DXGI or nvidia-smi
    pub total_bytes: u64,
    pub free_bytes: u64,
    pub used_bytes: u64,
}

pub fn device_info(index: u32) -> Result<GpuDeviceInfo, HypomnesisError>;
pub fn device_count() -> Result<u32, HypomnesisError>;

// Per-process query — what candle-mi needs
pub struct ProcessGpuInfo {
    pub used_bytes: u64,
    pub is_per_process: bool,       // false when falling back to device-wide
    pub source: GpuQuerySource,     // Dxgi | Nvml | NvidiaSmi
}

pub fn process_gpu_info(device_index: u32) -> Result<ProcessGpuInfo, HypomnesisError>;

// RAM — both crates need it
pub fn process_rss() -> Result<u64, HypomnesisError>;

// Convenience snapshot bundling everything
pub struct Snapshot {
    pub ram_bytes: u64,
    pub gpu: Option<ProcessGpuInfo>,
    pub gpu_device: Option<GpuDeviceInfo>,
}

impl Snapshot {
    pub fn now(device_index: u32) -> Result<Self, HypomnesisError>;
}
```

**No candle dependency.** That's the critical break from candle-mi's current API (`MemorySnapshot::now(&candle_core::Device)`).

**`MemoryReport` and printing helpers behind opt-in `report` feature, default off.** The default crate exposes only raw measurement (`Snapshot`, `GpuDeviceInfo`, `ProcessGpuInfo`, the three query functions). Enable `features = ["report"]` to get `MemoryReport` (delta between two snapshots), `print_delta` / `print_before_after`, and `ram_mb` / `vram_mb` formatting helpers — preserved verbatim from candle-mi for migration parity. Phase 3 (candle-mi adopts) becomes a one-line Cargo flag flip, not a code rewrite. Default consumers who want a quick delta can still subtract two `Snapshot`s in 5 lines without enabling the feature.

**Future-proofing rests on `#[non_exhaustive]`, not on parameter type elaboration.** Every public enum (`HypomnesisError`, `GpuQuerySource`) and every public struct (`GpuDeviceInfo`, `ProcessGpuInfo`, `Snapshot`) is `#[non_exhaustive]`, so new variants and fields can be added in 0.x patch releases without breaking callers. Multi-vendor addressing (UUID, LUID, AMD ROCm, Apple Metal) can therefore be layered in later as *new* selector types or *new* functions alongside the v0.1 `device_index: u32` API — no breaking change required.

## Feature gates

```toml
[features]
default = ["nvml", "nvidia-smi-fallback", "dxgi"]
nvml = ["dep:libloading"]            # dynamic NVML load via libloading
nvidia-smi-fallback = []             # no deps, just subprocess
dxgi = ["dep:windows"]               # Windows-only (target-conditional dep), pulls ~500 kB of bindings
report = []                          # MemoryReport + print_delta / print_before_after / ram_mb / vram_mb (candle-mi parity)
debug-output = []                    # raw NVML / DXGI values to stderr (diagnostic)
```

`dxgi` is in default features (Settled Decision #7). The `windows` dependency is target-conditional via `[target.'cfg(windows)'.dependencies]`, so non-Windows users pay nothing for it. Disabling defaults is only useful for a stripped build that wants process RSS only (no GPU backends, no `windows` crate, no `libloading`). The `report` and `debug-output` features are opt-in and pull no additional dependencies.

## Dependencies (total)

- `libloading` (already in candle-mi)
- `windows = "0.62"` with `Win32_Graphics_Dxgi`, `Win32_Graphics_Dxgi_Common` features (Windows only; already in candle-mi)
- `thiserror` for the error type

That's it. No candle, no serde, no tokio. MSRV 1.88 (match candle-mi).

## First consumer: hf-fetch-model

> **Status:** Phase 2 — **shipped**. `hf-fetch-model 0.10.1` (released 2026-05-12) adopts `hypomnesis = "0.2"` for the `inspect --check-gpu` flag described below. See [`docs/hypomnesis-adoption.md`](hypomnesis-adoption.md) for the dogfooding report and the v0.2.1 roadmap items the integration surfaced.

hf-fetch-model v0.10.x gets `--check-gpu [N]` on `inspect`:

```
$ hf-fm inspect google/gemma-4-E2B-it model.safetensors --check-gpu

  Model weights:  9.54 GiB  (BF16, 5.12B params)
  GPU 0:          NVIDIA GeForce RTX 5060 Ti — 16.0 GiB VRAM
                  free: 14.2 GiB, used: 1.8 GiB
  Fit:            ✓ 4.66 GiB headroom for weights + KV cache + runtime

  Note: reports weights only. Large-context inference typically needs ~1.3–1.5×
  weight size for KV cache and activations.
```

This is the proof-of-concept consumer. `hf-fm` uses `device_info` directly (well under 10% of hypomnesis's API surface). `device_count` is deferred to the multi-GPU follow-up (`--check-gpu all`, `hf-fm` v0.10.4) — `device_info`'s `DeviceIndexOutOfRange { index, count }` variant already exposes the count whenever an out-of-range index is queried, so the v0.10.1 single-device path does not need a separate count call. `candle-mi` is expected to exercise the broader surface (`Snapshot`, `report`-feature helpers) when it migrates to `hypomnesis 0.2.1`.

## Roadmap

**Phase 1 — Scaffold + name reservation**

*Wave 1 — ✅ completed 2026-04-29*

The full crate skeleton is in place, CI is green, and the v0.0.1 placeholder is on crates.io. Function bodies are still placeholders that return errors; Wave 2 ports the actual measurement code.

- Repo created at [github.com/PCfVW/hypomnesis](https://github.com/PCfVW/hypomnesis) (sibling of `anamnesis`, `candle-mi`, `hf-fetch-model`, `d-ary-heap`)
- Project metadata: `Cargo.toml` (edition 2024, MSRV 1.88, dual MIT-OR-Apache), `LICENSE-MIT` + `LICENSE-APACHE` (verbatim from `candle-mi`), `.gitignore`, `README.md` (anamnesis-aligned: 7 badges including a `NVIDIA NVML+DXGI` brand badge, Greek tagline, ToC, Install, Usage, Capabilities, Feature Flags, Used by, License, Development), `CHANGELOG.md` (Keep-a-Changelog format), `CONVENTIONS.md` (Grit + Grit-HMN extensions: NVML / DXGI / `K32GetProcessMemoryInfo` SAFETY guidance + a 5-step recipe for adding a new GPU backend), `docs/hypomnesis-brief.md` (this document)
- Source skeleton: `src/{lib,error,snapshot,ram,report}.rs` and `src/gpu/{mod,nvml,dxgi,nvidia_smi}.rs`. All public types defined, all function signatures wired, all bodies are deliberate placeholders that return `HypomnesisError::*("not yet implemented (Phase 1 scaffolding)")`
- Public API surface frozen per the *Decisions settled* section. Every public enum (`HypomnesisError`, `GpuQuerySource`) and every public struct (`GpuDeviceInfo`, `ProcessGpuInfo`, `Snapshot`, `MemoryReport`) is `#[non_exhaustive]`, so Wave 2 implementation work cannot accidentally widen the surface
- `tests/smoke.rs` — public API reachability test (2 unit tests + 1 doctest, all passing)
- `[package.metadata.docs.rs]` — docs.rs builds with `all-features = true` for both `x86_64-unknown-linux-gnu` and `x86_64-pc-windows-msvc`, exposing the Windows-only `dxgi` module on docs.rs alongside the cross-platform `nvml` path
- CI: [`.github/workflows/ci.yml`](https://github.com/PCfVW/hypomnesis/blob/main/.github/workflows/ci.yml) — matrix `(ubuntu-latest, windows-latest) × (MSRV 1.88, stable)` running `fmt --check` + `clippy --all-targets` + `clippy --all-targets --all-features` + `test --all-features`, plus a separate doc-check job (`RUSTDOCFLAGS=-D warnings`). Green on first push: run [25098689267](https://github.com/PCfVW/hypomnesis/actions/runs/25098689267), 5 jobs in 1m13s
- Publish workflow: [`.github/workflows/publish.yml`](https://github.com/PCfVW/hypomnesis/blob/main/.github/workflows/publish.yml) — anamnesis-aligned, triggers on `v*` tags or `workflow_dispatch`; runs full CI checks + doc check then `cargo publish`. Requires the `CARGO_REGISTRY_TOKEN` repo secret (set 2026-04-29 by the user across the four sibling crates)
- **[`hypomnesis 0.0.1`](https://crates.io/crates/hypomnesis) published to crates.io at `2026-04-29T08:32:49Z`** — placeholder for name reservation. Commit [`8785ade`](https://github.com/PCfVW/hypomnesis/commit/8785ade) (21 files, 1768 insertions), tag `v0.0.1`, publish run [25098851149](https://github.com/PCfVW/hypomnesis/actions/runs/25098851149) (23s). The placeholder push was originally scheduled for Phase 2 but pulled forward into Wave 1 once the scaffold was publishable — the Greek root makes drive-by squatting unlikely, but the cost of a placeholder push is low and the cost of losing the name is high

*Wave 2 — outstanding*

- Port `candle-mi/src/memory.rs` (889 lines) into the existing placeholder bodies, dropping `candle_core::Device` in favor of `device_index: u32` and removing the `MemorySnapshot` mixed-concerns struct
- Verify that `sync_and_trim_gpu` stays in candle-mi (out of scope for hypomnesis — design already excludes it; nothing to do here)
- Port + expand the inline test suite from `candle-mi/src/memory.rs` (currently 6 unit tests covering `snapshot_cpu_has_ram`, `report_delta_*`, `ram_mb_conversion`, `vram_qualifier_*`); add `#[ignore]` live-GPU tests gated for `cargo test -- --ignored`
- Live-GPU verification on both the Windows host (`RTX 5060 Ti`, native `NVML` + `DXGI` paths) and Ubuntu WSL2 (`NVML` via NVIDIA's CUDA-on-WSL driver) — confirms the cross-platform implementation matches across both `NVML` enumerations
- Cross-check the `R570` `u64::MAX` sentinel-handling path still triggers and falls back to `nvidia-smi` cleanly (unique to this driver/card combination, can't be tested in CI without hardware)
- Update `CHANGELOG.md` with `[0.1.0]` entry detailing every kernel ported
- Bump `Cargo.toml` to `0.1.0` and re-run the 8-step publish flow (see `reference_publish_flow.md` in this Claude Code project's memory) → first functional release on crates.io
- After `0.1.0` ships, the `[package.metadata.docs.rs]` block ensures docs.rs renders the full surface for both targets

**Phase 2 — hf-fm adopts (validates the API)**

- `hf-fetch-model 0.10.x` adds `inspect --check-gpu [N]` using `hypomnesis = "0.1"` directly from crates.io (no path dep needed — `0.1.0` is already published when this phase starts)
- Iterate on API rough edges surfaced by real use → `hypomnesis 0.1.x` patches as needed (the `#[non_exhaustive]` policy makes additive changes patch-release-safe)
- Optional: publish a `0.2.0` if a breaking surface change is unavoidable

**Phase 3 — candle-mi migrates (optional, cleanup)**

- candle-mi drops its in-tree `src/memory.rs`
- Depends on `hypomnesis = "0.1"` with `features = ["report"]` so the existing `MemorySnapshot` / `MemoryReport` / `print_delta` / `print_before_after` / `ram_mb` / `vram_mb` ergonomics are preserved verbatim — Phase 3 is a one-line Cargo flag flip plus a thin `MemorySnapshot` adapter wrapping `hypomnesis::Snapshot`, not a code rewrite
- Keeps `sync_and_trim_gpu` as its own (candle-specific, depends on `cudarc::driver::sys`)
- Shipped in candle-mi `v0.2`

## Decisions settled (Phase 1 — Wave 1, 2026-04-29)

1. **Repo location** — `github.com/PCfVW/hypomnesis`. Sibling on GitHub of `anamnesis`, `candle-mi`, `hf-fetch-model`, `d-ary-heap`.
2. **License** — `MIT OR Apache-2.0` (matches every sibling crate).
3. **Error type** — `HypomnesisError` enum with `thiserror`, marked `#[non_exhaustive]`. Variants: `Ram(String)`, `Nvml(String)`, `Dxgi(String)`, `NvidiaSmi(String)`, `DeviceIndexOutOfRange { index, count }`, `NoGpuSource`, `Io(#[from] std::io::Error)`.
4. **MSRV** — 1.88 (matches candle-mi).
5. **Authorship** — driven from candle-mi's `src/memory.rs` by copy-and-refactor.
6. **`device_index: u32` future-proofing** — keep `u32` for v0.1 (NVML-canonical, NVIDIA-only ordering on Windows: DXGI internally walks adapters and returns the N-th NVIDIA adapter with non-zero dedicated VRAM, skipping iGPUs and software drivers). Future-proofing rests on `#[non_exhaustive]` everywhere — see API section. Multi-vendor / UUID / LUID addressing can be added later as *new* functions alongside the existing `u32` API, never as a replacement.
7. **`dxgi` in default features** — yes. Linux users pay nothing (target-conditional dep); Windows users get per-process VRAM out of the box.
8. **`MemoryReport` and printing helpers** — gated behind opt-in `report` feature, default off. Names preserved verbatim from candle-mi so Phase 3 (candle-mi v0.2) is a one-line Cargo feature flip rather than a code rewrite. See "Proposed public API" for details.

## Scope boundaries (repeat, for clarity)

- Not a general system-info crate (use `sysinfo`, `hardware-query` for that)
- Not a CUDA wrapper (use `cust`, `cudarc` for that)
- Not a GUI or TUI (use `ratatui`, `iced` for that)
- Not opinionated about *when* to measure — caller's responsibility

One crate, one job: **tell you what's currently in this process's memory, precisely, across Windows, Linux, and macOS.**
