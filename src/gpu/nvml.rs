// SPDX-License-Identifier: MIT OR Apache-2.0

//! `NVML` backend (cross-platform).
//!
//! Dynamically loads `libnvidia-ml.so.1` (Linux) or `nvml.dll` (Windows)
//! via `libloading` and exposes three crate-internal entry points used
//! by the dispatchers in `src/gpu/mod.rs`:
//!
//! - [`query`] — combined per-process + device-wide query for one device
//!   index in a single `NVML` init/shutdown cycle.
//! - [`device_count`] — number of NVIDIA GPUs visible to `NVML`.
//! - [`list_compute_processes`] — every compute process on a given
//!   device (used by `crate::gpu_processes`).
//!
//! Each entry point performs its own `nvmlInit_v2` / `nvmlShutdown` pair.
//! Per the v0.1 design, this trades a few milliseconds of init overhead
//! per call for simpler lifecycle management; a long-lived `NVML` context
//! is a candidate for v0.2.
//!
//! # `WDDM` caveat
//!
//! On Windows the kernel memory manager owns GPU allocations under
//! `WDDM`, so `nvmlDeviceGetComputeRunningProcesses_v3` returns
//! `NVML_VALUE_NOT_AVAILABLE` for per-process memory. The dispatcher in
//! `src/gpu/mod.rs` handles this by trying `DXGI` first on Windows; the
//! `NVML` per-process value here is reliably populated only on Linux.
//!
//! # `R570` driver bug
//!
//! Some `R570`-series drivers (observed on `RTX 5060 Ti`) return
//! `u64::MAX` for every running process's GPU memory. Both [`query`]
//! (which records the calling process's row) and
//! [`list_compute_processes`] (which records every CUDA process on the
//! device) detect this sentinel and drop the affected row(s) so the
//! dispatcher can fall back to `nvidia-smi`. A second sanity check
//! catches the case where per-process > device-wide total (impossible
//! under normal conditions; assumed garbage and dropped).

/// Path to the `NVML` shared library on Linux (stable across driver versions).
#[cfg(target_os = "linux")]
const NVML_LIB_PATH: &str = "libnvidia-ml.so.1";

/// Path to the `NVML` shared library on Windows (stable across driver versions).
#[cfg(target_os = "windows")]
const NVML_LIB_PATH: &str = "nvml.dll";

/// Path to the `NVML` shared library on other platforms.
///
/// `query` and `device_count` will fail to load this and return `None`,
/// which the dispatchers handle as "no `NVML` source available". Defining
/// the constant here avoids `#[cfg]` noise on every `Library::new` call.
#[cfg(not(any(target_os = "linux", target_os = "windows")))]
const NVML_LIB_PATH: &str = "libnvidia-ml.so.1";

/// `NVML` return code: success.
const NVML_SUCCESS: u32 = 0;

/// `NVML` return code: caller-provided buffer too small.
///
/// We never retry with a larger buffer — for `nvmlDeviceGetComputeRunningProcesses_v3`
/// with a 64-slot buffer this is a soft success (we still got the first
/// 64 entries, which is enough to locate our PID on any sane system).
const NVML_ERROR_INSUFFICIENT_SIZE: u32 = 7;

/// Maximum number of processes we ask `NVML` to report per call.
///
/// 64 is generous — most machines have fewer than 10 GPU processes;
/// the buffer lives on the stack so the cost is small.
const NVML_MAX_PROCESSES: usize = 64;

/// Buffer size for `nvmlDeviceGetName`, per NVIDIA's `NVML_DEVICE_NAME_V2_BUFFER_SIZE`.
const NVML_DEVICE_NAME_BUFFER_SIZE: usize = 96;

/// Per-process GPU memory info returned by `NVML`.
///
/// Matches the C struct `nvmlProcessInfo_v2_t` (24 bytes) used by
/// `nvmlDeviceGetComputeRunningProcesses_v3`. The `_v3` suffix is a
/// function version, not a struct version.
/// See: <https://docs.nvidia.com/deploy/nvml-api/structnvmlProcessInfo__v2__t.html>
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct NvmlProcessInfo {
    /// Process ID.
    pid: u32,
    /// GPU memory used by this process in bytes.
    /// `u64::MAX` (`0xFFFF_FFFF_FFFF_FFFF`) means "not available".
    used_gpu_memory: u64,
    /// GPU instance ID (`MIG`); unused outside `MIG` mode.
    gpu_instance_id: u32,
    /// Compute instance ID (`MIG`); unused outside `MIG` mode.
    compute_instance_id: u32,
}

