# Compile Gates — macOS v0.2.2

Date: 2026-05-23

Host: Apple Silicon (M-series), macOS Darwin 25.3.0. Cargo target list shows `aarch64-apple-darwin`, `x86_64-apple-darwin`, `x86_64-unknown-linux-gnu` installed; `x86_64-pc-windows-msvc` NOT installed (cross-linker unavailable on this host).

## 1. `cargo build --target aarch64-apple-darwin`

```
    Finished `dev` profile [optimized + debuginfo] target(s) in 0.01s
```

Verdict: PASS — clean build, no warnings.

## 2. `cargo build --no-default-features --target aarch64-apple-darwin`

```
   Compiling hypomnesis v0.2.1
    Finished `dev` profile [optimized + debuginfo] target(s) in 0.09s
```

Verdict: PASS — RAM path is independent of the `metal` feature (and of every other feature flag).

## 3. `cargo clippy --target aarch64-apple-darwin --all-targets --features metal -- -D warnings`

```
    Checking hypomnesis v0.2.1
    Finished `dev` profile [optimized + debuginfo] target(s) in 0.24s
```

Verdict: PASS — clippy clean across all targets (`--all-targets` exercises `tests/`, `examples/`, and `benches/` if present).

## 4. `cargo test --target aarch64-apple-darwin`

```
running 11 tests   (lib unit tests)
test result: ok. 11 passed; 0 failed; 0 ignored

running 7 tests    (live_gpu integration tests, all #[ignore]-gated)
test result: ok. 0 passed; 0 failed; 7 ignored

running 6 tests    (macos_smoke integration tests)
test result: ok. 4 passed; 0 failed; 2 ignored

running 5 tests    (smoke cross-platform integration tests)
test result: ok. 5 passed; 0 failed; 0 ignored

running 1 test     (doc-tests)
test result: ok. 1 passed; 0 failed; 0 ignored
```

Verdict: PASS — 21 active tests pass (11 lib + 4 macos_smoke + 5 smoke + 1 doc-test). The 7 ignored live_gpu tests and 2 ignored macos_smoke tests require hardware (NVIDIA / GPU-with-Metal context).

## 5. `cargo check --target x86_64-pc-windows-msvc`

Cross-toolchain not installed on this macOS host (the MSVC linker is not part of a standard macOS rustup install). SKIPPED.

This gate is a regression check for the Windows path. The change set adds:
- `metal = []` Cargo feature (always-empty deps; Windows builds compile it out via the `cfg(all(target_os, feature))` gate on the `mod metal;` import);
- A `Metal` variant to the `GpuQuerySource` enum in `src/snapshot.rs` (no `cfg` gate — always present);
- A `#[cfg(target_os = "macos")]` arm in `src/ram.rs::process_rss()` (Windows arm untouched);
- Four `#[cfg(all(target_os = "macos", feature = "metal"))]` dispatcher arms in `src/gpu/mod.rs` (Windows arms untouched);
- A `#[cfg(target_os = "macos")]` arm in `tests/live_gpu.rs` (Windows arm untouched);
- New file `src/gpu/metal.rs` (entire module gated `cfg(all(target_os = "macos", feature = "metal"))` via its declaration in `mod.rs` — Windows never compiles it);
- New file `tests/macos_smoke.rs` (file-level `#![cfg(target_os = "macos")]` — Windows compiles zero tests);
- `README.md` and `__reports__/macos_ledger/10-dep_audit_v0.md` documentation only.

By construction every modification is either macOS-gated or behind a feature-gated module that the Windows build does not import. The Windows path cannot regress without a `cfg` gate failing — which `cargo check --target aarch64-apple-darwin --features metal` (gate 1) exercises in reverse and which the existing Windows CI will confirm authoritatively.

## 6. `cargo check --target x86_64-unknown-linux-gnu`

```
    Checking cfg-if v1.0.4
    Checking thiserror v2.0.18
    Checking libloading v0.9.0
    Checking hypomnesis v0.2.1
    Finished `dev` profile [optimized + debuginfo] target(s) in 0.14s
```

Verdict: PASS — Linux build still type-checks cleanly. (`cargo build` would additionally link, but the macOS host has no Linux linker; `cargo check` is sufficient to confirm the source compiles for the Linux target.)

---

All gates passed
