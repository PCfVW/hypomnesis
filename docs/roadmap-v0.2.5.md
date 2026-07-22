# `hypomnesis` v0.2.5 — roadmap

> *Resident, not committed. Episodes, not a boolean.*

**Status: ✅ shipped 2026-07-22.** This document was drafted the same
morning as the implementation (scope revision → plan-mode → code →
live validation, one day); the body below is the plan as it entered
plan-mode, kept intact for the record. Where the live hardware
disagreed with the plan, the
[Implementation notes (as shipped)](#implementation-notes-as-shipped)
section at the end is authoritative — most notably: **no
`Dedicated Limit` `PDH` counter exists** (capacity comes from `DXGI`),
and the dedicated-saturation default shipped at **85%**, not ~95%
(measured `VidMm` ceiling ≈ 88.6–91.3% of `DXGI` capacity).

---

## Why v0.2.5 (and not v0.3.0)

Every item is **additive and patch-safe** under the `#[non_exhaustive]`
policy: `GpuProcessEntry` gains one defaulted field
(`shared_used_bytes: u64`, `0` off-Windows), `SpillTracker` /
`SpillReport` / `SpillEpisode` are new types, `hmn spill` is a new
subcommand. No existing field changes type, no method changes signature,
no default feature flips. `used_bytes` keeps its exact v0.2.2 meaning
(PDH dedicated *commit*) — the spill logic deliberately never touches it
(see below).

---

## Origin — two dogfooding inputs, the second correcting the first

**Input 1 (the motivating slowdown).** The maintainer's own dogfooding
on the reference `Ryzen 9 5950X + RTX 5060 Ti / 16 GiB` (Windows 11 /
`WDDM`): a model committed beyond dedicated `VRAM`, paged into the
`WDDM` shared-system-memory budget, and the resulting `PCIe` traffic
slowed everything down. No per-process `VRAM` counter the crate exposes
can see this — peak `used_bytes` clips at the ceiling and says "you hit
it," not "you spilled 4 GiB into shared and that is what is slow."

**Input 2 (the correction).** A rhyme-mdlm dogfooding report
([2026-07-19](dogfooding-feedbacks/dogfooding-wddm-spill-detection.md))
caught, *live*, the false-positive the naive design would have shipped.
During an MDLM training smoke run the reporter read `hmn` mid-run and
inferred ~1.8 GiB of spill from the `committed − dedicated-in-use` gap —
while Task Manager's per-process *shared GPU memory* sat **flat at 0**
the entire run, and throughput was identical across batch sizes
(compute-bound; a real spill would have cratered it). The root cause:

> `WDDM` **commit ≠ residency**. A process can commit/reserve GPU memory
> well beyond dedicated `VRAM` with **zero** pages actually resident in
> shared system RAM — the normal steady state of any large PyTorch
> process (its caching allocator reserves a pool it has not made
> resident). The `committed − dedicated` gap is *reservation headroom*,
> not paging, and flagging it as spill cries wolf on essentially every
> serious compute process.

This is consistent with hypomnesis's own source: the v0.2.2 module docs
in `src/gpu/pdh.rs` already state that `Dedicated Usage` (today's
`used_bytes`) is "`VidMm`'s **committed** allocation total for the
process, **not** what is resident on the GPU at sample time."

**Input 3 (the maintainer's flicker observation, 2026-07-22).** Live
runs where spill *appears for a quick period, then disappears, then
reappears* — the working set hovering at the boundary. A boolean
"first spill + duration" model misreports that run entirely; the design
below makes the flicker pattern first-class.

---

## Semantics

### Spill is residency, not commitment

The spill number is PDH's **`\GPU Process Memory(*)\Shared Usage`** —
per-process *resident* shared bytes, the same quantity Task Manager's
per-process *shared GPU memory* column shows. Three distinct per-process
quantities exist and stay distinct:

| Quantity | PDH counter | hypomnesis field | Spill logic touches it? |
|---|---|---|---|
| Committed (reservation) | `\GPU Process Memory(*)\Dedicated Usage` | `used_bytes` (v0.2.2, unchanged) | **Never** |
| Dedicated-resident | `\GPU Adapter Memory(*)\Dedicated Usage` + **Limit** (adapter-wide) | internal to `SpillTracker` | Saturation co-condition |
| Shared-resident | `\GPU Process Memory(*)\Shared Usage` | `shared_used_bytes` (new) | **The spill signal** |

> **Plan-mode verification item:** the report notes its counter /
> instance names are from memory. Verify empirically with
> `typeperf -q "GPU Process Memory"` and `typeperf -q "GPU Adapter
> Memory"` on the reference card, and confirm the per-process shared
> figure matches Task Manager's *shared GPU memory* for the same PID.
> Whether `Shared Usage` shares the `pid_NNNN_luid_..._phys_N` instance
> mangling the v0.2.2 parser already handles is also confirmed here
> (expected: yes — same counter set).

### Two-sided spill condition, with a baseline

Shared usage has a **benign baseline**: staging/upload heaps and small
driver buffers live in shared memory *by design*, so `shared > 0` alone
is not spill. An observation counts as **spilling** when both hold:

1. **Dedicated-resident ≥ threshold** — default ~95% of the adapter's
   dedicated Limit; overridable via `with_dedicated_threshold(bytes)`
   for proactive back-off (e.g. fire at 14 GiB on a 16 GiB card, before
   the slowdown starts).
2. **Shared-resident has risen above its first-observation baseline**
   by a growth threshold (default a small fixed margin, tuned in
   plan-mode against the live fixture; overridable).

The co-condition suppresses most benign flicker for free: staging-heap
churn happens regardless of dedicated saturation, while true spill only
happens *at* saturation.

### Transient spills: two queries, honestly named

Flickering spill makes a single `has_spilled()` ambiguous — "is
currently spilled" and "has ever spilled" are different facts and
different consumers want each:

- **`is_spilling()`** — *instantaneous*: did the **latest** `observe()`
  meet the condition. May legitimately flip true → false → true as the
  working set hovers at the boundary. For adaptive consumers (drop
  batch size, wait, retry).
- **`has_spilled()`** — *latched*: has **any** observation met the
  condition since tracking began; never reverts to false. The
  past-tense name promises exactly this. What early-stop consumers
  almost always want — a spill that came and went between step 40 and
  step 41 still told you the budget is marginal.

Both are cheap queries over already-collected state — no fresh PDH
sample. hypomnesis exposes the two facts and no policy (Design
discipline below).

### Episode-based report

`first-spill + duration` assumes one contiguous spill; a flickering run
(five 2-second blips) would misreport as one long spill. Instead:

```rust
#[non_exhaustive]
pub struct SpillEpisode {
    pub start_label: String,        // first spilling observation
    pub end_label: Option<String>,  // None = still spilling at into_report()
    pub peak_shared_bytes: u64,
    pub observations: usize,
    pub duration: Duration,         // Instant-stamped at observe() time
}

#[non_exhaustive]
pub struct SpillReport {
    pub episodes: Vec<SpillEpisode>,
    pub peak_dedicated_bytes: u64,
    pub peak_shared_bytes: u64,
    pub baseline_shared_bytes: u64,
    pub observations: usize,
    // derived accessors: spilled(), first_spill_label(),
    // total_spill_duration(), longest_episode()
}
```

The flicker pattern *is* the data, and it is diagnostic: **many short
episodes** ⇒ working set marginally over budget (shave the batch size);
**one sustained episode** ⇒ genuinely over (rethink model / precision).

---

## Library surface (sketch — exact shape finalized in plan-mode)

- `src/gpu/pdh.rs` — add `Shared Usage` to the enumeration walk
  (second counter path per instance, same query, same
  `PdhCollectQueryData` sample); surface adapter-wide
  `\GPU Adapter Memory\Dedicated Usage` + Limit.
- `GpuProcessEntry::shared_used_bytes: u64` — additive under
  `#[non_exhaustive]`; populated on the Windows / PDH arm, `0`
  elsewhere. Per-process *attribution* ("who is spilling") comes from
  the existing `gpu_processes()` API via this field; the `SpillTracker`
  condition itself is adapter-scoped (spill is a device-level
  phenomenon; per-PID attribution is a rendering concern).
- `SpillTracker` — fold-over-snapshots, cousin of `MemoryReport`:

```rust
let mut tracker = SpillTracker::new(device_index)?
    .with_dedicated_threshold(14 * GIB);  // warn at 14, on a 16 GiB card

for step in 0..n_steps {
    tracker.observe(format!("step_{step}"));
    if tracker.has_spilled() {
        // Latched: fires even if the spill was transient.
        // Drop batch size, evict KV cache, switch to CPU — consumer's choice.
        break;
    }
    inference_step();
}
let report = tracker.into_report();
println!("{} spill episode(s)", report.episodes.len());
```

- `is_spill_measurable() -> bool` — `true` only on Windows / `WDDM`
  with the PDH counter set registered; `false` on Linux (`NVML` has no
  shared-usage field; normal CUDA OOMs rather than silently paging) and
  macOS (unified memory — nothing to spill *into*). `has_spilled()` /
  `is_spilling()` are constantly `false` there; consumers writing
  portable code skip the early-stop path entirely.

## CLI surface

```sh
hmn spill -- python train.py
# ... train.py runs to completion ...
# SpillReport prints here (stderr — preserves stdout for the wrapped command):
#   hmn spill: peak dedicated 16.0 GiB / 16.0 GiB
#              peak shared    4.2 GiB (baseline 0.3 GiB)
#              episodes       3 — total 9.8s, longest 3.1s, first +12.4s into run
```

- `time(1)`-style wrapper, **not** a TUI — sidesteps the standing
  `hmn watch` rejection. Sits on the same `SpillTracker`; labels become
  elapsed-time stamps.
- **`--interval <ms>` — polling interval, default 100 ms** (2026-07-22
  design pass: the previously fixed ~100 ms becomes the default of a
  user-settable flag). 100 ms is fine-grained enough to catch the
  flicker episodes the library's per-step `observe()` would alias away,
  cheap enough to be unnoticeable next to a training run. Sub-~50 ms
  values add PDH query cost without resolution (the GPU counters
  themselves update on driver cadence) — documented rather than
  clamped.
- `--json` — machine-readable `SpillReport` (mirrors `hmn ps --json`);
  composes with `jq` and the platform's native killer per the README
  "Composable workflows" idiom.
- On Linux / macOS: runs the wrapped command normally, then prints
  *"spill not measurable on this platform"* to stderr instead of a
  misleading all-zeros report. Exit code passes through from the
  wrapped command.

## Design discipline (deliberate non-features)

- **Measurement-only.** No background thread (consumer polls in their
  loop; lifetime + cross-process attribution stay clean). No callback
  closures (`Send`/`Sync` grief; obscures where signaling happens). No
  opinion on what to do — the consumer wires `break` / `AtomicBool` /
  `crossbeam::channel` / whatever their workload uses.
- **No built-in debounce / hysteresis.** An `n`-consecutive-observations
  knob means something completely different at 100 ms polling than at
  once-per-training-step, and time-based debounce needs clocks
  hypomnesis doesn't otherwise own. The `is_spilling()` /
  `has_spilled()` split plus the episode history lets a consumer
  implement any debounce policy in a few lines of their own code.
- **Documented sampling limitation (principle #4).** No background
  thread ⇒ a spill shorter than the consumer's inter-`observe()` gap is
  invisible; the tracker measures at observation points, full stop.
  Named in the docs; `hmn spill --interval` is the offered answer for
  fine temporal resolution.
- **No `hmn spill --kill` / `--throttle`.** Same scope discipline as
  the v0.2.3 *"Why no `hmn kill`?"* note — measurement, not control.
  Compose `hmn spill --json` with the platform's native killer.

---

## Verification

- **Unit — episode segmentation** over synthetic observation sequences
  (no FFI): flicker fixture (shared rises/falls across the condition
  three times ⇒ exactly 3 episodes, latched `has_spilled()` stays
  true, `is_spilling()` tracks the latest observation); benign-baseline
  fixture (shared constant at baseline, dedicated saturated ⇒ 0
  episodes); commit-gap fixture (dedicated below threshold, shared at
  baseline ⇒ 0 episodes); still-spilling-at-report fixture
  (`end_label == None`).
- **Live — the free acceptance test** from the dogfooding report: a
  compute-bound run whose commit exceeds dedicated must report **NOT
  spilling** (`Shared Usage ≈ 0`); the reporter's batch-96 / block-128
  MDLM case is the regression fixture. Correctness bar: the per-process
  shared number matches Task Manager's *shared GPU memory* for the same
  PID (there: 0).
- **Live — a forced true spill** on the reference `RTX 5060 Ti`:
  over-allocate past 16 GiB dedicated, confirm ≥ 1 episode, shared
  growth visible, and `hmn spill` output consistent with Task Manager.
- **Live — counter-name verification**: `typeperf -q` output recorded
  for both `GPU Process Memory` and `GPU Adapter Memory` on the
  reference card (the report flags its names as from-memory).
- `#[ignore]`-gated live tests join `tests/live_gpu.rs`; the 5 gates
  run on both Windows and Ubuntu WSL2 per the publish flow (the Linux
  leg exercises the `is_spill_measurable() == false` arm).

---

## Downstream payoff

`train_guarded.py` — the watchdog that polls `hmn` "because WDDM pages
silently past 16 GB and crashed the box once" — is the feature's first
real consumer. The actual hazard is **shared-resident growing unbounded
until the machine dies**; keyed off commit it both false-alarms on every
big compute run *and* can miss the real runaway. Pointing the guard at
`shared_used_bytes` (via `hmn spill --json` or `hmn ps --json`) closes
that loop. `candle-mi` inference loops get the library path:
`observe()` per step, `has_spilled()` to early-stop, episode report to
tell "marginally over" from "genuinely over."

---

## References

- WDDM GPU counters (PDH `GPU Process Memory` / `GPU Adapter Memory`):
  <https://learn.microsoft.com/windows-hardware/drivers/display/gpus-in-the-performance-tab>
- Dedicated vs shared GPU memory (WDDM segments):
  <https://learn.microsoft.com/windows-hardware/drivers/display/memory-segments>
- Dogfooding input: [`dogfooding-feedbacks/dogfooding-wddm-spill-detection.md`](dogfooding-feedbacks/dogfooding-wddm-spill-detection.md)
- Related prior report (same reference card): [`dogfooding-feedbacks/dogfooding-candle-mi-nvml-reserved.md`](dogfooding-feedbacks/dogfooding-candle-mi-nvml-reserved.md)

---

## Implementation notes (as shipped)

Everything above was the plan; this section records where the live
hardware and the implementation diverged from it. Shipped 2026-07-22;
all numbers measured on the reference `RTX 5060 Ti` (16 GiB, Windows
11 / `WDDM`, host `HAWKSWELL30`).

### Step-0 counter verification (`typeperf`)

- `\GPU Process Memory(*)\Shared Usage` **confirmed**, same
  `pid_NNNN_luid_0xHHHHHHHH_0xHHHHHHHH_phys_N` mangling as
  `Dedicated Usage`. Full counter list: `Shared Usage`,
  `Dedicated Usage`, `Non Local Usage`, `Local Usage`,
  `Total Committed`.
- Adapter instances are the bare `LUID` tail
  (`luid_0x00000000_0x0000F391_phys_0` live) — parsed by the new
  `parse_adapter_instance_name`, sharing a `parse_luid_tail` helper
  with the v0.2.2 process parser.
- **No `Dedicated Limit` counter exists anywhere in `PDH`** — not in
  `GPU Adapter Memory` (only `Shared Usage` / `Dedicated Usage` /
  `Total Committed`), nor in `GPU Local Adapter Memory` /
  `GPU Non Local Adapter Memory` (single usage gauge each). The
  planned fallback became the *sole* source: capacity is `DXGI`
  `DedicatedVideoMemory` via the new
  `dxgi::adapter_dedicated_video_memory`, captured once at
  `AdapterMemQuery::open`. `limit == 0` is documented as "unknown"
  and the condition then never fires without an absolute override.
- Live benign baseline on the active adapter: shared ≈ **134 MiB**
  idle — validating both the baseline concept and the 256 MiB growth
  margin.

### Threshold default: 85%, not ~95% (live-tuned)

A forced-spill fixture (`spillforge`: 20 GiB of `D3D11` default-heap
buffers on the 16 GiB card — initially a throwaway, since preserved at
[`tools/spillforge`](../tools/spillforge/) for future re-validation)
ran three times under `hmn spill`:

1. **Commit-only** (no uploads): dedicated stayed ~2.9 GiB, shared
   flat, **0 episodes** — the tracker correctly ignores pure commit;
   the rhyme-mdlm lesson self-validating.
2. **Upload-once, idle hold**: dedicated ceiling **13.9 GiB (88.6%
   of `DXGI` capacity)**, shared 102 → 508 MiB — the sketched 95%
   threshold produced a **false negative** on a real spill. `VidMm`
   keeps genuine headroom below the `DXGI` figure and demotes idle
   resources to backing store rather than shared residency.
3. **Hot churn** (round-robin touches keep the 20 GiB working set
   resident-demanded): dedicated peak **91.3%**, shared 163 MiB →
   **3.1 GiB**, **one episode, +2.0 s → +15.2 s (13.1 s)** — the
   full end-to-end validation. Even maximal churn never reached 95%.

`DEFAULT_DEDICATED_THRESHOLD_PCT` therefore shipped at **85** — below
the measured ceiling, far above benign desktop load (~15–20%), with
the shared-growth co-condition still suppressing false positives.

### Other deviations from the sketch

- **Live tests** landed in `tests/live_pdh.rs` (whole-file
  `windows + pdh` gate = exactly the spill precondition), not
  `tests/live_gpu.rs` as sketched. Three tests: measurability probe,
  idle-desktop no-false-positive (50 × 100 ms observations, 0
  episodes — the automated benign-baseline acceptance), per-process
  shared sanity with a Task Manager cross-check printout.
- **`SpillReportBuilder`** (`test-helpers` feature) was added:
  `SpillReport` is `#[non_exhaustive]`, so the `hmn` binary's own
  formatter tests could not construct fixtures — the first live
  instance of the ROADMAP's "builders per type as downstream tests
  demand" clause.
- **`SpillTracker::is_measurable(&self)`** (instance-level) joined
  the free `is_spill_measurable()` probe: the tracker can be
  individually non-measurable (adapter invisible, counter add
  refused) on a system where the counter set exists.
- **`hmn spill --device <INDEX>`** flag (default 0) was added; the
  sketch had only `--interval` and `--json`.
- **`hmn spill --json` always emits an object**, even on the
  hard-error path (an all-zeros `measurable: false` constant), so
  scripted consumers always parse one shape.
- Exit-code pass-through maps out-of-range codes (negative
  `NTSTATUS`, > 255) to `1` rather than bit-truncating — truncation
  could turn 256 into a false success. Live-verified:
  `hmn spill -- cmd /c "exit 7"` → `$LASTEXITCODE == 7`; a crashed
  fixture run passed its failure code through as 1.

### Consistency pass (pre-commit review)

Two parallel review agents (conventions compliance + adversarial
correctness) audited the full diff before commit; 15 findings, all
resolved. The three substantive ones: `fold` / `RawObservation` were
dead code on non-Windows lib builds (would have failed the Ubuntu
clippy gate — now `cfg(any(all(windows, feature = "pdh"), test))`);
`AdapterMemQuery::sample` degraded failed counter *reads* to `0`,
violating the "failed sample = skipped observation" contract (a
transient zero could falsely seal an open episode, or poison the
baseline at 0 on the first observation — reads now fail the whole
sample so `observe()` skips it); and the threshold-override unit test
had been made vacuous by the 95 → 85 retune (a 14 GiB fixture fired
under the new default too — now 12 GiB with a no-override negative
companion). Also from the pass: `is_spill_measurable()` requires a
non-empty instance list and documents that it is a probe
(`SpillTracker::is_measurable()` is the per-instance truth); a
sub-100-byte capacity can no longer produce a vacuous `>= 0`
threshold; `hmn spill --interval 0` is rejected (floor 1);
`ProcessMemoryRow.shared_bytes` renamed `shared_used_bytes` for
pipeline-wide naming consistency; stale "95%"/"14 GiB" doc references
retuned. One deliberate non-change: clap-visible `///` help text
keeps bare acronyms (`WDDM`, `PDH`), matching the pre-existing `Ps`
subcommand style — backticks would render literally in `--help`.

### Verification (as run)

The 5 gates green on **both** legs: Windows 11 (this host) and
Ubuntu WSL2 × {1.88, stable} with a separate `CARGO_TARGET_DIR`.
`cargo test --all-features` = 86 lib + 50 `hmn` + 8 smoke + 4
doctests on Windows (68 lib + 9 smoke on Linux — the extra smoke test
is the off-Windows `shared_used_bytes == 0` contract; the Linux run
exercises every non-measurable stub arm); `cargo test --test live_pdh
-- --ignored` = 6/6 on the reference card (~5.6 s, includes the idle
no-false-positive soak), re-run after the sample()-semantics fix.
WSL2 acceptance: `hmn spill -- false` → exit 1 + "spill not
measurable on this platform"; `hmn spill --json -- true` → exit 0
with a parseable `measurable: false` object.
