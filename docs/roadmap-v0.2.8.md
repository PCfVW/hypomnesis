# `hypomnesis` v0.2.8 — roadmap

> *The tool you install should install. The name you can't be shown, someone else can.*

**Status: shipped 2026-08-04.**

---

## Why v0.2.8 (and not v0.3.0)

Three of the four changes are purely additive: the `Toolhelp32Snapshot` name
fallback and its `[exited]`/`[protected]` brackets only affect rows that
previously rendered an anonymous `?`; the `--sort vram`/`committed` aliases
add accepted values without removing any. The one behavior change to the
install experience itself is `cli` flipping from a default-off to a
default-on feature — `cargo install hypomnesis` now installs the `hmn`
binary without `--features cli`, and library-only consumers who don't want
`clap`/`ctrlc` now need `--no-default-features` where they previously got
the same result for free. No public library types changed shape;
`GpuProcessEntry` is already `#[non_exhaustive]` and its `name` field's
*possible values* changed (Windows only), not its type.

## Origin — a dogfooding report with one wrong diagnosis, corrected before implementation

[`docs/dogfooding-feedbacks/dogfooding-install-no-binary-and-protected-names.md`](dogfooding-feedbacks/dogfooding-install-no-binary-and-protected-names.md)
(2026-08-03, askesis `canvas`) reported that `cargo install hypomnesis`
completed with exit `0` and installed no binary at all — `cli` was
default-off — during a rented RTX 5090 deploy, so a multi-hour training job
ran with no per-PID VRAM census. Reproduced live before implementation:
`cargo install --path .` with default features compiled clean, printed
`warning: none of the package's binaries are available for install using
the selected features`, and exited `0`.

The report's second finding — that most Windows `?` rows in `hmn ps`/`hmn
watch` are nameable without elevation — was correct in its conclusion but
wrong in its diagnosis. It claimed `name_from_pid_windows` used the full
`PROCESS_QUERY_INFORMATION` right and proposed switching to
`PROCESS_QUERY_LIMITED_INFORMATION` as the fix. Checked against the actual
source: `src/gpu/pdh.rs` has used the limited right since it was introduced
in v0.2.2 (`git log -p` shows no change to that line since). A direct
P/Invoke test against `dwm.exe`/`csrss.exe` from this machine's non-elevated
shell showed **both** rights fail identically with `ERROR_ACCESS_DENIED` —
so the report's proposed fix was already shipping and already insufficient.
The actual mechanism that resolves those names non-elevated — confirmed
live, `Get-Process -Id <dwm's pid>` names it instantly from the same
shell — is a system-wide process-enumeration snapshot
(`CreateToolhelp32Snapshot`), which reads every process's short executable
name without opening a per-process handle at all, so it isn't subject to
the same access check `OpenProcess` is. The report's own "route 1"
(`CreateToolhelp32Snapshot`/`Process32FirstW`/`NextW`) was the real fix; its
"route 2" (the `PROCESS_QUERY_LIMITED_INFORMATION` switch) was redundant
with already-shipped code and was dropped from the implementation.

The report's other two asks — `[exited]`/`[protected]` bracket rendering
instead of an anonymous `?`, and `hmn ps --sort vram`/`committed` as
aliases for `--sort dedicated` — were well-scoped and shipped as proposed.

## Design decisions

- **`[exited]`/`[protected]` bracket rendering is Windows-only**, confirmed
  with the user before implementation. Linux's unresolved `?` today is
  already a genuine `sudo`-requiring permission wall (`/proc/<pid>/comm`
  unreadable for another user), not a false one the way Windows'
  `OpenProcess`-only path was — there is no equivalent snapshot-based
  collapse opportunity there. macOS would require a larger architecture
  change: cross-user PIDs are currently filtered out entirely *before*
  reaching name resolution (`read_graphics_footprint`'s `EPERM` skips the
  row), so introducing `[protected]` there would mean making previously
  invisible rows visible — a bigger behavior change than either platform's
  half of this report asked for.
