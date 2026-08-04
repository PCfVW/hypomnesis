// SPDX-License-Identifier: MIT OR Apache-2.0

//! Live-PDH integration tests for the v0.2.2 Windows `PDH` per-process
//! `VRAM` backend and the v0.2.5 spill-detection path (`SpillTracker`,
//! `GpuProcessEntry::shared_used_bytes`). The spill tests live here —
//! not in `tests/live_gpu.rs`, as `docs/roadmap-v0.2.5.md` first
//! sketched — because this file's `#![cfg(all(windows, feature =
//! "pdh"))]` gate is exactly the spill-measurability precondition.
//! Every test is `#[ignore]`-gated and requires:
//!
//! - A Windows host with `WDDM 2.0`+ drivers (basically any modern
//!   Windows 10 / 11 / 12 system with an NVIDIA / AMD / Intel GPU).
//! - The `pdh` Cargo feature enabled (default-on).
//!
//! CI doesn't run these (hosted runners have no GPU); run them locally
//! with:
//!
//! ```sh
//! cargo test --features pdh --test live_pdh -- --ignored
//! ```
//!
//! Sibling to `tests/live_gpu.rs`, which covers `NVML` / `DXGI` /
//! `nvidia-smi` paths. The two live test files exercise different
//! backends so a maintainer can run them independently as the relevant
//! hardware is available.
//!
//! # Why these tests don't require the *calling* process in the list
//!
//! `gpu_processes()` walks `DXGI` only for adapter metadata
//! (`EnumAdapters1` + `GetDesc`) and then samples `PDH`. It does
//! **not** call `QueryVideoMemoryInfo` on the calling adapter, which
//! is the operation that would register the test binary with `VidMm`
//! for accounting. As a result, a test binary that has not separately
//! exercised the `DXGI` per-process path does not appear in PDH's
//! output — even though it ran the enumeration that produced the
//! list. This is correct behaviour, not a bug: PDH reflects what
//! `VidMm` is tracking, and `VidMm` tracks GPU memory holders, not
//! GPU memory queriers. These tests therefore assert on *observable*
//! invariants (non-empty list, plausible bytes, mostly-resolved
//! names) rather than the harder-to-satisfy "calling process is
//! present" invariant.

#![cfg(all(windows, feature = "pdh"))]

use hypomnesis::{GpuQuerySource, SpillTracker, gpu_processes, is_spill_measurable};

/// `gpu_processes(0)` on Windows / `WDDM` with `pdh` enabled returns a
/// non-empty list of rows whose `source` is [`GpuQuerySource::Pdh`].
///
/// On modern Windows the list is essentially never empty: the desktop
/// compositor (`dwm.exe`), `explorer.exe`, the browser, and any
/// running graphics-using application all surface through `PDH`'s
/// `GPU Process Memory` counter set.
#[test]
#[ignore = "requires Windows + WDDM 2.0+ GPU"]
#[allow(clippy::expect_used)]
fn gpu_processes_returns_pdh_rows() {
    let rows = gpu_processes(0).expect("gpu_processes(0) failed");
    assert!(
        !rows.is_empty(),
        "expected at least one GPU process under WDDM (compositor, browser, etc.); \
         got an empty list — PDH may not be the active backend"
    );
    for row in &rows {
        assert_eq!(
            row.source,
            GpuQuerySource::Pdh,
            "expected PDH source on Windows + pdh feature; got {:?} for pid {}",
            row.source,
            row.pid
        );
    }
}

/// Every returned row has a plausible `(pid, used_bytes)` shape:
///
/// - `pid > 0` (no zero PIDs).
/// - `used_bytes <= 1 TiB` (sanity bound — no GPU has > 1 TiB of
///   memory, so a per-process row exceeding it would indicate a unit
///   confusion or a sentinel leak).
///
/// Not asserting `used_bytes > 0` — `PDH` legitimately reports
/// `0` for some processes (just-registered, partially-released,
/// hmn.exe-style probe processes).
#[test]
#[ignore = "requires Windows + WDDM 2.0+ GPU"]
#[allow(clippy::expect_used)]
fn gpu_processes_returns_plausible_rows() {
    const ONE_TIB: u64 = 1024 * 1024 * 1024 * 1024;
    let rows = gpu_processes(0).expect("gpu_processes(0) failed");
    for row in &rows {
        assert!(row.pid > 0, "expected positive PID, got {}", row.pid);
        assert!(
            row.used_bytes <= ONE_TIB,
            "row for pid {} reports {} bytes — exceeds 1 TiB sanity bound, \
             likely a unit confusion or sentinel leak",
            row.pid,
            row.used_bytes,
        );
    }
}

/// At least one row has a resolved process name. `Win32`'s
/// `OpenProcess` + `QueryFullProcessImageNameW` succeeds for any
/// same-user process — and a typical Windows desktop runs many
/// same-user processes — so on a real machine the row count of
/// `name.is_some()` should be well into the double digits. The
/// assertion is conservatively `>= 1` to stay robust under unusual
/// elevated-only sessions.
///
/// Some rows may have `name.is_none()` — those are protected /
/// cross-user processes the calling user can't `OpenProcess` against.
/// That's documented behaviour, not a failure.
#[test]
#[ignore = "requires Windows + WDDM 2.0+ GPU"]
#[allow(clippy::expect_used)]
fn gpu_processes_resolves_at_least_one_name() {
    let rows = gpu_processes(0).expect("gpu_processes(0) failed");
    let resolved = rows.iter().filter(|r| r.name.is_some()).count();
    assert!(
        resolved >= 1,
        "expected at least one resolved process name in {} rows, got zero — \
         Win32 name lookup may be broken",
        rows.len()
    );
}

