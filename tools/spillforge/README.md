# spillforge

Forced-`WDDM`-spill fixture for validating hypomnesis spill detection.
**Windows-only**, `publish = false` — lives in the repository, not in the
published crate (`cargo package` auto-excludes nested packages).

Allocates `D3D11` default-heap buffers **with initial-data uploads** past the
card's dedicated `VRAM`, then keeps the whole working set hot with round-robin
touches so `VidMm` must keep it resident — producing a real, reproducible
spill: dedicated-resident pegged at its budget ceiling, overflow paged into
shared-system-memory residency.

## Usage

```sh
cargo build --release --manifest-path tools/spillforge/Cargo.toml
hmn spill -- tools\spillforge\target\release\spillforge.exe [TARGET_GIB] [HOLD_SECS]
```

- `TARGET_GIB` (default 20) — total working set; pick ~1.25× your dedicated
  `VRAM`.
- `HOLD_SECS` (default 10) — churn duration after allocation.

Expect brief desktop sluggishness during the churn; everything releases on
exit. Reference result (RTX 5060 Ti 16 GiB, v0.2.5 release validation): one
13.1 s episode, peak shared 3.1 GiB over a 163 MiB baseline.

## Why uploads *and* churn

Two measured `WDDM` facts this tool encodes (full record:
[`docs/roadmap-v0.2.5.md`](../../docs/roadmap-v0.2.5.md), *Implementation
notes (as shipped)*):

1. **Commit alone produces no spill** — buffers created without initial data
   are committed but never resident; hypomnesis correctly reports nothing.
2. **An idle working set is evicted to backing store, not shared residency** —
   without the touch loop, `VidMm` demotes untouched resources out of GPU
   visibility and the shared gauge under-reports.

Which is the point: this fixture also doubles as a regression check that the
detector ignores commit (run it with the upload path disabled) and fires on
genuine residency pressure.
