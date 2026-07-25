// SPDX-License-Identifier: MIT OR Apache-2.0

//! Snapshot data types — what a `hypomnesis` measurement returns.

use crate::Result;

/// Device-wide GPU information for a specific GPU index.
///
/// Reports what the device currently holds across **all** processes —
/// useful for sizing decisions ("can this model fit?"). For per-process
/// accounting, see `ProcessGpuInfo`.
///
/// `#[non_exhaustive]`: fields may be added in future releases (e.g.,
/// `temperature_celsius`, `pcie_link_gen`).
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct GpuDeviceInfo {
    /// Zero-based GPU index.
    ///
    /// For [`Snapshot::now`] and the NVIDIA portion of [`Snapshot::all`],
    /// this is the `NVML`-canonical index (which agrees with the `DXGI`
    /// NVIDIA-filtered index on Windows). For non-NVIDIA Windows
    /// adapters surfaced by [`Snapshot::all`] (e.g. AMD / Intel iGPUs),
    /// it is a synthetic index assigned after the NVIDIA count and is
    /// **not** addressable via [`Snapshot::now`].
    pub index: u32,
    /// Adapter name (e.g., `NVIDIA GeForce RTX 5060 Ti`).
    /// `None` when the source backend (e.g., `NVML` on a system where
    /// `nvmlDeviceGetName` failed) does not provide it.
    pub name: Option<String>,
    /// Total GPU memory in bytes.
    pub total_bytes: u64,
    /// Free GPU memory in bytes (device-wide).
    pub free_bytes: u64,
    /// Used GPU memory in bytes (device-wide; sum across all processes).
    pub used_bytes: u64,
    /// Device memory reserved for system use (driver or firmware), in
    /// bytes — the `nvmlMemory_v2_t.reserved` carve-out (page tables,
    /// context/channel structures, ECC parity).
    ///
    /// `Some` only on the `NVML` path with an R510+ driver (where
    /// `nvmlDeviceGetMemoryInfo_v2` succeeds). `None` on older drivers and
    /// on every non-`NVML` backend (`DXGI`, `nvidia-smi`, Metal).
    ///
    /// When `Some`, this is a **subset of** [`Self::total_bytes`], not an
    /// addition to it: NVML's `total` already satisfies
    /// `total = reserved + free + used`, so memory available for allocation
    /// is `total_bytes - reserved_bytes` (and [`Self::free_bytes`] already
    /// nets it out). `total_bytes` is the full framebuffer NVML reports —
    /// identical to what `nvidia-smi -q -d MEMORY` prints as `Total`, with
    /// `reserved_bytes` matching its `Reserved` line. (Do **not** confuse
    /// this with the smaller board/ECC overhead between a card's nominal
    /// capacity and NVML's `total` — that gap sits *below* `total_bytes`
    /// and NVML does not expose it.)
    pub reserved_bytes: Option<u64>,
}

/// Per-process GPU memory information.
///
/// `#[non_exhaustive]`: fields may be added in future releases.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct ProcessGpuInfo {
    /// GPU memory used by this process in bytes.
    ///
    /// When `is_per_process` is `false`, this is the device-wide total
    /// (the `nvidia-smi` fallback cannot break the figure down per process).
    pub used_bytes: u64,
    /// `true` when the value is genuinely per-process (`DXGI` or `NVML`);
    /// `false` when it falls back to a device-wide reading from `nvidia-smi`.
    pub is_per_process: bool,
    /// Which backend produced this measurement.
    pub source: GpuQuerySource,
}

/// The backend that produced a GPU memory measurement.
///
/// `#[non_exhaustive]`: more backends (e.g., AMD `ROCm` SMI, Apple Metal)
/// may be added.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuQuerySource {
    /// Windows `DXGI` per-process query (`IDXGIAdapter3::QueryVideoMemoryInfo`).
    Dxgi,
    /// `NVML` per-process query (`nvmlDeviceGetComputeRunningProcesses`).
    Nvml,
    /// Windows `PDH` (Performance Data Helper) per-process query of
    /// `\GPU Process Memory(<instance>)\Dedicated Usage`. The
    /// foreign-process per-process backend on consumer `WDDM`, where
    /// `NVML` returns `NVML_VALUE_NOT_AVAILABLE` and
    /// `IDXGIAdapter3::QueryVideoMemoryInfo` answers only for the
    /// calling process. Rows produced via this path memorialise that
    /// the bytes came directly from `VidMm` via `PDH` rather than
    /// being inferred via subprocess parsing.
    ///
    /// Unlike `Nvml` / `NvidiaSmi`, `Pdh` rows are **not** compute-only
    /// — `PDH` surfaces every process holding GPU memory under `WDDM`
    /// (compositor, browsers, games, compute alike). The semantics
    /// difference is documented on [`GpuProcessEntry`].
    Pdh,
    /// `nvidia-smi` subprocess. In [`ProcessGpuInfo`] this is the
    /// device-wide fallback (the subprocess cannot break the figure
    /// down per-process for the *calling* process). In
    /// [`GpuProcessEntry`] each row is per-process — one entry per
    /// `CUDA` process from `nvidia-smi --query-compute-apps`.
    NvidiaSmi,
    /// Apple Metal per-process query (`ledger(LEDGER_ENTRY_INFO_V2).graphics_footprint`).
    Metal,
}

