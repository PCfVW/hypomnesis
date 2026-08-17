# hypomnesis

[![CI](https://github.com/mi-for-the-rust-of-us/hypomnesis/actions/workflows/ci.yml/badge.svg)](https://github.com/mi-for-the-rust-of-us/hypomnesis/actions/workflows/ci.yml)
[![crates.io](https://img.shields.io/crates/v/hypomnesis.svg)](https://crates.io/crates/hypomnesis)
[![docs.rs](https://docs.rs/hypomnesis/badge.svg)](https://docs.rs/hypomnesis)
[![MSRV](https://img.shields.io/badge/MSRV-1.88-blue.svg)](https://www.rust-lang.org)
[![license](https://img.shields.io/crates/l/hypomnesis.svg)](https://github.com/mi-for-the-rust-of-us/hypomnesis#license)
[![unsafe: deny](https://img.shields.io/badge/unsafe-deny_(FFI_backends_only)-blue.svg)](https://github.com/rust-secure-code/safety-dance/)
[![NVIDIA](https://img.shields.io/badge/NVIDIA-NVML_%2B_DXGI-76B900.svg?logo=nvidia&logoColor=white)](#capabilities)

**ὑπόμνησις** — *External RAM and VRAM, measured.*

> 🆕 **`0.2.10` is a self-audit release: three silent failures fixed, three CI/process gaps closed.** A full-codebase audit — read as a dogfooding report in its own right (Principle 1: every patch is informed by a real adoption experience, and auditing the crate against its own documented conventions is exactly that, just conducted first-party) — found real gaps behind the crate's own docs and conventions. Three turned out to share one shape, silence: `hmn ps` silently capped at 64 compute processes on busy multi-tenant devices; a single malformed `DXGI` adapter could silently truncate the rest of an enumeration; a partially-broken driver install could make `hmn` print nothing — or, in `--json` mode, emit an indistinguishable `[]` — instead of saying so. All three now say so. `macOS` also joins CI for the first time since becoming a first-class platform in v0.2.3, alongside a `publish.yml` guard against a mistyped release tag. See [`CHANGELOG.md`](CHANGELOG.md) and [`docs/roadmap-v0.2.10.md`](docs/roadmap-v0.2.10.md).

> 🚀 **`0.2.9` adds `GpuDeviceInfo::driver_version` and `hmn --json`.** A candle-mi dogfooding report asked for the NVIDIA driver version to become part of `hypomnesis`'s output: `candle-mi`'s `RESURRECTION.md` provenance log stamps the Rust toolchain per verification run but not the GPU driver, and a driver change can move floating-point results — not hypothetical, since the report's own reference machine hit a `DPC_WATCHDOG_VIOLATION` bugcheck mid-run and needed a driver update (`591.86` → `610.88`) to recover. `driver_version: Option<String>` mirrors v0.2.4's `reserved_bytes` addition, sourced from `NVML` (`nvmlSystemGetDriverVersion`) and — since `nvidia-smi` can genuinely supply this figure, unlike `reserved_bytes` — the `nvidia-smi` fallback too. Renders on the `hmn` device-summary line (`..., driver 610.88`) and via a new `hmn --json` flag on the default subcommand, since no JSON surface existed there before. See [`CHANGELOG.md`](CHANGELOG.md) and [`docs/roadmap-v0.2.9.md`](docs/roadmap-v0.2.9.md).

> 🚀 **`0.2.8` fixes `cargo install hypomnesis` installing no binary, and collapses most Windows `?` rows.** An askesis `canvas` dogfooding report found `cli` was default-off, so a rented-GPU deploy's `cargo install hypomnesis` exited `0` and installed nothing — `cli` is now a default feature, install with a bare `cargo install hypomnesis`. The report also diagnosed most Windows `?` NAME rows as resolvable without elevation; its proposed fix (switch `OpenProcess`'s query right) turned out to already be shipping since v0.2.2 and insufficient — a live test showed both rights fail identically against `dwm.exe`/`csrss.exe`. The real fix, `CreateToolhelp32Snapshot`, reads process names from a system-wide enumeration with no per-process handle and collapses most former `?` rows to real names; what remains renders as an honest `[exited]` or `[protected]` instead of an anonymous `?`. `hmn ps --sort vram`/`committed` round out the release as aliases for `--sort dedicated`. See [`CHANGELOG.md`](CHANGELOG.md) and [`docs/roadmap-v0.2.8.md`](docs/roadmap-v0.2.8.md).

## Table of Contents

- [Install](#install)
- [Usage](#usage)
- [Try it](#try-it)
- [Binary (`hmn`)](#binary-hmn)
- [Capabilities](#capabilities)
- [Feature Flags](#feature-flags)
- [Documentation](#documentation)
- [Used by](#used-by)
- [License](#license)
- [Development](#development)

> **New to hypomnesis?**
> - **What's eating my GPU memory right now?** → [`hmn ps`](#binary-hmn) — every process holding GPU memory, with dedicated-commit and resident-shared columns, on Windows, Linux, and macOS.
> - **Is my training / inference run spilling into system RAM?** (Windows / `WDDM` only — spilling into a *separate* shared budget is a `WDDM` architectural concept; Linux gets a `CUDA` OOM instead, macOS `UMA` has nothing to spill *into*) → the [Is my run spilling?](docs/tutorials/is-my-run-spilling.md) tutorial — wrap the run with [`hmn spill`](#hmn-spill--wddm-spill-detection), read the episode pattern, react.
> - **Is a job that's already running spilling?** (Windows / `WDDM` only, same reason as above) → the [watch tutorial](docs/tutorials/watching-a-running-job.md) — [`hmn watch <pid>`](#hmn-watch--attach-to-a-running-pid) attaches directly, no restart needed.
> - **I want to measure my own process from Rust** → [Usage](#usage) — `Snapshot::now(0)`: process RSS + device-wide + per-process GPU in one call.
> - **I want my loop to stop (or adapt) when spill starts** → `SpillTracker` on [docs.rs](https://docs.rs/hypomnesis) — `observe()` per step, a latched `has_spilled()` to early-stop, an instantaneous `is_spilling()` to adapt; portable via `is_spill_measurable()`.
> - **My numbers look wrong** — `used_bytes` above the card's total, a nonzero SHARED column, all-zeros on Linux/macOS → the [FAQ](docs/FAQ.md), most of it is measured reality, not a bug.
>
> Common questions live in the [FAQ](docs/FAQ.md); every flag is in `hmn --help`.

## Install

```toml
[dependencies]
hypomnesis = "0.2"
```

The default feature set (`nvml`, `dxgi`, `pdh`, `metal`, `nvidia-smi-fallback`, `cli`) covers process RSS, per-process / device-wide GPU memory, the foreign-process GPU listing, `WDDM` spill detection on Windows (`IDXGIAdapter3` + `PDH` + `NVML`) and Linux (`NVML`), a `nvidia-smi` subprocess fallback, and (since v0.2.8) the `hmn` CLI binary itself — see the [Feature Flags](#feature-flags) table for the per-flag breakdown. The `windows`-crate dependency behind `dxgi` / `pdh` is target-conditional — Linux users pay nothing for it.

Library-only consumers who don't want `clap`/`ctrlc` pulled in should pass `--no-default-features` and select source features explicitly:

```toml
hypomnesis = { version = "0.2", default-features = false, features = ["nvml", "dxgi", "pdh"] }
```

On macOS, the `metal` feature is in the default set. Process RSS and per-process GPU memory come from libSystem syscalls (`task_info`, `ledger`, `sysctl`). The device-wide "free" figure comes from `MTLDevice.recommendedMaxWorkingSetSize` via the `objc2-metal` binding (target-conditional, macOS-only) — no libSystem signal on Apple Silicon UMA approximates Apple's own kernel-projected GPU working-set budget within useful accuracy.

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
        // `total_bytes` is the full NVML framebuffer (= `nvidia-smi` Total).
        // `reserved_bytes` is the driver/firmware carve-out *within* it
        // (NVML R510+); allocation headroom is `total - reserved`, which
        // `free_bytes` already reflects.
        if let Some(reserved) = dev.reserved_bytes {
            let reserved_mib = reserved as f64 / (1u64 << 20) as f64;
            println!("  ({:.0} MiB reserved)", reserved_mib);
        }
        // NVIDIA-branded driver string (e.g. "610.88") from NVML or the
        // `nvidia-smi` fallback — not the Windows PnP driver-store form.
        if let Some(driver) = &dev.driver_version {
            println!("  driver {driver}");
        }
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
GPU 0 [NVIDIA GeForce RTX 5060 Ti]: 1.8 / 15.9 GiB used
  (259 MiB reserved)
  driver 610.88
This process: 119 MiB (per-process)
```

## Try it

Real transcripts from the reference machine (Ryzen 9 5950X + RTX 5060 Ti 16 GiB, Windows 11 / `WDDM`):

```
$ hmn                                     # what's on the card?
GPU 0 [NVIDIA GeForce RTX 5060 Ti]: free 13284 MiB / 16311 MiB (259 MiB reserved), driver 610.88

$ hmn --json                              # same data, scriptable
[{"index":0,"name":"NVIDIA GeForce RTX 5060 Ti","total_bytes":17103323136,"free_bytes":14967820288,"used_bytes":2135502848,"reserved_bytes":271581184,"driver_version":"610.88"}]

$ hmn ps                                  # who's holding it? (top rows shown)
PID    NAME                 VRAM     SHARED  DEVICE
30136  QmlRenderer.exe      1.8 GiB  50 MiB  NVIDIA GeForce RTX 5060 Ti
3524   firefox.exe          866 MiB  25 MiB  NVIDIA GeForce RTX 5060 Ti
17020  Discord.exe          401 MiB  5 MiB   NVIDIA GeForce RTX 5060 Ti
13196  Code.exe             335 MiB  6 MiB   NVIDIA GeForce RTX 5060 Ti
...

$ hmn spill -- .\spillforge.exe           # is this run spilling? (the release-validation
                                          # fixture: a 20 GiB working set on the 16 GiB card)
... the wrapped command runs to completion, stdout untouched ...
hmn spill: peak dedicated 14.3 GiB / 15.7 GiB
           peak shared    3.1 GiB (baseline 163 MiB)
           episodes       1 — total 13.1s, longest 13.1s, first +2.0s into run

$ hmn spill --json -- python train.py | jq -e '.measurable and (.spilled | not)'
                                          # CI gate: fail the step when the run spilled

$ hmn watch 15884 --interval 3s           # is an ALREADY-RUNNING job spilling? (same
                                          # forced-spill fixture, attached mid-run)
hmn watch: device 0 [NVIDIA GeForce RTX 5060 Ti], interval 3.0s, watching 1 PID(s)
TIME      PID    NAME            COMMITTED  ΔCOMMIT   SHARED   ΔSHARED   SPILL
+0.0s     15884  spillforge.exe  9.3 GiB    +0 B      86 MiB   +0 B      no
...
+30.1s    15884  spillforge.exe  13.3 GiB   +255 MiB  304 MiB  +0 B      no
+33.1s    15884  spillforge.exe  13.3 GiB   -4 MiB    1.3 GiB  +980 MiB  SPILL
hmn watch: peak dedicated 15.0 GiB / 15.7 GiB
           peak shared    1.4 GiB (baseline 228 MiB)
           episodes       1 — total 0.0s, longest 0.0s, first +33.1s into run
                                          # exits 1 — spill was observed

$ hmn watch --follow-new --interval 3s    # stand guard through TWO sequential
                                          # spillforge runs, back to back
hmn watch: +3.0s followed set changed: entered pid=10640 (spillforge.exe); left pid=21716 (Code.exe)
hmn watch: +18.0s followed set changed: entered pid=13004 (SamsungMagician.exe); left pid=10640 (spillforge.exe)
hmn watch: +21.1s followed set changed: entered pid=29452 (spillforge.exe); left pid=13004 (SamsungMagician.exe)
hmn watch: +36.1s followed set changed: entered pid=13004 (SamsungMagician.exe); left pid=29452 (spillforge.exe)
hmn watch: peak dedicated 15.2 GiB / 15.7 GiB
           peak shared    551 MiB (baseline 155 MiB)
           episodes       2 — total 24.1s, longest 12.0s, first +3.0s into run
                                          # per_pid[] finalizes BOTH spillforge
                                          # PIDs plus 5 desktop processes: 7 total
```

The VRAM column is `WDDM`'s dedicated *commit* — a big process legitimately shows more than the card holds. The SHARED column is *resident* shared-system-memory: the spill signal, and the small nonzero values above are the normal benign baseline. When those two facts surprise you, that's the [FAQ](docs/FAQ.md)'s opening entries.

## Binary (`hmn`)

`hypomnesis` ships a small CLI binary, `hmn`, behind the (since v0.2.8) default-on `cli` feature. Install it with:

```sh
cargo install hypomnesis
```

`--features cli` is still accepted but redundant on the default feature set — only needed if you've already opted out with `--no-default-features` and want the binary back.

Four subcommands:

```sh
hmn                          # device summary (free / total per GPU)
hmn --json                   # same data as a JSON array
hmn ps                       # all GPU processes — discovery command
hmn ps --pid 12345           # filter to one PID
hmn ps --device 0            # filter to one GPU on multi-GPU rigs
hmn ps --json                # scriptable output
hmn ps --sort total           # order by dedicated + shared instead of dedicated alone
hmn spill -- python train.py # run a command, report WDDM spill on exit
hmn spill --interval 250 --json -- ollama serve   # slower polling, JSON report
hmn watch 12345               # attach to an ALREADY-RUNNING PID, watch for spill
hmn watch --top 3 --json      # no PID: auto-select top 3 by committed VRAM, JSONL
hmn watch --follow-new --json # re-select every interval: stand guard over a machine
                               # while arbitrary short-lived work happens
```

Example default output (single NVIDIA dGPU, the maintainer's reference machine — Ryzen 9 5950X has no iGPU, so only one adapter surfaces):

```
GPU 0 [NVIDIA GeForce RTX 5060 Ti]: free 13284 MiB / 16311 MiB (259 MiB reserved), driver 610.88
```

The `(259 MiB reserved)` parenthetical (NVML R510+) is the driver/firmware carve-out *within* the 16311 MiB total — matching `nvidia-smi -q -d MEMORY`'s `Reserved` line. It is elided on backends that don't expose it (DXGI, `nvidia-smi`, Metal, pre-R510).

The `, driver 610.88` suffix is the NVIDIA-branded driver version — from NVML (`nvmlSystemGetDriverVersion`) or the `nvidia-smi` fallback (`--query-gpu=driver_version`); elided on backends that don't expose it (DXGI, Metal, non-NVIDIA adapters). This is the same version string `nvidia-smi`, release notes, and bug reports use — **not** the Windows PnP driver-store form (e.g. `32.0.16.1088`), which NVML/`nvidia-smi` don't expose.

Apple Silicon, idle process (Apple M3 Pro, 36 GiB unified memory):

```
GPU 0 [Apple M3 Pro]: free 28753 MiB / 36864 MiB
```

The `free` figure here is `MTLDevice.recommendedMaxWorkingSetSize` — the kernel-projected GPU working-set budget on UMA — and `total` is `sysctl hw.memsize`. See the [macOS UMA semantics](#macos-uma-semantics-what-free_bytes-means) section below for what these numbers mean and why they differ from the discrete-GPU "free vs total" model.

Illustrative output on a *heterogeneous* machine (NVIDIA dGPU + Intel/AMD iGPU on Windows). Not yet verified end-to-end on real hardware — see [`docs/roadmap-v0.2.0.md`](docs/roadmap-v0.2.0.md) "Verification plan":

```
GPU 0 [NVIDIA GeForce RTX 5060 Ti]: free 13284 MiB / 16311 MiB (259 MiB reserved), driver 610.88
GPU 1 [Intel Iris Xe Graphics]: free 32768 MiB / 32768 MiB
```

(The Intel iGPU line has no reserved parenthetical or driver suffix — `DXGI` does not expose the NVML carve-out or an NVIDIA driver string, so `reserved_bytes` and `driver_version` are both `None` there.)

`hmn ps` (illustrative — empty on machines with no active CUDA workload):

```
PID    NAME              VRAM      SHARED   DEVICE
12345  lm-studio.exe     8.2 GiB   45 MiB   NVIDIA GeForce RTX 5060 Ti
67890  python.exe        1.4 GiB   0 MiB    NVIDIA GeForce RTX 5060 Ti
```

A one-line summary is written to **stderr** after each `hmn ps` run:

```
hmn: 2 GPU processes found (9.6 GiB committed total).
hmn: 0 GPU processes found matching pid=99 device=0.   # with filters
```

The stderr summary is always printed, even when the table is empty, so interactive users get an unambiguous "command worked, here's the count" line without breaking stdout's scriptability. Pipelines like `hmn ps | awk 'NR>1 {print $1}'` or `hmn ps --json | jq` work as expected. Redirect `2>/dev/null` to suppress the summary.

`--sort <KEY>` (`dedicated` default, `shared`, or `total`) reorders both the text table and `--json` output — three different questions, not interchangeable: `dedicated` ("who do I kill to free VRAM?"), `shared` ("who is currently being paged out?" — a symptom, not a cause), `total` (dedicated + shared, "who is the biggest GPU-memory citizen overall?"). `dedicated` also accepts `vram` and `committed` as aliases — the words the rest of the tool's own vocabulary uses for the same quantity (the `ps` column header and `watch`'s `COMMITTED` column, respectively). Tie-breaks (name ascending, then PID ascending) are identical regardless of key. `shared`/`total` are a documented no-op ordering on Linux and macOS, where `shared_used_bytes` is always `0`.

**Limitations** (intrinsic to the underlying data sources, not bugs — longer-form answers to the recurring ones live in the [FAQ](docs/FAQ.md)):

1. **Per-platform semantics differ — compute-only on Linux, all-GPU-users on Windows.** `hmn ps` on Linux (via `NVML`'s `nvmlDeviceGetComputeRunningProcesses_v3`) enumerates only processes with an active `CUDA` context — browsers using GPU compositing, games, and pure-graphics apps do not appear. `hmn ps` on Windows (via `PDH`'s `\GPU Process Memory(*)\Dedicated Usage`) enumerates **every** process holding GPU memory — the desktop compositor (`dwm.exe`), browsers, games, and `CUDA` / compute alongside. The semantic shift reflects what each platform's kernel actually accounts for; check the `source` field on `GpuProcessEntry` if you care about the distinction.

2. **Windows `used_bytes` reflects WDDM's *dedicated commit*, not resident set.** Under `WDDM` a process can commit GPU allocations exceeding physical `VRAM` — the kernel pages them via the shared system memory budget. Numbers exceeding the device's total `VRAM` are real, not bugs: they match Task Manager's `Dedicated GPU memory` column. (Example: on a 16 GiB GPU, a heavy browser process can show 15+ GiB committed.)

3. **The SHARED column (Windows / `PDH` only) shows *resident* shared-system-memory bytes — the `WDDM` spill signal.** Matches Task Manager's `Shared GPU memory` column for the same PID. A benign baseline (staging/upload heaps, tens of MiB) is normal by design; the spill signature is this number *growing* while dedicated `VRAM` saturates — which is exactly what `hmn spill` and the library's `SpillTracker` detect. Always `0` on Linux and macOS (no shared-residency counter exists there).

4. **(Windows) `?` in the NAME column is now rare.** Before v0.2.8, any PID `OpenProcess` couldn't resolve — including plenty of ordinary foreign-user/`SYSTEM` processes like `dwm.exe` and `csrss.exe` — rendered as a bare `?`. As of v0.2.8, a `Toolhelp32Snapshot` fallback resolves those the same way `Get-Process`/Task Manager do (a system-wide process enumeration that reads names without opening a per-process handle, so it isn't subject to the same access check `OpenProcess` is), collapsing the vast majority of former `?` rows to real names non-elevated:

   ```
   PID    NAME                         VRAM      SHARED  DEVICE
   26940  dwm.exe                      1001 MiB  4 MiB   NVIDIA GeForce RTX 5060 Ti
   18880  csrss.exe                    39 MiB    63 MiB  NVIDIA GeForce RTX 5060 Ti
   4      [kernel]                     4 MiB     0 MiB   NVIDIA GeForce RTX 5060 Ti
   ```

   *(real capture, non-elevated shell — both rows rendered `?` before v0.2.8)*

   What remains genuinely unresolvable now renders as one of two honest brackets instead of an anonymous `?`: **`[exited]`** — the process exited between `hypomnesis`'s VRAM sample and the name lookup; elevation would not help, this is a timing race, not a permission wall. **`[protected]`** — the `Toolhelp32Snapshot` fallback itself could not be taken (very rare — resource exhaustion), so "exited" vs. "still running but unresolvable" can't be told apart. The Windows kernel itself (`PID 4`) continues to render as `[kernel]`, not `?` or `[protected]` — there is no executable image to read, so it's special-cased. This `[exited]`/`[protected]` distinction is Windows-only; Linux/macOS unresolved rows remain a bare `?` in the table (`name: None` underneath), since there is no equivalent false-wall-vs-real-wall gap to collapse there — see the [FAQ](docs/FAQ.md#what-does-a--in-the-name-column-mean--and-when-do-i-need-elevation) for the platform breakdown.

   *Security note.* A `[protected]` row (or a bare `?` on Linux/macOS) that does not resolve under elevation is one of: a process owned by another user, a process running as `SYSTEM` / `LOCAL SERVICE` / `NETWORK SERVICE`, a `PPL`-protected process, or (rarely, post-v0.2.8) the snapshot API itself failing. None of these are intrinsically malicious — but on a single-user desktop, an *unexpected* unresolved row holding substantial VRAM is worth investigating: a malicious local process (including a privileged-or-cross-user AI agent) using GPU resources would land in exactly this set. The `(N protected — re-run elevated for names)` parenthetical on the `hmn ps` summary line is intentionally surfaced because this distinction is security-relevant, and (as of v0.2.8) counts `[protected]`/`None` rows and the rare pre-`WDDM 2.0` `nvidia-smi` fallback's literal `?` name (limitation 5, below) — not `[exited]`, since elevation can't help a process that's already gone. `hypomnesis` is a measurement tool, not a malware scanner — but its honesty about the gap is itself a defensive primitive.

5. **Pre-`WDDM 2.0` Windows falls back to `nvidia-smi --query-compute-apps`.** Vanishingly rare in 2026 — `WDDM 2.0` shipped with Windows 10 1709 (October 2017). On the fallback path, `hmn ps` is compute-only (matching the Linux semantic) and `used_memory` may be `[N/A]` under `WDDM` (parser drops those rows). The `source` field on `GpuProcessEntry` reads `GpuQuerySource::NvidiaSmi` rather than `GpuQuerySource::Pdh` on this path.

6. **`R570`-class driver-bug filtering.** The `u64::MAX` sentinel (`R570` driver bug on `RTX 5060 Ti` and similar consumer GeForce cards) and the `used > total` corruption checks are applied per-row in `hmn ps`; affected rows are dropped rather than reported as garbage.

7. **macOS `used_bytes` reflects currently-resident GPU pages.** The kernel evicts idle Metal pages from a process's `graphics_footprint`, so the same PID may report different values across successive `hmn ps` calls when its working set has cooled. This is the same resident-bytes semantics as Windows `WorkingSetSize` and Linux `VmRSS` — not a macOS quirk, the cross-platform contract.

8. **macOS cross-user PIDs are silently skipped.** The per-PID `ledger` syscall returns `EPERM` for processes owned by another user. `hmn ps` enumerates same-user PIDs only by default; run elevated (`sudo hmn ps`) to include cross-user PIDs such as `WindowServer`, `kernel_task`, and other-user-owned applications.

### `hmn spill` — WDDM spill detection

`hmn spill -- <command>` wraps a command `time(1)`-style: it spawns the command with inherited stdio, polls the spill state at a configurable interval (`--interval <MS>`, default **100 ms**), prints a `SpillReport` to **stderr** when the command exits, and **passes the wrapped command's exit code through** (so it drops into existing scripts and CI steps unchanged):

```sh
hmn spill -- python train.py
# ... train.py runs to completion, its stdout untouched ...
# SpillReport prints here (stderr):
#   hmn spill: peak dedicated 14.3 GiB / 15.7 GiB
#              peak shared    3.1 GiB (baseline 163 MiB)
#              episodes       1 — total 13.1s, longest 13.1s, first +2.0s into run
```

*(The report block is real output from the release-validation forced-spill run on the reference RTX 5060 Ti — a 20 GiB working set forced onto the 16 GiB card; `python train.py` stands in for whatever you wrap.)*

**What "spill" means here — residency, not commitment.** Under `WDDM`, a process can *commit* GPU memory far past dedicated `VRAM` with zero bytes actually paged out (see Limitation 2 above — that's every big PyTorch process, and it is *not* spill). Spill is **resident shared-system-memory growth while dedicated `VRAM` saturates** — the state where `VidMm` is actually paging GPU allocations over `PCIe` and your throughput craters. `hmn spill` flags an *episode* only when both hold: adapter dedicated-resident ≥ 85% of capacity **and** shared-resident has risen ≥ 256 MiB above its start-of-run baseline (staging heaps live in shared memory by design, so a baseline is normal). Transient spills are first-class: each contiguous spilling stretch is one episode, so *many short episodes* reads as "marginally over budget — shave the batch size" while *one sustained episode* reads as "genuinely over — rethink model / precision".

`--json` emits the report as a single JSON object on stdout (fields: `measurable`, `spilled`, `observations`, `baseline_shared_bytes`, `peak_shared_bytes`, `peak_dedicated_bytes`, `dedicated_limit_bytes`, `total_spill_duration_ms`, `episodes[]`). Check `measurable` before trusting `spilled: false` — on Linux and macOS the wrapped command still runs, but there is nothing to measure (`is_spill_measurable()` is `false`: normal `CUDA` OOMs rather than silently paging, and Apple `UMA` has nothing to spill *into*), so stderr says `spill not measurable on this platform` instead of printing a misleading all-zeros report.

Library consumers get the same primitive as [`SpillTracker`](https://docs.rs/hypomnesis) — `observe()` in their own loop, an instantaneous `is_spilling()` and a latched `has_spilled()`, and the episode history via `into_report()`. One honest limitation, shared by both: there is no background thread, so **a spill shorter than the gap between two observations is invisible** — `hmn spill`'s 100 ms default is the answer when temporal resolution matters more than in-loop integration.

### `hmn watch` — attach to a running PID

`hmn spill` only wraps a *new* command. `hmn watch [PID...]` attaches to
process(es) that are **already running** — the gap a rhyme-mdlm dogfooding
report hit three times triaging a 15-hour training campaign, hand-rolling
"two `hmn ps` samples minutes apart, diff by eye" every time
([`docs/dogfooding-feedbacks/dogfooding-spill-triage-watch-mode.md`](docs/dogfooding-feedbacks/dogfooding-spill-triage-watch-mode.md)).
Same `SpillTracker` core as `hmn spill`, on a timer instead of a wrapped
child — a `time(1)`-style scrolling sampler, **not a TUI** (same discipline
as `hmn spill`; see [Why no `hmn kill`?](#why-no-hmn-kill) for the same
scope-discipline reasoning applied to "why not a live-refresh dashboard"):

```sh
hmn watch 21844                          # attach to a known PID
hmn watch                                # no PID: auto-select top 5 by committed VRAM
hmn watch --top 3 --interval 30s --duration 10m --json   # tune interval/window, stream JSONL
```

With no PID, `hmn watch` auto-selects the top `--top` (default 5) processes
by committed `VRAM` from the first sample and keeps that fixed set for the
run. Each interval prints one row per watched PID — committed / shared
`VRAM`, per-interval deltas, and a SPILL flag (the same adapter-wide
condition `hmn spill` uses, reused unchanged). A watched PID absent from a
sample renders `0 B` — `hmn watch` cannot distinguish "exited" from
"currently holds no GPU memory" and does not auto-stop on this basis; use
`--duration` or Ctrl+C. `--interval` / `--duration` take duration strings
(`500ms`, `30s`, `5m`, `1h`, or a bare number of seconds) rather than raw
milliseconds.

**`--follow-new`** (auto-select mode only — a hard error combined with an
explicit PID) re-runs the top-`--top` selection *every* interval instead of
once at attach, for the shape a candle-mi dogfooding report hit running
`hmn watch` alongside a 19-process sequential test suite: successive
short-lived GPU processes that are all born *after* attach, which a frozen
selection never sees. A PID entering starts with a fresh baseline; a PID
leaving (exited, or dropped below rank `--top`) stops appearing in the live
rows and is *finalized* into the closing summary's `per_pid[]` with its
peak/baseline, instead of rendering `0` forever — so the summary becomes a
roster of everyone who mattered during the watch, not just whoever was on
top at `t=0`. A stderr breadcrumb reports each change
(`entered pid=... (name); left pid=... (name)`).

Runs until `--duration` elapses or Ctrl+C, then prints a closing summary
(the same `SpillReport` shape as `hmn spill`, plus a per-PID peak/baseline
table) and exits **`0`** if spill was never observed, **`1`** if it was at
least once, **`2`** on a hard error — designed for a watchdog script to check
directly:

```sh
hmn watch 21844 --duration 5m
[ $? -eq 1 ] && echo "spilled in the last 5 minutes"
```

`--json` streams JSON Lines to stdout — one `{"kind":"sample",...}` object
per PID per interval as it happens, plus a final `{"kind":"summary",...}`
object (the `SpillReport` fields plus `per_pid[]`) when the watch ends;
pipeable to `jq -c` live. Full walkthrough, including the campaign that
motivated it:
[Triage a job that's already running](docs/tutorials/watching-a-running-job.md).

### Composable workflows

`hmn ps --json` exists for scripting and survives across platforms (same JSON shape on Windows, Linux, and macOS). Two recipes that have come up in dogfooding:

**Top-5 GPU consumers** (any platform with `jq` installed):

```sh
hmn ps --json | jq 'sort_by(-.used_bytes) | .[:5]'
```

(`hmn ps --sort dedicated` — the default — now covers this natively for the whole table, in both text and `--json` form; the `jq` recipe stays handy for slicing to a specific top-N or sorting by a field `--sort` doesn't offer, like `pid` or `name`.)

**Terminate any process holding more than 1 GiB of `VRAM`** — the JSON output composes with the platform's native kill command. Windows (PowerShell or cmd):

```sh
hmn ps --json | jq -r '.[] | select(.used_bytes > 1073741824) | .pid' | ForEach-Object { taskkill /F /PID $_ }
```

Linux / macOS:

```sh
hmn ps --json | jq -r '.[] | select(.used_bytes > 1073741824) | .pid' | xargs -r kill -TERM
```

(Use `kill -KILL` instead of `-TERM` if you want the hard variant; `-r` skips empty input.)

**Fail a CI step when a run spilled** — `hmn spill --json` composes the same way (`jq -e` sets the exit code from the expression):

```sh
hmn spill --json -- python train.py | jq -e '.measurable and (.spilled | not)'
```

**Watch a specific process's shared-residency from outside** (the `train_guarded.py`-style watchdog — key the guard off `shared_used_bytes`, never off the commit figure):

```sh
hmn ps --json | jq '.[] | select(.pid == 12345) | .shared_used_bytes'
```

#### Why no `hmn kill`?

A `hmn kill <pid>` subcommand was considered for v0.2.3 and rejected to preserve `hypomnesis`'s "measurement, not control" scope discipline. Process termination is not a *measurement* operation — it's a control operation, and one with platform-specific permission models (`taskkill` vs `kill -SIGNAL` vs `sudo kill`) that `hmn` would inevitably get wrong on at least one platform. Piping JSON to the platform's native killer is more honest about what's happening, more flexible (filter on any field, not just PID), and keeps `hypomnesis`'s API surface small.

#### Why no `hmn spill --kill` / `--throttle`?

Same discipline, same answer (recorded here so it isn't re-argued in a future PR — v0.2.5 considered and rejected both). What to *do* about a spill — kill the run, drop the batch size, switch to CPU — is the consumer's decision, wired through whatever primitive their workload already uses. `hmn spill --json` composed with `jq` and the platform's native killer covers the automation case; the library's `SpillTracker` deliberately exposes queryable state (`is_spilling()` / `has_spilled()`) and no callbacks, no background thread, and no built-in debounce for the same reason.

## Capabilities

| Metric | Windows | Linux | macOS |
|--------|---------|-------|-------|
| Process RSS | `K32GetProcessMemoryInfo` | `/proc/self/status` (no `unsafe`) | `task_info(TASK_VM_INFO_PURGEABLE).phys_footprint` |
| Device-wide GPU memory | `NVML` (`nvml.dll`) | `NVML` (`libnvidia-ml.so.1`) | `sysctl hw.memsize` (total) + `MTLDevice.recommendedMaxWorkingSetSize` (free) |
| Device reserved memory | `NVML` v2 (`nvmlDeviceGetMemoryInfo_v2`, R510+) | `NVML` v2 (R510+) | n/a (`None` — UMA has no carve-out) |
| Driver version | `NVML` (`nvmlSystemGetDriverVersion`) + `nvidia-smi` fallback (`--query-gpu=driver_version`) | same as Windows | n/a (`None` — no NVIDIA driver on Apple Silicon) |
| Per-process GPU memory | `DXGI` (`IDXGIAdapter3::QueryVideoMemoryInfo`) | `NVML` (`nvmlDeviceGetComputeRunningProcesses`) | `ledger(LEDGER_ENTRY_INFO_V2).graphics_footprint` |
| GPU-process listing (other PIDs) | `PDH` (`\GPU Process Memory(*)\Dedicated Usage` + `Shared Usage`) + `OpenProcess` / `QueryFullProcessImageNameW`; `nvidia-smi` fallback | `NVML` + `/proc/<pid>/comm` (compute-only) | `proc_listpids` + per-PID `ledger` + `proc_pidpath` (same-user; `sudo` for cross-user) |
| Spill detection (`SpillTracker`, `hmn spill`, `hmn watch`) | `PDH` `\GPU Adapter Memory(*)\Dedicated Usage` + `Shared Usage` (`WDDM 2.0`+) | n/a (`is_spill_measurable()` = `false` — normal `CUDA` OOMs rather than silently paging) | n/a (`false` — `UMA` has nothing to spill *into*) |
| Fallback | `nvidia-smi` subprocess | `nvidia-smi` subprocess | none (libSystem syscalls always succeed on Apple Silicon) |

`hypomnesis` uses `IDXGIAdapter3` on Windows because `WDDM` means the kernel memory manager — not the NVIDIA driver — owns GPU allocations, so `NVML`'s per-process query returns `NOT_AVAILABLE` under Windows. `DXGI 1.4` is the only reliable per-process source. On Linux, `NVML`'s `nvmlDeviceGetComputeRunningProcesses_v3` returns true per-process figures. On Apple Silicon (M-series), the GPU shares system DRAM via unified memory architecture (UMA), so `hw.memsize` is both the system RAM total and the GPU memory pool.

The crate handles two known driver bugs out of the box:

1. **`NVML` `u64::MAX` sentinel** — some `R570`-series drivers report `0xFFFFFFFFFFFFFFFF` for every running process's memory (observed on `RTX 5060 Ti`). `hypomnesis` detects this and falls back to `nvidia-smi`.
2. **`used > total` corruption** — sanity-checks each per-process reading against the device-wide total; falls back to `nvidia-smi` on detected corruption.

### macOS UMA semantics: what `free_bytes` means

On a discrete GPU, `free_bytes` is "untaken bytes in the VRAM pool" — a hard number bounded by the card's physical memory. On Apple Silicon the GPU has no separate pool: it shares system DRAM via unified memory architecture (UMA). `hypomnesis` therefore reports `free_bytes` as `MTLDevice.recommendedMaxWorkingSetSize` — the kernel-projected GPU working-set budget that Apple's Metal driver itself computes, factoring in wired-page reserves, system memory pressure, and the kernel's known compression / eviction capability.

Two consequences worth noting:

- **The number changes slowly under load.** Apple's driver smooths it; it is a policy figure, not an instant-state reading. Expect it to shrink modestly as system memory pressure rises and recover as pressure abates.
- **Per-process `used_bytes` (from `graphics_footprint`, used by `gpu_processes()` and `process_gpu_info()`) reflects currently resident GPU pages**, matching the resident-bytes semantics of Windows `WorkingSetSize` and Linux `VmRSS`. Idle apps' Metal pages get evicted by the kernel; the same PID may report different values across calls. This is the contract Windows and Linux already exhibit, not a macOS-specific quirk.

## Feature Flags

| Feature | Default | Description |
|---------|---------|-------------|
| `nvml` | yes | `NVML` dynamic load via `libloading` (Linux + Windows-`WDDM` device-wide) |
| `dxgi` | yes | Windows per-process `VRAM` via `IDXGIAdapter3` (no-op on non-Windows) |
| `pdh` | yes | Windows foreign-process `VRAM` listing (`\GPU Process Memory(*)\Dedicated Usage` + `Shared Usage`) and the `\GPU Adapter Memory(*)` counters backing `SpillTracker`'s live path, under `WDDM 2.0`+ (no-op on non-Windows; depends on `dxgi`). `SpillTracker` itself compiles everywhere regardless — without this feature it is simply never measurable. |
| `metal` | yes | macOS device-wide GPU budget via `objc2-metal` (`MTLDevice.recommendedMaxWorkingSetSize`); no-op on non-macOS. RAM and per-process GPU paths are libSystem-only and unaffected by this flag. |
| `nvidia-smi-fallback` | yes | Subprocess fallback when `NVML` / `DXGI` / `PDH` fail or are otherwise unavailable (e.g. pre-`WDDM 2.0` Windows) |
| `report` | no | `MemoryReport` delta + `print_delta` / `print_before_after` / `ram_mb` / `vram_mb` helpers (`candle-mi` parity, candidate for `candle-mi` v0.2 migration via Cargo flag flip); `format_free` / `print_free` / `format_total` / `format_used` formatting helpers on `GpuDeviceInfo` |
| `debug-output` | no | Print raw `NVML` / `DXGI` / `PDH` / `nvidia-smi` / spill values to stderr (diagnostic) |
| `cli` | yes (since v0.2.8) | Build the `hmn` CLI binary (pulls `clap` 4 and `ctrlc` as deps — the latter backs `hmn watch`'s graceful Ctrl+C summary). Library-only consumers who don't want the extra deps use `--no-default-features` and select source features explicitly. |
| `test-helpers` | no | Expose `GpuDeviceInfoBuilder`, `GpuProcessEntryBuilder`, and `SpillReportBuilder` for downstream tests that need synthetic fixtures. Default-off, additive — production code must never enable it. |

## Documentation

| Doc | |
|-----|---|
| [FAQ](docs/FAQ.md) | Common questions — commit vs resident, `hmn spill` vs `hmn watch`, the SHARED baseline, the spill condition and its 85% threshold, per-platform zeros, `?`/`[exited]`/`[protected]` rows and elevation, threading, polling cost, upgrading |
| [Tutorial: Is my run spilling?](docs/tutorials/is-my-run-spilling.md) | Walkthrough: wrap a run with `hmn spill`, read the episode pattern, attribute per-PID, wire `SpillTracker` into your own loop |
| [Tutorial: Triage a job that's already running](docs/tutorials/watching-a-running-job.md) | Walkthrough: attach `hmn watch` to a running PID, read the live SPILL column, script the exit code — and `--follow-new` to stand guard over a machine through a suite of short-lived jobs |
| [ROADMAP](ROADMAP.md) | Status snapshot: shipped, committed, speculative, and deliberately-rejected ideas |
| [Per-release roadmaps](docs/) | The detailed plan behind each release (`docs/roadmap-vX.Y.Z.md`), including live-measured deviations |
| [The brief](docs/hypomnesis-brief.md) | Why this crate exists — Plato, the v0.1.x VRAM saga, the extraction from `candle-mi` |
| [CHANGELOG](CHANGELOG.md) | Release history |

## Used by

- [candle-mi](https://github.com/mi-for-the-rust-of-us/candle-mi) — mechanistic-interpretability toolkit for `candle`. As of **v0.1.16** it deletes its in-tree measurement FFI and delegates `src/memory.rs` to `hypomnesis` (lean feature set: `nvml`, `dxgi`, `nvidia-smi-fallback`, `metal`), flattening a `hypomnesis::Snapshot` into its own `MemorySnapshot`. Its v0.1.16 dogfooding report — live-validated on an `RTX 5060 Ti` (16 GiB, Windows / `WDDM`) — drove v0.2.4's `reserved_bytes` addition. Its `resurrect.ps1` verification pipeline's live-caught `DPC_WATCHDOG_VIOLATION` bugcheck drove v0.2.9's `driver_version` addition, so the pipeline's provenance record can stamp the GPU driver alongside the Rust toolchain. Its `scripts/resurrect.ps1` oracle suite is a load-bearing `hmn spill --json` / `hmn watch` consumer and the source of the v0.2.7 `--follow-new` / `ps --sort` dogfooding report.
- [hf-fetch-model](https://github.com/mi-for-the-rust-of-us/hf-fetch-model) — Hugging Face model weights and metadata fetcher (uses `device_info` for `inspect --check-gpu`)

## License

Licensed under either of [Apache License, Version 2.0](LICENSE-APACHE) or [MIT License](LICENSE-MIT) at your option.

## Development

- Exclusively developed with [Claude Code](https://claude.com/product/claude-code) (dev)
- Git workflow managed with [Fork](https://fork.dev/)
- All code follows [CONVENTIONS.md](CONVENTIONS.md), derived from [Amphigraphic-Strict](https://github.com/PCfVW/Amphigraphic-Strict)'s [Grit](https://github.com/PCfVW/Amphigraphic-Strict/tree/master/Grit) — a strict Rust subset designed to improve AI coding accuracy.
