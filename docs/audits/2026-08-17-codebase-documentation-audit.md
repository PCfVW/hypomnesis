# hypomnesis — Codebase & Documentation Audit

**Date:** 2026-08-17
**Audited at:** commit `c7c5248` (v0.2.9, tagged and published), working tree clean
**Auditor:** Claude Code (full read of every tracked source, test, workflow, and doc file; local gate verification on the Windows reference machine)

---

## Scope and method

- **Read in full:** all 14 `src/` files (incl. the 3,618-line `src/bin/hmn.rs`), all 6 `tests/` files, both GitHub workflows, `Cargo.toml`, `CHANGELOG.md`, `ROADMAP.md`, `CONVENTIONS.md`, `README.md`, `docs/FAQ.md`, both tutorials, the per-release roadmaps, `examples/`, and `tools/spillforge/`.
- **Verified live (Windows 11, this machine):** `cargo fmt --check` ✅ · `cargo clippy --all-targets --all-features -- -D warnings` ✅ · `cargo test --all-features` ✅ (239 passed, 0 failed, 18 `#[ignore]`-gated live tests not run) · `cargo doc --all-features --no-deps` with `RUSTDOCFLAGS="-D warnings"` ✅ · `cargo check --no-default-features` ✅.
- **Not verified here:** the Ubuntu/WSL2 leg of the usual pre-push dry-run, macOS builds (no hardware in this session), and the `#[ignore]`-gated live-GPU tests.

**Overall assessment.** The codebase is in excellent shape: the five quality gates pass clean, `unsafe` is scoped/annotated/feature-gated exactly as `CONVENTIONS.md` demands, the pure cores (spill fold, PDH parsers, CLI formatters) are thoroughly unit-tested, and the documentation culture (measured-not-invented numbers, named limitations) is far above crates.io norms. The gaps found are concentrated in **documentation drift left behind by v0.2.8** and **CI coverage that has not kept up with the platform matrix**. Nothing found rises to a correctness bug in shipped behavior.

---

## P1 — User-facing documentation that contradicts shipped behavior

### 1.1 `hmn --help` still documents the pre-v0.2.8 `?`-row semantics

