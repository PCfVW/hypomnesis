// SPDX-License-Identifier: MIT OR Apache-2.0

//! Live-GPU integration tests. Every test is `#[ignore]`-gated — they
//! require an NVIDIA GPU + driver to be installed on the host. CI does
//! not run these (hosted runners have no NVIDIA hardware); run them
//! locally with:
//!
//! ```sh
//! cargo test -- --ignored
//! ```
//!
//! Recommended environments:
//!
//! - Windows host with the NVIDIA driver installed (exercises the
//!   `DXGI` per-process path + `NVML` device-wide path).
//! - Ubuntu (native or WSL2 with the CUDA-on-WSL driver) — exercises
//!   the `NVML` per-process path.

use hypomnesis::{Snapshot, device_count, device_info, gpu_processes, process_gpu_info};

#[test]
#[ignore = "requires NVIDIA GPU + driver"]
#[allow(clippy::expect_used)]
fn device_count_succeeds() {
    let count = device_count().expect("NVIDIA GPU + driver required");
    assert!(count >= 1, "expected at least one NVIDIA GPU, got {count}");
}

#[test]
#[ignore = "requires NVIDIA GPU + driver"]
#[allow(clippy::expect_used)]
fn device_info_returns_plausible_total_bytes() {
    let info = device_info(0).expect("device_info(0) requires NVIDIA GPU + driver");
    // Any modern NVIDIA card has at least 1 GiB of VRAM
    assert!(
        info.total_bytes >= 1024 * 1024 * 1024,
        "total_bytes={} (expected ≥ 1 GiB)",
        info.total_bytes
    );
    // And less than 1 TiB (sanity bound; even H100 = 80 GiB)
    assert!(
        info.total_bytes <= 1024_u64.pow(4),
        "total_bytes={} (expected ≤ 1 TiB)",
        info.total_bytes
    );
    // free_bytes <= total_bytes
    assert!(info.free_bytes <= info.total_bytes);
    // used_bytes <= total_bytes
    assert!(info.used_bytes <= info.total_bytes);
}

#[test]
#[ignore = "requires NVIDIA GPU + driver (R510+ for a populated reserved figure)"]
#[allow(clippy::expect_used)]
fn device_info_reserved_bytes_is_plausible_when_present() {
    let info = device_info(0).expect("device_info(0) requires NVIDIA GPU + driver");
    // `reserved_bytes` is `Some` only on the NVML path with an R510+
    // driver (where `nvmlDeviceGetMemoryInfo_v2` succeeds). When present,
    // the driver/firmware carve-out is a *subset* of `total_bytes` (NVML's
    // `total = reserved + free + used`): non-zero in practice on consumer
    // NVIDIA hardware, and a small slice of the card — strictly less than
    // `total_bytes`, which itself stays within the 1 TiB sanity bound.
    if let Some(reserved) = info.reserved_bytes {
        assert!(
            reserved > 0,
            "reserved_bytes={reserved} (expected a non-zero carve-out on R510+ NVML)"
        );
        assert!(
            reserved < info.total_bytes,
            "reserved_bytes={reserved} >= total_bytes={} (carve-out is a subset of total)",
            info.total_bytes
        );
        assert!(
            info.total_bytes <= 1024_u64.pow(4),
            "total_bytes={} (expected ≤ 1 TiB)",
            info.total_bytes
        );
    }
}

#[test]
#[ignore = "requires NVIDIA GPU + driver"]
#[allow(clippy::expect_used)]
fn device_info_driver_version_is_plausible_when_present() {
    let info = device_info(0).expect("device_info(0) requires NVIDIA GPU + driver");
    // `driver_version` is `Some` on the NVML and nvidia-smi paths. When
    // present, it's the NVIDIA-branded version string (e.g. "591.86"),
    // never empty and always containing at least one digit.
    if let Some(version) = info.driver_version {
        assert!(!version.is_empty(), "driver_version was Some(\"\")");
        assert!(
            version.chars().any(|c| c.is_ascii_digit()),
            "driver_version={version:?} (expected at least one digit)"
        );
    }
}

