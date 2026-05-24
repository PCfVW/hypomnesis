# CLI output verifier agent

**Goal**: Spawn a dedicated, observation-only sub-agent that executes the `hypomnesis` CLI on Apple Silicon, captures stdout for every command, and asserts the values match the cross-platform contract — semantic correctness, not just exit-zero.
**Pre-conditions**:
- [ ] `compile_gates.md` is `done` — the binary built and tests passed
- [ ] `cargo build --release --features cli --target aarch64-apple-darwin` succeeds (the CLI binary exists at `target/aarch64-apple-darwin/release/hypomnesis`)
**Success Gates**:
- ⬜ [static] `__reports__/macos_ledger/12-verification_cli_output_v0.md` exists and contains: raw stdout of every CLI invocation; a one-line PASS/FAIL per assertion below; the verifier agent's final verdict
- ⬜ [behavioral] Assertion 1 — `process_rss()` is reported, 1 MiB < RSS < 16 GiB
- ⬜ [behavioral] Assertion 2 — `device_count == 1`
- ⬜ [behavioral] Assertion 3 — device name contains "Apple"
- ⬜ [behavioral] Assertion 4 — `total_bytes` matches `sysctl -n hw.memsize` exactly (byte-for-byte)
- ⬜ [behavioral] Assertion 5 — `used_bytes` is a non-negative u64
- ⬜ [behavioral] Assertion 6 (residency contract) — after writing every byte of a 256 MiB `Vec<u8>`, re-reading `process_rss()` shows a delta of +256 MiB ± 16 KiB
- ⬜ [behavioral] Assertion 7 (gpu_processes parity) — `gpu_processes(0)` returns a non-empty Vec when the test harness has just written a 256 MiB allocation; each entry has `pid > 0`, non-empty `name`, `used_bytes > 0`
- ⬜ [behavioral] Assertion 8 (graceful degradation) — running the CLI without `sudo` returns a `gpu_processes(0)` Vec that is a subset of the `sudo`-run version; neither call panics or returns an `Err`
**References**: [R02 §Headline Result](../../../../__reports__/macos_ledger/05-findings_writes_v0.md) (the 256 MiB write probe), [R03 §Verification — Layer 2 & 3](/Users/hacker/.claude/plans/reports-macos-ledger-gives-you-the-effervescent-river.md)

## Step 1: Spawn verifier sub-agent with the explicit protocol and observation-only constraint

**Goal**: Launch a `general-purpose` sub-agent (foreground), instructed to run the CLI, capture stdout for every invocation, and write the per-assertion PASS/FAIL report. The agent is forbidden from modifying any implementation file.
**Implementation Logic**:
Use the Agent tool with `subagent_type=general-purpose` and a self-contained prompt that includes: (a) the eight assertions above verbatim; (b) the exact CLI invocations to run (the verifier discovers the subcommand surface from `src/bin/` if it is not given); (c) the requirement to paste raw stdout (not paraphrased) into its returned report; (d) explicit instruction to NOT edit any source file — only `__reports__/macos_ledger/12-verification_cli_output_v0.md`; (e) the 256 MiB write-probe protocol from [R02 §Headline Result] adapted to call the Rust API (allocate `Vec<u8>`, touch every byte via `for i in 0..len { vec[i] = (i & 0xff) as u8 }`); (f) instruction to run the CLI both with and without `sudo` and compare; (g) tell the agent to use colgrep (via Bash) as the primary search tool. The agent returns its report; the main thread reads the report file before marking this leaf `done`.
**Deliverables**: `__reports__/macos_ledger/12-verification_cli_output_v0.md` (new) — agent-authored report with sections §Invocations (raw stdout per command), §Assertions (PASS/FAIL × 8), §Verdict (single line "Contract holds" or "Contract violated: <which>"), §Re-runnable protocol (exact commands)
**Consistency Checks**: `test -s __reports__/macos_ledger/12-verification_cli_output_v0.md && grep -q '^Contract holds$' __reports__/macos_ledger/12-verification_cli_output_v0.md` (expected: PASS)
**Commit**: `chore(verification): cli output verifier report for v0.2.2`
