# Add metal Cargo feature

**Goal**: Register `metal` as a Cargo feature, include it in `default`, and verify `cargo build` still succeeds on Windows + Linux + macOS targets — without adding any new dependency.
**Pre-conditions**:
- [ ] `dep_audit.md` is `done`
**Success Gates**:
- ⬜ [static] `Cargo.toml` contains `metal = []` in `[features]`
- ⬜ [static] `default` features list contains `"metal"`
- ⬜ [static] `Cargo.toml` has NO new `[target.'cfg(target_os = "macos")'.dependencies]` block
- ⬜ [static] `Cargo.toml`'s `version` field is still `"0.2.1"`
- ⬜ [run] `cargo build --target aarch64-apple-darwin --features metal` succeeds (the feature alone, with `src/gpu/metal.rs` still absent, compiles because the dispatcher gate uses `cfg(all(target_os, feature))` and is sibling-parallel)
**References**: [R03 §Modify](/Users/hacker/.claude/plans/reports-macos-ledger-gives-you-the-effervescent-river.md), [R01 §Dependencies — libSystem-only](../../__reports__/macos_ledger/09-knowledge_transfer_v3.md)

## Step 1: Edit Cargo.toml — add `metal` feature, append to default

**Goal**: Make `metal` a recognised Cargo feature, included in default builds, with no dep change.
**Implementation Logic**:
In `[features]`, add the line `metal = []` (empty dependency list — libSystem syscalls need no crate dep). In the `default` array, append `"metal"`. Do **not** add `[target.'cfg(target_os = "macos")'.dependencies]`. Do **not** change `version`. This mirrors how `dxgi` is in `default` and is platform-gated by the module-import `cfg` in `src/gpu/mod.rs`.
**Deliverables**: `Cargo.toml` — adds `metal = []` to `[features]`; appends `"metal"` to the `default` array; no `[target.'cfg(target_os = "macos")'.dependencies]` block; `version` field unchanged at `"0.2.1"`
**Consistency Checks**: `cargo build --target aarch64-apple-darwin --features metal && grep -q '^metal = \[\]' Cargo.toml` (expected: PASS)
**Commit**: `feat(cargo): add metal feature for macOS GPU backend`