/// `NVML` memory info for a device.
///
/// Matches the C struct `nvmlMemory_t`.
/// See: <https://docs.nvidia.com/deploy/nvml-api/structnvmlMemory__t.html>
#[repr(C)]
#[derive(Debug, Clone, Copy)]
struct NvmlMemoryInfo {
    /// Total GPU memory in bytes.
    total: u64,
    /// Free GPU memory in bytes.
    free: u64,
    /// Used GPU memory in bytes.
    used: u64,
}

/// Opaque `NVML` device handle.
type NvmlDevice = *mut std::ffi::c_void;

/// Function signature: `nvmlInit_v2()`.
type NvmlInitFn = unsafe extern "C" fn() -> u32;

/// Function signature: `nvmlShutdown()`.
type NvmlShutdownFn = unsafe extern "C" fn() -> u32;

/// Function signature: `nvmlDeviceGetHandleByIndex_v2(idx, *device)`.
type NvmlDeviceGetHandleByIndexFn = unsafe extern "C" fn(u32, *mut NvmlDevice) -> u32;

/// Function signature: `nvmlDeviceGetMemoryInfo(device, *info)`.
type NvmlDeviceGetMemoryInfoFn = unsafe extern "C" fn(NvmlDevice, *mut NvmlMemoryInfo) -> u32;

/// Function signature: `nvmlDeviceGetComputeRunningProcesses_v3(device, *count, *infos)`.
type NvmlDeviceGetComputeRunningProcessesFn =
    unsafe extern "C" fn(NvmlDevice, *mut u32, *mut NvmlProcessInfo) -> u32;

/// Function signature: `nvmlDeviceGetCount_v2(*count)`.
type NvmlDeviceGetCountFn = unsafe extern "C" fn(*mut u32) -> u32;

/// Function signature: `nvmlDeviceGetName(device, *name, length)`.
type NvmlDeviceGetNameFn = unsafe extern "C" fn(NvmlDevice, *mut std::ffi::c_char, u32) -> u32;

/// Combined result of a single `NVML` query for a given device index.
///
/// Returned by [`query`].
pub(super) struct NvmlQueryResult {
    /// Per-process GPU memory in bytes for the calling process.
    ///
    /// `None` when our PID is absent from `NVML`'s process list, when
    /// the per-process query failed (e.g. `WDDM` `NVML_VALUE_NOT_AVAILABLE`),
    /// or when the driver reports a sentinel/garbage value (`R570`
    /// `u64::MAX` bug, or per-process > device-wide total).
    pub process_used_bytes: Option<u64>,
    /// Device-wide total memory in bytes.
    pub device_total: u64,
    /// Device-wide free memory in bytes.
    pub device_free: u64,
    /// Device-wide used memory in bytes (sum across processes).
    pub device_used: u64,
    /// Adapter name as reported by `nvmlDeviceGetName`,
    /// e.g. `"NVIDIA GeForce RTX 5060 Ti"`.
    /// `None` when the name query fails.
    pub device_name: Option<String>,
}

