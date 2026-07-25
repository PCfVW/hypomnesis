# `hypomnesis` v0.2.6 — roadmap

> *Not a TUI. Same tracker, a timer instead of a wrapped child.*

**Status: ✅ shipped 2026-07-25.**

---

## Why v0.2.6 (and not v0.3.0)

Everything below is **additive and CLI-only**: `hmn watch` is a new
subcommand in `src/bin/hmn.rs`, plus one test-only `GpuProcessEntryBuilder`
addition (`src/snapshot.rs`, `test-helpers` feature). No change to
`src/spill.rs`, `src/gpu/pdh.rs`, `GpuProcessEntry`'s production fields, or
`SpillTracker`'s public API. No existing field changes type, no method
changes signature, no default feature flips. `format_spill_report` and
`format_spill_json` (the `hmn spill` formatters) keep byte-identical output
after being generalized to share code with `hmn watch`'s formatters — their
existing tests pass unchanged.

## Origin — a dogfooding report hitting the same gap three times

[`docs/dogfooding-feedbacks/dogfooding-spill-triage-watch-mode.md`](dogfooding-feedbacks/dogfooding-spill-triage-watch-mode.md)
(2026-07-25) field-validated v0.2.5's spill semantics across a ~15-hour,
3-seed `MDLM` training campaign — three verdicts, all correct:

1. **Evening — "saturated, not spilling."** Trainer committed 14.9 GiB
   (under the 16.3 GiB physical), SHARED drifted 142 → 184 MiB (benign
   staging-heap scale). Exactly the v0.2.5 acceptance semantics, now
   passing in production.
2. **Morning — "leak-driven spill," a real bug found, fixed, and
   verified.** SHARED jumped to 718 MiB against a 142–184 MiB baseline,
   resume legs ran 3× slower. Root cause: `torch.load(map_location=device)`
   parked 480 MB of resume state in `VRAM` for the whole run. Fixed
   (askesis commit `5779858`), and the fix was verified by the same
   instrument — SHARED back to 184 MiB, pace back to normal.
3. **Late morning — "tenant-driven spill," no action needed.** Trainer
   SHARED only 224 MiB, but *system-wide* committed exceeded physical
   `VRAM` because new desktop tenants (a browser, a chat client) pushed the
   adapter over budget. Self-resolved when the tenants closed.

The discriminator every time was the same per-process committed-vs-SHARED
decomposition v0.2.5 shipped. What cost real time was reaching it: **there
was no way to attach to a PID that was already running.** `hmn spill`
wraps a *new* command; the trainer was hours into its run each time. The
workaround, repeated three separate times: run `hmn ps` twice minutes
apart and diff the SHARED column by eye, with the pace tiebreaker coming
from checkpoint-file mtimes because the trainer's `stdout` was
block-buffered into uselessness. The report's concrete ask:
`hmn watch [PID ...] [--interval 30s] [--duration 10m]`.

**Why this isn't the previously-rejected `hmn watch`.** `ROADMAP.md`'s
"Carried forward" table had an `hmn watch (TUI live-refresh)` row, rejected
pending "a consumer who's tried `watch -n1 hmn` and explained why it's
insufficient." This report *is* that consumer — and what it asks for is
explicitly not a curses-style redraw dashboard: a `time(1)`-style scrolling
sampler with a SPILL flag, delta columns, and an exit code scripts can
branch on. Same "not a TUI" discipline `hmn spill` already carries.

## Design decisions

**Reuse the shipped v0.2.5 primitives unchanged.** `SpillTracker`
(adapter-scoped dedicated-saturation + shared-growth co-condition, episode
history) and `gpu_processes()` (per-PID `used_bytes` commit /
`shared_used_bytes` resident) already carry everything `watch` needs — no
new measurement source, only a timer loop and delta bookkeeping around
both.

- **PID selection.** `hmn watch [PID...]` watches explicit PIDs exactly as
  given. No PIDs given → auto-select the top `--top` (default 5) by
  committed `used_bytes` from the *first* sample, then keep that fixed set
  for the whole run — no re-selection mid-run, which would churn the
  table and complicate "per-PID deltas."
- **Time format.** `--interval` / `--duration` take duration strings
  (`"500ms"`, `"30s"`, `"5m"`, `"1h"`, bare integer = seconds) — matching
  the report's own sketch, and more ergonomic than `hmn spill`'s raw
  milliseconds for a tool meant to attach and run for minutes to hours.
  Hand-rolled parser (`parse_duration`), no new dependency.