#[test]
#[ignore = "requires NVIDIA GPU + driver"]
#[allow(clippy::expect_used)]
fn snapshot_now_returns_ram_and_gpu_device() {
    let snap = Snapshot::now(0).expect("Snapshot::now failed");
    assert!(snap.ram_bytes > 0);
    // On a system with NVIDIA + driver, gpu_device should be populated
    assert!(
        snap.gpu_device.is_some(),
        "expected gpu_device to be populated on an NVIDIA-equipped host"
    );
}

/// `process_gpu_info` should succeed on a machine with NVIDIA + driver,
/// with platform-specific expected backends:
///
/// - **Windows + `DXGI`**: `WDDM` per-process path. `is_per_process = true`,
///   `source = Dxgi`. `CurrentUsage` may be 0 for a test binary that
///   hasn't allocated any `D3D` / `DXGI` memory itself.
/// - **Linux + `NVML`**: `nvmlDeviceGetComputeRunningProcesses_v3` only
///   lists processes with an **active CUDA context**. A vanilla test
///   binary has no CUDA context, so the dispatcher falls through to
///   `nvidia-smi` (device-wide). `is_per_process = false`,
///   `source = NvidiaSmi`. Verified on Ubuntu WSL2 with the
///   CUDA-on-WSL driver during v0.1.0 testing.
#[test]
#[ignore = "requires NVIDIA GPU + driver"]
#[allow(clippy::expect_used)]
fn process_gpu_info_returns_expected_source_per_platform() {
    let info = process_gpu_info(0).expect("process_gpu_info(0) requires NVIDIA GPU + driver");

    #[cfg(target_os = "windows")]
    {
        assert_eq!(info.source, hypomnesis::GpuQuerySource::Dxgi);
        assert!(info.is_per_process);
    }

    #[cfg(target_os = "linux")]
    {
        // For a non-CUDA test binary, nvidia-smi is the expected
        // fallback. If the binary somehow holds a CUDA context (rare
        // for unit tests), NVML would succeed and `is_per_process = true`.
        assert!(
            info.source == hypomnesis::GpuQuerySource::NvidiaSmi
                || info.source == hypomnesis::GpuQuerySource::Nvml,
            "expected NvidiaSmi or Nvml on Linux, got {:?}",
            info.source
        );
    }

    #[cfg(target_os = "macos")]
    {
        // On macOS the Metal backend is the per-process path — the
        // BSD ledger `graphics_footprint` entry for the calling PID.
        assert_eq!(info.source, hypomnesis::GpuQuerySource::Metal);
        assert!(info.is_per_process);
    }
}

/// Out-of-range index should yield `DeviceIndexOutOfRange` when at least
/// one count source (`NVML` or `DXGI`) reports a count.
#[test]
#[ignore = "requires NVIDIA GPU + driver"]
fn out_of_range_index_yields_device_index_error() {
    let result = device_info(255);
    assert!(
        matches!(
            result,
            Err(hypomnesis::HypomnesisError::DeviceIndexOutOfRange { .. })
        ),
        "expected DeviceIndexOutOfRange, got {result:?}"
    );
}

