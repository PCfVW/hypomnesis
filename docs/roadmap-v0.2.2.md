# `hypomnesis` v0.2.2 — roadmap

> *Truer, not wider. Same `hmn ps` — fewer silent omissions on Windows.*

---

## Why v0.2.2 (and not v0.3.0)

Every item in this roadmap is **additive and patch-safe** under the `#[non_exhaustive]` policy already in place. No public type changes shape, no existing method changes signature, no default feature flips that downstream consumers would notice at the `Cargo.toml` line. The release exists to close a *measurement honesty* gap — `hmn ps` on consumer Windows / `WDDM` currently reports `0 compute processes found` while 27 processes are using the GPU, because `nvidia-smi --query-compute-apps` returns `[N/A]` for `used_memory` and the parser silently drops every row.

**v0.2.2 is the right vehicle, not v0.3.0.** A new backend behind a new Cargo feature, a dispatcher tweak, and a docs refresh do not warrant a minor bump:

1. No new public type — `GpuProcessEntry::used_bytes` stays `u64`. No `Option<u64>` reshape (that would be v0.3.0).
2. No structural API shape change — the new code is a fourth Windows backend slotted into the existing dispatcher chain alongside `NVML` / `DXGI` / `nvidia-smi`.
3. New default-on Cargo feature (`pdh`) follows the same shape as the existing `dxgi` feature: target-conditional dependency, no-op on non-Windows, included by default so the common case is zero-config.

The fix exists at the kernel-data-source layer, not at the API surface. That is exactly what a patch release is for.

---

## Scope

Three waves, ordered code-first then docs. The maintainer's existing hardware (Windows + Ryzen 9 5950X + RTX 5060 Ti, `WDDM` driver `32.0.15.9186`; Ubuntu WSL2 + RTX 5060 Ti) covers verification on the affected platform. Non-Windows targets remain unchanged.

### Wave A — `src/gpu/pdh.rs`: PDH FFI bindings and counter enumeration

**Motivation.** Consumer Windows / `WDDM 2.0`+ moved per-process GPU memory accounting authority from the GPU vendor's driver to the Windows kernel's `VidMm`. `NVML` returns `NVML_VALUE_NOT_AVAILABLE`. `IDXGIAdapter3::QueryVideoMemoryInfo` answers only for the calling process. `nvidia-smi --query-compute-apps` writes `[N/A]`. The data exists — Task Manager's "Dedicated GPU memory" column proves it — but every backend hypomnesis currently queries is blind.

Microsoft exposes the same `VidMm` data through the **Performance Data Helper (PDH)** API. The relevant counter set is `\GPU Process Memory(<instance>)\Dedicated Usage`, with instances of the form `pid_NNNN_luid_0xHHHHHHHH_0xHHHHHHHH_phys_N`. PDH is documented, has a stable C ABI, and is callable from any program — no admin elevation needed for read access.

**Module structure** (`src/gpu/pdh.rs`, new file):

```rust
//! Windows PDH (Performance Data Helper) per-process VRAM backend.
//!
//! Closes the per-process-memory gap on consumer Windows / `WDDM` by
//! reading `\GPU Process Memory(*)\Dedicated Usage` through `pdh.dll`.
//! Same `VidMm` data Task Manager surfaces, accessed through a stable
//! and documented C API.
//!
//! ## Known accounting drift (KB 4490156)
//!
//! Windows' `GPU Process Memory` counters can over-report per-process
//! `VRAM` by ~100 MiB per cycle for applications that go through
//! repeated discard-and-restore of GPU caches (the documented example is
//! Office in Low Resource Mode). This is a `WDDM 2.0`+ architectural
//! artifact: `VidMm` asynchronously defers allocation destruction, and
//! the user-mode driver's discards may not yet be reflected at sample
//! time. Microsoft considers this a known issue with no fix as of 2026.
//!
//! Compute workloads (CUDA, PyTorch, llama.cpp, ollama, candle) do not
//! exhibit the trigger pattern — they allocate coarse-grained buffers
//! and hold them for the duration of a run. For the `hmn ps` target use
//! case the drift is in practice zero. Tools needing byte-exact
//! accounting for graphics-cache-flush workloads should prefer Task
//! Manager's Performance pane or `WPR` / `WPA` (ETW-based), both of
//! which use independent paths through `VidMm` and are unaffected.

pub(super) fn query_per_process_vram(
    device_index: u32,
) -> Result<Vec<(u32, u64)>, HypomnesisError> { ... }
```