/// One process holding GPU memory on a given device.
///
/// Distinct from [`ProcessGpuInfo`]: that type describes the *calling*
/// process's own usage; `GpuProcessEntry` is one row of an enumeration
/// over processes on the device, returned by [`crate::gpu_processes`].
///
/// # Semantics per-backend
///
/// What "processes on the device" *means* depends on which backend
/// produced the row:
///
/// - [`GpuQuerySource::Nvml`] and [`GpuQuerySource::NvidiaSmi`] are
///   **compute-only**. They only see processes with an active `CUDA`
///   context — browsers using GPU compositing, games, and
///   pure-graphics apps do not appear.
/// - [`GpuQuerySource::Pdh`] (Windows-only, consumer `WDDM`) reports
///   **every** process holding GPU memory: the desktop compositor,
///   browsers using GPU rendering, games, and compute processes all
///   appear with their `Dedicated Usage` byte count. This is more
///   information than the compute-only sources but a different
///   denominator — callers comparing across platforms or backends
///   should check `source` before assuming compute-only semantics.
///
/// `#[non_exhaustive]`: fields may be added in future releases.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct GpuProcessEntry {
    /// OS process ID.
    pub pid: u32,
    /// Process name. `None` when no name source is available — `PDH`
    /// rows whose `OpenProcess` was access-denied (cross-user, protected
    /// processes), or `NVML` rows whose `/proc/<pid>/comm` was
    /// unreadable. Two synthetic non-`None` values are produced:
    /// `Some("[kernel]")` for `PID 4` on the Windows `PDH` path (the
    /// kernel pseudo-process has no executable image, but is
    /// special-cased so it doesn't pollute the "unresolvable even
    /// elevated" set); `Some("?")` for protected processes on the
    /// Windows `nvidia-smi` fallback path, where `nvidia-smi` itself
    /// writes a literal `?` rather than failing the row.
    pub name: Option<String>,
    /// GPU memory used by this process in bytes. On the Windows
    /// [`GpuQuerySource::Pdh`] path this is `VidMm`'s dedicated
    /// **commit** (reservation) figure, not the resident set — it can
    /// legitimately exceed physical `VRAM`; see the sibling
    /// `shared_used_bytes` for the residency-based spill signal. On
    /// `NVML` / `nvidia-smi` / `Metal` rows it carries those
    /// backends' documented per-process semantics.
    pub used_bytes: u64,
    /// Resident shared-system-memory bytes for this process — the
    /// `WDDM` spill signal (`\GPU Process Memory(*)\Shared Usage`, the
    /// same quantity Task Manager's per-process *Shared GPU memory*
    /// column shows). This — not `used_bytes`, which on the `PDH` path
    /// is `VidMm`'s dedicated **commit** — is the number that grows
    /// when the kernel pages GPU allocations out of dedicated `VRAM`
    /// into shared system memory. Populated only on the Windows
    /// [`GpuQuerySource::Pdh`] path; `0` on `NVML`, `nvidia-smi`, and
    /// `Metal` rows (no shared-residency counter exists on those
    /// backends).
    pub shared_used_bytes: u64,
    /// Which backend produced this row. One of [`GpuQuerySource::Nvml`],
    /// [`GpuQuerySource::Pdh`], [`GpuQuerySource::NvidiaSmi`], or
    /// [`GpuQuerySource::Metal`].
    /// `DXGI`'s `QueryVideoMemoryInfo` only answers for the calling
    /// process and cannot enumerate other PIDs, so
    /// [`GpuQuerySource::Dxgi`] is never the source of a
    /// `GpuProcessEntry`.
    pub source: GpuQuerySource,
}

/// Combined snapshot of process `RAM` and GPU memory state at a point in time.
///
/// Constructed via [`Snapshot::now`] (one device) or [`Snapshot::all`]
/// (every visible GPU). `RAM` measurement is mandatory; both GPU fields
/// are best-effort and set to `None` when no backend is usable.
///
/// `#[non_exhaustive]`: fields may be added in future releases.
#[non_exhaustive]
#[derive(Debug, Clone)]
pub struct Snapshot {
    /// Process resident set size in bytes.
    pub ram_bytes: u64,
    /// Per-process GPU memory information for the requested device.
    /// `None` when no GPU source is usable.
    pub gpu: Option<ProcessGpuInfo>,
    /// Device-wide GPU information for the requested device.
    /// `None` when no GPU source is usable.
    pub gpu_device: Option<GpuDeviceInfo>,
}

impl Snapshot {
    /// Capture a fresh snapshot of process `RAM` and GPU memory for the given device index.
    ///
    /// `RAM` is always measured. GPU measurement failures are non-fatal —
    /// the corresponding fields are set to `None` rather than producing an error.
    ///
    /// # Performance
    ///
    /// Each call performs a full `NVML` init/shutdown cycle (and, on Windows,
    /// a fresh `IDXGIFactory1` walk). This adds a few milliseconds of
    /// overhead per call — fine for occasional sampling around training
    /// steps or model loads, less ideal for tight per-frame polling. A
    /// long-lived `NVML` context is planned for v0.2.
    ///
    /// # Per-process vs device-wide
    ///
    /// When `DXGI` (Windows) or `NVML` (Linux) succeeds, `gpu.used_bytes`
    /// is genuinely per-process and `gpu.is_per_process` is `true`. When
    /// the dispatcher falls back to `nvidia-smi` (no `NVML`/`DXGI`
    /// available, or `WDDM` `NVML_VALUE_NOT_AVAILABLE`), `gpu.used_bytes`
    /// reflects the **device-wide** total and `gpu.is_per_process` is
    /// `false`. Callers that need true per-process accounting should
    /// check `is_per_process` before interpreting the value.
    ///
    /// # Errors
    ///
    /// Returns [`crate::HypomnesisError::Ram`] if the platform `RAM`
    /// query fails — including the Linux path, where `/proc/self/status`
    /// read errors and `VmRSS` parse failures are wrapped into the
    /// `Ram` variant rather than surfaced as `Io`.
    pub fn now(device_index: u32) -> Result<Self> {
        let ram_bytes = crate::ram::process_rss()?;
        let gpu = crate::gpu::process_gpu_info(device_index).ok();
        let gpu_device = crate::gpu::device_info(device_index).ok();
        Ok(Self {
            ram_bytes,
            gpu,
            gpu_device,
        })
    }

