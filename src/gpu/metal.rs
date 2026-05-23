// SPDX-License-Identifier: MIT OR Apache-2.0

// Stubs in this module are populated in the gpu-metal leaf's Steps 2–4
// (`query`/`device_count`, `process_gpu_info`, `list_compute_processes`).
// The dead-code allow is dropped as each step wires its surface up.
#![allow(dead_code)]

//! macOS GPU backend — per-process Metal memory on Apple Silicon UMA.
//!
//! Reads `graphics_footprint` from the BSD kernel ledger for any
//! same-user PID via `ledger(LEDGER_ENTRY_INFO_V2, pid, …)`. On a
//! unified-memory-architecture (`UMA`) Apple Silicon `SoC` the GPU and
//! CPU share the same physical pages, so device-wide `total_bytes` is
//! `sysctl hw.memsize` and the adapter name is the CPU brand string
//! (`machdep.cpu.brand_string`). Cross-user PIDs require root; their
//! ledger reads return `EPERM` and are skipped silently in enumeration.
//!
//! Source map: `ledger` (per-process graphics resident bytes),
//! `sysctlbyname` (device totals, name), `proc_listpids` +
//! `proc_pidpath` (enumeration). No Mach `task_for_pid` is used and no
//! Apple-framework dependency is required — every call is a libSystem
//! syscall.
//!
//! Semantic equivalence: `graphics_footprint` is **resident** bytes,
//! mirroring Windows `WorkingSetSize` (DXGI `CurrentUsage`) and Linux
//! `VmRSS` (NVML `used`). This is the choice forced by the
//! cross-platform contract — `MTLDevice.currentAllocatedSize` would be
//! allocator-tracked (virtual) and is therefore not used.
//!
//! The `graphics_footprint` ledger entry index is **discovered by name
//! at first call** via `LEDGER_TEMPLATE_INFO`, then cached in a
//! `OnceLock<i32>`. The index observed on macOS 26.x happens to be 36
//! but must never be hardcoded as a literal expression — see R01
//! § Reference Design and § Open Questions.
//!
//! References:
//! - R01 (`__reports__/macos_ledger/09-knowledge_transfer_v3.md`) —
//!   reference design and name-lookup mandate.
//! - R02 (`__reports__/macos_ledger/05-findings_writes_v0.md`) —
//!   writes-corrected probe; Appendix A's C externs map 1:1 to the
//!   Rust externs declared in [`libsystem_ffi`].

use core::ffi::c_char;

/// libSystem FFI declarations for the macOS GPU backend.
///
/// Every entry below is a stable libSystem syscall available on every
/// macOS install since at least 10.15. No header from
/// `Kernel.framework` is shipped in user space for `ledger()`, so the
/// signature is declared inline per R02 Appendix A's `bridge.h`.
mod libsystem_ffi {
    use core::ffi::{c_char, c_void};

