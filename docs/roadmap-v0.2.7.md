# `hypomnesis` v0.2.7 — roadmap

> *Follow the work, not just the machine. Sort by the question you're actually asking.*

**Status: ✅ shipped 2026-08-02.**

---

## Why v0.2.7 (and not v0.3.0)

Both changes below are **additive, CLI-only, and backward-compatible**: `hmn
watch --follow-new` is a new opt-in boolean flag (the pre-existing frozen-set
behavior stays the default); `hmn ps --sort` is a new opt-in flag defaulting
to the exact pre-v0.2.7 fixed order. No library-surface change at all — both
features live entirely in `src/bin/hmn.rs`. A third, unrelated piece rides
along: the repo transferred from `PCfVW/hypomnesis` to the
`mi-for-the-rust-of-us` GitHub org (joining `anamnesis` and `candle-mi`),
and this release carries the corrected `repository`/`homepage` metadata for
the next `cargo publish` — the transfer itself needed no code change, GitHub
redirects the old URLs, but crates.io's metadata only refreshes on publish.

## Origin — a second dogfooding report on the same feature, one release later

[`docs/dogfooding-feedbacks/dogfooding-watch-follow-new.md`](dogfooding-feedbacks/dogfooding-watch-follow-new.md)
(2026-07-27, extended 2026-08-01) ran `hmn watch --duration 80m --json`
alongside candle-mi's `scripts/resurrect.ps1` oracle suite — 19 sequential
`cargo test` steps, each a **fresh short-lived process**, over 44m36s. The
adapter-level machinery was flawless: three real spill episodes detected,
including a 20-second Mistral-7B `F32` spike (~31 GB committed against a
16 GiB card) that candle-mi's own wall-clock heuristic had never caught,
because it only sees *slow* spills and this one was fast and massive.

But the per-PID half answered the wrong question. `watch`'s auto-selected
PID set — five processes at `t=0`: the desktop compositor, a browser, two
editors — never changed, and **not one of the nineteen `cargo test`
processes that actually caused the spills ever appeared in a row**. They
were all born after attach. `watch` could say *that* the adapter spilled,
but not *who* — the exact question a v0.2.5-era dogfooding report already
established the per-PID decomposition exists to answer (tenant-driven vs.
workload-driven is *the* discriminating diagnosis). The report's concrete
ask: **`--follow-new`**, re-run top-N selection every interval instead of
once at attach.

A second, smaller request rode along, filed in a follow-up session against
the same suite: **`hmn ps --sort`**. `hmn ps` was the natural tool for "what
do I kill to free VRAM for `longrope`'s Phi-3.5-mini load," and answered
well, then stopped short — display order is hardcoded to dedicated-VRAM
descending, even though `--json` has emitted `shared_used_bytes` per row
since v0.2.5. The report's own semantic point: sorting by SHARED answers a
*different* question ("who's being paged out," a symptom) than sorting by
dedicated ("who do I kill," the cause), and a `total` key answers a third
("who's the biggest citizen overall") — three real orderings, not a boolean.

**Validated with the same rigor as the report that drove v0.2.6**: every
line-number citation (`hmn.rs:413`, `hmn.rs:978` in the pre-v0.2.7 file)
checked against the actual source and found exact; every JSON figure in the
report cross-checked against the real `fold()`/episode-duration semantics
(the subtlety that an episode's `duration_ms` excludes its trailing
non-spilling probe sample, so a 25.0s label span correctly reports as
~20.0s of actual spilling — reproducing this by accident without running
the real binary is essentially impossible); the report's own domain
arithmetic (`3.82 × 10⁹ × 4 bytes / MiB = 14,571.98 MiB`, `16,311 − 259 =
16,052`) matched prior release-validated reference-card numbers exactly.
One report hypothesis — a `firefox.exe` row implausibly peaking at 15.7 GB,
explained as "the best-effort PID-reuse reset didn't fire" — was traced to
a **confirmed** code-level gap: the reset only fires when *both* the old
and new sample have a resolved name, so a name-resolution race on a
short-lived recycled PID silently defeats it. Both requests were
well-scoped and technically sound; both shipped largely as proposed.

