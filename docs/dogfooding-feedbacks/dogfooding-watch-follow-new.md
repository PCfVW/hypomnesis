# Dogfooding report (from candle-mi): `hmn watch` catches every spill at the adapter — and attributes none of them, because its PID set is frozen at the first sample

**Date:** 2026-07-27, extended 2026-08-01
**Reporter:** candle-mi oracle resurrection (`scripts/resurrect.ps1` Default tier, 19 steps, RTX 5060 Ti 16 GiB, Windows 11 / WDDM, `hmn 0.2.6`; the 2026-08-01 addition on `hmn 0.2.6` likewise)
**Severity:** Field validation of v0.2.6 `watch` (adapter level: three for three) + **two** feature requests for **v0.2.7**: `watch --follow-new` and `ps --sort`
**Affected area:** `hmn watch` auto-selection — the PID set is chosen once, at the first sample, and never revisited; and `hmn ps` display ordering, which is fixed to dedicated-descending
**Status:** Proposed — v0.2.7 candidates

---

## TL;DR

`hmn watch --duration 80m --json` ran unattended alongside candle-mi's full oracle
resurrection: 19 sequential `cargo test` steps, **each a fresh short-lived process**, on a
desktop also running a browser, VS Code, and a Qt renderer. The adapter-level machinery was
flawless: **three spill episodes detected, all three real, one of them a finding candle-mi's
own tooling had missed** (Mistral-7B at `F32` briefly commits ~31 GB against a 16 GiB card).
Exit code `1` made the verdict scriptable, exactly as designed.

But the per-PID half of the instrument answered the wrong question. Auto-selection froze on
the five processes holding VRAM at `t=0` — compositor, browser, editors — and **not one of the
nineteen processes that actually caused the spills ever appeared in a row**. The workload's
protagonists were all born *after* the first sample. `watch` could say **that** the adapter
spilled, but not **who** — which is the question `watch` exists to answer (see the v0.2.5
report's triage lesson: tenant-driven vs. workload-driven is *the* discriminating diagnosis).

Request for v0.2.7: **`--follow-new`** — re-run selection each interval so PIDs that enter the
top-N are picked up (baseline = first sighting), and departed PIDs are finalized instead of
rendering as `0` forever.

## The workload — a shape `watch` will meet often

`resurrect.ps1` is a verification suite: 19 oracle steps, each `cargo test … --release` in a
**fresh process** that loads a model (0.2 B–7 B, `F32`), runs forwards on CPU and GPU, and
exits. Total 44m36s. This "successive short-lived GPU processes" shape is not exotic — it is
every CI lane, every benchmark suite, every multi-model validation script. The watch was
started seconds *before* the suite, `--top 5` (default), so selection saw only the desktop:

```
QmlRenderer.exe (2.0 GiB), <unresolved PID 9916> (1.1 GiB), firefox.exe (643 MiB),
claude.exe (272 MiB), Code.exe (264 MiB)
```

## What worked — the adapter-level SpillReport (verbatim)

```json
"measurable": true, "spilled": true, "observations": 959,
"baseline_shared_bytes": 170295296, "peak_shared_bytes": 14694887424,
"peak_dedicated_bytes": 16785399808, "dedicated_limit_bytes": 16831741952,
"total_spill_duration_ms": 950849,
"episodes": [
  {"start_label":"+475.7s","end_label":"+480.7s","peak_shared_bytes":1881935872,"observations":1,"duration_ms":0},
  {"start_label":"+530.8s","end_label":"+555.8s","peak_shared_bytes":14694887424,"observations":5,"duration_ms":20030},
  {"start_label":"+946.4s","end_label":"+1882.2s","peak_shared_bytes":6952767488,"observations":187,"duration_ms":930818}
]
```

Cross-checked against the suite's own per-step wall-clock log (`resurrect.ps1` records one
per step, and this run stamped all 19):

- **Episode 3 is the known positive control.** candle-mi's `longrope` step (Phi-3.5-mini,
  `F32`, exceeds 16 GiB) ran 17m01s and was flagged `⚠️ slow (VRAM spill?)` by resurrect's
  own crude wall-clock heuristic. The watch saw a **930.8 s** continuous episode; the test
  binary itself reported `finished in 944.70s`. Two independent instruments, same window,
  ~1.5 % apart — and `watch`'s version comes with the mechanism (shared-resident growth under
  dedicated saturation, peak 6.95 GB) rather than an inference from slowness.
- **Episode 2 is a real discovery.** At +530→556 s the suite was in its Mistral-7B GPU
  forward. Peak: dedicated **16.79 GB of the 16.83 GB limit** plus **14.69 GB shared** —
  ~31 GB committed, which is exactly Mistral-7B at `F32` (~28 GB weights + activations +
  desktop tenants). candle-mi never knew this step spills: it passes parity in 85.3 s, far
  under the `-SpillWarnSeconds 300` heuristic, so nothing ever flagged it. The wall-clock
  heuristic sees only *slow* spills; `watch` saw a *fast, massive* one. This is the
  instrument earning its keep.
