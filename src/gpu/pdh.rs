// SPDX-License-Identifier: MIT OR Apache-2.0

//! Windows `PDH` (Performance Data Helper) per-process `VRAM` backend.
//!
//! Closes the per-process-memory gap on consumer Windows / `WDDM` by
//! reading `\GPU Process Memory(*)\Dedicated Usage` and its sibling
//! `Shared Usage` counter through `pdh.dll`. Same `VidMm` data Task
//! Manager surfaces, accessed through a stable and documented C API.
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
//! # `shared_used_bytes` semantics: resident shared, the spill signal
//!
//! The `Shared Usage` sibling counter (v0.2.5) is a different animal:
//! it reports the process's **resident** shared-system-memory bytes —
//! the same quantity Task Manager's per-process *Shared GPU memory*
//! column shows. This — not the commit figure above — is the `WDDM`
//! spill signal: when `VidMm` pages GPU allocations out of dedicated
//! `VRAM`, the evicted pages become resident in shared system memory
//! and this counter grows. A compute-bound process routinely shows
//! `used_bytes` (commit) far above dedicated `VRAM` while
//! `shared_used_bytes` stays ≈ 0 — that is reservation headroom, not
//! spill (rhyme-mdlm dogfooding report, 2026-07-19). The
//! [`KB 4490156`] drift below afflicts the commit accounting, not
//! this residency gauge.
//!
//! Counter presence verified live on the reference `RTX 5060 Ti`
//! (2026-07-22, `typeperf -q "GPU Process Memory"`): `Shared Usage`
//! is enumerated alongside `Dedicated Usage`, `Local Usage`,
//! `Non Local Usage`, and `Total Committed`, with the identical
//! `pid_NNNN_luid_0xHHHHHHHH_0xHHHHHHHH_phys_N` instance mangling.
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
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
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
    /// `VidMm` dedicated **commit** semantics — see the module docs.
    used_bytes: u64,
    /// Resident shared-system-memory bytes for this `(pid, segment)`
    /// row, read from `\GPU Process Memory(<instance>)\Shared Usage` —
    /// the `WDDM` spill signal (see the module docs). `0` when the
    /// shared counter could not be added or read for this instance
    /// (best-effort degradation, the row itself is kept).
    shared_used_bytes: u64,
}

