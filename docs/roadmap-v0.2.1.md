# `hypomnesis` v0.2.1 — roadmap

> *Sharper, not wider. Same surface — easier to test against, kinder to repeat callers.*

---

## Why v0.2.1 (and not 0.3.0 or a stack of git tags)

Every item in this roadmap is **additive and patch-safe** under the `#[non_exhaustive]` policy already in place. No public type changes shape, no existing method changes signature, no default feature flips. The release exists to absorb wear-and-tear feedback from `hypomnesis`'s first real downstream consumer (`hf-fetch-model 0.10.1`, see [docs/hypomnesis-adoption.md](hypomnesis-adoption.md)) before a second consumer (`candle-mi 0.2`) arrives.

**0.2.1 is the right vehicle, not 0.3.0.** Three small code additions and two doc passes do not warrant a minor bump:

1. No new artifact (unlike 0.2.0's `hmn` binary).
2. No structural API shape change (unlike 0.2.0's `Snapshot::all`).
3. Every new public item is either feature-gated default-off or a thin convenience method on an existing type.

If pre-release review surfaces a controversial item, it can be peeled out into 0.2.2 rather than blocking the polished bits.

---

## Scope

Five waves, ordered code-first then docs (impact descending within each group). Three code waves (additive API surface), two doc waves (no compiled artifact). The maintainer's existing hardware (Windows + Ryzen 9 5950X + RTX 5060 Ti; Ubuntu WSL2 + RTX 5060 Ti) covers verification — no wave touches a backend, so no new hardware path is exercised.

### Wave A — `test-helpers` feature + `GpuDeviceInfo::builder()`

**Motivation.** `GpuDeviceInfo` is `#[non_exhaustive]` and has no constructor, so downstream test fixtures cannot construct it from struct-literal syntax. `hf-fetch-model 0.10.1` filed this as the loudest adoption finding — three planned unit tests in `src/gpu_check.rs` (fit-hit JSON, fit-miss JSON, render-path) could not be written and were replaced by a comment block and manual smoke tests. The same gap will block `candle-mi`'s render-side tests when it adopts.

**Why a feature-gated builder, not a `pub const fn synthetic(...)` or `#[doc(hidden)] pub fn`.**

- A positional `synthetic(index, name, total, free, used)` constructor *partially defeats* `#[non_exhaustive]`. When `temperature_celsius` lands in a future release, the constructor's signature shifts and every downstream test fixture breaks — precisely what `#[non_exhaustive]` was supposed to prevent. A builder absorbs new fields with new defaulted setters.
- `#[doc(hidden)] pub` is hidden-but-still-public for semver: hiding from docs does not exempt it from the API contract. A Cargo feature gate is the standard idiom for "public-but-clearly-not-the-real-API" surface.

**API additions:**

```rust
// Behind feature = "test-helpers" — default-off, additive.
#[cfg(feature = "test-helpers")]
impl GpuDeviceInfo {
    /// Start a builder for constructing synthetic `GpuDeviceInfo` values
    /// in downstream tests. Not intended for production code.
    pub fn builder() -> GpuDeviceInfoBuilder { ... }
}

#[cfg(feature = "test-helpers")]
pub struct GpuDeviceInfoBuilder { /* private fields */ }

#[cfg(feature = "test-helpers")]
impl GpuDeviceInfoBuilder {
    pub fn index(self, index: u32) -> Self { ... }
    pub fn name(self, name: Option<String>) -> Self { ... }
    pub fn total_bytes(self, total: u64) -> Self { ... }
    pub fn free_bytes(self, free: u64) -> Self { ... }
    pub fn used_bytes(self, used: u64) -> Self { ... }
    pub fn build(self) -> GpuDeviceInfo { ... }
}
```

Default values: `index = 0`, `name = None`, `total_bytes = 0`, `free_bytes = 0`, `used_bytes = 0`. The defaults make the builder a one-line synthesis (`GpuDeviceInfo::builder().total_bytes(16 * 1_024 * 1_024 * 1_024).build()`) for tests that only care about one field.

**Verify against the current struct layout before sketching.** Lesson carried forward from Wave A of v0.2.0 — grep the `pub` surface for `builder` / `GpuDeviceInfoBuilder` / `build` before the wave starts, in case a method by that name already exists.

**Cargo.toml addition:**

```toml
[features]
test-helpers = []          # default-off — downstream test fixtures only
```

**Documentation.** The `#[cfg(feature = "test-helpers")]` items need module-level documentation calling out that:

- The feature is intended for downstream test code (`[dev-dependencies] hypomnesis = { version = "0.2", features = ["test-helpers"] }`).
- The builder is **not** semver-stable in the production sense — adding setters when `GpuDeviceInfo` gains fields is expected and does not warrant a major bump.
- Production code should never enable the feature.

**Effort.** Half a day. Builder type + 5 setter methods + 1 `build()` + 4-6 unit tests (one for each setter, one for `build()` with all defaults, one for `build()` round-tripping all fields).

### Wave B — `GpuDeviceInfo::name_or_unknown(&self) -> &str`

**Motivation.** Every consumer that renders the device name will write the same `unwrap_or("...")` fallback. `hf-fetch-model 0.10.1` did at [gpu_check.rs:200](../../hf-fetch-model/src/gpu_check.rs#L200). `candle-mi 0.2` will too. Without an upstream nudge, the two consumers will land on different phrases (`"unknown GPU"` vs `"Unknown"` vs `"<unknown>"`) and the ecosystem accumulates drift.

The wear-and-tear concern is **not** keystroke savings — it's one line at the call site. The concern is consumer divergence on the fallback phrase.

**API addition:**

```rust
impl GpuDeviceInfo {
    /// Adapter name, or `"unknown GPU"` when [`Self::name`] is `None`.
    ///
    /// Convenience wrapper for `self.name.as_deref().unwrap_or("unknown GPU")`,
    /// added so multiple downstream consumers don't diverge on the fallback
    /// phrase. The returned string is **not** localized — consumers needing
    /// a different phrase or non-English output should match on
    /// [`Self::name`] directly.
    #[must_use]
    pub fn name_or_unknown(&self) -> &str {
        self.name.as_deref().unwrap_or("unknown GPU")
    }
}
```

Always-on (not feature-gated) because it's zero-cost and zero-dependency. Lives on `GpuDeviceInfo` for parity with the existing `format_free` / `print_free` feature-gated methods (Wave A of v0.2.0 established the "methods on the type, not free functions" convention).

**Documentation.** The non-localization caveat must be in the doc-comment — both as honesty about the limitation and as a forward-pointer for the only realistic reason a consumer would skip the helper.

**Effort.** 15 minutes including 2 unit tests (`name = Some(...)` returns the inner; `name = None` returns `"unknown GPU"`).

### Wave C — `format_total` / `format_used` parity for the `report` feature

**Motivation.** `hypomnesis 0.2` ships `GpuDeviceInfo::format_free` / `print_free` under the `report` feature. Consumers that want the matching `total` and `used` summaries — `candle-mi 0.2` is the immediate one — will hand-roll the same MiB-conversion-plus-format three times each.

**Honest caveat:** This wave does **not** retroactively serve `hf-fetch-model 0.10.1`. `format_free` is opinionated — `"  GPU {idx}: free {N} MB / {T} MB[ [name]]\n"` (two-space indent, MiB-displayed-as-MB for parity with `ram_mb`/`vram_mb`, trailing newline). `hf-fm` prints with its own `format_size` in GiB, no indent, and a column-aligned `"GPU N:"` prefix that does not match `format_free`'s shape. The wave lands for `report`-feature consumers (candle-mi and future), not for `hf-fm`.

**Option (a) chosen over (b) / (c).** Three options were on the table in [hypomnesis-adoption.md §2](hypomnesis-adoption.md):

- **(a)** `format_total` / `format_used` parity helpers — closest in style to `format_free`, smallest API surface.
- **(b)** `format_summary` returning a one-liner — most opinionated, locks the format and the field order.
- **(c)** `format_free_used_total` returning a tuple — most flexible for callers who template their own output.

**(a)** wins for patch-release scope: it's a strict parity extension of an existing feature, with no new format to defend in code review. **(b)** and **(c)** wait for a second `report`-feature consumer to validate the desired shape — see *Out of scope* below.

**API additions:**

```rust
#[cfg(feature = "report")]
impl GpuDeviceInfo {
    /// Format a one-line total-`VRAM` summary as an owned `String` ending in a newline.
    ///
    /// Format: `  GPU <idx>: total <T> MB[ [<adapter name>]]\n`.
    /// Mirrors [`Self::format_free`]'s style exactly; the adapter-name
    /// suffix is omitted when [`Self::name`] is `None`. `MB` here means
    /// `MiB` (`bytes / 1_048_576`).
    #[must_use]
    pub fn format_total(&self) -> String { ... }

    /// Format a one-line used-`VRAM` summary as an owned `String` ending in a newline.
    ///
    /// Format: `  GPU <idx>: used <U> MB[ [<adapter name>]]\n`.
    /// Style and unit conventions identical to [`Self::format_total`].
    #[must_use]
    pub fn format_used(&self) -> String { ... }
}
```

No `print_total` / `print_used` companions in this wave. `print_free` exists because v0.2.0's Wave A had a real consumer use case for direct stdout output (LM-Studio-style headroom log line); `total` and `used` do not have an equivalent motivating consumer yet. Add them when one asks.

**Documentation.** Update the `## Feature flags` section of `lib.rs` and the `## Feature Flags` table in `README.md` to mention the new helpers under the existing `report` entry.

**Effort.** 1 hour. Two methods + 6 unit tests (name-present / name-absent / fully-allocated edge for each of `format_total` and `format_used`, matching the existing `format_free` test suite at [snapshot.rs:428-483](../src/snapshot.rs#L428-L483)).

### Wave D — `HypomnesisError` `Display` vs structured-fields contract (doc-only)

**Motivation.** `hf-fetch-model 0.10.1` chose to render error variants by matching on the variant and formatting from structured fields (`DeviceIndexOutOfRange { index, count }`) rather than by using the `Display` impl directly — specifically to fix the `"1 devices"` → `"1 device"` plural agreement. The choice is correct but undocumented: the next consumer will re-discover it from scratch, and a future hypomnesis change that "improves" `Display` (e.g., adding the count again to a variant that already exposed it) could create double-printed redundancies in downstream output.

**Doc addition.** Append to the `HypomnesisError` module-level doc-comment in [src/error.rs](../src/error.rs):

```text
## Display vs structured fields

`HypomnesisError`'s `Display` impl is the **default English one-liner** —
suitable for logs, library-tier error reporting, and `?`-propagation
where the consumer is content with the default rendering. Structured
fields (`DeviceIndexOutOfRange { index, count }`, the inner `String`
of `Nvml` / `Dxgi` / `NvidiaSmi`) are the **canonical source** for any
consumer that wants to:

- Localize the message to a non-English language.
- Restyle for a CLI / GUI / JSON output.
- Apply singular/plural agreement, custom punctuation, or richer formatting.

This contract makes `Display` stable for the common case while leaving
custom-render consumers free to assemble their own strings without
fighting the default. Consumers writing user-facing tools should
prefer `match err { E::DeviceIndexOutOfRange { index, count } => ... }`
over `format!("{err}")`.
```

The wording explicitly names the use cases (localization, restyle, plural agreement) so the contract is testable: any future change that breaks one of those use cases is a contract violation, not a debatable refactor.

**No code changes.** The existing `#[error(...)]` strings are correct and remain unchanged.

**Effort.** 30 minutes including a once-over of every existing variant's `#[error(...)]` string to confirm none of them is so consumer-unfriendly that the doc note is overpromising.

### Wave E — `docs/hypomnesis-brief.md` correction + `README.md` "Used by"

**Motivation.** Two stale facts in user-facing docs now that `hf-fetch-model 0.10.1` has shipped:

1. **The brief overestimates `hf-fm`'s surface usage.** [hypomnesis-brief.md](hypomnesis-brief.md) (around line 137) says *"hf-fm uses ~10% of hypomnesis's API surface (`device_info` + `device_count`)."* Actual v0.10.1 usage is **only `device_info`** — `device_count` is not called because `--check-gpu N` targets a single device and the out-of-range path is handled by `device_info`'s `DeviceIndexOutOfRange` variant carrying the count for free.
2. **The `README.md` "Used by" section still says "No consumers yet."** With `hf-fm 0.10.1` shipped, the section should match the convention used in `hf-fm`'s own README (`- [candle-mi](...) — Mechanistic interpretability toolkit for language models`).

**Brief edit** (`docs/hypomnesis-brief.md`):

Replace the *"hf-fm uses ~10% of hypomnesis's API surface (`device_info` + `device_count`)"* sentence with:

```text
hf-fm uses `device_info` directly (well under 10% of hypomnesis's API
surface). `device_count` is deferred to the multi-GPU follow-up
(`--check-gpu all`, hf-fm v0.10.4) — `device_info`'s
`DeviceIndexOutOfRange { index, count }` variant already exposes the
count whenever an out-of-range index is queried, so the v0.10.1 single-
device path does not need a separate count call.
```

**README edit** (`README.md`):

Replace the current `## Used by` body with:

```markdown
## Used by

- [hf-fetch-model](https://github.com/PCfVW/hf-fetch-model) — Hugging Face model weights and metadata fetcher (uses `device_info` for `inspect --check-gpu`)

_Forthcoming: [candle-mi](https://github.com/PCfVW/candle-mi) is expected to migrate its in-tree memory module to `hypomnesis` (`features = ["report"]`) after v0.2.1 lands._
```

Format follows the `hf-fetch-model` README convention (`- [name](url) — purpose`), with a single parenthetical noting which API surface the consumer touches. The italics "Forthcoming" sentence preserves the `candle-mi` forward-reference that the original "No consumers yet" body carried, without lying about current state.

**Effort.** 15 minutes total. Pure text edits, reviewable in one pass.

---

## Out of scope for v0.2.1 (carrying the discipline forward)

| Idea | Why deferred |
|---|---|
| `format_summary(&self) -> String` (`hypomnesis-adoption.md` finding #2 option (b)) | Opinionated one-liner — wait for a second `report`-feature consumer to validate the shape and the field order. candle-mi's adoption will be the natural moment. |
| `format_free_used_total(&self) -> (String, String, String)` (finding #2 option (c)) | Tuple-of-three return is over-engineered for the current consumer count. Revisit only if a templating consumer surfaces. |
| `print_total` / `print_used` companions to Wave C's `format_*` | `print_free` exists because of a real LM-Studio-style log-line consumer; no equivalent ask for total/used yet. Pure parity-for-parity's-sake is a code-bloat trap. |
| Localization of `name_or_unknown`'s fallback string | Baking `"unknown GPU"` into the library is documented as English-only; consumers wanting other languages match on `name` directly. A real internationalization story belongs in a v0.3+ release with `unic-langid` or equivalent, not in a patch. |
| Long-lived NVML context (the "planned for v0.2" note that slipped) | Real perf work — needs design ([snapshot.rs:207](../src/snapshot.rs#L207) defers it to "a later release"). Patch releases should not introduce performance-sensitive infrastructure. v0.3 or later. |
| Constructors for `ProcessGpuInfo`, `Snapshot`, `GpuProcessEntry` under `test-helpers` | Wave A only adds the `GpuDeviceInfo` builder because that is the type currently blocking downstream tests. Other `#[non_exhaustive]` types can grow builders the same way when a downstream actually asks. |
| AMD ROCm backend, Apple Metal backend | Carried over from v0.2.0 — still no maintainer hardware access. |
| Peak / high-water-mark tracking | Carried over from v0.2.0 — still no benchmark-loop consumer asking. |
| `hmn` CLI surface changes (new subcommands, new flags) | The CLI just shipped in v0.2.0. Patch release is too early to widen its surface; let real users exercise the existing shape first. |

---

## Verification plan

All five waves are additive and touch no backend code, so no new hardware path is exercised. The verification surface is:

- **Wave A** — `cargo test --features test-helpers` exercises the builder unit tests on Windows + WSL2. `cargo test` (no extra features) confirms the builder is invisible to default builds. `cargo build` and `cargo doc` with `--all-features` confirm there is no symbol collision with existing items.
- **Wave B** — `cargo test` covers the two unit tests. No feature gating.
- **Wave C** — `cargo test --features report` covers the six unit tests, matching the existing `format_free` test pattern at [snapshot.rs:428-483](../src/snapshot.rs#L428-L483).
- **Wave D** — `cargo doc --all-features` renders the appended doc paragraph; visual review on docs.rs after publish confirms it lands cleanly.
- **Wave E** — text-only edits; markdown lint clean, no broken links (`README.md` and `hypomnesis-brief.md` links checked manually).

**Live-hardware tests** (`tests/live_gpu.rs`) remain untouched — Wave A's builder is for offline fixtures, not hardware mocking. The existing live-test suite continues to validate the real backends.

**Publish flow** unchanged from v0.1.0 / v0.2.0 — commit → dry-run CI → push → watch → `cargo publish --dry-run` → tag → watch → verify, per `reference_publish_flow.md` in the Claude Code project memory.

---

## After v0.2.1 (gestures only — not commitments)

These are likely candidates for v0.3.0+ if a real consumer or hardware path surfaces:

- **`format_summary` / `format_free_used_total`** — promote one of the deferred Wave C options when a second `report`-feature consumer asks.
- **Builders for `ProcessGpuInfo`, `Snapshot`, `GpuProcessEntry`** under `test-helpers` — add per type as downstream tests demand.
- **Long-lived NVML context** — perf work, would land alongside any other performance-sensitive infrastructure for v0.3.
- **AMD ROCm backend, Apple Metal backend** — still gated on hardware access.
- **`HypomnesisError` localization helpers** — only if a real CLI / GUI consumer is doing non-English rendering.

`#[non_exhaustive]` keeps every one of these additive — none requires a 1.0 bump.

---

## Decisions settled (this roadmap, 2026-05-12)

1. **0.2.1 patch, not 0.3.0** — every item is additive; no minor bump warranted.
2. **`test-helpers` feature gate over `#[doc(hidden)] pub fn`** — feature gate is the honest idiom for "public-but-not-the-real-API." `#[doc(hidden)]` is still public for semver.
3. **Builder over positional `synthetic(...)` constructor** — a positional constructor partially defeats `#[non_exhaustive]`. Builder absorbs new fields via new defaulted setters.
4. **Option (a) `format_total` / `format_used` over (b) `format_summary` and (c) tuple-return** — parity extension of the existing `format_free`; smallest API surface to defend in review.
5. **`"unknown GPU"` as the `name_or_unknown` fallback, documented as English-only** — settles consumer divergence (`hf-fm` vs `candle-mi` would otherwise pick different phrases), defers localization to v0.3+.
6. **`Display` = default English one-liner, structured fields = canonical source** — the contract is doc-only and matches the convention every error-rich library converges on.
7. **All five waves in one 0.2.1 release** — bundling reduces release overhead; any controversial item can be peeled into 0.2.2 in review.
8. **No `print_total` / `print_used` companions** — wait for a real motivating consumer, the way `print_free` had one.

---

## One crate, one job — still

> *Tell you what's currently in this process's memory, precisely, across Windows and Linux.*

v0.2.1 stays inside that motto. It does not widen the matrix, add a backend, or change the answer to the central question — it polishes the way callers ask and the way consumers test against the answer.
