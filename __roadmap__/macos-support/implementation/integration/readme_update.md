# Update README capabilities + install

**Goal**: Add a macOS row to the `README.md` capabilities table and extend the install snippet so the existing user-facing documentation reflects the new platform support.
**Pre-conditions**:
- [ ] `cargo_features.md` and `gpu_metal.md` are `done` (their existence anchors what the README claims)
**Success Gates**:
- ⬜ [static] The capabilities table at [README.md:141-148](../../../README.md) has a `macOS` column with rows: Process RSS → `task_info(TASK_VM_INFO_PURGEABLE).phys_footprint`; Device-wide GPU memory → `sysctl hw.memsize`; Per-process GPU memory → `ledger(LEDGER_ENTRY_INFO_V2).graphics_footprint`; Fallback → none
- ⬜ [static] The install section at [README.md:26-45](../../../README.md) mentions macOS once and lists the `metal` feature (in the default set)
- ⬜ [static] The README mentions Apple Silicon UMA exactly once with one sentence noting "system DRAM is the GPU memory pool"
- ⬜ [static] The "Future Platforms" section (if present) no longer lists macOS as future
**References**: [R03 §Modify — README.md](/Users/hacker/.claude/plans/reports-macos-ledger-gives-you-the-effervescent-river.md), [README.md](../../../README.md)

## Step 1: Extend capabilities table + install snippet with macOS row

**Goal**: Make the README factually represent the new macOS support, mirroring the existing wording style (factual, metric-first, API names in backticks).
**Implementation Logic**:
Locate the capabilities table in `README.md` (around lines 141-148). Add a third column "macOS" with one row per existing metric. Match the existing style — API names in backticks, no marketing language. Locate the install section (around lines 26-45). Note that the `metal` feature is enabled by default on macOS via `cfg(all(target_os = "macos", feature = "metal"))`, the same way `dxgi` is gated on Windows. Add a one-sentence note about Apple Silicon UMA and that the GPU memory pool IS the system DRAM. Search the file for any "macOS" or "Apple" references in a "future platforms" or "roadmap" section and remove the line (or mark it done).
**Deliverables**: `README.md` — modified capabilities table (now 3 platform columns: Windows, Linux, macOS); modified install section noting `metal` is in default features and macOS Apple Silicon UMA architecture; one sentence stating "system DRAM is the GPU memory pool" on Apple Silicon
**Consistency Checks**: `grep -q '| Per-process GPU memory' README.md && grep -q 'hw.memsize' README.md && grep -q 'Apple Silicon' README.md` (expected: PASS)
**Commit**: `docs(readme): document macOS RAM + GPU support`