/// Counter handles for one enumerated `(pid, segment)` instance,
/// correlating sampled values back to their source row after the
/// single [`PdhCollectQueryData`] call.
struct InstanceCounters {
    /// OS process ID parsed from the instance name.
    pid: u32,
    /// Memory partition index parsed from the instance name.
    segment_idx: u32,
    /// `Dedicated Usage` counter handle. Always present — instances
    /// whose dedicated counter fails to add are skipped entirely.
    dedicated: PDH_HCOUNTER,
    /// `Shared Usage` counter handle. `None` when [`PdhAddCounterW`]
    /// failed for the shared path — best-effort: the row survives with
    /// `shared_used_bytes: 0` rather than being dropped.
    shared: Option<PDH_HCOUNTER>,
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

/// Parse the shared `luid_0xHHHHHHHH_0xHHHHHHHH_phys_N` tail of a
/// `PDH` GPU counter instance name into
/// `((luid_high, luid_low), segment_idx)`.
///
/// Common to both counter sets this module reads: `GPU Process
/// Memory` instances carry a `pid_NNNN_` prefix before this tail,
/// `GPU Adapter Memory` instances are the bare tail. The `LUID`
/// `HighPart` is a Windows `LONG` (`i32`); `PDH` writes its bit
/// pattern as unsigned hex, so we parse as `u32` then bit-reinterpret.
#[must_use]
fn parse_luid_tail(rest: &str) -> Option<((i32, u32), u32)> {
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

    Some(((high, low), segment_idx))
}

/// Parse a `PDH` `\GPU Process Memory` instance name into
/// `(pid, (luid_high, luid_low), segment_idx)`.
///
/// Format: `pid_NNNN_luid_0xHHHHHHHH_0xHHHHHHHH_phys_N` (the shared
/// tail is delegated to [`parse_luid_tail`]). Returns `None` for
/// malformed or non-`GPU Process Memory` instance names (e.g., the
/// `_Total` instance, or instances from an unrelated counter set if
/// the API ever reuses them).
#[must_use]
fn parse_instance_name(name: &str) -> Option<(u32, (i32, u32), u32)> {
    let rest = name.strip_prefix("pid_")?;
    let (pid_str, rest) = rest.split_once('_')?;
    let pid: u32 = pid_str.parse().ok()?;
    let (luid, segment_idx) = parse_luid_tail(rest)?;
    Some((pid, luid, segment_idx))
}

/// Parse a `PDH` `\GPU Adapter Memory` instance name into
/// `((luid_high, luid_low), segment_idx)`.
///
/// Format: `luid_0xHHHHHHHH_0xHHHHHHHH_phys_N` — the bare `LUID`
/// tail, with no `pid_` prefix (adapter instances are
/// per-adapter-segment, not per-process). Verified live on the
/// reference `RTX 5060 Ti` (2026-07-22,
/// `typeperf -qx "GPU Adapter Memory"`). Returns `None` for
/// `pid_`-prefixed process instances, the `_Total` instance, and
/// anything else not matching the tail format.
#[must_use]
fn parse_adapter_instance_name(name: &str) -> Option<((i32, u32), u32)> {
    parse_luid_tail(name)
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

/// Add one counter path to an open `PDH` query, returning the counter
/// handle on success.
///
/// Failures are best-effort by design: callers skip or degrade the
/// affected row rather than aborting the whole query (matching the
/// per-instance error policy documented on [`collect_segmented_rows`]).
/// The failed status code is debug-traced with the offending path.
#[allow(unsafe_code)]
fn add_counter(query: PDH_HQUERY, counter_path: &str) -> Option<PDH_HCOUNTER> {
    // BORROW: explicit `encode_utf16` + chain(Some(0)) — PdhAddCounterW
    // requires a NUL-terminated UTF-16 string; the encoded path is
    // owned for the duration of the call.
    let counter_path_wide: Vec<u16> = counter_path
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    let mut h_counter = PDH_HCOUNTER::default();
    // SAFETY: `query` is a valid open query handle (caller invariant —
    // every call site owns a live QueryGuard). counter_path_wide is
    // NUL-terminated and lives until the call returns. h_counter is a
    // valid stack-allocated out-parameter (zero-initialised).
    let status = unsafe {
        PdhAddCounterW(
            query,
            PCWSTR::from_raw(counter_path_wide.as_ptr()),
            0,
            &raw mut h_counter,
        )
    };

    if status != PDH_SUCCESS {
        #[cfg(feature = "debug-output")]
        eprintln!("[PDH debug] PdhAddCounterW failed for path {counter_path:?}: 0x{status:08X}");
        return None;
    }
    Some(h_counter)
}

/// Enumerate the instance names of one `PDH` counter object (the
/// two-call size-query / data-fetch protocol).
///
/// Returns `Ok(None)` when the counter set is not registered on the
/// system (`PDH_CSTATUS_NO_OBJECT`) — a capability absence the caller
/// maps to its own semantics: a hard error with a pre-`WDDM 2.0` hint
/// for `GPU Process Memory`, a graceful "spill not measurable" for
/// `GPU Adapter Memory`. Returns `Ok(Some(vec))` (possibly empty)
/// when the set exists. `object` and `object_label` must name the
/// same counter set — the former as the `w!` UTF-16 literal `PDH`
/// consumes, the latter for error messages.
///
/// # Errors
///
/// Returns [`HypomnesisError::Pdh`] when either [`PdhEnumObjectItemsW`]
/// call fails with a status other than `PDH_MORE_DATA` (size query) or
/// [`PDH_SUCCESS`] (data fetch).
#[allow(unsafe_code)]
fn enum_object_instances(object: PCWSTR, object_label: &str) -> Result<Option<Vec<String>>> {
    // ---- Size query -------------------------------------------------------
    let mut counter_size: u32 = 0;
    let mut instance_size: u32 = 0;
    // SAFETY: PdhEnumObjectItemsW called with None buffers is the documented
    // "tell me the required buffer sizes" mode. The two pcch* parameters are
    // valid mutable references; `object` is a static UTF-16 string from `w!`
    // (caller invariant). PDH writes only into the size out-parameters on
    // this call and returns PDH_MORE_DATA on success.
    let status = unsafe {
        PdhEnumObjectItemsW(
            PCWSTR::null(),
            PCWSTR::null(),
            object,
            None,
            &raw mut counter_size,
            None,
            &raw mut instance_size,
            PERF_DETAIL_WIZARD,
            0,
        )
    };

    if status == PDH_CSTATUS_NO_OBJECT {
        return Ok(None);
    }

    // No instances at all → empty list (the counter set itself exists).
    if status == PDH_SUCCESS && instance_size <= 1 {
        return Ok(Some(Vec::new()));
    }

    if status != PDH_MORE_DATA {
        return Err(HypomnesisError::Pdh(format!(
            "PdhEnumObjectItemsW (size query, {object_label}) failed: 0x{status:08X}"
        )));
    }

    // ---- Data fetch -------------------------------------------------------
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
            object,
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
            "PdhEnumObjectItemsW (data fetch, {object_label}) failed: 0x{status:08X}"
        )));
    }

    Ok(Some(parse_multi_string(&instance_buffer)))
}

