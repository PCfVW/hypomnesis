# Dogfooding report (from rhyme-mdlm / askesis): WDDM spill detection must measure residency, not commit

**Date:** 2026-07-19
**Reporter:** rhyme-mdlm (askesis, PyTorch training on RTX 5060 Ti, 16 GiB, Windows 11 / WDDM)
**Severity:** Design input — prevents a false-positive in the planned v0.2.5 "spilling" feature
**Affected area:** the planned spill detection (`hmn` / `hmn ps` shared-memory reporting) and the
`train_guarded.py` watchdog that polls it
**Status:** ✅ Adopted — shipped in v0.2.5 (2026-07-22). The residency-not-commit semantics, the
saturation + shared-growth co-condition, the WDDM gating, and the watchdog guidance below all
landed as recommended; one amendment from live tuning: the "≥ ~95% of the dedicated limit"
co-condition shipped as **85%** (`VidMm`'s measured dedicated-resident ceiling is ≈ 88.6–91.3%
of `DXGI` capacity, so 95% is unreachable), and no `Dedicated Limit` PDH counter exists — the
capacity comes from `DXGI` `DedicatedVideoMemory`. See
[`docs/roadmap-v0.2.5.md`](../roadmap-v0.2.5.md#implementation-notes-as-shipped).

---

## TL;DR

Measure **resident shared bytes** (`GPU Process Memory\Shared Usage`), never infer spill from
**committed** bytes. Show `committed / dedicated-resident / shared-resident` as three separate
numbers. Flag spill only when shared-resident is non-trivial **and** dedicated is near its limit.
Gate the feature to discrete WDDM adapters. Point `train_guarded.py` at the shared number.

## What happened (the live false-positive)

During an MDLM training smoke run (block 128, batch 96) I read `hmn` mid-run and concluded the
process was **spilling ~1.8 GiB** to shared memory. It was **not**. Task Manager's *Processeur
graphique* panel showed **"Mémoire GPU partagée" flat at 0** the entire run — zero shared residency.

My wrong inference came from the committed-vs-dedicated gap:

| `hmn` reading (batch 96, block 128) | value |
|---|---|
| GPU free (dedicated) | 2 461 MiB / 16 311 MiB |
| `python.exe` committed | 11.4 GiB |
| committed total (all procs) | 15.7 GiB |
| dedicated-in-use (`total − free`) | ~13.85 GiB |
| committed − dedicated-in-use | **~1.85 GiB** ← I called this "spill" |
| Task Manager **shared** GPU memory | **0** ← ground truth: no spill |

Corroborating that it was **not** spilling: batch 96 and batch 64 had **identical throughput**
(~10 s/epoch on the smoke set) with the *3D* engine pegged near 100% — i.e. the workload is
**compute-bound**, and a spill would have *cratered* throughput, not left it flat.

## Root cause

WDDM **commit ≠ residency**. A process can *commit/reserve* GPU memory well beyond dedicated VRAM
with **zero** pages actually paged to shared system RAM — this is the normal steady state of any
large PyTorch process (its caching allocator reserves a big pool it has not made resident). `hmn`'s
current `used_bytes` is the **commit** figure. Spill is **resident shared bytes**, a different
quantity. The `committed − dedicated` gap is *reservation headroom*, not paging, and flagging it as
spill cries wolf on essentially every serious compute process.

## Recommendation

### 1. Read the residency counters (same source Task Manager uses)

Windows exposes these via PDH — the backend `hmn` already uses. Counter/instance names below are
from memory; verify with `typeperf -q "GPU Process Memory"` on the reference card:

- **`GPU Process Memory(<inst>)\Shared Usage`** — per-process resident shared bytes. **This is the
  spill number.**
- `GPU Process Memory\Dedicated Usage` — per-process resident VRAM.
- `GPU Process Memory\Total Committed` — the reservation (≈ today's `used_bytes`).
- `GPU Adapter Memory\{Dedicated,Shared} Usage` and the dedicated **Limit** — the system-wide view.

### 2. Report three numbers, clearly distinguished

Per process: `committed / dedicated-resident / shared-resident`. Relabel the current `used` as
**committed** and keep it (useful for "how much has this process reserved"), but never let the spill
logic touch it.

### 3. Flag spill on residency + saturation, not on the commit gap

Fire a `SPILL` flag when `adapter dedicated-resident ≥ ~95% of the dedicated limit` **AND**
`shared-resident > threshold` (and, when polling, *rising*). The near-limit co-condition matters
because **shared usage has a benign baseline**: staging/upload heaps and small driver buffers live
in shared memory *by design*, so `shared > 0` alone is not spill — the signal is shared **growing
as dedicated saturates**. A per-poll delta beats a single snapshot.

### 4. Gate to discrete WDDM adapters

Spill is essentially a **WDDM** concept. On **Linux/NVML** there is no silent paging of GPU
allocations to system RAM for normal CUDA — you get an OOM, not a spill (managed/UMA migration is
niche); NVML has no `Shared Usage` field, so the feature should no-op with a clear message. On
**macOS** memory is unified — discrete-VRAM overflow does not apply. Otherwise expect "why is spill
always 0 on my A100?" issues.

## Downstream payoff — the watchdog is the real consumer

`train_guarded.py` polls `hmn` "because WDDM pages silently past 16 GB and crashed the box once."
That crash is the actual hazard: **shared-resident growing unbounded eats system RAM** until the
machine dies. The guard's kill/pause threshold should key off **shared-resident**, not commit —
otherwise it *both* false-alarms on every big compute run *and* can miss the real runaway. Wiring the
guard to the new field closes that loop and is the feature's highest-value use.

## Acceptance test (free — we already ran it)

A **compute-bound** run must report **NOT spilling**: committed can exceed dedicated while
`Shared Usage ≈ 0`. Use the batch-96/block-128 case above as a regression fixture — the feature is
correct iff its per-process shared number matches Task Manager's *Mémoire GPU partagée* for the same
PID (here: 0).

## Confidence

High on the **semantics** (commit ≠ resident; shared-resident is the spill signal; PDH is the
source). Lower on the exact **instance-name formatting** PDH uses for GPU counters (the
`pid_..._luid_..._phys_...` mangling is fiddly) — verify empirically against Task Manager, which is
exactly the dogfooding loop already in place.

## References

- WDDM GPU counters (PDH `GPU Process Memory` / `GPU Adapter Memory`):
  <https://learn.microsoft.com/windows-hardware/drivers/display/gpus-in-the-performance-tab>
- Dedicated vs shared GPU memory (WDDM segments):
  <https://learn.microsoft.com/windows-hardware/drivers/display/memory-segments>
- Related: [`dogfooding-candle-mi-nvml-reserved.md`](dogfooding-candle-mi-nvml-reserved.md)
  (device-total vs reserved; same reference RTX 5060 Ti).
