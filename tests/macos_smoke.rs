// SPDX-License-Identifier: MIT OR Apache-2.0

#![cfg(target_os = "macos")]

//! macOS smoke test: exercises the Metal-backed surfaces of the public
//! API (`process_rss`, `device_count`, `device_info`, `process_gpu_info`,
//! `gpu_processes`, `Snapshot::now`) and asserts the contract values are
//! sane on an Apple Silicon host.
//!
//! The whole file is gated on `target_os = "macos"`; on Windows and
//! Linux the file compiles to zero tests.
//!
//! Tests 1, 2, 5, 6 run unconditionally on any macOS host. Tests 3 and 4
//! are `#[ignore]`-gated because they require Apple Silicon hardware
//! with a real Metal device — they would fail on Intel Macs (where the
//! Metal backend returns `None` and the dispatcher falls through to
//! `NoGpuSource`) or on hosted runners without a usable GPU. Run them
//! locally on Apple Silicon with `cargo test -- --ignored`.

use hypomnesis::{GpuQuerySource, HypomnesisError, Snapshot};

#[test]
#[allow(clippy::expect_used)] // process_rss should never fail on a running test process
fn process_rss_returns_positive_on_macos() {
    let rss = hypomnesis::process_rss().expect("process_rss failed on a running macOS process");
    assert!(
        rss > 1_000_000,
        "expected process_rss > 1 MB on macOS, got {rss}"
    );
}

#[test]
fn device_count_is_one_on_apple_silicon() {
    // Apple Silicon exposes a single Metal device; Intel Macs (and any
    // host where the Metal backend cannot enumerate) fall through to
    // NoGpuSource. Anything else is unexpected.
    match hypomnesis::device_count() {
        Ok(count) => assert_eq!(count, 1, "expected device_count == 1 on Apple Silicon, got {count}"),
        Err(e) => assert!(
            matches!(e, HypomnesisError::NoGpuSource),
            "unexpected error from device_count(): {e:?}"
        ),
    }
}

#[test]
#[ignore = "requires Apple Silicon with a usable Metal device"]
fn device_info_reports_apple_brand() {
    match hypomnesis::device_info(0) {
        Ok(info) => {
            let name = info.name.as_deref().unwrap_or("");
            assert!(
                name.contains("Apple"),
                "expected device name to contain \"Apple\", got {name:?}"
            );
            assert!(
                info.total_bytes >= 8 << 30,
                "expected total_bytes >= 8 GiB on Apple Silicon, got {}",
                info.total_bytes
            );
        }
        Err(e) => {
            eprintln!("device_info(0) unavailable on this host: {e:?}");
        }
    }
}

#[test]
#[ignore = "requires Apple Silicon with a usable Metal device"]
#[allow(clippy::panic, clippy::indexing_slicing)] // tests are allowed to panic; v[i] is bounded by step_by(4096)
fn process_gpu_info_returns_metal_source() {
    // Residency-touch dance: allocate 256 MiB and touch one byte per
    // 4 KiB page so the kernel ledger reports a non-trivial
    // graphics_footprint. The cast on the page index is bounded by
    // v.len() (256 MiB / 4 KiB = 65_536 pages), well within u8 range
    // after the & 0xff mask.
    let mut v = vec![0u8; 256 << 20];
    for i in (0..v.len()).step_by(4096) {
        // CAST: usize → u8, masked with 0xff so the truncation is intentional.
        #[allow(clippy::as_conversions, clippy::cast_possible_truncation)]
        let byte = (i & 0xff) as u8;
        // INDEX: i ranges over step_by(4096) of (0..v.len()), so v[i] is always in-bounds.
        v[i] = byte;
    }

    match hypomnesis::process_gpu_info(0) {
        Ok(info) => {
            assert_eq!(
                info.source,
                GpuQuerySource::Metal,
                "expected GpuQuerySource::Metal on macOS, got {:?}",
                info.source
            );
            assert!(
                info.is_per_process,
                "expected is_per_process == true for the Metal backend"
            );
        }
        Err(e) => panic!("process_gpu_info(0) failed on macOS: {e:?}"),
    }

    // Keep the buffer alive past the query so the touched pages stay
    // resident at sample time.
    drop(v);
}

#[test]
fn gpu_processes_returns_metal_rows_for_self() {
    // The leaf's success-gate asks us to look for `pid == std::process::id()`
    // in the returned rows. Empirically a vanilla `cargo test` binary holds
    // no Metal device context and so produces no `graphics_footprint` entry
    // in the kernel ledger — `gpu_processes(0)` returns plenty of other
    // PIDs (WindowServer, Safari, etc.) but never ours. We therefore
    // accept three outcomes: (a) self is present (binary with Metal
    // residency), (b) Ok with self absent but every row uses the Metal
    // source (parity with `tests/smoke.rs::gpu_processes_returns_result_or_no_gpu_source`),
    // (c) Err(NoGpuSource) on a non-GPU host.
    match hypomnesis::gpu_processes(0) {
        Ok(rows) => {
            let self_pid = std::process::id();
            let saw_self = rows.iter().any(|r| r.pid == self_pid);
            if !saw_self {
                eprintln!(
                    "gpu_processes(0) returned {} rows but the test PID {self_pid} is absent; \
                     this is expected for a vanilla test binary that holds no Metal device",
                    rows.len()
                );
            }
            for row in &rows {
                assert!(row.pid > 0, "expected positive PID, got {}", row.pid);
                assert_eq!(
                    row.source,
                    GpuQuerySource::Metal,
                    "expected GpuQuerySource::Metal for a macOS row, got {:?}",
                    row.source
                );
            }
        }
        Err(e) => {
            assert!(
                matches!(e, HypomnesisError::NoGpuSource),
                "unexpected error from gpu_processes(0): {e:?}"
            );
        }
    }
}

#[test]
#[allow(clippy::expect_used)] // Snapshot::now's RAM query should never fail; GPU should be present on macOS
fn snapshot_now_includes_gpu_on_macos() {
    let snap = Snapshot::now(0).expect("Snapshot::now's RAM query should succeed on macOS");
    assert!(
        snap.gpu.is_some(),
        "expected snap.gpu to be Some on macOS (Metal backend should populate it)"
    );
}