/// Read a sampled counter's formatted value as a non-negative byte
/// count.
///
/// Returns `None` when [`PdhGetFormattedCounterValue`] fails or when
/// the counter reports a negative value (which would indicate a
/// counter-implementation bug / sentinel — rejected rather than
/// bit-wrapped). Callers skip or zero the affected row.
#[allow(unsafe_code)]
#[must_use]
fn read_counter_bytes(h_counter: PDH_HCOUNTER) -> Option<u64> {
    let mut value = PDH_FMT_COUNTERVALUE::default();
    // SAFETY: h_counter was returned by a successful PdhAddCounterW on
    // a query that has since been sampled via PdhCollectQueryData
    // (caller invariant). `value` is a valid stack-allocated
    // out-parameter. PDH_FMT_LARGE selects the `largeValue` (i64)
    // union arm, set on success.
    let status =
        unsafe { PdhGetFormattedCounterValue(h_counter, PDH_FMT_LARGE, None, &raw mut value) };
    if status != PDH_SUCCESS {
        return None;
    }

    // SAFETY: PDH_FMT_LARGE was passed to PdhGetFormattedCounterValue
    // and the call succeeded; per Microsoft docs that sets the
    // `largeValue` arm of the union. Reading any other arm would be
    // unsound, but we read exactly the arm we requested.
    let raw_value: i64 = unsafe { value.Anonymous.largeValue };
    if raw_value < 0 {
        return None;
    }
    // CAST: i64 → u64, non-negative just checked; byte counts are
    // documented non-negative.
    #[allow(clippy::as_conversions, clippy::cast_sign_loss)]
    Some(raw_value as u64)
}

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

    // ---- 2. Enumerate instances -------------------------------------------
    // On absence of the counter set, the query closes via Drop.
    let Some(instances) = enum_object_instances(w!("GPU Process Memory"), "GPU Process Memory")?
    else {
        return Err(HypomnesisError::Pdh(
            "GPU Process Memory counter set not registered (pre-WDDM 2.0?)".to_owned(),
        ));
    };

    #[cfg(feature = "debug-output")]
    eprintln!("[PDH debug] enumerated {} instance(s)", instances.len());

    // ---- 3. Add counters per matching instance ----------------------------
    // Stores per-instance handles so we can correlate values back to
    // their source row after PdhCollectQueryData. Two counters per
    // instance: `Dedicated Usage` (mandatory — skip the row without it)
    // and its `Shared Usage` sibling (best-effort — the row degrades to
    // shared_used_bytes = 0 without it). Both attach to the same query,
    // so the single collect in step 5 samples both at once.
    let mut counter_handles: Vec<InstanceCounters> = Vec::new();

    for instance in &instances {
        let Some((pid, luid, segment_idx)) = parse_instance_name(instance) else {
            continue;
        };
        if luid != target_luid {
            continue;
        }

        let dedicated_path = format!("\\GPU Process Memory({instance})\\Dedicated Usage");
        let Some(dedicated) = add_counter(query.handle, &dedicated_path) else {
            #[cfg(feature = "debug-output")]
            eprintln!("[PDH debug] instance {instance:?} skipped (no Dedicated Usage counter)");
            continue;
        };

        let shared_path = format!("\\GPU Process Memory({instance})\\Shared Usage");
        let shared = add_counter(query.handle, &shared_path);

        counter_handles.push(InstanceCounters {
            pid,
            segment_idx,
            dedicated,
            shared,
        });
    }

    if counter_handles.is_empty() {
        return Ok(Vec::new());
    }

    // ---- 4. Collect one sample for the whole query ------------------------
    // SAFETY: query.handle is valid; PdhCollectQueryData samples every
    // counter added to the query in a single call. `Dedicated Usage` and
    // `Shared Usage` are instantaneous gauges (not rates), so one sample
    // suffices for both.
    let status = unsafe { PdhCollectQueryData(query.handle) };
    if status != PDH_SUCCESS {
        return Err(HypomnesisError::Pdh(format!(
            "PdhCollectQueryData failed: 0x{status:08X}"
        )));
    }

    // ---- 5. Read each counter's formatted value ---------------------------
    let mut rows: Vec<SegmentRow> = Vec::with_capacity(counter_handles.len());
    for ic in counter_handles {
        // Dedicated read failure (or negative sentinel) drops the row —
        // same policy as before v0.2.5.
        let Some(used_bytes) = read_counter_bytes(ic.dedicated) else {
            #[cfg(feature = "debug-output")]
            eprintln!(
                "[PDH debug] Dedicated Usage read failed for pid={} seg={} (skipped)",
                ic.pid, ic.segment_idx
            );
            continue;
        };

        // Shared read failure degrades to 0 — the dedicated figure is
        // still valuable on its own.
        let shared_used_bytes = ic.shared.and_then(read_counter_bytes).unwrap_or(0);

        rows.push(SegmentRow {
            pid: ic.pid,
            segment_idx: ic.segment_idx,
            used_bytes,
            shared_used_bytes,
        });
    }

    // Query closes here via QueryGuard::drop.
    Ok(rows)
}