    // SAFETY: These are stable libSystem entry points with documented
    // C ABI. `getpid` and `sysctlbyname` are POSIX. `proc_listpids` and
    // `proc_pidpath` are declared in `<libproc.h>`. `ledger` has no
    // user-space header but its ABI is fixed (`SYS_ledger = 373`).
    // Each call's safety contract is upheld at its call site.
    #[allow(unsafe_code)]
    unsafe extern "C" {
        /// Returns the calling process's PID. Cannot fail.
        ///
        /// See: `<unistd.h>`. Marked `safe` per Rust 2024 idiom — the
        /// kernel guarantees a valid PID is always returned.
        pub(super) safe fn getpid() -> i32;

        /// BSD kernel ledger syscall (no user-space header ships this).
        ///
        /// `cmd` selects the operation: [`super::LEDGER_INFO`],
        /// [`super::LEDGER_TEMPLATE_INFO`], [`super::LEDGER_ENTRY_INFO_V2`].
        /// `arg1`/`arg2`/`arg3` semantics depend on `cmd`; see R02
        /// Appendix A `bridge.h` for the call conventions actually
        /// exercised here. Returns `0` on success, `-1` with `errno`
        /// set on failure (e.g. `EPERM` for cross-user reads).
        pub(super) unsafe fn ledger(
            cmd: i32,
            arg1: i32,
            arg2: *mut c_void,
            arg3: *mut c_void,
        ) -> i32;

        /// `sysctlbyname` — read a kernel state variable by name.
        ///
        /// See: `<sys/sysctl.h>`. `name` is a NUL-terminated C string.
        /// `oldp`/`oldlenp` form the standard in/out buffer pair;
        /// `newp`/`newlen` are zero/null for read-only queries.
        pub(super) unsafe fn sysctlbyname(
            name: *const c_char,
            oldp: *mut c_void,
            oldlenp: *mut usize,
            newp: *mut c_void,
            newlen: usize,
        ) -> i32;

        /// `proc_listpids` — enumerate process IDs by type.
        ///
        /// See: `<libproc.h>`. With `type_ = PROC_ALL_PIDS` and
        /// `typeinfo = 0`, fills `buffer` with `i32` PIDs and returns
        /// the number of bytes written. Calling with
        /// `buffer = NULL, buffersize = 0` returns the buffer size in
        /// bytes the kernel would need (i.e. `4 * pid_count`).
        pub(super) unsafe fn proc_listpids(
            type_: u32,
            typeinfo: u32,
            buffer: *mut c_void,
            buffersize: i32,
        ) -> i32;

        /// `proc_pidpath` — resolve a PID's executable path.
        ///
        /// See: `<libproc.h>`. Writes a NUL-terminated path into
        /// `buffer`. Returns the path length on success (excluding
        /// NUL), or `0` on failure (e.g. process exited, permission
        /// denied for cross-user PIDs).
        pub(super) unsafe fn proc_pidpath(
            pid: i32,
            buffer: *mut c_void,
            buffersize: u32,
        ) -> i32;
    }
}

/// `LEDGER_INFO` command — query the per-PID ledger metadata
/// (`li_entries` = number of entries, `li_name` = task name).
///
/// Value from XNU `osfmk/kern/ledger.h` (and R02 Appendix A `bridge.h`).
const LEDGER_INFO: i32 = 0;

/// `LEDGER_TEMPLATE_INFO` command — fetch the array of
/// [`LedgerTemplateInfo`] rows describing every ledger entry by name.
///
/// Used at init to discover the `graphics_footprint` entry index by
/// name. Value from XNU `osfmk/kern/ledger.h`.
const LEDGER_TEMPLATE_INFO: i32 = 2;

/// `LEDGER_ENTRY_INFO_V2` command — fetch the per-PID
/// [`LedgerEntryInfo`] rows. Each entry's `lei_balance` is the current
/// resident-bytes count for that ledger category.
///
/// Value from XNU `osfmk/kern/ledger.h`.
const LEDGER_ENTRY_INFO_V2: i32 = 4;

/// `proc_listpids` selector — enumerate every PID on the system.
///
/// Value from XNU `bsd/sys/proc_info.h`.
const PROC_ALL_PIDS: u32 = 1;

/// `PROC_PIDPATHINFO_MAXSIZE` — maximum path length returned by
/// `proc_pidpath`. `4 * MAXPATHLEN` from `<sys/proc_info.h>`.
const PROC_PIDPATHINFO_MAXSIZE: usize = 4096;

/// `ledger_template_info` from XNU `osfmk/kern/ledger.h`.
///
/// One row per ledger entry, returned in an array by
/// `ledger(LEDGER_TEMPLATE_INFO, …)`. Fields are 32-byte
/// NUL-terminated C strings.
///
/// See: <https://github.com/apple-oss-distributions/xnu/blob/main/osfmk/kern/ledger.h>
#[repr(C)]
#[allow(clippy::struct_field_names)] // `lti_*` prefix is the XNU kernel ABI field naming
struct LedgerTemplateInfo {
    /// Entry name (e.g. `"graphics_footprint"`). NUL-terminated.
    lti_name: [c_char; 32],
    /// Group name (e.g. `"phys"`). NUL-terminated.
    lti_group: [c_char; 32],
    /// Units (e.g. `"bytes"`). NUL-terminated.
    lti_units: [c_char; 32],
}