/// Run a single `NVML` query session for the given device index.
///
/// Loads `NVML`, runs init, queries the device handle, device-wide memory
/// info, adapter name, and per-process info, then shuts `NVML` down before
/// returning. Per-process query failures are tolerated
/// (`process_used_bytes = None`) since the device-wide info is still
/// useful. If the library load, init, handle, or device memory query
/// fails, the function returns `None`.
#[allow(unsafe_code)]
pub(super) fn query(idx: u32) -> Option<NvmlQueryResult> {
    // SAFETY: libloading::Library::new dynamically loads a shared library.
    // NVML is a stable NVIDIA driver component with a well-defined C ABI;
    // the library is reference-counted by the OS and unloaded when `lib`
    // is dropped at scope exit.
    let lib = unsafe { libloading::Library::new(NVML_LIB_PATH) }.ok()?;

    // SAFETY: Loading function symbols from the NVML library. Each name
    // matches the documented NVML C API exactly. The function signatures
    // (type aliases above) match the NVML header definitions.
    let init: libloading::Symbol<'_, NvmlInitFn> = unsafe { lib.get(b"nvmlInit_v2\0") }.ok()?;
    let shutdown: libloading::Symbol<'_, NvmlShutdownFn> =
        unsafe { lib.get(b"nvmlShutdown\0") }.ok()?;
    let get_handle: libloading::Symbol<'_, NvmlDeviceGetHandleByIndexFn> =
        unsafe { lib.get(b"nvmlDeviceGetHandleByIndex_v2\0") }.ok()?;
    let get_memory: libloading::Symbol<'_, NvmlDeviceGetMemoryInfoFn> =
        unsafe { lib.get(b"nvmlDeviceGetMemoryInfo\0") }.ok()?;
    let get_processes: libloading::Symbol<'_, NvmlDeviceGetComputeRunningProcessesFn> =
        unsafe { lib.get(b"nvmlDeviceGetComputeRunningProcesses_v3\0") }.ok()?;
    let get_name: libloading::Symbol<'_, NvmlDeviceGetNameFn> =
        unsafe { lib.get(b"nvmlDeviceGetName\0") }.ok()?;

    // SAFETY: nvmlInit_v2 is reentrant + thread-safe; it initializes
    // internal NVML state. NVML_SUCCESS (0) is the success return code.
    let ret = unsafe { init() };
    if ret != NVML_SUCCESS {
        #[cfg(feature = "debug-output")]
        eprintln!("[NVML debug] nvmlInit_v2 returned {ret}");
        return None;
    }

    // From here, every return path MUST call shutdown to balance the init.

    // SAFETY: nvmlDeviceGetHandleByIndex_v2 writes a valid opaque handle
    // into `device` when it returns NVML_SUCCESS. The pointer is owned
    // by NVML (we treat it as opaque).
    let mut device: NvmlDevice = std::ptr::null_mut();
    let ret = unsafe { get_handle(idx, &raw mut device) };
    if ret != NVML_SUCCESS {
        #[cfg(feature = "debug-output")]
        eprintln!("[NVML debug] nvmlDeviceGetHandleByIndex_v2(idx={idx}) returned {ret}");
        // SAFETY: nvmlShutdown is always safe to call after a successful nvmlInit.
        unsafe { shutdown() };
        return None;
    }

    // SAFETY: nvmlDeviceGetMemoryInfo writes into the caller-provided
    // NvmlMemoryInfo struct. The device handle is valid (acquired above
    // with NVML_SUCCESS).
    let mut mem_info = NvmlMemoryInfo {
        total: 0,
        free: 0,
        used: 0,
    };
    let ret = unsafe { get_memory(device, &raw mut mem_info) };
    if ret != NVML_SUCCESS {
        #[cfg(feature = "debug-output")]
        eprintln!("[NVML debug] nvmlDeviceGetMemoryInfo returned {ret}");
        // SAFETY: nvmlShutdown after init.
        unsafe { shutdown() };
        return None;
    }

    // Adapter name (best-effort; failure is non-fatal for the rest of the result).
    let device_name = read_device_name(&get_name, device);

    // Per-process query (best-effort; can fail under WDDM as NVML_VALUE_NOT_AVAILABLE).
    let process_used_bytes = read_process_used(&get_processes, device, mem_info.total);

    // SAFETY: nvmlShutdown balances the matched nvmlInit_v2.
    unsafe { shutdown() };

    #[cfg(feature = "debug-output")]
    eprintln!(
        "[NVML debug] device {idx}: total={} free={} used={} per_process={:?} name={:?}",
        mem_info.total, mem_info.free, mem_info.used, process_used_bytes, device_name
    );

    Some(NvmlQueryResult {
        process_used_bytes,
        device_total: mem_info.total,
        device_free: mem_info.free,
        device_used: mem_info.used,
        device_name,
    })
}

