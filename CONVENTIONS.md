# hypomnesis Coding Conventions (Grit + Grit-HMN Extensions)

This document describes the [Amphigraphic coding](https://github.com/PCfVW/Amphigraphic-Strict)
conventions used in `hypomnesis`. It is a superset of the
[Grit — Strict Rust for AI-Assisted Development](https://github.com/PCfVW/Amphigraphic-Strict/tree/master/Grit)
base, with hypomnesis-specific extensions for FFI safety and feature-gated
backends.

The trigger checklist, doc-comment, signature, and `unsafe` rules below are
aligned with [`anamnesis/CONVENTIONS.md`](https://github.com/mi-for-the-rust-of-us/anamnesis/blob/main/CONVENTIONS.md)
and [`candle-mi/CONVENTIONS.md`](https://github.com/mi-for-the-rust-of-us/candle-mi/blob/main/CONVENTIONS.md).
The numeric / SIMD sections of those documents are deliberately omitted
here: hypomnesis is a measurement crate, not a numeric pipeline. The
[FFI patterns](#ffi-patterns) and [feature-gated backends](#feature-gated-backends)
sections are unique to hypomnesis.

## Trigger Checklist

**Before writing any line of code, check which triggers apply.**

| You are about to... | Check these rules |
|---|---|
| Write a `///` or `//!` comment | [Backtick hygiene](#backtick-hygiene), [field-level docs](#field-level-docs), [intra-doc link safety](#intra-doc-link-safety) |
| Write a `pub fn` or `pub const fn` | [`const fn`](#const-fn), [`#[must_use]`](#must_use-policy), [pass by value](#pass-by-value-vs-reference) |
| Write a `pub fn` returning `Result<T>` | [`# Errors` section](#errors-doc-section) |
| Write a `pub enum` or `pub struct` | [`#[non_exhaustive]`](#non_exhaustive-policy) or [`// EXHAUSTIVE:`](#exhaustive-annotation) |
| Write an `as` cast | [`// CAST:`](#cast-annotation) |
| Write `slice[i]` or `slice[a..b]` | [`// INDEX:`](#index-annotation) |
| Write `.as_str()`, `.to_owned()` | [`// BORROW:`](#borrow-annotation) |
| Write an `unsafe` block | [`// SAFETY:`](#safety-annotation), [feature-gating](#feature-gating-policy-for-unsafe), [FFI patterns](#ffi-patterns) |
| Write `Box<dyn T>` or `&dyn T` | [`// TRAIT_OBJECT:`](#trait_object-annotation) |
| Write a `match` or `if let` | [`if let` vs `match`](#if-let-vs-match), [`// EXPLICIT:`](#explicit-annotation) if no-op arm |
| Write error strings | [Error message wording](#error-message-wording) |
| Add `#[allow(clippy::...)]` for a newer lint | [MSRV lint guard](#msrv-lint-guard) |
| Add a new GPU backend (Metal, ROCm, ...) | [Feature-gated backends](#feature-gated-backends) |

---

## When Writing Doc Comments (`///`, `//!`)

### Backtick Hygiene

All identifiers, types, trait names, field names, crate names, and
file-format names in doc comments must be wrapped in backticks so that
rustdoc renders them as inline code and Clippy's `doc_markdown` lint passes.

Applies to: struct/enum/field names, method names (`fn foo`), types
(`Vec<T>`, `Option<u64>`), crate names (`libloading`, `windows`, `thiserror`),
shared-library names (`nvml.dll`, `libnvidia-ml.so.1`), and acronyms that
double as types (`DXGI`, `NVML`, `WDDM`, `RSS`, `VRAM`, `IDXGIAdapter3`).

> ✅ `` /// Calls `IDXGIAdapter3::QueryVideoMemoryInfo` to read per-process VRAM. ``
> ❌ `/// Calls IDXGIAdapter3::QueryVideoMemoryInfo to read per-process VRAM.`

### Intra-Doc Link Safety

Rustdoc intra-doc links must resolve under all feature-flag combinations.
Feature-gated items (e.g., `MemoryReport` behind the `report` feature) must
use plain backtick text in cross-references from feature-independent
modules, not link syntax:

> ✅ `` /// See `MemoryReport` (requires `report` feature). ``
> ❌ `` /// See [`MemoryReport`](crate::report::MemoryReport). ``

Within a feature-gated module, intra-doc links to other items in the same
module are safe. Cross-module links between feature-gated areas are safe
when the linked target is in the same feature set as the linker.

### Field-Level Docs

Every field of every `pub` struct must carry a `///` doc comment describing:

1. what the field represents,
2. its unit (bytes, MiB, milliseconds) or valid range where applicable,
3. when the field can be `None` or zero (for `Option`-typed and accumulator fields).

> Example:
> ```rust
> pub struct GpuDeviceInfo {
>     /// Zero-based GPU index (NVML-canonical ordering on Windows).
>     pub index: u32,
>     /// Adapter name (e.g., `NVIDIA GeForce RTX 5060 Ti`).
>     /// `None` when the source backend does not provide it.
>     pub name: Option<String>,
>     /// Total GPU memory in bytes.
>     pub total_bytes: u64,
> }
> ```

### `# Errors` Doc Section

All public fallible methods (`-> Result<T>`) must include an `# Errors`
section. Each bullet uses the format:

    /// # Errors
    /// Returns [`HypomnesisError::Nvml`] if NVML fails to load or report a count.
    /// Returns [`HypomnesisError::DeviceIndexOutOfRange`] if `index` exceeds the device count.

Rules:

- Start each bullet with `Returns` followed by the variant in rustdoc link
  syntax: `` [`HypomnesisError::Variant`] ``.
- Follow with `if` (condition), `on` (event), or `when` (circumstance).
- One bullet per distinct error path.

---

## When Writing Function Signatures

### `const fn`

Declare a function `const fn` when **all** of the following hold:

1. The body contains no heap allocation, I/O, or `dyn` dispatch.
2. All called functions are themselves `const fn`.
3. There are no trait-method calls that are not yet `const`.

Constructors of plain data structs (`MemoryReport::new`), accessors, and
pure arithmetic helpers should be `const fn`. When in doubt, annotate and
let the compiler reject it — do not omit `const` preemptively.

> ✅ `pub const fn new(before: Snapshot, after: Snapshot) -> Self { ... }`

### `#[must_use]` Policy

All public functions and methods that return a value and have no side
effects must be annotated `#[must_use]`. This includes constructors,
accessors, and pure queries.

`Result<T>` is already `#[must_use]` at the type level; explicit annotation
on `pub fn -> Result<T>` is redundant but not wrong. The
`clippy::must_use_candidate` lint is at `warn`.

> ✅ `#[must_use] pub fn ram_delta_mb(&self) -> f64 { ... }`

### Pass by Value vs Reference

| Type | Rule |
|---|---|
| `Copy` type ≤ 2 words (`u32`, `u64`, `bool`, `GpuQuerySource`) | Pass by value |
| `Copy` type > 2 words | Pass by reference |
| Non-`Copy`, not mutated (`String`, `Snapshot`) | Pass by `&T` |
| Non-`Copy`, mutated | Pass by `&mut T` |
| Owned, consumed by callee | Pass by value (move semantics) |

Never accept `&mut T` when the body never writes through the reference;
Clippy's `needless_pass_by_ref_mut` will flag it. Similarly,
`trivially_copy_pass_by_ref` flags `&T` where `T: Copy` and is small
enough to pass by value.

---

## When Writing Public Enums and Structs

### `#[non_exhaustive]` Policy

Future-proofing rests on `#[non_exhaustive]`, not on parameter type elaboration.

- **Public enums** that may gain new variants: `#[non_exhaustive]`.
  `HypomnesisError`, `GpuQuerySource` are non-exhaustive — new error
  paths and new GPU backends will land in patch releases.
- **Public structs** that may gain new fields: `#[non_exhaustive]`.
  `GpuDeviceInfo`, `ProcessGpuInfo`, `Snapshot`, `MemoryReport` are
  non-exhaustive — fields like `temperature_celsius`, `pcie_link_gen`,
  or per-segment VRAM (local vs non-local) may be added.
- **Internal dispatch enums** matched exhaustively by this crate:
  `#[allow(clippy::exhaustive_enums)] // EXHAUSTIVE: <reason>`.

Once a type is `#[non_exhaustive]`, downstream callers cannot construct
it via struct literal syntax. Provide a constructor (`Snapshot::now`,
`MemoryReport::new`) for every public type that callers need to build.

---

## When Writing Expressions

These annotations are required **on or immediately before** the line where
the pattern occurs. Apply them as you write the line, not in a review pass.

### CAST Annotation

`// CAST: <from> → <to>, <reason>` — required on every `as` cast between
numeric types. Prefer `From`/`Into` for lossless conversions and
`TryFrom`/`TryInto` with `?` for fallible ones. Use `as` only when
truncation or wrapping is the deliberate intent, or when the bit-level
reinterpretation is the whole point.

> Example: `// CAST: u64 → f64, byte count for MiB conversion (fits in f64 mantissa for any process)`
> Example: `// CAST: usize → u32, struct size for K32GetProcessMemoryInfo cb field (fixed 80 bytes on x64)`

### INDEX Annotation

`// INDEX: <reason>` — required on every direct slice index (`slice[i]`,
`slice[a..b]`) that cannot be replaced by an iterator. Direct indexing
panics on out-of-bounds; prefer `.get(i)` with `?` or explicit error
handling.

> Example: `// INDEX: nvml_infos buffer is sized to NVML_MAX_PROCESSES; count is bounded above`

### BORROW Annotation

`// BORROW: <what is converted>` — required on explicit `.as_str()`,
`.as_bytes()`, `.to_owned()` conversions (Grit Rule 2).

> Example: `// BORROW: explicit String::from_utf16_lossy — DXGI Description is fixed-size UTF-16 array`

### TRAIT_OBJECT Annotation

`// TRAIT_OBJECT: <reason>` — required on every `Box<dyn Trait>` or
`&dyn Trait` usage. hypomnesis avoids dynamic dispatch where possible;
backends are selected by feature flag and `#[cfg]`, not by trait objects.

---

## When Writing `unsafe`

hypomnesis carries `#![deny(unsafe_code)]` at the crate root. `forbid` is
not appropriate because the crate fundamentally relies on three distinct
FFI surfaces (NVML dynamic load, DXGI COM, `K32GetProcessMemoryInfo`
extern) — `unsafe` is intrinsic to the work.

Every `unsafe` block must be **scoped, annotated, and feature-gated**.

### SAFETY Annotation

`// SAFETY: <invariants>` — required on every `unsafe` block or
`unsafe fn` (inline comment, not a doc comment). Document:

1. What invariants the call requires (e.g., "buffer is at least N bytes",
   "handle is valid for the lifetime of the process").
2. How those invariants are established at this call site.

> Example:
> ```rust
> // SAFETY: K32GetProcessMemoryInfo writes into the stack-allocated counters
> // struct (cb field set to its size). GetCurrentProcess returns a pseudo-handle
> // that is always valid for the lifetime of the process.
> let ok = unsafe { K32GetProcessMemoryInfo(handle, &raw mut counters, cb) };
> ```

### Feature-Gating Policy for `unsafe`

| Feature gate | Accepted `unsafe` scope |
|---|---|
| (always, on `target_os = "windows"`) | `K32GetProcessMemoryInfo` extern via `unsafe extern "system"` block in `src/ram.rs` |
| (always, on `target_os = "macos"`) | libSystem syscalls (`task_info`, `ledger`, `sysctl`, `proc_listpids`, `proc_pidpath`) in `src/ram.rs` + `src/gpu/metal.rs` |
| `nvml` | NVML dynamic load and FFI in `src/gpu/nvml.rs` |
| `dxgi` | DXGI COM calls in `src/gpu/dxgi.rs` (Windows-only) |
| `pdh` | PDH counter API (`PdhOpenQueryW` / `PdhEnumObjectItemsW` / `PdhAddCounterW` / `PdhCollectQueryData` / `PdhGetFormattedCounterValue`) for the `GPU Process Memory` and `GPU Adapter Memory` counter sets, plus `OpenProcess` + `QueryFullProcessImageNameW` name lookup, in `src/gpu/pdh.rs` (Windows-only). The v0.2.5 spill module (`src/spill.rs`) contains **no** `unsafe` of its own — it delegates to this backend. |
| `metal` | `objc2-metal` `MTLDevice` calls in `src/gpu/metal.rs` (macOS-only) |

Each accepted use must satisfy all of:

1. The `unsafe` block lives in a **single, dedicated module** — never
   scattered across the codebase.
2. Every `unsafe` block carries a `// SAFETY:` comment.
3. The module is gated behind `#[cfg(feature = "...")]` — users who
   don't enable the feature get zero unsafe code from that backend.
4. A safe Linux equivalent exists where the platform makes one possible
   (Linux RAM uses `/proc/self/status` with no unsafe; the Windows RAM
   path is the only platform where `unsafe` is unavoidable).

Adding a new backend requires updating this table and adding the
appropriate `#[cfg(feature = "...")]` to the new module's parent
declaration in `src/gpu/mod.rs`.

### FFI Patterns

Three FFI patterns recur in hypomnesis. Each has its own conventions.

**Pattern 1 — `unsafe extern "system"` block (Windows ABI).**
Used for stable Windows API functions whose ABI is well-known
(`K32GetProcessMemoryInfo`, `GetCurrentProcess`). The block declares the
function signatures inline. Each declared function gets a doc comment
linking to the Microsoft documentation page.

**Pattern 2 — `libloading::Library::new` + symbol lookup (NVML).**
Used for NVML, which is loaded at runtime so the crate works even when
the NVIDIA driver is absent. `unsafe` is required at three points:
library load, symbol lookup, and call. Conventions:

- Hold the `Library` for the duration of all calls; don't load and
  re-load per query.
- Initialize (`nvmlInit_v2`) and shut down (`nvmlShutdown`) in matched
  pairs — on every return path, including error paths.
- Treat `nvmlReturn_t` constants (`NVML_SUCCESS = 0`,
  `NVML_ERROR_INSUFFICIENT_SIZE = 7`) as named consts, not magic numbers.

**Pattern 3 — `windows` crate COM (`IDXGIFactory1` → `IDXGIAdapter3`).**
Used for DXGI on Windows. The `windows` crate handles refcounting via
`Drop`, so manual `Release` is not needed. Conventions:

- Use `.cast::<T>()` for safe COM interface upcasting; check the
  `Result` and surface failure as `HypomnesisError::Dxgi`.
- Do not store COM pointers across function boundaries; they are cheap
  to re-acquire and have non-trivial thread-safety considerations.
- Prefer `&raw mut foo` over `&mut foo as *mut _` for taking pointers
  to be passed into FFI (Rust 2024 edition idiom).

### MSRV Lint Guard

`lib.rs` carries `#![allow(unknown_lints)]` so that
`#[allow(clippy::newer_lint)]` in test modules does not break the MSRV
CI build. Without it, every new clippy lint suppression risks an MSRV
failure under future `#![deny(warnings)]`.

No special action is required when adding a new `#[allow(clippy::...)]`.
If MSRV is bumped, this guard remains necessary as long as MSRV trails
the development toolchain.

---

## When Writing Control Flow

### `if let` vs `match`

Use the most specific construct for the pattern at hand:

| Situation | Preferred form |
|---|---|
| Testing a single variant, no binding needed | `matches!(expr, Pat)` |
| Testing a single variant, binding needed | `if let Pat(x) = expr { … }` |
| Two or more variants with different bodies | `match expr { … }` |
| Exhaustive dispatch over an enum | `match expr { … }` |

Never `match` with a single non-`_` arm and a no-op `_ => {}` where
`if let` or `matches!` would be clearer.

### EXPLICIT Annotation

`// EXPLICIT: <reason>` — required when a match arm is intentionally a
no-op, or when an imperative loop is used instead of an iterator chain
for a stateful computation.

> Example: `// EXPLICIT: NVML init failed; nothing to clean up before returning None`

### EXHAUSTIVE Annotation

`// EXHAUSTIVE: <reason>` — required on `#[allow(clippy::exhaustive_enums)]`
or `#[allow(clippy::wildcard_enum_match_arm)]` when a wildcard is used on
a foreign `#[non_exhaustive]` enum that we cannot match exhaustively.

> Example: `// EXHAUSTIVE: HypomnesisError is internal; this dispatcher owns and matches all variants`

---

## When Writing Error Strings

### Error Message Wording

Error strings passed to `HypomnesisError` variants follow two patterns:

- **External failures** (FFI, subprocess, I/O): `"failed to <verb>: {e}"`
  > Example: `HypomnesisError::Nvml(format!("failed to load nvml: {e}"))`
  > Example: `HypomnesisError::NvidiaSmi(format!("failed to spawn nvidia-smi: {e}"))`
- **Validation failures** (sentinel value, out-of-range): `"<noun> <problem> (<context>)"`
  > Example: `HypomnesisError::Nvml(format!("nvmlDeviceGetMemoryInfo returned NVML_ERROR ({code})"))`
  > Example: `HypomnesisError::Dxgi("no DXGI adapter with non-zero dedicated VRAM found".into())`

Rules:

- Use lowercase, no trailing period.
- Include the offending value and the valid range when applicable.
- Wrap external errors with `: {e}`, not `.to_string()`.
- For NVML return codes, name the code (`NVML_ERROR_*`) where possible;
  fall back to the numeric value when the code is unknown to us.

---

## Feature-Gated Backends

hypomnesis's GPU backends are independent units of code, each gated by a
Cargo feature:

| Feature | Module | Since | Adds dep |
|---|---|---|---|
| `nvml` | `src/gpu/nvml.rs` | v0.1 | `libloading` |
| `dxgi` | `src/gpu/dxgi.rs` (Windows only) | v0.1 | `windows` |
| `pdh` | `src/gpu/pdh.rs` (Windows only; also feeds the always-compiled `src/spill.rs`) | v0.2.2 (adapter counters v0.2.5) | `windows` (shared with `dxgi`) |
| `metal` | `src/gpu/metal.rs` (macOS only) | v0.2.3 | `objc2-metal`, `objc2` |
| `nvidia-smi-fallback` | `src/gpu/nvidia_smi.rs` | v0.1 | none (stdlib) |
| `report` | `src/report.rs` | v0.1 | none |
| `debug-output` | (cross-cutting) | v0.1 | none |
| `test-helpers` | (builders in `src/snapshot.rs`, `src/spill.rs`) | v0.2.1 | none |
| `cli` | `src/bin/hmn.rs` | v0.2.0 | `clap` |
| `rocm` (future) | `src/gpu/rocm.rs` | — | TBD |

Adding a new backend (for a new GPU vendor or new measurement source) is
a five-step recipe:

1. Add the feature to `Cargo.toml` `[features]` and any optional dependency.
2. Create `src/gpu/<backend>.rs` with module-level `//!` docs explaining
   the FFI mechanism, what it measures, and platform constraints.
3. Add `#[cfg(feature = "<backend>")] pub mod <backend>;` (and a
   `cfg(target_os)` guard if platform-specific) to `src/gpu/mod.rs`.
4. Wire the backend into the dispatchers (`device_info`, `device_count`,
   `process_gpu_info`) in priority order. Backends try in sequence;
   first success wins.
5. Add a new variant to `GpuQuerySource` (the enum is `#[non_exhaustive]`,
   so this is non-breaking) and update the smoke tests.

Default features (`nvml`, `nvidia-smi-fallback`, `dxgi`) cover the
ecosystem's most common case (NVIDIA on Windows or Linux). Disabling
defaults yields a nearly empty crate that compiles to almost nothing —
useful for callers that only want process RSS.
