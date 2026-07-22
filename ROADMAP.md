# hypomnesis — Roadmap

> *External RAM and VRAM, measured. Status snapshot — updated continuously as plans change.*

Per-release detail lives in [`docs/roadmap-vX.Y.Z.md`](docs/) (indexed below).
Shipped history lives in [`CHANGELOG.md`](CHANGELOG.md).
The crate's *why* lives in [`docs/hypomnesis-brief.md`](docs/hypomnesis-brief.md).

---

## Current state

**v0.2.5** shipped 2026-07-22 — `WDDM` spill detection. *Resident, not committed. Episodes, not a boolean.* A `SpillTracker` that compiles on every platform (honest `is_spill_measurable()` returns `false` off-Windows) reads the `PDH` `\GPU Adapter Memory(*)` residency gauges and flags spill only when dedicated-resident saturates **and** shared-resident grows past its benign first-observation baseline — never from the `committed − dedicated` gap, per the rhyme-mdlm dogfooding report's live false-positive ([2026-07-19](docs/dogfooding-feedbacks/dogfooding-wddm-spill-detection.md)). Transient spills are first-class: an instantaneous `is_spilling()` / latched `has_spilled()` split plus an episode-based `SpillReport`. Per-process attribution rides along as additive `GpuProcessEntry::shared_used_bytes` (+ `hmn ps` SHARED column), and `hmn spill -- <command>` wraps any run `time(1)`-style (`--interval` default 100 ms, `--json`, exit-code pass-through). Two live-measured corrections to the design sketch: no `Dedicated Limit` counter exists in `PDH` (capacity comes from `DXGI` `DedicatedVideoMemory`), and the dedicated-saturation default is **85%**, not ~95% — a forced-spill fixture measured `VidMm`'s dedicated-resident ceiling at ≈ 88.6–91.3% of `DXGI` capacity, making 95% unreachable. Release-validated with a real 13.1 s spill episode (3.1 GiB peak shared) on the reference `RTX 5060 Ti`, produced by a forced-spill fixture preserved at [`tools/spillforge`](tools/spillforge/) (repo-only, `publish = false`) for future re-validation on new drivers or contributor hardware. Detailed plan: [`docs/roadmap-v0.2.5.md`](docs/roadmap-v0.2.5.md).

The preceding **v0.2.4** shipped 2026-06-29 — surfaces NVIDIA's driver/firmware **reserved** memory carve-out. *The same total. Now with the carve-out shown.* A new additive `GpuDeviceInfo::reserved_bytes: Option<u64>` exposes the carve-out NVML holds *within* its reported `total` (`total = reserved + free + used`) — **live-measured at 259 MiB** on the reference `RTX 5060 Ti`, byte-identical to `nvidia-smi -q -d MEMORY`'s `Reserved` line beside `Total: 16311 MiB`. It is a subset of `total_bytes`, so allocation headroom is `total_bytes − reserved_bytes` (which `free_bytes` already reflects). Sourced from NVML's v2 memory query (`nvmlDeviceGetMemoryInfo_v2`, R510+) with a graceful pre-R510 fallback to `None`; `total_bytes` is unchanged (the v1 figure = `nvidia-smi` `Total`). Driven by a [`candle-mi`](https://github.com/PCfVW/candle-mi) v0.1.16 dogfooding report — whose *inferred* 73 MiB carve-out (`DXGI nominal − NVML total`) the live v2 query revealed to be a *different* quantity (board/ECC overhead below NVML's `total`) from the true 259 MiB driver reservation. Detailed plan: [`docs/roadmap-v0.2.4.md`](docs/roadmap-v0.2.4.md).

