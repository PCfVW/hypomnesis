# macos-support

## Context

Adds macOS (Apple Silicon) support to `hypomnesis` for v0.2.2 framing, mirroring the existing Windows + Linux RAM + GPU measurement surface. The PR does not modify `Cargo.toml`'s `version` field; the maintainer applies the bump after acceptance. Restart from `main` (v0.2.1) — nothing from the abandoned `claude/compassionate-moore-709120` is reused.

## Reference Documents

- [R01 Knowledge Transfer v3](../__reports__/macos_ledger/09-knowledge_transfer_v3.md) — design rationale, dependency audit, anti-source list, next-cycle changes
- [R02 Writes-Corrected Probe Findings](../__reports__/macos_ledger/05-findings_writes_v0.md) — Round 05 empirical proof that `graphics_footprint` is resident-tracked; Swift+C probe source in Appendix A
- [R03 Plan](/Users/hacker/.claude/plans/reports-macos-ledger-gives-you-the-effervescent-river.md) — the planning document this roadmap implements

## Goal

Ship macOS RAM + GPU measurement (resident-byte semantics, fixed-capacity `total_bytes`) via libSystem syscalls only — no `objc2`, no `objc2-metal`, no Apple-framework crates — with feature parity to the existing Windows + Linux dispatchers.

## Pre-conditions
- [ ] Working tree is on `main` (v0.2.1); no commits from `claude/compassionate-moore-709120` are present
- [ ] Hardware available for verification: Apple Silicon Mac (M-series), macOS 10.15+ (Round 05 verified on macOS 26.x)
- [ ] `cargo`, `clippy`, and the `aarch64-apple-darwin` target are installed

## Success Gates
- ✅ `cargo build --target aarch64-apple-darwin` succeeds with zero warnings
- ✅ `cargo clippy --target aarch64-apple-darwin -- -D warnings` is clean
- ✅ `cargo build --target x86_64-pc-windows-msvc` and `cargo build --target x86_64-unknown-linux-gnu` still succeed (no regression on existing platforms)
- ✅ CLI output verifier agent reports: process RSS > 0; `device_count == 1`; device name contains "Apple"; `total_bytes` matches `sysctl -n hw.memsize` exactly; `used_bytes` is a non-negative u64
- ✅ Allocation-residency probe: writing 256 MiB Vec<u8> increases reported `phys_footprint` by +256 MiB ± 16 KiB (the contract validator from [R02 §Headline Result])
- ✅ `Cargo.toml` `version` field is unchanged at `"0.2.1"`
- ✅ No new Cargo dependency added under `[target.'cfg(target_os = "macos")'.dependencies]`
- ✅ [CONVENTIONS.md](../CONVENTIONS.md) safety/performance-critical sections are unmodified — unless the PR also contains explicit no-alternative proof, in which case the modification ships as its own commit

## Gotchas

- `LEDGER_ENTRY_INFO_V2` entry indices are NOT stable across macOS versions. The implementation must enumerate template entries by `lti_name == "graphics_footprint"` at init; the index 36 observed on macOS 26.x is **not** to be hardcoded.
- Cross-user PIDs require root and must degrade silently (return zero-row entries, never panic / propagate `Err`).
- The `metal` Cargo feature is gated by `cfg(all(target_os = "macos", feature = "metal"))`, mirroring how `dxgi` is gated by `cfg(all(windows, feature = "dxgi"))`. Same pattern; same default-features role.

## Status
```mermaid
graph TD
    dep_audit[libSystem-only dep audit]:::done
    implementation[Implementation]:::done
    classDef done       fill:#166534,color:#bbf7d0
    classDef inprogress fill:#854d0e,color:#fef08a
    classDef planned    fill:#374151,color:#e5e7eb
    classDef amendment  fill:#1e3a5f,color:#bfdbfe
    classDef blocked    fill:#7f1d1d,color:#fecaca
```

## Nodes
| Node | Type | Status |
|:-----|:-----|:-------|
| `dep_audit.md` | 📄 Leaf Task | ✅ Done |
| `implementation/` | 📁 Directory | ✅ Done |

## Amendment Log
| ID | Date | Source | Nodes Added | Rationale |
|:---|:-----|:-------|:------------|:----------|

## Progress
| Node | Branch | Commits | Notes |
|:-----|:-------|:--------|:------|
