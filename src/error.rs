// SPDX-License-Identifier: MIT OR Apache-2.0

//! Error types for `hypomnesis`.

/// Errors that can occur during a `hypomnesis` measurement.
///
/// `#[non_exhaustive]`: new variants will be added as new backends are introduced
/// (e.g., AMD `ROCm` SMI, Apple Metal). Patch-release-safe.
///
/// # `Display` vs structured fields
///
/// `HypomnesisError`'s `Display` impl is the **default English one-liner** —
/// suitable for logs, library-tier error reporting, and `?`-propagation where
/// the consumer is content with the default rendering. Structured fields
/// ([`Self::DeviceIndexOutOfRange`]'s `index` / `count`, the inner `String` of
/// [`Self::Nvml`] / [`Self::Dxgi`] / [`Self::Pdh`] / [`Self::NvidiaSmi`]) are
/// the **canonical source** for any consumer that wants to:
///
/// - Localize the message to a non-English language.
/// - Restyle for a CLI / GUI / JSON output (column-aligned tables,
///   wrap-aware formatting, JSON keys for the structured pieces).
/// - Apply singular / plural agreement, custom punctuation, or richer
///   formatting (`"have 1 device"` vs the default `"have 1 devices"`,
///   for instance).
///
/// This contract makes `Display` stable for the common case while leaving
/// custom-render consumers free to assemble their own strings without
/// fighting the default. Consumers writing user-facing tools should prefer
/// `match err { HypomnesisError::DeviceIndexOutOfRange { index, count } => ... }`
/// over `format!("{err}")`. Future `Display`-string improvements will avoid
/// adding structural information that the structured fields already expose
/// (so consumers that hand-format from the fields cannot end up
/// double-rendering the count).
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum HypomnesisError {
    /// Process `RSS` query failed (platform API error).
    #[error("RAM query failed: {0}")]
    Ram(String),

    /// `NVML` query failed (library load, symbol lookup, FFI call,
    /// or driver-reported error code).
    #[error("NVML error: {0}")]
    Nvml(String),

    /// `DXGI` query failed (factory creation, adapter enumeration,
    /// `IDXGIAdapter3` cast, or interface call).
    #[error("DXGI error: {0}")]
    Dxgi(String),

    /// `PDH` (Windows Performance Data Helper) query failed (counter
    /// enumeration, counter add, value collection, or instance parsing
    /// for the `GPU Process Memory` or `GPU Adapter Memory` counter
    /// sets — the latter backs the v0.2.5 spill-detection path).
    /// Includes the case where the counter set is unregistered on
    /// pre-`WDDM 2.0` systems.
    #[error("PDH error: {0}")]
    Pdh(String),

    /// `nvidia-smi` subprocess invocation failed or produced unparseable output.
    #[error("nvidia-smi error: {0}")]
    NvidiaSmi(String),

    /// Requested device index is past the number of available GPUs.
    #[error("device index {index} out of range (have {count} devices)")]
    DeviceIndexOutOfRange {
        /// The requested zero-based index.
        index: u32,
        /// The number of available devices.
        count: u32,
    },

    /// No GPU measurement source was usable.
    ///
    /// Returned when `NVML`, `DXGI`, `PDH`, and `nvidia-smi` all failed
    /// (or were disabled by feature flags) for a single query.
    #[error(
        "no GPU measurement source available (NVML, DXGI, PDH, and nvidia-smi all failed or are disabled)"
    )]
    NoGpuSource,

    /// Generic I/O error.
    ///
    /// Reserved for a possible future I/O-based backend — this crate
    /// does not currently construct this variant itself: the one
    /// existing filesystem read (`/proc/self/status` on Linux) is
    /// deliberately wrapped into [`Self::Ram`] instead (see
    /// [`crate::Snapshot::now`]'s `# Errors` section), so downstream
    /// code should not expect to observe `Io` from this crate's public
    /// API today.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Result alias for `hypomnesis` operations.
pub type Result<T> = std::result::Result<T, HypomnesisError>;
