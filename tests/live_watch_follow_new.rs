// SPDX-License-Identifier: MIT OR Apache-2.0

//! Live end-to-end test for `hmn watch --follow-new` against two
//! *sequential* `spillforge` runs — the closest reproduction, using the
//! existing forced-spill fixture, of the workload shape (successive
//! short-lived GPU processes) that motivated `--follow-new`: a candle-mi
//! dogfooding report (`docs/dogfooding-feedbacks/dogfooding-watch-follow-new.md`)
//! ran `hmn watch` alongside 19 sequential `cargo test` processes and
//! found the pre-`--follow-new` frozen-at-attach PID set never saw any
//! of them.
//!
//! Same gate and prerequisite as `tests/live_watch.rs` (`windows + pdh +
//! cli`; `tools/spillforge/target/release/spillforge.exe` prebuilt):
//!
//! ```sh
//! cargo build --release --manifest-path tools/spillforge/Cargo.toml
//! cargo test --features cli,pdh --test live_watch_follow_new -- --ignored
//! ```

#![cfg(all(windows, feature = "pdh", feature = "cli"))]

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

/// Path to the spillforge fixture — see `tests/live_watch.rs`'s copy of
/// this helper for the full rationale (kept duplicated rather than
/// shared: these are two independent `#[ignore]`-gated integration test
/// binaries, and Cargo has no lightweight way to share a helper between
/// them without a `[lib]`/`dev-dependencies` shim for two lines of
/// code).
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

/// End-to-end acceptance test for the workload shape `--follow-new` was
/// built for: `hmn watch --follow-new` attaches to an otherwise-idle-ish
/// desktop, then two *sequential* `spillforge` processes run to
/// completion one after another while it observes. Asserts the closing
/// summary reports real spill and includes at least one `spillforge.exe`
/// entry in `per_pid[]` with substantial committed VRAM — proving a
/// process that was born *after* attach, and that may have already
/// exited by the time the watch ends, is still attributed in the final
/// report rather than silently absent (the exact gap the motivating
/// dogfooding report found).
#[test]
#[ignore = "requires Windows + WDDM 2.0+ GPU and a prebuilt spillforge fixture"]
#[allow(clippy::expect_used, clippy::panic)] // test-only
fn hmn_watch_follow_new_tracks_two_sequential_spillforge_runs() {
    // A real spillforge run commits well into double-digit GiB; 5 GiB is
    // a generous floor that rejects a stray 0-byte / desktop-scale
    // per_pid entry from a name coincidence without depending on the
    // exact figure (which varies run to run with desktop load).
    const FIVE_GIB: u64 = 5 * 1024 * 1024 * 1024;

    let watch = Command::new(env!("CARGO_BIN_EXE_hmn"))
        .args([
            "watch",
            "--follow-new",
            "--interval",
            "2s",
            "--duration",
            "90s",
            "--json",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn hmn watch");

    // Let the watch take its first sample (the desktop-only baseline)
    // before either spillforge run starts, so both are genuinely
    // "entered" transitions rather than racing the watch's own attach.
    std::thread::sleep(Duration::from_secs(1));

    for run in 1..=2 {
        // 20 GiB working set (spillforge's own default), 10 s churn —
        // long enough for hmn watch's 2 s interval to reliably sample
        // during the hold phase.
        let status = Command::new(spillforge_path())
            .args(["20", "10"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .expect("failed to run spillforge");
        assert!(
            status.success(),
            "spillforge run {run} exited with {status:?}"
        );
    }

    let output = watch
        .wait_with_output()
        .expect("failed to wait on hmn watch");

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
        "expected spilled:true (two forced spillforge runs should both spill), got: {summary_line}"
    );
    assert!(
        !summary_line.contains(r#""episodes":[]"#),
        "expected at least one spill episode, got: {summary_line}"
    );

    let spillforge_entries = summary_line.matches(r#""name":"spillforge.exe""#).count();
    assert!(
        spillforge_entries >= 1,
        "expected at least one per_pid entry named spillforge.exe (both runs share a PID only \
         in the rare case the OS immediately reuses the exact same PID with the same-name \
         reset unable to distinguish them — see the per-PID-attribution caveat in \
         docs/tutorials/watching-a-running-job.md), got: {summary_line}"
    );

    let peak_bytes: Vec<u64> = summary_line
        .split(r#""name":"spillforge.exe""#)
        .skip(1)
        .filter_map(|tail| {
            let key = r#""peak_used_bytes":"#;
            let start = tail.find(key)? + key.len();
            let digits: String = tail
                .get(start..)?
                .chars()
                .take_while(char::is_ascii_digit)
                .collect();
            digits.parse::<u64>().ok()
        })
        .collect();
    assert!(
        peak_bytes.iter().any(|&b| b > FIVE_GIB),
        "expected at least one spillforge.exe per_pid entry with peak_used_bytes > 5 GiB, \
         got peaks {peak_bytes:?} in: {summary_line}"
    );

    assert_eq!(
        output.status.code(),
        Some(1),
        "hmn watch --follow-new should exit 1 when spill was observed; stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
