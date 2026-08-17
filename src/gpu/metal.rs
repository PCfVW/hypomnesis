// SPDX-License-Identifier: MIT OR Apache-2.0

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
//! but must never be hardcoded as a literal expression — the entry
//! ordering is not part of any stable ABI guarantee.

use core::ffi::{c_char, c_void};
use std::sync::OnceLock;

/// libSystem FFI declarations for the macOS GPU backend.
///
/// Every entry below is a stable libSystem syscall available on every
/// macOS install since at least 10.15. No header from
/// `Kernel.framework` is shipped in user space for `ledger()`, so the
/// signature is declared inline against the kernel's documented ABI.
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
        /// `arg1`/`arg2`/`arg3` semantics depend on `cmd`. All three
        /// have C type `caddr_t` (`char *`); for `LEDGER_ENTRY_INFO_V2`
        /// arg1 is the target PID reinterpreted as a pointer-sized
        /// integer (kernel convention — see `osfmk/kern/ledger.c`).
        /// Returns `0` on success, `-1` with `errno` set on failure
        /// (e.g. `EPERM` for cross-user reads).
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
        pub(super) unsafe fn proc_pidpath(pid: i32, buffer: *mut c_void, buffersize: u32) -> i32;
    }
}

/// `LEDGER_INFO` command — query the per-PID ledger metadata
/// (`li_entries` = number of entries, `li_name` = task name).
///
/// Value from XNU `osfmk/kern/ledger.h`. Not currently issued by this
/// module (only [`LEDGER_TEMPLATE_INFO`] and [`LEDGER_ENTRY_INFO_V2`]
/// are) — kept as a zero-cost `const` documenting the complete 3-command
/// family this FFI surface is built against, so a future reader sees the
/// full picture rather than two commands with no visible sibling.
#[allow(dead_code)]
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
/// (sizeof = 88 bytes) — this is **not** the V1 `ledger_entry_info`
/// shape.
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
/// Shape mirrors [`super::dxgi::DxgiQueryResult`] in spirit: the device
/// total, the device-wide working-set budget, and the adapter name.
/// Unlike `DxgiQueryResult`, there is no per-process figure here — the
/// per-process path is [`process_gpu_info`], which reads
/// `graphics_footprint` independently rather than through this struct.
/// Returned by [`query`].
pub(super) struct MetalQueryResult {
    /// Total physical memory in bytes — `sysctl hw.memsize`. On UMA
    /// this is the system DRAM size, which is also the GPU's address
    /// space ceiling.
    pub dedicated_video_memory: u64,
    /// Apple-driver-recommended GPU working-set budget in bytes —
    /// `MTLDevice.recommendedMaxWorkingSetSize`. The value the kernel
    /// projects as "memory the GPU can hold resident with good
    /// performance," factoring in compression + system reserves. Used
    /// as the macOS analogue of `free_bytes` on a discrete GPU.
    pub recommended_max_working_set: u64,
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
/// — the kernel's entry ordering is not part of any stable ABI.
static GRAPHICS_FOOTPRINT_INDEX: OnceLock<i32> = OnceLock::new();

/// Maximum ledger-entry count probed at init.
///
/// XNU defines ~70 entries on macOS 26.x; this cap bounds the
/// `LEDGER_TEMPLATE_INFO` buffer growth. Set well above the observed
/// count to absorb future kernel additions without truncation.
const LEDGER_TEMPLATE_BUF_CAP: usize = 128;

/// Cached system-default Metal device handle.
///
/// Acquired on first call to [`recommended_max_working_set_size`] via
/// `MTLCreateSystemDefaultDevice`; never re-acquired thereafter. The
/// driver's per-device cost of acquiring a handle is ~100-200 µs the
/// first time and unmeasurable thereafter (the property read on a
/// cached handle is a synchronised getter on the device object).
///
/// `Option<...>` because the call may legitimately return `nil` on a
/// system without a usable Metal device (extremely rare on Apple
/// Silicon; possible in very locked-down environments).
static METAL_DEVICE: OnceLock<
    Option<objc2::rc::Retained<objc2::runtime::ProtocolObject<dyn objc2_metal::MTLDevice>>>,
> = OnceLock::new();

/// Compile-time guard: if a future `objc2-metal` version drops the
/// `Send + Sync` supertraits on `MTLDevice`, this fails to compile and
/// the implementation must switch to a newtype with explicit
/// `unsafe impl Send + Sync` to remain safe in the static `OnceLock`.
const _: fn() = || {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<
        objc2::rc::Retained<objc2::runtime::ProtocolObject<dyn objc2_metal::MTLDevice>>,
    >();
};

/// Read `MTLDevice.recommendedMaxWorkingSetSize` for the system-default
/// device, caching the device handle for the life of the process.
///
/// Returns `None` if `MTLCreateSystemDefaultDevice` returns `nil`. The
/// caller (in [`query`]) falls back to the total physical DRAM in that
/// case — that's the conservative upper bound on what the GPU can hold.
fn recommended_max_working_set_size() -> Option<u64> {
    // objc2-metal's bindings expose this property as a safe method; no
    // `unsafe` block is required at the call site. The trait import is
    // necessary because the method is a trait method, not an inherent.
    use objc2_metal::MTLDevice;
    METAL_DEVICE
        // `MTLCreateSystemDefaultDevice` is declared `extern "C-unwind"`
        // and doesn't coerce to a closure type; wrap in `||` so
        // `get_or_init` accepts it.
        .get_or_init(|| objc2_metal::MTLCreateSystemDefaultDevice())
        .as_ref()
        .map(|d| d.recommendedMaxWorkingSetSize())
}

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
/// Calls `ledger(LEDGER_TEMPLATE_INFO, buf, &count, NULL)` and
/// linear-scans the returned [`LedgerTemplateInfo`] rows for an
/// `lti_name` whose decoded prefix equals `"graphics_footprint"`.
/// Returns `None` if the syscall fails or the entry is absent on this
/// kernel.
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
    #[allow(
        clippy::as_conversions,
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap
    )]
    let mut count: i32 = LEDGER_TEMPLATE_BUF_CAP as i32;
    // SAFETY: arg1 is the template buffer (`caddr_t`); arg2 is the
    // count in/out pointer (`caddr_t` aliasing an `i32`); arg3 is
    // NULL. The kernel writes at most `count` rows and updates
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
            #[allow(
                clippy::as_conversions,
                clippy::cast_possible_truncation,
                clippy::cast_possible_wrap
            )]
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
/// `graphics_footprint` tracks resident Metal-written pages on Apple
/// Silicon UMA: writing every byte of a 256 MiB `MTLBuffer` increases
/// the entry by exactly 256 MiB (resident-bytes semantics, the macOS
/// analogue of Windows `WorkingSetSize` and Linux `VmRSS`).
#[allow(unsafe_code)]
fn read_graphics_footprint(pid: i32) -> Option<u64> {
    let idx =
        *GRAPHICS_FOOTPRINT_INDEX.get_or_init(|| resolve_graphics_footprint_index().unwrap_or(-1));
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
    #[allow(
        clippy::as_conversions,
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap
    )]
    let mut count: i32 = LEDGER_TEMPLATE_BUF_CAP as i32;
    // CAST: i32 PID → *mut c_void. The `ledger` syscall reinterprets
    // arg1's pointer-sized bits as the target PID (kernel convention,
    // mirroring `caddr_t(bitPattern: Int(pid))` on the Swift side).
    // PIDs are non-negative on macOS, so the sign-loss step is benign.
    #[allow(clippy::as_conversions, clippy::cast_sign_loss)]
    let pid_as_ptr = pid as usize as *mut c_void;
    // SAFETY: arg1 is the PID encoded as a pointer; arg2 is the
    // entry-array buffer; arg3 is the count in/out. Cross-user PIDs
    // surface as a non-zero return (errno = EPERM); PID-exited
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
/// [`MetalQueryResult`] populated from `sysctl hw.memsize` for the
/// total and `sysctl machdep.cpu.brand_string` for the name. (Per-process
/// usage is not part of this device-wide query — see [`process_gpu_info`],
/// which reads `graphics_footprint` independently.)
pub(super) fn query(idx: u32) -> Option<MetalQueryResult> {
    if idx != 0 {
        return None;
    }
    let dedicated_video_memory = read_sysctl_u64(b"hw.memsize\0")?;
    let adapter_name =
        read_sysctl_string(b"machdep.cpu.brand_string\0").unwrap_or_else(|| "Apple GPU".into());
    // Falls back to total DRAM if the Metal driver cannot be loaded
    // (extremely rare on Apple Silicon; possible on very locked-down
    // environments). `dedicated_video_memory` is the conservative
    // upper bound on what `free_bytes` can be.
    let recommended_max_working_set =
        recommended_max_working_set_size().unwrap_or(dedicated_video_memory);
    Some(MetalQueryResult {
        dedicated_video_memory,
        recommended_max_working_set,
        adapter_name,
    })
}