    /// Capture a fresh snapshot of process `RAM` and GPU memory **for every
    /// visible GPU**.
    ///
    /// On Linux: enumerates NVIDIA dGPU(s) via `NVML`. AMD / Intel iGPUs
    /// do not surface — there is no AMD / Intel backend yet (an AMD
    /// `ROCm` SMI backend and an Apple Metal backend are possibilities
    /// for a later release; see `docs/roadmap-v0.2.0.md`).
    ///
    /// On Windows: enumerates NVIDIA dGPU(s) via `NVML` *plus* every
    /// other `DXGI` adapter that exposes
    /// `DedicatedVideoMemory > 0` or `SharedSystemMemory > 0` (e.g.
    /// AMD / Intel iGPUs). For non-NVIDIA adapters, `total_bytes` is
    /// `DedicatedVideoMemory` when non-zero (matches what dGPUs and
    /// UMA-allocated iGPUs expose), otherwise the `WDDM` shared-memory
    /// budget (`SharedSystemMemory`). The semantics of `total_bytes`
    /// therefore differ subtly between dGPUs and iGPUs. The Microsoft
    /// Basic Render Driver (`VendorId = 0x1414`) is always skipped — it
    /// has no real GPU memory to report.
    ///
    /// Each returned [`Snapshot`] carries the same `ram_bytes`: a single
    /// `RSS` measurement is taken once and reused, since the wall-time
    /// delta across the GPU walk is microseconds and per-snapshot
    /// re-measurement would add no useful precision.
    ///
    /// Returns an empty `Vec` when no GPUs are visible. Callers needing
    /// RAM-only state should use [`crate::process_rss`] or
    /// [`Self::now`] (which returns a single `Snapshot` with `gpu` and
    /// `gpu_device` set to `None`).
    ///
    /// # Performance
    ///
    /// Each NVIDIA device queried calls [`crate::process_gpu_info`] and
    /// [`crate::device_info`], each of which performs a fresh `NVML`
    /// init / shutdown cycle (and, on Windows, a fresh `IDXGIFactory1`
    /// walk). On Windows, `Snapshot::all` additionally walks
    /// `IDXGIFactory1` once more for the non-NVIDIA enumeration. For an
    /// N-GPU system the worst-case cost is therefore
    /// `N × (NVML init + shutdown + DXGI walk) + 1 × DXGI walk` on
    /// Windows, and `N × (NVML init + shutdown)` on Linux. A long-lived
    /// `NVML` context is planned for a later release.
    ///
    /// # Errors
    ///
    /// Returns [`crate::HypomnesisError::Ram`] if the platform `RAM`
    /// query fails — including the Linux path, where `/proc/self/status`
    /// read errors and `VmRSS` parse failures are wrapped into the
    /// `Ram` variant rather than surfaced as `Io`.
    pub fn all() -> Result<Vec<Self>> {
        let ram_bytes = crate::ram::process_rss()?;

        // device_count() returning Err here means "no enumeration backend
        // is enabled / every backend failed to report a count" — treat
        // that as zero NVIDIA GPUs and let the Windows DXGI extras path
        // (if compiled in) still surface iGPUs. RAM is already captured.
        let nvidia_count = crate::gpu::device_count().unwrap_or(0);
        // CAST: u32 → usize, NVML / DXGI device counts are bounded by
        // hardware (handfuls in practice); fits trivially in usize.
        #[allow(clippy::as_conversions)]
        let mut snapshots: Vec<Self> = Vec::with_capacity(nvidia_count as usize);

        for idx in 0..nvidia_count {
            snapshots.push(Self {
                ram_bytes,
                gpu: crate::gpu::process_gpu_info(idx).ok(),
                gpu_device: crate::gpu::device_info(idx).ok(),
            });
        }

        #[cfg(all(windows, feature = "dxgi"))]
        for (gpu_device, gpu) in crate::gpu::dxgi_non_nvidia_devices(nvidia_count) {
            snapshots.push(Self {
                ram_bytes,
                gpu: Some(gpu),
                gpu_device: Some(gpu_device),
            });
        }

        Ok(snapshots)
    }
}

/// Convenience formatting helpers, available with `features = ["report"]`.
///
/// Located on `Snapshot` (rather than `MemoryReport`) for parity with
/// `candle-mi`'s `MemorySnapshot::ram_mb` / `vram_mb` API surface, so
/// candle-mi v0.2 can adopt `hypomnesis` with a thin adapter wrapper
/// rather than relocating the methods.
#[cfg(feature = "report")]
impl Snapshot {
    /// `RAM` (`RSS`) usage as megabytes (`bytes / 1_048_576`).
    #[must_use]
    pub fn ram_mb(&self) -> f64 {
        // CAST: u64 → f64, value is memory in bytes — fits in f64 mantissa
        // for any realistic process size (< 2^53 bytes ≈ 8 PiB).
        #[allow(clippy::cast_precision_loss, clippy::as_conversions)]
        let mb = self.ram_bytes as f64 / 1_048_576.0;
        mb
    }

