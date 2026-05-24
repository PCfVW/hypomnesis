# Integration

## Context

Runs after the parallel implementation leaves (`cargo_features`, `ram_darwin`, `gpu_metal`) at depth 2. Wires the new `metal` module into the four GPU dispatchers, extends `GpuQuerySource` with the `Metal` variant, updates user-facing documentation, and adds the macOS smoke test. The `verification/` branch at depth 4 is the final gate.

## Reference Documents

- [R01 Knowledge Transfer v3](../../../__reports__/macos_ledger/09-knowledge_transfer_v3.md) — design rationale, anti-source list, next-cycle changes
- [R03 Plan](/Users/hacker/.claude/plans/reports-macos-ledger-gives-you-the-effervescent-river.md) — planning document
- [R04 src/gpu/mod.rs](../../../src/gpu/mod.rs) — the four-dispatcher template + `GpuQuerySource` enum to extend

## Goal

Connect the three depth-2 surfaces to the public API, document the new platform support, and add the platform-gated smoke test that proves the wiring on macOS without breaking Windows + Linux.

## Pre-conditions
- [ ] `cargo_features.md`, `ram_darwin.md`, `gpu_metal.md` are all `done`
- [ ] `src/gpu/metal.rs` exists and `cargo check --target aarch64-apple-darwin --features metal` succeeds

## Success Gates
- ✅ `GpuQuerySource` has a `Metal` variant accessible from the crate root (verified by extending `public_types_are_reachable_via_crate_root` in `tests/smoke.rs`)
- ✅ All four dispatchers in `src/gpu/mod.rs` (`device_count`, `device_info`, `process_gpu_info`, `gpu_processes`) route to `metal::*` on macOS using the existing `if let Some(...) = backend::fn() { return ... }` idiom
- ✅ `README.md` has a macOS row in the capabilities table at lines 141-148 and a macOS-aware install snippet
- ✅ `tests/macos_smoke.rs` exists, is `#[cfg(target_os = "macos")]`-gated, and passes on Apple Silicon
- ✅ `cargo build --target x86_64-pc-windows-msvc` and `cargo build --target x86_64-unknown-linux-gnu` remain successful

## Status
```mermaid
graph TD
    gpu_dispatcher_wiring[Wire metal into GPU dispatchers]:::done
    readme_update[Update README capabilities + install]:::done
    macos_smoke_test[Add macOS smoke test]:::done
    verification[Verification]:::done
    classDef done       fill:#166534,color:#bbf7d0
    classDef inprogress fill:#854d0e,color:#fef08a
    classDef planned    fill:#374151,color:#e5e7eb
    classDef amendment  fill:#1e3a5f,color:#bfdbfe
    classDef blocked    fill:#7f1d1d,color:#fecaca
```

## Nodes
| Node | Type | Status |
|:-----|:-----|:-------|
| `gpu_dispatcher_wiring.md` | 📄 Leaf Task | ✅ Done |
| `readme_update.md` | 📄 Leaf Task | ✅ Done |
| `macos_smoke_test.md` | 📄 Leaf Task | ✅ Done |
| `verification/` | 📁 Directory | ✅ Done |

## Amendment Log
| ID | Date | Source | Nodes Added | Rationale |
|:---|:-----|:-------|:------------|:----------|

## Progress
| Node | Branch | Commits | Notes |
|:-----|:-------|:--------|:------|