/// Number of Metal devices visible — `Some(1)` on Apple Silicon,
/// `None` elsewhere (Intel Macs are out of scope for v0.2.2).
///
/// Detects Apple Silicon by reading `machdep.cpu.brand_string` and
/// looking for "Apple"; the alternative `hw.optional.arm64` sysctl
/// returns a 32-bit `int` and so cannot be read through the
/// [`read_sysctl_u64`] helper without a separate u32 variant.
pub(super) fn device_count() -> Option<u32> {
    let brand = read_sysctl_string(b"machdep.cpu.brand_string\0")?;
    if brand.contains("Apple") {
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
/// skipped silently, as are PIDs with a zero `graphics_footprint`
/// balance (mirrors NVML's per-process filter on Linux).
///
/// Two-phase `proc_listpids`: query the buffer size first, then fill.
/// PID count may grow between the two calls; the iteration is capped
/// at the buffer's filled length to remain safe.
#[allow(unsafe_code)]
pub(super) fn list_compute_processes(device_index: u32) -> Option<Vec<crate::GpuProcessEntry>> {
    if device_index != 0 {
        return None;
    }

    // Phase 1 — size probe: `buffer = NULL, buffersize = 0` returns
    // the byte count the kernel would write.
    // SAFETY: `PROC_ALL_PIDS` is a documented selector; `typeinfo = 0`
    // means "any predicate"; `buffer = NULL`/`buffersize = 0` is the
    // documented size-probe convention.
    let size_bytes =
        unsafe { libsystem_ffi::proc_listpids(PROC_ALL_PIDS, 0, core::ptr::null_mut(), 0) };
    if size_bytes <= 0 {
        return None;
    }
    // CAST: i32 → usize, `size_bytes > 0` just checked. `size_bytes`
    // is bounded by the kernel's max PID count × 4; comfortably fits.
    #[allow(clippy::as_conversions, clippy::cast_sign_loss)]
    let size_usize = size_bytes as usize;
    let pid_count = size_usize / size_of::<i32>();
    if pid_count == 0 {
        return None;
    }

    // Phase 2 — fill. Allocate `pid_count` i32 slots; the kernel may
    // see a slightly larger live PID count by the time it runs but
    // will not exceed the byte budget we pass.
    let mut pids: Vec<i32> = vec![0_i32; pid_count];
    // CAST: usize → i32, `size_usize == pid_count * 4` and was just
    // computed from the kernel's own i32 return; round-trips safely.
    #[allow(
        clippy::as_conversions,
        clippy::cast_possible_truncation,
        clippy::cast_possible_wrap
    )]
    let cap_bytes = (pid_count * size_of::<i32>()) as i32;
    // SAFETY: `pids.as_mut_ptr` is valid for `pid_count *
    // size_of::<i32>()` bytes (matches `cap_bytes`). The kernel
    // writes up to `cap_bytes` bytes worth of PIDs and returns the
    // actual byte count written.
    let written_bytes = unsafe {
        libsystem_ffi::proc_listpids(
            PROC_ALL_PIDS,
            0,
            pids.as_mut_ptr().cast::<c_void>(),
            cap_bytes,
        )
    };
    if written_bytes <= 0 {
        return None;
    }
    // CAST: i32 → usize, `written_bytes > 0` just checked.
    #[allow(clippy::as_conversions, clippy::cast_sign_loss)]
    let written_usize = (written_bytes as usize).min(size_usize);
    let written_pids = written_usize / size_of::<i32>();

    let mut out: Vec<crate::GpuProcessEntry> = Vec::new();
    // Iterate up to the buffer's actual filled length; cap by
    // `pid_count` defensively against any kernel-side count growth.
    for &pid in pids.iter().take(written_pids) {
        if pid <= 0 {
            continue;
        }
        let Some(used) = read_graphics_footprint(pid) else {
            // Cross-user EPERM, ESRCH (exited), or absent index —
            // all surface as `None` here and are silently skipped.
            continue;
        };
        if used == 0 {
            continue;
        }
        let name = read_proc_pidpath_basename(pid);
        // CAST: i32 → u32, `pid > 0` just checked; PIDs are
        // non-negative on macOS.
        #[allow(clippy::as_conversions, clippy::cast_sign_loss)]
        let pid_u32 = pid as u32;
        out.push(crate::GpuProcessEntry {
            pid: pid_u32,
            name,
            used_bytes: used,
            // Apple Silicon UMA is a single physical pool — there is
            // no separate shared budget to spill into (see
            // GpuProcessEntry docs).
            shared_used_bytes: 0,
            source: crate::GpuQuerySource::Metal,
        });
    }

    Some(out)
}