## Design decisions

**Reuse everything v0.2.5/v0.2.6 already shipped, unchanged.** Neither
feature touches `src/spill.rs`, `src/gpu/pdh.rs`, `SpillTracker`, or
`gpu_processes()`. `--follow-new` re-times an existing selection function;
`--sort` re-parameterizes an existing comparator.

- **`hmn ps --sort`: three keys, not five.** The report's prose only
  argues for `dedicated` (default) / `shared` / `total`; its example flag
  syntax listed `pid`/`name` too, but never justified them, and both are
  already trivial via the `jq`/`Sort-Object` one-liners the report itself
  demonstrates. Shipped with just the three argued-for keys.
- **A single shared comparator, `ps_row_comparator(SortKey) -> impl Fn(&PsRow, &PsRow) -> Ordering`**,
  used by both `run_ps` (user-selectable) and `select_top_n_pids` (always
  pinned to `Dedicated`) — exactly the report's ask ("share the
  comparator... so they don't drift"). Tie-breaks (name ascending, then
  PID ascending) are identical across all three keys, matching the
  report's explicit "keep the existing tie-breaks exactly."
  **Consequence, verified and now documented**: sharing the comparator
  means `select_top_n_pids`'s tie-break changed from PID-only to
  name-then-PID — an adversarial review confirmed this can select a
  *different* PID into `hmn watch`'s auto-selected top-N at an exact tie
  boundary, not just reorder display, and independently confirmed exact
  ties are a real (not hypothetical) occurrence by finding one live on the
  reference machine's own process list.
- **`--follow-new` is auto-select-mode only; explicit PIDs + `--follow-new`
  is a hard error (exit `2`)**, checked first, before any device query —
  matching the report's explicit "explicit PIDs should stay exactly as
  they are."
- **Departed PIDs are finalized, not dropped**: a new `WatchState` wraps
  the existing per-PID `HashMap` with a `seen_order: Vec<u32>` recording
  first-seen order (no PID ever duplicated — `WatchState::track` is the
  sole insertion point and only records order on genuine first sight).
  The closing `per_pid[]` walks `seen_order` — *everyone who mattered
  during the watch* — instead of the old fixed `watched` list.
- **Re-entry resumes, it doesn't reset.** A PID that drops out of the
  followed set (ranked below `--top`) and later re-enters keeps its
  existing baseline/peak/history — only a genuine resolved-name change
  (the existing, separate OS-PID-reuse detector, unmodified) resets a
  row. Verified by adversarial review: an OS process legitimately
  dipping below rank `--top` for one interval and recovering is common
  and must not spuriously reset; only real PID reuse should.
