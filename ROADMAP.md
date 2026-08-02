# hypomnesis — Roadmap

> *External RAM and VRAM, measured. Status snapshot — updated continuously as plans change.*

Per-release detail lives in [`docs/roadmap-vX.Y.Z.md`](docs/) (indexed below).
Shipped history lives in [`CHANGELOG.md`](CHANGELOG.md).
The crate's *why* lives in [`docs/hypomnesis-brief.md`](docs/hypomnesis-brief.md).

---

## Current state

**v0.2.7** shipped 2026-08-02 — `hmn watch --follow-new` and `hmn ps
--sort`. *Follow the work, not just the machine. Sort by the question
you're actually asking.* A candle-mi dogfooding report
([2026-07-27, extended 2026-08-01](docs/dogfooding-feedbacks/dogfooding-watch-follow-new.md))
ran `hmn watch` alongside candle-mi's `scripts/resurrect.ps1` oracle suite —
19 sequential `cargo test` processes over 44 minutes — and found the
adapter-level spill machinery flawless (three real episodes, including a
fast 20-second Mistral-7B spike candle-mi's own wall-clock heuristic had
never caught) while the per-PID half answered the wrong question: `watch`'s
auto-selected set froze at the first sample, so none of the nineteen
processes that actually caused the spills were ever attributed. `--follow-new`
(auto-select mode only; a hard error combined with explicit PIDs) re-runs the
top-`--top` selection every interval instead of once at attach — a PID
entering starts fresh, a PID leaving is *finalized* into the closing
summary's `per_pid[]` instead of rendering `0` forever, tracked via a new
`WatchState`'s `seen_order` (first-seen, no duplicates). Re-entry after a gap
resumes existing history; only the existing OS-PID-reuse name-change
detector resets a row. A companion request from the same suite, filed
separately: `hmn ps --sort <dedicated|shared|total>`, sharing a single
`ps_row_comparator` with `hmn watch`'s auto-selection (`select_top_n_pids`,
always pinned to `Dedicated`) so the two orderings can't drift apart — a
deliberate, live-confirmed consequence being that `hmn watch`'s auto-selected
top-N can now pick a different PID than pre-v0.2.7 at an exact VRAM tie.
Both features validated the same way v0.2.6 was: two sequential real
`spillforge` forced-spill runs under `hmn watch --follow-new --json`,
correctly tracked as distinct entries with two separate spill episodes and
all seven ever-seen PIDs finalized into the summary, both manually and via a
new automated `#[ignore]`-gated end-to-end test. The repo also transferred
to the `mi-for-the-rust-of-us` GitHub org during v0.2.7 (joining `anamnesis`
and `candle-mi`); v0.2.7 carries the corrected crates.io metadata.
Detailed plan: [`docs/roadmap-v0.2.7.md`](docs/roadmap-v0.2.7.md).