**Implementation outline** (one PDH session per call; cheap because no rate counters are involved):

1. `PdhOpenQueryW(NULL, 0, &mut hQuery)` — open a query.
2. `PdhEnumObjectItemsW(NULL, NULL, L"GPU Process Memory", ...)` — enumerate live counter instances. Cannot construct instance names from `pid` alone; have to enumerate. One extra call per `gpu_processes()` invocation, negligible cost.
3. For each instance matching `pid_NNNN_...` and (optional v0.2.2-or-later) matching the target adapter `LUID`: `PdhAddCounterW(hQuery, path, 0, &mut hCounter)`.
4. `PdhCollectQueryData(hQuery)` — single sample (the `Dedicated Usage` counter is an instantaneous gauge, not a rate; one collection suffices).
5. `PdhGetFormattedCounterValue(hCounter, PDH_FMT_LARGE, NULL, &mut value)` — read each counter as a 64-bit integer.
6. `PdhCloseQuery(hQuery)` — release.

**Feature gating.** Add a `pdh` Cargo feature, default-on, mirroring `dxgi`:

```toml
[features]
default = ["nvml", "nvidia-smi-fallback", "dxgi", "pdh"]
pdh = ["dep:windows"]   # shares the `windows` dep with `dxgi`

[target.'cfg(windows)'.dependencies]
windows = { version = "0.62", optional = true, features = [
    "Win32_Graphics_Dxgi",
    "Win32_Graphics_Dxgi_Common",
    "Win32_System_Performance",    # ← new for PDH counters
    "Win32_System_Threading",      # ← new for OpenProcess + QueryFullProcessImageNameW (name lookup)
] }
```

Non-Windows users pay nothing — the entire module is gated on `cfg(all(target_os = "windows", feature = "pdh"))`.

**Device-index attribution.** PDH instance names include the adapter `LUID`. The existing `device_count()` / `device_info()` path enumerates NVIDIA devices via `NVML`. To attribute a PDH row to a hypomnesis `device_index`, we need to correlate `NVML` device → adapter `LUID`. `NVML` exposes `nvmlDeviceGetPciInfo` (returns `bus_id`, not `LUID`) and on Windows `nvmlDeviceGetUUID`. Direct `LUID` mapping requires `DXGI`'s `IDXGIAdapter::GetDesc` (returns `LUID`). The implementation will resolve `device_index → LUID` once at first call via a DXGI walk, cache it, and filter PDH instances by that `LUID`. The `DXGI` path is already in the crate (the calling-process fast path); we reuse the walk.

Single-`GPU` systems — the common case, including the maintainer's machine — bypass this entirely: a single device means the `LUID` filter accepts everything. The multi-`GPU` branch is best-effort and length-tolerant in the smoke test (a wrong `LUID` filter produces an empty result, not a wrong result).

**Errors.** Add `HypomnesisError::Pdh(String)`, following the existing one-variant-per-backend pattern (`Ram`, `Nvml`, `Dxgi`, `NvidiaSmi`). Patch-safe under `#[non_exhaustive]`. Renders as `"PDH error: ..."` via a `#[error("PDH error: {0}")]` derive — matching the wording style of the sibling variants rather than producing awkward `"DXGI error: pdh: ..."` nesting.

**Effort.** Roughly a day. PDH FFI is documented, the C signatures are stable, and the `windows` crate already provides safe wrappers for the relevant entry points under `Win32::System::Performance`. 6–10 inline unit tests covering: instance-name parsing (`pid_24168_luid_..._phys_0` → `(24168, luid)`), the `LUID`-correlation cache, and an `EnumObjectItems` empty-buffer path. Live-hardware verification happens in Wave B.

### Wave B — `src/gpu/mod.rs`: wire PDH into `gpu_processes()`

**Motivation.** Once `pdh::query_per_process_vram(device_index)` works, `gpu_processes()` needs to consume it. The current dispatcher tries `NVML` (Linux primary), then `nvidia-smi` (Windows primary today). PDH slots in as the Windows primary, with **two** distinct paths:

