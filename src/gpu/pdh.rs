// SPDX-License-Identifier: MIT OR Apache-2.0

//! Windows `PDH` (Performance Data Helper) per-process `VRAM` backend.
//!
//! Closes the per-process-memory gap on consumer Windows / `WDDM` by
//! reading `\GPU Process Memory(*)\Dedicated Usage` through `pdh.dll`.
//! Same `VidMm` data Task Manager surfaces, accessed through a stable
//! and documented C API.
//!
//! # Background
//!
//! Under `WDDM 2.0`+, the Windows kernel's video memory manager
//! (`VidMm`) owns per-process GPU memory accounting — not the GPU
//! vendor's driver. As a result, `NVML`'s
//! `nvmlDeviceGetComputeRunningProcesses_v3` returns
//! `NVML_VALUE_NOT_AVAILABLE` for foreign processes on consumer
//! Windows, `IDXGIAdapter3::QueryVideoMemoryInfo` answers only for the
//! *calling* process, and `nvidia-smi --query-compute-apps` writes
//! `[N/A]` for `used_memory`. The data is recorded — Task Manager's
//! "Dedicated GPU memory" column proves it — but every backend
//! `hypomnesis` queried before v0.2.2 was blind to it.
//!
//! Microsoft exposes the same `VidMm` data through the Performance
//! Data Helper (`PDH`) API. The relevant counter set is `GPU Process
//! Memory`, with instances of the form
//! `pid_NNNN_luid_0xHHHHHHHH_0xHHHHHHHH_phys_N`. `PDH` is documented,
//! has a stable C ABI, and is callable from any program — no admin
//! elevation required for read access.
//!
//! # `used_bytes` semantics: dedicated commit, not resident set
//!
//! The `Dedicated Usage` counter reports `VidMm`'s **committed**
//! allocation total for the process, **not** what is resident on the
//! GPU at sample time. Under `WDDM` a process can commit GPU
//! allocations exceeding physical `VRAM` — the kernel pages the
//! committed pages over the shared system memory budget. As a result,
//! a single heavy-graphics process (browser, modern editor with GPU
//! compositing) can show `used_bytes` exceeding the device's physical
//! `VRAM`; on a 16 GiB card the maintainer has observed Firefox at
//! ~15 GiB committed. These numbers are not bugs in `hypomnesis` —
//! they match what Task Manager's `Dedicated GPU memory` column
//! displays, because both ultimately read from the same `VidMm`
//! ledger. Consumers wanting "resident bytes only" must look
//! elsewhere (there is no public `WDDM` API for it; ETW provides it
//! at the cost of a heavy session setup, out of scope here).
//!
//! # Adapter targeting
//!
//! Each `PDH` instance name encodes the WDDM adapter `LUID`. To
//! attribute a per-process row to a specific `hypomnesis`
//! `device_index`, this module fetches the target adapter's `LUID`
//! once via the existing `DXGI` walk
//! ([`crate::gpu::dxgi::adapter_luid`]) and filters `PDH` instances
//! by it. Single-`GPU` systems (the common case) match every instance;
//! multi-`GPU` systems get correctly attributed rows.
//!
//! # Known accounting drift ([`KB 4490156`])
//!
//! Windows' `GPU Process Memory` counters can over-report per-process
//! `VRAM` by roughly 100 MiB per cycle for applications that go
//! through repeated discard-and-restore of GPU caches (the documented
//! example is Office in Low Resource Mode: hide the window, the UMD
//! flushes cached GPU resources, restore the window, the UMD
//! re-creates them — the counter accumulates both instead of
//! decrementing the discarded set). This is a `WDDM 2.0`+
//! architectural artefact: `VidMm` asynchronously defers allocation
//! destruction, and the user-mode driver's discards may not yet be
//! reflected at counter-sample time. Microsoft considers this a known
//! issue with no fix as of 2026.
//!
//! Compute workloads (`CUDA`, `PyTorch`, `llama.cpp`, `ollama`,
//! `candle`) do **not** exhibit the trigger pattern — they allocate
//! coarse-grained buffers and hold them for the duration of a run.
//! For the `hmn ps` target use case the drift is in practice zero.
//! Tools needing byte-exact accounting for graphics-cache-flush
//! workloads should prefer Task Manager's *Performance* pane or
//! `WPR` / `WPA` (`ETW`-based), both of which use independent paths
//! through `VidMm` and are unaffected.
//!
//! [`KB 4490156`]: https://learn.microsoft.com/en-us/troubleshoot/windows-client/performance/gpu-process-memory-counters-report-wrong-value
//!
//! # Memory aggregation
//!
//! `PDH` exposes per-`(pid, segment)` rows via the `phys_N` suffix in
//! each instance name. This module aggregates by `pid` before
//! returning, producing one entry per process. The internal helper
//! [`collect_segmented_rows`] preserves the per-segment data and is
//! kept private deliberately — see its doc-comment for the deferred
//! segmented-API rationale.