    /// Per-process `VRAM` usage as megabytes, if available.
    ///
    /// Returns `None` when `gpu` is `None` (no GPU source succeeded).
    /// Reflects the dispatcher's mixed semantics: per-process when
    /// `DXGI` / `NVML` produced the value, device-wide when the
    /// `nvidia-smi` fallback was used (check `gpu.is_per_process`).
    #[must_use]
    pub fn vram_mb(&self) -> Option<f64> {
        // CAST: u64 → f64, same justification as ram_mb.
        #[allow(clippy::cast_precision_loss, clippy::as_conversions)]
        let mb = self.gpu.as_ref().map(|p| p.used_bytes as f64 / 1_048_576.0);
        mb
    }
}

impl GpuDeviceInfo {
    /// Adapter name, or `"unknown GPU"` when [`Self::name`] is `None`.
    ///
    /// Convenience wrapper for `self.name.as_deref().unwrap_or("unknown GPU")`,
    /// added so downstream consumers don't diverge on the fallback phrase.
    /// The returned string is **not** localized — consumers needing a
    /// different phrase or non-English output should match on [`Self::name`]
    /// directly.
    #[must_use]
    pub fn name_or_unknown(&self) -> &str {
        self.name.as_deref().unwrap_or("unknown GPU")
    }
}

/// Free-`VRAM` formatting helpers for [`GpuDeviceInfo`], available with `features = ["report"]`.
///
/// Mirrors the [`Snapshot::ram_mb`] / [`Snapshot::vram_mb`] convention:
/// feature-gated formatting helpers live next to the type they describe.
/// The motivating use case is the LM-Studio-style headroom check —
/// *"if I load this model now, will it fit?"* — which is a one-liner via
/// the existing `free_bytes` field; this helper makes the printed
/// reporting equally short.
#[cfg(feature = "report")]
impl GpuDeviceInfo {
    /// Format a one-line free-`VRAM` summary as an owned `String` ending in a newline.
    ///
    /// Format: `  GPU <idx>: free <N> MB / <T> MB[ [<adapter name>]]\n`.
    /// The adapter-name suffix is omitted when [`Self::name`] is `None`.
    /// `MB` here means `MiB` (`bytes / 1_048_576`), matching
    /// [`Snapshot::ram_mb`] and [`Snapshot::vram_mb`].
    ///
    /// Suitable for log frameworks (`tracing::info!("{}", dev.format_free())`),
    /// file output, or test assertions. [`Self::print_free`] delegates here.
    #[must_use]
    pub fn format_free(&self) -> String {
        // CAST: u64 → f64, byte count for MiB conversion (fits in f64
        // mantissa for any realistic VRAM size; same justification as
        // Snapshot::ram_mb).
        #[allow(clippy::cast_precision_loss, clippy::as_conversions)]
        let free_mb = self.free_bytes as f64 / 1_048_576.0;
        #[allow(clippy::cast_precision_loss, clippy::as_conversions)]
        let total_mb = self.total_bytes as f64 / 1_048_576.0;
        // BORROW: explicit Option::as_deref + map_or — name is
        // Option<String>; we need an owned String for the suffix.
        let name_suffix = self
            .name
            .as_deref()
            .map_or(String::new(), |n| format!(" [{n}]"));
        format!(
            "  GPU {}: free {free_mb:.0} MB / {total_mb:.0} MB{name_suffix}\n",
            self.index
        )
    }

    /// Print a one-line free-`VRAM` summary to stdout.
    ///
    /// Delegates to [`Self::format_free`]; the printed string is
    /// byte-for-byte identical to the formatted return value.
    pub fn print_free(&self) {
        print!("{}", self.format_free());
    }

    /// Format a one-line total-`VRAM` summary as an owned `String` ending in a newline.
    ///
    /// Format: `  GPU <idx>: total <T> MB[ [<adapter name>]]\n`.
    /// Mirrors [`Self::format_free`]'s style exactly — two-space indent,
    /// `MB` displayed for `MiB` (`bytes / 1_048_576`), trailing newline,
    /// optional ` [<name>]` suffix omitted when [`Self::name`] is `None`.
    ///
    /// Suitable for log frameworks (`tracing::info!("{}", dev.format_total())`),
    /// file output, or test assertions.
    #[must_use]
    pub fn format_total(&self) -> String {
        // CAST: u64 → f64, byte count for MiB conversion (fits in f64
        // mantissa for any realistic VRAM size; same justification as
        // Snapshot::ram_mb).
        #[allow(clippy::cast_precision_loss, clippy::as_conversions)]
        let total_mb = self.total_bytes as f64 / 1_048_576.0;
        // BORROW: explicit Option::as_deref + map_or — name is
        // Option<String>; we need an owned String for the suffix.
        let name_suffix = self
            .name
            .as_deref()
            .map_or(String::new(), |n| format!(" [{n}]"));
        format!(
            "  GPU {}: total {total_mb:.0} MB{name_suffix}\n",
            self.index
        )
    }

    /// Format a one-line used-`VRAM` summary as an owned `String` ending in a newline.
    ///
    /// Format: `  GPU <idx>: used <U> MB[ [<adapter name>]]\n`.
    /// Mirrors [`Self::format_free`]'s style exactly — two-space indent,
    /// `MB` displayed for `MiB` (`bytes / 1_048_576`), trailing newline,
    /// optional ` [<name>]` suffix omitted when [`Self::name`] is `None`.
    ///
    /// Suitable for log frameworks (`tracing::info!("{}", dev.format_used())`),
    /// file output, or test assertions.
    #[must_use]
    pub fn format_used(&self) -> String {
        // CAST: u64 → f64, byte count for MiB conversion (fits in f64
        // mantissa for any realistic VRAM size; same justification as
        // Snapshot::ram_mb).
        #[allow(clippy::cast_precision_loss, clippy::as_conversions)]
        let used_mb = self.used_bytes as f64 / 1_048_576.0;
        // BORROW: explicit Option::as_deref + map_or — name is
        // Option<String>; we need an owned String for the suffix.
        let name_suffix = self
            .name
            .as_deref()
            .map_or(String::new(), |n| format!(" [{n}]"));
        format!("  GPU {}: used {used_mb:.0} MB{name_suffix}\n", self.index)
    }
}