The preceding **v0.2.6** shipped 2026-07-25 — `hmn watch [PID...]`, attach-to-a-running-PID
spill triage. *Not a TUI. Same tracker, a timer instead of a wrapped child.*
`hmn spill -- <command>` only wraps a *new* process; a rhyme-mdlm dogfooding
report ([2026-07-25](docs/dogfooding-feedbacks/dogfooding-spill-triage-watch-mode.md))
hit that wall three times triaging a 15-hour training campaign, hand-rolling
"two `hmn ps` samples minutes apart, diff by eye" every time because there
was no way to attach to a trainer already hours into its run. `hmn watch`
closes the gap as a pure CLI addition — zero changes to `src/spill.rs` or
`src/gpu/pdh.rs`: it samples the unchanged `SpillTracker` (adapter-wide
dedicated-saturation + shared-growth co-condition) and the unchanged
`gpu_processes()` (per-PID committed/shared bytes) on a timer, printing one
row per watched PID per interval with per-interval deltas and a live SPILL
flag. No PID given auto-selects the top `--top` (default 5) by committed
`VRAM` from the first sample, kept fixed for the run; explicit PIDs are
watched exactly as given. `--interval` / `--duration` take duration strings
(`30s`, `5m`, bare seconds — a hand-rolled parser, no new duration-parsing
dependency) rather than `hmn spill`'s raw milliseconds, tuned for an
attach-and-leave-running tool rather than a tight wrap. Ctrl+C (via the new
`ctrlc` dependency, `cli`-feature-only) and a natural `--duration` stop both
print the same closing summary — the same `SpillReport` shape `hmn spill`
emits, plus a per-PID peak/baseline table — and set the same exit-code
contract: `0` no spill observed, `1` spill observed at least once, `2` on a
hard error, designed for a watchdog script to check directly without parsing
JSON. `--json` streams JSON Lines (one `"kind":"sample"` object per PID per
interval, a closing `"kind":"summary"` object) rather than a single blob, to
match `watch`'s live-tailing character. Two small best-effort robustness
additions found during an adversarial pre-commit review (the same two-agent
conventions-plus-correctness pass v0.2.5 used): a watched PID whose resolved
name changes between samples (OS PID reuse) resets that row's baseline
rather than mixing two processes' readings, and an unresolved (`?`) watched
PID that grows past 256 MiB since attach gets a one-shot elevation hint.
Live-validated against the same `spillforge` forced-spill fixture that
validated `hmn spill` in v0.2.5 (both an automated `#[ignore]`-gated
end-to-end test spawning the real compiled binaries, and manual dogfooding
runs recorded in the roadmap doc) plus a real idle-desktop no-false-positive
run with auto-selected top-3 PIDs. This resolves the "Carried forward"
table's `hmn watch (TUI live-refresh)` row below — the rejected item was a
curses-style redraw dashboard; what shipped is explicitly not that. Detailed
plan: [`docs/roadmap-v0.2.6.md`](docs/roadmap-v0.2.6.md).