use std::collections::HashMap;

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Performance::{
    PDH_FMT_COUNTERVALUE, PDH_FMT_LARGE, PDH_HCOUNTER, PDH_HQUERY, PERF_DETAIL_WIZARD,
    PdhAddCounterW, PdhCloseQuery, PdhCollectQueryData, PdhEnumObjectItemsW,
    PdhGetFormattedCounterValue, PdhOpenQueryW,
};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::core::{PCWSTR, PWSTR, w};

use crate::{HypomnesisError, Result};

// -----------------------------------------------------------------------
// PDH status codes
// -----------------------------------------------------------------------

/// `PDH` status: function completed successfully.
const PDH_SUCCESS: u32 = 0;

/// `PDH` status: caller's buffer was too small; `pcch*` parameters now
/// hold the required size. Expected on the first
/// [`PdhEnumObjectItemsW`] call (size query) and must be re-issued
/// with adequately sized buffers.
const PDH_MORE_DATA: u32 = 0x8000_07D2;

/// `PDH` status: the named object (counter set) is not registered on
/// the system. For `\GPU Process Memory` this signals a pre-`WDDM 2.0`
/// driver or a `Windows` SKU that doesn't carry the `GPU` performance
/// providers — callers should fall back to a different backend.
const PDH_CSTATUS_NO_OBJECT: u32 = 0xC000_0BB8;

// -----------------------------------------------------------------------
// Internal types
// -----------------------------------------------------------------------

/// One raw `PDH` row keyed by `(pid, segment)`.
///
/// Returned by [`collect_segmented_rows`] before per-`pid` aggregation
/// in [`query_per_process_vram`]. The `segment_idx` corresponds to the
/// `phys_N` suffix on the `PDH` instance name and identifies one
/// memory partition on the target adapter. On single-segment hardware
/// (the common consumer case) every `pid` produces exactly one row;
/// on multi-segment hardware a single `pid` may produce multiple rows
/// that aggregate together.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SegmentRow {
    /// OS process ID parsed from the `pid_NNNN_` prefix.
    pid: u32,
    /// Memory partition index parsed from the trailing `_phys_N` suffix.
    /// Currently informational only; aggregated away in the public
    /// query path.
    segment_idx: u32,
    /// Dedicated `VRAM` bytes for this `(pid, segment)` row, read from
    /// `\GPU Process Memory(<instance>)\Dedicated Usage` via
    /// [`PdhGetFormattedCounterValue`] with `PDH_FMT_LARGE`.
    used_bytes: u64,
}

/// `RAII` guard for a `PDH` query handle.
///
/// Ensures [`PdhCloseQuery`] runs on every exit path — including
/// early-return on intermediate `PDH` errors — without scattering
/// manual cleanup through the function body.
struct QueryGuard {
    /// `PDH` query handle returned by [`PdhOpenQueryW`]. Treated as an
    /// opaque kernel-side capability; never read or modified from
    /// userspace except through `PDH` APIs.
    handle: PDH_HQUERY,
}

impl Drop for QueryGuard {
    fn drop(&mut self) {
        // SAFETY: `self.handle` was obtained from a successful
        // PdhOpenQueryW call (constructor invariant). PdhCloseQuery is
        // documented as safe to call on any valid query handle and
        // releases all associated counter handles in one shot. Return
        // status is intentionally discarded — cleanup is best-effort.
        #[allow(unsafe_code)]
        unsafe {
            let _ = PdhCloseQuery(self.handle);
        }
    }
}

// -----------------------------------------------------------------------
// Pure helpers (unit-testable without FFI)
// -----------------------------------------------------------------------

