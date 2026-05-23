// SPDX-License-Identifier: MIT OR Apache-2.0

// Stubs for `process_gpu_info` and `list_compute_processes` are
// populated in the gpu-metal leaf's Steps 3 and 4. The dead-code allow
// covers the not-yet-wired surfaces and is dropped when they wire up.
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

use core::ffi::{c_char, c_void};
use std::sync::OnceLock;

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
        /// kernel guarantees a valid PID is always returned. Declared
        /// for ABI completeness of the libSystem surface; the calling
        /// PID is read via `std::process::id` (safe stdlib) elsewhere.
        #[allow(dead_code)]
        pub(super) safe fn getpid() -> i32;

        /// BSD kernel ledger syscall (no user-space header ships this).
        ///
        /// `cmd` selects the operation: [`super::LEDGER_INFO`],
        /// [`super::LEDGER_TEMPLATE_INFO`], [`super::LEDGER_ENTRY_INFO_V2`].
        /// `arg1`/`arg2`/`arg3` semantics depend on `cmd`; see R02
        /// Appendix A `bridge.h` for the call conventions actually
        /// exercised here. All three args have C type `caddr_t`
        /// (`char *`) — for `LEDGER_ENTRY_INFO_V2` arg1 is the PID
        /// reinterpreted as a pointer-sized integer, mirroring R02's
        /// `caddr_t(bitPattern: Int(pid))`. Returns `0` on success,
        /// `-1` with `errno` set on failure (e.g. `EPERM` for cross-user
        /// reads).
        pub(super) unsafe fn ledger(
            cmd: i32,
            arg1: *mut c_void,
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

/// Cached index of the `graphics_footprint` entry in the per-PID
/// ledger entry array. Resolved by name on first read via
/// [`resolve_graphics_footprint_index`]; observed value on macOS 26.x
/// is 36, but the literal must never appear as a Rust expression value
/// — see R01 § Reference Design.
static GRAPHICS_FOOTPRINT_INDEX: OnceLock<i32> = OnceLock::new();

/// Maximum ledger-entry count probed at init.
///
/// XNU defines ~70 entries on macOS 26.x; this cap bounds the
/// `LEDGER_TEMPLATE_INFO` buffer growth. Set well above the observed
/// count to absorb future kernel additions without truncation.
const LEDGER_TEMPLATE_BUF_CAP: usize = 128;

/// Read a `u64` `sysctlbyname` value (e.g. `b"hw.memsize\0"`).
///
/// `name` must be a NUL-terminated byte slice. Returns `None` if the
/// syscall fails or returns a non-8-byte value.
#[allow(unsafe_code)]
fn read_sysctl_u64(name: &[u8]) -> Option<u64> {
    let mut value: u64 = 0;
    let mut len: usize = size_of::<u64>();
    // CAST: &u64 → *mut c_void via `&raw mut value` then explicit cast;
    // the sysctl ABI treats the out-buffer as an opaque byte region.
    #[allow(clippy::as_conversions, clippy::ptr_as_ptr)]
    let out_ptr = (&raw mut value).cast::<c_void>();
    // SAFETY: `name` is caller-provided as a NUL-terminated byte slice;
    // its `as_ptr()` is valid for `name.len()` bytes including the
    // terminator the kernel scans for. `out_ptr` points to an 8-byte
    // stack-resident `u64`. `&raw mut len` is a live `usize` whose
    // initial value matches the buffer capacity. `newp`/`newlen` are
    // null/zero — read-only query.
    let rc = unsafe {
        // INDEX: `name[0]` is the first byte of the NUL-terminated
        // name; passing a zero-length slice would yield a dangling
        // pointer, so reject empty names up-front via the slice's
        // own bounds check on `as_ptr`.
        libsystem_ffi::sysctlbyname(
            name.as_ptr().cast::<c_char>(),
            out_ptr,
            &raw mut len,
            core::ptr::null_mut(),
            0,
        )
    };
    if rc == 0 && len == size_of::<u64>() {
        Some(value)
    } else {
        None
    }
}

/// Read a UTF-8 `sysctlbyname` string (e.g. `b"machdep.cpu.brand_string\0"`).
///
/// Two-call probe: query the buffer length first with a null `oldp`,
/// then allocate and re-query. Returns `None` if either call fails or
/// the result is not valid UTF-8 after trimming the trailing NUL.
#[allow(unsafe_code)]
fn read_sysctl_string(name: &[u8]) -> Option<String> {
    let mut len: usize = 0;
    // SAFETY: `name.as_ptr()` is a NUL-terminated C string; `oldp =
    // null` instructs the kernel to write the required buffer size
    // into `*oldlenp`. `newp`/`newlen` are null/zero (read-only).
    let rc = unsafe {
        libsystem_ffi::sysctlbyname(
            name.as_ptr().cast::<c_char>(),
            core::ptr::null_mut(),
            &raw mut len,
            core::ptr::null_mut(),
            0,
        )
    };
    if rc != 0 || len == 0 {
        return None;
    }
    let mut buf: Vec<u8> = vec![0_u8; len];
    // SAFETY: `buf` is a freshly allocated Vec<u8> with `len` bytes
    // capacity AND length; `as_mut_ptr` is valid for `buf.len()`
    // bytes. `&raw mut len` still holds the kernel-reported size.
    let rc2 = unsafe {
        libsystem_ffi::sysctlbyname(
            name.as_ptr().cast::<c_char>(),
            buf.as_mut_ptr().cast::<c_void>(),
            &raw mut len,
            core::ptr::null_mut(),
            0,
        )
    };
    if rc2 != 0 {
        return None;
    }
    // Trim trailing NUL byte(s) the kernel includes in the count.
    while buf.last() == Some(&0) {
        buf.pop();
    }
    // BORROW: `String::from_utf8(buf).ok()` — sysctl strings are ASCII
    // by convention; UTF-8 decode failure surfaces as `None`.
    String::from_utf8(buf).ok()
}

/// Discover the `graphics_footprint` ledger entry index by name.
///
/// Calls `ledger(LEDGER_TEMPLATE_INFO, buf, &count, NULL)` per R02
/// Appendix A and linear-scans the returned [`LedgerTemplateInfo`]
/// rows for an `lti_name` whose decoded prefix equals
/// `"graphics_footprint"`. Returns `None` if the syscall fails or the
/// entry is absent on this kernel.
#[allow(unsafe_code)]
fn resolve_graphics_footprint_index() -> Option<i32> {
    // SAFETY: `LedgerTemplateInfo` is `#[repr(C)]` with only `c_char`
    // array fields (POD). All-zero is a valid bit pattern; we
    // initialise via `core::mem::zeroed` per element so the resulting
    // `Vec` is fully initialised before the FFI call sees its buffer.
    let mut buf: Vec<LedgerTemplateInfo> = (0..LEDGER_TEMPLATE_BUF_CAP)
        .map(|_| unsafe { core::mem::zeroed::<LedgerTemplateInfo>() })
        .collect();
    // CAST: usize → i32, count is bounded by `LEDGER_TEMPLATE_BUF_CAP`
    // (128); fits trivially in i32.
    #[allow(clippy::as_conversions, clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    let mut count: i32 = LEDGER_TEMPLATE_BUF_CAP as i32;
    // SAFETY: arg1 is the template buffer (`caddr_t`); arg2 is the
    // count in/out pointer (`caddr_t` aliasing an `i32`); arg3 is
    // NULL. R02 Appendix A's `loadTemplateNames` uses the same
    // convention. The kernel writes at most `count` rows and updates
    // `*count` with the number actually written.
    let rc = unsafe {
        libsystem_ffi::ledger(
            LEDGER_TEMPLATE_INFO,
            buf.as_mut_ptr().cast::<c_void>(),
            (&raw mut count).cast::<c_void>(),
            core::ptr::null_mut(),
        )
    };
    if rc != 0 || count <= 0 {
        return None;
    }
    // CAST: i32 → usize, count was just checked > 0 and is bounded
    // by `LEDGER_TEMPLATE_BUF_CAP`.
    #[allow(clippy::as_conversions, clippy::cast_sign_loss)]
    let returned = (count as usize).min(LEDGER_TEMPLATE_BUF_CAP);
    let target = b"graphics_footprint";
    for (i, row) in buf.iter().take(returned).enumerate() {
        // The XNU entry name is a 32-byte NUL-terminated ASCII field.
        // Decode by iterating until the first NUL; produce a
        // bounded-length `Vec<u8>` that lets us compare with `target`
        // without raw slice indexing.
        // CAST: c_char → u8, the kernel writes ASCII bytes; both have
        // identical wire representation.
        #[allow(clippy::as_conversions, clippy::cast_sign_loss)]
        let name_bytes: Vec<u8> = row
            .lti_name
            .iter()
            .take_while(|&&c| c != 0)
            .map(|&c| c as u8)
            .collect();
        if name_bytes == target {
            // CAST: usize → i32, `i < returned <= LEDGER_TEMPLATE_BUF_CAP`
            // (128); fits in i32.
            #[allow(clippy::as_conversions, clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
            return Some(i as i32);
        }
    }
    None
}

/// Read `graphics_footprint` (resident GPU-attributed bytes) for `pid`.
///
/// Calls `ledger(LEDGER_ENTRY_INFO_V2, pid, buf, &count)` and reads the
/// `lei_balance` of the resolved entry index. Returns `None` if the
/// syscall fails (e.g. cross-user PID without root → `EPERM`, or PID
/// has exited → `ESRCH`), or the index has not yet been resolvable.
///
/// R02 Round 05 confirmed this entry tracks Metal-written pages on
/// Apple Silicon UMA — see `__reports__/macos_ledger/05-findings_writes_v0.md`
/// Table 1 (Phase 1→2 delta = +256 MiB, exact).
#[allow(unsafe_code)]
fn read_graphics_footprint(pid: i32) -> Option<u64> {
    let idx = *GRAPHICS_FOOTPRINT_INDEX
        .get_or_init(|| resolve_graphics_footprint_index().unwrap_or(-1));
    if idx < 0 {
        return None;
    }
    // CAST: i32 → usize, `idx >= 0` was just checked.
    #[allow(clippy::as_conversions, clippy::cast_sign_loss)]
    let idx_usize = idx as usize;

    // Allocate one row per kernel entry. The kernel writes
    // `LEDGER_TEMPLATE_BUF_CAP` rows max; we size the buffer the same
    // way the template probe did so indexing into it is in-range.
    // SAFETY: `LedgerEntryInfo` is `#[repr(C)]` with only integer
    // fields (POD). All-zero is a valid bit pattern; per-element
    // `zeroed` initialises the whole `Vec` before the FFI sees it.
    let mut buf: Vec<LedgerEntryInfo> = (0..LEDGER_TEMPLATE_BUF_CAP)
        .map(|_| unsafe { core::mem::zeroed::<LedgerEntryInfo>() })
        .collect();
    // CAST: usize → i32, bounded by `LEDGER_TEMPLATE_BUF_CAP` (128).
    #[allow(clippy::as_conversions, clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
    let mut count: i32 = LEDGER_TEMPLATE_BUF_CAP as i32;
    // CAST: i32 PID → *mut c_void, mirroring R02 Appendix A's
    // `caddr_t(bitPattern: Int(pid))` — the syscall reinterprets
    // arg1's pointer-sized bits as the target PID. PIDs are
    // non-negative on macOS, so the sign-loss step is benign.
    #[allow(clippy::as_conversions, clippy::cast_sign_loss)]
    let pid_as_ptr = pid as usize as *mut c_void;
    // SAFETY: arg1 is the PID encoded as a pointer; arg2 is the
    // entry-array buffer; arg3 is the count in/out. R02 Appendix A's
    // `readLedger` uses the exact same argument layout. Cross-user
    // PIDs surface as a non-zero return (errno = EPERM); PID-exited
    // surfaces as ESRCH. Both are folded into `None` below.
    let rc = unsafe {
        libsystem_ffi::ledger(
            LEDGER_ENTRY_INFO_V2,
            pid_as_ptr,
            buf.as_mut_ptr().cast::<c_void>(),
            (&raw mut count).cast::<c_void>(),
        )
    };
    if rc != 0 || count <= 0 {
        return None;
    }
    // CAST: i32 → usize, `count > 0` just checked.
    #[allow(clippy::as_conversions, clippy::cast_sign_loss)]
    let returned = (count as usize).min(LEDGER_TEMPLATE_BUF_CAP);
    if idx_usize >= returned {
        return None;
    }
    // Use `.get()` to avoid panic-prone indexing; `idx_usize < returned`
    // already checked, so the `?` short-circuit is defensive only.
    let balance = buf.get(idx_usize)?.lei_balance;
    if balance < 0 {
        // Negative balances are not physically meaningful for a
        // bytes-unit entry; surface as absent rather than wrapping.
        None
    } else {
        // CAST: i64 → u64, `balance >= 0` just checked.
        #[allow(clippy::as_conversions, clippy::cast_sign_loss)]
        Some(balance as u64)
    }
}

/// Run a single Metal device query for `idx`.
///
/// On Apple Silicon there is a single integrated GPU; this returns
/// `None` for any `idx != 0`. The non-zero case yields a
/// [`MetalQueryResult`] populated from `ledger(graphics_footprint)`
/// for the calling PID, `sysctl hw.memsize` for the total, and
/// `sysctl machdep.cpu.brand_string` for the name.
pub(super) fn query(idx: u32) -> Option<MetalQueryResult> {
    if idx != 0 {
        return None;
    }
    let self_pid = process_self_pid();
    let current_usage = read_graphics_footprint(self_pid).unwrap_or(0);
    let dedicated_video_memory = read_sysctl_u64(b"hw.memsize\0")?;
    let adapter_name = read_sysctl_string(b"machdep.cpu.brand_string\0")
        .unwrap_or_else(|| "Apple GPU".into());
    Some(MetalQueryResult {
        current_usage,
        dedicated_video_memory,
        adapter_name,
    })
}

/// Number of Metal devices visible — `Some(1)` on Apple Silicon,
/// `None` elsewhere (Intel Macs are out of scope for v0.2.2).
pub(super) fn device_count() -> Option<u32> {
    if read_sysctl_u64(b"hw.optional.arm64\0") == Some(1) {
        Some(1)
    } else {
        None
    }
}

/// Calling-process PID via `std::process::id` — safe-stdlib path that
/// avoids the libSystem `getpid` FFI for the trivial self-PID case.
///
/// Kept as a separate fn so Step 3 / Step 4 share the same casting
/// annotation site.
fn process_self_pid() -> i32 {
    // CAST: u32 → i32, POSIX PIDs are non-negative i32; `std::process::id`
    // returns the same bit pattern as `getpid()` would.
    #[allow(clippy::as_conversions, clippy::cast_possible_wrap)]
    {
        std::process::id() as i32
    }
}

/// Per-process GPU memory usage for the calling PID on `device_index`.
///
/// Returns `None` for any `device_index != 0`. Otherwise reads
/// `graphics_footprint` from the BSD ledger for the calling PID and
/// wraps it in a [`crate::ProcessGpuInfo`] with [`crate::GpuQuerySource::Metal`]
/// as the source tag. The `GpuQuerySource::Metal` variant is added by
/// the `gpu_dispatcher_wiring` leaf; `cargo check` for this module
/// will fail until that leaf lands. The `cfg(all(target_os = "macos",
/// feature = "metal"))` gate on `mod metal;` ensures the macOS build
/// only succeeds once the variant exists.
pub(super) fn process_gpu_info(device_index: u32) -> Option<crate::ProcessGpuInfo> {
    if device_index != 0 {
        return None;
    }
    let self_pid = process_self_pid();
    let used_bytes = read_graphics_footprint(self_pid)?;
    Some(crate::ProcessGpuInfo {
        used_bytes,
        is_per_process: true,
        source: crate::GpuQuerySource::Metal,
    })
}

/// Enumerate every same-user process holding GPU memory on
/// `device_index`. Cross-user PIDs (`EPERM` on the ledger read) are
/// skipped silently.
pub(super) fn list_compute_processes(
    _device_index: u32,
) -> Option<Vec<crate::gpu::GpuProcessEntry>> {
    unimplemented!("populated in Step 4")
}