- **An empty first sample is not an error under `--follow-new`** (the
  whole point is to wait for work to appear — matches the report's own
  reproduction steps, "start the watch first, so selection predates the
  workload"); non-`--follow-new` auto-select keeps the v0.2.6 hard-error
  (nothing to lock onto, no way to recover).
- **A stderr breadcrumb** on every followed-set change (`entered
  pid=... (name); left pid=... (name)`) — not requested explicitly by the
  report but proposed and confirmed wanted in plan-mode discussion.
  Entered-PID names resolve from the current sample; left-PID names from
  `state`'s last-known reading, since a departed PID is by definition
  absent from the current sample. Purely cosmetic — doesn't touch the
  JSONL stream shape.

## CLI surface (as shipped)

```sh
hmn ps --sort total                       # biggest GPU-memory citizen overall
hmn ps --sort shared                      # who's currently being paged out
hmn watch --follow-new                    # stand guard over a machine while arbitrary work happens
hmn watch --follow-new --top 3 --json     # CI/verification-suite shape
```

```
hmn watch: device 0 [NVIDIA GeForce RTX 5060 Ti], interval 3.0s, following top 5 by committed (re-selected every interval), 5 initially
hmn watch: +3.0s followed set changed: entered pid=10640 (spillforge.exe); left pid=21716 (Code.exe)
hmn watch: +18.0s followed set changed: entered pid=13004 (SamsungMagician.exe); left pid=10640 (spillforge.exe)
hmn watch: +21.1s followed set changed: entered pid=29452 (spillforge.exe); left pid=13004 (SamsungMagician.exe)
hmn watch: +36.1s followed set changed: entered pid=13004 (SamsungMagician.exe); left pid=29452 (spillforge.exe)
hmn watch: peak dedicated 15.2 GiB / 15.7 GiB
           peak shared    551 MiB (baseline 155 MiB)
           episodes       2 — total 24.1s, longest 12.0s, first +3.0s into run
```

*(Real output — two sequential `spillforge` runs, both correctly tracked as
distinct entries, both correctly finalized into the closing `per_pid[]`
alongside five desktop processes. See "Live validation" below.)*

Exit code contract, JSON Lines streaming shape, and the `0`/`1`/`2` codes
are unchanged from v0.2.6 — `--follow-new` only changes *which* PIDs
appear in the stream and the closing summary, not the wire format.

## Design discipline (deliberate non-features)

- **No `--sort` on `hmn watch`.** The report explicitly separated the two
  concerns: `select_top_n_pids`'s selection criterion ("which PIDs to
  watch") stays fixed to `Dedicated`; only `hmn ps`'s *display* order is
  user-selectable. Changing what `--top` selects by would be a different,
  larger-scoped ask than what was requested.
  `select_top_n_pids` shares the comparator so the two can't drift, but
  its own selection key is not exposed as a flag.
- **No footer hint for `?` rows surfacing under `--sort shared`** — the
  report flagged this as "possibly worth" doing, explicitly optional;
  deferred, matching the "un-gate on a real ask" discipline elsewhere in
  this project.
- **No cap on `--follow-new`'s `per_pid[]` size.** A pathological
  CI runner spawning thousands of short processes could in principle
  produce a very large array; not capped for this release (YAGNI —
  the motivating report's own 44-minute, 19-process run produced ~24
  entries, nowhere near a real concern).

---

## Implementation notes (as shipped)

### Consistency passes (pre-review, both features)

Following the same two-agent (conventions-plus-adversarial-correctness)
process used for v0.2.6, run separately for each phase:

**`hmn ps --sort`**: a test that didn't actually discriminate the
tie-break behavior it was named for (both its PID and name orderings
happened to agree, so it would have passed against the *old* PID-only
rule too) — fixed with a fixture where the two criteria deliberately
disagree; an unnecessary `#[allow(clippy::exhaustive_enums)]` that
corresponded to no actual clippy warning (verified empirically: `Commands`,
the file's other private dispatch enum, carries none either, and removing
it from `SortKey` produced no new warning); a function that could be
`const fn` and wasn't (verified by direct compilation against MSRV 1.88,
not just reasoned about); five instances of a literal `--` where the
file's established convention is an em dash; one missed org-reference
(the v0.2.4 README banner's `candle-mi` link, inconsistent with the
"Used by" section's already-updated one 400 lines later).

**`hmn watch --follow-new`**: a test whose scenario name and comments
described a PID being genuinely excluded from the followed set and later
re-entering, but whose code never actually excluded it from either
`process_sample` call — rewritten with a real three-interval fixture that
asserts the excluded PID's state is untouched during the gap and its
delta on re-entry is computed against the pre-gap reading, not a value it
drifted to while unwatched; an unnecessary `.clone()` of the `seen_order`
roster, verified removable by direct compilation (ordinary disjoint-field
borrowing handles it); the `--follow-new` + explicit-PIDs hard-error
check hoisted above the `device_info` call so it truly fails fast before
any backend dispatch, not just before the two heaviest calls; a
message-prefix inconsistency between two sibling branches of the same
`if watched.is_empty()` block; five more instances of the same `--`-vs-`—`
mistake. The adversarial pass additionally *traced and confirmed correct*
(not just tested): `WatchState::track`'s `or_insert_with` genuinely only
pushes to `seen_order` on a vacant entry; the re-entry-resumes semantics;
that the adapter-level `SpillTracker` keeps sampling every interval even
when zero PIDs are currently followed; and — critically — that the new
live test (`tests/live_watch_follow_new.rs`) would actually *fail* under
a reverted `--follow-new` (the two sequential `spillforge` PIDs cannot,
by construction, ever appear in a *frozen* pre-attach selection), i.e.
it tests what it claims to.

### Live validation

**`hmn ps --sort`**: unit-tested only (comparator behavior is pure,
platform-independent code — the "shared always 0 on Linux/macOS" fact
lives in the already-validated `gpu_processes()` backends, not in the new
sort logic). `hmn ps --help`'s rendered output manually inspected for
correct em-dash rendering and consistent voice against the file's
existing flags.

**`hmn watch --follow-new`**, all on the reference `RTX 5060 Ti`, GPU
confirmed idle (~14 GiB free) before each run:

1. **Conflict/empty-selection checks** (no GPU load needed — these
   return before any device query): `hmn watch <pid> --follow-new` →
   exit `2`, correct message, before `device_info` is even called.
   `hmn watch --top 0` (no `--follow-new`) → exit `2` (unchanged v0.2.6
   behavior). `hmn watch --top 0 --follow-new --duration 3s` → **not**
   an error, prints the waiting message, runs the full duration,
   exit `0` — confirming the adapter-level tracker keeps observing
   (`episodes 0 — no spill observed`) even with zero followed PIDs.
2. **Two sequential forced-spill `spillforge` runs** (20 GiB target,
   ~12 s hold each) under `hmn watch --follow-new --json`: both
   processes correctly entered/left the followed set as distinct
   entries (breadcrumbs above), **two separate spill episodes** detected
   (`+3.0s→+18.0s`, `+21.1s→+36.1s`), closing `per_pid[]` contained
   **all seven** PIDs ever seen (five desktop + both `spillforge.exe`
   instances) in first-seen chronological order with real committed/
   shared figures (`pid 10640`: baseline 13.0 GiB → peak 13.3 GiB
   committed, 280 MiB → 371 MiB shared), exit code `1`. Reproduced twice:
   once manually (PowerShell), once via the new automated
   `#[ignore]`-gated `tests/live_watch_follow_new.rs`
   (`cargo test --features cli,pdh --test live_watch_follow_new --
   --ignored`, ~91 s, green).
3. **Idle-desktop no-false-positive with `--follow-new` on**: zero
   episodes, followed set stable (no spurious entered/left breadcrumbs),
   exit `0`.
4. The pre-existing single-process `tests/live_watch.rs` (no
   `--follow-new`) re-run and confirmed still green after the
   `WatchState` refactor.

### Verification (as run)

`cargo test --all-features`: 86 lib + 115 `hmn` (up from 102 pre-review;
+13 from `--follow-new`'s tests, net of the consistency-pass fixes) + 8
smoke + 5 doctests, all green on **both** Windows and Ubuntu WSL2, **both**
MSRV 1.88 and stable. `cargo clippy --all-targets -- -D warnings`
(defaults) and `cargo clippy --all-features --all-targets -- -D warnings`
both clean on the full matrix. `cargo fmt --check` clean. `cargo doc
--all-features --no-deps` with `RUSTDOCFLAGS=-D warnings` clean on both
toolchains. `cargo test --no-default-features --features
"cli,nvml,dxgi,pdh,nvidia-smi-fallback" --bin hmn --no-run` (the
`test-helpers`-off build the v0.2.6 review caught a real gap in) green.

---

## References

- Dogfooding input: [`dogfooding-feedbacks/dogfooding-watch-follow-new.md`](dogfooding-feedbacks/dogfooding-watch-follow-new.md)
- Predecessor / semantics reused unchanged: [`docs/roadmap-v0.2.6.md`](roadmap-v0.2.6.md)
- Forced-spill fixture (unmodified): [`tools/spillforge`](../tools/spillforge/)
- New live test: [`tests/live_watch_follow_new.rs`](../tests/live_watch_follow_new.rs)