/// Parse a `PDH` `\GPU Process Memory` instance name into
/// `(pid, (luid_high, luid_low), segment_idx)`.
///
/// Format: `pid_NNNN_luid_0xHHHHHHHH_0xHHHHHHHH_phys_N`. The `LUID`
/// `HighPart` is a Windows `LONG` (`i32`); `PDH` writes its bit
/// pattern as unsigned hex, so we parse as `u32` then bit-reinterpret.
/// Returns `None` for malformed or non-`GPU Process Memory` instance
/// names (e.g., the `_Total` instance, or instances from an unrelated
/// counter set if the API ever reuses them).
#[must_use]
fn parse_instance_name(name: &str) -> Option<(u32, (i32, u32), u32)> {
    let rest = name.strip_prefix("pid_")?;
    let (pid_str, rest) = rest.split_once('_')?;
    let pid: u32 = pid_str.parse().ok()?;

    let rest = rest.strip_prefix("luid_0x")?;
    let (high_str, rest) = rest.split_once("_0x")?;
    let high_u32: u32 = u32::from_str_radix(high_str, 16).ok()?;
    // CAST: u32 → i32, bit-reinterpret of `LUID::HighPart`. `PDH` writes
    // the field as unsigned hex regardless of the underlying `LONG`
    // signedness; round-tripping via `as` preserves the bit pattern.
    #[allow(clippy::as_conversions, clippy::cast_possible_wrap)]
    let high = high_u32 as i32;

    let (low_str, seg_str) = rest.split_once("_phys_")?;
    let low: u32 = u32::from_str_radix(low_str, 16).ok()?;

    let segment_idx: u32 = seg_str.parse().ok()?;

    Some((pid, (high, low), segment_idx))
}

/// Parse `PDH`'s multi-string buffer into individual instance names.
///
/// `PDH` returns instance lists as a contiguous sequence of
/// NUL-terminated UTF-16 strings, terminated by an extra NUL (a
/// "double-NUL terminator"). This helper splits on NULs, lossily
/// decodes each chunk to UTF-8, and drops any empty trailing entries.
#[must_use]
fn parse_multi_string(buf: &[u16]) -> Vec<String> {
    buf.split(|&c| c == 0)
        .filter(|s| !s.is_empty())
        .map(String::from_utf16_lossy)
        .collect()
}

// -----------------------------------------------------------------------
// PDH FFI: counter enumeration + value collection
// -----------------------------------------------------------------------

