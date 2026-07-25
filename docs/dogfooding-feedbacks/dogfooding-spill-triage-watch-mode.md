# Dogfooding report (from rhyme-mdlm / askesis): v0.2.5 spill semantics survive live fire; the missing piece is a watch mode

**Date:** 2026-07-25
**Reporter:** rhyme-mdlm (askesis, PyTorch overnight training on RTX 5060 Ti, 16 GiB, Windows 11 / WDDM)
**Severity:** Field validation of v0.2.5 + feature request for **v0.2.6** (`hmn watch`)
**Affected area:** `hmn ps` SHARED reporting (shipped v0.2.5); the absent attach-to-running-PID mode
**Status:** Proposed — v0.2.6 candidate

---

## TL;DR

Across a ~15-hour training campaign, `hmn` delivered **three distinct spill verdicts — all three
correct**, including one true leak it helped find, fix, and verify. The v0.2.5 semantics
(commit ≠ resident; SHARED-growth-under-saturation is the signal) held up exactly as designed.
The one missing piece cost us hand-rolled workarounds every time: **there is no way to watch an
already-running PID**. `hmn spill` only wraps a *new* command; we needed per-PID deltas on a
trainer that was already 6 hours into its run. Request: `hmn watch [PID]` for v0.2.6.

## What happened — three verdicts in one campaign

The workload: 3-seed MDLM training (6L/384d, batch 256), each seed staged as fresh e0→e60 +
resume e60→e100, on a desktop also running Zed, Firefox, browsers, iCUE.

### Verdict 1 — "saturated, NOT spilling" (evening, correct)

Perf monitor showed VRAM full; the question was spill. `hmn ps`: trainer committed **14.9 GiB**
(under the 16.3 physical — wrong side of the spill precondition), SHARED **142 → 184 MiB** across
two samples minutes apart (~1% of dedicated: staging-heap scale, a benign drift with no runaway
growth). Corroborated by
throughput: ~118–121 s/epoch. Exactly the v0.2.5 report's acceptance semantics, now passed in
production: *committed can look scary while shared-resident stays benign*.

### Verdict 2 — "leak-driven spill" (morning, correct — found a real bug)

The user's eye caught spill on the perf monitor at 08:00. `hmn ps`: trainer SHARED at **718 MiB**
(vs the 142–184 baseline) with dedicated pinned. Pace history (checkpoint-file mtimes): fresh legs
~118 s/epoch, **resume legs ~362 s/epoch — 3× slower, both seeds**. Root cause in the training
script: `torch.load(map_location=device)` parked the full 480 MB resume state (model + EMA +
optimizer) in VRAM for the whole run. Fixed (askesis commit `5779858`: load to CPU, restore, free)
and **verified by the same instrument**: post-fix resume legs ran 119 s/epoch with SHARED back to
184 MiB. Diagnose → fix → verify, all through the SHARED column.

### Verdict 3 — "tenant-driven spill" (late morning, correct — no action needed)

Perf monitor showed spill again. `hmn ps`: trainer SHARED only 224 MiB — but **committed total
16.4 GiB > 16.3 physical**, driven by *new desktop tenants* (Firefox +184 MiB, a chat client
+172 MiB, an unresolved `?` process growing 628→707 MiB). System-level paging, mostly shuffling
the idle tenants; trainer took a ~15% pace brush (138 s/epoch), self-resolved when the run ended.

**The triage lesson:** the same perf-monitor symptom split into three different diagnoses with
three different actions (do nothing / fix a leak / do nothing), and the per-PID
committed-vs-SHARED decomposition is what discriminated them. `nvidia-smi` cannot make these
distinctions on WDDM.

## What was missing — the v0.2.6 request: `hmn watch`

Every verdict above needed **deltas over time**, and `hmn` only gives snapshots:

- We hand-rolled "two `hmn ps` samples minutes apart, diff by eye" three separate times.
- The pace tiebreaker had to come from **checkpoint-file mtimes**, because the trainer's Python
  stdout was block-buffered into uselessness — worth remembering as a diagnostic pattern, but it
  should not be necessary.
- `hmn spill -- <command>` is exactly the right report, but it **cannot attach to a PID that is
  already running** — which is the situation every single time a user walks up to a busy machine
  and asks "is this spilling?".

### Sketch

```
hmn watch [PID ...] [--interval 30s] [--duration 10m]
```

- One timestamped row per interval per watched PID (default: top-N by committed):
  `committed / dedicated-resident / shared-resident` plus **per-interval deltas**.
- A `SPILL` flag per row using the shipped v0.2.5 co-condition (adapter dedicated-resident ≥ 85%
  of DXGI capacity AND shared-resident non-trivial **and rising** — the "rising" part is native
  here, unlike in a snapshot).
- Exit code conveys "spill observed during the watch" so scripts and the `train_guarded.py`
  watchdog can consume it without parsing.
- Nice-to-have: `--baseline` takes the first sample as reference and prints cumulative drift, so
  "SHARED grew +530 MiB since attach" is one glance.

With `watch`, Verdict 1 is one command instead of two samples + a mental diff; Verdict 2's leak
signature (SHARED climbing every interval while dedicated saturates) would have been flagged
**during the night** rather than found at 08:00.

## Smaller observations

1. **The `?` row behaved exactly as the help warns.** An unresolvable PID held 508→732 MiB and
   *grew* across the campaign. Suggestion: when a `?` row's committed delta is large across a
   watch, say so explicitly ("unresolved process grew +224 MiB — re-run elevated to identify").
2. **Benign-baseline calibration point:** a fp32 PyTorch trainer with periodic full-state
   checkpoint saves showed SHARED at 142–224 MiB against 14.9 GiB dedicated (~1–1.5%), drifting
   upward apparently in step with checkpoint I/O (correlation observed, not isolated). Useful as a
   default "non-trivial" threshold anchor: the true leak sat at
   ~5% and climbing.
3. **Names matter in triage:** having `firefox.exe` / `claude.exe` / `QmlRenderer.exe` alongside
   the trainer in one list (the PDH every-holder semantics, deliberately different from NVML) is
   what made Verdict 3 instant. The design choice paid off.

## Acceptance fixtures (already run, free to regress against)

| fixture | committed | SHARED | pace | expected verdict |
|---|---|---|---|---|
| V1 evening | 14.9 < 16.3 GiB | 142→184 MiB benign drift | 118–121 s/ep | NOT spilling |
| V2 resume leak | 14.9 GiB + 480 MB dead state | **718 MiB** | **362 s/ep (3×)** | SPILL (leak) |
| V2 post-fix | 14.9 GiB | 184 MiB | 119 s/ep | NOT spilling |
| V3 tenants | **Σ 16.4 > 16.3 GiB** | 224 MiB (trainer) | 138 s/ep (−15%) | SPILL (system, benign) |

## Confidence

High throughout — every verdict was cross-validated by an independent signal (throughput via
checkpoint mtimes) and the leak diagnosis was confirmed by its cure.

## References

- Predecessor: [`dogfooding-wddm-spill-detection.md`](dogfooding-wddm-spill-detection.md) — the
  report that specified v0.2.5's semantics; this report is its field validation.
- askesis: `reference/rhyme_mdlm/docs/devlog.md` entries 2026-07-24/25; fix commit `5779858`
  (resume-path VRAM leak).
