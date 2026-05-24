# Add macOS smoke test

**Goal**: Add a platform-gated integration test that exercises `process_rss`, `device_count`, `device_info`, `process_gpu_info`, and `gpu_processes` on macOS and asserts the contract values are sane.
**Pre-conditions**:
- [ ] `gpu_dispatcher_wiring.md` is `done` — the four dispatchers route to `metal::*` on macOS
**Success Gates**:
- ⬜ [static] `tests/macos_smoke.rs` exists and is `#[cfg(target_os = "macos")]`-gated at the file level (every test inside is `#[cfg(target_os = "macos")]` or the whole file is)
- ⬜ [run] `cargo test --target aarch64-apple-darwin --test macos_smoke` passes
- ⬜ [run] `cargo test --target x86_64-unknown-linux-gnu --test macos_smoke` succeeds with zero tests run (no compile errors — the cfg gate compiles out cleanly)
- ⬜ [run] `cargo test --target x86_64-pc-windows-msvc --test macos_smoke` succeeds with zero tests run
- ⬜ [behavioral] The test asserts: process RSS > 1 MiB; `device_count() == Some(1)`; `device_info(0)` returns a name containing "Apple" AND `total_bytes >= 8 << 30`; `process_gpu_info(0)` returns `Some(_)` with `is_per_process == true` and `source == GpuQuerySource::Metal`; `gpu_processes(0)` returns `Ok(rows)` containing at least one entry whose `pid == std::process::id() as i32` (closes parity with the cross-platform `gpu_processes_returns_result_or_no_gpu_source` at [tests/smoke.rs:77](../../../tests/smoke.rs)); `snapshot_now()` returns a `Snapshot` whose `.gpu.is_some()` holds (tightens the cross-platform `snapshot_now_returns_ram_without_gpu` smoke test which only asserts RAM)
**References**: [R03 §Verification](/Users/hacker/.claude/plans/reports-macos-ledger-gives-you-the-effervescent-river.md), [R02 §Headline Result](../../../__reports__/macos_ledger/05-findings_writes_v0.md) (the 256 MiB write-every-byte pattern), [tests/smoke.rs](../../../tests/smoke.rs) (template for assertion style)

## Step 1: Create `tests/macos_smoke.rs` with the platform-gated assertion suite

**Goal**: Land a `#[cfg(target_os = "macos")]`-gated integration test that exercises every macOS surface and asserts cross-platform-contract-correct values.
**Implementation Logic**:
Create `tests/macos_smoke.rs`. Wrap the entire file body (or every `#[test]` fn) in `#[cfg(target_os = "macos")]`. Define six tests: (1) `process_rss_returns_positive_on_macos` — calls `hypomnesis::process_rss()`, asserts `> 1_000_000`; (2) `device_count_is_one_on_apple_silicon` — asserts `Some(1)`; (3) `device_info_reports_apple_brand` — asserts the name `.contains("Apple")` AND `total_bytes >= 8 << 30` (8 GiB minimum on shipping Apple Silicon); (4) `process_gpu_info_returns_metal_source` — writes a 256 MiB `vec![0u8; ...]` and touches every byte, then calls `hypomnesis::process_gpu_info(0)`, asserts the `Result<Some(_)>` shape with `source == GpuQuerySource::Metal` and `is_per_process == true`; (5) `gpu_processes_returns_metal_rows_for_self` — calls `hypomnesis::gpu_processes(0)`, asserts `Ok(rows)` shape AND `rows.iter().any(|r| r.pid == std::process::id() as i32)` (closes parity with the cross-platform `gpu_processes_returns_result_or_no_gpu_source` at [tests/smoke.rs:77](../../../tests/smoke.rs)); (6) `snapshot_now_includes_gpu_on_macos` — asserts `hypomnesis::snapshot_now().gpu.is_some()` (tightens the cross-platform `snapshot_now_returns_ram_without_gpu` which only requires RAM). Use the same assertion style as `tests/smoke.rs`.
**Deliverables**: `tests/macos_smoke.rs` (new) — six `#[cfg(target_os = "macos")]`-gated tests: `process_rss_returns_positive_on_macos`, `device_count_is_one_on_apple_silicon`, `device_info_reports_apple_brand`, `process_gpu_info_returns_metal_source`, `gpu_processes_returns_metal_rows_for_self`, `snapshot_now_includes_gpu_on_macos`
**Consistency Checks**: `cargo test --target aarch64-apple-darwin --test macos_smoke` (expected: PASS)
**Commit**: `test(macos): add macOS smoke test covering RAM + GPU surfaces`
