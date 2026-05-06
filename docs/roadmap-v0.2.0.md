# `hypomnesis` v0.2.0 — roadmap

> *Wider, not taller. Same job — more callers can ask, more devices can answer.*

---

## Why v0.2.0 (and not 0.1.x patches)

Nothing in this roadmap is a breaking change. Every item is additive under the `#[non_exhaustive]` policy already in place, so the contents could ship as a stack of `0.1.x` patches without breaking any caller.

**0.2.0 is chosen as a signal, not a forced bump.** Two surface expansions warrant a minor-version tour:

1. A shipped binary (`hmn`) — even gated behind a default-off feature, this is a new public artifact with its own UX.
2. Multi-adapter enumeration on Windows — `Snapshot::all()` is a structural addition to the device side of the API.

Anything that turns out to be controversial in pre-release review can be peeled back into a 0.1.x patch instead.

## Scope

Three waves, ordered for testability — each wave's output is verifiable on the maintainer's actual hardware (Windows + RTX 5060 Ti + AMD iGPU; Ubuntu WSL2 + RTX 5060 Ti via NVIDIA's CUDA-on-WSL driver).

### Wave A — `GpuDeviceInfo::print_free` (shipped)

> **Status: implemented.** See commit `9a965ec` and CHANGELOG `[Unreleased]`.
> The original sketch proposed a `free_bytes()` method *and* a `print_free`
> helper. Implementation review found that `pub free_bytes: u64` already
> exists as a field on `GpuDeviceInfo` (since v0.1.0), so the proposed
> method would have collided with the field name. The field was kept,
> and only the print helper was added as a feature-gated method on
> `GpuDeviceInfo` — see *"Lessons"* below.

**Motivation (still applies).** The free-VRAM-right-now question — *"if I load this model now, will it fit?"* — should be a one-liner. The existing `dev.free_bytes` field already delivers that at the data layer; the wave adds the matching `report`-feature print helper for the LM-Studio-style headroom-check log line.

**API delivered:**

```rust
// Behind feature = "report" — methods on GpuDeviceInfo, mirroring
// Snapshot::ram_mb / vram_mb (feature-gated impl on the type):
impl GpuDeviceInfo {
    pub fn format_free(&self) -> String { ... }
    pub fn print_free(&self) { ... }
}
```

Format: `  GPU <idx>: free <N> MB / <T> MB[ [<adapter name>]]\n`. `print_free` delegates to `format_free`, which locks the format under unit-test verification (4 inline tests covering name-present / name-absent / fully-allocated device with `free=0` / `print_*` smoke).

**Lessons (to apply when drafting Wave D+):**

- **Verify against the current struct layout before sketching API additions.** The `free_bytes()` proposal would have failed to compile because the field already had that name. A 30-second `Read` of `src/snapshot.rs` would have caught it before the roadmap shipped. Future waves should grep the existing `pub` items for any name they propose to add.
- **Driver-reported numbers can be more accurate than derived ones.** On the NVML path, the field stores `nvmlDeviceGetMemoryInfo`'s `free` — the driver knows about reservations and alignment that `total - used` does not. The proposed `free_bytes()` method would have *thrown away* that precision on the most common path. When the field exists, prefer it over a recomputation method.
- **Method symmetry beats roadmap-literal signatures.** The roadmap sketched `print_free(dev: &GpuDeviceInfo)` as a free function; implementation chose `dev.print_free()` to mirror the existing `Snapshot::ram_mb` / `vram_mb` pattern. Free functions are fine in isolation, but consistency with the type's existing feature-gated methods wins on discoverability.

**Out of this wave (still applies).** Free RAM (`/proc/meminfo` `MemAvailable`, Windows `GlobalMemoryStatusEx`) is *not* added. That's a different job and would justify a `system_free_bytes()` call only when a real consumer asks. Process RSS already covers "what this process holds"; "what the OS has free" is a system-info question.

**Effort actual.** ~30 minutes including the field/method-collision discussion and 4 unit tests.

### Wave B — Multi-adapter enumeration on Windows

**Motivation.** Today `Snapshot::now(device_index: u32)` answers for one device. The DXGI path already calls `IDXGIFactory1::EnumAdapters1`, which returns *every* adapter (the existing code skips Microsoft Basic Render Driver by name, then picks one). Real machines — including the maintainer's — have iGPU + dGPU. Closing this gap is "finish the device side," not new work.

**API additions:**

```rust
impl Snapshot {
    /// Snapshot every NVIDIA dGPU plus, on Windows, every additional
    /// DXGI adapter that exposes non-zero VRAM (e.g. AMD/Intel iGPU).
    pub fn all() -> Result<Vec<Self>, HypomnesisError> { ... }
}
```

`Snapshot::now(idx)` keeps its current shape unchanged. `Snapshot::all()` is purely additive.

**Cross-platform parity, honest:**

| Platform | What `all()` returns |
|---|---|
| Windows | NVIDIA dGPU(s) via NVML + every other DXGI adapter with non-zero VRAM (iGPU). iGPU `total_bytes` is the WDDM shared-memory budget, not dedicated VRAM. |
| Linux | NVIDIA dGPU(s) via NVML only. AMD iGPUs do not surface — NVML is NVIDIA-only and there is no AMD/Intel backend yet. |

The Linux limitation is a property of the data sources, not a v0.2 bug — the AMD ROCm and Apple Metal backends remain out of scope (see §Out of scope below).

**Effort.** Small. The DXGI loop already enumerates all adapters; v0.2 keeps the full list rather than reducing to one. Plus a Linux-side NVML device-count loop.

### Wave C — `hmn` CLI behind `cli` feature

**Motivation.** Hypomnesis is a library; for a free-VRAM check or a process-VRAM lookup the natural shape is a one-shot terminal command. `nvidia-smi` exists but has two real gaps `hmn` closes:

- On Windows under WDDM, `nvidia-smi`'s per-process compute-app query returns blank — the same gap the DXGI path was built to close.
- Output shape is identical on Windows and Linux, scriptable from PowerShell or bash without per-platform parsing.

**Subcommand surface:**

```
hmn                    # device summary (free/used/total per GPU)
hmn ps                 # all GPU processes — discovery command
hmn ps --pid 12345     # filter to one PID
hmn ps --device 0      # filter to one GPU on multi-GPU rigs
hmn ps --json          # scriptable output
```

`hmn ps` columns:

```
PID     NAME              VRAM      DEVICE
12345   lm-studio.exe     8.2 GiB   RTX 5060 Ti
67890   python.exe        1.4 GiB   RTX 5060 Ti
```

**Naming choice for `ps`.** `docker ps` is the precedent: a domain-scoped `ps` that lists what *this* tool tracks (here, processes holding GPU memory), not Unix processes. `--help` clarifies *"processes using GPU memory."* Alternatives (`gps`, `rp`, `gpu-ps`) considered and rejected — see commit log / PR discussion.

**Data sources, all already in the library:**

| Platform | `hmn ps` backend |
|---|---|
| Linux | `nvmlDeviceGetComputeRunningProcesses_v3` for `(pid, used_memory)`; `/proc/<pid>/comm` for name |
| Windows | `nvidia-smi --query-compute-apps=pid,name,used_memory --format=csv` (already plumbed as a fallback) |

No new backends. The binary shapes data the library already collects into a table.

**Packaging.**

```toml
[features]
cli = ["dep:clap"]              # default-off — library users don't pull clap

[[bin]]
name = "hmn"
required-features = ["cli"]
```

`cargo install hypomnesis --features cli` installs `hmn`. Splitting into a separate `hypomnesis-cli` crate later remains cheap if it grows; not needed now.

**Effort.** Weekend. Most of the work is `clap` arg parsing and table formatting; the backends are reused as-is.

## Honest limitations baked into Wave C from day one

These are properties of the underlying data sources and must be documented in `--help` and `README.md`, not papered over:

1. **Compute-only.** Both `nvmlDeviceGetComputeRunningProcesses_v3` (Linux) and `nvidia-smi --query-compute-apps` (Windows) see CUDA workloads. Browsers using GPU compositing, games, and pure-graphics apps do not appear in `hmn ps`.
2. **Windows process names may be `?`.** Querying another PID's image name can fail for protected processes. Show `?` rather than failing the whole command.
3. **WDDM bug parity.** The same `u64::MAX` sentinel and `used > total` corruption checks the library already handles must apply per-row in `hmn ps`. Rows that trip a sentinel fall back to `nvidia-smi` (already plumbed) or display as `?`.
4. **Windows `--all` per-process attribution is `nvidia-smi`-backed.** DXGI's `QueryVideoMemoryInfo` only answers for the *calling* process — there is no "for that other PID" version, and NVML's per-process query returns `NOT_AVAILABLE` under WDDM (the original reason DXGI exists in this crate). So `hmn ps` on Windows is honest-but-second-class compared to Linux NVML's clean enumeration.

## Out of scope for v0.2.0 (carrying the discipline forward)

| Idea | Why deferred |
|---|---|
| AMD ROCm backend (`rocm_smi_lib`) | Maintainer has no AMD dGPU; shipping untested code violates project discipline. Wait for hardware access or a real PR. |
| Apple Metal backend (`MTLDevice.currentAllocatedSize`) | Same reason — no Apple machine. |
| Peak / high-water-mark tracking (`MemoryReport::fold`) | Real motivating use case is *point-in-time free* (your LM Studio scenario), already solved by Wave A. Peak-over-time is a benchmark-loop need, not a current consumer ask. |
| RSS column in `hmn ps` | RSS for *another* PID needs `OpenProcess + K32GetProcessMemoryInfo` on Windows (permission caveat) and `/proc/<pid>/status` on Linux. Start of "extend the crate beyond GPU memory." Ship `ps` VRAM-only first; add RSS only when a consumer asks. |
| AMD iGPU on Linux | NVML doesn't see it; would need a separate Linux DRM/sysfs path. Defer with the AMD ROCm backend. |
| TUI / live mode (`hmn top`) | That's `nvtop`'s job. Shell loop with `watch -n 1 hmn` is the Unix answer. |
| `hmn` reading from another machine over SSH/RPC | Not in scope; users run `ssh host hmn`. |

## Verification plan (Wave 2 of v0.2.0)

The same hardware that validated v0.1.0 covers v0.2.0:

- **Windows host (RTX 5060 Ti + AMD iGPU)** — verifies Wave B's multi-adapter enumeration with two real adapters of different vendors. Verifies Wave A's `free_bytes` matches `nvidia-smi --query-gpu=memory.free` to within the WDDM-commit-vs-nvidia-smi rounding gap. Verifies `hmn ps` against `nvidia-smi --query-compute-apps`.
- **Ubuntu WSL2 (RTX 5060 Ti via NVIDIA's CUDA-on-WSL driver)** — verifies the Linux NVML-only path of `Snapshot::all()` (single device, no AMD iGPU surfaced — that's expected). Verifies `hmn ps` enumerates a Python+CUDA process correctly.
- **R570 sentinel path** — manual verification that the `u64::MAX` fallback still triggers and `hmn ps` rows fall back to `nvidia-smi` cleanly. Cannot be automated in CI without hardware.

CI matrix and publish flow remain unchanged from v0.1.0 — see `reference_publish_flow.md` in this Claude Code project's memory.

## After v0.2.0 (gestures only — not commitments)

These are likely candidates for v0.3.0+ but require either a real consumer ask or hardware access before promotion:

- AMD ROCm backend (needs maintainer access to AMD hardware, or an external contributor with the hardware)
- Apple Metal backend (needs Apple Silicon machine, or external contributor)
- Peak-over-time tracking in `report` (needs a benchmark-loop consumer to ask)
- `hmn ps --rss` column (needs a consumer who wants RSS+VRAM in one shot)
- `hmn watch` (live-refresh TUI; only if `watch hmn` proves insufficient)

`#[non_exhaustive]` keeps every one of these additive — none requires a 1.0 bump.

## Decisions settled (this roadmap, 2026-05-06)

1. **0.2.0 over 0.1.x stack** — chosen as a signal of surface expansion (CLI binary + multi-adapter), not because anything breaks.
2. **`Snapshot::all()` as additive method, not a shape change** — `Snapshot::now(idx)` keeps its current shape. No breaking change.
3. **`hmn` as the binary name** — matches the project's Greek-shorthand convention (`hmn` = `hypomnesis`, mirroring `amn` for `anamnesis`).
4. **`ps` as the listing subcommand** — `docker ps` precedent. Alternatives (`gps`, `rp`, `procs`, `gpu-ps`) rejected for either acronym collision (`gps`), invented-for-this-tool opacity (`rp`), or verbosity (`gpu-ps`). `procs` retained as an acceptable fallback if `ps` proves contentious in review.
5. **`cli` feature default-off** — library users don't pull `clap`. Installation is `cargo install hypomnesis --features cli`.
6. **Compute-only `ps` is documented, not papered over** — the limitation is intrinsic to NVML/`nvidia-smi`'s data; `--help` and README state it plainly.

## One crate, one job — still

> *Tell you what's currently in this process's memory, precisely, across Windows and Linux.*

v0.2.0 stays inside that motto. It widens the matrix (more devices visible, more callers — including a CLI — can ask), but it doesn't change the job.