/// Read the adapter name via `nvmlDeviceGetName`.
///
/// Returns `None` on `NVML` failure or empty name. Caller must already
/// hold an initialized `NVML` and a valid device handle.
#[allow(unsafe_code)]
fn read_device_name(
    get_name: &libloading::Symbol<'_, NvmlDeviceGetNameFn>,
    device: NvmlDevice,
) -> Option<String> {
    let mut name_buf = [0_u8; NVML_DEVICE_NAME_BUFFER_SIZE];
    // CAST: usize → u32, NVML_DEVICE_NAME_BUFFER_SIZE = 96 fits in u32
    #[allow(clippy::as_conversions, clippy::cast_possible_truncation)]
    let len = NVML_DEVICE_NAME_BUFFER_SIZE as u32;
    // SAFETY: nvmlDeviceGetName writes a null-terminated UTF-8 C string
    // into name_buf, up to `len` bytes. The buffer is stack-allocated
    // and sized per NVIDIA's NVML_DEVICE_NAME_V2_BUFFER_SIZE.
    let ret = unsafe {
        get_name(
            device,
            name_buf.as_mut_ptr().cast::<std::ffi::c_char>(),
            len,
        )
    };
    if ret != NVML_SUCCESS {
        #[cfg(feature = "debug-output")]
        eprintln!("[NVML debug] nvmlDeviceGetName returned {ret}");
        return None;
    }
    let nul_pos = name_buf
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(name_buf.len());
    // BORROW: explicit String::from_utf8_lossy + into_owned — name_buf
    // is stack-local; we need an owned String to return.
    name_buf
        .get(..nul_pos)
        .map(|slice| String::from_utf8_lossy(slice).into_owned())
        .filter(|s| !s.is_empty())
}

/// Read this process's per-process VRAM via
/// `nvmlDeviceGetComputeRunningProcesses_v3`, applying sentinel and
/// sanity checks.
///
/// Returns `None` when our PID is absent from the list, when the call
/// itself fails (`WDDM` `NVML_VALUE_NOT_AVAILABLE`), when the value is
/// the `R570` `u64::MAX` sentinel, or when per-process > device total.
#[allow(unsafe_code)]
fn read_process_used(
    get_processes: &libloading::Symbol<'_, NvmlDeviceGetComputeRunningProcessesFn>,
    device: NvmlDevice,
    device_total: u64,
) -> Option<u64> {
    // CAST: usize → u32, NVML_MAX_PROCESSES = 64 fits in u32
    #[allow(clippy::as_conversions, clippy::cast_possible_truncation)]
    let mut count = NVML_MAX_PROCESSES as u32;
    let mut infos = [NvmlProcessInfo {
        pid: 0,
        used_gpu_memory: 0,
        gpu_instance_id: 0,
        compute_instance_id: 0,
    }; NVML_MAX_PROCESSES];

    // SAFETY: nvmlDeviceGetComputeRunningProcesses_v3 fills `infos` with
    // up to `count` entries and updates `count` to the actual number
    // written. The buffer is stack-allocated with NVML_MAX_PROCESSES
    // slots; if the device has more processes than that, NVML returns
    // NVML_ERROR_INSUFFICIENT_SIZE and we still have the first 64
    // entries — sufficient to locate our PID on any sane system.
    let ret = unsafe { get_processes(device, &raw mut count, infos.as_mut_ptr()) };

    if ret != NVML_SUCCESS && ret != NVML_ERROR_INSUFFICIENT_SIZE {
        #[cfg(feature = "debug-output")]
        eprintln!(
            "[NVML debug] nvmlDeviceGetComputeRunningProcesses_v3 returned {ret} \
             (likely WDDM NVML_VALUE_NOT_AVAILABLE)"
        );
        return None;
    }

    let my_pid = std::process::id();
    // CAST: u32 → usize, NVML count is bounded by buffer size; usize >= 32 bits everywhere
    #[allow(clippy::as_conversions)]
    let actual_count = (count as usize).min(NVML_MAX_PROCESSES);
    let my_vram = infos
        .get(..actual_count)
        .and_then(|s| s.iter().find(|info| info.pid == my_pid))
        .map(|info| info.used_gpu_memory);

    match my_vram {
        Some(u64::MAX) => {
            #[cfg(feature = "debug-output")]
            eprintln!(
                "[NVML debug] process used_gpu_memory == u64::MAX (R570 sentinel); falling back"
            );
            None
        }
        Some(used) if used > device_total => {
            #[cfg(feature = "debug-output")]
            eprintln!(
                "[NVML debug] process used_gpu_memory ({used}) > device total ({device_total}); \
                 falling back"
            );
            None
        }
        other => other,
    }
}

