# hypomnesis

[![CI](https://github.com/PCfVW/hypomnesis/actions/workflows/ci.yml/badge.svg)](https://github.com/PCfVW/hypomnesis/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/hypomnesis.svg)](https://crates.io/crates/hypomnesis)
[![docs.rs](https://docs.rs/hypomnesis/badge.svg)](https://docs.rs/hypomnesis)
[![MSRV](https://img.shields.io/badge/MSRV-1.88-blue.svg)](https://www.rust-lang.org)
[![license](https://img.shields.io/crates/l/hypomnesis.svg)](https://github.com/PCfVW/hypomnesis#license)
[![unsafe: deny](https://img.shields.io/badge/unsafe-deny_(NVML_%2B_DXGI_%2B_K32_only)-blue.svg)](https://github.com/rust-secure-code/safety-dance/)
[![NVIDIA](https://img.shields.io/badge/NVIDIA-NVML_%2B_DXGI-76B900.svg?logo=nvidia&logoColor=white)](#capabilities)

**ὑπόμνησις** — *External RAM and VRAM, measured.*

> 🚀 **`0.2.2` is the first Rust crate to expose per-process `VRAM` for foreign processes on consumer Windows / `WDDM`.** A new `PDH` (Performance Data Helper) backend reads `VidMm`'s `\GPU Process Memory(*)\Dedicated Usage` counter — the same data Task Manager's "Dedicated GPU memory" column surfaces. Closes the dogfooding gap where `hmn ps` reported `"0 compute processes found"` despite dozens of processes holding GPU memory. Per-process names resolve via `Win32 OpenProcess` + `QueryFullProcessImageNameW` (cross-platform-consistent with the Linux `/proc/<pid>/comm` and macOS `proc_pidpath` patterns); `PID 4` is special-cased to `[kernel]` so it doesn't pollute the "unresolvable even elevated" set; the `hmn ps` summary line gains a `(N protected — re-run elevated for names)` parenthetical that's intentionally surfaced for its security-relevance. All additive under the `#[non_exhaustive]` policy carried over from v0.2.0 / v0.2.1. See [`CHANGELOG.md`](CHANGELOG.md) for the v0.2.2 entry and [`docs/roadmap-v0.2.2.md`](docs/roadmap-v0.2.2.md) for the wave-by-wave rationale.

## Table of Contents

- [Install](#install)
- [Usage](#usage)
- [Binary (`hmn`)](#binary-hmn)
- [Capabilities](#capabilities)
- [Feature Flags](#feature-flags)
- [Used by](#used-by)
- [License](#license)
- [Development](#development)

## Install

```toml
[dependencies]
hypomnesis = "0.2"
```

The default feature set (`nvml`, `dxgi`, `nvidia-smi-fallback`) covers process RSS and per-process / device-wide GPU memory on both Windows (`IDXGIAdapter3` + `NVML`) and Linux (`NVML`), with a `nvidia-smi` subprocess fallback. The `dxgi` dependency on the `windows` crate is target-conditional — Linux users pay nothing for it.

For candle-mi-compatible delta and printing helpers (`MemoryReport`, `print_delta`, `print_before_after`, `ram_mb`, `vram_mb`):

```toml
hypomnesis = { version = "0.2", features = ["report"] }
```

For a stripped-down build (process RSS only, no GPU backends):

```toml
hypomnesis = { version = "0.2", default-features = false }
```

## Usage

```rust
use hypomnesis::Snapshot;

fn main() -> Result<(), hypomnesis::HypomnesisError> {
    let snap = Snapshot::now(0)?;
    println!("RAM: {} bytes", snap.ram_bytes);

    if let Some(dev) = snap.gpu_device {
        let total_gib = dev.total_bytes as f64 / (1u64 << 30) as f64;
        let used_gib  = dev.used_bytes  as f64 / (1u64 << 30) as f64;
        println!(
            "GPU 0 [{}]: {:.1} / {:.1} GiB used",
            dev.name.as_deref().unwrap_or("unknown"),
            used_gib, total_gib,
        );
    }

    if let Some(proc_gpu) = snap.gpu {
        let kind = if proc_gpu.is_per_process { "per-process" } else { "device-wide" };
        let mib  = proc_gpu.used_bytes as f64 / (1u64 << 20) as f64;
        println!("This process: {:.0} MiB ({})", mib, kind);
    }

    Ok(())
}
```

Expected output (RTX 5060 Ti, Windows, idle process):

```
RAM: 142475264 bytes
GPU 0 [NVIDIA GeForce RTX 5060 Ti]: 1.8 / 16.0 GiB used
This process: 119 MiB (per-process)
```

## Binary (`hmn`)

`hypomnesis` ships a small CLI binary, `hmn`, behind the default-off `cli` feature. Install it with:

```sh
cargo install hypomnesis --features cli
```

Two subcommands:

```sh
hmn                    # device summary (free / total per GPU)
hmn ps                 # all GPU processes — discovery command
hmn ps --pid 12345     # filter to one PID
hmn ps --device 0      # filter to one GPU on multi-GPU rigs
hmn ps --json          # scriptable output
```

Example default output (single NVIDIA dGPU, the maintainer's reference machine — Ryzen 9 5950X has no iGPU, so only one adapter surfaces):

```
GPU 0 [NVIDIA GeForce RTX 5060 Ti]: free 13284 MiB / 16384 MiB
```

Illustrative output on a *heterogeneous* machine (NVIDIA dGPU + Intel/AMD iGPU on Windows). Not yet verified end-to-end on real hardware — see [`docs/roadmap-v0.2.0.md`](docs/roadmap-v0.2.0.md) "Verification plan":

```
GPU 0 [NVIDIA GeForce RTX 5060 Ti]: free 13284 MiB / 16384 MiB
GPU 1 [Intel Iris Xe Graphics]: free 32768 MiB / 32768 MiB
```

`hmn ps` (illustrative — empty on machines with no active CUDA workload):

```
PID    NAME              VRAM      DEVICE
12345  lm-studio.exe     8.2 GiB   NVIDIA GeForce RTX 5060 Ti
67890  python.exe        1.4 GiB   NVIDIA GeForce RTX 5060 Ti
```

A one-line summary is written to **stderr** after each `hmn ps` run:

```
hmn: 2 compute processes found.
hmn: 0 compute processes found matching pid=99 device=0.   # with filters
```

The stderr summary is always printed, even when the table is empty, so interactive users get an unambiguous "command worked, here's the count" line without breaking stdout's scriptability. Pipelines like `hmn ps | awk 'NR>1 {print $1}'` or `hmn ps --json | jq` work as expected. Redirect `2>/dev/null` to suppress the summary.

**Limitations** (intrinsic to the underlying data sources, not bugs):

1. **Per-platform semantics differ — compute-only on Linux, all-GPU-users on Windows.** `hmn ps` on Linux (via `NVML`'s `nvmlDeviceGetComputeRunningProcesses_v3`) enumerates only processes with an active `CUDA` context — browsers using GPU compositing, games, and pure-graphics apps do not appear. `hmn ps` on Windows (via `PDH`'s `\GPU Process Memory(*)\Dedicated Usage`) enumerates **every** process holding GPU memory — the desktop compositor (`dwm.exe`), browsers, games, and `CUDA` / compute alongside. The semantic shift reflects what each platform's kernel actually accounts for; check the `source` field on `GpuProcessEntry` if you care about the distinction.

2. **Windows `used_bytes` reflects WDDM's *dedicated commit*, not resident set.** Under `WDDM` a process can commit GPU allocations exceeding physical `VRAM` — the kernel pages them via the shared system memory budget. Numbers exceeding the device's total `VRAM` are real, not bugs: they match Task Manager's `Dedicated GPU memory` column. (Example: on a 16 GiB GPU, a heavy browser process can show 15+ GiB committed.)

3. **`?` in the NAME column means the calling user cannot resolve that PID's name via `OpenProcess`.** Most cases — system services, other-user processes like `dwm.exe`, `csrss.exe`, vendor services — resolve when `hmn ps` is run as Administrator. **The Windows kernel itself (`PID 4`) is rendered as `[kernel]`, not `?`** — there is no executable image to read, so it's special-cased so it does not pollute the "unresolvable" count. `PPL`-protected processes (Windows Defender, anti-cheat engines) would also remain `?` even elevated, but typically do not appear in `hmn ps` output unless they are actively holding GPU memory.

   *Security note.* By construction, a `?` row that does not resolve under elevation is one of: a process owned by another user, a process running as `SYSTEM` / `LOCAL SERVICE` / `NETWORK SERVICE`, a `PPL`-protected process, or a transient race between PDH's sample and the `OpenProcess` call. None of these are intrinsically malicious — but on a single-user desktop, an *unexpected* `?` row holding substantial VRAM is worth investigating: a malicious local process (including a privileged-or-cross-user AI agent) using GPU resources would land in exactly this set. The `(N protected — re-run elevated for names)` parenthetical on the `hmn ps` summary line is intentionally surfaced because this distinction is security-relevant. `hypomnesis` is a measurement tool, not a malware scanner — but its honesty about the gap is itself a defensive primitive.

4. **Pre-`WDDM 2.0` Windows falls back to `nvidia-smi --query-compute-apps`.** Vanishingly rare in 2026 — `WDDM 2.0` shipped with Windows 10 1709 (October 2017). On the fallback path, `hmn ps` is compute-only (matching the Linux semantic) and `used_memory` may be `[N/A]` under `WDDM` (parser drops those rows). The `source` field on `GpuProcessEntry` reads `GpuQuerySource::NvidiaSmi` rather than `GpuQuerySource::Pdh` on this path.

5. **`R570`-class driver-bug filtering.** The `u64::MAX` sentinel (`R570` driver bug on `RTX 5060 Ti` and similar consumer GeForce cards) and the `used > total` corruption checks are applied per-row in `hmn ps`; affected rows are dropped rather than reported as garbage.

## Capabilities

| Metric | Windows | Linux |
|--------|---------|-------|
| Process RSS | `K32GetProcessMemoryInfo` | `/proc/self/status` (no `unsafe`) |
| Device-wide GPU memory | `NVML` (`nvml.dll`) | `NVML` (`libnvidia-ml.so.1`) |
| Per-process GPU memory | `DXGI` (`IDXGIAdapter3::QueryVideoMemoryInfo`) | `NVML` (`nvmlDeviceGetComputeRunningProcesses`) |
| Fallback | `nvidia-smi` subprocess | `nvidia-smi` subprocess |

`hypomnesis` uses `IDXGIAdapter3` on Windows because `WDDM` means the kernel memory manager — not the NVIDIA driver — owns GPU allocations, so `NVML`'s per-process query returns `NOT_AVAILABLE` under Windows. `DXGI 1.4` is the only reliable per-process source. On Linux, `NVML`'s `nvmlDeviceGetComputeRunningProcesses_v3` returns true per-process figures.

The crate handles two known driver bugs out of the box:

1. **`NVML` `u64::MAX` sentinel** — some `R570`-series drivers report `0xFFFFFFFFFFFFFFFF` for every running process's memory (observed on `RTX 5060 Ti`). `hypomnesis` detects this and falls back to `nvidia-smi`.
2. **`used > total` corruption** — sanity-checks each per-process reading against the device-wide total; falls back to `nvidia-smi` on detected corruption.

## Feature Flags

| Feature | Default | Description |
|---------|---------|-------------|
| `nvml` | yes | `NVML` dynamic load via `libloading` (Linux + Windows-`WDDM` device-wide) |
| `dxgi` | yes | Windows per-process `VRAM` via `IDXGIAdapter3` (no-op on non-Windows) |
| `nvidia-smi-fallback` | yes | Subprocess fallback when `NVML` / `DXGI` fail or are disabled |
| `report` | no | `MemoryReport` delta + `print_delta` / `print_before_after` / `ram_mb` / `vram_mb` helpers (`candle-mi` parity, candidate for `candle-mi` v0.2 migration via Cargo flag flip); `format_free` / `print_free` / `format_total` / `format_used` formatting helpers on `GpuDeviceInfo` |
| `debug-output` | no | Print raw `NVML` / `DXGI` values to stderr (diagnostic) |
| `cli` | no | Build the `hmn` CLI binary (pulls `clap` 4 as a dep). Library users do not need this; install via `cargo install hypomnesis --features cli`. |
| `test-helpers` | no | Expose `GpuDeviceInfoBuilder` for downstream tests that need synthetic `GpuDeviceInfo` fixtures. Default-off, additive — production code must never enable it. |

## Used by

- [hf-fetch-model](https://github.com/PCfVW/hf-fetch-model) — Hugging Face model weights and metadata fetcher (uses `device_info` for `inspect --check-gpu`)

_Forthcoming: [candle-mi](https://github.com/PCfVW/candle-mi) is expected to migrate its in-tree memory module to `hypomnesis` (`features = ["report"]`) after v0.2.1 lands._

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT License](LICENSE-MIT) at your option.

## Development

- Exclusively developed with [Claude Code](https://claude.com/product/claude-code) (dev) and [Augment Code](https://www.augmentcode.com/) (review)
- Git workflow managed with [Fork](https://fork.dev/)
- All code follows [CONVENTIONS.md](CONVENTIONS.md), derived from [Amphigraphic-Strict](https://github.com/PCfVW/Amphigraphic-Strict)'s [Grit](https://github.com/PCfVW/Amphigraphic-Strict/tree/master/Grit) — a strict Rust subset designed to improve AI coding accuracy.
