# Compile gates on three targets

**Goal**: Run `cargo build`, `cargo clippy -- -D warnings`, and `cargo test` on `aarch64-apple-darwin`, `x86_64-pc-windows-msvc`, and `x86_64-unknown-linux-gnu`; capture the outputs into a single attached log; no warnings on any target.
**Pre-conditions**:
- [ ] All integration-level leaves are `done`
- [ ] Apple Silicon machine has all three target toolchains installed (`rustup target add aarch64-apple-darwin x86_64-pc-windows-msvc x86_64-unknown-linux-gnu`)
**Success Gates**:
- ⬜ [run] `cargo build --target aarch64-apple-darwin` exits 0 with no warnings
- ⬜ [run] `cargo build --no-default-features --target aarch64-apple-darwin` exits 0 (RAM path independent of `metal`)
- ⬜ [run] `cargo clippy --target aarch64-apple-darwin --all-targets --features metal -- -D warnings` exits 0
- ⬜ [run] `cargo test --target aarch64-apple-darwin` exits 0 with `tests/smoke.rs` and `tests/macos_smoke.rs` passing
- ⬜ [run] `cargo build --target x86_64-pc-windows-msvc` exits 0
- ⬜ [run] `cargo build --target x86_64-unknown-linux-gnu` exits 0
- ⬜ [static] Log file `__reports__/macos_ledger/11-verification_compile_gates_v0.md` exists and contains the stdout of each command above with the result line
**References**: [R03 §Verification — Layer 1](/Users/hacker/.claude/plans/reports-macos-ledger-gives-you-the-effervescent-river.md)

## Step 1: Run all six compile/test invocations and persist the log

**Goal**: Execute the compile-test matrix and persist the raw output for the PR description.
**Implementation Logic**:
Run each command in sequence, redirecting stdout+stderr to per-command files under `/tmp/`. Concatenate into `__reports__/macos_ledger/11-verification_compile_gates_v0.md` with a section header per command and the verdict (PASS/FAIL) at the top. If any FAIL, do not proceed to commit — escalate per [R01 §Failure Escalation Ladder].
**Deliverables**: `__reports__/macos_ledger/11-verification_compile_gates_v0.md` (new) — six sections (one per command), each with verdict + full stdout/stderr; concludes with a single sentinel line "All gates passed"
**Consistency Checks**: `test -s __reports__/macos_ledger/11-verification_compile_gates_v0.md && grep -q '^All gates passed$' __reports__/macos_ledger/11-verification_compile_gates_v0.md` (expected: PASS)
**Commit**: `chore(verification): record compile-gate matrix for v0.2.2`
