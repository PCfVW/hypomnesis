// SPDX-License-Identifier: MIT OR Apache-2.0

//! Smoke test: verify the public API surface compiles, is reachable from
//! outside the crate, and that the always-available entry points
//! (`process_rss`, `Snapshot::now`, `Snapshot::all`) succeed without an
//! NVIDIA GPU. `Snapshot::all` returns an empty `Vec` when no GPUs are
//! visible — callers wanting RAM-only state should use `process_rss` or
//! `Snapshot::now` instead.
//!
//! Live-GPU tests requiring NVIDIA hardware live in `tests/live_gpu.rs`
//! and are `#[ignore]`-gated.

use hypomnesis::{
    GpuDeviceInfo, GpuProcessEntry, GpuQuerySource, HypomnesisError, ProcessGpuInfo, Snapshot,
    SpillEpisode, SpillReport, SpillTracker,
};

#[test]
fn public_types_are_reachable_via_crate_root() {
    let _: Option<GpuDeviceInfo> = None;
    let _: Option<ProcessGpuInfo> = None;
    let _: Option<GpuProcessEntry> = None;
    let _: Option<Snapshot> = None;
    let _: Option<SpillEpisode> = None;
    let _: Option<SpillReport> = None;
    let _: Option<SpillTracker> = None;
    let _: GpuQuerySource = GpuQuerySource::Dxgi;
    let _: GpuQuerySource = GpuQuerySource::Nvml;
    let _: GpuQuerySource = GpuQuerySource::NvidiaSmi;
    let _: GpuQuerySource = GpuQuerySource::Metal;
    let _: HypomnesisError = HypomnesisError::NoGpuSource;
}

#[cfg(feature = "report")]
#[test]
fn report_feature_types_are_reachable() {
    let _: Option<hypomnesis::MemoryReport> = None;
}

#[test]
#[allow(clippy::expect_used)] // process_rss should never fail on a running test process
fn process_rss_returns_positive() {
    let rss = hypomnesis::process_rss().expect("process_rss failed on a running process");
    assert!(rss > 0, "process_rss should be positive, got {rss}");
}

#[test]
#[allow(clippy::expect_used)] // Snapshot::now's RAM query should never fail; GPU failures are non-fatal
fn snapshot_now_returns_ram_without_gpu() {
    // GPU calls inside Snapshot::now are wrapped in .ok() — they may
    // return None on a runner without an NVIDIA driver, which is the
    // expected case in CI. Snapshot::now should still succeed.
    let snap = Snapshot::now(0).expect("Snapshot::now's RAM query should succeed");
    assert!(
        snap.ram_bytes > 0,
        "Snapshot::now should return positive RAM, got {}",
        snap.ram_bytes
    );
    // gpu / gpu_device are best-effort; we don't assert on them in the
    // CI path because hosted runners typically lack NVIDIA hardware.
}

#[test]
#[allow(clippy::expect_used)] // RAM query should never fail; GPU absence yields an empty Vec
fn snapshot_all_returns_ok_and_carries_ram() {
    // On a runner without NVIDIA / DXGI extras, Snapshot::all() returns
    // an empty Vec (RAM is captured but discarded — callers wanting
    // RAM-only state should use process_rss or Snapshot::now). We assert
    // on shape only so the test passes on hosted runners and on
    // hardware alike.
    let snaps = Snapshot::all().expect("Snapshot::all's RAM query should succeed");
    for snap in &snaps {
        assert!(
            snap.ram_bytes > 0,
            "every Snapshot::all entry should carry positive RAM, got {}",
            snap.ram_bytes
        );
    }
}

#[test]
fn is_spill_measurable_is_callable_everywhere() {
    // The capability probe must be callable on every platform. Its
    // value is platform-dependent (true only on Windows / WDDM 2.0+
    // with the pdh feature), so the only universal assertion is the
    // off-Windows one.
    let measurable = hypomnesis::is_spill_measurable();
    #[cfg(not(windows))]
    assert!(
        !measurable,
        "is_spill_measurable() must be false off-Windows"
    );
    #[cfg(windows)]
    let _ = measurable; // true or false depending on the runner's GPU stack
}