/// Open a `PDH` query, enumerate `GPU Process Memory` instances,
/// filter to those matching `target_luid`, sample each, and return one
/// [`SegmentRow`] per matching instance.
///
/// Kept **private** (not `pub(super)`) deliberately: promoting this
/// function to `pub(super)` is the no-refactor path for exposing a
/// future `query_per_process_vram_segmented()` library API and / or a
/// `hmn ps --show-segments` CLI flag. The aggregation step in
/// [`query_per_process_vram`] collapses per-segment rows into one row
/// per `pid`; downstream segmented exposure would skip that step. See
/// `ROADMAP.md` (Speculative section) and `docs/roadmap-v0.2.2.md`
/// "Out of scope" table for the deferred segmented-API rationale, and
/// the [module-level doc-comment][self] for the data-source overview.
///
/// # Errors
///
/// Returns [`HypomnesisError::Pdh`] when [`PdhOpenQueryW`] fails,
/// when [`PdhEnumObjectItemsW`] returns a status other than
/// `PDH_MORE_DATA` (size query) or [`PDH_SUCCESS`] (data fetch), or
/// when the `GPU Process Memory` counter set is unregistered
/// (`PDH_CSTATUS_NO_OBJECT` — pre-`WDDM 2.0` driver). Per-instance
/// failures during [`PdhAddCounterW`] or
/// [`PdhGetFormattedCounterValue`] are best-effort: the affected row
/// is dropped, other rows still report.
#[allow(unsafe_code)]
fn collect_segmented_rows(target_luid: (i32, u32)) -> Result<Vec<SegmentRow>> {
    // ---- 1. Open the query ------------------------------------------------
    let mut raw_handle = PDH_HQUERY::default();
    // SAFETY: PdhOpenQueryW with NULL data-source and 0 user-data is the
    // documented "open a new realtime query against the local performance
    // data source" form. `phquery` is a valid out-parameter pointer to a
    // stack-allocated PDH_HQUERY (zero-initialised). Status return is
    // checked below; on success the handle is moved into the RAII guard
    // immediately.
    let status = unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &raw mut raw_handle) };
    if status != PDH_SUCCESS {
        return Err(HypomnesisError::Pdh(format!(
            "PdhOpenQueryW failed: 0x{status:08X}"
        )));
    }
    let query = QueryGuard { handle: raw_handle };

    // ---- 2. Enumerate instances (size query) ------------------------------
    let mut counter_size: u32 = 0;
    let mut instance_size: u32 = 0;
    // SAFETY: PdhEnumObjectItemsW called with None buffers is the documented
    // "tell me the required buffer sizes" mode. The two pcch* parameters are
    // valid mutable references; the object-name literal is a static UTF-16
    // string from `w!`. PDH writes only into the size out-parameters on this
    // call and returns PDH_MORE_DATA on success.
    let status = unsafe {
        PdhEnumObjectItemsW(
            PCWSTR::null(),
            PCWSTR::null(),
            w!("GPU Process Memory"),
            None,
            &raw mut counter_size,
            None,
            &raw mut instance_size,
            PERF_DETAIL_WIZARD,
            0,
        )
    };

    if status == PDH_CSTATUS_NO_OBJECT {
        return Err(HypomnesisError::Pdh(
            "GPU Process Memory counter set not registered (pre-WDDM 2.0?)".to_owned(),
        ));
    }

    // No instances at all → return empty. The query closes via Drop.
    if status == PDH_SUCCESS && instance_size <= 1 {
        return Ok(Vec::new());
    }

    if status != PDH_MORE_DATA {
        return Err(HypomnesisError::Pdh(format!(
            "PdhEnumObjectItemsW (size query) failed: 0x{status:08X}"
        )));
    }

    // ---- 3. Enumerate instances (data fetch) ------------------------------
    // CAST: u32 → usize, sizes are PDH-reported buffer lengths in u16
    // chars; fit trivially in usize on every supported platform.
    #[allow(clippy::as_conversions)]
    let mut counter_buffer: Vec<u16> = vec![0; counter_size as usize];
    #[allow(clippy::as_conversions)]
    let mut instance_buffer: Vec<u16> = vec![0; instance_size as usize];

    // SAFETY: PdhEnumObjectItemsW now called with PWSTR-wrapped buffers
    // sized per the previous PDH_MORE_DATA response. Buffer pointers are
    // valid for the full `counter_size` / `instance_size` UTF-16 chars (Vec
    // allocation matches). The size out-parameters get rewritten with the
    // actual written length. PWSTR (mutable wide) is the right wrapper here
    // because PDH writes into the buffers.
    let status = unsafe {
        PdhEnumObjectItemsW(
            PCWSTR::null(),
            PCWSTR::null(),
            w!("GPU Process Memory"),
            Some(PWSTR::from_raw(counter_buffer.as_mut_ptr())),
            &raw mut counter_size,
            Some(PWSTR::from_raw(instance_buffer.as_mut_ptr())),
            &raw mut instance_size,
            PERF_DETAIL_WIZARD,
            0,
        )
    };

    if status != PDH_SUCCESS {
        return Err(HypomnesisError::Pdh(format!(
            "PdhEnumObjectItemsW (data fetch) failed: 0x{status:08X}"
        )));
    }

    let instances = parse_multi_string(&instance_buffer);

    #[cfg(feature = "debug-output")]
    eprintln!("[PDH debug] enumerated {} instance(s)", instances.len());

    // ---- 4. Add a counter per matching instance ---------------------------
    // Stores (pid, segment_idx, counter handle) so we can correlate values
    // back to their source row after PdhCollectQueryData.
    let mut counter_handles: Vec<(u32, u32, PDH_HCOUNTER)> = Vec::new();

    for instance in &instances {
        let Some((pid, luid, segment_idx)) = parse_instance_name(instance) else {
            continue;
        };
        if luid != target_luid {
            continue;
        }

        // BORROW: explicit `encode_utf16` + chain(Some(0)) — PdhAddCounterW
        // requires a NUL-terminated UTF-16 string; the encoded path is
        // owned for the duration of the call.
        let counter_path = format!("\\GPU Process Memory({instance})\\Dedicated Usage");
        let counter_path_wide: Vec<u16> = counter_path
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();

        let mut h_counter = PDH_HCOUNTER::default();
        // SAFETY: query.handle is valid (RAII invariant). counter_path_wide
        // is NUL-terminated and lives until end of iteration. h_counter is a
        // valid stack-allocated out-parameter (zero-initialised). Per-instance
        // failure is best-effort: skip the row rather than abort the whole
        // enumeration.
        let status = unsafe {
            PdhAddCounterW(
                query.handle,
                PCWSTR::from_raw(counter_path_wide.as_ptr()),
                0,
                &raw mut h_counter,
            )
        };

        if status == PDH_SUCCESS {
            counter_handles.push((pid, segment_idx, h_counter));
        } else {
            #[cfg(feature = "debug-output")]
            eprintln!(
                "[PDH debug] PdhAddCounterW failed for instance {instance:?}: 0x{status:08X} (skipped)"
            );
        }
    }

    if counter_handles.is_empty() {
        return Ok(Vec::new());
    }

    // ---- 5. Collect one sample for the whole query ------------------------
    // SAFETY: query.handle is valid; PdhCollectQueryData samples every
    // counter added to the query in a single call. `Dedicated Usage` is an
    // instantaneous gauge (not a rate), so one sample suffices.
    let status = unsafe { PdhCollectQueryData(query.handle) };
    if status != PDH_SUCCESS {
        return Err(HypomnesisError::Pdh(format!(
            "PdhCollectQueryData failed: 0x{status:08X}"
        )));
    }

    // ---- 6. Read each counter's formatted value ---------------------------
    let mut rows: Vec<SegmentRow> = Vec::with_capacity(counter_handles.len());
    for (pid, segment_idx, h_counter) in counter_handles {
        let mut value = PDH_FMT_COUNTERVALUE::default();
        // SAFETY: h_counter was returned by a successful PdhAddCounterW
        // within this query; the query has been sampled (step 5). value is
        // a valid stack-allocated out-parameter. PDH_FMT_LARGE selects the
        // `largeValue` (i64) union arm, set on success.
        let status =
            unsafe { PdhGetFormattedCounterValue(h_counter, PDH_FMT_LARGE, None, &raw mut value) };

        if status != PDH_SUCCESS {
            #[cfg(feature = "debug-output")]
            eprintln!(
                "[PDH debug] PdhGetFormattedCounterValue failed for pid={pid} seg={segment_idx}: 0x{status:08X} (skipped)"
            );
            continue;
        }

        // SAFETY: PDH_FMT_LARGE was passed to PdhGetFormattedCounterValue
        // and the call succeeded; per Microsoft docs that sets the
        // `largeValue` arm of the union. Reading any other arm would be
        // unsound, but we read exactly the arm we requested.
        let raw_value: i64 = unsafe { value.Anonymous.largeValue };

        // CAST: i64 → u64, `Dedicated Usage` is a byte count and is
        // documented non-negative. Negative values would indicate a
        // counter-implementation bug (sentinel) — skip with debug trace.
        if raw_value < 0 {
            #[cfg(feature = "debug-output")]
            eprintln!(
                "[PDH debug] negative Dedicated Usage for pid={pid} seg={segment_idx}: {raw_value} (skipped)"
            );
            continue;
        }
        #[allow(clippy::as_conversions, clippy::cast_sign_loss)]
        let used_bytes = raw_value as u64;

        rows.push(SegmentRow {
            pid,
            segment_idx,
            used_bytes,
        });
    }

    // Query closes here via QueryGuard::drop.
    Ok(rows)
}