[`src/bin/hmn.rs:91-105`](../../src/bin/hmn.rs#L91-L105) — the `long_about` Limitations text still says:

> *"`?` in the NAME column on Windows means the calling user cannot resolve the process's name via `OpenProcess`. Most cases (system services, other-user processes like `dwm.exe`, `csrss.exe`) resolve when `hmn ps` is run as Administrator. … PPL-protected processes … would remain `?` even elevated …"*

This is the pre-v0.2.8 story. Since v0.2.8, `dwm.exe`/`csrss.exe` resolve **non-elevated** via the `Toolhelp32Snapshot` fallback, and what remains renders as `[exited]` / `[protected]` — the README, FAQ, and `GpuProcessEntry` rustdoc all say so, but the tool's own `--help` does not. This matters doubly because `ROADMAP.md`'s "Standalone CLI reference doc" entry explicitly leans on *"`hmn --help` is already comprehensive at runtime"* as the reason not to write one. The security-note paragraph in the same block ([`hmn.rs:99-105`](../../src/bin/hmn.rs#L99-L105)) has the same drift (`?`-centric wording, no brackets).

**Fix:** rewrite the two `?`-related bullets of `long_about` to the v0.2.8 vocabulary (snapshot fallback, `[exited]`/`[protected]`, `[kernel]`), mirroring README Limitation 4.

### 1.2 `hmn.rs` module docs still say `cli` is default-off

[`src/bin/hmn.rs:3-4`](../../src/bin/hmn.rs#L3-L4): *"built only when the **default-off** `cli` feature is enabled"* and [`src/bin/hmn.rs:35`](../../src/bin/hmn.rs#L35): *"Install with `cargo install hypomnesis --features cli`."* Both false since v0.2.8 — and the stale install line is precisely the defect class v0.2.8 existed to fix. (README, FAQ, tutorials, and `Cargo.toml` were all updated; this file's header was missed.)

### 1.3 `gpu_processes()` rustdoc Limitations predate v0.2.8 (rendered on docs.rs)

[`src/gpu/mod.rs:344-352`](../../src/gpu/mod.rs#L344-L352) — the *Limitations* section of the public dispatcher still says:

> *"**Windows process names may be `None` (`PDH` path)** … access-denied for cross-user or protected processes yields `name: None` (mirroring the Linux `/proc/<pid>/comm`-unreadable case)."*

This contradicts the same file's own `resolve_unresolved_windows_names` ([`mod.rs:481-508`](../../src/gpu/mod.rs#L481-L508)), which guarantees `None` never survives the Windows PDH path — every row ends as a real name, `[kernel]`, `[exited]`, or `[protected]`. The `GpuProcessEntry::name` field doc in `snapshot.rs` was correctly updated in v0.2.8; this second rendering of the same contract was not. Since this is the primary API entry point on docs.rs, a downstream consumer reading only `gpu_processes()` gets the wrong contract.

**Fix:** rewrite that Limitations paragraph to match `GpuProcessEntry::name`'s v0.2.8 wording (and mention `Some("?")` remains only on the `nvidia-smi` fallback path and on Linux/macOS as `None`).

---

## P2 — Process and infrastructure gaps

### 2.1 No macOS leg in CI

[`ci.yml:21`](../../.github/workflows/ci.yml#L21) — the matrix is `{ubuntu, windows} × {1.88, stable}`. macOS has been a first-class supported platform since v0.2.3, has a dedicated test file (`tests/macos_smoke.rs`, four non-`#[ignore]` tests), a dedicated backend (`src/gpu/metal.rs`, 736 lines of libSystem FFI), and a real daily user/contributor on Apple Silicon — yet no macOS code ever compiles in CI. A regression in `metal.rs` (or in the `objc2-metal 0.3` pin) would only surface on a contributor's machine. GitHub's `macos-latest` runners are Apple Silicon (arm64), so the non-`#[ignore]` smoke tests, clippy, and the doc build would all run meaningfully.

**Fix:** add `macos-latest` to the matrix (at minimum for `stable`; MSRV too if runner minutes allow). The two `#[ignore]`-gated Metal-device tests can stay gated.

### 2.2 `ROADMAP.md` contradicts itself about v0.2.8/v0.2.9 publication state

Three spots in one file, all contradicting its own "Current state" (v0.2.9 shipped 2026-08-12):

- [`ROADMAP.md:187`](../../ROADMAP.md#L187) — per-release index entry for v0.2.8: *"implemented and locally verified 2026-08-04, **not yet published**"*. It was published 2026-08-04 (tag `v0.2.8` exists; v0.2.9 shipped on top of it).
- [`ROADMAP.md:207`](../../ROADMAP.md#L207) — footer: *"Last revised 2026-08-04: **v0.2.8 implemented and locally verified, not yet published**"*. The footer was not revised for the v0.2.9 release even though "Current state" was.
- The **per-release index has no `docs/roadmap-v0.2.9.md` entry** — the file exists and is linked from "Current state", but the index (the section a reader scans) stops at v0.2.8.

Same class: [`docs/roadmap-v0.2.8.md:5`](../roadmap-v0.2.8.md#L5) — *"Status: implemented and locally verified 2026-08-04; not yet published."* — was never flipped after publication, and ROADMAP designates per-release roadmaps as *"the authoritative source"* for shipped detail.

**Fix:** flip the two "not yet published" statuses, add the v0.2.9 index row, and re-date the footer.

### 2.3 `CONVENTIONS.md` closes with a stale default-feature list

[`CONVENTIONS.md:402`](../../CONVENTIONS.md#L402): *"Default features (`nvml`, `nvidia-smi-fallback`, `dxgi`) cover the ecosystem's most common case."* The default set has been `nvml, nvidia-smi-fallback, dxgi, pdh, metal, cli` since v0.2.8 (pdh/metal since v0.2.2/v0.2.3). The backend table just above it ([`CONVENTIONS.md:375-386`](../../CONVENTIONS.md#L375-L386)) is current, which makes the closing paragraph's staleness more likely to mislead. (Same table: the `cli` row's "Adds dep" column lists `clap` but not `ctrlc`, added in v0.2.6.)

### 2.4 CI never builds the documented non-default feature combinations

CI runs clippy/tests on the default set and `--all-features` only. The README explicitly markets two other configurations — `--no-default-features` (RSS-only) and `default-features = false, features = ["nvml", "dxgi", "pdh"]` (library-only, no `clap`/`ctrlc`) — and neither is ever compiled in CI. `cargo check --no-default-features` passes today (verified in this audit), but nothing guards it; a `cfg` mistake in the heavily feature-gated dispatchers (`src/gpu/mod.rs` has 20+ `cfg` combinations) would ship silently. A single cheap `cargo check --no-default-features` step (and optionally `--no-default-features --features nvml,dxgi,pdh`) on both OSes would close this.

### 2.5 `publish.yml` doesn't verify the tag matches `Cargo.toml`

[`publish.yml:3-8`](../../.github/workflows/publish.yml#L3-L8) publishes on any `v*` tag with no check that the tag equals `package.version`. A mistyped tag (e.g. `v0.2.10` pushed while `Cargo.toml` still says `0.2.9`) would republish-fail confusingly or, worse, publish a version whose tag doesn't match its content. A one-line guard (`[ "v$(cargo pkgid | cut -d# -f2)" = "$GITHUB_REF_NAME" ]`) is standard practice for this workflow shape.

---

## P3 — Code-level robustness and minor documentation nits

### 3.1 DXGI enumeration walks abort on a single bad adapter

[`src/gpu/dxgi.rs:333-355`](../../src/gpu/dxgi.rs#L333-L355) (`enumerate_non_nvidia`) and [`dxgi.rs:428-452`](../../src/gpu/dxgi.rs#L428-L452) (`device_count`): a failed `cast::<IDXGIAdapter>()` or `GetDesc()` on adapter *N* executes `break`, silently dropping every adapter after *N* — `device_count` undercounts and `Snapshot::all` loses iGPUs. On real hardware these calls essentially never fail mid-walk, but `continue` (after `raw_idx += 1`) is strictly more robust and costs nothing. Given the project's "No silent caps" instincts, a `debug-output` trace on the skip would fit the house style.

### 3.2 `HypomnesisError::Io` is declared but unconstructible from this crate

[`src/error.rs:82-84`](../../src/error.rs#L82-L84) carries `Io(#[from] std::io::Error)`, but no code path in the crate produces it — the only I/O (Linux `/proc` read) is deliberately wrapped into `Ram`, as `Snapshot::now`'s `# Errors` doc itself points out. A dead public variant costs match arms downstream and implies an error path that cannot occur. Options: remove it in the next minor (it is `#[non_exhaustive]`-shielded, but removal is still breaking — so more realistically), or document on the variant that it is currently reserved/never produced.

### 3.3 NVML 64-process cap can silently truncate `hmn ps` on busy Linux boxes

[`src/gpu/nvml.rs:65-69`](../../src/gpu/nvml.rs#L65-L69), [`nvml.rs:581-715`](../../src/gpu/nvml.rs#L581-L715): `NVML_ERROR_INSUFFICIENT_SIZE` is treated as soft success and the listing keeps only the first 64 compute processes. Correct and documented for the *calling-process* lookup, but for `list_compute_processes` on a shared multi-tenant box (the H100/GB200 field-validation environments ROADMAP mentions), `hmn ps` would silently omit rows with no indication. Cheap improvements: retry once with `count` as returned by NVML, or emit a stderr note ("N of M shown") when the cap bites — the latter matches the existing `hmn ps` summary-line philosophy.

### 3.4 Stale "planned for v0.2" forward references

[`src/snapshot.rs:233`](../../src/snapshot.rs#L233) (*"A long-lived `NVML` context is planned for v0.2."*) and [`src/gpu/nvml.rs:17-18`](../../src/gpu/nvml.rs#L17-L18) (*"a candidate for v0.2"*) — the crate has been in 0.2.x for nine releases and ROADMAP now lists the long-lived context as *speculative v0.3.0*. Both rustdoc-visible. Reword to "a later release (see `ROADMAP.md`)".

### 3.5 `Cargo.toml` `test-helpers` comment mentions one builder of three

[`Cargo.toml:81-84`](../../Cargo.toml#L81-L84) describes the feature as exposing *"a `GpuDeviceInfoBuilder`"*; since v0.2.5/v0.2.6 it also exposes `SpillReportBuilder` and `GpuProcessEntryBuilder` (lib.rs's feature table is correct). Trivial, but this comment is the first thing a reader of the manifest sees.

### 3.6 `CHANGELOG.md` bracketed versions have no link definitions

The file claims Keep-a-Changelog format and uses `## [0.2.9] - …` headings, but has no link-reference definitions (`[0.2.9]: https://github.com/...compare/v0.2.8...v0.2.9`) at the bottom, so the brackets render as plain text. Cosmetic; either add the links or drop the brackets.

### 3.7 `hmn` (no subcommand) can print nothing on a half-broken driver stack

[`src/bin/hmn.rs:352-357`](../../src/bin/hmn.rs#L352-L357): the "no visible GPUs" message triggers only when `Snapshot::all()` returns an empty `Vec`. If devices enumerate but every `device_info` call fails (partial driver install), the snapshots exist with `gpu_device: None`, `format_summary` skips them all, and the text mode prints *nothing* — indistinguishable from success with no output. Edge case; a "N device(s) enumerated but none readable" line would keep the tool's honesty contract.

---

## Watch items (no action required; recorded so they aren't re-derived)

- **PDH counter-set names and OS localization** — `PdhEnumObjectItemsW(w!("GPU Process Memory"))` relies on the counter-set name as registered. The GPU perflib-V2 sets are registered with English names and this works on the (non-English-locale) reference machine, so this is believed safe — but if a user on some Windows SKU ever reports the pre-`WDDM 2.0` fallback firing on modern hardware, check localization first.
- **PID-reuse name-race in `hmn watch`** — already tracked as an open ROADMAP item (process-start-time as second identity signal); nothing new found.
- **32-bit Windows** — the `usize → u64` casts in `dxgi.rs` are correct on both widths; no issue, verified during read.

## What is demonstrably healthy (verified during this audit)

- All five gates green locally on Windows (fmt, clippy defaults + all-features under `-D warnings`, 239 tests, doc build under `-D warnings`), plus `--no-default-features` check.
- Every `unsafe` block read carries an accurate `// SAFETY:` comment consistent with the actual call site; `#![deny(unsafe_code)]` with scoped module allows holds everywhere; `src/spill.rs` genuinely contains no `unsafe` of its own.
- FFI struct layouts spot-checked against their C definitions (`nvmlMemory_v2_t` version tag, `PROCESS_MEMORY_COUNTERS`, `task_vm_info` rev3, `PROCESSENTRY32W`): no discrepancies found.
- Init/shutdown pairing in `nvml.rs` is balanced on every return path, as claimed.
- The `fold` spill core, PDH instance parsers, `nvidia-smi` CSV parsers, and all CLI formatters are covered by platform-independent unit tests, including the documented regression fixtures (commit-gap false positive, `[exited]`-vs-`[protected]` counting, bracket-flicker reset safety).
- README/FAQ/tutorials are current with v0.2.9 behavior (the drift found is confined to the files listed above).

---

## Suggested remediation order

| # | Item | Effort |
|---|------|--------|
| 1 | 1.1 + 1.2 — bring `hmn.rs` header + `long_about` to v0.2.8 semantics | ~30 min, docs-only, ride-along in next release |
| 2 | 1.3 — fix `gpu_processes()` rustdoc Limitations | ~15 min, docs-only |
| 3 | 2.2 — ROADMAP/roadmap-v0.2.8 status flips + v0.2.9 index row | ~10 min |
| 4 | 2.3 + 3.4 + 3.5 — remaining stale-prose fixes | ~15 min |
| 5 | 2.1 — macOS CI leg | ~30 min + one CI iteration |
| 6 | 2.4 + 2.5 — CI feature-combo check + tag/version guard | ~20 min |
| 7 | 3.1 — DXGI walk `continue`-on-bad-adapter | small code change + review |
| 8 | 3.3, 3.2, 3.6, 3.7 — as convenient | opportunistic |

Items 1–4 are pure documentation and could ship together as a `docs:` consistency-pass commit; items 5–6 are CI-only; items 7–8 are the only code changes and all are behavior-preserving on healthy hardware.