#[test]
fn spill_tracker_constructs_or_errs_cleanly() {
    // Portable-consumer contract: SpillTracker::new must either
    // construct (possibly non-measurable) or return the documented Pdh
    // error — on ANY host, GPU or not. Shape-only assertions so this
    // passes on hosted CI runners and on hardware alike.
    match SpillTracker::new(0) {
        Ok(mut tracker) => {
            tracker.observe("smoke");
            // Cheap queries must be callable regardless of measurability.
            let _ = tracker.is_spilling();
            let _ = tracker.has_spilled();
            let measurable = tracker.is_measurable();
            let report = tracker.into_report();
            assert_eq!(report.measurable, measurable);
            assert!(
                report.observations <= 1,
                "one observe() call cannot yield more than one observation, got {}",
                report.observations
            );
            if !measurable {
                assert_eq!(
                    report.observations, 0,
                    "a non-measurable tracker must record no observations"
                );
                assert!(!report.spilled());
            }
        }
        Err(e) => {
            // Windows hard-failure path (no adapter at index 0 /
            // query open failure) — must be the documented variant.
            assert!(
                matches!(e, HypomnesisError::Pdh(_)),
                "unexpected error from SpillTracker::new(0): {e:?}"
            );
        }
    }
}

#[cfg(not(windows))]
#[test]
fn gpu_process_entry_shared_bytes_zero_off_windows() {
    // The shared_used_bytes contract: populated only on the Windows
    // PDH path, 0 everywhere else.
    if let Ok(rows) = hypomnesis::gpu_processes(0) {
        for row in &rows {
            assert_eq!(
                row.shared_used_bytes, 0,
                "shared_used_bytes must be 0 off-Windows, got {} for pid {}",
                row.shared_used_bytes, row.pid
            );
        }
    }
}

#[test]
fn gpu_processes_returns_result_or_no_gpu_source() {
    // On a runner without NVIDIA / nvidia-smi, gpu_processes(0) typically
    // returns Err(NoGpuSource) (or DeviceIndexOutOfRange when bounds_check
    // catches a count source). Either is acceptable. On a host with
    // NVIDIA, returns Ok(Vec) — possibly empty on Linux (NVML compute-only,
    // no CUDA process active) or essentially never empty on Windows (PDH
    // surfaces every GPU memory holder, compositor included). We assert
    // on shape only so the test passes on hosted runners and on hardware
    // alike.
    match hypomnesis::gpu_processes(0) {
        Ok(rows) => {
            for row in &rows {
                // PID 0 is reserved on every platform we support — the
                // Linux/Windows scheduler "swapper" and the macOS `kernel_task`
                // entry would never surface through a userland enumerator like
                // NVML, PDH, nvidia-smi, or the macOS ledger. Sanity-check.
                assert!(row.pid > 0, "expected positive PID, got {}", row.pid);
                // Source must be one of the enumerable backends —
                // DXGI cannot enumerate other PIDs; NVML / nvidia-smi
                // enumerate NVIDIA processes; PDH enumerates Windows
                // VidMm-tracked GPU memory holders; Metal enumerates
                // same-user PIDs via the macOS kernel ledger.
                assert!(
                    matches!(
                        row.source,
                        GpuQuerySource::Nvml
                            | GpuQuerySource::Pdh
                            | GpuQuerySource::NvidiaSmi
                            | GpuQuerySource::Metal
                    ),
                    "unexpected source {:?} on a gpu_processes row",
                    row.source
                );
            }
        }
        Err(e) => {
            // Expected on hosted runners with no NVIDIA hardware.
            assert!(
                matches!(
                    e,
                    HypomnesisError::NoGpuSource | HypomnesisError::DeviceIndexOutOfRange { .. }
                ),
                "unexpected error from gpu_processes(0): {e:?}"
            );
        }
    }
}