// -----------------------------------------------------------------------
// Public entry point
// -----------------------------------------------------------------------

/// Per-process dedicated `VRAM` for every process holding memory on
/// the adapter at `device_index`, aggregated across memory segments.
///
/// Returned vector has **at most one entry per `pid`** — segmented
/// `PDH` rows for the same process on the target adapter are summed
/// before return.
///
/// # Errors
///
/// Returns [`HypomnesisError::Pdh`] if the `DXGI` walk cannot locate
/// the requested `device_index`, or if any `PDH` enumeration / query
/// call fails fatally. The `GPU Process Memory` counter set being
/// unregistered (pre-`WDDM 2.0`) surfaces as a specific `PDH` error
/// message naming the cause, so [`crate::gpu::gpu_processes`] can
/// pattern-match and fall back to `nvidia-smi` if desired.
pub(super) fn query_per_process_vram(device_index: u32) -> Result<Vec<(u32, u64)>> {
    let target_luid = super::dxgi::adapter_luid(device_index).ok_or_else(|| {
        HypomnesisError::Pdh(format!(
            "no NVIDIA adapter at device_index {device_index} via DXGI walk"
        ))
    })?;

    let segments = collect_segmented_rows(target_luid)?;

    let mut by_pid: HashMap<u32, u64> = HashMap::with_capacity(segments.len());
    for row in segments {
        by_pid
            .entry(row.pid)
            .and_modify(|acc| *acc = acc.saturating_add(row.used_bytes))
            .or_insert(row.used_bytes);
    }

    Ok(by_pid.into_iter().collect())
}

// -----------------------------------------------------------------------
// Win32 process-name lookup (companion to PDH per-process VRAM)
// -----------------------------------------------------------------------

/// `RAII` guard for a `Win32` `HANDLE`.
///
/// Closes the handle via [`CloseHandle`] on drop. Mirrors the
/// `QueryGuard` pattern above for the `PDH` query handle: every
/// successful `OpenProcess` is paired with exactly one `CloseHandle`
/// regardless of which return path the caller takes.
struct HandleGuard {
    /// `Win32` handle returned by [`OpenProcess`]. Treated as an
    /// opaque kernel-side capability; never read or modified from
    /// userspace except through `Win32` APIs.
    handle: HANDLE,
}

