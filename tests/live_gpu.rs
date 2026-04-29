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

use hypomnesis::{Snapshot, device_count, device_info, process_gpu_info};

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