- **The `Toolhelp32Snapshot` fallback is batched once per `gpu_processes()`
  call, not once per unresolved PID**, also confirmed with the user before
  implementation. `gpu_processes()` already builds the full row list before
  returning; a new `pdh::resolve_names_via_snapshot(&unresolved_pids)` takes
  exactly one snapshot per call — zero extra cost when every row resolves
  via the fast `OpenProcess` path (the common desktop case), bounded cost
  otherwise. Matters most for `hmn watch`, which calls `gpu_processes()`
  every interval tick; a per-PID snapshot would repeat a full
  system-wide process-table walk once per unresolved row per tick.
- **`resolve_names_via_snapshot` returns `Option<Vec<(u32, String)>>`, not a
  bare `Vec`.** An empty-but-successful snapshot (every requested PID had
  already exited) must be distinguishable from a snapshot that could not be
  taken at all (`CreateToolhelp32Snapshot` itself failing) — the former
  renders as `[exited]` per PID, the latter as `[protected]` for every
  requested PID, and a bare `Vec` can't tell the two apart when the result
  is empty either way.
- **`HandleGuard` was generalized, not forked.** Both `OpenProcess` and
  `CreateToolhelp32Snapshot` return a `Win32` `HANDLE` closed the same way
  via `CloseHandle`; the existing RAII guard's doc comment was widened from
  "obtained from `OpenProcess`" to "obtained from `OpenProcess` or
  `CreateToolhelp32Snapshot`" rather than duplicating a near-identical type.
- **A correctness gap in existing `hmn watch` logic, found and fixed while
  wiring the brackets in, not requested by the report.** `process_sample`'s
  OS-PID-reuse detector compares a watched PID's resolved name between
  samples and resets its baseline on a change; before this release it only
  guarded against `None`. Once `[protected]`/`[exited]` could appear as
  `Some` values, a transient snapshot failure on a single interval (name
  flips `real name → [protected] → real name`) would have been
  misread as "the OS recycled this PID" and spuriously reset the baseline —
  and, worse, once `[protected]` overwrote the sticky `last_name` state, a
  *genuine* subsequent PID reuse could have gone undetected. Fixed with a
  `resolved_name()` filter (excludes `None`, `[protected]`, `[exited]`;
  keeps `[kernel]`, which is permanently stable) used both for the
  comparison and for `last_name`'s sticky update. The one-shot "unresolved
  PID grew" hint was extended the same way: it now fires for `[protected]`
  (still genuinely unresolved) but not `[exited]` (a confirmed-gone process
  cannot meaningfully "grow", and elevation cannot identify it).
- **The `format_ps_summary` protected count now checks for the literal
  `"[protected]"` string in addition to `name.is_none()`**, and explicitly
  excludes `"[exited]"` — directly fixing the report's own critique that
  the pre-v0.2.8 count "counts everything unresolved and so overstates what
  elevation would buy."

## CLI surface (as shipped)

```sh
cargo install hypomnesis                  # cli is now default-on
hmn ps --sort vram                        # alias for --sort dedicated
hmn ps --sort committed                   # same alias, watch's own vocabulary
```

```
PID    NAME                         VRAM      SHARED  DEVICE
20404  QmlRenderer.exe              1.0 GiB   49 MiB  NVIDIA GeForce RTX 5060 Ti
26940  dwm.exe                      1001 MiB  4 MiB   NVIDIA GeForce RTX 5060 Ti
...
18880  csrss.exe                    39 MiB    63 MiB  NVIDIA GeForce RTX 5060 Ti
...
4      [kernel]                     4 MiB     0 MiB   NVIDIA GeForce RTX 5060 Ti
...
hmn: 24 GPU processes found (3.5 GiB committed total).
```

*(Real output, reference RTX 5060 Ti, non-elevated shell. PID 26940 (`dwm`)
and PID 18880 (`csrss`) both rendered `?` before this release — reproduced
live in this same session, on this same machine, prior to implementation.
No protected-count parenthetical because nothing in this sample is
unresolved.)*