impl Drop for HandleGuard {
    fn drop(&mut self) {
        // SAFETY: `self.handle` was obtained from a successful
        // OpenProcess call (constructor invariant). CloseHandle is
        // documented as safe to call on any valid handle. The return
        // status is intentionally discarded — cleanup is best-effort.
        #[allow(unsafe_code)]
        unsafe {
            let _ = CloseHandle(self.handle);
        }
    }
}

/// Buffer length (in UTF-16 chars) for [`name_from_pid_windows`]'s
/// path read. ~2 KiB stack. Covers any Win32-namespace executable
/// path on a real system; the `\\?\` long-path namespace (up to
/// 32767 chars) is out of scope — no realistic executable uses it.
const NAME_BUF_LEN: usize = 1024;

/// Synthetic display name for `PID 4`, the Windows kernel
/// pseudo-process.
///
/// Defined as a named constant so the security-relevant intent —
/// "this row is the kernel itself, not an unresolvable user process" —
/// is greppable from a single place.
const KERNEL_PROCESS_NAME: &str = "[kernel]";

/// Map a Windows PID to a synthetic name when the PID is a kernel
/// pseudo-process (currently only `PID 4`). Returns `None` for every
/// other PID — the caller falls through to the `OpenProcess`-based
/// lookup.
///
/// Why a special case: `PID 4` is the Windows kernel itself, owning
/// all kernel-mode threads. There is no executable image to read, so
/// [`QueryFullProcessImageNameW`] fails for fundamental architectural
/// reasons rather than privilege reasons. Without this special case,
/// `PID 4` would render as `?` in `hmn ps` — indistinguishable from a
/// foreign-user process that would resolve under elevation, which is
/// a real security signal hidden by the noise. Mapping `PID 4` to
/// `[kernel]` removes the most common false positive from the
/// "unresolvable even elevated" set, leaving only genuinely
/// suspicious `?` rows to investigate.
///
/// The PID-4-is-kernel convention has been stable on Windows since
/// at least NT 5.0 (Windows 2000); Microsoft has not signalled any
/// intent to change it.
#[must_use]
const fn kernel_name_for_pid(pid: u32) -> Option<&'static str> {
    if pid == 4 {
        Some(KERNEL_PROCESS_NAME)
    } else {
        None
    }
}

/// Resolve a `Windows` `PID` to its executable basename, using
/// `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, FALSE, pid)` +
/// `QueryFullProcessImageNameW(PROCESS_NAME_WIN32, ...)` followed by
/// [`basename_from_path`].
///
/// Returns `None` on any of these failure modes:
///
/// - The target process exited between enumeration and lookup
///   (`OpenProcess` fails with `ERROR_INVALID_PARAMETER`).
/// - Access denied (cross-user or protected processes — Windows
///   restricts handle acquisition the same way `task_for_pid` does
///   on macOS). Mirrors the Linux behaviour where unreadable
///   `/proc/<pid>/comm` yields `None`.
/// - `QueryFullProcessImageNameW` fails (very short buffer,
///   pathologically long path, kernel error). Buffer is sized to
///   [`NAME_BUF_LEN`] UTF-16 chars — covers any reasonable Win32
///   path; the `\\?\` long-path namespace (up to 32767 chars) is
///   intentionally not supported because no realistic executable
///   lives there.
///
/// **Privilege model.** No admin or special privilege required for
/// processes owned by the calling user. Foreign-user PIDs return
/// `None` without `SeDebugPrivilege`; this is the documented Windows
/// security gate, not a hypomnesis limitation. Callers wanting
/// system-wide visibility should run elevated.
#[allow(unsafe_code)]
#[must_use]
pub(super) fn name_from_pid_windows(pid: u32) -> Option<String> {
    // Kernel pseudo-process short-circuit: PID 4 has no executable
    // image, so the OpenProcess path below would fail and produce a
    // `?` row. Render it as `[kernel]` instead so it isn't confused
    // with foreign-user / privileged processes that would resolve
    // under elevation. See `kernel_name_for_pid` doc-comment for the
    // security rationale.
    if let Some(synthetic) = kernel_name_for_pid(pid) {
        return Some(synthetic.to_owned());
    }

    // SAFETY: OpenProcess is a documented Win32 function. Failure modes
    // (PID exited, access-denied, invalid PID) all surface as Err; the
    // .ok()? converts that into a None return without leaking any
    // partially-acquired state.
    let raw_handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
    let guard = HandleGuard { handle: raw_handle };

    let mut buf = [0_u16; NAME_BUF_LEN];
    // CAST: usize → u32, NAME_BUF_LEN is a const 1024 → fits trivially.
    #[allow(clippy::as_conversions, clippy::cast_possible_truncation)]
    let mut size: u32 = NAME_BUF_LEN as u32;

    // SAFETY: guard.handle is valid (RAII invariant). buf is a stack
    // array sized to NAME_BUF_LEN; the PWSTR wrapper points into it.
    // `size` is initialised to the buffer capacity and rewritten by
    // the call to the actual length written (excluding the trailing
    // NUL on success). PROCESS_NAME_WIN32 (= 0) requests Win32-
    // namespace path format (drive-letter `C:\...`), not the NT
    // namespace (`\Device\HarddiskVolume...`).
    let result = unsafe {
        QueryFullProcessImageNameW(
            guard.handle,
            PROCESS_NAME_WIN32,
            PWSTR::from_raw(buf.as_mut_ptr()),
            &raw mut size,
        )
    };

    if result.is_err() {
        // Drop runs at end of scope; explicit `drop(guard)` not
        // needed. Naming the cleanup invariant here keeps the
        // error-path and happy-path symmetrical for readers.
        return None;
    }

    // CAST: u32 → usize, `size` was clamped by NAME_BUF_LEN going in
    // and the kernel rewrites with the actual char count written; fits
    // in usize.
    #[allow(clippy::as_conversions)]
    let written = (size as usize).min(NAME_BUF_LEN);

    // BORROW: explicit slice into the written prefix — UTF-16 decode
    // must not include the uninitialised tail of the buffer.
    #[allow(clippy::indexing_slicing)]
    let full_path = String::from_utf16_lossy(&buf[..written]);
    let base = basename_from_path(&full_path);
    if base.is_empty() { None } else { Some(base) }
}