The preceding **v0.2.5** shipped 2026-07-22 — `WDDM` spill detection. *Resident, not committed. Episodes, not a boolean.* A `SpillTracker` that compiles on every platform (honest `is_spill_measurable()` returns `false` off-Windows) reads the `PDH` `\GPU Adapter Memory(*)` residency gauges and flags spill only when dedicated-resident saturates **and** shared-resident grows past its benign first-observation baseline — never from the `committed − dedicated` gap, per the rhyme-mdlm dogfooding report's live false-positive ([2026-07-19](docs/dogfooding-feedbacks/dogfooding-wddm-spill-detection.md)). Transient spills are first-class: an instantaneous `is_spilling()` / latched `has_spilled()` split plus an episode-based `SpillReport`. Per-process attribution rides along as additive `GpuProcessEntry::shared_used_bytes` (+ `hmn ps` SHARED column), and `hmn spill -- <command>` wraps any run `time(1)`-style (`--interval` default 100 ms, `--json`, exit-code pass-through). Two live-measured corrections to the design sketch: no `Dedicated Limit` counter exists in `PDH` (capacity comes from `DXGI` `DedicatedVideoMemory`), and the dedicated-saturation default is **85%**, not ~95% — a forced-spill fixture measured `VidMm`'s dedicated-resident ceiling at ≈ 88.6–91.3% of `DXGI` capacity, making 95% unreachable. Release-validated with a real 13.1 s spill episode (3.1 GiB peak shared) on the reference `RTX 5060 Ti`, produced by a forced-spill fixture preserved at [`tools/spillforge`](tools/spillforge/) (repo-only, `publish = false`) for future re-validation on new drivers or contributor hardware. Detailed plan: [`docs/roadmap-v0.2.5.md`](docs/roadmap-v0.2.5.md).

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
- **Builders for `ProcessGpuInfo` and `Snapshot` under `test-helpers`** — add per type as downstream tests demand. (Clause exercised twice so far, both times demanded by the `hmn` binary's own tests: `SpillReportBuilder` in v0.2.5, `GpuProcessEntryBuilder` in v0.2.6 — so `GpuProcessEntry` has left this list. `ProcessGpuInfo` and `Snapshot` are the two still pending.)
- **`hmn spill` partial report on Ctrl+C** (deferred from v0.2.5) — a `ctrlc` handler so an interrupted run still prints what was observed. **The original rationale has expired**: it was deferred "to keep v0.2.5 dependency-free", but v0.2.6 took `ctrlc` as a `cli`-feature dependency for `hmn watch`, so the cost is already paid and `run_spill` could reuse the same `Arc<AtomicBool>` + interruptible-sleep pattern `run_watch` already runs. What remains is the genuinely harder half, and it is a design question rather than a dependency one: Ctrl+C reaches the whole process group, so `hmn spill` must decide what it owes the *wrapped child* (has it died? should we wait? what exit code do we then pass through?) before it can report honestly. Current behaviour is documented in the `--help` text. Still un-gated by a consumer who actually loses a report they needed.
- **`SpillTracker` auto-reopen after driver reset / `TDR`** (deferred from v0.2.5) — today a reset invalidates the long-lived `PDH` query and every later `observe()` is a skipped observation (documented). Un-gated by a real consumer whose runs survive TDRs.
- **Per-process attribution inside `SpillTracker` / adapter `Total Committed` exposure** (deferred from v0.2.5) — the tracker's condition is deliberately adapter-scoped; per-PID shared attribution lives in `gpu_processes()`. Fold attribution into the report only if a consumer shows the two-step flow (`hmn spill` then `hmn ps`) losing the culprit in practice. **Near-miss on that gate in v0.2.7**: the candle-mi report *did* lose the culprit — the adapter shouted SPILL for 16 minutes while the per-PID table showed only desktop tenants — but the cause was `hmn watch`'s frozen PID set, not the tracker's adapter-scoping, and `--follow-new` fixed it entirely at the CLI layer. The library-level fold stays un-gated.
- **Harden the `hmn watch` PID-reuse reset against name-resolution races** (surfaced by the v0.2.7 report) — the reset that detects OS PID reuse fires only when *both* the previous and current sample carry a resolved process name, so a short-lived recycled PID whose name lookup loses the race silently keeps the old process's baseline. The report saw exactly this shape: a `firefox.exe` row peaking at an implausible 15.7 GB, timed with the big-model steps. Documented as best-effort in the `--help` text and the watch tutorial's Gotchas. A fix would need a second identity signal beyond the name (process start time is the obvious candidate, at the cost of a per-PID `OpenProcess`); un-gated by a consumer for whom the mis-attribution actually changes a diagnosis.

---

## Carried forward (out of scope until specifically un-gated)

| Idea | Why deferred | What would un-gate it |
|------|--------------|----------------------|
| **AMD `ROCm` backend** (`rocm_smi_lib`) | Maintainer has no AMD dGPU; shipping untested `FFI` violates project discipline | Hardware access **or** a contributor PR with maintainer hardware coverage |
| **AMD iGPU on Linux** | `NVML` doesn't see it; needs separate Linux `DRM` / `sysfs` path | Same as AMD `ROCm` |
| **Intel Arc / Intel iGPU on Linux** | No backend in the crate; same Linux-`DRM` problem | Hardware access or contributor PR |
| **Apple Metal on Intel Macs** (legacy `AMD` / Intel discrete GPUs) | v0.2.3's `ledger` mechanism likely works, but no Intel-Mac test hardware | Intel-Mac test machine or contributor PR |
| **Strict-accounting `D3DKMTQueryStatistics` Windows backend** | "Reserved for system use. Do not use." per Microsoft docs; undocumented kernel-thunk surface | A real consumer who reports KB 4490156 drift biting their specific workload |
| ~~**`hmn watch` (TUI live-refresh)**~~ | *Resolved in v0.2.6* — but not as a TUI. The rejected item was an `nvtop`-style curses redraw dashboard; `hmn watch [PID...]` is a `time(1)`-style scrolling sampler (same discipline as `hmn spill`), un-gated by the rhyme-mdlm dogfooding report that tried the `hmn ps`-diff-by-eye workaround and explained why it was insufficient. See [`docs/roadmap-v0.2.6.md`](docs/roadmap-v0.2.6.md). | — |
| **`hmn` reading from another machine over SSH / RPC** | Out of scope; users run `ssh host hmn` | Not planned |
| **TUI / live mode (`hmn top`)** | That's `nvtop`'s job — `hmn watch` (v0.2.6) is deliberately not this: no redraw, no cursor control, plain scrolling output | Not planned |

`#[non_exhaustive]` keeps every one of these additive — none requires a 1.0 bump.

---

## Per-release detail (index)

- [`docs/roadmap-v0.2.0.md`](docs/roadmap-v0.2.0.md) — shipped 2026-05-06. *Wider, not taller.* `Snapshot::all`, `gpu_processes`, `hmn` CLI, `report`-feature `format_free` / `print_free`.
- [`docs/roadmap-v0.2.1.md`](docs/roadmap-v0.2.1.md) — shipped 2026-05-13. *Sharper, not wider.* `test-helpers` builder, `name_or_unknown`, `format_total` / `format_used`, `HypomnesisError` `Display` contract, README "Used by" + brief refresh.
- [`docs/roadmap-v0.2.2.md`](docs/roadmap-v0.2.2.md) — shipped 2026-06-02. *Truer, not wider.* Windows `PDH` per-process backend, `?`-row security-relevant hint, PID 4 rendered as `[kernel]`.
- *v0.2.3 — no separate per-release document; the [PR #1](https://github.com/PCfVW/hypomnesis/pull/1) body served as the per-release roadmap.*
- [`docs/roadmap-v0.2.4.md`](docs/roadmap-v0.2.4.md) — shipped 2026-06-29. *The same total. Now with the carve-out shown.* NVML v2 `reserved` carve-out surfaced as additive `GpuDeviceInfo::reserved_bytes`, `hmn` summary parenthetical, pre-R510 graceful fallback.
- [`docs/roadmap-v0.2.5.md`](docs/roadmap-v0.2.5.md) — shipped 2026-07-22. *Resident, not committed. Episodes, not a boolean.* `WDDM` spill detection: `PDH` `Shared Usage` residency gauges, `SpillTracker` with `is_spilling()` / `has_spilled()` split + episode-based `SpillReport`, `GpuProcessEntry::shared_used_bytes` + `hmn ps` SHARED column, `hmn spill -- <command>` wrapper with exit-code pass-through. Threshold default live-tuned to 85% via a forced-spill fixture.
- [`docs/roadmap-v0.2.6.md`](docs/roadmap-v0.2.6.md) — shipped 2026-07-25. *Not a TUI. Same tracker, a timer instead of a wrapped child.* `hmn watch [PID...]`: attach-to-a-running-PID spill triage, pure CLI addition over the unchanged `SpillTracker` / `gpu_processes()`, auto top-N PID selection, duration-string `--interval` / `--duration`, `0`/`1`/`2` exit-code contract, JSON Lines streaming, `ctrlc`-backed graceful Ctrl+C summary, best-effort PID-reuse baseline reset.
- [`docs/roadmap-v0.2.7.md`](docs/roadmap-v0.2.7.md) — shipped 2026-08-02. *Follow the work, not just the machine. Sort by the question you're actually asking.* `hmn watch --follow-new`: re-run top-N selection every interval, departed PIDs finalized into `per_pid[]` via a new `WatchState` `seen_order` roster instead of frozen at attach. `hmn ps --sort <dedicated|shared|total>`: a shared `ps_row_comparator` between `hmn ps` and `hmn watch`'s auto-selection. GitHub org transfer to `mi-for-the-rust-of-us`.

Foundational documents (not per-release):

- [`docs/hypomnesis-brief.md`](docs/hypomnesis-brief.md) — *why this crate exists*: Plato + the v0.1.x VRAM saga + the extraction rationale from `candle-mi`.
- [`docs/hypomnesis-adoption.md`](docs/hypomnesis-adoption.md) — `hf-fetch-model 0.10.1` dogfooding report (the basis of v0.2.1's wave list).

---

## Principles

1. **Every patch is informed by at least one real consumer's adoption experience.** Codified in v0.2.1's CHANGELOG intro; v0.2.2 follows it (driven by the `WDDM` `[N/A]` finding on a maintainer's RTX 5060 Ti); v0.2.3 follows it (driven by the contributor's actual macOS adoption); v0.2.4 follows it (driven by a `candle-mi` v0.1.16 dogfooding report asking for the NVML driver-reserved carve-out on the same RTX 5060 Ti); v0.2.5 follows it (driven by the maintainer's own `WDDM` spill scenario on the same RTX 5060 Ti / 16 GiB host, then course-corrected by a rhyme-mdlm dogfooding report's live commit-vs-residency false-positive before a line of code was written); v0.2.6 follows it (driven by a rhyme-mdlm dogfooding report's 15-hour field-validation campaign, which hit the "can't attach to an already-running PID" gap three separate times); v0.2.7 follows it (driven by a candle-mi dogfooding report running `hmn watch` against a 19-process sequential test suite, which found the per-PID auto-selection frozen at attach never saw any of the processes that caused the spills it correctly detected at the adapter level).
2. **Additive-by-default under `#[non_exhaustive]`.** New variants and fields land in patch releases. Type-shape changes (`u64 → Option<u64>`, etc.) are minor bumps, never patches.
3. **No new hardware backends without maintainer-accessible hardware or a contributor PR.** AMD `ROCm` and Apple Metal sat behind this gate until v0.2.3 (Apple Silicon via PR #1) un-gated half of it.
4. **Documented limitations beat papered-over half-fixes.** R570 `u64::MAX` sentinel, `WDDM` `NVML_VALUE_NOT_AVAILABLE`, KB 4490156 PDH drift, macOS cross-user `EPERM` — each is named in the source and README rather than hidden.
5. **`Display` is the default English one-liner; structured fields are canonical.** v0.2.1 Wave D's `HypomnesisError` contract — applies to every future error / measurement variant.
6. **One crate, one job.** *Tell you what's currently in this process's memory, precisely, across Windows, Linux, and macOS.* (macOS support shipped in v0.2.3 via [PR #1](https://github.com/PCfVW/hypomnesis/pull/1).) Anything that widens the job (system-wide free RAM, GPU temperature trends, live TUI, process termination) belongs in a different crate — see the *"Why no `hmn kill`?"* note in the README for the canonical example of scope discipline in action.

---

*Living document — update as plans evolve. Last revised 2026-08-02: **v0.2.7 shipped** (`hmn watch --follow-new` + `hmn ps --sort` — driven by a candle-mi dogfooding report against a 19-process sequential test suite; two independent two-agent conventions-plus-adversarial-correctness passes, one per feature, each finding and fixing a test that didn't actually discriminate the behavior it claimed to test plus several smaller issues; live validation via two sequential real `spillforge` forced-spill runs under `--follow-new`, both manually and via a new automated end-to-end test; same-release GitHub org transfer to `mi-for-the-rust-of-us`). Previous revisions: 2026-07-25 (v0.2.6 shipped — `hmn watch [PID...]`, same-day sequence: plan-mode design session resolving the rejected "TUI live-refresh" `hmn watch` item into a non-TUI `time(1)`-style sampler per a rhyme-mdlm dogfooding report; implementation as a pure CLI addition with zero library-surface changes; a two-agent conventions-plus-adversarial-correctness pass that found and fixed one compile-breaking test gap and added a best-effort PID-reuse baseline reset; live validation against the `spillforge` fixture via both an automated end-to-end test and manual dogfooding); 2026-07-22 (v0.2.5 shipped — `WDDM` spill detection, same-day sequence: morning scope revision correcting spill semantics to **residency, not commit** per the rhyme-mdlm dogfooding report of 2026-07-19 and adding transient-spill handling; implementation + live validation on the reference `RTX 5060 Ti` including a forced-spill fixture that tuned the dedicated-saturation default from the sketched ~95% down to the measured 85%); 2026-06-29 (v0.2.4 shipped; spill detection bumped one slot to v0.2.5, scope unchanged); 2026-06-13 (spill detection promoted from Speculative to Committed). Reviewer hint: for **shipped** details, the per-release roadmap (or PR body, for v0.2.3) is the authoritative source; for **forthcoming** plans, this document is the source until a per-release roadmap is drafted.*
