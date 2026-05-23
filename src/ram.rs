// SPDX-License-Identifier: MIT OR Apache-2.0

//! Process `RAM` (`RSS`) measurement.
//!
//! - **Windows**: `K32GetProcessMemoryInfo` → `WorkingSetSize` (exact, per-process).
//! - **Linux**: `/proc/self/status` → `VmRSS` (exact, per-process, no `unsafe`).
//! - **macOS**: `task_info(TASK_VM_INFO_PURGEABLE).phys_footprint` via libSystem
//!   (exact, per-process; the kernel ledger figure backing Activity Monitor's
//!   "Memory" column).

use crate::{HypomnesisError, Result};

/// Query the current process's resident set size (`RSS`) in bytes.
///
/// Returns the per-process resident-set figure as reported by the operating
/// system: working-set bytes on Windows, `VmRSS` on Linux, `phys_footprint`
/// on macOS. On other targets this returns an error.
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
    #[cfg(target_os = "macos")]
    {
        macos_rss()
    }
    #[cfg(not(any(target_os = "windows", target_os = "linux", target_os = "macos")))]
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
    // GetLastError reads thread-local storage; safe to call any time.
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

        /// Returns the calling thread's last-error code.
        ///
        /// See: <https://learn.microsoft.com/en-us/windows/win32/api/errhandlingapi/nf-errhandlingapi-getlasterror>
        pub(super) safe fn GetLastError() -> u32;
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
        let code = win_ffi::GetLastError();
        Err(HypomnesisError::Ram(format!(
            "K32GetProcessMemoryInfo failed (GetLastError = {code})"
        )))
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

// ---------------------------------------------------------------------------
// macOS
// ---------------------------------------------------------------------------

/// macOS FFI types and functions for `task_info(TASK_VM_INFO_PURGEABLE)`.
///
/// libSystem-only — no third-party Apple-framework crate. Mirrors the Mach
/// kernel interface in <`https://github.com/apple-oss-distributions/xnu`>
/// `osfmk/mach/task_info.h`. The struct laid out below matches the
/// kernel-side `struct task_vm_info` through the rev3 ledger fields; the
/// kernel writes only up to the caller-requested count in `u32`-words, so
/// older kernels safely write a prefix and leave the tail untouched
/// (zero-initialised on our side).
#[cfg(target_os = "macos")]
mod darwin_ffi {
    /// `task_vm_info_data_t` from `<mach/task_info.h>` (XNU).
    ///
    /// Fields are laid out in declaration order per the kernel header. The
    /// fields critical to RAM accounting are `phys_footprint` (the kernel
    /// ledger figure Activity Monitor displays as "Memory") and the count
    /// constants below for ABI-versioned `task_info` calls.
    ///
    /// See: <https://github.com/apple-oss-distributions/xnu/blob/main/osfmk/mach/task_info.h>
    #[repr(C)]
    pub(super) struct TaskVmInfo {
        /// Total virtual address space size in bytes.
        pub virtual_size: u64,
        /// Number of memory regions in the task's address space.
        pub region_count: i32,
        /// Page size for the task, in bytes.
        pub page_size: i32,
        /// Resident set size in bytes (Mach-level, pre-footprint accounting).
        pub resident_size: u64,
        /// Peak resident set size in bytes.
        pub resident_size_peak: u64,
        /// Bytes mapped from device memory (e.g. GPU shared regions).
        pub device: u64,
        /// Peak device bytes.
        pub device_peak: u64,
        /// Internal (anonymous) bytes.
        pub internal: u64,
        /// Peak internal bytes.
        pub internal_peak: u64,
        /// External (file-backed) bytes.
        pub external: u64,
        /// Peak external bytes.
        pub external_peak: u64,
        /// Reusable bytes (e.g. madvise-freeable).
        pub reusable: u64,
        /// Peak reusable bytes.
        pub reusable_peak: u64,
        /// Purgeable-volatile bytes in the pmap.
        pub purgeable_volatile_pmap: u64,
        /// Purgeable-volatile resident bytes.
        pub purgeable_volatile_resident: u64,
        /// Purgeable-volatile virtual bytes.
        pub purgeable_volatile_virtual: u64,
        /// Compressed bytes.
        pub compressed: u64,
        /// Peak compressed bytes.
        pub compressed_peak: u64,
        /// Lifetime compressed bytes.
        pub compressed_lifetime: u64,
        // rev1
        /// Physical footprint in bytes — the kernel ledger figure used by
        /// Activity Monitor's "Memory" column. This is the field
        /// [`super::macos_rss`] returns.
        pub phys_footprint: u64,
        // rev2
        /// Smallest address mapped in the task.
        pub min_address: u64,
        /// Largest address mapped in the task.
        pub max_address: u64,
        // rev3 (ledger fields)
        /// Peak physical footprint, ledger-recorded.
        pub ledger_phys_footprint_peak: i64,
        /// Purgeable non-volatile bytes (ledger).
        pub ledger_purgeable_nonvolatile: i64,
        /// Purgeable non-volatile compressed bytes (ledger).
        pub ledger_purgeable_novolatile_compressed: i64,
        /// Purgeable volatile bytes (ledger).
        pub ledger_purgeable_volatile: i64,
        /// Purgeable volatile compressed bytes (ledger).
        pub ledger_purgeable_volatile_compressed: i64,
        /// Network non-volatile bytes (ledger).
        pub ledger_tag_network_nonvolatile: i64,
        /// Network non-volatile compressed bytes (ledger).
        pub ledger_tag_network_nonvolatile_compressed: i64,
        /// Network volatile bytes (ledger).
        pub ledger_tag_network_volatile: i64,
        /// Network volatile compressed bytes (ledger).
        pub ledger_tag_network_volatile_compressed: i64,
        /// Media footprint bytes (ledger).
        pub ledger_tag_media_footprint: i64,
        /// Media footprint compressed bytes (ledger).
        pub ledger_tag_media_footprint_compressed: i64,
        /// Media no-footprint bytes (ledger).
        pub ledger_tag_media_nofootprint: i64,
        /// Media no-footprint compressed bytes (ledger).
        pub ledger_tag_media_nofootprint_compressed: i64,
        /// Graphics footprint bytes (ledger).
        pub ledger_tag_graphics_footprint: i64,
        /// Graphics footprint compressed bytes (ledger).
        pub ledger_tag_graphics_footprint_compressed: i64,
        /// Neural footprint bytes (ledger).
        pub ledger_tag_neural_footprint: i64,
        /// Neural footprint compressed bytes (ledger).
        pub ledger_tag_neural_footprint_compressed: i64,
    }

