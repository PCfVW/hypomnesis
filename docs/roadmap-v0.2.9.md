# `hypomnesis` v0.2.9 — roadmap

> *The same total. Now with the driver behind it.*

---

## Why v0.2.9 (and not v0.3.0)

Every item in this release is **additive and patch-safe** under the
`#[non_exhaustive]` policy already in place. `GpuDeviceInfo` gains one new
field (`driver_version: Option<String>`); no existing field changes type,
no method changes signature, no default feature flips. The new field is
*purely additive information*: when a backend can't supply it, it is
`None`.

**v0.2.9 is the right vehicle, not v0.3.0.** Adding a defaulted field to a
`#[non_exhaustive]` struct is the canonical patch-safe change (same
Principle 2 that shipped `reserved_bytes` in v0.2.4). The only place the
addition is observable is in code that explicitly reads `driver_version`,
and that code is new by definition. The new `hmn --json` flag on the
default (no-subcommand) summary is likewise additive — a brand-new flag,
not a change to any existing flag or output.

---

## Origin — a `candle-mi` dogfooding report

`candle-mi`'s `scripts/resurrect.ps1` oracle suite certifies numeric
parity on GPU: forward passes matched against Python reference values to
six decimal places, with each verified entry stamped into
`RESURRECTION.md`. That stamp pins the Rust toolchain but not the GPU
driver — and a driver change can move floating-point results.

The report ([`docs/dogfooding-feedbacks/dogfooding-driver-version-provenance.md`](dogfooding-feedbacks/dogfooding-driver-version-provenance.md))
wasn't a hypothetical: mid-verification-run, the reference machine hit a
`DPC_WATCHDOG_VIOLATION` (bugcheck `0x133`) — an NVIDIA display-driver
interrupt-service-routine stall, identical to a prior bugcheck six weeks
earlier. The fix was a driver update:

| | before | after |
|---|---|---|
| NVML / `nvidia-smi` | `591.86` | `610.88` |

Eleven of nineteen entries had already passed on `591.86` before the
crash. Had the run completed instead, `RESURRECTION.md` would have
asserted parity verified on a driver the machine had since replaced, with
nothing in the file to reveal the discrepancy. The report asks for one
`Option<String>` from `nvmlSystemGetDriverVersion`, mirroring the
`reserved_bytes` precedent "so there is no new pattern to defend."

---

## Scope

Code-first, then docs. The maintainer's `RTX 5060 Ti` (Windows / `WDDM`,
driver 610.88) covers live verification.

### `src/gpu/nvml.rs` — new system-level query

`nvmlSystemGetDriverVersion` is **system-level**, unlike every other NVML
call in this file — no device handle, just a buffer + length, because the
same driver serves every GPU index on the machine. The new
`read_driver_version` helper is modeled on the existing `read_device_name`
(buffer + nul-scan + `String::from_utf8_lossy`), called once per `query()`
session from the already-open library — no second `Library::new` load,
per the FFI-pattern rule to hold the library for the duration of all
calls. Best-effort: symbol-lookup or call failure maps to `None`, the same
policy `read_device_reserved` already established for `reserved_bytes`.

### `src/gpu/nvidia_smi.rs` — extend the existing device-wide query

Unlike `reserved_bytes`, `nvidia-smi` genuinely has this figure:
`--query-gpu=memory.used,memory.total,driver_version` adds one CSV column
to the **existing** device-wide subprocess call — no second spawn. The
CSV-parsing tail of `query()` was extracted into a pure `parse_query_line`
function (mirroring this file's own `parse_compute_app_line`), enabling
direct unit tests. `used`/`total` stay mandatory; the driver field is
non-fatal — empty or missing degrades to `None` without failing the whole
query.

### `src/snapshot.rs` — the additive field + builder

- `GpuDeviceInfo::driver_version: Option<String>` — `Some` on the `NVML`
  and `nvidia-smi` paths; `None` on `DXGI`-alone, non-NVIDIA `DXGI`
  adapters, and `Metal` (macOS has no NVIDIA driver).
- `GpuDeviceInfoBuilder::driver_version(Option<String>)` setter under the
  `test-helpers` feature, defaulting to `None`. Not `const fn` — unlike
  `reserved_bytes`'s `Option<u64>` setter, `Option<String>` is non-`Copy`,
  so this follows the existing `name()` setter's precedent instead (same
  struct, same non-`const` shape).

