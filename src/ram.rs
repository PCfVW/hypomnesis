// SPDX-License-Identifier: MIT OR Apache-2.0

//! Process `RAM` (`RSS`) measurement.
//!
//! - **Windows**: `K32GetProcessMemoryInfo` → `WorkingSetSize` (exact, per-process).
//! - **Linux**: `/proc/self/status` → `VmRSS` (exact, per-process, no `unsafe`).

use crate::{HypomnesisError, Result};

/// Query the current process's resident set size (`RSS`) in bytes.
///
/// Returns the per-process resident-set figure as reported by the operating
/// system: working-set bytes on Windows, `VmRSS` on Linux. On other
/// targets this returns an error.
///
/// # Errors
///
/// Returns [`HypomnesisError::Ram`] if the platform API call fails or — on
/// Linux — if `/proc/self/status` cannot be read or its `VmRSS` line cannot
/// be parsed.
pub fn process_rss() -> Result<u64> {
    #[cfg(target_os = "windows")]
    {
        windows_rss()
    }
    #[cfg(target_os = "linux")]
    {
        linux_rss()
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        Err(HypomnesisError::Ram(
            "RAM measurement not supported on this platform".into(),
        ))
    }
}

// ---------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------

/// Windows FFI types and functions for `K32GetProcessMemoryInfo`.
#[cfg(target_os = "windows")]
mod win_ffi {
    /// `PROCESS_MEMORY_COUNTERS` structure from the Windows API.
    ///
    /// See: <https://learn.microsoft.com/en-us/windows/win32/api/psapi/ns-psapi-process_memory_counters>
    #[repr(C)]
    pub(super) struct ProcessMemoryCounters {
        /// Size of this structure in bytes.
        pub cb: u32,
        /// Number of page faults.
        pub page_fault_count: u32,
        /// Peak working set size in bytes.
        pub peak_working_set_size: usize,
        /// Current working set size in bytes (= `RSS`).
        pub working_set_size: usize,
        /// Peak paged pool usage in bytes.
        pub quota_peak_paged_pool_usage: usize,
        /// Current paged pool usage in bytes.
        pub quota_paged_pool_usage: usize,
        /// Peak non-paged pool usage in bytes.
        pub quota_peak_non_paged_pool_usage: usize,
        /// Current non-paged pool usage in bytes.
        pub quota_non_paged_pool_usage: usize,
        /// Current pagefile usage in bytes.
        pub pagefile_usage: usize,
        /// Peak pagefile usage in bytes.
        pub peak_pagefile_usage: usize,
    }

    // SAFETY: These are stable Windows API functions with well-defined ABI.
    // GetCurrentProcess always returns a valid pseudo-handle.
    // K32GetProcessMemoryInfo writes to caller-provided memory of known size.
    #[allow(unsafe_code)]
    unsafe extern "system" {
        /// Returns a pseudo-handle to the current process (always valid, never null).
        ///
        /// See: <https://learn.microsoft.com/en-us/windows/win32/api/processthreadsapi/nf-processthreadsapi-getcurrentprocess>
        pub(super) safe fn GetCurrentProcess() -> isize;

        /// Retrieves memory usage information for the specified process.
        ///
        /// See: <https://learn.microsoft.com/en-us/windows/win32/api/psapi/nf-psapi-k32getprocessmemoryinfo>
        pub(super) unsafe fn K32GetProcessMemoryInfo(
            process: isize,
            ppsmem_counters: *mut ProcessMemoryCounters,
            cb: u32,
        ) -> i32;
    }
}

/// Query `RSS` on Windows via `K32GetProcessMemoryInfo`.
#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
fn windows_rss() -> Result<u64> {
    let mut counters = win_ffi::ProcessMemoryCounters {
        cb: 0,
        page_fault_count: 0,
        peak_working_set_size: 0,
        working_set_size: 0,
        quota_peak_paged_pool_usage: 0,
        quota_paged_pool_usage: 0,
        quota_peak_non_paged_pool_usage: 0,
        quota_non_paged_pool_usage: 0,
        pagefile_usage: 0,
        peak_pagefile_usage: 0,
    };
    // CAST: usize → u32, struct size is 80 bytes on x64 — fits in u32
    #[allow(clippy::as_conversions, clippy::cast_possible_truncation)]
    let cb = std::mem::size_of::<win_ffi::ProcessMemoryCounters>() as u32;
    counters.cb = cb;

    let handle = win_ffi::GetCurrentProcess();

    // SAFETY: K32GetProcessMemoryInfo writes into the stack-allocated
    // `counters` struct, which is correctly sized (the cb field above is
    // set to the struct's byte size). The process handle from
    // GetCurrentProcess is a pseudo-handle that is always valid for the
    // lifetime of the process.
    let ok = unsafe { win_ffi::K32GetProcessMemoryInfo(handle, &raw mut counters, cb) };

    if ok != 0 {
        // CAST: usize → u64, working-set size in bytes — always fits
        #[allow(clippy::as_conversions)]
        let rss = counters.working_set_size as u64;
        Ok(rss)
    } else {
        Err(HypomnesisError::Ram(
            "K32GetProcessMemoryInfo failed".into(),
        ))
    }
}