/// Resolve `pid`'s executable basename via `proc_pidpath`.
///
/// Returns `None` on syscall failure (process exited, cross-user
/// permission denial, or path-decoding failure). The basename is the
/// final `/`-separated component of the full executable path.
#[allow(unsafe_code)]
fn read_proc_pidpath_basename(pid: i32) -> Option<String> {
    let mut buf: [u8; PROC_PIDPATHINFO_MAXSIZE] = [0; PROC_PIDPATHINFO_MAXSIZE];
    // CAST: usize → u32, `PROC_PIDPATHINFO_MAXSIZE` is 4096; fits.
    #[allow(clippy::as_conversions, clippy::cast_possible_truncation)]
    let cap_u32 = PROC_PIDPATHINFO_MAXSIZE as u32;
    // SAFETY: `buf.as_mut_ptr` is valid for `PROC_PIDPATHINFO_MAXSIZE`
    // bytes (its declared length). The kernel writes a NUL-terminated
    // path of at most `cap_u32` bytes and returns the length excluding
    // the NUL. PID validity is handled by the kernel; a stale/cross-user
    // PID returns 0.
    let len =
        unsafe { libsystem_ffi::proc_pidpath(pid, buf.as_mut_ptr().cast::<c_void>(), cap_u32) };
    if len <= 0 {
        return None;
    }
    // CAST: i32 → usize, `len > 0` just checked and bounded by
    // `cap_u32`.
    #[allow(clippy::as_conversions, clippy::cast_sign_loss)]
    let len_usize = (len as usize).min(PROC_PIDPATHINFO_MAXSIZE);
    // Take only the bytes the kernel wrote (excludes terminator).
    let path_bytes = buf.get(..len_usize)?;
    // BORROW: `from_utf8` borrows; `to_owned` produces the returned
    // `String` so the stack buffer can be dropped.
    let path_str = core::str::from_utf8(path_bytes).ok()?.to_owned();
    // Extract basename — the substring after the final '/'.
    let basename = path_str.rsplit('/').next().unwrap_or(&path_str);
    if basename.is_empty() {
        None
    } else {
        // BORROW: `to_owned` — `basename` is borrowed from `path_str`
        // which is dropped at function return.
        Some(basename.to_owned())
    }
}