/// Extract the basename (final path component) from a `Windows` path.
///
/// Handles both `\` and `/` separators (`Windows` accepts both in many
/// API contexts; some processes register their image path with mixed
/// separators). Returns an owned `String`; the input path is borrowed
/// only for the split.
///
/// Edge cases:
/// - Empty input → empty output.
/// - Path with no separator → whole path returned (e.g., bare image
///   names that some kernel-mode processes register with).
/// - Trailing separator → empty basename (e.g., `"C:\\Windows\\"`).
#[must_use]
fn basename_from_path(path: &str) -> String {
    path.rsplit_once(['\\', '/'])
        .map_or_else(|| path.to_owned(), |(_, base)| base.to_owned())
}

// -----------------------------------------------------------------------
// Inline tests (pure helpers only — FFI exercised via live tests in
// `tests/live_pdh.rs`)
// -----------------------------------------------------------------------

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::missing_docs_in_private_items
)]
mod tests {
    use super::{basename_from_path, kernel_name_for_pid, parse_instance_name, parse_multi_string};

    #[test]
    fn parse_instance_name_basic() {
        let (pid, luid, seg) =
            parse_instance_name("pid_24168_luid_0x00000000_0x00012345_phys_0").unwrap();
        assert_eq!(pid, 24168);
        assert_eq!(luid, (0, 0x0001_2345));
        assert_eq!(seg, 0);
    }

    #[test]
    fn parse_instance_name_nonzero_segment() {
        let (pid, _luid, seg) =
            parse_instance_name("pid_4242_luid_0x00000000_0x00000abc_phys_3").unwrap();
        assert_eq!(pid, 4242);
        assert_eq!(seg, 3);
    }

    #[test]
    fn parse_instance_name_high_bit_luid() {
        // PDH writes LUID HighPart as unsigned hex; 0xFFFFFFFF round-trips
        // to i32::MIN-adjacent bit pattern via `as i32`.
        let (_pid, luid, _seg) =
            parse_instance_name("pid_1_luid_0xFFFFFFFF_0x00000001_phys_0").unwrap();
        assert_eq!(luid.0, -1_i32);
        assert_eq!(luid.1, 1);
    }

    #[test]
    fn parse_instance_name_full_low_part() {
        let (_pid, luid, _seg) =
            parse_instance_name("pid_1_luid_0x00000000_0xDEADBEEF_phys_0").unwrap();
        assert_eq!(luid, (0, 0xDEAD_BEEF));
    }