## Design discipline (deliberate non-features)

- **No `[exited]`/`[protected]` on Linux/macOS.** See "Design decisions"
  above — confirmed with the user as an explicit scope decision, not an
  oversight. A future dogfooding report surfacing an equivalent gap on
  those platforms would be the trigger to revisit, matching this project's
  "un-gate on a real ask" discipline.
- **No new public API.** The bracket values are plain strings inside the
  existing `name: Option<String>` field — the same convention `[kernel]`
  already established — rather than a new enum threaded through
  `GpuProcessEntry`, `PsRow`, `WatchSampleRow`, and every formatter that
  touches `name`. Consistent with the existing precedent and avoids a
  breaking-shaped change for a Windows-only display refinement.
- **No `errno`-based exited-vs-denied distinction added to Linux/macOS's
  existing name lookups**, even though this release added exactly that
  distinction on Windows. Out of scope per the above; the underlying
  `read_proc_comm`/`read_proc_pidpath_basename` collapse-to-`None` behavior
  is unchanged.

---

## Implementation notes (as shipped)

### Correcting the report's diagnosis before writing any code

Before implementation began, the report's core technical claim (`hmn.rs`
uses `PROCESS_QUERY_INFORMATION`) was checked against `src/gpu/pdh.rs` and
found false — the limited right has been in place since v0.2.2. A live
P/Invoke test (`OpenProcess` called directly from a small PowerShell
harness against PID 26940/`dwm.exe` and PID 18880/`csrss.exe`, both rights
requested in turn) confirmed `ERROR_ACCESS_DENIED` for both, ruling out the
report's proposed fix before any implementation time was spent on it. This
reframed the report's own "two lighter routes" into one real fix
(`CreateToolhelp32Snapshot`) and one already-shipped no-op
(`PROCESS_QUERY_LIMITED_INFORMATION`).

### Windows `windows`-crate surface

The `windows` crate feature gating `CreateToolhelp32Snapshot` /
`PROCESSENTRY32W` / `Process32FirstW` / `Process32NextW` —
`Win32_System_Diagnostics_ToolHelp` — is not referenced anywhere in this
repository's history; it was confirmed by extracting the vendored
`windows-0.62.2` crate archive from the local Cargo registry cache and
reading the generated bindings directly (`CreateToolhelp32Snapshot` returns
`windows_core::Result<HANDLE>`; `Process32FirstW`/`Process32NextW` return
`windows_core::Result<()>`; `PROCESSENTRY32W` derives `Default` via
`core::mem::zeroed()`). `cargo build --all-features` compiled clean on the
first attempt after adding the feature, confirming the name.

### Consistency pass (pre-review)

A dedicated pass over the diff against `CONVENTIONS.md`, plus an
adversarial-correctness read, run after initial implementation and before
this doc was finalized — following the same discipline v0.2.6/v0.2.7 used.

- **Doc wording**: `GpuProcessEntry.name`'s field doc originally described
  a real resolved name as one of "four synthetic values" alongside
  `[kernel]`/`[exited]`/`[protected]` — a real name isn't synthetic;
  reworded to "one of four outcomes."
- **A pre-existing gap found and fixed, adjacent to but not caused by this
  release's changes**: `format_ps_summary`'s protected count never counted
  the pre-`WDDM 2.0` `nvidia-smi` fallback path's literal `Some("?")` name
  (`src/gpu/nvidia_smi.rs` writes this when `nvidia-smi` itself couldn't
  identify a row) — only `name.is_none()`, both before and after this
  release's `[protected]` addition. Same "might resolve under elevation"
  meaning as the other two cases, silently excluded since the check was
  first written (`src/bin/hmn.rs`, Wave C of v0.2.2). Fixed by extending
  the same filter this release already touched; a new test
  (`format_ps_summary_nvidia_smi_question_mark_counts_as_protected`) pins
  it. Low-risk, directly adjacent to code already under review, and closes
  a real (if narrow — pre-`WDDM 2.0` is rare in 2026) undercount of the
  exact metric this release exists to make honest.
