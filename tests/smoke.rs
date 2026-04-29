// SPDX-License-Identifier: MIT OR Apache-2.0

//! Smoke test: verify the public API surface compiles and is reachable
//! from outside the crate.
//!
//! `v0.0.1` placeholder bodies return errors; this test only exercises
//! types and re-exports. Functional tests land in Wave 2 alongside the
//! port from `candle-mi/src/memory.rs`.

use hypomnesis::{GpuDeviceInfo, GpuQuerySource, HypomnesisError, ProcessGpuInfo, Snapshot};

#[test]
fn public_types_are_reachable_via_crate_root() {
    let _: Option<GpuDeviceInfo> = None;
    let _: Option<ProcessGpuInfo> = None;
    let _: Option<Snapshot> = None;
    let _: GpuQuerySource = GpuQuerySource::Dxgi;
    let _: GpuQuerySource = GpuQuerySource::Nvml;
    let _: GpuQuerySource = GpuQuerySource::NvidiaSmi;
    let _: HypomnesisError = HypomnesisError::NoGpuSource;
}

#[cfg(feature = "report")]
#[test]
fn report_feature_types_are_reachable() {
    let _: Option<hypomnesis::MemoryReport> = None;
}
