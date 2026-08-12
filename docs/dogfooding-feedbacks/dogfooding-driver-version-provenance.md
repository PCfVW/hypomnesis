# Dogfooding report (from candle-mi): expose the GPU driver version

**Date:** 2026-08-12
**Reporter:** candle-mi (v0.1.22 release verification, `scripts/resurrect.ps1`)
**Severity:** Enhancement, additive and non-breaking
**Affected area:** `gpu::device_info` / `GpuDeviceInfo`, plus the `hmn` device summary
**Status:** ✅ **Resolved in v0.2.9** (2026-08-12)

---

> ## Resolution (v0.2.9)
>
> Shipped as `GpuDeviceInfo::driver_version: Option<String>`, sourced from
> `nvmlSystemGetDriverVersion` and — since `nvidia-smi` can genuinely
> supply this figure, unlike `reserved_bytes` — also from the
> `nvidia-smi` fallback (`--query-gpu=driver_version`, one extra CSV
> column on the existing device-wide query). `None` on `DXGI`-alone,
> non-NVIDIA `DXGI` adapters, and `Metal` (macOS has no NVIDIA driver).
> Renders on the `hmn` device-summary line
> (`..., driver 610.88`) and via a new `hmn --json` flag on the default
> subcommand — no JSON surface existed for the bare summary before this
> release, so one was added rather than deferred. See
> [`CHANGELOG.md`](../../CHANGELOG.md) and
> [`docs/roadmap-v0.2.9.md`](../roadmap-v0.2.9.md).
>
> **Live-validated on the reference `RTX 5060 Ti`, post-update driver.**
> `nvmlSystemGetDriverVersion` and the `nvidia-smi` CSV column both report
> `driver_version = Some("610.88")` — the exact post-update version from
> this report's own near-miss story, confirming the two sources agree.

## Summary

`hypomnesis` is the only crate in candle-mi's verification pipeline that talks to
NVML, and it is already load-bearing there: `scripts/resurrect.ps1` drives
`hmn spill --json` for the whole oracle run. But it does not report the **GPU
driver version**, so the pipeline cannot stamp it into the provenance record.

The ask is one `Option<String>` from `nvmlSystemGetDriverVersion`, mirroring the
`reserved_bytes` precedent from v0.2.4 exactly.

## Why this is provenance, not decoration

candle-mi's oracle suite certifies **numeric parity on GPU**: forward passes
matched against Python reference values to six decimal places. `RESURRECTION.md`
stamps each entry with a last-verified date and pins the Rust toolchain:

```
- **Toolchain:** rustc 1.97.1 (8bab26f4f 2026-07-14)
```

It does not pin the GPU driver. **A driver change can move floating-point
results**, so a stamp that records `rustc` but not the driver can certify a
configuration the machine is no longer running, with nothing in the record to
reveal the discrepancy.

This was a live near-miss today, not a hypothetical.

## What happened

A default-tier `resurrect.ps1` run (19 entries) was interrupted at entry 12 by a
bugcheck:

```
DPC_WATCHDOG_VIOLATION (133)
FAILURE_BUCKET_ID: 0x133_ISR_nvlddmkm!unknown_function
Debug session time: Wed Aug 12 09:51:26.255 2026
```

The previous bugcheck on this machine, 2026-06-14, carries the **identical**
bucket. Both are interrupt-service-routine stalls in the NVIDIA display driver.
The remedy was to update it:

| | before | after |
|---|---|---|
| NVML / `nvidia-smi` | `591.86` | `610.88` |
| Windows PnP | `32.0.15.9186` (2026-01-20) | `32.0.16.1088` (2026-07-22) |

Eleven entries had already passed on `591.86` before the crash. Had the run
completed instead, `RESURRECTION.md` would now assert parity verified on
2026-08-12 while the machine had since moved to `610.88`, and **nothing in the
file would show it**. The next person to read that stamp, including a future
release, would have trusted a figure measured on a driver that was gone.

The fix is to record the driver alongside the toolchain. hypomnesis is where
that value naturally comes from.

## Evidence that the project already wants this field

`docs/dogfooding-feedbacks/dogfooding-candle-mi-nvml-reserved.md` opens its
resolution block with:

> live-validated on the reference RTX 5060 Ti, **driver 591.86**

The driver version is already treated as part of what makes a measurement
citable. It is simply transcribed by hand today, which is exactly the kind of
manual step that goes stale or gets forgotten.

## Proposed shape

Mirroring `reserved_bytes` (v0.2.4), so there is no new pattern to defend:

- **Library:** `GpuDeviceInfo::driver_version: Option<String>`, from
  `nvmlSystemGetDriverVersion`. `None` when NVML is unavailable.
- **CLI:** one line in the `hmn` device summary, and a `driver_version` key in
  `hmn --json` so scripts can read it without parsing prose.

`GpuDeviceInfo` is `#[non_exhaustive]`, so this is additive and patch-safe, per
the standing note in `ROADMAP.md` that these enhancements never require a 1.0
bump.

### Notes for the implementer

- **Which number.** NVML returns the NVIDIA-branded version (`610.88`), not the
  Windows PnP form (`32.0.16.1088`). The branded one is what `nvidia-smi`,
  release notes and bug reports all use, so it is the right one to surface. The
  two are not interchangeable and the PnP form is not obtainable from NVML.
- **Backend coverage.** `nvmlSystemGetDriverVersion` is available wherever NVML
  loads (Windows and Linux). The `nvidia-smi` fallback backend can supply the
  same string via `--query-gpu=driver_version`. DXGI and PDH have no equivalent,
  and macOS/Metal has no NVIDIA driver at all, so `Option` is the honest type
  and `None` is a real outcome rather than an error.
- **Optional companion.** `nvmlSystemGetCudaDriverVersion` gives the CUDA driver
  API version, which is the figure that actually gates whether a given CUDA
  toolkit will run. Useful for the same provenance reason, but a separate ask;
  the display driver version is the one that moves numerics.

## What un-gates this

It is already un-gated by this report: candle-mi wants to add a driver line to
`RESURRECTION.md` next to the toolchain line, and `resurrect.ps1` should read it
from `hmn --json` rather than shelling out to `nvidia-smi` separately, given
`hmn` is already invoked throughout that script.