- **`resolve_names_via_snapshot`'s `.contains()` linear scan** over `pids`
  (checked, not changed): acceptable — `pids` is bounded by a single
  device's GPU-process-table size (tens, not thousands), matching the
  complexity budget every other small-`N` scan in this module already
  uses without comment (e.g. the sibling `resolved.iter().find(...)` scan
  in `resolve_unresolved_windows_names`).
- **`"[protected]"`/`"[exited]"` as repeated string literals** (checked,
  not changed) across `src/gpu/mod.rs` and `src/bin/hmn.rs`, rather than a
  shared named constant the way `pdh.rs`'s private `KERNEL_PROCESS_NAME`
  centralizes `"[kernel]"`: confirmed consistent with existing practice,
  not a regression — `KERNEL_PROCESS_NAME` is itself private to `pdh.rs`
  and not shared across the `hypomnesis`/`hmn` crate boundary either, and
  `pdh.rs`'s own test module already hardcodes the literal `"[kernel]"`
  rather than referencing the constant. A cross-crate shared-constant
  refactor for all three bracket values would be a reasonable future
  cleanup but is out of scope for a Windows-only display fix.
- **`BORROW` vs. `INDEX` annotation choice** (checked, not changed): the
  new `szexefile_to_string` uses `// INDEX:` for its `&buf[..len]` slice,
  matching `CONVENTIONS.md`'s literal trigger-table text for `slice[a..b]`.
  The adjacent pre-existing `name_from_pid_windows` uses `// BORROW:` for
  an equivalent `&buf[..written]` pattern — an existing, pre-v0.2.8
  inconsistency, not one introduced by this release; left as-is since
  fixing it means touching unrelated, already-shipped code.
- **Full local verification gate re-run after every fix above** (`cargo
  build/test/clippy -D warnings/fmt --check/doc --all-features`, plus the
  `--no-default-features --features "cli,nvml,dxgi,pdh,nvidia-smi-fallback"
  --no-run` `test-helpers`-off compile check the v0.2.7 pass also ran):
  all green, 128 `hmn` unit tests (was 127 before the `nvidia-smi "?"` fix).

### Live validation

All on the reference RTX 5060 Ti, this machine, non-elevated shell:

1. **The install-path defect**, reproduced pre-fix: `cargo install --path
   .` with default features compiled, printed the "none of the package's
   binaries are available" warning, exited `0`, installed nothing.
2. **The `?`-row defect**, reproduced pre-fix: `hmn ps --sort dedicated`
   showed PID 26940 as `?` (the top VRAM holder in that sample, 731 MiB)
   and PID 18880 as `?`; a non-elevated `Get-Process -Id 26940`/`-Id 18880`
   named both (`dwm`, `csrss`) instantly from the same shell.
3. **Post-fix**, same machine: `hmn ps --sort dedicated` (built with
   `--features cli`) renders `dwm.exe` and `csrss.exe` correctly, zero `?`
   rows in the sample, no protected-count parenthetical.
4. **The install-path fix**, post-fix: `cargo install --path .` with
   default features (no `--features cli`) compiled, installed `hmn.exe`,
   and `hmn --version` printed `hmn 0.2.8` — closing the loop on the
   report's first finding end to end.
5. **`cargo test --features pdh --test live_pdh -- --ignored`**: all 7
   live `pdh` tests green, including the new
   `gpu_processes_never_leaves_a_row_unresolved`, which asserts no row's
   name is the literal `"?"` placeholder on this live sample.

### Verification (as run)

