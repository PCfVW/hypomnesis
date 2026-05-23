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
};

#[test]
fn public_types_are_reachable_via_crate_root() {
    let _: Option<GpuDeviceInfo> = None;
    let _: Option<ProcessGpuInfo> = None;
    let _: Option<GpuProcessEntry> = None;
    let _: Option<Snapshot> = None;
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
fn gpu_processes_returns_result_or_no_gpu_source() {
    // On a runner without NVIDIA / nvidia-smi, gpu_processes(0) typically
    // returns Err(NoGpuSource) (or DeviceIndexOutOfRange when bounds_check
    // catches a count source). Either is acceptable. On a host with
    // NVIDIA, returns Ok(Vec) — possibly empty if no CUDA processes are
    // active. We assert on shape only so the test passes on hosted
    // runners and on hardware alike.
    match hypomnesis::gpu_processes(0) {
        Ok(rows) => {
            for row in &rows {
                // PIDs of 0 would be impossible on Linux/Windows; sanity-check.
                assert!(row.pid > 0, "expected positive PID, got {}", row.pid);
                // Source must be one of the enumerable backends —
                // DXGI cannot enumerate other PIDs; NVML / nvidia-smi enumerate
                // NVIDIA processes; Metal enumerates same-user PIDs via the
                // macOS kernel ledger.
                assert!(
                    matches!(
                        row.source,
                        GpuQuerySource::Nvml
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
