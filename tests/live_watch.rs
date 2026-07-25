// SPDX-License-Identifier: MIT OR Apache-2.0

//! Live end-to-end test for `hmn watch` against the `spillforge`
//! forced-spill fixture (`tools/spillforge`). `#![cfg(all(windows,
//! feature = "pdh", feature = "cli"))]` — `windows + pdh` is the spill
//! measurability precondition (same gate as `tests/live_pdh.rs`); `cli`
//! is required because this file resolves the compiled `hmn` binary via
//! `env!("CARGO_BIN_EXE_hmn")`, which Cargo only defines when the `hmn`
//! binary target (`required-features = ["cli"]`) is actually built.
//!
//! Requires `tools/spillforge/target/release/spillforge.exe` to be built
//! first:
//!
//! ```sh
//! cargo build --release --manifest-path tools/spillforge/Cargo.toml
//! cargo test --features cli,pdh --test live_watch -- --ignored
//! ```
//!
//! Spawns `spillforge` as a child, attaches `hmn watch --json` to its
//! PID, and asserts the closing summary reports a real spill episode —
//! the same fixture that validated `hmn spill` in v0.2.5, now validating
//! `hmn watch`'s attach-to-running-PID path end-to-end via the compiled
//! binary rather than the library API directly.

#![cfg(all(windows, feature = "pdh", feature = "cli"))]

use std::path::PathBuf;
use std::process::{Command, Stdio};

/// Path to the spillforge fixture, built separately per this file's
/// module docs. Panics with actionable instructions if missing rather
/// than silently skipping — an `#[ignore]`-gated test that's run
/// explicitly is expected to have its prerequisite already satisfied.
#[allow(clippy::expect_used, clippy::panic)] // test-only, actionable failure message
fn spillforge_path() -> PathBuf {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tools/spillforge/target/release/spillforge.exe");
    assert!(
        path.exists(),
        "spillforge fixture not built — run: cargo build --release \
         --manifest-path tools/spillforge/Cargo.toml (path checked: {})",
        path.display()
    );
    path
}

/// End-to-end acceptance test: `hmn watch <pid>` attached to a live,
/// forced-spilling `spillforge` process must report `spilled: true`
/// with at least one episode and exit `1`, exercising the full CLI path
/// (arg parsing, PID attach, per-interval sampling, JSONL rows, the
/// closing summary, and the exit-code contract) against real WDDM
/// spill — not a mock.
#[test]
#[ignore = "requires Windows + WDDM 2.0+ GPU and a prebuilt spillforge fixture"]
#[allow(clippy::expect_used, clippy::panic)] // test-only
fn hmn_watch_reports_spill_against_spillforge() {
    // 20 GiB working set (spillforge's own default), 20 s churn — long
    // enough that hmn watch's short interval below reliably samples
    // during the hold phase even accounting for process-spawn /
    // allocation latency.
    let mut spillforge = Command::new(spillforge_path())
        .args(["20", "20"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn spillforge");
    let pid = spillforge.id();

    let output = Command::new(env!("CARGO_BIN_EXE_hmn"))
        .args([
            "watch",
            &pid.to_string(),
            "--interval",
            "200ms",
            "--duration",
            "35s",
            "--json",
        ])
        .output()
        .expect("failed to run hmn watch");

    // spillforge exits on its own once the hold phase completes; reap it
    // regardless (it should already be gone by the time hmn watch's
    // --duration elapses, but wait() is cheap and avoids a zombie
    // either way).
    let _ = spillforge.wait();

    let stdout = String::from_utf8_lossy(&output.stdout);
    let summary_line = stdout
        .lines()
        .find(|l| l.contains(r#""kind":"summary""#))
        .unwrap_or_else(|| panic!("no summary line in hmn watch --json output:\n{stdout}"));

    assert!(
        summary_line.contains(r#""measurable":true"#),
        "expected measurable:true, got: {summary_line}"
    );
    assert!(
        summary_line.contains(r#""spilled":true"#),
        "expected spilled:true (spillforge should force a real WDDM spill), got: {summary_line}"
    );
    assert!(
        !summary_line.contains(r#""episodes":[]"#),
        "expected at least one spill episode, got: {summary_line}"
    );
    assert!(
        summary_line.contains(&format!("\"pid\":{pid}")),
        "expected a per_pid entry for the watched PID {pid}, got: {summary_line}"
    );

    assert_eq!(
        output.status.code(),
        Some(1),
        "hmn watch should exit 1 when spill was observed; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