`cargo build --all-features`, `cargo test --all-features` (128 `hmn` +
89 lib + 8 smoke + 5 doctests, all green), `cargo clippy --all-features
--all-targets -- -D warnings` (clean), `cargo fmt --check` (clean), `cargo
doc --all-features --no-deps` (clean after fixing one broken intra-doc
link — a `[\`crate::gpu::gpu_processes\`]` reference inside `src/bin/hmn.rs`,
which is a separate binary crate and can't link into the library crate's
module tree; replaced with plain backtick text per `CONVENTIONS.md`'s
intra-doc-link-safety rule), re-run with `RUSTDOCFLAGS="-D warnings"` (same
flag the CI `doc` job uses — clean) — all run on this machine, `rustc`/
`cargo` 1.97.1, Windows only. **The full CI matrix (MSRV 1.88 + stable,
Windows + Ubuntu WSL2) has not yet been run** — this release stopped at
local implementation and verification per the user's explicit instruction,
ahead of the separate commit/dry-run-CI/push/publish flow.

A separate documentation consistency pass (markdown docs + the actual
rendered `cargo doc` HTML, not just "it built without warnings") was run
after the code consistency pass above — see the next section.

### Documentation consistency pass

Requested separately from the code pass above, and prompted by a direct
question — "did you check the Rust documentation?" — that exposed a real
gap: every prior `cargo doc` run had only been checked for build warnings,
never for `RUSTDOCFLAGS=-D warnings` parity with CI, and never by actually
reading the rendered HTML. Both were done this pass: the `-D warnings` run
above, plus spot-reading three representative generated pages
(`struct.GpuProcessEntry.html`'s `name` field doc, the crate-root
`index.html`'s two markdown tables, `enum.SortKey.html`'s `Dedicated`
variant doc) with HTML tags stripped to confirm actual rendered text —
code spans render as `<code>`, the `GPU-process listing` / `cli` table
rows both render as single well-formed `<tr>`s despite dense
parenthetical/backtick content, no literal unrendered markdown (`**`,
`` [` ``) leaked through.

Markdown-doc sweep across `README.md`, `docs/FAQ.md`, `CHANGELOG.md`,
`ROADMAP.md`, `docs/roadmap-v0.2.8.md`, and both tutorials found and fixed:

- **`README.md`'s "what's new" banner stack was never updated** — a
  mechanical per-release convention (confirmed via `git log -p`: each
  release prepends a new 🆕 banner, demotes the previous 🆕 to 🚀, and
  drops the oldest of the three shown) that this release's earlier doc
  pass missed entirely, leaving `0.2.7` marked 🆕 with no `0.2.8` banner at
  all. Added the `0.2.8` banner, demoted `0.2.7` to 🚀, dropped the `0.2.5`
  banner (oldest of the three).
- **A second-order consequence of the `nvidia-smi "?"` fix** (from the
  code consistency pass): README's Limitation 4 security-note sentence
  ("counts only true `[protected]`/`None` rows") became inaccurate once
  the protected count was extended to also catch the `nvidia-smi` `?`
  case — fixed to name that third case explicitly.
- Verified (not just assumed) that every `docs/FAQ.md#...` anchor
  referenced from `README.md` and both tutorials still resolves — none of
  this release's edits changed any FAQ heading text, only body content, so
  all anchors were confirmed unbroken rather than silently trusted.
- Confirmed the historical per-release entries in `CHANGELOG.md`,
  `ROADMAP.md`'s "Per-release detail" index, and older `docs/roadmap-vX.md`
  files were correctly left untouched, per this project's own "historical
  narrative... deliberately left as period record" principle — not an
  oversight, checked explicitly.

---

## References

- Dogfooding input: [`dogfooding-feedbacks/dogfooding-install-no-binary-and-protected-names.md`](dogfooding-feedbacks/dogfooding-install-no-binary-and-protected-names.md)
- Predecessor: [`docs/roadmap-v0.2.7.md`](roadmap-v0.2.7.md)
- New live test: [`tests/live_pdh.rs`](../tests/live_pdh.rs) (`gpu_processes_never_leaves_a_row_unresolved`)