/// Builder for synthetic [`GpuDeviceInfo`] values in downstream tests.
///
/// Available only with `features = ["test-helpers"]`. Not intended for
/// production code — the canonical way to obtain a `GpuDeviceInfo` is
/// via [`crate::device_info`], [`Snapshot::now`], or [`Snapshot::all`],
/// each of which goes through a real GPU backend.
///
/// `#[non_exhaustive]` on `GpuDeviceInfo` forbids struct-literal
/// construction from external crates, which would otherwise block
/// downstream unit tests that need synthetic fixtures (e.g., to test
/// render paths or arithmetic over the type's fields). The builder
/// closes that gap without weakening the future-proofing the
/// `#[non_exhaustive]` annotation provides — new fields on
/// `GpuDeviceInfo` will be exposed as new defaulted setters here.
///
/// Each setter consumes `self` and returns `Self`, enabling a chained
/// construction (`GpuDeviceInfo::builder().index(1).total_bytes(N).build()`).
/// Unset fields take the documented defaults: `index = 0`, `name = None`,
/// `total_bytes = 0`, `free_bytes = 0`, `used_bytes = 0`,
/// `reserved_bytes = None`.
///
/// # Semver
///
/// The `test-helpers` feature is **not** semver-stable in the same sense
/// as the production API. New setters are expected to land alongside any
/// new field added to `GpuDeviceInfo`, and downstream tests using the
/// builder are expected to absorb those additions. Production code must
/// never enable the feature.
///
/// # Example
///
/// ```
/// # #[cfg(feature = "test-helpers")]
/// # {
/// use hypomnesis::GpuDeviceInfo;
///
/// let dev = GpuDeviceInfo::builder()
///     .index(0)
///     .name(Some("Synthetic GPU".to_owned()))
///     .total_bytes(16 * 1_024 * 1_024 * 1_024)
///     .free_bytes(14 * 1_024 * 1_024 * 1_024)
///     .used_bytes(2 * 1_024 * 1_024 * 1_024)
///     .build();
/// assert_eq!(dev.total_bytes, 16 * 1_024 * 1_024 * 1_024);
/// # }
/// ```
#[cfg(feature = "test-helpers")]
#[derive(Debug, Clone, Default)]
pub struct GpuDeviceInfoBuilder {
    /// Pending [`GpuDeviceInfo::index`] value, defaults to `0`.
    index: u32,
    /// Pending [`GpuDeviceInfo::name`] value, defaults to `None`.
    name: Option<String>,
    /// Pending [`GpuDeviceInfo::total_bytes`] value, defaults to `0`.
    total_bytes: u64,
    /// Pending [`GpuDeviceInfo::free_bytes`] value, defaults to `0`.
    free_bytes: u64,
    /// Pending [`GpuDeviceInfo::used_bytes`] value, defaults to `0`.
    used_bytes: u64,
    /// Pending [`GpuDeviceInfo::reserved_bytes`] value, defaults to `None`.
    reserved_bytes: Option<u64>,
}

#[cfg(feature = "test-helpers")]
impl GpuDeviceInfo {
    /// Start a builder for constructing synthetic `GpuDeviceInfo` values
    /// in downstream tests.
    ///
    /// Available only with `features = ["test-helpers"]`. See
    /// [`GpuDeviceInfoBuilder`] for the full discussion.
    #[must_use]
    pub fn builder() -> GpuDeviceInfoBuilder {
        GpuDeviceInfoBuilder::default()
    }
}

#[cfg(feature = "test-helpers")]
impl GpuDeviceInfoBuilder {
    /// Set the zero-based GPU index.
    #[must_use]
    pub const fn index(mut self, index: u32) -> Self {
        self.index = index;
        self
    }

    /// Set the adapter name (`None` to leave unset).
    #[must_use]
    pub fn name(mut self, name: Option<String>) -> Self {
        self.name = name;
        self
    }

    /// Set the total GPU memory in bytes.
    #[must_use]
    pub const fn total_bytes(mut self, total: u64) -> Self {
        self.total_bytes = total;
        self
    }

    /// Set the free GPU memory in bytes (device-wide).
    #[must_use]
    pub const fn free_bytes(mut self, free: u64) -> Self {
        self.free_bytes = free;
        self
    }

    /// Set the used GPU memory in bytes (device-wide).
    #[must_use]
    pub const fn used_bytes(mut self, used: u64) -> Self {
        self.used_bytes = used;
        self
    }

    /// Set the device memory reserved for system use, in bytes
    /// (`None` to leave unset).
    #[must_use]
    pub const fn reserved_bytes(mut self, reserved: Option<u64>) -> Self {
        self.reserved_bytes = reserved;
        self
    }

    /// Consume the builder and produce the configured `GpuDeviceInfo`.
    ///
    /// Unset fields take the documented defaults.
    #[must_use]
    pub fn build(self) -> GpuDeviceInfo {
        GpuDeviceInfo {
            index: self.index,
            name: self.name,
            total_bytes: self.total_bytes,
            free_bytes: self.free_bytes,
            used_bytes: self.used_bytes,
            reserved_bytes: self.reserved_bytes,
        }
    }
}

