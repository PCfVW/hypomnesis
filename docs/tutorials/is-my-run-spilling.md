# Tutorial: Is my run spilling?

*Wrap a run with `hmn spill`, read the episode pattern, attribute the spill
per-process, then wire `SpillTracker` into your own loop.*

Every measurement below is real, captured on the reference machine (Ryzen 9
5950X + RTX 5060 Ti 16 GiB, Windows 11 / `WDDM`) during the v0.2.5 release
validation. Command lines like `python train.py` stand in for whatever you
wrap — the spilling-run figures come from the validation's forced-spill
fixture, a 20 GiB working set churned on the 16 GiB card.

---

## The problem — and the trap

Under Windows / `WDDM`, when a workload's GPU working set outgrows dedicated
`VRAM`, the kernel doesn't fail the allocation — it silently pages GPU memory
into the **shared system-memory budget** and keeps going over `PCIe`. Your run
doesn't crash; it *craters*: a 10 s/epoch loop becomes minutes, and in the
worst case shared residency grows until the machine dies. No per-process
`VRAM` counter shows this happening.

The trap is the obvious-looking heuristic. Task Manager (and `hmn ps`) show a
*committed* figure that routinely exceeds the card — a big PyTorch process
reserves a pool it never makes resident, so `committed − dedicated` is
**reservation headroom, not spill**. A dogfooding report caught this live: a
compute-bound MDLM run "spilling ~1.8 GiB" by the commit gap, while the true
shared residency sat flat at **0** the whole run and throughput was identical
across batch sizes. Spill is **residency, not commitment** — and that's what
`hmn spill` measures.

**Prerequisites:** Windows 10/11 with `WDDM 2.0`+ (any modern machine), and:

```sh
cargo install hypomnesis
```

