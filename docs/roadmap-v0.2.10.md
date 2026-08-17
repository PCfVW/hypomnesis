# `hypomnesis` v0.2.10 — roadmap

> *Audited, not assumed.*

**Status: implemented and locally verified 2026-08-17; not yet published.**

---

## Why v0.2.10 (and not v0.3.0)

Every item in this release is patch-safe: three correctness/robustness
fixes with no public API change, three CI/release-process hardenings
invisible to library consumers, and a batch of documentation
corrections. Nothing adds, removes, or changes the signature of any
public item — `HypomnesisError::Io`'s doc comment is rewritten, not the
enum itself. Principle 2 (additive-by-default) is satisfied trivially
here: there is nothing to be additive *about*.

---

## Origin — a self-audit, read as a dogfooding report

Every prior v0.2.x release traces back to a named downstream consumer's
adoption experience — Principle 1. v0.2.10 traces back to the crate
auditing itself: a full read of every tracked source, test, workflow,
and doc file, cross-checked line-by-line against actual shipped
behavior, followed by a second adversarial pass (independent code-review
agents plus a manual documentation-consistency sweep) on the fixes it
produced. That is, in substance, exactly what Principle 1 asks for — a
real adoption experience surfacing a gap between what the docs promise
and what the code does — just conducted first-party instead of waiting
for the next external report. Full findings:
[`docs/audits/2026-08-17-codebase-documentation-audit.md`](audits/2026-08-17-codebase-documentation-audit.md).

Three of the gaps it found turned out to share one shape: **silence**.
`hmn ps` could silently drop rows past the first 64 on a busy device. A
single malformed `DXGI` adapter could silently truncate the rest of an
enumeration. A partially-broken driver install could make `hmn` print
nothing — or, in `--json` mode, emit an indistinguishable `[]` — as if
nothing were wrong. Each is now honest about what happened instead.

---

## Scope

### `src/gpu/nvml.rs` — the 64-process cap

`list_compute_processes` (the `hmn ps` / `gpu_processes()` enumeration
path) capped silently at 64 compute processes. `NVML`'s own documented
contract for `NVML_ERROR_INSUFFICIENT_SIZE` writes the *true* required
count back into the count out-parameter, so a single retry with a heap
buffer sized to that count (defensively capped at 65536 against a
corrupted report) removes the cap entirely. `read_process_used` (the
narrower single-calling-process lookup inside `query()`) is unaffected —
out of scope, a different failure shape (it only needs to find one
specific PID, not enumerate every row for display). The shared per-row
sentinel/sanity filtering was extracted into a new pure
`filter_process_rows` helper, gaining unit test coverage for the first
time.

### `src/gpu/dxgi.rs` — abort-on-one-bad-adapter

`enumerate_non_nvidia` and `device_count` both `break`-ed the whole
adapter walk if a single adapter's `IDXGIAdapter` cast or `GetDesc` call
failed, silently dropping every adapter after it. Only `EnumAdapters1`
failing is a genuine end-of-walk signal; the other two now skip that one
adapter and continue.

### `src/bin/hmn.rs` — the no-subcommand path could go silent

If `Snapshot::all()` enumerated devices but `device_info` failed for
every one of them (a partial driver install), text mode printed nothing
and `--json` printed a bare `[]` — both indistinguishable from a
genuinely empty or fully-working system. Both paths now say so: text
mode on stdout, `--json` on stderr (mirroring `hmn ps`'s always-on
stderr summary) without changing the documented `[]` JSON shape on
stdout.

### CI and release process

- `.github/workflows/ci.yml` gains a `macos-latest` leg
  (`× {1.88, stable}`) — macOS has been a first-class supported platform
  since v0.2.3 but had never actually compiled in CI.
- The same workflow now checks `--no-default-features` and the README's
  documented library-only feature set (`nvml,dxgi,pdh`) on every matrix
  leg, so those configurations can't silently drift from what's
  advertised.