/// Builder for synthetic [`GpuProcessEntry`] values in downstream tests.
///
/// Available only with `features = ["test-helpers"]`. `GpuProcessEntry`
/// is `#[non_exhaustive]`, so struct-literal construction is unavailable
/// outside this crate — including to the `hmn` binary's own `watch`
/// per-sample formatter tests, the first consumer of this builder (the
/// same circumstance that produced [`crate::SpillReportBuilder`] in
/// v0.2.5; see `ROADMAP.md`'s "add per type as downstream tests demand"
/// clause).
///
/// Each setter consumes `self` and returns `Self`. Unset fields take the
/// documented defaults: `pid = 0`, `name = None`, `used_bytes = 0`,
/// `shared_used_bytes = 0`, `source` = [`GpuQuerySource::Pdh`].
///
/// # Example
///
/// ```
/// # #[cfg(feature = "test-helpers")]
/// # {
/// use hypomnesis::GpuProcessEntry;
///
/// let entry = GpuProcessEntry::builder()
///     .pid(1234)
///     .name(Some("python.exe".to_owned()))
///     .used_bytes(8 * 1_024 * 1_024 * 1_024)
///     .shared_used_bytes(256 * 1_024 * 1_024)
///     .build();
/// assert_eq!(entry.pid, 1234);
/// # }
/// ```
#[cfg(feature = "test-helpers")]
#[derive(Debug, Clone)]
pub struct GpuProcessEntryBuilder {
    /// Pending [`GpuProcessEntry::pid`] value, defaults to `0`.
    pid: u32,
    /// Pending [`GpuProcessEntry::name`] value, defaults to `None`.
    name: Option<String>,
    /// Pending [`GpuProcessEntry::used_bytes`] value, defaults to `0`.
    used_bytes: u64,
    /// Pending [`GpuProcessEntry::shared_used_bytes`] value, defaults to `0`.
    shared_used_bytes: u64,
    /// Pending [`GpuProcessEntry::source`] value, defaults to
    /// [`GpuQuerySource::Pdh`] (the backend that populates
    /// `shared_used_bytes`, the field most downstream tests care about).
    source: GpuQuerySource,
}

#[cfg(feature = "test-helpers")]
impl Default for GpuProcessEntryBuilder {
    fn default() -> Self {
        Self {
            pid: 0,
            name: None,
            used_bytes: 0,
            shared_used_bytes: 0,
            source: GpuQuerySource::Pdh,
        }
    }
}

#[cfg(feature = "test-helpers")]
impl GpuProcessEntry {
    /// Start a builder for constructing synthetic `GpuProcessEntry`
    /// values in downstream tests.
    ///
    /// Available only with `features = ["test-helpers"]`. See
    /// [`GpuProcessEntryBuilder`] for the full discussion.
    #[must_use]
    pub fn builder() -> GpuProcessEntryBuilder {
        GpuProcessEntryBuilder::default()
    }
}

#[cfg(feature = "test-helpers")]
impl GpuProcessEntryBuilder {
    /// Set the OS process ID.
    #[must_use]
    pub const fn pid(mut self, pid: u32) -> Self {
        self.pid = pid;
        self
    }

    /// Set the process name (`None` to leave unset).
    #[must_use]
    pub fn name(mut self, name: Option<String>) -> Self {
        self.name = name;
        self
    }

    /// Set the committed GPU memory in bytes.
    #[must_use]
    pub const fn used_bytes(mut self, used_bytes: u64) -> Self {
        self.used_bytes = used_bytes;
        self
    }

    /// Set the resident shared-system-memory bytes (the `WDDM` spill
    /// signal).
    #[must_use]
    pub const fn shared_used_bytes(mut self, shared_used_bytes: u64) -> Self {
        self.shared_used_bytes = shared_used_bytes;
        self
    }

    /// Set the backend that produced this row.
    #[must_use]
    pub const fn source(mut self, source: GpuQuerySource) -> Self {
        self.source = source;
        self
    }

    /// Consume the builder and produce the configured `GpuProcessEntry`.
    ///
    /// Unset fields take the documented defaults.
    #[must_use]
    pub fn build(self) -> GpuProcessEntry {
        GpuProcessEntry {
            pid: self.pid,
            name: self.name,
            used_bytes: self.used_bytes,
            shared_used_bytes: self.shared_used_bytes,
            source: self.source,
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::missing_docs_in_private_items
)]
mod tests {
    use super::*;

    /// Build a `Snapshot` for tests. Sets `gpu` only when `vram_used` is
    /// `Some`; sets `gpu_device` only when `total` is non-zero.
    fn make_snapshot(
        ram: u64,
        vram_used: Option<u64>,
        is_per_process: bool,
        total: u64,
    ) -> Snapshot {
        Snapshot {
            ram_bytes: ram,
            gpu: vram_used.map(|used| ProcessGpuInfo {
                used_bytes: used,
                is_per_process,
                source: if is_per_process {
                    GpuQuerySource::Nvml
                } else {
                    GpuQuerySource::NvidiaSmi
                },
            }),
            gpu_device: if total > 0 {
                Some(GpuDeviceInfo {
                    index: 0,
                    name: None,
                    total_bytes: total,
                    free_bytes: total.saturating_sub(vram_used.unwrap_or(0)),
                    used_bytes: vram_used.unwrap_or(0),
                    reserved_bytes: None,
                })
            } else {
                None
            },
        }
    }

    #[test]
    fn snapshot_constructs_with_no_gpu() {
        let snap = make_snapshot(0, None, false, 0);
        assert_eq!(snap.ram_bytes, 0);
        assert!(snap.gpu.is_none());
        assert!(snap.gpu_device.is_none());
    }