// -----------------------------------------------------------------------
// Public entry point
// -----------------------------------------------------------------------

/// One aggregated per-process memory row from the `PDH` walk.
///
/// `dedicated_committed_bytes` carries the v0.2.2 `Dedicated Usage`
/// semantics unchanged (`VidMm` dedicated **commit** — the name spells
/// it out at the hand-off point, where `GpuProcessEntry` flattens it
/// back into `used_bytes`); `shared_used_bytes` is the resident
/// shared-system-memory figure from `Shared Usage` — the `WDDM` spill
/// signal, named identically to its [`SegmentRow`] source and its
/// `GpuProcessEntry` destination. Named fields rather than a tuple
/// because the two byte counts are trivially easy to transpose
/// silently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct ProcessMemoryRow {
    /// OS process ID.
    pub(super) pid: u32,
    /// Dedicated commit bytes (`Dedicated Usage`), aggregated across
    /// memory segments. See the module docs for the commit-vs-resident
    /// distinction.
    pub(super) dedicated_committed_bytes: u64,
    /// Resident shared bytes (`Shared Usage`), aggregated across
    /// memory segments — the `WDDM` spill signal.
    pub(super) shared_used_bytes: u64,
}

/// Per-process dedicated `VRAM` commit plus resident shared bytes for
/// every process holding memory on the adapter at `device_index`,
/// aggregated across memory segments.
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
pub(super) fn query_per_process_vram(device_index: u32) -> Result<Vec<ProcessMemoryRow>> {
    let target_luid = super::dxgi::adapter_luid(device_index).ok_or_else(|| {
        HypomnesisError::Pdh(format!(
            "no NVIDIA adapter at device_index {device_index} via DXGI walk"
        ))
    })?;

    let segments = collect_segmented_rows(target_luid)?;

    let mut by_pid: HashMap<u32, (u64, u64)> = HashMap::with_capacity(segments.len());
    for row in segments {
        let acc = by_pid.entry(row.pid).or_insert((0, 0));
        acc.0 = acc.0.saturating_add(row.used_bytes);
        acc.1 = acc.1.saturating_add(row.shared_used_bytes);
    }

    Ok(by_pid
        .into_iter()
        .map(
            |(pid, (dedicated_committed_bytes, shared_used_bytes))| ProcessMemoryRow {
                pid,
                dedicated_committed_bytes,
                shared_used_bytes,
            },
        )
        .collect())
}

// -----------------------------------------------------------------------
// Adapter-wide memory query (spill-detection support, v0.2.5)
// -----------------------------------------------------------------------

/// One sampled adapter-wide memory reading, summed across the `phys_N`
/// segments of the target adapter.
///
/// Crate-internal (the module is `pub(crate)`), constructed only by
/// [`AdapterMemQuery::sample`] — deliberately not `#[non_exhaustive]`;
/// field additions are same-crate refactors, not API evolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
// Field names deliberately keep the `_bytes` unit suffix used across
// the crate's public types (`used_bytes`, `total_bytes`, ...).
#[allow(clippy::struct_field_names)]
pub struct AdapterMemSample {
    /// Resident dedicated `VRAM` bytes, adapter-wide
    /// (`\GPU Adapter Memory(<instance>)\Dedicated Usage`).
    pub dedicated_bytes: u64,
    /// Dedicated `VRAM` capacity in bytes, from `DXGI`'s static
    /// `DedicatedVideoMemory`. No `Dedicated Limit` counter exists in
    /// the `GPU Adapter Memory` set (verified live on the reference
    /// `RTX 5060 Ti`, 2026-07-22 — the set carries only
    /// `Dedicated Usage`, `Shared Usage`, `Total Committed`), so the
    /// capacity is captured once at [`AdapterMemQuery::open`] time.
    /// `0` when the `DXGI` walk could not resolve the adapter —
    /// consumers must treat `0` as "limit unknown", never as a real
    /// capacity.
    pub limit_bytes: u64,
    /// Resident shared-system-memory bytes, adapter-wide
    /// (`\GPU Adapter Memory(<instance>)\Shared Usage`).
    pub shared_bytes: u64,
}

