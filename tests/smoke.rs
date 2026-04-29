// SPDX-License-Identifier: MIT OR Apache-2.0

//! Smoke test: verify the public API surface compiles, is reachable from
//! outside the crate, and that the always-available functions
//! (`process_rss`, `Snapshot::now`) succeed without an NVIDIA GPU.
//!
//! Live-GPU tests requiring NVIDIA hardware live in `tests/live_gpu.rs`
//! and are `#[ignore]`-gated.

use hypomnesis::{GpuDeviceInfo, GpuQuerySource, HypomnesisError, ProcessGpuInfo, Snapshot};

#[test]
fn public_types_are_reachable_via_crate_root() {
    let _: Option<GpuDeviceInfo> = None;
    let _: Option<ProcessGpuInfo> = None;
    let _: Option<Snapshot> = None;
    let _: GpuQuerySource = GpuQuerySource::Dxgi;
    let _: GpuQuerySource = GpuQuerySource::Nvml;
    let _: GpuQuerySource = GpuQuerySource::NvidiaSmi;
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
