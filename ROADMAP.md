# hypomnesis — Roadmap

> *External RAM and VRAM, measured. Status snapshot — updated continuously as plans change.*

Per-release detail lives in [`docs/roadmap-vX.Y.Z.md`](docs/) (indexed below).
Shipped history lives in [`CHANGELOG.md`](CHANGELOG.md).
The crate's *why* lives in [`docs/hypomnesis-brief.md`](docs/hypomnesis-brief.md).

---

## Current state

**v0.2.3** shipped 2026-06-11 — first-class macOS support on Apple Silicon. *Three platforms. Same contract. Resident-bytes everywhere.* Contributor [@LittleCoinCoin](https://github.com/LittleCoinCoin)'s [PR #1](https://github.com/PCfVW/hypomnesis/pull/1) lands the macOS path: libSystem-only RAM + per-process GPU + compute-process listing (`task_info`, `ledger`, `sysctl`, `proc_listpids`, `proc_pidpath`), with `MTLDevice.recommendedMaxWorkingSetSize` via a minimal `objc2-metal` binding for the device-wide GPU budget. Two dogfooding-driven UX additions rode along: `hmn ps` stderr summary gains a "committed total" figure (signalling the `WDDM` commit-vs-resident distinction without naming it), and a "Composable workflows" `README.md` subsection documents `hmn ps --json` with two `jq` recipes — including a *"Why no `hmn kill`?"* scope-discipline note declining a hmn-side kill subcommand to preserve hypomnesis's measurement-not-control boundary. Field-validated post-release on H100 / GB200 (Linux) and a 48 GiB MacBook Pro alongside the contributor's M3 Pro daily-driver. The [PR #1 body](https://github.com/PCfVW/hypomnesis/pull/1) served as the per-release roadmap.

The preceding **v0.2.2** (2026-06-02) shipped the Windows `PDH` per-process backend — first Rust crate (to the maintainer's knowledge) to expose per-process `VRAM` for foreign processes on consumer Windows / `WDDM`, closing the dogfooding gap where `hmn ps` had silently dropped 27 processes including the maintainer's own `ollama.exe`. Detailed plan: [`docs/roadmap-v0.2.2.md`](docs/roadmap-v0.2.2.md).

---

## Committed: v0.2.4 — Spill detection (Windows / `WDDM`)

*Goal: surface the `WDDM` dedicated → shared-system-memory paging signal as a first-class measurement, so consumers can diagnose — and react to — the kind of GPU-spill slowdown that is invisible to every per-process `VRAM` counter the crate already exposes.*

Surfaced by the maintainer's own dogfooding on the `Ryzen 9 5950X + RTX 5060 Ti / 16 GiB` reference machine: a model committed beyond dedicated `VRAM`, paged into the `WDDM` shared system memory budget, and the resulting `PCIe` traffic slowed everything down. The closest existing instrument — peak `used_bytes` — clips at `total_bytes` and only tells you "you hit the ceiling," not "you spilled 4 GiB into shared and that is what is slow." The actual signal lives in PDH's `\GPU Process Memory(*)\Shared Usage` counter (sibling to the `Dedicated Usage` counter v0.2.2 already reads). Design discussion (2026-06-11 and 2026-06-13) settled the scope below.

### Why Windows-only

Spill into a separate shared-memory budget is a `WDDM` architectural concept. The asymmetry is honest, not arbitrary:

| Platform | Does "spill" exist? | Measurable per-process? |
|---|---|---|
| Windows / `WDDM` | **Yes** — dedicated `VRAM` → shared system memory budget | **Yes** — PDH `\GPU Process Memory(*)\Shared Usage` |
| Linux / `NVML` | Yes via `UVM` paging | **No** — `nvmlDeviceGetComputeRunningProcesses_v3` caps at `VRAM`; `CUDA UVM` attributes are per-allocation, not per-process |
| macOS / Apple Silicon `UMA` | **No** — single physical pool, nothing to spill *into* | N/A |

The library surface stays *cross-platform-uniform* — the same `SpillTracker` type compiles on every platform — but an honest `is_spill_measurable() -> bool` hint returns `false` on Linux and macOS, and `has_spilled()` always returns `false` there because there is no shared-memory counter to populate. Lets consumers writing portable code skip the early-stop path entirely on non-Windows targets rather than silently failing to fire. Documented limitation per principle #4.

### Library surface

- **Read the sibling PDH counter.** Add `Shared Usage` to the `src/gpu/pdh.rs` enumeration walk; add a `shared_used_bytes: u64` field to `GpuProcessEntry` (under `#[non_exhaustive]`, additive — no breaking change). Populated only on the Windows / `PDH` path; defaults to `0` on the other arms.
- **Fold-over-snapshots `SpillTracker`** — cousin of `MemoryReport`. Consumer drives observation timing: `tracker.observe(label)` in their existing inference loop, `tracker.has_spilled()` queries the latest observation (no fresh snapshot needed), `tracker.into_report()` returns peak dedicated + peak shared + first-spill label + spill duration.
- **Two usage modes via the same struct.**
  - *End-of-run report* (default): collect everything, return a `SpillReport` at the end with per-label history.
  - *Early-stop polling*: consumer queries `has_spilled()` between steps and reacts however they want (`break`, drop batch size, switch to CPU, set their own `AtomicBool`, send a `crossbeam::channel` message — hypomnesis stays out of that decision).
- **Configurable threshold for early warning.** Default threshold is `total_bytes` (real spill). Optional `with_dedicated_threshold(bytes)` lets the consumer fire `has_spilled()` *before* the actual spill — e.g. at 14 GiB on a 16 GiB card — so they can back off proactively (drop batch size, evict cached tensors) rather than reactively after the slowdown has started.
- **`is_spill_measurable() -> bool`** — honest cross-platform-portability primitive (see *"Why Windows-only"* above).

Consumer-side sketch (exact API shape TBD when scoping enters plan-mode):

```rust
let mut tracker = SpillTracker::new(device_index)
    .with_dedicated_threshold(14 * GIB);  // warn at 14, on a 16 GiB card

for step in 0..n_steps {
    tracker.observe(format!("step_{step}"));
    if tracker.has_spilled() {
        // Drop batch size, evict KV cache, switch to CPU — consumer's choice.
        break;
    }
    inference_step();
}
let report = tracker.into_report();
```

### CLI surface

A new `hmn spill -- <command>` subcommand wraps a command (`time(1)`-style — *not* a TUI), polls at a fixed interval (~100 ms) during the wrapped command's lifetime, and prints a `SpillReport` to stderr when the wrapped process exits. Internally sits on the same `SpillTracker` primitive the library exposes — labels become timestamps instead of user-supplied strings.

```sh
hmn spill -- python train.py
# ... train.py runs to completion ...
# SpillReport prints here (stderr — preserves stdout for the wrapped command):
#   hmn spill: peak dedicated 16.0 GiB / 16.0 GiB
#              peak shared    4.2 GiB
#              first spill    +12.4s into run
#              spill duration 3.8s
```

The subcommand sidesteps the `hmn watch` rejection in [Carried forward](#carried-forward-out-of-scope-until-specifically-un-gated) — it is not a live TUI, it is a record-on-exit wrapper. The `time(1)` precedent is the canonical Unix shape for this kind of measurement.

On Linux and macOS, `hmn spill` runs the wrapped command and exits with a *"spill not measurable on this platform"* message rather than printing a misleading all-zeros report.

A `--json` flag is planned (mirroring `hmn ps --json`) so a `SpillReport` can compose with `jq` and the platform's native killer — the "Composable workflows" README idiom continues to apply.

### Design discipline (deliberate non-features)

- **Stays measurement-only.** No background thread (consumer polls in their loop; lifetime + cross-process attribution stay clean). No callback closures (`Send` / `Sync` grief, obscures where signaling happens). No opinion on what to do (the queryable struct exposes state; the consumer wires whatever signaling primitive their workload uses).
- **No `hmn spill --kill` / `--throttle`.** Same scope discipline as the v0.2.3 *"Why no `hmn kill`?"* note — measurement, not control. Consumers who want automatic termination on spill compose `hmn spill --json` with the platform's native killer.

Patch-safe — additive `GpuProcessEntry::shared_used_bytes` field, new `SpillTracker` type, new `hmn spill` subcommand. No breaking changes to existing API. Per-release detail document will land at `docs/roadmap-v0.2.4.md` when scoping enters plan-mode.

---

## Speculative: v0.2.5 / v0.3.0

Items that *might* land, gated on real consumer demand:

- **Cross-platform "unmeasurable rows" diagnostic** — emit a count of detected-but-unmeasurable processes in the `hmn ps` stderr summary. Probably **never needed** now that v0.2.2 + v0.2.3 have shipped, because all three platforms reliably deliver bytes for every readable PID. Resurfaces only if an adopter reports a process they can't see, or if very-old Windows / `WDDM 1.x` environments matter to a real user.
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
- *v0.2.4 — committed; scope lives in the [Committed: v0.2.4](#committed-v024--spill-detection-windows--wddm) section above pending implementation. A `docs/roadmap-v0.2.4.md` per-release detail document will land when scoping enters plan-mode.*

Foundational documents (not per-release):

- [`docs/hypomnesis-brief.md`](docs/hypomnesis-brief.md) — *why this crate exists*: Plato + the v0.1.x VRAM saga + the extraction rationale from `candle-mi`.
- [`docs/hypomnesis-adoption.md`](docs/hypomnesis-adoption.md) — `hf-fetch-model 0.10.1` dogfooding report (the basis of v0.2.1's wave list).

---

## Principles

1. **Every patch is informed by at least one real consumer's adoption experience.** Codified in v0.2.1's CHANGELOG intro; v0.2.2 follows it (driven by the `WDDM` `[N/A]` finding on a maintainer's RTX 5060 Ti); v0.2.3 follows it (driven by the contributor's actual macOS adoption); v0.2.4 follows it (driven by the maintainer's own `WDDM` spill scenario on the same RTX 5060 Ti / 16 GiB host).
2. **Additive-by-default under `#[non_exhaustive]`.** New variants and fields land in patch releases. Type-shape changes (`u64 → Option<u64>`, etc.) are minor bumps, never patches.
3. **No new hardware backends without maintainer-accessible hardware or a contributor PR.** AMD `ROCm` and Apple Metal sat behind this gate until v0.2.3 (Apple Silicon via PR #1) un-gated half of it.
4. **Documented limitations beat papered-over half-fixes.** R570 `u64::MAX` sentinel, `WDDM` `NVML_VALUE_NOT_AVAILABLE`, KB 4490156 PDH drift, macOS cross-user `EPERM` — each is named in the source and README rather than hidden.
5. **`Display` is the default English one-liner; structured fields are canonical.** v0.2.1 Wave D's `HypomnesisError` contract — applies to every future error / measurement variant.
6. **One crate, one job.** *Tell you what's currently in this process's memory, precisely, across Windows, Linux, and macOS.* (macOS support shipped in v0.2.3 via [PR #1](https://github.com/PCfVW/hypomnesis/pull/1).) Anything that widens the job (system-wide free RAM, GPU temperature trends, live TUI, process termination) belongs in a different crate — see the *"Why no `hmn kill`?"* note in the README for the canonical example of scope discipline in action.

---

*Living document — update as plans evolve. Last revised 2026-06-13: spill detection promoted from Speculative to [Committed: v0.2.4](#committed-v024--spill-detection-windows--wddm) after the design conversation settled the library `SpillTracker` + CLI `hmn spill -- <command>` wrapper + `is_spill_measurable()` cross-platform-honest hint. Previous revision (2026-06-11, post-v0.2.3 release) recorded macOS as shipped (Apple Silicon via PR #1) and elevated spill detection from carried-forward. Reviewer hint: for **shipped** details, the per-release roadmap (or PR body, for v0.2.3) is the authoritative source; for **forthcoming** plans, this document is the source until a per-release roadmap is drafted.*