/// `Snapshot::all` should return at least one entry on a machine with
/// NVIDIA hardware. On Windows with an additional iGPU (Intel / AMD), a
/// second entry should appear after the NVIDIA one — the test is
/// length-tolerant so machines without an iGPU still pass.
#[test]
#[ignore = "requires NVIDIA GPU + driver"]
#[allow(clippy::expect_used)]
fn snapshot_all_enumerates_nvidia_and_optional_extras() {
    let snaps = Snapshot::all().expect("Snapshot::all failed on NVIDIA-equipped host");
    assert!(
        !snaps.is_empty(),
        "expected at least one GPU snapshot on an NVIDIA-equipped host"
    );

    // Every entry carries RAM and a populated gpu_device with a
    // monotonically increasing index starting at 0.
    for (expected_idx, snap) in snaps.iter().enumerate() {
        assert!(snap.ram_bytes > 0, "entry {expected_idx} ram_bytes == 0");
        let dev = snap
            .gpu_device
            .as_ref()
            .expect("Snapshot::all entry missing gpu_device");
        // CAST: usize → u32, expected_idx is bounded by snaps.len() which
        // never exceeds u32::MAX in any realistic system.
        #[allow(clippy::as_conversions, clippy::cast_possible_truncation)]
        let want = expected_idx as u32;
        assert_eq!(
            dev.index, want,
            "entry {expected_idx} has index {} (expected {want})",
            dev.index
        );
        assert!(
            dev.total_bytes > 0,
            "entry {expected_idx} total_bytes == 0 (name={:?})",
            dev.name
        );
    }

    // On Windows hosts that expose a non-NVIDIA `DXGI` adapter with
    // non-zero memory (e.g. an Intel / AMD iGPU alongside the NVIDIA
    // dGPU), the second entry should be the non-NVIDIA DXGI extra. The
    // maintainer's reference machine (Ryzen 9 5950X + RTX 5060 Ti) has
    // no iGPU on the CPU, so this branch isn't exercised here — it
    // waits on iGPU-equipped hardware or a contributor's PR for live
    // verification. Adapter name is best-effort.
    #[cfg(windows)]
    if let Some(extra) = snaps.get(1) {
        let dev = extra
            .gpu_device
            .as_ref()
            .expect("second Snapshot::all entry missing gpu_device");
        assert!(dev.index >= 1, "second entry index < 1");
        // The non-NVIDIA extra is reported via DXGI's per-process path.
        if let Some(p) = &extra.gpu {
            assert_eq!(p.source, hypomnesis::GpuQuerySource::Dxgi);
            assert!(p.is_per_process);
        }
    }
}

/// `gpu_processes` should succeed on a machine with an NVIDIA driver.
///
/// On a vanilla test binary with no CUDA context, the returned `Vec`
/// is typically empty (compute-only enumeration). The test is
/// length-tolerant — it asserts `Ok(...)` and validates whatever rows
/// exist, without requiring any to be present.
#[test]
#[ignore = "requires NVIDIA GPU + driver"]
#[allow(clippy::expect_used)]
fn gpu_processes_succeeds_on_live_host() {
    let rows = gpu_processes(0).expect("gpu_processes(0) failed on NVIDIA-equipped host");

    for row in &rows {
        assert!(row.pid > 0, "expected positive PID, got {}", row.pid);
        // Source on a live host is `Nvml` (Linux primary), `Pdh`
        // (Windows / WDDM 2.0+ primary, since v0.2.2), `Metal` (macOS
        // primary, since v0.2.3), or `NvidiaSmi` (fallback on any
        // platform). DXGI is never the source for an enumeration — it
        // only answers for the calling process. Mirrors the allow-list in
        // `tests/smoke.rs::gpu_processes_returns_result_or_no_gpu_source`.
        assert!(
            matches!(
                row.source,
                hypomnesis::GpuQuerySource::Nvml
                    | hypomnesis::GpuQuerySource::Pdh
                    | hypomnesis::GpuQuerySource::Metal
                    | hypomnesis::GpuQuerySource::NvidiaSmi
            ),
            "unexpected source {:?} for gpu_processes row",
            row.source
        );
        // PDH is *not* compute-only — it enumerates every process in the
        // Windows `GPU Process Memory` counter set, including ones holding
        // 0 dedicated bytes at the sampling instant (shared-only or
        // transient instances), so a 0 is legitimate on that path. The
        // compute-only sources (`Nvml`, `NvidiaSmi`) and macOS `Metal`
        // (which filters to footprint > 0) only surface rows with memory
        // actually held, so `used_bytes` stays positive there.
        if !matches!(row.source, hypomnesis::GpuQuerySource::Pdh) {
            assert!(
                row.used_bytes > 0,
                "process pid {} ({:?}) should have positive used_bytes",
                row.pid,
                row.source
            );
        }
    }
}