    /// `task_info` flavor selector for the purgeable VM info variant.
    ///
    /// In XNU `<mach/task_info.h>` both `TASK_VM_INFO` and
    /// `TASK_VM_INFO_PURGEABLE` are defined to the same value (22). The
    /// purgeable variant fills the `purgeable_volatile_*` fields; the
    /// regular variant zeroes them. The numeric flavor is the same.
    pub(super) const TASK_VM_INFO_PURGEABLE: u32 = 22;

    /// Number of `u32` words in [`TaskVmInfo`].
    ///
    /// `task_info` reports counts in `mach_msg_type_number_t` units (each a
    /// 32-bit word). Computed at compile time so the constant tracks any
    /// future field addition.
    // CAST: usize → u32, struct size is at most ~320 bytes — fits in u32
    #[allow(clippy::as_conversions, clippy::cast_possible_truncation)]
    pub(super) const TASK_VM_INFO_PURGEABLE_COUNT: u32 =
        (size_of::<TaskVmInfo>() / size_of::<u32>()) as u32;

    /// `KERN_SUCCESS` from `<mach/kern_return.h>` (XNU).
    pub(super) const KERN_SUCCESS: i32 = 0;

    // SAFETY: These are stable libSystem (Mach kernel) entry points with
    // well-defined C ABI. `mach_task_self` is documented to always return
    // the calling task's port — never fails, never returns an invalid port
    // — and is therefore safe to mark `safe` in the Rust 2024 idiom.
    // `task_info` takes a raw out-pointer + length-cell pointer; the safety
    // contract is upheld at the call site in `super::macos_rss`.
    #[allow(unsafe_code)]
    unsafe extern "C" {
        /// Returns the Mach port representing the calling task.
        ///
        /// See: <https://github.com/apple-oss-distributions/xnu/blob/main/osfmk/mach/mach_init.h>
        pub(super) safe fn mach_task_self() -> u32;

        /// Retrieves information about the specified task.
        ///
        /// `task_info_outCnt` is in/out: callers set it to the buffer
        /// capacity in 32-bit words; the kernel overwrites it with the
        /// number of words actually written.
        ///
        /// See: <https://github.com/apple-oss-distributions/xnu/blob/main/osfmk/mach/task.h>
        #[allow(non_snake_case)]
        pub(super) unsafe fn task_info(
            target_task: u32,
            flavor: u32,
            task_info_out: *mut u32,
            task_info_outCnt: *mut u32,
        ) -> i32;
    }
}

/// Query `RSS` on macOS via `task_info(TASK_VM_INFO_PURGEABLE).phys_footprint`.
#[cfg(target_os = "macos")]
#[allow(unsafe_code)]
fn macos_rss() -> Result<u64> {
    // Stack-allocate the struct zero-initialised. `#[repr(C)]` + all-POD
    // numeric fields makes this a sound bit-pattern; the kernel will
    // overwrite the prefix it understands and we read only `phys_footprint`,
    // which has been present since macOS 10.11.
    // SAFETY: `TaskVmInfo` is `#[repr(C)]` and contains only integer fields
    // (u64 / i64 / i32). All-zero is a valid bit pattern for each.
    let mut info: darwin_ffi::TaskVmInfo = unsafe { core::mem::zeroed() };
    let mut count: u32 = darwin_ffi::TASK_VM_INFO_PURGEABLE_COUNT;

    let task = darwin_ffi::mach_task_self();

    // SAFETY: `task` is a valid Mach task port returned by `mach_task_self`,
    // which the kernel guarantees is always the calling task. The out-buffer
    // pointer is derived from `&raw mut info` and is well-aligned, non-null,
    // and lives for the duration of the call. `&raw mut count` likewise
    // points to a live `u32` on this stack frame; its initial value
    // (`TASK_VM_INFO_PURGEABLE_COUNT`) tells the kernel the buffer capacity
    // in 32-bit words, exactly matching `size_of::<TaskVmInfo>()`. The
    // kernel writes at most that many words and then sets `count` to the
    // number actually written.
    // CAST: &raw mut TaskVmInfo → *mut u32, the kernel ABI treats the
    // out-buffer as an array of 32-bit words; the cast keeps the same
    // address with a narrower element type.
    #[allow(clippy::as_conversions, clippy::ptr_as_ptr)]
    let kr = unsafe {
        darwin_ffi::task_info(
            task,
            darwin_ffi::TASK_VM_INFO_PURGEABLE,
            (&raw mut info).cast::<u32>(),
            &raw mut count,
        )
    };

    if kr == darwin_ffi::KERN_SUCCESS {
        Ok(info.phys_footprint)
    } else {
        Err(HypomnesisError::Ram(format!(
            "task_info(TASK_VM_INFO_PURGEABLE) failed (kern_return = {kr})"
        )))
    }
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