    #[test]
    fn parse_instance_name_rejects_missing_prefix() {
        assert!(parse_instance_name("luid_0x00000000_0x00000001_phys_0").is_none());
        assert!(parse_instance_name("_Total").is_none());
        assert!(parse_instance_name("").is_none());
    }

    #[test]
    fn parse_instance_name_rejects_non_hex_luid() {
        assert!(parse_instance_name("pid_1_luid_0xZZZZZZZZ_0x00000001_phys_0").is_none());
        assert!(parse_instance_name("pid_1_luid_0x00000000_0xZZZZZZZZ_phys_0").is_none());
    }

    #[test]
    fn parse_instance_name_rejects_non_numeric_pid() {
        assert!(parse_instance_name("pid_abc_luid_0x00000000_0x00000001_phys_0").is_none());
    }

    #[test]
    fn parse_instance_name_rejects_missing_phys_suffix() {
        assert!(parse_instance_name("pid_1_luid_0x00000000_0x00000001").is_none());
        assert!(parse_instance_name("pid_1_luid_0x00000000_0x00000001_phys_").is_none());
    }

    #[test]
    fn parse_multi_string_basic() {
        // "foo\0bar\0\0" — two strings, double-NUL terminator
        let buf: Vec<u16> = "foo\0bar\0\0".encode_utf16().collect();
        let parsed = parse_multi_string(&buf);
        assert_eq!(parsed, vec!["foo".to_owned(), "bar".to_owned()]);
    }

    #[test]
    fn parse_multi_string_empty() {
        let buf: Vec<u16> = vec![0, 0];
        let parsed = parse_multi_string(&buf);
        assert!(parsed.is_empty());
    }

    #[test]
    fn parse_multi_string_single_entry() {
        let buf: Vec<u16> = "only\0\0".encode_utf16().collect();
        let parsed = parse_multi_string(&buf);
        assert_eq!(parsed, vec!["only".to_owned()]);
    }

    // -------------------------------------------------------------------
    // basename_from_path tests (Win32 process-name lookup support)
    // -------------------------------------------------------------------

    #[test]
    fn basename_from_path_windows_backslashes() {
        assert_eq!(
            basename_from_path("C:\\Program Files\\Mozilla Firefox\\firefox.exe"),
            "firefox.exe"
        );
    }

    #[test]
    fn basename_from_path_forward_slashes() {
        // Some Win32 APIs accept forward slashes; processes may register
        // their image path with them.
        assert_eq!(
            basename_from_path("C:/Users/Eric/AppData/ollama.exe"),
            "ollama.exe"
        );
    }

    #[test]
    fn basename_from_path_mixed_separators() {
        assert_eq!(
            basename_from_path("C:\\Users/Eric\\AppData/ollama.exe"),
            "ollama.exe"
        );
    }

    #[test]
    fn basename_from_path_no_separator() {
        // Some kernel-mode processes register a bare image name.
        assert_eq!(basename_from_path("System"), "System");
        assert_eq!(basename_from_path("idle.exe"), "idle.exe");
    }

    #[test]
    fn basename_from_path_empty_input() {
        assert_eq!(basename_from_path(""), "");
    }

    #[test]
    fn basename_from_path_trailing_separator() {
        // Pathological but possible — defensive coverage. Trailing
        // separator yields empty basename; the caller treats empty as
        // "no name available" and returns None.
        assert_eq!(basename_from_path("C:\\Windows\\"), "");
        assert_eq!(basename_from_path("/tmp/"), "");
    }

    #[test]
    fn basename_from_path_single_separator() {
        // Just a separator → empty basename.
        assert_eq!(basename_from_path("\\"), "");
        assert_eq!(basename_from_path("/"), "");
    }

    // -------------------------------------------------------------------
    // kernel_name_for_pid tests (Wave C follow-up — security relevance)
    // -------------------------------------------------------------------

    #[test]
    fn kernel_name_for_pid_recognises_pid_4() {
        // The Windows kernel pseudo-process. Synthetic name is `[kernel]`.
        assert_eq!(kernel_name_for_pid(4), Some("[kernel]"));
    }

    #[test]
    fn kernel_name_for_pid_returns_none_for_other_pids() {
        // No other PIDs are special-cased. The caller falls through to
        // OpenProcess.
        assert_eq!(kernel_name_for_pid(0), None);
        assert_eq!(kernel_name_for_pid(1), None);
        assert_eq!(kernel_name_for_pid(3), None);
        assert_eq!(kernel_name_for_pid(5), None);
        assert_eq!(kernel_name_for_pid(1000), None);
        assert_eq!(kernel_name_for_pid(u32::MAX), None);
    }
}