- Episode 1 (+475.7 s, single observation, 1.88 GB) sits at the Phi-3-mini → Mistral
  boundary; a load-time flicker, plausibly the first shard landing while the previous step's
  memory drains. Below any actionable threshold, and correctly reported as a separate
  blink-length episode rather than merged into episode 2.

Exit code `1` (spill observed) surfaced through the calling harness as designed —
scripts/watchdogs get the verdict for free.

## What was missing — the frozen PID set

The per-PID summary, verbatim:

```json
"per_pid": [
  {"pid":22372,"name":"QmlRenderer.exe","baseline_used_bytes":2020679680,"peak_used_bytes":3316617216,...},
  {"pid":9916,"name":null,"baseline_used_bytes":1131868160,"peak_used_bytes":1131872256,...},
  {"pid":12944,"name":"firefox.exe","baseline_used_bytes":643284992,"peak_used_bytes":15702519808,...},
  {"pid":19544,"name":"claude.exe","baseline_used_bytes":272826368,"peak_used_bytes":387244032,...},
  {"pid":7596,"name":"Code.exe","baseline_used_bytes":264695808,"peak_used_bytes":369238016,...}
]
```

Nineteen `cargo test` processes came and went — including the two that committed ~31 GB and
~23 GB against a 16 GiB card — and the table contains a compositor, a browser, two editors,
and an unresolved PID. The v0.2.5 triage lesson ("the same symptom splits into three
diagnoses, and per-PID decomposition is what discriminates them") is exactly the capability
that went dark here: had this been a real investigation, "tenant-driven or workload-driven?"
would have been unanswerable from the per-PID table, while the adapter row shouted SPILL for
16 minutes.

Documented behavior, to be fair: *"auto-selects … from the first sample and keeps that fixed
set for the run."* The docs also say a watched PID that exits "renders as 0 bytes each
interval." Both held. The gap is that the documented semantics don't fit the
successive-process workload shape at all.

**One oddity worth a look:** the `firefox.exe` row peaks at **15.70 GB** `used_bytes` from a
643 MiB baseline. Firefox committing 15.7 GB dedicated is implausible on this desktop; the
timing coincides with the big-model steps. Two candidate explanations: (a) PID 12944 was
recycled onto a `cargo test` process and the best-effort name-change reset didn't fire
(resolution raced the short-lived process?), in which case one resurrect step *was* briefly
watched under a false name; or (b) a WDDM commit-accounting interaction under adapter
pressure. Either way it's per-PID attribution being unreliable precisely when the adapter is
saturated — worth a targeted probe.

## The v0.2.7 request: `--follow-new`

- **`--follow-new`** (auto-select mode only): re-run the top-`N` selection at each sampling
  interval. A PID entering the set starts with `baseline = first sighting` and a fresh delta
  history; a PID leaving the set (exited or dropped below top-N) is *finalized* into the
  summary's `per_pid` array with its peak/baseline, instead of rendering `0` rows forever.
  The summary then reads as a roster of *everyone who mattered during the watch* — for this
  run, ~24 entries instead of 5, with the nineteen `cargo test` rows carrying the story.
- A **name filter** (`--match "cargo*"` or similar) would compose well: follow new PIDs, but
  only those matching, so a CI watchdog can ignore the desktop entirely.
- Keep the current frozen-set behavior as the default or behind the explicit-PID form —
  explicit PIDs given on the command line should stay exactly as they are.

## A second v0.2.7 request: `hmn ps --sort` (added 2026-08-01)

Different session, same suite, different question. The task was mundane: free enough VRAM
to run candle-mi's `longrope` oracle, which loads Phi-3.5-mini at F32. The arithmetic is
unforgiving — 3.82 B parameters means **14,572 MiB of weights** against a card reporting
**16,311 MiB total / 16,052 usable**, so under 1.5 GiB is left for activations, and in
practice WDDM pages the shortfall. `hmn ps` was the natural tool for "what do I kill?".

It answered well, and then stopped short: the display order is fixed. `src/bin/hmn.rs:413`
sorts by `used_bytes` descending (name ascending, then PID, as tie-breaks). There is no way
to reorder by the SHARED column, even though **the data is already there** — `ps --json`
has emitted `shared_used_bytes` per row since spill landed. This is a display gap, not a
measurement gap, which is what makes it cheap.

**The semantic point worth building into the flag.** Sorting by SHARED answers a *different*
question, and conflating the two would make the feature actively misleading:

- **dedicated** descending (today's default) — "who do I kill to free VRAM?" This is the
  common case and the current default is right for it. The reasoning in the comment at
  `hmn.rs:413` still holds and should survive unchanged.
- **shared** descending — "who is *being paged out*?" That is a symptom, not a cause. A
  process high in SHARED has already lost the fight for dedicated VRAM; killing it frees
  system memory, not much VRAM.
- **total** (dedicated + shared) — "who is the biggest GPU-memory citizen overall?" For the
  original question this is arguably the best key of the three: a process at 1 GiB dedicated
  plus 2 GiB shared outweighs one at 1.5 GiB dedicated.

So the request is **not** a boolean `--sort-shared`. There are at least three useful
orderings and a bool cannot grow into them:

```
--sort <KEY>    dedicated (default) | shared | total | pid | name
```

Keep the existing tie-breaks exactly (name ascending clusters duplicate-name processes like
`msedgewebview2.exe`; PID ascending keeps output stable across runs); only the primary key
becomes selectable. Keep `dedicated` as the default.

Three implementation notes:

- **`select_top_n_pids` (`hmn.rs:978`) carries the same hardcoded dedicated-only sort** and
  feeds `watch --top`. If `ps` gains a sort policy, that function should share the
  comparator or document why it deliberately does not. Extracting one comparator keeps them
  from drifting, and since that function is already treated as pure and unit-testable
  without FFI, a comparator would test the same way.
- **`shared` is always 0 on Linux and macOS** (no shared-residency counter exists), so
  `--sort shared` is a silent no-op there. Given how carefully this crate documents
  per-platform semantics elsewhere, the help should say so outright rather than let a Linux
  user conclude the flag is broken.
- **Sorting by SHARED systematically surfaces the unnamed `?` rows.** It did here: the top
  row by shared was a nameless PID. That makes the existing "run elevated to resolve names"
  guidance far more load-bearing in this view than in the default one — possibly worth a
  footer when `--sort shared` is used and any `?` appears in the top N.

**What works today**, and why this is a convenience rather than a blocker:

```powershell
hmn ps --json | ConvertFrom-Json |
  Sort-Object shared_used_bytes -Descending |
  Select-Object -First 8 pid, name, used_bytes, shared_used_bytes | Format-Table
```

Effort looks like a `--sort` arg, a `match` producing the comparator, help text, and two or
three unit tests. Call it 30 to 40 lines.

## How to reproduce

```powershell
# terminal 1 — start the watch first, so selection predates the workload
hmn watch --duration 80m --json > resurrect_watch.jsonl

# terminal 2 — any successive-process GPU suite; candle-mi's oracle tier is one
$env:OTHELLO_MDLM_FIXTURES = "<fixtures>"; ./scripts/resurrect.ps1
```

Then compare `episodes[]` in the closing summary against the suite's per-step wall-clocks
(`RESURRECTION.md` timing column), and observe `per_pid[]` contains only pre-existing desktop
processes.

## What hypomnesis gets out of it

- Adapter-level spill semantics now field-validated on a *second* workload shape (short-lived
  process trains, not one long trainer) — including catching a fast 20-second episode no
  wall-clock heuristic can see.
- `--follow-new` turns `watch` from "attach to something I already suspect" into "stand
  guard over a machine while arbitrary work happens" — the CI/verification-suite use case,
  which is likely more common than the overnight-trainer one that motivated v0.2.6.
- `ps --sort` is smaller, but it closes the loop between the two tools: `spill` and `watch`
  tell you *that* the machine is over-subscribed, and `ps --sort total` tells you what to do
  about it. Right now the last step is the one a user has to improvise.

## Unrelated, noted in passing: hypomnesis should join the org

candle-mi has moved to the **`mi-for-the-rust-of-us`** GitHub organization, joining
`anamnesis` (which moved first, because CodSpeed wallclock benchmarking requires an org).
`hypomnesis` and `hf-fetch-model` are the two still outside it, and hypomnesis is now
load-bearing for candle-mi's oracle suite — `scripts/resurrect.ps1` shells out to
`hmn spill --json` to record measured WDDM spill per entry — so the ecosystem grouping has
stopped being cosmetic.

Two findings from candle-mi's own transfer, both verified rather than assumed, that make the
move cheaper than it looks:

- **Repository secrets survive a transfer.** `CARGO_REGISTRY_TOKEN` came through with its
  original `created_at` intact (a delete-and-re-add would have reset it), and the crates.io
  Publish workflow ran green from the new location afterwards. Nothing needs rotating; a
  crates.io token is a crates.io credential and does not care where the repo lives.
- **Old URLs keep working**, so existing clones, links and `git remote`s do not break. The
  one caveat is that a redirect dies if a repo of the same name is later created under the
  old owner, so do not leave placeholder repos behind.

The one thing that does *not* update automatically is the `repository`/`homepage` metadata
on crates.io, which only refreshes on publish. So the tidiest sequencing is to transfer
first and let the next release carry the corrected links — which is what candle-mi is doing
for v0.1.21, and what v0.2.7 could do here.