- **JSON shape.** `--json` streams JSON Lines: one `"kind":"sample"`
  object per PID per interval as it happens, plus a closing
  `"kind":"summary"` object (`SpillReport` fields + `per_pid[]`). `hmn
  spill --json`'s single-blob-at-exit shape doesn't fit a
  potentially-unbounded, live-tailed command.
- **No per-PID spill semantics invented.** The SPILL flag is the
  *adapter-wide* `SpillTracker::is_spilling()` state at that interval,
  replicated on every row sharing the timestamp — spill is a device-level
  phenomenon in the shipped model, and re-deriving a per-PID threshold
  heuristic would be new, unvalidated semantics. Attribution ("which PID
  is the culprit") comes for free from that row's own SHARED delta
  column — visible directly, exactly like Verdict 2's diagnosis ("SHARED
  climbing every interval while dedicated saturates").
- **No process-liveness detection.** `gpu_processes()` cannot distinguish
  "PID exited" from "PID alive but currently holds no GPU memory on this
  device." A watched PID absent from a sample renders `0 B` / `0 B` —
  documented behavior, not an error, and `hmn watch` does not auto-stop on
  this basis. Keeps the feature free of a new `OpenProcess`-based liveness
  probe; `--duration` or Ctrl+C are the offered stop mechanisms.
- **Ctrl+C prints the same closing summary as a natural stop.** Unlike
  `hmn spill` (which deliberately stayed dependency-free because Ctrl+C
  reaches the whole process group including the wrapped child, and the
  report was judged not worth a new dependency for that case), `hmn
  watch` doesn't wrap a child — Ctrl+C only reaches `hmn` itself, and the
  interactive "attach and watch until you've seen enough" case is common
  enough to justify the small `ctrlc` crate (safe cross-platform API, no
  new `unsafe` code, `cli`-feature-gated so library users never pull it).

## CLI surface (as shipped)

```sh
hmn watch 21844                          # attach to a known PID
hmn watch                                # no PID: auto-select top 5 by committed VRAM
hmn watch --top 3 --interval 30s --duration 10m --json
```

```
hmn watch: device 0 [NVIDIA GeForce RTX 5060 Ti], interval 3.0s, watching 1 PID(s)
TIME      PID    NAME            COMMITTED  ΔCOMMIT   SHARED   ΔSHARED   SPILL
+0.0s     15884  spillforge.exe  9.3 GiB    +0 B      86 MiB   +0 B      no
...
+33.1s    15884  spillforge.exe  13.3 GiB   -4 MiB    1.3 GiB  +980 MiB  SPILL
hmn watch: peak dedicated 15.0 GiB / 15.7 GiB
           peak shared    1.4 GiB (baseline 228 MiB)
           episodes       1 — total 0.0s, longest 0.0s, first +33.1s into run
```

*(Real output — see "Live validation" below.)*

Exit code: `0` no spill observed, `1` spill observed at least once, `2` on
a hard error (bad `--device`, or nothing to auto-select). `--json` streams
one `{"kind":"sample",...}` line per PID per interval plus a closing
`{"kind":"summary",...}` line.

## Design discipline (deliberate non-features, carried from `hmn spill`)

- **Measurement-only.** No opinion on what to do about a spill — same
  "why no `hmn kill`" scope discipline as v0.2.3/v0.2.5.
- **No per-PID spill heuristic.** See "Design decisions" above.
- **No process-liveness probe.** See "Design decisions" above.
- **Documented sampling limitation.** Same as `SpillTracker` generally: a
  spill shorter than the polling interval is invisible at observation
  points, full stop.

---

## Implementation notes (as shipped)

### Consistency pass (pre-commit review)

Following the exact process v0.2.5 used, two parallel review agents
(`CONVENTIONS.md` compliance + adversarial correctness) audited the full
diff before commit. Findings, all resolved:

- **One compile-breaking gap**: a new `process_sample` test helper called
  the `test-helpers`-gated `GpuProcessEntry::builder()` without itself
  being `#[cfg(feature = "test-helpers")]`-gated — invisible under
  `--all-features` (what local gates ran) but broke
  `cargo build --no-default-features --features "cli,nvml,dxgi,pdh,nvidia-smi-fallback"`.
  Fixed; both feature combinations now verified to build.
- **Convention nits**: a couple of literal `--` where the file's
  established style uses an em dash (including one in the `Watch` clap
  variant's own doc comment, which clap surfaces as the short summary in
  `hmn --help`'s command table — caught only by diffing the *rendered*
  `--help` output, not just reading source); a missing `// BORROW:`
  annotation on a `.clone()` call; `--top` unbacked in the `Cli` long
  About text.
- **One correctness improvement, not just a nit**: the adversarial pass
  flagged that OS PID reuse mid-watch (a watched PID exits, the OS
  reassigns the number to an unrelated process) would silently mix two
  processes' committed/shared readings under one row, since nothing
  detected the identity discontinuity. Implemented a best-effort
  mitigation — a resolved-name change between samples resets that PID's
  baseline/peak/prev — with two new unit tests (reset-on-rename,
  no-reset-on-transient-`?`).

### Live validation

`tools/spillforge` (the v0.2.5 forced-spill fixture — unmodified) provided
every real number in this document. Three methodology notes worth keeping
for future re-validation:

- **Sequential tool calls introduce enough latency to miss a short churn
  window.** An early manual run launched `spillforge` and `hmn watch`
  from separate tool invocations; by the time `hmn watch` attached,
  `spillforge` had already finished its churn and exited, so every
  sample read `0 B`. Launching both back-to-back in one shell (or with a
  generous `HOLD_SECS`) avoids this.
- **A shell's own PID reporting can disagree with the real Win32 PID.** A
  run using Git Bash's `$!` job-control PID never matched `spillforge`
  in `gpu_processes()` (per-row `0 B`/`?` throughout), while the adapter
  tracker still correctly reported a real spill episode system-wide —
  consistent with an MSYS2/POSIX-emulation PID translation layer
  reporting a different number than the native Windows PID `PDH` sees.
  PowerShell's `Start-Process -PassThru`'s `.Id` matched correctly every
  time. Real users obtaining PIDs from `hmn ps`, Task Manager, or their
  own process's native PID are unaffected — this is a test-harness
  artifact, not an `hmn watch` bug.
- **Run-to-run spill magnitude varies** depending on exactly when the
  polling interval lands relative to `spillforge`'s upload/churn
  timing — expected given the flicker behavior v0.2.5 already documented,
  and not a regression.

Two runs, PowerShell-launched (correct native PID both times):

1. **Forced spill** (`spillforge 20 20`, attached at start, `--json`):
   `spilled: true`, one episode `+54.1s → +56.1s`, peak shared 3.18 GiB
   over a 459 MiB baseline, peak dedicated 15.2 GiB / 15.7 GiB (96.7%),
   exit code `1`.
2. **Forced spill, text mode** (`spillforge 20 30`, attached at start):
   one episode, peak shared 1.4 GiB over a 228 MiB baseline, peak
   dedicated 15.0 / 15.7 GiB, exit code `1` — the transcript reproduced
   above and in the README.
3. **Idle-desktop no-false-positive** (`hmn watch --top 3`, ordinary
   desktop load, no training run): zero episodes across the run,
   including a genuine negative delta (`QmlRenderer.exe`, `-40 MiB`) —
   confirms benign churn does not fire the condition, exit code `0`.

Plus the automated `tests/live_watch.rs` (`#[ignore]`-gated,
`windows + pdh + cli`): spawns `spillforge` as a child, attaches the
**compiled** `hmn` binary (`env!("CARGO_BIN_EXE_hmn")`) to its real PID,
and asserts `spilled: true`, at least one episode, and exit code `1` from
the actual process output — not a mock, not the library API called
directly. Green.

### Verification (as run)

`cargo test --all-features`: 86 lib + 93 `hmn` (up from 91 pre-review; +2
from the PID-reuse fix) + 8 smoke + 5 doctests, all green.
`cargo test --no-default-features --features "cli,nvml,dxgi,pdh,nvidia-smi-fallback" --bin hmn`
(post-fix) green — the feature-combination gap the review caught. `cargo
clippy --all-features --all-targets -- -D warnings` and `cargo fmt --check`
clean. `cargo doc --all-features --no-deps` with `RUSTDOCFLAGS=-D warnings`
clean (the new `GpuProcessEntryBuilder` doctest included).
`cargo test --features cli,pdh --test live_watch -- --ignored` green
(~36 s) on the reference `RTX 5060 Ti`.

---

## References

- Dogfooding input: [`dogfooding-feedbacks/dogfooding-spill-triage-watch-mode.md`](dogfooding-feedbacks/dogfooding-spill-triage-watch-mode.md)
- Predecessor / shipped semantics this reuses unchanged: [`docs/roadmap-v0.2.5.md`](roadmap-v0.2.5.md)
- New tutorial: [`docs/tutorials/watching-a-running-job.md`](tutorials/watching-a-running-job.md)
- New FAQ entry: [`docs/FAQ.md`](FAQ.md#hmn-spill-or-hmn-watch--which-do-i-use)
- Forced-spill fixture (unmodified): [`tools/spillforge`](../tools/spillforge/)