/// Counter-handle pair for one `GPU Adapter Memory` segment instance.
struct AdapterSegmentCounters {
    /// `Dedicated Usage` counter handle.
    dedicated: PDH_HCOUNTER,
    /// `Shared Usage` counter handle.
    shared: PDH_HCOUNTER,
}

/// Long-lived adapter-wide memory query — the Windows data source
/// behind `SpillTracker` (see `crate::spill`).
///
/// Unlike [`collect_segmented_rows`], which opens and closes a query
/// per call because `GPU Process Memory` instances churn with process
/// lifetimes, this type holds one open `PDH` query with the target
/// adapter's counters added once: `GPU Adapter Memory` instances are
/// stable for the adapter's lifetime, and spill polling at ~100 ms
/// would waste real work re-running the open / enumerate / add / close
/// cycle on every sample.
///
/// Holds raw `PDH` handles, so the type is `!Send` / `!Sync` —
/// construct and poll it on one thread (surfaced to consumers in the
/// `SpillTracker` rustdoc).
pub struct AdapterMemQuery {
    /// RAII guard owning the query handle; closes the query (and every
    /// counter added to it) on drop.
    guard: QueryGuard,
    /// Per-segment counter-handle pairs for the target adapter.
    counters: Vec<AdapterSegmentCounters>,
    /// Static dedicated capacity from `DXGI` — see
    /// [`AdapterMemSample::limit_bytes`]. `0` = unknown.
    limit_bytes: u64,
}

impl AdapterMemQuery {
    /// Open the adapter-wide query for `device_index`: resolve the
    /// adapter's `LUID` and dedicated capacity via the `DXGI` walk,
    /// enumerate `GPU Adapter Memory` instances, and add the
    /// `Dedicated Usage` + `Shared Usage` counter pair for each
    /// segment matching the adapter's `LUID`.
    ///
    /// Returns `Ok(None)` — "spill not measurable", deliberately not
    /// an error — when the `GPU Adapter Memory` counter set is not
    /// registered (pre-`WDDM 2.0`), when no enumerated instance
    /// matches the target adapter's `LUID` (unexpected instance-name
    /// format, adapter invisible to the provider), or when every
    /// matching instance rejects one of the two counters. Degrading
    /// keeps the caller's contract symmetric with Linux / macOS
    /// ("this platform cannot measure spill").
    ///
    /// # Errors
    ///
    /// Returns [`HypomnesisError::Pdh`] if the `DXGI` walk cannot
    /// locate `device_index`, if [`PdhOpenQueryW`] fails, or if
    /// instance enumeration fails fatally.
    #[allow(unsafe_code)]
    pub fn open(device_index: u32) -> Result<Option<Self>> {
        let target_luid = super::dxgi::adapter_luid(device_index).ok_or_else(|| {
            HypomnesisError::Pdh(format!(
                "no NVIDIA adapter at device_index {device_index} via DXGI walk"
            ))
        })?;
        let limit_bytes = super::dxgi::adapter_dedicated_video_memory(device_index).unwrap_or(0);

        let Some(instances) =
            enum_object_instances(w!("GPU Adapter Memory"), "GPU Adapter Memory")?
        else {
            return Ok(None);
        };

        let mut raw_handle = PDH_HQUERY::default();
        // SAFETY: same documented "open a new realtime query against the
        // local performance data source" form as collect_segmented_rows.
        // `phquery` is a valid out-parameter pointer to a stack-allocated
        // PDH_HQUERY (zero-initialised). Status is checked below; on
        // success the handle moves into the RAII guard immediately.
        let status = unsafe { PdhOpenQueryW(PCWSTR::null(), 0, &raw mut raw_handle) };
        if status != PDH_SUCCESS {
            return Err(HypomnesisError::Pdh(format!(
                "PdhOpenQueryW failed: 0x{status:08X}"
            )));
        }
        let guard = QueryGuard { handle: raw_handle };

        let mut counters: Vec<AdapterSegmentCounters> = Vec::new();
        for instance in &instances {
            let Some((luid, _segment_idx)) = parse_adapter_instance_name(instance) else {
                continue;
            };
            if luid != target_luid {
                continue;
            }

            let dedicated_path = format!("\\GPU Adapter Memory({instance})\\Dedicated Usage");
            let shared_path = format!("\\GPU Adapter Memory({instance})\\Shared Usage");
            // Both counters are mandatory here — the spill condition
            // needs dedicated saturation AND shared growth, so a
            // segment with only half the pair is skipped outright.
            let (Some(dedicated), Some(shared)) = (
                add_counter(guard.handle, &dedicated_path),
                add_counter(guard.handle, &shared_path),
            ) else {
                continue;
            };
            counters.push(AdapterSegmentCounters { dedicated, shared });
        }

        if counters.is_empty() {
            return Ok(None);
        }
        Ok(Some(Self {
            guard,
            counters,
            limit_bytes,
        }))
    }