1. **Name lookup via Win32 `QueryFullProcessImageNameW`** — PDH instance names give us `(pid, luid)` but not a process name. The cross-platform precedent is to use the platform's native process-name API: Linux uses `/proc/<pid>/comm` (the existing `read_proc_comm` helper in `src/gpu/mod.rs`), macOS uses `proc_pidpath` (per PR #1). The Windows analog is `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pid)` + `QueryFullProcessImageNameW`, then take the path's basename. No subprocess, no NVIDIA-specific dependency, vendor-neutral. Access-denied for cross-user / protected processes yields `name: None`, mirroring how `read_proc_comm` handles unreadable `/proc` entries on Linux.
2. **Fallback to pure-`nvidia-smi`** — environments without `WDDM 2.0`+ drivers (rare in 2026 but not zero) fall back to the existing `nvidia-smi`-only path. Detection: PDH `PdhEnumObjectItemsW` returns `PDH_CSTATUS_NO_OBJECT` when the `GPU Process Memory` counter set isn't registered. In this fallback the name comes from `nvidia-smi`'s output as today.

**Dispatcher order** (Windows-only branch in `gpu_processes`):

```text
1. PDH per-process VRAM      → Vec<(pid, luid, used_bytes)>
2. Win32 process-name lookup → name per PID (None on access-denied)
3. join + filter by device   → Vec<GpuProcessEntry>
4. on PDH unavailable: pure nvidia-smi (current path, names included)
```

Sanity checks remain: the R570 `u64::MAX` sentinel and the `used > device_total` check apply per-row, regardless of source. KB 4490156 drift is **not** sanity-checked — by the time a counter has drifted ~100 MiB it's still within plausible bounds, and the trigger pattern is irrelevant for compute workloads anyway.

**`GpuQuerySource` variant.** Add `GpuQuerySource::Pdh`. The enum is `#[non_exhaustive]`, so this is patch-safe. Rows produced via the PDH path (memory from PDH, name from Win32) get `source: GpuQuerySource::Pdh`; rows from the pure-`nvidia-smi` fallback keep `source: GpuQuerySource::NvidiaSmi`. Downstream consumers can distinguish "data from `VidMm` directly via PDH" from "data inferred via subprocess parsing."

**Effort.** Half a day. Dispatcher wiring + the `Pdh` enum variant + a small `name_from_pid_windows` helper (`OpenProcess` + `QueryFullProcessImageNameW` + basename extraction) + 4–5 inline tests covering: PDH-success-then-name-merge, PDH-success-name-access-denied-yields-`None`, PDH-empty-then-fallback, KB-4490156-sentinel-doesn't-trigger, basename-extraction-from-full-path.

### Wave C — Documentation refresh

**Motivation.** Two user-facing surfaces still describe the broken state:

- **`README.md` `Limitations` for the `hmn` binary** (the bulleted list under the `## Binary (`hmn`)` section) — limitation #4 currently reads "Windows compute-process attribution is `nvidia-smi`-backed... `NVML`'s per-process query returns `NVML_VALUE_NOT_AVAILABLE` under `WDDM`. So `hmn ps` on Windows is honest-but-second-class compared to Linux's clean `NVML` enumeration." After v0.2.2 this becomes "PDH-backed on Windows; falls back to `nvidia-smi` on pre-`WDDM 2.0` systems. Honesty parity with Linux `NVML`." Limitation #1 (compute-only) also needs softening on Windows: PDH sees all GPU users, not just CUDA processes — so the `hmn ps` table will now show graphics apps too, with real memory numbers.
- **`hmn --help` `Limitations`** — currently lumps Windows and Linux together under "only processes with an active CUDA context appear." After v0.2.2 the Windows bullet splits: Linux/NVML stays compute-only; Windows/PDH lists **all** GPU users (compositor, browsers, games, compute). This is more truthful and matches Task Manager's behavior.

**Module-level doc-comment** (`src/gpu/pdh.rs`) — see Wave A's outline; the KB 4490156 caveat is mandatory.

**`docs/hypomnesis-brief.md`** — the brief's [Capabilities table](hypomnesis-brief.md#why-this-crate) gains a row: "Per-process VRAM on Windows via `PDH` performance counters (consumer `WDDM`) | **hypomnesis (this release)**." Sibling to the existing DXGI calling-process row.

**`CHANGELOG.md`** — `[Unreleased]` → `[0.2.2]` at release time, motto blockquote, intro paragraph, three Added/Changed entries (one per wave).

**Effort.** Two hours. Pure text edits, no code review beyond the prose.

---

## Out of scope for v0.2.2 (carrying the discipline forward)