/// `ledger_entry_info_v2` from XNU `osfmk/kern/ledger.h`.
///
/// One row per ledger entry, returned in an array by
/// `ledger(LEDGER_ENTRY_INFO_V2, pid, …)`. Layout is the V2 ABI
/// (sizeof = 88 bytes) verified empirically in R02 Appendix A; this
/// is **not** the V1 `ledger_entry_info` shape.
///
/// See: <https://github.com/apple-oss-distributions/xnu/blob/main/osfmk/kern/ledger.h>
#[repr(C)]
#[allow(clippy::struct_field_names)] // `lei_*` prefix is the XNU kernel ABI field naming
struct LedgerEntryInfo {
    /// Current ledger balance in entry units (for `graphics_footprint`:
    /// resident GPU-attributed bytes).
    lei_balance: i64,
    /// Credit total (bytes ever credited to this entry; monotonic).
    lei_credit: i64,
    /// Debit total (bytes ever debited from this entry; monotonic).
    lei_debit: i64,
    /// Limit in entry units (`-1` = no limit).
    lei_limit: u64,
    /// Refill period in absolute-time units (`0` = no refill).
    lei_refill_period: u64,
    /// Last refill timestamp in absolute-time units.
    lei_last_refill: u64,
    /// Lifetime maximum value of `lei_balance` (peak).
    lei_lifetime_max: i64,
    /// Reserved for future ABI growth. Kernel writes zero.
    lei_reserved: [u64; 4],
}

/// Combined result of a single Metal device-wide query.
///
/// Shape mirrors [`super::dxgi::DxgiQueryResult`] in spirit: the
/// per-process current usage, the device total, and the adapter name.
/// Returned by [`query`].
pub(super) struct MetalQueryResult {
    /// Per-process GPU memory usage in bytes — `graphics_footprint`
    /// ledger balance for the calling PID. This is the macOS analogue
    /// of DXGI's `CurrentUsage` and NVML's `process.used`.
    pub current_usage: u64,
    /// Total physical memory in bytes — `sysctl hw.memsize`. On UMA
    /// this is the system DRAM size, which is also the GPU's address
    /// space ceiling.
    pub dedicated_video_memory: u64,
    /// Adapter name — the CPU brand string
    /// (`machdep.cpu.brand_string`, e.g. `"Apple M3 Pro"`). On Apple
    /// Silicon the CPU and GPU share the same die, so the CPU brand
    /// identifies the GPU.
    pub adapter_name: String,
}

/// Run a single Metal device query for `idx`.
///
/// On Apple Silicon there is a single integrated GPU; this returns
/// `None` for any `idx != 0`. The non-zero case yields a
/// [`MetalQueryResult`] populated from `ledger(graphics_footprint)`
/// for the calling PID, `sysctl hw.memsize` for the total, and
/// `sysctl machdep.cpu.brand_string` for the name.
pub(super) fn query(_idx: u32) -> Option<MetalQueryResult> {
    unimplemented!("populated in Step 2")
}

/// Number of Metal devices visible — `Some(1)` on Apple Silicon,
/// `None` elsewhere (Intel Macs are out of scope for v0.2.2).
pub(super) fn device_count() -> Option<u32> {
    unimplemented!("populated in Step 2")
}

/// Per-process GPU memory usage for the calling PID on `device_index`.
///
/// Returns `None` for any `device_index != 0`. Otherwise reads
/// `graphics_footprint` from the BSD ledger for the calling PID.
pub(super) fn process_gpu_info(_device_index: u32) -> Option<crate::gpu::ProcessGpuInfo> {
    unimplemented!("populated in Step 3")
}

/// Enumerate every same-user process holding GPU memory on
/// `device_index`. Cross-user PIDs (`EPERM` on the ledger read) are
/// skipped silently.
pub(super) fn list_compute_processes(
    _device_index: u32,
) -> Option<Vec<crate::gpu::GpuProcessEntry>> {
    unimplemented!("populated in Step 4")
}
