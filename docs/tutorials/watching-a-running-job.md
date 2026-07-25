# Tutorial: Triage a job that's already running — `hmn watch`

*Attach to a PID that's already hours into its run, read the live SPILL
column, and script a watchdog off the exit code.*

This tutorial picks up where [Is my run spilling?](is-my-run-spilling.md)
leaves off. That one wraps a **new** command with `hmn spill -- <command>`.
This one covers the situation that motivated `hmn watch` in the first place:
you walk up to a machine that's already been running for hours and ask "is
*this* spilling?" — with no way to restart it under a wrapper.

## The problem `hmn spill` can't solve

A rhyme-mdlm dogfooding report ([2026-07-25](../dogfooding-feedbacks/dogfooding-spill-triage-watch-mode.md))
tracked a 15-hour, 3-seed training campaign and hit the same wall three
times: the perf monitor showed a spill symptom on an *already-running*
trainer, and `hmn spill` — a wrapper for new commands — couldn't attach to
it. Every time, the workaround was "run `hmn ps` twice, minutes apart, and
diff the SHARED column by eye." That hand-rolled loop is exactly what
`hmn watch` automates. The campaign's three triage verdicts, all reached
through the same per-process committed-vs-SHARED decomposition v0.2.5
shipped:

| Verdict | Trainer SHARED | Dedicated | Diagnosis | Action |
|---|---|---|---|---|
| Evening | 142 → 184 MiB (benign drift) | 14.9 GiB commit, under budget | Not spilling — commit looked scary, residency didn't | None |
| Morning | 718 MiB (vs. 142–184 baseline) | pinned at capacity | Real leak — `torch.load` parked 480 MB of resume state in `VRAM` all run | Fixed the load path; verified the fix by watching SHARED drop back to 184 MiB |
| Late morning | 224 MiB (trainer) | Σ 16.4 > 16.3 GiB *system-wide* | Spill, but **not the trainer's** — new desktop tenants (a browser, a chat client) pushed the adapter over budget | None — self-resolved when the tenants closed |

Same symptom, three different diagnoses, three different actions — and the
discriminator every time was **per-process committed vs. SHARED**, read live
against a running process. That's what `hmn watch` gives you directly instead
of two manual `hmn ps` samples and a mental diff.

**Prerequisites:** same as the other tutorial — Windows 10/11 with
`WDDM 2.0`+, no elevation needed:

```sh
cargo install hypomnesis --features cli
```

