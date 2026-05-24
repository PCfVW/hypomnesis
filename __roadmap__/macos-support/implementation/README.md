# Implementation

## Context

Holds the code-writing phase of the macOS port. Runs after `dep_audit.md` (parent campaign's only leaf at depth 1) confirms the libSystem-only path is viable. Three independent code leaves run in parallel at depth 2, then an `integration/` branch wires the pieces together at depth 3 and adds documentation + tests, with `verification/` at depth 4 as the final gate.

## Reference Documents

- [R01 Knowledge Transfer v3](../../__reports__/macos_ledger/09-knowledge_transfer_v3.md) — § Reference Design table; the cross-platform-consistency contract
- [R02 Round 05](../../__reports__/macos_ledger/05-findings_writes_v0.md) — empirical proof for `graphics_footprint` resident-tracking
- [R03 Plan](/Users/hacker/.claude/plans/reports-macos-ledger-gives-you-the-effervescent-river.md) — the planning document

## Goal

Land the three independent code surfaces of the macOS port — Cargo feature flag, `src/ram.rs` macOS branch, `src/gpu/metal.rs` module — then wire them, document them, and verify them.

## Pre-conditions
- [ ] `dep_audit.md` is `done` and `__reports__/macos_ledger/10-dep_audit_v0.md` exists
- [ ] No `objc2` / `objc2-metal` dependency has been added (negative pre-condition; verified by absence in `Cargo.toml`)

## Success Gates
- ✅ `src/gpu/metal.rs` exists and compiles on `aarch64-apple-darwin`
- ✅ `src/ram.rs` has a working `#[cfg(target_os = "macos")]` branch returning `phys_footprint` for the calling process
- ✅ `Cargo.toml` has `metal = []` in `[features]` and `"metal"` in the `default` list; no new `[target.'cfg(target_os = "macos")'.dependencies]` block
- ✅ The four GPU dispatchers in `src/gpu/mod.rs` route to `metal::*` on macOS with `if let Some(...)` fall-through, mirroring the existing DXGI/NVML pattern

## Status
```mermaid
graph TD
    cargo_features[Add metal Cargo feature]:::done
    ram_darwin[Add macOS RAM path]:::done
    gpu_metal[Create src/gpu/metal.rs]:::done
    integration[Integration]:::done
    classDef done       fill:#166534,color:#bbf7d0
    classDef inprogress fill:#854d0e,color:#fef08a
    classDef planned    fill:#374151,color:#e5e7eb
    classDef amendment  fill:#1e3a5f,color:#bfdbfe
    classDef blocked    fill:#7f1d1d,color:#fecaca
```

## Nodes
| Node | Type | Status |
|:-----|:-----|:-------|
| `cargo_features.md` | 📄 Leaf Task | ✅ Done |
| `ram_darwin.md` | 📄 Leaf Task | ✅ Done |
| `gpu_metal.md` | 📄 Leaf Task | ✅ Done |
| `integration/` | 📁 Directory | ✅ Done |

## Amendment Log
| ID | Date | Source | Nodes Added | Rationale |
|:---|:-----|:-------|:------------|:----------|

## Progress
| Node | Branch | Commits | Notes |
|:-----|:-------|:--------|:------|