    /// Collect one sample and read every counter, summing segments
    /// with saturating adds.
    ///
    /// Takes `&mut self` because [`PdhCollectQueryData`] advances the
    /// query's kernel-side sample state.
    ///
    /// # Errors
    ///
    /// Returns [`HypomnesisError::Pdh`] when [`PdhCollectQueryData`]
    /// fails (e.g. after a driver reset / `TDR` invalidates the
    /// query), **or when any per-counter read fails** — the whole
    /// sample fails so the consumer's `observe()` takes its
    /// skip-this-observation path. Degrading a failed read to `0`
    /// instead would fabricate an observation that can falsely seal an
    /// open spill episode (a zero dedicated reading looks like a
    /// falling edge) or, on the first observation, poison the shared
    /// baseline at `0` and over-detect for the rest of the run.
    #[allow(unsafe_code)]
    pub fn sample(&mut self) -> Result<AdapterMemSample> {
        // SAFETY: guard.handle is valid (RAII invariant);
        // PdhCollectQueryData samples every counter added to this query
        // in a single call. Both gauges are instantaneous (not rates),
        // so one sample suffices.
        let status = unsafe { PdhCollectQueryData(self.guard.handle) };
        if status != PDH_SUCCESS {
            return Err(HypomnesisError::Pdh(format!(
                "PdhCollectQueryData (adapter query) failed: 0x{status:08X}"
            )));
        }

        let mut dedicated_bytes: u64 = 0;
        let mut shared_bytes: u64 = 0;
        for c in &self.counters {
            let (Some(dedicated), Some(shared)) = (
                read_counter_bytes(c.dedicated),
                read_counter_bytes(c.shared),
            ) else {
                return Err(HypomnesisError::Pdh(
                    "PdhGetFormattedCounterValue failed for an adapter counter (sample skipped)"
                        .to_owned(),
                ));
            };
            dedicated_bytes = dedicated_bytes.saturating_add(dedicated);
            shared_bytes = shared_bytes.saturating_add(shared);
        }

        Ok(AdapterMemSample {
            dedicated_bytes,
            limit_bytes: self.limit_bytes,
            shared_bytes,
        })
    }
}

/// Cheap capability probe backing `is_spill_measurable` (see
/// `crate::spill`): does this system register the `GPU Adapter
/// Memory` counter set **with at least one instance**? Returns
/// `false` on enumeration failure of any kind — capability probes
/// never error. The non-empty requirement keeps the probe honest on
/// systems where the set is registered but no adapter surfaces
/// through it (a registered-but-empty set can never yield a
/// measurable [`AdapterMemQuery`]).
#[must_use]
pub fn adapter_counter_set_available() -> bool {
    matches!(
        enum_object_instances(w!("GPU Adapter Memory"), "GPU Adapter Memory"),
        Ok(Some(instances)) if !instances.is_empty()
    )
}

// -----------------------------------------------------------------------
// Win32 process-name lookup (companion to PDH per-process VRAM)
// -----------------------------------------------------------------------

/// `RAII` guard for a `Win32` `HANDLE`.
///
/// Closes the handle via [`CloseHandle`] on drop. Mirrors the
/// `QueryGuard` pattern above for the `PDH` query handle: every
/// successful handle-returning call ([`OpenProcess`] or
/// [`CreateToolhelp32Snapshot`]) is paired with exactly one
/// [`CloseHandle`] regardless of which return path the caller takes —
/// both APIs hand back an opaque kernel object closed the same way, so
/// one guard type covers both rather than forking a near-identical copy.
struct HandleGuard {
    /// `Win32` handle returned by a successful [`OpenProcess`] or
    /// [`CreateToolhelp32Snapshot`] call. Treated as an opaque
    /// kernel-side capability; never read or modified from userspace
    /// except through `Win32` APIs.
    handle: HANDLE,
}