### `src/gpu/mod.rs` — dispatcher plumbing

All five `GpuDeviceInfo` construction sites populate the field. The NVML
arm forwards `snap.driver_version`; the **`nvidia-smi` arm now also
forwards `result.driver_version`** (the one site that diverges from
`reserved_bytes`'s treatment, which hard-codes `None` there); the Metal,
DXGI-alone, and non-NVIDIA-DXGI arms set `None` (none of those sources
expose an NVIDIA driver string).

### `hmn` — render the driver version, plus a new `--json` flag

The device summary appends the driver version after the reserved
parenthetical:

```text
GPU 0 [NVIDIA GeForce RTX 5060 Ti]: free 14274 MiB / 16311 MiB (259 MiB reserved), driver 610.88
```

Elided on backends that report `None`, so the existing line is unchanged
where no driver string exists. The dogfooding report also asked for "a
`driver_version` key in `hmn --json`" — but no `--json` surface existed
for the bare summary subcommand (only `ps`/`spill`/`watch` had one). This
release adds a top-level `--json` flag to the default subcommand, emitting
one JSON object per visible GPU with every `GpuDeviceInfo` field.

---

## Verification

- `cargo test --all-features` (unit) — builder round-trip asserts
  `driver_version` plumbs through; `nvidia_smi::parse_query_line` unit
  tests cover the new CSV column (present, empty, missing entirely,
  unparseable `used`/`total`); `format_summary_json_empty_input` covers
  the new JSON formatter's empty case (the only case unit-testable at the
  `hmn` binary level — `Snapshot` is `#[non_exhaustive]` with no builder,
  so `hmn.rs` cannot construct a populated one even under
  `test-helpers`, same pre-existing gap `reserved_bytes` hit).
- `tests/live_gpu.rs::device_info_driver_version_is_plausible_when_present`
  — **live** integration test on the `RTX 5060 Ti`: when `Some`, the
  string is non-empty and contains at least one digit. `#[ignore]`-gated
  like the other live-GPU tests; run with `cargo test -- --ignored`. Live
  result on the reference card: `driver_version = Some("610.88")` —
  the exact post-update version from the dogfooding report's own
  near-miss story, confirming the NVML system-level query and the
  `nvidia-smi` CSV column agree.
- `cargo run --bin hmn` / `-- --json` captured real output on the
  reference machine (used verbatim in the CHANGELOG and README rather
  than invented numbers):
  ```text
  GPU 0 [NVIDIA GeForce RTX 5060 Ti]: free 14274 MiB / 16311 MiB (259 MiB reserved), driver 610.88
  ```
  ```json
  [{"index":0,"name":"NVIDIA GeForce RTX 5060 Ti","total_bytes":17103323136,"free_bytes":14967820288,"used_bytes":2135502848,"reserved_bytes":271581184,"driver_version":"610.88"}]
  ```

---

## Downstream payoff

Once `driver_version` lands, `candle-mi`'s `resurrect.ps1` reads it from
`hmn --json` (rather than shelling out to `nvidia-smi` separately, given
`hmn` is already invoked throughout that script) and stamps it into
`RESURRECTION.md` next to the toolchain line. A future re-run that lands
on a different driver becomes visible in the provenance record itself,
instead of silently certifying a configuration the machine no longer
runs.

---

## References

- NVML `nvmlSystemGetDriverVersion`: <https://docs.nvidia.com/deploy/nvml-api/group__nvmlSystemQueries.html>