On Linux/macOS `hmn watch` still attaches and shows real per-PID `VRAM`
deltas — the SPILL column just never fires (see the
[FAQ](../FAQ.md#why-is-everything-spill-related-0--false-on-linux-and-macos)).

## Step 1 — Attach

You need a PID, or none at all. With no PID, `hmn watch` auto-selects the
top `--top` processes (default 5) by committed `VRAM` from its first sample
and watches that fixed set for the run — useful when you don't already know
which PID is the trainer:

```
$ hmn watch --top 3 --interval 2s --duration 20s
hmn watch: device 0 [NVIDIA GeForce RTX 5060 Ti], interval 2.0s, watching 3 PID(s) (top 3 by committed)
TIME      PID     NAME             COMMITTED  ΔCOMMIT    SHARED     ΔSHARED    SPILL
+0.0s     2624    ?                964 MiB    +0 B       0 MiB      +0 B       no
+0.0s     19356   QmlRenderer.exe  870 MiB    +0 B       86 MiB     +0 B       no
+0.0s     24712   firefox.exe      339 MiB    +0 B       0 MiB      +0 B       no
+2.0s     2624    ?                964 MiB    +0 MiB     1 MiB      +0 MiB     no
+2.0s     19356   QmlRenderer.exe  829 MiB    -40 MiB    92 MiB     +6 MiB     no
+2.0s     24712   firefox.exe      339 MiB    +0 B       0 MiB      +0 B       no
...
hmn watch: peak dedicated 1.8 GiB / 15.7 GiB
           peak shared    150 MiB (baseline 122 MiB)
           episodes       0 — no spill observed
hmn watch: per-PID  PID    NAME             BASELINE COMMIT  PEAK COMMIT  BASELINE SHARED  PEAK SHARED
                    2624   ?                964 MiB          996 MiB      0 MiB            2 MiB
                    19356  QmlRenderer.exe  870 MiB          870 MiB      86 MiB           92 MiB
                    24712  firefox.exe      339 MiB          431 MiB      0 MiB            20 MiB
```

*(Real output, captured live on the reference RTX 5060 Ti alongside ordinary
desktop load — no training run in this transcript, which is exactly why
`episodes 0` is the right answer: negative deltas like `QmlRenderer.exe`'s
`-40 MiB` are normal churn, not spill.)*

When you already know the PID — from `hmn ps`, from your training script's
own PID, or from Task Manager — watch it directly: `hmn watch 21844`. Explicit
PIDs are watched exactly as given; `--top` is ignored.

## Step 2 — Read the live SPILL column

Each row is one PID at one interval: committed `VRAM` and its delta, resident
SHARED bytes and its delta, and a SPILL flag. The flag is the **adapter-wide**
instantaneous state ([`SpillTracker::is_spilling`](https://docs.rs/hypomnesis) —
spill is a device-level phenomenon, so it reads the same on every row sharing
a timestamp) — the row whose own SHARED delta is climbing *at that moment* is
your culprit, the same per-process attribution
[Step 3 of the other tutorial](is-my-run-spilling.md#step-3--attribute-it-per-process)
covers for `hmn ps`. Forced onto a real spill (the same `spillforge` fixture
that validated `hmn spill` in v0.2.5, `20` GiB target / `20` s hold), watched
mid-run:

```
$ hmn watch 18640 --interval 2s --duration 90s --json
```

Two real lines from that JSON Lines stream — one per-interval sample, and the
closing summary:

```json
{"kind":"sample","t_ms":0,"pid":18640,"name":"spillforge.exe","used_bytes":14239346688,"used_delta_bytes":0,"shared_used_bytes":310120448,"shared_delta_bytes":0,"spilling":false}
{"kind":"summary","measurable":true,"spilled":true,"observations":45,"baseline_shared_bytes":458842112,"peak_shared_bytes":3179388928,"peak_dedicated_bytes":16272728064,"dedicated_limit_bytes":16831741952,"total_spill_duration_ms":0,"episodes":[{"start_label":"+54.1s","end_label":"+56.1s","peak_shared_bytes":3179388928,"observations":1,"duration_ms":0}],"per_pid":[{"pid":18640,"name":"spillforge.exe","baseline_used_bytes":14239346688,"peak_used_bytes":14239375360,"baseline_shared_bytes":310120448,"peak_shared_bytes":3042873344}]}
```

The **episode pattern** (many short episodes vs. one sustained one) reads
exactly like `hmn spill`'s report —
[Step 2 of the other tutorial](is-my-run-spilling.md#step-2--read-the-episode-pattern)
is the same table, unchanged. `--json` streams one `"kind":"sample"` object
per PID per interval as it happens (pipe live to `jq -c`), plus a single
`"kind":"summary"` object when the watch ends — the same `SpillReport` shape
`hmn spill --json` emits, plus a `per_pid[]` peak/baseline array.

## Step 3 — Script the exit code

`hmn watch` doesn't wrap a child, so its exit code is free to *mean*
something: `0` if spill was never observed, `1` if it was at least once, `2`
on a hard error (bad `--device`, or nothing to auto-select). That is a direct
"is this run spilling *right now*, yes or no" answer for a watchdog script —
no JSON parsing required for the common case:

```sh
hmn watch 21844 --duration 5m
if [ $? -eq 1 ]; then
    echo "spilled during the last 5 minutes — investigate before the next resume"
fi
```

Or attach indefinitely (omit `--duration`) and let Ctrl+C stop it — the
closing summary and exit code print the same way on interrupt as on a natural
`--duration` stop.

## Gotchas specific to `watch`

- **A `0 B` / `0 B` row doesn't mean the process exited.** `hmn watch` can't
  distinguish "PID exited" from "PID alive but currently holds no GPU memory
  on this device" — a watched PID simply renders zeroed that interval either
  way, and `hmn watch` does not auto-stop on this basis. Use `--duration` or
  Ctrl+C to end the watch.
- **PID reuse is handled best-effort.** If the OS recycles a watched PID onto
  an unrelated process mid-watch, a resolved-name change between samples is
  used as the signal to reset that row's baseline — so the closing summary
  describes the new process, not a mix of both. This can't catch every case
  (two same-named processes recycling the PID would look identical), but it
  closes the common one.
- **`--interval` and `--duration` take duration strings**, not raw
  milliseconds like `hmn spill --interval` — `500ms`, `30s`, `5m`, `1h`, or a
  bare number (seconds). Shorter intervals catch brief flicker episodes at
  the cost of more `PDH` queries per second; the default is `5s`, tuned for
  attach-and-leave-running rather than `hmn spill`'s tight 100 ms wrap.
- **An unresolved (`?`) watched PID that grows** — committed or shared, by
  256 MiB or more since attach — gets a one-shot stderr hint
  (`re-run elevated to identify`), the same elevation story as `hmn ps`'s `?`
  rows (see the
  [FAQ](../FAQ.md#what-does-a--in-the-name-column-mean--and-when-do-i-need-elevation)).

## Where the numbers come from

Same source as `hmn spill`: `PDH`'s `\GPU Adapter Memory(*)` residency gauges
for the adapter-wide SPILL flag and episode history, and
`\GPU Process Memory(*)\Dedicated Usage` / `\Shared Usage` for each row's
per-PID committed/shared figures — `hmn watch` adds no new measurement
source, only a timer loop and delta bookkeeping around
[`SpillTracker`](https://docs.rs/hypomnesis) and
[`gpu_processes()`](https://docs.rs/hypomnesis). The design rationale and the
dogfooding report that motivated it live in
[`docs/roadmap-v0.2.6.md`](../roadmap-v0.2.6.md) and
[`dogfooding-spill-triage-watch-mode.md`](../dogfooding-feedbacks/dogfooding-spill-triage-watch-mode.md).
