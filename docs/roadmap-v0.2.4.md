# `hypomnesis` v0.2.4 — roadmap

> *The same total. Now with the carve-out shown.*

---

## Why v0.2.4 (and not v0.3.0)

Every item in this release is **additive and patch-safe** under the
`#[non_exhaustive]` policy already in place. `GpuDeviceInfo` gains one new
field (`reserved_bytes: Option<u64>`); no existing field changes type, no
method changes signature, no default feature flips. `total_bytes` keeps its
exact meaning — the usable, `nvidia-smi`-consistent figure from NVML v1
`nvmlDeviceGetMemoryInfo` — so every existing consumer sees byte-for-byte
identical behaviour. The new field is *purely additive information*: when a
backend can't supply it, it is `None`.

**v0.2.4 is the right vehicle, not v0.3.0.** Adding a defaulted field to a
`#[non_exhaustive]` struct is the canonical patch-safe change (Principle 2).
The only place the addition is observable is in code that explicitly reads
`reserved_bytes`, and that code is new by definition.

---

## Origin — a `candle-mi` dogfooding report

`candle-mi` v0.1.16 migrated `src/memory.rs` to `hypomnesis 0.2.3` (lean
feature set: `nvml`, `dxgi`, `nvidia-smi-fallback`, `metal`), deleting its
in-tree measurement FFI. Live validation on the reference `RTX 5060 Ti`
(16 GiB, Windows / `WDDM`) confirmed per-process used (DXGI), device name,
and the per-process flag were all correct. The report flagged one
enhancement, not a defect:

> `device_info().total_bytes` reports **16311 MiB** — the NVML v1 total,
> which equals `nvidia-smi` (good, ecosystem-consistent). But the card's
> physical capacity is **16384 MiB** (DXGI `DedicatedVideoMemory`). The
> **73 MiB** gap is NVIDIA's documented driver/firmware reservation, and a
> consumer had no way to see the breakdown.

So the report *inferred* `73 reserved` from `DXGI 16384 − NVML 16311`.
Before v0.2.4 hypomnesis surfaced only the `total`; this release adds the
carve-out — read **directly** from NVML rather than inferred.