impl Drop for HandleGuard {
    fn drop(&mut self) {
        // SAFETY: `self.handle` was obtained from a successful
        // OpenProcess or CreateToolhelp32Snapshot call (constructor
        // invariant). CloseHandle is documented as safe to call on any
        // valid handle regardless of its origin API. The return status
        // is intentionally discarded — cleanup is best-effort.
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
/// `None` — but that is *not* the final answer for most of them: see
/// [`resolve_names_via_snapshot`], which resolves the majority of these
/// `None`s (including `SYSTEM`/other-session processes like `dwm.exe`,
/// `csrss.exe`) without elevation, via a different Win32 mechanism that
/// doesn't open a per-process handle at all. `OpenProcess` denial here
/// is a property of *this specific lookup method*, not a hard privilege
/// wall — confirmed live: `PROCESS_QUERY_LIMITED_INFORMATION` and the
/// full `PROCESS_QUERY_INFORMATION` right both fail identically
/// (`ERROR_ACCESS_DENIED`) against `dwm.exe` from a non-elevated
/// caller, yet `Get-Process`/Task Manager name it instantly — because
/// they use the snapshot mechanism, not `OpenProcess`.
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

/// Resolve process names for PIDs that [`name_from_pid_windows`] could
/// not name via `OpenProcess`, using one `CreateToolhelp32Snapshot` scan
/// shared across every PID in `pids`.
///
/// The snapshot exposes every running process's short executable name
/// (`szExeFile`) without opening a per-process handle, so it succeeds
/// for foreign-user / `SYSTEM` processes (`dwm.exe`, `csrss.exe`) that
/// `OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` denies even
/// non-elevated — see [`name_from_pid_windows`]'s doc comment for the
/// live-verified evidence that this is a lookup-method gap, not a
/// privilege wall.
///
/// Returns `Some(pairs)` when the snapshot was taken successfully —
/// `pairs` contains one `(pid, name)` entry per requested PID that was
/// actually found in it. A requested PID *absent* from `pairs` has
/// exited since the caller's earlier sample (the caller renders that as
/// `"[exited]"`). Returns `None` only when
/// [`CreateToolhelp32Snapshot`] itself fails (very rare — resource
/// exhaustion) — the caller cannot distinguish "exited" from "unknown"
/// in that case and falls back to `"[protected]"` for every requested
/// PID. This `Option` split is why the return type isn't a bare `Vec`:
/// an empty-but-successful snapshot (every requested PID had already
/// exited) must be distinguishable from a snapshot that couldn't be
/// taken at all.
///
/// One snapshot is taken regardless of `pids.len()` — the cost is a
/// single system-wide enumeration, not one per PID. This matters
/// because the caller ([`crate::gpu::gpu_processes`]) invokes this once
/// per sampling pass, including every `hmn watch` interval tick; a
/// per-PID snapshot would repeat a full process-table walk once per
/// unresolved row per tick.
#[allow(unsafe_code)]
#[must_use]
pub(super) fn resolve_names_via_snapshot(pids: &[u32]) -> Option<Vec<(u32, String)>> {
    if pids.is_empty() {
        return Some(Vec::new());
    }

    // SAFETY: CreateToolhelp32Snapshot is a documented Win32 function.
    // TH32CS_SNAPPROCESS requests a process-only snapshot (no thread /
    // heap / module entries). Failure (extremely rare — resource
    // exhaustion) surfaces as Err; the let-else below returns `None`
    // without leaking any partially-acquired state.
    let snapshot_result = unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) };
    let Ok(raw_handle) = snapshot_result else {
        return None;
    };
    let guard = HandleGuard { handle: raw_handle };

    // CAST: usize → u32, size_of::<PROCESSENTRY32W>() is a small fixed
    // struct size, fits trivially.
    #[allow(clippy::as_conversions, clippy::cast_possible_truncation)]
    let entry_size = size_of::<PROCESSENTRY32W>() as u32;
    let mut entry = PROCESSENTRY32W {
        dwSize: entry_size,
        ..Default::default()
    };

