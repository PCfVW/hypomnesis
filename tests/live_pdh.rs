// SPDX-License-Identifier: MIT OR Apache-2.0

//! Live-PDH integration tests for the v0.2.2 Windows `PDH` per-process
//! `VRAM` backend. Every test is `#[ignore]`-gated and requires:
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

use hypomnesis::{GpuQuerySource, gpu_processes};

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