/// v0.2.8: every row's name is either resolved, or one of the
/// documented synthetic brackets (`[kernel]`, `[exited]`,
/// `[protected]`) — never a bare `None`/`?`. The `Toolhelp32Snapshot`
/// fallback in `resolve_names_via_snapshot` (wired into `gpu_processes`
/// via `resolve_unresolved_windows_names`) runs after the
/// `OpenProcess`-based fast path, so by the time rows reach the caller
/// every name should be `Some`. Not asserting *which* rows resolve to
/// real names vs. `[protected]` — that depends on what's running on the
/// live machine (foreign-user / `SYSTEM` / `PPL` processes holding GPU
/// memory) — only that the anonymous `?`/`None` case the pre-v0.2.8
/// dogfooding report flagged no longer reaches this point.
#[test]
#[ignore = "requires Windows + WDDM 2.0+ GPU"]
#[allow(clippy::expect_used)]
fn gpu_processes_never_leaves_a_row_unresolved() {
    let rows = gpu_processes(0).expect("gpu_processes(0) failed");
    for row in &rows {
        assert!(
            row.name.is_some(),
            "pid {} has name: None — Toolhelp32Snapshot fallback should have replaced this \
             with a real name, [exited], or [protected]",
            row.pid
        );
        assert_ne!(
            row.name.as_deref(),
            Some("?"),
            "pid {} still resolved to the literal \"?\" placeholder — expected a real name \
             or a synthetic bracket ([kernel]/[exited]/[protected])",
            row.pid
        );
    }
}

// -----------------------------------------------------------------------
// v0.2.5 spill-detection live tests
// -----------------------------------------------------------------------

/// On a live `WDDM 2.0`+ host the `GPU Adapter Memory` counter set is
/// registered, so the capability probe answers `true`.
#[test]
#[ignore = "requires Windows + WDDM 2.0+ GPU"]
fn live_is_spill_measurable_true_on_wddm() {
    assert!(
        is_spill_measurable(),
        "is_spill_measurable() should be true on a live WDDM 2.0+ host — \
         GPU Adapter Memory counter set may be unregistered"
    );
}

/// The automated benign-baseline acceptance test (the rhyme-mdlm
/// dogfooding regression, idle-desktop form): ~5 s of 100 ms polling
/// on a desktop that is *not* running a VRAM-saturating workload must
/// report **zero** spill episodes, even though shared usage sits at
/// its benign baseline (staging/upload heaps — ~134 MiB live on the
/// reference `RTX 5060 Ti`) and dedicated commit may exceed dedicated
/// `VRAM` elsewhere in the system. Run this on an idle-ish desktop;
/// a genuinely spilling workload running concurrently would rightly
/// fail it.
#[test]
#[ignore = "requires Windows + WDDM 2.0+ GPU"]
#[allow(clippy::expect_used)]
fn live_spill_tracker_idle_desktop_no_false_positive() {
    const TWO_GIB: u64 = 2 * 1024 * 1024 * 1024;
    const ONE_TIB: u64 = 1024 * 1024 * 1024 * 1024;

    let mut tracker = SpillTracker::new(0).expect("SpillTracker::new(0) failed on live host");
    assert!(
        tracker.is_measurable(),
        "tracker should be measurable on a live WDDM host"
    );
    for i in 0..50 {
        tracker.observe(format!("poll_{i}"));
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let report = tracker.into_report();

    assert!(report.measurable);
    assert!(
        report.observations >= 50,
        "expected ≥ 50 successful observations, got {}",
        report.observations
    );
    assert!(
        report.episodes.is_empty(),
        "idle desktop must not report spill episodes, got {} (baseline {} B, peak shared {} B)",
        report.episodes.len(),
        report.baseline_shared_bytes,
        report.peak_shared_bytes
    );
    assert!(
        report.baseline_shared_bytes < TWO_GIB,
        "benign shared baseline should be well under 2 GiB, got {} B",
        report.baseline_shared_bytes
    );
    assert!(
        report.dedicated_limit_bytes > 0,
        "DXGI dedicated capacity should resolve on a live NVIDIA host"
    );
    assert!(
        report.peak_dedicated_bytes <= ONE_TIB,
        "peak dedicated {} B exceeds the 1 TiB sanity bound",
        report.peak_dedicated_bytes
    );
}

/// Every per-process row's new `shared_used_bytes` is within the same
/// 1 TiB sanity bound as `used_bytes`, and the rows print for a
/// manual cross-check against Task Manager's *Shared GPU memory*
/// column (`cargo test ... -- --ignored --nocapture`).
#[test]
#[ignore = "requires Windows + WDDM 2.0+ GPU"]
#[allow(clippy::expect_used)]
fn live_process_shared_bytes_surface() {
    const ONE_TIB: u64 = 1024 * 1024 * 1024 * 1024;
    let rows = gpu_processes(0).expect("gpu_processes(0) failed");
    assert!(!rows.is_empty(), "expected PDH rows on a live WDDM host");
    for row in &rows {
        assert!(
            row.shared_used_bytes <= ONE_TIB,
            "row for pid {} reports {} shared bytes — exceeds 1 TiB sanity bound",
            row.pid,
            row.shared_used_bytes,
        );
        println!(
            "pid {:>6}  committed {:>14} B  shared {:>12} B  {}",
            row.pid,
            row.used_bytes,
            row.shared_used_bytes,
            row.name.as_deref().unwrap_or("?"),
        );
    }
}