    let mut found = Vec::new();
    // SAFETY: guard.handle is valid (RAII invariant). `entry` is a
    // stack-allocated PROCESSENTRY32W with dwSize pre-set to its own
    // size, as Process32FirstW/Process32NextW require to validate the
    // caller's struct layout matches theirs.
    let mut has_entry = unsafe { Process32FirstW(guard.handle, &raw mut entry) }.is_ok();
    // EXPLICIT: Process32FirstW/Process32NextW is a C-style
    // out-parameter cursor API with no iterator equivalent in the
    // `windows` crate bindings; an imperative loop is the only way to
    // drive it.
    while has_entry {
        if pids.contains(&entry.th32ProcessID) {
            let name = szexefile_to_string(&entry.szExeFile);
            if !name.is_empty() {
                found.push((entry.th32ProcessID, name));
            }
        }
        // SAFETY: same invariants as the Process32FirstW call above;
        // `entry` is reused as the out-parameter for the next row.
        has_entry = unsafe { Process32NextW(guard.handle, &raw mut entry) }.is_ok();
    }

    Some(found)
}

/// Decode a `Win32` `PROCESSENTRY32W.szExeFile` fixed buffer (`[u16;
/// 260]`, `NUL`-terminated) into an owned `String`.
///
/// Unlike [`basename_from_path`], no path-separator stripping is
/// needed: `Toolhelp32` always returns the bare executable file name in
/// this field, never a full path.
#[must_use]
fn szexefile_to_string(buf: &[u16; 260]) -> String {
    let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    // INDEX: `len` is the position of the first NUL found via
    // `.position()` above (or the full buffer length when
    // unterminated), so it is always `<= buf.len()` by construction.
    #[allow(clippy::indexing_slicing)]
    let slice = &buf[..len];
    String::from_utf16_lossy(slice)
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
    use super::{
        basename_from_path, kernel_name_for_pid, parse_adapter_instance_name, parse_instance_name,
        parse_multi_string, szexefile_to_string,
    };

    /// Build a `szExeFile`-shaped `[u16; 260]` buffer from a `&str`,
    /// `NUL`-terminated and zero-padded, matching what
    /// `Process32FirstW`/`Process32NextW` write into the field.
    fn szexefile_buf(name: &str) -> [u16; 260] {
        let mut buf = [0_u16; 260];
        for (slot, ch) in buf.iter_mut().zip(name.encode_utf16()) {
            *slot = ch;
        }
        buf
    }

    #[test]
    fn szexefile_to_string_decodes_nul_terminated_name() {
        assert_eq!(szexefile_to_string(&szexefile_buf("dwm.exe")), "dwm.exe");
    }

    #[test]
    fn szexefile_to_string_empty_buffer_yields_empty_string() {
        assert_eq!(szexefile_to_string(&[0_u16; 260]), "");
    }

    #[test]
    fn szexefile_to_string_unterminated_buffer_uses_full_length() {
        // Pathological but defensively covered: a buffer with no NUL at
        // all (shouldn't happen in practice — Win32 always terminates
        // szExeFile — but `.position()` returning `None` must not panic).
        let buf = [u16::from(b'a'); 260];
        let decoded = szexefile_to_string(&buf);
        assert_eq!(decoded.chars().count(), 260);
    }

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

    // -------------------------------------------------------------------
    // parse_adapter_instance_name tests (v0.2.5 spill support)
    // -------------------------------------------------------------------

    #[test]
    fn parse_adapter_instance_name_basic() {
        // Live format from the reference RTX 5060 Ti (2026-07-22).
        let (luid, seg) = parse_adapter_instance_name("luid_0x00000000_0x0000F391_phys_0").unwrap();
        assert_eq!(luid, (0, 0x0000_F391));
        assert_eq!(seg, 0);
    }

    #[test]
    fn parse_adapter_instance_name_high_bit_luid() {
        // PDH writes LUID HighPart as unsigned hex; bit pattern must
        // round-trip through the i32 reinterpretation.
        let (luid, _seg) =
            parse_adapter_instance_name("luid_0xFFFFFFFF_0x00000001_phys_2").unwrap();
        assert_eq!(luid.0, -1_i32);
        assert_eq!(luid.1, 1);
    }

    #[test]
    fn parse_adapter_instance_name_rejects_total() {
        assert!(parse_adapter_instance_name("_Total").is_none());
        assert!(parse_adapter_instance_name("").is_none());
    }

    #[test]
    fn parse_adapter_instance_name_rejects_pid_prefixed() {
        // Process instances must NOT parse as adapter instances — the
        // pid_ prefix breaks the required luid_0x lead.
        assert!(
            parse_adapter_instance_name("pid_1234_luid_0x00000000_0x0000F391_phys_0").is_none()
        );
    }

    #[test]
    fn parse_adapter_instance_name_rejects_missing_phys() {
        assert!(parse_adapter_instance_name("luid_0x00000000_0x0000F391").is_none());
        assert!(parse_adapter_instance_name("luid_0x00000000_0x0000F391_phys_").is_none());
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
