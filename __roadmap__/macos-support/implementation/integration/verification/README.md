# Verification

## Context

Final gate of the campaign. Runs after the integration-level leaves (`gpu_dispatcher_wiring`, `readme_update`, `macos_smoke_test`) are `done`. Two parallel leaves: `compile_gates` (build + clippy + test on three targets) and `cli_output_verifier` (a dedicated sub-agent runs the CLI, captures stdout, and asserts the values are contract-correct).

## Reference Documents

- [R01 Knowledge Transfer v3](../../../../__reports__/macos_ledger/09-knowledge_transfer_v3.md) — § Next-Cycle Changes (PR-description requirements)
- [R02 Round 05](../../../../__reports__/macos_ledger/05-findings_writes_v0.md) — empirical baseline for the contract-validator probe
- [R03 Plan §Verification](/Users/hacker/.claude/plans/reports-macos-ledger-gives-you-the-effervescent-river.md) — three-layer verification design

## Goal

Prove the campaign delivered the contract: compilation is clean on three targets, the CLI emits semantically correct values, and the empirical residency-contract holds end-to-end.

## Pre-conditions
- [ ] All integration-level leaves are `done`
- [ ] Apple Silicon hardware (M-series) is available

## Success Gates
- ✅ Compilation layer (cargo build, clippy, test) passes cleanly on `aarch64-apple-darwin`, `x86_64-pc-windows-msvc`, `x86_64-unknown-linux-gnu`
- ✅ CLI-output verifier agent's report (returned in this conversation as a sub-agent tool result) contains the raw stdout from each command it ran, with a pass/fail line per assertion enumerated in `cli_output_verifier.md`
- ✅ The empirical residency-contract probe (256 MiB Vec<u8> write) shows a `process_rss()` delta of +256 MiB ± 16 KiB

## Status
```mermaid
graph TD
    compile_gates[Compile gates on three targets]:::done
    cli_output_verifier[CLI output verifier agent]:::done
    classDef done       fill:#166534,color:#bbf7d0
    classDef inprogress fill:#854d0e,color:#fef08a
    classDef planned    fill:#374151,color:#e5e7eb
    classDef amendment  fill:#1e3a5f,color:#bfdbfe
    classDef blocked    fill:#7f1d1d,color:#fecaca
```

## Nodes
| Node | Type | Status |
|:-----|:-----|:-------|
| `compile_gates.md` | 📄 Leaf Task | ✅ Done |
| `cli_output_verifier.md` | 📄 Leaf Task | ✅ Done |

## Amendment Log
| ID | Date | Source | Nodes Added | Rationale |
|:---|:-----|:-------|:------------|:----------|

## Progress
| Node | Branch | Commits | Notes |
|:-----|:-------|:--------|:------|