/// Enumerate every compute process on the given device, returning
/// `(pid, used_bytes)` for each row that survives the sentinel and
/// sanity checks.
///
/// Used by `crate::gpu_processes` on Linux, where `NVML`'s
/// `nvmlDeviceGetComputeRunningProcesses_v3` is the authoritative
/// source. On Windows the same call returns `NVML_VALUE_NOT_AVAILABLE`
/// under `WDDM` and the dispatcher falls back to `nvidia-smi`.
///
/// Returns `None` when `NVML` cannot be loaded, `nvmlInit_v2` /
/// `nvmlDeviceGetHandleByIndex_v2` / `nvmlDeviceGetMemoryInfo` fail, or
/// `nvmlDeviceGetComputeRunningProcesses_v3` returns an error other than
/// `NVML_ERROR_INSUFFICIENT_SIZE` (which we treat as a soft success and
/// keep the first 64 entries — see [`NVML_MAX_PROCESSES`]).
///
/// Per-row filtering matches [`read_process_used`]:
/// - `used_gpu_memory == u64::MAX` (R570 sentinel) → row dropped.
/// - `used_gpu_memory > device_total` → row dropped (impossible under
///   normal conditions; assumed garbage).
///
/// Caps at 64 processes per device — the existing `NVML_MAX_PROCESSES`
/// stack-buffer size. Documented limit; sufficient for any realistic
/// machine.
#[allow(unsafe_code)]
#[must_use]
pub(super) fn list_compute_processes(idx: u32) -> Option<Vec<(u32, u64)>> {
    // SAFETY: same justification as in `query`.
    let lib = unsafe { libloading::Library::new(NVML_LIB_PATH) }.ok()?;

    // SAFETY: same — symbol names match the documented NVML C API.
    let init: libloading::Symbol<'_, NvmlInitFn> = unsafe { lib.get(b"nvmlInit_v2\0") }.ok()?;
    let shutdown: libloading::Symbol<'_, NvmlShutdownFn> =
        unsafe { lib.get(b"nvmlShutdown\0") }.ok()?;
    let get_handle: libloading::Symbol<'_, NvmlDeviceGetHandleByIndexFn> =
        unsafe { lib.get(b"nvmlDeviceGetHandleByIndex_v2\0") }.ok()?;
    let get_memory: libloading::Symbol<'_, NvmlDeviceGetMemoryInfoFn> =
        unsafe { lib.get(b"nvmlDeviceGetMemoryInfo\0") }.ok()?;
    let get_processes: libloading::Symbol<'_, NvmlDeviceGetComputeRunningProcessesFn> =
        unsafe { lib.get(b"nvmlDeviceGetComputeRunningProcesses_v3\0") }.ok()?;

    // SAFETY: nvmlInit_v2 is reentrant + thread-safe.
    let ret = unsafe { init() };
    if ret != NVML_SUCCESS {
        #[cfg(feature = "debug-output")]
        eprintln!("[NVML debug] nvmlInit_v2 returned {ret} in list_compute_processes");
        return None;
    }

    // From here, every return path MUST call shutdown to balance the init.

    // SAFETY: nvmlDeviceGetHandleByIndex_v2 writes a valid opaque handle
    // into `device` when it returns NVML_SUCCESS.
    let mut device: NvmlDevice = std::ptr::null_mut();
    let ret = unsafe { get_handle(idx, &raw mut device) };
    if ret != NVML_SUCCESS {
        #[cfg(feature = "debug-output")]
        eprintln!(
            "[NVML debug] nvmlDeviceGetHandleByIndex_v2(idx={idx}) returned {ret} \
             in list_compute_processes"
        );
        // SAFETY: nvmlShutdown is always safe to call after a successful nvmlInit.
        unsafe { shutdown() };
        return None;
    }

    // SAFETY: nvmlDeviceGetMemoryInfo writes into the caller-provided
    // NvmlMemoryInfo struct. Used to bound the per-row sanity check.
    let mut mem_info = NvmlMemoryInfo {
        total: 0,
        free: 0,
        used: 0,
    };
    let ret = unsafe { get_memory(device, &raw mut mem_info) };
    if ret != NVML_SUCCESS {
        #[cfg(feature = "debug-output")]
        eprintln!("[NVML debug] nvmlDeviceGetMemoryInfo returned {ret} in list_compute_processes");
        // SAFETY: nvmlShutdown after init.
        unsafe { shutdown() };
        return None;
    }
    let device_total = mem_info.total;

    // CAST: usize → u32, NVML_MAX_PROCESSES = 64 fits in u32
    #[allow(clippy::as_conversions, clippy::cast_possible_truncation)]
    let mut count = NVML_MAX_PROCESSES as u32;
    let mut infos = [NvmlProcessInfo {
        pid: 0,
        used_gpu_memory: 0,
        gpu_instance_id: 0,
        compute_instance_id: 0,
    }; NVML_MAX_PROCESSES];

    // SAFETY: nvmlDeviceGetComputeRunningProcesses_v3 fills `infos` with
    // up to `count` entries and updates `count` to the actual number
    // written. On NVML_ERROR_INSUFFICIENT_SIZE we still have the first
    // 64 entries and treat that as a soft success.
    let ret = unsafe { get_processes(device, &raw mut count, infos.as_mut_ptr()) };

    // SAFETY: nvmlShutdown balances the matched nvmlInit_v2.
    unsafe { shutdown() };

    if ret != NVML_SUCCESS && ret != NVML_ERROR_INSUFFICIENT_SIZE {
        #[cfg(feature = "debug-output")]
        eprintln!(
            "[NVML debug] nvmlDeviceGetComputeRunningProcesses_v3 returned {ret} \
             in list_compute_processes (likely WDDM NVML_VALUE_NOT_AVAILABLE)"
        );
        return None;
    }

    // CAST: u32 → usize, NVML count is bounded by buffer size; usize >= 32 bits everywhere
    #[allow(clippy::as_conversions)]
    let actual_count = (count as usize).min(NVML_MAX_PROCESSES);

    // BORROW: explicit slice + filter_map — bypasses the R570 u64::MAX
    // sentinel and the used > device_total sanity check on a per-row
    // basis, matching `read_process_used`'s policy.
    let rows: Vec<(u32, u64)> = infos
        .get(..actual_count)
        .map(|s| {
            s.iter()
                .filter_map(|info| {
                    if info.used_gpu_memory == u64::MAX {
                        #[cfg(feature = "debug-output")]
                        eprintln!(
                            "[NVML debug] list_compute_processes: pid {} used_gpu_memory == u64::MAX \
                             (R570 sentinel); dropping row",
                            info.pid
                        );
                        None
                    } else if info.used_gpu_memory > device_total {
                        #[cfg(feature = "debug-output")]
                        eprintln!(
                            "[NVML debug] list_compute_processes: pid {} used_gpu_memory ({}) > \
                             device total ({device_total}); dropping row",
                            info.pid, info.used_gpu_memory
                        );
                        None
                    } else {
                        Some((info.pid, info.used_gpu_memory))
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    #[cfg(feature = "debug-output")]
    eprintln!(
        "[NVML debug] list_compute_processes(idx={idx}): {} row(s) after filtering \
         ({} reported by NVML, {} buffer cap)",
        rows.len(),
        count,
        NVML_MAX_PROCESSES
    );

    Some(rows)
}

/// Number of NVIDIA GPUs visible to `NVML`.
///
/// Returns `None` if `NVML` can't be loaded or `nvmlDeviceGetCount_v2`
/// fails. Used by the public `device_count()` dispatcher and (when
/// available) for bounds-checking `idx` in `device_info`.
#[allow(unsafe_code)]
pub(super) fn device_count() -> Option<u32> {
    // SAFETY: same justifications as in `query`.
    let lib = unsafe { libloading::Library::new(NVML_LIB_PATH) }.ok()?;

    // SAFETY: same — symbol names match the documented NVML C API.
    let init: libloading::Symbol<'_, NvmlInitFn> = unsafe { lib.get(b"nvmlInit_v2\0") }.ok()?;
    let shutdown: libloading::Symbol<'_, NvmlShutdownFn> =
        unsafe { lib.get(b"nvmlShutdown\0") }.ok()?;
    let get_count: libloading::Symbol<'_, NvmlDeviceGetCountFn> =
        unsafe { lib.get(b"nvmlDeviceGetCount_v2\0") }.ok()?;

    // SAFETY: nvmlInit_v2 is reentrant + thread-safe.
    let ret = unsafe { init() };
    if ret != NVML_SUCCESS {
        #[cfg(feature = "debug-output")]
        eprintln!("[NVML debug] nvmlInit_v2 returned {ret} in device_count");
        return None;
    }

    let mut count: u32 = 0;
    // SAFETY: nvmlDeviceGetCount_v2 writes one u32 to the caller-provided pointer.
    let ret = unsafe { get_count(&raw mut count) };

    // SAFETY: nvmlShutdown balances the matched nvmlInit_v2.
    unsafe { shutdown() };

    if ret == NVML_SUCCESS {
        #[cfg(feature = "debug-output")]
        eprintln!("[NVML debug] device_count = {count}");
        Some(count)
    } else {
        #[cfg(feature = "debug-output")]
        eprintln!("[NVML debug] nvmlDeviceGetCount_v2 returned {ret}");
        None
    }
}