| Idea | Why deferred |
|------|--------------|
| **Cross-platform "unmeasurable rows" diagnostic** — emit a count of detected-but-unmeasurable processes in the `hmn ps` stderr summary | Probably never needed once v0.2.2 + v0.2.3 ship — both platforms then reliably deliver bytes for every readable PID. Resurfaces only if an adopter reports a process they can't see, or if very-old Windows / `WDDM 1.x` matters to a real user. |
| **`Option<u64>` for `GpuProcessEntry::used_bytes`** — list (not just count) unmeasurable processes with `VRAM: N/A` | Breaking change → v0.3.0, not patch. Deferred unless a consumer specifically asks to *list* unmeasurable processes. The PDH backend's existence makes this much less likely. |
| **Segmented per-process VRAM API** — sibling `query_per_process_vram_segmented()` library function + `hmn ps --show-segments` CLI flag | PDH internally exposes per-(`pid`, segment) data via instances like `pid_X_luid_Y_phys_N`; Wave A aggregates by-PID before returning. The internal helper is deliberately structured to be promotable later (promote to `pub(super)`, add a sibling dispatcher entry, no refactor). Gated on a real consumer ask or hardware exhibiting multi-segment behaviour — the maintainer's `RTX 5060 Ti` is single-segment, so the segmented path is not currently exercised against real multi-segment hardware. |
| **Strict-accounting `D3DKMTQueryStatistics` Windows backend** | "Reserved for system use. Do not use." per Microsoft docs. KB 4490156 drift doesn't bite the compute workloads `hmn ps` actually targets — see module doc-comment in Wave A. Revisit only if a consumer reports drift biting their specific workload. |
| **ETW-based per-process counters** (`WPR` / `WPA` path) | Heavy session setup, typically admin-required, accurate but overkill for `hmn ps`. PDH is the right ergonomics tier. |
| **Per-GPU-engine breakdown** (`\GPU Engine(*)\Utilization Percentage`) | A different counter set than memory. `hmn ps` is about *memory*; engine utilization is a separate, future feature. |
| **Adapter-LUID-correlation cache across calls** | First-call cost is a single `DXGI` walk (~1 ms on the maintainer's machine). Cross-call caching would complicate the no-shared-state model and isn't motivated by a real perf consumer ask. |
| **`hmn ps` showing PDH-source rows for adapters not in `device_count()`** (e.g., Intel `iGPU`) | The PDH counter is GPU-vendor-neutral, but hypomnesis's device enumeration is NVIDIA-only via `NVML`. Surfacing Intel `iGPU` rows would require expanding `device_count()` to enumerate all `WDDM` adapters — proper minor-version work. |
| **AMD `ROCm` backend, Apple Metal on Intel Macs, Linux AMD `iGPU`** | Carried over from v0.2.0 / v0.2.1 — still no maintainer hardware access. |

`#[non_exhaustive]` keeps every one of these additive — none requires a 1.0 bump.

---

## Verification plan

All three waves touch a real Windows backend, so the verification surface includes hardware-attached live tests:

- **Wave A** — Unit tests for instance-name parsing, `LUID`-correlation cache, and `EnumObjectItems`-empty handling run on every platform under `cargo test --features pdh` (the FFI stubs are no-op outside `cfg(windows)`). On the maintainer's Windows host, a `#[ignore]`-gated live test reads `\GPU Process Memory(*)\Dedicated Usage` and asserts ≥ 1 instance is returned.
- **Wave B** — On the maintainer's Windows host, running `hmn ps` should produce non-empty output where v0.2.1 returned `0 compute processes found`. Specifically: with `ollama serve` loaded, `hmn ps` should show `ollama.exe` with a `VRAM` value within ~50 MiB of `nvidia-smi --query-gpu=memory.used,memory.free --format=csv,noheader,nounits`. With `cargo run --example vocab_scan` running, that PID should appear too.
- **Wave B regression** — `hmn ps` on Ubuntu WSL2 must produce identical output to v0.2.1 (PDH branch is unreachable; `NVML` path unchanged).
- **Wave C** — `cargo doc --all-features --no-deps` renders the new module doc-comment cleanly; `markdown` lint on README + brief stays clean; `hmn --help` text update visually reviewed.

**Live-hardware test placement.** A new `tests/live_pdh.rs` mirrors `tests/live_gpu.rs`: `#[ignore]`-gated, runs under `cargo test --features pdh -- --ignored` on Windows. Three test functions covering: PDH instance enumeration non-empty, per-process value plausibility against a known consumer (start `notepad.exe` → expect 0 or a small value; doesn't appear → also acceptable on idle systems), and the calling-process matches DXGI's value within ~1 MiB.

**Publish flow** unchanged from v0.2.0 / v0.2.1 — commit → dry-run CI → push → watch → `cargo publish --dry-run` → tag → watch → verify, per `reference_publish_flow.md` in the Claude Code project memory.

---

## After v0.2.2 (gestures only — not commitments)

These are likely candidates for v0.2.3 or later releases:

- **v0.2.3 — macOS support** ([PR #1](https://github.com/PCfVW/hypomnesis/pull/1), already committed). Apple Silicon `ledger`-based per-process accounting + `MTLDevice.recommendedMaxWorkingSetSize` for the device-wide budget. Independent of v0.2.2's PDH work; both deliver real `u64` bytes through different kernel-level accounting paths.
- **Brief & motto refresh in v0.2.3** — update `docs/hypomnesis-brief.md` byline and closing motto from "Windows and Linux" to "Windows, Linux, and macOS." Mirrors v0.2.1 Wave E's brief correction. ROADMAP.md's Principle #6 parenthetical can then drop.
- **Cross-platform "unmeasurable rows" diagnostic** — only if a real consumer reports a hidden process. Vanishingly unlikely after v0.2.2 + v0.2.3.
- **AMD `ROCm` / Apple Metal on Intel Macs** — still gated on hardware access.

`#[non_exhaustive]` keeps every one of these additive — none requires a 1.0 bump.

---

## Decisions settled (this roadmap, 2026-06-01)

1. **v0.2.2 patch, not v0.3.0** — every item is additive; no public-API shape change.
2. **PDH over `D3DKMTQueryStatistics`** — PDH is documented and public; `D3DKMTQueryStatistics` is officially "Reserved for system use. Do not use." The KB 4490156 drift doesn't bite compute workloads, which is what `hmn ps` actually targets.
3. **`pdh` Cargo feature, default-on, mirrors `dxgi`** — target-conditional, no-op on non-Windows, shares the `windows` crate dependency, included by default so the common case is zero-config.
4. **PDH (memory) + Win32 `QueryFullProcessImageNameW` (names) on Windows** — PDH instance names give `(pid, luid)`, not process names. Win32 is the cross-platform-consistent choice: Linux uses `/proc/<pid>/comm` (`read_proc_comm`), macOS uses `proc_pidpath` (per PR #1), Windows uses `OpenProcess` + `QueryFullProcessImageNameW`. No subprocess, no `nvidia-smi` coupling for the PDH path. The pure-`nvidia-smi` fallback retains today's subprocess-name lookup for pre-`WDDM 2.0` environments only.
5. **`GpuQuerySource::Pdh` variant added** — patch-safe under `#[non_exhaustive]`. Lets downstream consumers tell "from VidMm directly" apart from "inferred via subprocess parsing."
6. **Add `HypomnesisError::Pdh(String)` variant** — patch-safe under `#[non_exhaustive]`. Follows the existing one-variant-per-backend pattern (`Ram`, `Nvml`, `Dxgi`, `NvidiaSmi`); renders cleanly as `"PDH error: ..."` rather than the awkward `"DXGI error: pdh: ..."` nesting that string-prefix reuse would produce.
7. **Document KB 4490156 in the module doc-comment** — name the KB explicitly, explain the trigger pattern (graphics-cache-flush), note CUDA / compute workloads are unaffected. Same "documented limitations beat papered-over half-fixes" principle that named the R570 sentinel and the `NVML_VALUE_NOT_AVAILABLE` situation.
8. **Multi-GPU `LUID` correlation via cached DXGI walk** — first-call cost is one DXGI adapter walk (~1 ms), result cached for the process lifetime. The common single-GPU case bypasses the filter entirely.
9. **No `hmn ps` UX change beyond what naturally falls out** — the existing table layout absorbs the new rows. Same per-platform `--help` Limitations section, refreshed wording. Compute-only labeling stays for Linux; Windows now lists all GPU users honestly.

---

## One crate, one job — still

> *Tell you what's currently in this process's memory, precisely, across Windows and Linux.*

v0.2.2 stays inside that motto. It does not widen the matrix, add a backend, or change the answer to the central question — it closes a measurement-honesty gap on Windows that was forcing `hmn ps` to lie about what it could see. The shipped result: the same `hmn ps` command, on the same hardware, now telling the truth.