// ---------------------------------------------------------------------------
// Linux
// ---------------------------------------------------------------------------

/// Query `RSS` on Linux via `/proc/self/status`.
#[cfg(target_os = "linux")]
fn linux_rss() -> Result<u64> {
    let status = std::fs::read_to_string("/proc/self/status")
        .map_err(|e| HypomnesisError::Ram(format!("failed to read /proc/self/status: {e}")))?;
    parse_vmrss(&status)
}

/// Parse the `VmRSS` line out of `/proc/self/status` content.
///
/// Extracted from [`linux_rss`] for unit-testability — the parsing logic
/// works on any string formatted like `/proc/self/status`. Returns the
/// resident-set size in bytes (the file reports kilobytes).
///
/// # Errors
///
/// Returns [`HypomnesisError::Ram`] if no `VmRSS:` line is found, or if
/// the kilobyte value on that line is not parseable as `u64`.
#[cfg(target_os = "linux")]
fn parse_vmrss(status: &str) -> Result<u64> {
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmRSS:") {
            let kb_str = rest.trim().trim_end_matches(" kB").trim();
            let kb: u64 = kb_str.parse().map_err(|e| {
                HypomnesisError::Ram(format!("failed to parse VmRSS value '{kb_str}': {e}"))
            })?;
            return Ok(kb * 1024);
        }
    }
    Err(HypomnesisError::Ram(
        "VmRSS not found in /proc/self/status".into(),
    ))
}

#[cfg(all(test, target_os = "linux"))]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::missing_docs_in_private_items
)]
mod tests {
    use super::*;

    #[test]
    fn parse_vmrss_basic() {
        let status = "Name:\tcat\nVmPeak:\t  50000 kB\nVmRSS:\t   12345 kB\nThreads:\t1\n";
        let rss = parse_vmrss(status).unwrap();
        assert_eq!(rss, 12_345 * 1024);
    }

    #[test]
    fn parse_vmrss_zero() {
        let status = "VmRSS:\t       0 kB\n";
        let rss = parse_vmrss(status).unwrap();
        assert_eq!(rss, 0);
    }

    #[test]
    fn parse_vmrss_no_kb_suffix() {
        // The trim_end_matches(" kB") branch is a no-op here; the parse
        // should still succeed treating the trailing token as the number.
        let status = "VmRSS:\t   42\n";
        let rss = parse_vmrss(status).unwrap();
        assert_eq!(rss, 42 * 1024);
    }

    #[test]
    fn parse_vmrss_missing() {
        let status = "Name:\tcat\nVmPeak:\t  50000 kB\nThreads:\t1\n";
        let err = parse_vmrss(status).unwrap_err();
        assert!(err.to_string().contains("VmRSS not found"));
    }

    #[test]
    fn parse_vmrss_unparseable() {
        let status = "Name:\tcat\nVmRSS:\tnot_a_number kB\n";
        let err = parse_vmrss(status).unwrap_err();
        assert!(err.to_string().contains("failed to parse"));
    }

    #[test]
    fn parse_vmrss_real_proc_status() {
        // Excerpt of a real /proc/self/status (Linux 6.x). Only the VmRSS
        // line matters for the parser; the rest exercises the line-skip path.
        let status = "Name:\tbash\n\
            Umask:\t0022\n\
            State:\tS (sleeping)\n\
            Tgid:\t12345\n\
            VmPeak:\t   12000 kB\n\
            VmSize:\t   11000 kB\n\
            VmLck:\t       0 kB\n\
            VmRSS:\t    4096 kB\n\
            VmData:\t     500 kB\n\
            Threads:\t1\n";
        let rss = parse_vmrss(status).unwrap();
        assert_eq!(rss, 4096 * 1024);
    }
}
