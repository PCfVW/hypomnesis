# hypomnesis — FAQ

Common questions, mostly of the form *"this number looks wrong"* — where the
answer is almost always *"that's measured reality"*. Each entry links back to
the authoritative source (README limitation, rustdoc, or per-release roadmap).

- [Why does `used_bytes` exceed my card's total VRAM?](#why-does-used_bytes-exceed-my-cards-total-vram)
- [`hmn spill` or `hmn watch` — which do I use?](#hmn-spill-or-hmn-watch--which-do-i-use)
- [Why doesn't `hmn watch` show processes that start after I attach?](#why-doesnt-hmn-watch-show-processes-that-start-after-i-attach)
- [What do the `hmn ps --sort <KEY>` keys mean, and which should I use?](#what-do-the-hmn-ps---sort-key-keys-mean-and-which-should-i-use)
- [Why is the SHARED column nonzero when nothing is wrong?](#why-is-the-shared-column-nonzero-when-nothing-is-wrong)
- [How does hypomnesis decide a run is spilling?](#how-does-hypomnesis-decide-a-run-is-spilling)
- [Why is the saturation threshold 85% and not 95% (or 100%)?](#why-is-the-saturation-threshold-85-and-not-95-or-100)
- [Why is everything spill-related 0 / `false` on Linux and macOS?](#why-is-everything-spill-related-0--false-on-linux-and-macos)
- [What does a `?` in the NAME column mean — and when do I need elevation?](#what-does-a--in-the-name-column-mean--and-when-do-i-need-elevation)
- [Why is there no `hmn kill` or `hmn spill --kill`?](#why-is-there-no-hmn-kill-or-hmn-spill---kill)
- [Are the numbers exact? What about the KB 4490156 drift and the R570 sentinel?](#are-the-numbers-exact-what-about-the-kb-4490156-drift-and-the-r570-sentinel)
- [Can I use `SpillTracker` from another thread?](#can-i-use-spilltracker-from-another-thread)
- [How fast can I poll? Will `hmn spill` slow my run down?](#how-fast-can-i-poll-will-hmn-spill-slow-my-run-down)
- [How do I upgrade `hmn`? Why does `cargo install` keep the old version?](#how-do-i-upgrade-hmn-why-does-cargo-install-keep-the-old-version)

---

## Why does `used_bytes` exceed my card's total VRAM?

Because on Windows it is `WDDM`'s **dedicated commit**, not the resident set.
Under `WDDM` a process can *commit* (reserve) GPU allocations far past physical
`VRAM` with zero bytes actually paged anywhere — this is the normal steady
state of any large PyTorch-style process, whose caching allocator reserves a
pool it has not made resident. The figure matches Task Manager's
`Dedicated GPU memory` column byte-for-byte, because both read the same
`VidMm` ledger. A 15 GiB Firefox on a 16 GiB card is real, and it is *not*
spilling.

Corollary: **never infer spill from `committed − dedicated`** — that gap is
reservation headroom. A dogfooding report caught this false-positive live (a
compute-bound run "spilling ~1.8 GiB" by the commit gap while Task Manager's
shared column sat flat at 0); the correction shaped the whole v0.2.5 design.
See [`docs/dogfooding-feedbacks/dogfooding-wddm-spill-detection.md`](dogfooding-feedbacks/dogfooding-wddm-spill-detection.md).

## `hmn spill` or `hmn watch` — which do I use?

`hmn spill -- <command>` if you're launching the run yourself — it's a
`time(1)`-style wrapper, so it only works on a command it starts. `hmn watch
[PID...]` if the process is **already running** and you can't restart it
under a wrapper — the exact gap a rhyme-mdlm dogfooding report hit three
times in one 15-hour campaign (hand-rolling "two `hmn ps` samples minutes
apart, diff by eye" each time). See the
[watch tutorial](tutorials/watching-a-running-job.md) for the full
walkthrough; the two share the same `SpillTracker` core and episode
semantics, so everything in
[Is my run spilling?](tutorials/is-my-run-spilling.md) about reading the
episode pattern and attributing per-process applies to both.

Three things specific to `watch`:

- **No PID given** auto-selects the top `--top` (default 5) processes by
  committed VRAM from the first sample. By default this set is **frozen**
  for the run; use **`--follow-new`** to re-select every interval, tracking
  processes that start after attach (the `--follow-new` mode was added
  after a candle-mi dogfooding report found the frozen set missed all 19 of
  a suite's sequential `cargo test` processes — including the ones that
  caused the real spills it detected).
- **Exit code is the point**: `0` no spill observed, `1` spill observed at
  least once, `2` on a hard error (bad `--device`, nothing to auto-select,
  or `--follow-new` combined with an explicit PID) — designed for a
  watchdog script to check directly
  (`hmn watch 21844 --duration 5m; [ $? -eq 1 ] && alert`), no JSON parsing
  needed for the common case.
- **A `0 B` row isn't necessarily "exited."** `hmn watch` can't tell "PID
  exited" from "PID alive, holds no GPU memory right now" apart, and doesn't
  try to — it renders zero either way and does not auto-stop. Use
  `--duration` or Ctrl+C.

## Why doesn't `hmn watch` show processes that start after I attach?

By default it **doesn't** — `hmn watch` freezes its PID set at attach time:
it auto-selects the top `--top` (default 5) processes by committed VRAM from
the *first* sample and keeps that exact set for the run. A process born
afterward never appears, even if it later becomes the dominant GPU consumer.
This is intentional for the original use case (attach to a known
long-running job).

To track newly-spawned processes, add **`--follow-new`** — this makes
`hmn watch` re-run the top-N selection every interval instead of keeping the
frozen set from the first sample. New PIDs enter the followed set with a
fresh baseline (first sighting), and PIDs that exit or drop below rank
`--top` are finalized into the closing summary rather than rendering `0 B`
forever. The closing summary's `per_pid[]` lists *everyone who mattered
during the watch*, not the initial snapshot.

This mode exists because a candle-mi dogfooding report
([2026-07-27, extended 2026-08-01](dogfooding-feedbacks/dogfooding-watch-follow-new.md))
ran `hmn watch --duration 80m` alongside 19 sequential `cargo test` steps —
the adapter-level spill detector fired correctly, but the frozen PID set
missed every single process that actually caused the spills.

## What do the `hmn ps --sort <KEY>` keys mean, and which should I use?

The three keys answer three **distinct diagnostic questions**:

- **`dedicated`** (the default, same order as v0.2.6) — sorts by per-process
  committed dedicated VRAM, descending. Use this to answer **who do I kill
  to free VRAM?** — the processes at the top are holding the most GPU memory
  you can reclaim by terminating them.
- **`shared`** — sorts by per-process resident shared-system-memory bytes,
  descending. Use this to answer **who is currently being paged out?** — the
  symptom of spill, not its cause. Note this column is `0` on Linux (normal
  OOMs, not paging) and macOS (UMA, nothing to spill into).
- **`total`** — sorts by `dedicated + shared`, descending. Use this to
  answer **who is the biggest GPU-memory citizen overall?** — the total
  footprint, regardless of where it resides.

All three share the same tie-break rule: name ascending, then PID ascending.
On non-Windows platforms `shared` is always `0`, so `dedicated` and `total`
produce identical orderings.

## Why is the SHARED column nonzero when nothing is wrong?

Shared usage has a **benign baseline by design**: staging/upload heaps and
small driver buffers live in shared system memory even when nothing is
spilling. Live on the reference RTX 5060 Ti, an idle desktop shows an
adapter-wide shared baseline around 100–163 MiB, with per-process values of a
few hundred KiB to a few tens of MiB. `shared > 0` alone is *not* spill — the
spill signature is this number **growing while dedicated `VRAM` saturates**,
which is exactly the two-sided condition `SpillTracker` and `hmn spill` apply.
The per-process figure matches Task Manager's `Shared GPU memory` column for
the same PID.

## How does hypomnesis decide a run is spilling?

An observation counts as spilling only when **both** hold:

1. **Dedicated-resident ≥ 85% of the adapter's dedicated capacity** (or an
   absolute override via `SpillTracker::with_dedicated_threshold` — e.g.
   12 GiB on a 16 GiB card to back off *before* the slowdown starts), and
2. **Shared-resident has risen ≥ 256 MiB above its baseline** — the first
   observation's shared reading (`with_shared_growth_threshold` to override).

The co-condition suppresses the benign baseline for free: staging churn
happens regardless of saturation, while true spill only happens *at*
saturation. Each contiguous spilling stretch becomes one **episode** in the
`SpillReport` — *many short episodes* reads as "marginally over budget, shave
the batch size"; *one sustained episode* reads as "genuinely over, rethink
model / precision". Both figures are measured **residency** (`PDH`
`\GPU Adapter Memory(*)` gauges), never commit.

## Why is the saturation threshold 85% and not 95% (or 100%)?

Because `VidMm` keeps real headroom below the nominal capacity, and a
threshold you can never reach detects nothing. The original design sketch
assumed ~95%; release validation with a forced-spill fixture (a 20 GiB hot
working set churned on the 16 GiB reference card) measured the adapter-wide
dedicated-resident **ceiling at ≈ 88.6–91.3%** of `DXGI`
`DedicatedVideoMemory` — even under maximal pressure, 95% never fired while a
real 3.1 GiB spill was underway. 85% sits safely below the measured ceiling
and far above any benign desktop load (~15–20%), with the shared-growth
co-condition still suppressing false positives. The full three-run tuning
record is in
[`docs/roadmap-v0.2.5.md`](roadmap-v0.2.5.md#implementation-notes-as-shipped).

## Why is everything spill-related 0 / `false` on Linux and macOS?

Because spill into a separate shared budget is a `WDDM` architectural concept
and the other platforms honestly cannot exhibit or measure it:

| Platform | Why not |
|---|---|
| Linux | Normal `CUDA` gets an **OOM** rather than silent paging; `NVML` has no shared-residency field (managed/`UVM` migration is per-allocation, not per-process) |
| macOS / Apple Silicon | `UMA` — one physical pool; there is nothing to spill *into* |

`SpillTracker` still compiles and constructs everywhere (portable consumers
need no `cfg`), but `is_spill_measurable()` returns `false`, `observe()` is a
no-op, and `hmn spill` runs your command then prints *"spill not measurable on
this platform"* instead of a misleading all-zeros report. In `--json` output,
check `measurable` before trusting `spilled: false`.

## What does a `?` in the NAME column mean — and when do I need elevation?

`?` means the calling user cannot resolve that PID's name via `OpenProcess` —
usually a system service or another user's process. Run `hmn ps` as
Administrator to resolve most of them (macOS equivalent: `sudo hmn ps`, which
also un-skips cross-user PIDs the `ledger` syscall rejects with `EPERM`). The
Windows kernel itself (PID 4) renders as `[kernel]`, not `?`, so it never
pollutes the count.

The distinction is deliberately surfaced because it is security-relevant: a
`?` row holding substantial `VRAM` that *still* doesn't resolve under
elevation is one of — another user's process, `SYSTEM`, a `PPL`-protected
process, or a transient race — and on a single-user desktop an unexpected one
is worth investigating. Note that **measurement itself never needs
elevation**: the `PDH` counters, including everything `hmn spill` reads, are
readable unprivileged; elevation only improves *name resolution*.

## Why is there no `hmn kill` or `hmn spill --kill`?

Scope discipline: hypomnesis is **measurement, not control** (Principle 6 in
[`ROADMAP.md`](../ROADMAP.md)). Termination has platform-specific permission
models that a portable tool would inevitably get wrong somewhere, and what to
do about a spill — kill, drop the batch size, switch to CPU — is the
consumer's decision. The JSON outputs compose with the platform's native
tools instead:

```sh
hmn ps --json | jq -r '.[] | select(.used_bytes > 1073741824) | .pid' | xargs -r kill -TERM
hmn spill --json -- python train.py | jq -e '.measurable and (.spilled | not)'   # CI gate
```

The same reasoning keeps `SpillTracker` free of callbacks, background threads,
and built-in debounce — it exposes queryable state (`is_spilling()` /
`has_spilled()` / the episode history) and you wire the reaction through
whatever primitive your workload already uses.

## Are the numbers exact? What about the KB 4490156 drift and the R570 sentinel?

Exact to what the platform records, with two known platform bugs handled and
named rather than papered over:

- **KB 4490156** — Windows' `GPU Process Memory` **commit** counters can
  over-report by ~100 MiB per discard-and-restore cycle for graphics apps
  (Office-style cache churn). Compute workloads don't exhibit the trigger
  pattern, and the drift afflicts the commit accounting, **not** the
  `Shared Usage` residency gauge the spill path reads. The 256 MiB
  shared-growth margin clears it regardless.
- **`R570` `u64::MAX` sentinel** — some `R570`-class drivers report
  `0xFFFFFFFFFFFFFFFF` for every process's memory via `NVML`; hypomnesis
  detects the sentinel (and `used > total` corruption) per-row and falls back
  to `nvidia-smi` rather than reporting garbage.

Also remember the *semantic* differences across backends: Windows `PDH`
`used_bytes` is commit; macOS `graphics_footprint` and the shared column are
resident (values legitimately shrink as the kernel evicts idle pages). Check
`GpuProcessEntry::source` when comparing across platforms.

## Can I use `SpillTracker` from another thread?

Construct and poll it on **one** thread. On Windows it holds raw `PDH`
handles, so it is `!Send` / `!Sync` by construction — the compiler enforces
this; there is no `unsafe impl` escape hatch, deliberately. The intended
pattern is to signal *out* of the polling thread through whatever primitive
your workload already uses:

```rust,ignore
let spilled = Arc::new(AtomicBool::new(false));
// polling thread (owns the tracker):
tracker.observe(format!("step_{step}"));
if tracker.has_spilled() {
    spilled.store(true, Ordering::Relaxed);
}
// any other thread: spilled.load(...) and react.
```

## How fast can I poll? Will `hmn spill` slow my run down?

The counters are instantaneous kernel gauges; one `observe()` is a single
`PdhCollectQueryData` on a long-lived query — microseconds, invisible next to
a training step. `hmn spill`'s default is 100 ms (`--interval`, floor 1 ms);
values below ~50 ms add query cost without extra resolution because the GPU
counters update on driver cadence.

The honest limitation runs the other way: there is **no background thread**,
so a spill shorter than the gap between two observations is invisible. Library
consumers observing once per step get step-granularity; `hmn spill`'s 100 ms
is the answer when temporal resolution matters more than in-loop integration.

## How do I upgrade `hmn`? Why does `cargo install` keep the old version?

With a stale local registry index, `cargo install` can resolve to the version
you already have and exit `0` without building — the command appears to
succeed, but the binary on `PATH` is unchanged. `--force` rebuilds and
reinstalls unconditionally:

```sh
cargo install hypomnesis --features cli --force
hmn --version
```
