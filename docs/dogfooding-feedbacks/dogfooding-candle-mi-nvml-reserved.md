# Dogfooding report (from candle-mi): surface NVML `reserved` memory

**Date:** 2026-06-29
**Reporter:** candle-mi (v0.1.16 — migrated `src/memory.rs` to `hypomnesis 0.2.3`)
**Severity:** Enhancement — additive, non-breaking
**Affected area:** `gpu::device_info` / `GpuDeviceInfo` (the device-wide total/free/used triplet)
**Status:** ✅ **Resolved in v0.2.4** (2026-06-29)

---

> ## Resolution (v0.2.4)
>
> Shipped as `GpuDeviceInfo::reserved_bytes: Option<u64>`, sourced from
> `nvmlDeviceGetMemoryInfo_v2` (R510+) with a graceful pre-R510 fallback to
> `None`. `total_bytes` is unchanged. See [`CHANGELOG.md`](../../CHANGELOG.md)
> and [`docs/roadmap-v0.2.4.md`](../roadmap-v0.2.4.md).
>
> **Correction to this report's figures (live-validated on the reference
> RTX 5060 Ti, driver 591.86).** The real driver/firmware reservation is
> **259 MiB**, not the 73 MiB inferred below. Three sources agree:
> `nvidia-smi -q -d MEMORY` prints `Total: 16311 MiB` / `Reserved: 259 MiB`;
> NVML v1 `total` == v2 `total` == 16311 MiB; and the raw v2 struct satisfies
> `reserved + free + used == total` exactly — so `reserved` is a **subset of**
> `total`, not added on top.
>
> The report's **73 MiB** (`DXGI 16384 − NVML 16311`) is a *different*
> quantity: board/ECC overhead sitting *below* what NVML reports, which NVML
> does not expose as a field. It was also computed from two non-agreeing
> sources — live `DXGI DedicatedVideoMemory` is `16052 MiB` here, not the
> `16384` assumed below. The shipped field reads NVML's own v2 `reserved`
> directly rather than reverse-engineering it from a cross-source
> subtraction. The request was sound; only the inferred magnitude was off.

---

## Context — the migration went well

candle-mi v0.1.16 deletes its ~600 lines of in-tree measurement FFI and delegates
to `hypomnesis 0.2.3` (lean set: `nvml`, `dxgi`, `nvidia-smi-fallback`, `metal`).
Live validation on an RTX 5060 Ti (16 GB, Windows/WDDM):

- per-process **used** (DXGI) — a 512 MiB GPU allocation produced an **exact
  512 MB** `MemorySnapshot` VRAM delta;
- GPU **name** — `dxgi::adapter_name().or(nvml)` gives byte-identical
  `"NVIDIA GeForce RTX 5060 Ti"`;
- the per-process flag and RAM are all correct.

No defect. One observation worth an enhancement.

## Observation — the device total is the *usable* figure, with no way to see the carveout

`device_info().total_bytes` reports **16311 MiB** on this card — the NVML v1
`nvmlDeviceGetMemoryInfo` total, which equals `nvidia-smi` (good, ecosystem-consistent).
But the card's **physical** capacity is **16384 MiB** (DXGI `DedicatedVideoMemory`).
The **73 MiB** gap is NVIDIA's documented driver/firmware reservation:

> NVML `nvmlMemory_v2_t.reserved` — *"Device memory (in bytes) reserved for system
> use (driver or firmware)."* (page tables, context/channel structures, ECC parity).

So `16384 physical = 16311 usable + 73 reserved`. Today hypomnesis surfaces only
the usable total; a consumer cannot show the breakdown (e.g. candle-mi would like
to print `… / 16311 MB usable (+73 MB driver-reserved)`).

## Request — expose `reserved` via `nvmlDeviceGetMemoryInfo_v2`

Add an optional reserved-memory figure to `GpuDeviceInfo`, sourced from NVML's
**v2** memory query:

- Call `nvmlDeviceGetMemoryInfo_v2` (`nvmlMemory_v2_t`: `version, total, reserved,
  free, used`). In v2, `total` is the **full physical** (16384) and `reserved`
  (73) is broken out — unlike v1, whose `total` is already net of the reservation.
- Add `reserved_bytes: Option<u64>` to `GpuDeviceInfo` (the struct is
  `#[non_exhaustive]`, so this is additive/non-breaking).
- **Driver-compat fallback:** `nvmlDeviceGetMemoryInfo_v2` is R510+; on older
  drivers it returns `NVML_ERROR_FUNCTION_NOT_FOUND` (see nvidia-settings #78).
  Fall back to v1 and set `reserved_bytes = None`.
- Consider whether `total_bytes` should remain the usable figure (16311, current,
  `nvidia-smi`-consistent — recommended for stability) or move to the v2 physical
  (16384); if the latter, that's a semantic change worth a major/minor bump and a
  CHANGELOG note. Keeping `total_bytes` as-is and *adding* `reserved_bytes` is the
  non-breaking path.

## Evidence (RTX 5060 Ti, Windows 11, driver 591.86)

| Source | Total | Notes |
|---|---|---|
| DXGI `DedicatedVideoMemory` | 16384 MiB | full physical / nominal "16 GB" |
| NVML v1 `nvmlDeviceGetMemoryInfo.total` | 16311 MiB | = `nvidia-smi` (usable) |
| `nvidia-smi --query-gpu=memory.total` | 16311 MiB | confirms NVML v1 |
| implied `reserved` | 73 MiB | = NVML v2 `reserved` |

## Downstream payoff

Once `reserved_bytes` lands, candle-mi (and `hmn`) can render the honest
breakdown — *usable + reserved (+ physical)* — turning "why is it 16311 not
16384?" from a FAQ into a one-line answer in the output itself.

## References

- NVML `nvmlMemory_v2_t`: <https://docs.nvidia.com/deploy/nvml-api/structnvmlMemory__v2__t.html>
- NVML device queries: <https://docs.nvidia.com/deploy/nvml-api/group__nvmlDeviceQueries.html>
- `nvmlDeviceGetMemoryInfo_v2` driver-compat (R510+ `FUNCTION_NOT_FOUND`): nvidia-settings issue #78
- NVIDIA forum — NVML total = `nvidia-smi`: <https://forums.developer.nvidia.com/t/why-cudamemgetinfo-total-memory-less-than-nvmldevicegetmemoryinfo-total-memory/370883>