The preceding **v0.2.3** shipped 2026-06-10 — first-class macOS support on Apple Silicon. *Three platforms. Same contract. Resident-bytes everywhere.* Contributor [@LittleCoinCoin](https://github.com/LittleCoinCoin)'s [PR #1](https://github.com/PCfVW/hypomnesis/pull/1) lands the macOS path: libSystem-only RAM + per-process GPU + compute-process listing (`task_info`, `ledger`, `sysctl`, `proc_listpids`, `proc_pidpath`), with `MTLDevice.recommendedMaxWorkingSetSize` via a minimal `objc2-metal` binding for the device-wide GPU budget. Two dogfooding-driven UX additions rode along: `hmn ps` stderr summary gains a "committed total" figure (signalling the `WDDM` commit-vs-resident distinction without naming it), and a "Composable workflows" `README.md` subsection documents `hmn ps --json` with two `jq` recipes — including a *"Why no `hmn kill`?"* scope-discipline note declining a hmn-side kill subcommand to preserve hypomnesis's measurement-not-control boundary. Field-validated post-release on H100 / GB200 (Linux) and a 48 GiB MacBook Pro alongside the contributor's M3 Pro daily-driver. The [PR #1 body](https://github.com/PCfVW/hypomnesis/pull/1) served as the per-release roadmap.

The preceding **v0.2.2** (2026-06-02) shipped the Windows `PDH` per-process backend — first Rust crate (to the maintainer's knowledge) to expose per-process `VRAM` for foreign processes on consumer Windows / `WDDM`, closing the dogfooding gap where `hmn ps` had silently dropped 27 processes including the maintainer's own `ollama.exe`. Detailed plan: [`docs/roadmap-v0.2.2.md`](docs/roadmap-v0.2.2.md).

---

## Speculative: v0.3.0

Items that *might* land, gated on real consumer demand:

- **Cross-platform "unmeasurable rows" diagnostic** — emit a count of detected-but-unmeasurable processes in the `hmn ps` stderr summary. Probably **never needed** now that v0.2.2 + v0.2.3 have shipped, because all three platforms reliably deliver bytes for every readable PID. Resurfaces only if an adopter reports a process they can't see, or if very-old Windows / `WDDM 1.x` environments matter to a real user.
- **`Option<u64>` for `GpuProcessEntry::used_bytes`** — breaking change, would be v0.3.0 not patch. Deferred unless a consumer specifically asks to *list* unmeasurable processes (rather than just count them).
- **Segmented per-process VRAM API** — sibling library function `query_per_process_vram_segmented()` returning one row per `(pid, segment)` from PDH's `pid_NNNN_luid_X_phys_N` instances, plus a `hmn ps --show-segments` (or similar) CLI flag. The v0.2.2 PDH backend internally enumerates segmented data before collapsing to per-PID totals; a future patch would promote the internal helper to `pub(super)` and add a sibling dispatcher entry. Gated on either a real consumer ask or hardware exhibiting multi-segment behaviour (single-partition GPUs collapse the two paths identically, so the maintainer's `RTX 5060 Ti` can't validate the segmented path).
- **`format_summary` / `format_free_used_total`** (deferred from v0.2.1 Wave C) — promote when a second `report`-feature consumer validates the shape.
- **Long-lived `NVML` context** — performance work, deferred from v0.2.0 / v0.2.1, no benchmark-loop consumer asking yet.
- **Builders for `ProcessGpuInfo`, `Snapshot`, `GpuProcessEntry` under `test-helpers`** — add per type as downstream tests demand. (Clause first exercised in v0.2.5: `SpillReportBuilder`, demanded by the `hmn` binary's own formatter tests.)
- **`hmn spill` partial report on Ctrl+C** (deferred from v0.2.5) — a `ctrlc` handler so an interrupted run still prints what was observed. Deferred to keep v0.2.5 dependency-free; documented in the `--help` text. Un-gated by a consumer who actually loses a report they needed.
- **`SpillTracker` auto-reopen after driver reset / `TDR`** (deferred from v0.2.5) — today a reset invalidates the long-lived `PDH` query and every later `observe()` is a skipped observation (documented). Un-gated by a real consumer whose runs survive TDRs.
- **Per-process attribution inside `SpillTracker` / adapter `Total Committed` exposure** (deferred from v0.2.5) — the tracker's condition is deliberately adapter-scoped; per-PID shared attribution lives in `gpu_processes()`. Fold attribution into the report only if a consumer shows the two-step flow (`hmn spill` then `hmn ps`) losing the culprit in practice.

---

## Carried forward (out of scope until specifically un-gated)

| Idea | Why deferred | What would un-gate it |
|------|--------------|----------------------|
| **AMD `ROCm` backend** (`rocm_smi_lib`) | Maintainer has no AMD dGPU; shipping untested `FFI` violates project discipline | Hardware access **or** a contributor PR with maintainer hardware coverage |
| **AMD iGPU on Linux** | `NVML` doesn't see it; needs separate Linux `DRM` / `sysfs` path | Same as AMD `ROCm` |
| **Intel Arc / Intel iGPU on Linux** | No backend in the crate; same Linux-`DRM` problem | Hardware access or contributor PR |
| **Apple Metal on Intel Macs** (legacy `AMD` / Intel discrete GPUs) | v0.2.3's `ledger` mechanism likely works, but no Intel-Mac test hardware | Intel-Mac test machine or contributor PR |
| **Strict-accounting `D3DKMTQueryStatistics` Windows backend** | "Reserved for system use. Do not use." per Microsoft docs; undocumented kernel-thunk surface | A real consumer who reports KB 4490156 drift biting their specific workload |
| **`hmn watch` (TUI live-refresh)** | Shell loop `watch -n 1 hmn` is the Unix answer; `nvtop`-style work belongs elsewhere | A consumer who's tried `watch` and explained why it's insufficient |
| **`hmn` reading from another machine over SSH / RPC** | Out of scope; users run `ssh host hmn` | Not planned |
| **TUI / live mode (`hmn top`)** | That's `nvtop`'s job | Not planned |

`#[non_exhaustive]` keeps every one of these additive — none requires a 1.0 bump.

---

## Per-release detail (index)

- [`docs/roadmap-v0.2.0.md`](docs/roadmap-v0.2.0.md) — shipped 2026-05-06. *Wider, not taller.* `Snapshot::all`, `gpu_processes`, `hmn` CLI, `report`-feature `format_free` / `print_free`.
- [`docs/roadmap-v0.2.1.md`](docs/roadmap-v0.2.1.md) — shipped 2026-05-13. *Sharper, not wider.* `test-helpers` builder, `name_or_unknown`, `format_total` / `format_used`, `HypomnesisError` `Display` contract, README "Used by" + brief refresh.
- [`docs/roadmap-v0.2.2.md`](docs/roadmap-v0.2.2.md) — shipped 2026-06-02. *Truer, not wider.* Windows `PDH` per-process backend, `?`-row security-relevant hint, PID 4 rendered as `[kernel]`.
- *v0.2.3 — no separate per-release document; the [PR #1](https://github.com/PCfVW/hypomnesis/pull/1) body served as the per-release roadmap.*
- [`docs/roadmap-v0.2.4.md`](docs/roadmap-v0.2.4.md) — shipped 2026-06-29. *The same total. Now with the carve-out shown.* NVML v2 `reserved` carve-out surfaced as additive `GpuDeviceInfo::reserved_bytes`, `hmn` summary parenthetical, pre-R510 graceful fallback.
- [`docs/roadmap-v0.2.5.md`](docs/roadmap-v0.2.5.md) — shipped 2026-07-22. *Resident, not committed. Episodes, not a boolean.* `WDDM` spill detection: `PDH` `Shared Usage` residency gauges, `SpillTracker` with `is_spilling()` / `has_spilled()` split + episode-based `SpillReport`, `GpuProcessEntry::shared_used_bytes` + `hmn ps` SHARED column, `hmn spill -- <command>` wrapper with exit-code pass-through. Threshold default live-tuned to 85% via a forced-spill fixture.

Foundational documents (not per-release):

- [`docs/hypomnesis-brief.md`](docs/hypomnesis-brief.md) — *why this crate exists*: Plato + the v0.1.x VRAM saga + the extraction rationale from `candle-mi`.
- [`docs/hypomnesis-adoption.md`](docs/hypomnesis-adoption.md) — `hf-fetch-model 0.10.1` dogfooding report (the basis of v0.2.1's wave list).

---

## Principles

1. **Every patch is informed by at least one real consumer's adoption experience.** Codified in v0.2.1's CHANGELOG intro; v0.2.2 follows it (driven by the `WDDM` `[N/A]` finding on a maintainer's RTX 5060 Ti); v0.2.3 follows it (driven by the contributor's actual macOS adoption); v0.2.4 follows it (driven by a `candle-mi` v0.1.16 dogfooding report asking for the NVML driver-reserved carve-out on the same RTX 5060 Ti); v0.2.5 follows it (driven by the maintainer's own `WDDM` spill scenario on the same RTX 5060 Ti / 16 GiB host, then course-corrected by a rhyme-mdlm dogfooding report's live commit-vs-residency false-positive before a line of code was written).
2. **Additive-by-default under `#[non_exhaustive]`.** New variants and fields land in patch releases. Type-shape changes (`u64 → Option<u64>`, etc.) are minor bumps, never patches.
3. **No new hardware backends without maintainer-accessible hardware or a contributor PR.** AMD `ROCm` and Apple Metal sat behind this gate until v0.2.3 (Apple Silicon via PR #1) un-gated half of it.
4. **Documented limitations beat papered-over half-fixes.** R570 `u64::MAX` sentinel, `WDDM` `NVML_VALUE_NOT_AVAILABLE`, KB 4490156 PDH drift, macOS cross-user `EPERM` — each is named in the source and README rather than hidden.
5. **`Display` is the default English one-liner; structured fields are canonical.** v0.2.1 Wave D's `HypomnesisError` contract — applies to every future error / measurement variant.
6. **One crate, one job.** *Tell you what's currently in this process's memory, precisely, across Windows, Linux, and macOS.* (macOS support shipped in v0.2.3 via [PR #1](https://github.com/PCfVW/hypomnesis/pull/1).) Anything that widens the job (system-wide free RAM, GPU temperature trends, live TUI, process termination) belongs in a different crate — see the *"Why no `hmn kill`?"* note in the README for the canonical example of scope discipline in action.

---

*Living document — update as plans evolve. Last revised 2026-07-22: **v0.2.5 shipped** (`WDDM` spill detection — same-day sequence: morning scope revision correcting spill semantics to **residency, not commit** per the rhyme-mdlm dogfooding report of 2026-07-19 and adding transient-spill handling; implementation + live validation on the reference `RTX 5060 Ti` including a forced-spill fixture that tuned the dedicated-saturation default from the sketched ~95% down to the measured 85%). Previous revisions: 2026-06-29 (v0.2.4 shipped; spill detection bumped one slot to v0.2.5, scope unchanged); 2026-06-13 (spill detection promoted from Speculative to Committed). Reviewer hint: for **shipped** details, the per-release roadmap (or PR body, for v0.2.3) is the authoritative source; for **forthcoming** plans, this document is the source until a per-release roadmap is drafted.*