> **Live correction (v0.2.4 validation).** Running the new NVML v2 query on
> the same `RTX 5060 Ti` (driver 591.86) reports the true reservation as
> **259 MiB** (`271 581 184` bytes), not the 73 MiB the report inferred.
> Three sources agree on the real picture:
>
> - `nvidia-smi -q -d MEMORY` → `Total: 16311 MiB`, `Reserved: 259 MiB`.
> - NVML v1 `total` = NVML v2 `total` = `17 103 323 136` B (16311 MiB) —
>   **identical**; and the raw v2 struct satisfies
>   `reserved + free + used == total` exactly, so `reserved` is a *subset*
>   of `total`, not stacked on top of it.
>
> The report's 73 MiB is a **different** quantity: `nominal 16384 − NVML
> total 16311` — board/ECC overhead that sits *below* what NVML reports and
> which NVML does not expose as a field. It was computed by subtracting two
> *different* sources (`DXGI DedicatedVideoMemory` minus NVML `total`) that
> don't even agree on capacity — live `DXGI DedicatedVideoMemory` is itself
> `16052 MiB` here, not the `16384` the report assumed. This is exactly why
> the field is sourced from NVML's own v2 query: the carve-out is reported,
> not reverse-engineered from a cross-source subtraction.

---

## Scope

Code-first, then docs. The maintainer's `RTX 5060 Ti` (Windows / `WDDM`,
driver 591.86; Ubuntu WSL2) covers live verification. Non-NVIDIA and
pre-R510 paths degrade to `reserved_bytes = None` and are unchanged.

### `src/gpu/nvml.rs` — NVML v2 memory query

NVML's v2 query (`nvmlDeviceGetMemoryInfo_v2`) is the only call that breaks
out the reservation. Its struct differs from v1 only by exposing `reserved`
separately — **both report the same `total`** (verified live: v1 `total` ==
v2 `total` == 16311 MiB), satisfying `total = reserved + free + used`. v1
folds the reservation into `used`; v2 surfaces it as its own field:

| Struct | Fields | `total` semantics |
|---|---|---|
| `nvmlMemory_t` (v1) | `total`, `free`, `used` | full framebuffer = `nvidia-smi` `Total`; `reserved` bundled into `used` |
| `nvmlMemory_v2_t` | `version`, `total`, `reserved`, `free`, `used` | same `total`; `reserved` broken out (`total = reserved + free + used`) |

`reserved` is therefore a **subset** of `total`, never an addition on top —
allocation headroom is `total − reserved` (and `free` already reflects it).

Two implementation details matter:

1. **The `version` field must be set.** `nvmlMemory_v2_t` carries a leading
   struct-version tag computed as `size_of::<nvmlMemory_v2_t>() | (2 << 24)`
   (NVML's `NVML_STRUCT_VERSION(Memory, 2)` macro). An unset or mismatched
   version yields `NVML_ERROR_INVALID_ARGUMENT`. hypomnesis encodes this as
   the `NVML_MEMORY_V2_VERSION` constant.
2. **Pre-R510 fallback is a symbol-absence, not a runtime error.**
   `nvmlDeviceGetMemoryInfo_v2` is R510+. Because hypomnesis loads symbols
   via `libloading`, an older driver's `nvml.dll` / `libnvidia-ml.so.1`
   simply doesn't export the symbol — `lib.get(...)` returns `Err`, mapped
   to `None`. The documented `NVML_ERROR_FUNCTION_NOT_FOUND` return code
   belongs to NVML's versioned-pointer dispatch, which we don't use; the
   net effect (`reserved_bytes = None` on old drivers) is the same and even
   simpler.

The v1 `total` / `free` / `used` path is untouched. The v2 call is a
best-effort *add-on*, run after the v1 query succeeds, purely to read
`reserved`. This keeps `total_bytes` the v1 figure with zero regression
risk to the existing triplet.

### `src/snapshot.rs` — the additive field + builder

- `GpuDeviceInfo::reserved_bytes: Option<u64>` — `Some` only on the NVML
  R510+ path; `None` everywhere else (DXGI, nvidia-smi, Metal, pre-R510).
- `GpuDeviceInfoBuilder::reserved_bytes(Option<u64>)` setter under the
  `test-helpers` feature, defaulting to `None`.

### `src/gpu/mod.rs` — dispatcher plumbing

All five `GpuDeviceInfo` construction sites populate the field: the NVML
arm forwards `snap.reserved_bytes`; the Metal, DXGI-alone, `nvidia-smi`,
and non-NVIDIA-DXGI arms set `None` (none of those sources expose the NVML
carve-out).

### `hmn` — render the breakdown

The device summary appends a parenthetical when present:

```text
GPU 0 [NVIDIA GeForce RTX 5060 Ti]: free 13284 MiB / 16311 MiB (259 MiB reserved)
```

`total` (16311) is the full NVML framebuffer = `nvidia-smi` `Total`; the
parenthetical reads as "of which 259 MiB is driver-reserved" — a subset,
matching `nvidia-smi -q`'s separate `Total` / `Reserved` lines, **not** an
addition on top. Elided on backends that report `None`, so the existing
line is unchanged where no reserved figure exists.

---

## Verification

- `cargo test` (unit) — builder round-trip asserts `reserved_bytes` plumbs
  through and that it is a subset of `total` (`reserved < total`, headroom
  = `total − reserved`).
- `tests/live_gpu.rs::device_info_reserved_bytes_is_plausible_when_present`
  — **live** integration test on the `RTX 5060 Ti`: when `Some`, the
  carve-out is non-zero and a small slice of the card (`reserved < total`),
  with `total` within the 1 TiB sanity bound. `#[ignore]`-gated like the other
  live-GPU tests; run with `cargo test -- --ignored`. Live result on the
  reference card: `reserved_bytes = Some(271_581_184)` (259 MiB), driver
  591.86, NVML v2 — confirming the `NVML_MEMORY_V2_VERSION` struct-version
  tag is correct (a wrong tag would yield `NVML_ERROR_INVALID_ARGUMENT` and
  `None`).

This release's live run also surfaced and fixed two **pre-existing** stale
assertions in `tests/live_gpu.rs::gpu_processes_succeeds_on_live_host`
(unrelated to `reserved_bytes`, latent since the v0.2.2 PDH backend because
the test is `#[ignore]`-gated and never runs in CI): it rejected the `Pdh`
source, and it asserted `used_bytes > 0` for every row even though PDH is
not compute-only and legitimately surfaces 0-dedicated-byte rows. Both now
match the documented backend semantics (and the `tests/smoke.rs`
allow-list).

---

## Downstream payoff

Once `reserved_bytes` lands, `candle-mi` (and `hmn`) render the honest
breakdown — *total, of which N reserved* — without inventing the figure,
exactly as `nvidia-smi -q -d MEMORY` prints `Total` next to `Reserved`.
`reserved_bytes` is a subset of `total_bytes`, so allocation headroom is
`total_bytes − reserved_bytes` (already reflected in `free_bytes`).
Consumers that only care about the reported `total` keep reading
`total_bytes` exactly as before.

---

## References

- NVML `nvmlMemory_v2_t`: <https://docs.nvidia.com/deploy/nvml-api/structnvmlMemory__v2__t.html>
- NVML device queries: <https://docs.nvidia.com/deploy/nvml-api/group__nvmlDeviceQueries.html>