    #[test]
    fn snapshot_constructs_with_full_gpu() {
        let snap = make_snapshot(1_048_576, Some(500 * 1_048_576), true, 16_384 * 1_048_576);
        assert_eq!(snap.ram_bytes, 1_048_576);
        assert_eq!(snap.gpu.as_ref().unwrap().used_bytes, 500 * 1_048_576);
        assert!(snap.gpu.as_ref().unwrap().is_per_process);
        assert_eq!(
            snap.gpu_device.as_ref().unwrap().total_bytes,
            16_384 * 1_048_576
        );
    }

    #[cfg(feature = "report")]
    #[test]
    fn ram_mb_conversion() {
        // exactly 1 MB
        let snap = make_snapshot(1_048_576, None, false, 0);
        assert!((snap.ram_mb() - 1.0).abs() < 0.001);
    }

    #[cfg(feature = "report")]
    #[test]
    fn ram_mb_zero() {
        let snap = make_snapshot(0, None, false, 0);
        assert!(snap.ram_mb().abs() < 0.001);
    }

    #[cfg(feature = "report")]
    #[test]
    fn vram_mb_none_when_no_gpu() {
        let snap = make_snapshot(100, None, false, 0);
        assert!(snap.vram_mb().is_none());
    }

    #[cfg(feature = "report")]
    #[test]
    fn vram_mb_some_when_gpu_present() {
        let snap = make_snapshot(100, Some(2 * 1_048_576), true, 16 * 1_048_576);
        assert!((snap.vram_mb().unwrap() - 2.0).abs() < 0.001);
    }

    // -----------------------------------------------------------------------
    // GpuDeviceInfo::format_free / print_free tests (Wave A)
    // -----------------------------------------------------------------------

    #[cfg(feature = "report")]
    #[test]
    fn format_free_with_name() {
        let dev = GpuDeviceInfo {
            index: 0,
            name: Some("NVIDIA Test GPU".to_owned()),
            total_bytes: 16_384 * 1_048_576,
            free_bytes: 13_284 * 1_048_576,
            used_bytes: 3_100 * 1_048_576,
            reserved_bytes: None,
        };
        assert_eq!(
            dev.format_free(),
            "  GPU 0: free 13284 MB / 16384 MB [NVIDIA Test GPU]\n"
        );
    }

    #[cfg(feature = "report")]
    #[test]
    fn format_free_without_name() {
        let dev = GpuDeviceInfo {
            index: 1,
            name: None,
            total_bytes: 8_192 * 1_048_576,
            free_bytes: 4_096 * 1_048_576,
            used_bytes: 4_096 * 1_048_576,
            reserved_bytes: None,
        };
        assert_eq!(dev.format_free(), "  GPU 1: free 4096 MB / 8192 MB\n");
    }

    #[cfg(feature = "report")]
    #[test]
    fn format_free_full_device() {
        // free == 0 (fully-allocated device); should still render cleanly.
        let dev = GpuDeviceInfo {
            index: 2,
            name: Some("Saturated GPU".to_owned()),
            total_bytes: 4_096 * 1_048_576,
            free_bytes: 0,
            used_bytes: 4_096 * 1_048_576,
            reserved_bytes: None,
        };
        assert_eq!(
            dev.format_free(),
            "  GPU 2: free 0 MB / 4096 MB [Saturated GPU]\n"
        );
    }

    #[cfg(feature = "report")]
    #[test]
    fn print_free_does_not_panic() {
        let dev = GpuDeviceInfo {
            index: 0,
            name: None,
            total_bytes: 1_000,
            free_bytes: 500,
            used_bytes: 500,
            reserved_bytes: None,
        };
        dev.print_free();
    }

    // -----------------------------------------------------------------------
    // GpuDeviceInfo::format_total / format_used tests (Wave C of v0.2.1)
    // -----------------------------------------------------------------------

    #[cfg(feature = "report")]
    #[test]
    fn format_total_with_name() {
        let dev = GpuDeviceInfo {
            index: 0,
            name: Some("NVIDIA Test GPU".to_owned()),
            total_bytes: 16_384 * 1_048_576,
            free_bytes: 13_284 * 1_048_576,
            used_bytes: 3_100 * 1_048_576,
            reserved_bytes: None,
        };
        assert_eq!(
            dev.format_total(),
            "  GPU 0: total 16384 MB [NVIDIA Test GPU]\n"
        );
    }

    #[cfg(feature = "report")]
    #[test]
    fn format_total_without_name() {
        let dev = GpuDeviceInfo {
            index: 1,
            name: None,
            total_bytes: 8_192 * 1_048_576,
            free_bytes: 4_096 * 1_048_576,
            used_bytes: 4_096 * 1_048_576,
            reserved_bytes: None,
        };
        assert_eq!(dev.format_total(), "  GPU 1: total 8192 MB\n");
    }

    #[cfg(feature = "report")]
    #[test]
    fn format_total_full_device() {
        // free == 0 (fully-allocated device); total still renders correctly.
        let dev = GpuDeviceInfo {
            index: 2,
            name: Some("Saturated GPU".to_owned()),
            total_bytes: 4_096 * 1_048_576,
            free_bytes: 0,
            used_bytes: 4_096 * 1_048_576,
            reserved_bytes: None,
        };
        assert_eq!(
            dev.format_total(),
            "  GPU 2: total 4096 MB [Saturated GPU]\n"
        );
    }