- `.github/workflows/publish.yml` now verifies the pushed tag matches
  `Cargo.toml`'s version before running the release gates.

### Documentation

`hmn --help`'s `?`/elevation Limitations text and its module doc still
described pre-v0.2.8 behavior; `hypomnesis::gpu_processes`'s rustdoc
Limitations section still promised `name: None` reaches callers on the
Windows `PDH` path, contradicting the crate's own
`resolve_unresolved_windows_names`; `ROADMAP.md` and
`docs/roadmap-v0.2.8.md` still said v0.2.8 was "not yet published" after
it shipped; `CONVENTIONS.md`'s default-feature list and `cli`-feature
dependency row predated `pdh`/`metal`/`cli` joining the defaults and
`ctrlc` joining `cli`'s dependencies. All corrected to match shipped
reality.

---

## Verification

- `cargo fmt --check`, `cargo clippy --all-targets [--all-features] --
  -D warnings`, `cargo test --all-features`, `cargo doc --all-features
  --no-deps` under `RUSTDOCFLAGS="-D warnings"`, plus the two new
  `cargo check --no-default-features[ --features nvml,dxgi,pdh]`
  combinations — all run clean on **both** Windows and WSL2 Ubuntu. The
  Linux leg was essential, not optional: `list_compute_processes`,
  `filter_process_rows`, and its six new unit tests are all
  `target_os = "linux"`-gated and cannot be exercised on Windows at all.
- Two independent adversarial passes on the fixes themselves (an
  initial correctness review, then a second round of finder agents — a
  removed-behavior audit, a cross-file call-site tracer, a
  `CONVENTIONS.md` compliance check, and a cleanup/simplification scan)
  found and closed four further issues before this release: `--json`
  had the identical silent-`[]` ambiguity the text-mode fix addressed,
  which the first pass's own `CHANGELOG` entry incorrectly claimed
  didn't apply; the `NVML` retry path paid for a row-extraction copy
  even on hard-error returns; the debug-output trace lost visibility
  into NVML's reported-vs-captured count on retry; and a defensive
  constant's justification cited an unverified Linux `pid_max`
  assumption, corrected to the actually-solid "no real GPU hosts
  anywhere near this many concurrent compute processes" reasoning.
- `hmn --help`'s rendered output was read end-to-end post-fix to confirm
  the rewritten Limitations text reads correctly in context, not just
  in isolation.
- The real push to `origin/main` — the first time this exact diff ran on
  actual GitHub-hosted runners, not just local dry-runs — immediately
  caught a genuine `-D warnings` clippy failure on `macos-latest` (both
  toolchains): a three-release-old `dead_code` bug in
  `src/gpu/metal.rs` (`MetalQueryResult::current_usage` computed via a
  real syscall on every `device_info()` call, then never read). Fixed
  and re-verified via a real cross-compiled `cargo check --target
  aarch64-apple-darwin --all-features` (clean, zero warnings) before
  re-pushing, since no local macOS hardware was available. A separate,
  unrelated `windows-latest` job failure in the same run
  (`429`/`503` errors downloading the `dtolnay/rust-toolchain` and
  `Swatinem/rust-cache` actions from `codeload.github.com`) was
  confirmed to be transient GitHub infrastructure flakiness, not a code
  issue.

---

## What this protects going forward

The macOS CI leg closes a three-release-old blind spot: `src/gpu/metal.rs`
and `tests/macos_smoke.rs` have existed since v0.2.3 with zero CI
coverage — and, as it turned out, that gap was not hypothetical: the
leg's first real run caught a genuine bug within seconds of existing
(see Verification, above). The `publish.yml` tag guard closes a
different failure mode, one that hasn't happened yet — a mistyped tag
either failing confusingly or, worse, publishing a version whose tag
doesn't match its content. It didn't need to catch something live to be
worth adding; the macOS leg is proof the same reasoning already pays
off in practice, not just in theory.
