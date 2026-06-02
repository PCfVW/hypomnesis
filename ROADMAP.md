# hypomnesis — Roadmap

> *External RAM and VRAM, measured. Status snapshot — updated continuously as plans change.*

Per-release detail lives in [`docs/roadmap-vX.Y.Z.md`](docs/) (indexed below).
Shipped history lives in [`CHANGELOG.md`](CHANGELOG.md).
The crate's *why* lives in [`docs/hypomnesis-brief.md`](docs/hypomnesis-brief.md).

---

## Current state

**v0.2.1** shipped 2026-05-13 — the first dogfooding-driven patch. *Sharper, not wider. Same surface — easier to test against, kinder to repeat callers.* Five waves of wear-and-tear feedback from [`hf-fetch-model 0.10.1`](https://github.com/PCfVW/hf-fetch-model)'s adoption: `test-helpers` builder, `name_or_unknown` convenience, `format_total` / `format_used` parity for `report`-feature consumers, an `HypomnesisError` `Display`-vs-structured-fields contract, and a `README.md` "Used by" + brief refresh. Detailed plan: [`docs/roadmap-v0.2.1.md`](docs/roadmap-v0.2.1.md).

---

## In flight: v0.2.2 — Windows PDH per-process backend

*Goal: make `hmn ps` report real per-process VRAM on consumer Windows / `WDDM`, where `nvidia-smi --query-compute-apps` currently returns `[N/A]` for `used_memory` and the parser silently drops every row.*

- New module wrapping the Windows **PDH (Performance Data Helper)** API to query `\GPU Process Memory(*)\Dedicated Usage` — the same `VidMm` data Task Manager surfaces.
- Wired as the primary Windows foreign-process backend; `nvidia-smi` retained as fallback for name lookup and for environments without `WDDM 2.0`+ drivers.
- Documents the [KB 4490156](https://learn.microsoft.com/en-us/troubleshoot/windows-client/performance/gpu-process-memory-counters-report-wrong-value) graphics-cache-flush drift in the new module's doc-comment — relevant to graphics-window-managed apps (Office Low Resource Mode, browser minimize/restore), **irrelevant to `CUDA` / compute workloads** that are `hmn ps`'s actual target.
- Refreshes the `README.md` `Limitations` section and `hmn --help` to reflect the closed gap.

Patch-safe — additive Cargo feature, no public-API shape change, `GpuProcessEntry::used_bytes` stays `u64`.

Motivated by the dogfooding finding on a maintainer's RTX 5060 Ti under `WDDM` driver `32.0.15.9186`: 27 GPU processes silently dropped, including the maintainer's own `ollama.exe` and `vocab_scan.exe`.

---

## Committed: v0.2.3 — macOS support (Apple Silicon + Intel Macs)

*Goal: parity with Windows / Linux for process `RSS`, device-wide GPU memory, per-process GPU memory, and the `hmn` / `hmn ps` CLI surface.*

- Merges [PR #1](https://github.com/PCfVW/hypomnesis/pull/1) (contributor: [@LittleCoinCoin](https://github.com/LittleCoinCoin)).
- Adds `metal` Cargo feature (target-gated to `cfg(target_os = "macos")`); joins the default set alongside `dxgi`.
- Adds `GpuQuerySource::Metal` variant under existing `#[non_exhaustive]` policy.
- Per-process accounting via `ledger(LEDGER_ENTRY_INFO_V2).graphics_footprint`, entry index discovered by name at runtime via `LEDGER_TEMPLATE_INFO` — no hardcoded ABI tag.
- Device-wide GPU budget via `MTLDevice.recommendedMaxWorkingSetSize` through a minimal `objc2-metal` dependency (two symbols only: `MTLCreateSystemDefaultDevice` + the property).
- Independent of v0.2.2: both releases deliver real `u64` bytes for foreign processes through different kernel-level accounting mechanisms (PDH on Windows, ledger on macOS).

Pre-merge ask of the contributor: a user-facing "cross-user PIDs need `sudo`" note in `hmn --help` Limitations (currently only in code comments and module-level `//!` docs).

---

## Speculative: v0.2.4 / v0.3.0

Items that *might* land, gated on real consumer demand:

- **Cross-platform "unmeasurable rows" diagnostic** — emit a count of detected-but-unmeasurable processes in the `hmn ps` stderr summary. Probably **never needed** once v0.2.2 + v0.2.3 ship, because both platforms then reliably deliver bytes for every readable PID. Resurfaces only if an adopter reports a process they can't see, or if very-old Windows / `WDDM 1.x` environments matter to a real user.
- **`Option<u64>` for `GpuProcessEntry::used_bytes`** — breaking change, would be v0.3.0 not patch. Deferred unless a consumer specifically asks to *list* unmeasurable processes (rather than just count them).
- **Segmented per-process VRAM API** — sibling library function `query_per_process_vram_segmented()` returning one row per `(pid, segment)` from PDH's `pid_NNNN_luid_X_phys_N` instances, plus a `hmn ps --show-segments` (or similar) CLI flag. The v0.2.2 PDH backend internally enumerates segmented data before collapsing to per-PID totals; a future patch would promote the internal helper to `pub(super)` and add a sibling dispatcher entry. Gated on either a real consumer ask or hardware exhibiting multi-segment behaviour (single-partition GPUs collapse the two paths identically, so the maintainer's `RTX 5060 Ti` can't validate the segmented path).
- **`format_summary` / `format_free_used_total`** (deferred from v0.2.1 Wave C) — promote when a second `report`-feature consumer validates the shape.
- **Long-lived `NVML` context** — performance work, deferred from v0.2.0 / v0.2.1, no benchmark-loop consumer asking yet.
- **Builders for `ProcessGpuInfo`, `Snapshot`, `GpuProcessEntry` under `test-helpers`** — add per type as downstream tests demand.

---

## Carried forward (out of scope until specifically un-gated)

| Idea | Why deferred | What would un-gate it |
|------|--------------|----------------------|
| **AMD `ROCm` backend** (`rocm_smi_lib`) | Maintainer has no AMD dGPU; shipping untested `FFI` violates project discipline | Hardware access **or** a contributor PR with maintainer hardware coverage |
| **AMD iGPU on Linux** | `NVML` doesn't see it; needs separate Linux `DRM` / `sysfs` path | Same as AMD `ROCm` |
| **Intel Arc / Intel iGPU on Linux** | No backend in the crate; same Linux-`DRM` problem | Hardware access or contributor PR |
| **Apple Metal on Intel Macs** (legacy `AMD` / Intel discrete GPUs) | v0.2.3's `ledger` mechanism likely works, but no Intel-Mac test hardware | Intel-Mac test machine or contributor PR |
| **Peak / high-water-mark tracking** | No benchmark-loop consumer asking | A real consumer request |
| **Strict-accounting `D3DKMTQueryStatistics` Windows backend** | "Reserved for system use. Do not use." per Microsoft docs; undocumented kernel-thunk surface | A real consumer who reports KB 4490156 drift biting their specific workload |
| **`hmn watch` (TUI live-refresh)** | Shell loop `watch -n 1 hmn` is the Unix answer; `nvtop`-style work belongs elsewhere | A consumer who's tried `watch` and explained why it's insufficient |
| **`hmn` reading from another machine over SSH / RPC** | Out of scope; users run `ssh host hmn` | Not planned |
| **TUI / live mode (`hmn top`)** | That's `nvtop`'s job | Not planned |

`#[non_exhaustive]` keeps every one of these additive — none requires a 1.0 bump.

---

## Per-release detail (index)

- [`docs/roadmap-v0.2.0.md`](docs/roadmap-v0.2.0.md) — shipped 2026-05-06. *Wider, not taller.* `Snapshot::all`, `gpu_processes`, `hmn` CLI, `report`-feature `format_free` / `print_free`.
- [`docs/roadmap-v0.2.1.md`](docs/roadmap-v0.2.1.md) — shipped 2026-05-13. *Sharper, not wider.* `test-helpers` builder, `name_or_unknown`, `format_total` / `format_used`, `HypomnesisError` `Display` contract, README "Used by" + brief refresh.
- *v0.2.2 detailed roadmap will appear here once drafted (in flight).*
- *v0.2.3 detailed roadmap will appear here once PR #1 enters merge-prep (committed).*

Foundational documents (not per-release):

- [`docs/hypomnesis-brief.md`](docs/hypomnesis-brief.md) — *why this crate exists*: Plato + the v0.1.x VRAM saga + the extraction rationale from `candle-mi`.
- [`docs/hypomnesis-adoption.md`](docs/hypomnesis-adoption.md) — `hf-fetch-model 0.10.1` dogfooding report (the basis of v0.2.1's wave list).

---

## Principles

1. **Every patch is informed by at least one real consumer's adoption experience.** Codified in v0.2.1's CHANGELOG intro; v0.2.2 follows it (driven by the `WDDM` `[N/A]` finding on a maintainer's RTX 5060 Ti); v0.2.3 follows it (driven by the contributor's actual macOS adoption).
2. **Additive-by-default under `#[non_exhaustive]`.** New variants and fields land in patch releases. Type-shape changes (`u64 → Option<u64>`, etc.) are minor bumps, never patches.
3. **No new hardware backends without maintainer-accessible hardware or a contributor PR.** AMD `ROCm` and Apple Metal sat behind this gate until v0.2.3 (Apple Silicon via PR #1) un-gated half of it.
4. **Documented limitations beat papered-over half-fixes.** R570 `u64::MAX` sentinel, `WDDM` `NVML_VALUE_NOT_AVAILABLE`, KB 4490156 PDH drift, macOS cross-user `EPERM` — each is named in the source and README rather than hidden.
5. **`Display` is the default English one-liner; structured fields are canonical.** v0.2.1 Wave D's `HypomnesisError` contract — applies to every future error / measurement variant.
6. **One crate, one job.** *Tell you what's currently in this process's memory, precisely, across Windows and Linux* (extending to macOS in v0.2.3 via [PR #1](https://github.com/PCfVW/hypomnesis/pull/1)). Anything that widens the job (system-wide free RAM, GPU temperature trends, live TUI) belongs in a different crate.

---

*Living document — update as plans evolve. Last revised at the v0.2.1 → v0.2.2 transition (2026-05-13). Reviewer hint: for **shipped** details, the per-release roadmap is the authoritative source; for **forthcoming** plans, this document is the source until a per-release roadmap is drafted.*