    #[cfg(feature = "report")]
    #[test]
    fn format_used_with_name() {
        let dev = GpuDeviceInfo {
            index: 0,
            name: Some("NVIDIA Test GPU".to_owned()),
            total_bytes: 16_384 * 1_048_576,
            free_bytes: 13_284 * 1_048_576,
            used_bytes: 3_100 * 1_048_576,
            reserved_bytes: None,
        };
        assert_eq!(
            dev.format_used(),
            "  GPU 0: used 3100 MB [NVIDIA Test GPU]\n"
        );
    }

    #[cfg(feature = "report")]
    #[test]
    fn format_used_without_name() {
        let dev = GpuDeviceInfo {
            index: 1,
            name: None,
            total_bytes: 8_192 * 1_048_576,
            free_bytes: 4_096 * 1_048_576,
            used_bytes: 4_096 * 1_048_576,
            reserved_bytes: None,
        };
        assert_eq!(dev.format_used(), "  GPU 1: used 4096 MB\n");
    }

    #[cfg(feature = "report")]
    #[test]
    fn format_used_idle_device() {
        // used == 0 (idle / unallocated device); used line still renders.
        let dev = GpuDeviceInfo {
            index: 2,
            name: Some("Idle GPU".to_owned()),
            total_bytes: 4_096 * 1_048_576,
            free_bytes: 4_096 * 1_048_576,
            used_bytes: 0,
            reserved_bytes: None,
        };
        assert_eq!(dev.format_used(), "  GPU 2: used 0 MB [Idle GPU]\n");
    }

    // -----------------------------------------------------------------------
    // GpuDeviceInfo::name_or_unknown tests (Wave B of v0.2.1)
    // -----------------------------------------------------------------------

    #[test]
    fn name_or_unknown_returns_inner_when_some() {
        let dev = GpuDeviceInfo {
            index: 0,
            name: Some("NVIDIA Test GPU".to_owned()),
            total_bytes: 0,
            free_bytes: 0,
            used_bytes: 0,
            reserved_bytes: None,
        };
        assert_eq!(dev.name_or_unknown(), "NVIDIA Test GPU");
    }

    #[test]
    fn name_or_unknown_returns_fallback_when_none() {
        let dev = GpuDeviceInfo {
            index: 0,
            name: None,
            total_bytes: 0,
            free_bytes: 0,
            used_bytes: 0,
            reserved_bytes: None,
        };
        assert_eq!(dev.name_or_unknown(), "unknown GPU");
    }

    // -----------------------------------------------------------------------
    // GpuDeviceInfoBuilder tests (Wave A of v0.2.1)
    // -----------------------------------------------------------------------

    #[cfg(feature = "test-helpers")]
    #[test]
    fn builder_defaults_produce_zeroed_device() {
        let dev = GpuDeviceInfo::builder().build();
        assert_eq!(dev.index, 0);
        assert!(dev.name.is_none());
        assert_eq!(dev.total_bytes, 0);
        assert_eq!(dev.free_bytes, 0);
        assert_eq!(dev.used_bytes, 0);
        assert!(dev.reserved_bytes.is_none());
    }

    #[cfg(feature = "test-helpers")]
    #[test]
    fn builder_index_setter() {
        let dev = GpuDeviceInfo::builder().index(7).build();
        assert_eq!(dev.index, 7);
    }

    #[cfg(feature = "test-helpers")]
    #[test]
    fn builder_name_setter_some() {
        let dev = GpuDeviceInfo::builder()
            .name(Some("Synthetic GPU".to_owned()))
            .build();
        assert_eq!(dev.name.as_deref(), Some("Synthetic GPU"));
    }

    #[cfg(feature = "test-helpers")]
    #[test]
    fn builder_name_setter_none_is_default() {
        let dev = GpuDeviceInfo::builder().name(None).build();
        assert!(dev.name.is_none());
    }

    #[cfg(feature = "test-helpers")]
    #[test]
    fn builder_byte_setters() {
        let dev = GpuDeviceInfo::builder()
            .total_bytes(16 * 1_024 * 1_024 * 1_024)
            .free_bytes(14 * 1_024 * 1_024 * 1_024)
            .used_bytes(2 * 1_024 * 1_024 * 1_024)
            .build();
        assert_eq!(dev.total_bytes, 16 * 1_024 * 1_024 * 1_024);
        assert_eq!(dev.free_bytes, 14 * 1_024 * 1_024 * 1_024);
        assert_eq!(dev.used_bytes, 2 * 1_024 * 1_024 * 1_024);
    }

    #[cfg(feature = "test-helpers")]
    #[test]
    fn builder_full_round_trip() {
        let dev = GpuDeviceInfo::builder()
            .index(1)
            .name(Some("Round-Trip GPU".to_owned()))
            .total_bytes(17_179_869_184)
            .free_bytes(15_246_684_160)
            .used_bytes(1_933_185_024)
            .reserved_bytes(Some(76_546_048))
            .build();
        assert_eq!(dev.index, 1);
        assert_eq!(dev.name.as_deref(), Some("Round-Trip GPU"));
        assert_eq!(dev.total_bytes, 17_179_869_184);
        assert_eq!(dev.free_bytes, 15_246_684_160);
        assert_eq!(dev.used_bytes, 1_933_185_024);
        assert_eq!(dev.reserved_bytes, Some(76_546_048));
        // `reserved_bytes` is a subset of `total_bytes` (NVML's
        // `total = reserved + free + used`), so memory available for
        // allocation is `total - reserved`, never `total + reserved`.
        assert!(dev.reserved_bytes.unwrap() < dev.total_bytes);
        assert_eq!(
            dev.total_bytes - dev.reserved_bytes.unwrap(),
            17_179_869_184 - 76_546_048
        );
    }
}
