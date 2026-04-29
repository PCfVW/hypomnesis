// SPDX-License-Identifier: MIT OR Apache-2.0

//! Error types for `hypomnesis`.

/// Errors that can occur during a `hypomnesis` measurement.
///
/// `#[non_exhaustive]`: new variants will be added as new backends are introduced
/// (e.g., AMD `ROCm` SMI, Apple Metal). Patch-release-safe.
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
    /// Returned when `NVML`, `DXGI`, and `nvidia-smi` all failed (or were
    /// disabled by feature flags) for a single query.
    #[error(
        "no GPU measurement source available (NVML, DXGI, and nvidia-smi all failed or are disabled)"
    )]
    NoGpuSource,

    /// I/O error (e.g., reading `/proc/self/status` on Linux).
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Result alias for `hypomnesis` operations.
pub type Result<T> = std::result::Result<T, HypomnesisError>;