No elevation needed — the counters are readable unprivileged. On Linux/macOS
everything below runs but reports *"spill not measurable on this platform"*;
see the [FAQ](../FAQ.md#why-is-everything-spill-related-0--false-on-linux-and-macos).

**Already running and can't be restarted under a wrapper?** `hmn spill` only
wraps a *new* command. For a job that's already hours into its run, see
[Triage a job that's already running](watching-a-running-job.md) —
`hmn watch <pid>` attaches to it directly; the episode-pattern and
per-process-attribution steps below apply identically once attached.

## Step 1 — Wrap the run

`hmn spill` is a `time(1)`-style wrapper: your command runs unchanged (stdout
untouched, exit code passed through), while `hmn` polls the adapter's
residency gauges every 100 ms. The report lands on stderr when the command
exits.

A healthy, compute-bound run — *committed over the card, and still not
spilling*:

```
$ hmn spill -- python train.py
... train.py output ...
hmn spill: peak dedicated 2.9 GiB / 15.7 GiB
           peak shared    107 MiB (baseline 107 MiB)
           episodes       0 — no spill observed
```

The ~100 MiB shared figure is the **benign baseline** — staging/upload heaps
live in shared memory by design. Flat baseline ⇒ no spill, whatever the
committed column claimed.

A genuinely spilling run (these figures are the release-validation fixture —
the 20 GiB hot working set — wrapped exactly as shown):

```
$ hmn spill -- python train.py
... train.py output ...
hmn spill: peak dedicated 14.3 GiB / 15.7 GiB
           peak shared    3.1 GiB (baseline 163 MiB)
           episodes       1 — total 13.1s, longest 13.1s, first +2.0s into run
```

Both conditions fired: dedicated-resident saturated (≥ 85% of capacity) *and*
shared-resident grew ~3 GiB past its baseline. That's the state where `VidMm`
is actively paging your tensors over `PCIe`.

## Step 2 — Read the episode pattern

Spill flickers in real workloads — the working set hovers at the boundary, so
the condition comes and goes. `hmn spill` records each contiguous spilling
stretch as one **episode**, and the pattern is the diagnosis:

| Report shape | Reading | Reaction |
|---|---|---|
| `episodes 0` | Not spilling — committed figures are irrelevant | Nothing; maybe grow the batch |
| Many short episodes (`episodes 5 — total 9.8s, longest 3.1s`) | **Marginally** over budget; the working set grazes the ceiling | Shave the batch size / context a notch; evict caches between phases |
| One sustained episode (`episodes 1 — total 13.1s, longest 13.1s`) | **Genuinely** over budget | Rethink model size, precision, or offload strategy |

The `first +2.0s into run` stamp tells you *when* — a spill that starts at the
first optimizer step is a capacity problem; one that starts 40 minutes in is a
leak or a growing KV cache.

## Step 3 — Attribute it per-process

The report is adapter-wide. To see *who* is resident in shared memory, use the
SHARED column of `hmn ps` (the same quantity as Task Manager's
`Shared GPU memory` column). An illustrative mid-spill listing:

```
$ hmn ps
PID    NAME         VRAM      SHARED   DEVICE
21844  python.exe   15.8 GiB  2.9 GiB  NVIDIA GeForce RTX 5060 Ti
3524   firefox.exe  866 MiB   25 MiB   NVIDIA GeForce RTX 5060 Ti
...
```

Scriptable — watch one PID's shared residency from outside (a watchdog keyed
off **this** field, never off commit, both false-alarms less *and* catches the
real runaway):

```sh
hmn ps --json | jq '.[] | select(.pid == 21844) | .shared_used_bytes'
```

## Step 4 — React automatically

`hmn spill --json` emits one JSON object on stdout; `jq -e` turns it into an
exit code. Fail a CI step when a run spilled:

```sh
hmn spill --json -- python train.py | jq -e '.measurable and (.spilled | not)'
```

(Check `measurable` — on a Linux runner `spilled` is `false` because nothing
*can* be measured, not because nothing happened.) There is deliberately no
`--kill` flag: what to do about a spill is your decision, composed with your
platform's native tools — see the
[FAQ](../FAQ.md#why-is-there-no-hmn-kill-or-hmn-spill---kill).

## Step 5 — Inside your own loop (`SpillTracker`)

The CLI wrapper is `SpillTracker` with a 100 ms loop. In your own Rust
inference/training loop you drive the timing yourself:

```rust,no_run
use hypomnesis::SpillTracker;

const GIB: u64 = 1024 * 1024 * 1024;

fn main() -> Result<(), hypomnesis::HypomnesisError> {
    // Optional: back off BEFORE the slowdown — 12 GiB on a 16 GiB card
    // undercuts the 85%-of-capacity default (~13.3 GiB there; capacity
    // is DXGI's figure, ~15.7 GiB on the reference card).
    let mut tracker = SpillTracker::new(0)?
        .with_dedicated_threshold(12 * GIB);

    for step in 0..10_000 {
        tracker.observe(format!("step_{step}"));
        if tracker.has_spilled() {
            // Latched: fires even if the spill was transient.
            // Drop batch size, evict KV cache, switch to CPU — your call.
            break;
        }
        // inference_step();
    }

    let report = tracker.into_report();
    eprintln!(
        "{} episode(s), peak shared {} B over a {} B baseline",
        report.episodes.len(),
        report.peak_shared_bytes,
        report.baseline_shared_bytes,
    );
    Ok(())
}
```

Three things worth knowing:

- **`has_spilled()` vs `is_spilling()`.** The latched `has_spilled()` never
  reverts — right for early-stop (a spill that came and went still told you
  the budget is marginal). The instantaneous `is_spilling()` tracks the latest
  observation — right for adaptive loops (drop batch size, wait, retry).
  There's no built-in debounce; the split plus the episode history lets you
  write any policy in a few lines.
- **Portability.** `SpillTracker` compiles on every platform. Gate the polling
  path with `hypomnesis::is_spill_measurable()` (`false` on Linux/macOS) or
  just let `observe()` no-op — `has_spilled()` stays `false` there.
- **Start the tracker before the workload.** The shared baseline is the
  *first* observation's reading; a tracker started mid-spill inflates the
  baseline and under-detects from then on.

## Gotchas

- **A spill shorter than your observation gap is invisible.** No background
  thread, by design — the tracker measures at observation points. Observe once
  per step for step-granularity, or use `hmn spill`'s 100 ms wrapper for fine
  resolution.
- **Ctrl+C kills the report.** The interrupt reaches the whole process group,
  so `hmn` dies with the wrapped command before printing.
- **On Windows, the tracker is `!Send`/`!Sync`** (raw `PDH` handles) — poll on
  one thread, signal out via an `AtomicBool` or a channel
  ([FAQ](../FAQ.md#can-i-use-spilltracker-from-another-thread)).

## Where the numbers come from

The spill signal is `PDH`'s `\GPU Adapter Memory(*)\Shared Usage` residency
gauge (adapter-wide) and `\GPU Process Memory(*)\Shared Usage` (per-process) —
the same source Task Manager reads. The 85% saturation default is not a guess:
release validation measured `VidMm`'s dedicated-resident ceiling at
≈ 88.6–91.3% of `DXGI` capacity under maximal pressure, making the sketched
95% unreachable. The full tuning record, counter verification, and design
rationale live in [`docs/roadmap-v0.2.5.md`](../roadmap-v0.2.5.md); the
dogfooding report that set the semantics is
[`docs/dogfooding-feedbacks/dogfooding-wddm-spill-detection.md`](../dogfooding-feedbacks/dogfooding-wddm-spill-detection.md).
