# Wire metal into GPU dispatchers

**Goal**: Extend `src/gpu/mod.rs` so the four public dispatchers (`device_count`, `device_info`, `process_gpu_info`, `gpu_processes`) route to `metal::*` on macOS, and add the `Metal` variant to the `GpuQuerySource` enum.
**Pre-conditions**:
- [ ] `gpu_metal.md` is `done` — `src/gpu/metal.rs` exists with all four `pub(crate) fn` surfaces implemented
- [ ] `ram_darwin.md` is `done` — `cargo build --target aarch64-apple-darwin --features metal` succeeds
- [ ] `cargo_features.md` is `done` — `Cargo.toml` has `metal = []` in `default`
**Success Gates**:
- ⬜ [static] `GpuQuerySource` (in `src/gpu/mod.rs`) has a `Metal` variant that is always present in the enum (matching the existing `Dxgi`/`Nvml`/`NvidiaSmi` pattern, see `tests/smoke.rs::public_types_are_reachable_via_crate_root` at [tests/smoke.rs](../../../tests/smoke.rs))
- ⬜ [static] Each of the four dispatchers contains a `if let Some(...) = metal::fn() { return ... }` block, gated on `cfg(all(target_os = "macos", feature = "metal"))`, placed FIRST in the fall-through chain on macOS
- ⬜ [run] `cargo build --target aarch64-apple-darwin --features metal` succeeds
- ⬜ [run] `cargo build --target x86_64-pc-windows-msvc` succeeds (no regression)
- ⬜ [run] `cargo build --target x86_64-unknown-linux-gnu` succeeds (no regression)
- ⬜ [run] `cargo clippy --all-targets --features metal --target aarch64-apple-darwin -- -D warnings` is clean
**References**: [R03 §Modify — src/gpu/mod.rs](/Users/hacker/.claude/plans/reports-macos-ledger-gives-you-the-effervescent-river.md), [src/gpu/mod.rs](../../../src/gpu/mod.rs) for the existing dispatcher pattern

## Step 1: Add `Metal` variant to `GpuQuerySource` enum

**Goal**: Make `GpuQuerySource::Metal` reachable from the crate root on every platform, so the existing `public_types_are_reachable_via_crate_root` smoke test pattern can be extended to cover it.
**Implementation Logic**:
Find the `GpuQuerySource` enum declaration in `src/gpu/mod.rs` (it sits near the dispatchers and contains `Dxgi`, `Nvml`, `NvidiaSmi`). Add a `Metal` variant with the same derives. The variant is NOT `cfg`-gated — it is always in the enum, mirroring how `Dxgi` is always in the enum even on Linux. Then extend `tests/smoke.rs::public_types_are_reachable_via_crate_root` to instantiate `GpuQuerySource::Metal` alongside the existing variants.
**Deliverables**: `src/gpu/mod.rs` — adds `Metal` variant to `GpuQuerySource` (always present on every platform, matching the existing `Dxgi`/`Nvml`/`NvidiaSmi` pattern); `tests/smoke.rs` — extends `public_types_are_reachable_via_crate_root` to instantiate `GpuQuerySource::Metal`
**Consistency Checks**: `cargo test --target aarch64-apple-darwin --test smoke public_types_are_reachable_via_crate_root && cargo test --target x86_64-unknown-linux-gnu --test smoke public_types_are_reachable_via_crate_root` (expected: PASS)
**Commit**: `feat(gpu): add Metal variant to GpuQuerySource`

## Step 2: Route the four dispatchers to `metal::*` on macOS

**Goal**: Make `device_count`, `device_info`, `process_gpu_info`, and `gpu_processes` consult the `metal` backend first when the platform is macOS and the feature is enabled, falling back to the existing NVML / nvidia-smi chain.
**Implementation Logic**:
In `src/gpu/mod.rs`, locate each of the four dispatcher functions: `device_count` (around line 39), `device_info` (around line 82), `process_gpu_info` (around line 145), `gpu_processes` (around line 276). At the top of each fall-through chain, add a `#[cfg(all(target_os = "macos", feature = "metal"))] { if let Some(result) = metal::fn(...) { return Ok(result); } }` block. The macOS arm is the priority-0 path on macOS — analogous to how DXGI is priority-0 on Windows for the per-process dispatcher. Set `source = GpuQuerySource::Metal` and `is_per_process = true` where the existing patterns set their respective source/per-process flags. **`device_info` mapping**: the macOS arm builds `GpuDeviceInfo { total_bytes, free_bytes, used_bytes, name }` from `MetalQueryResult`'s `dedicated_video_memory`, `current_usage`, and `adapter_name`, with `free_bytes = total_bytes.saturating_sub(used_bytes)` — mirroring the DXGI-alone branch already present at [src/gpu/mod.rs:101-110](../../../src/gpu/mod.rs).
**Deliverables**: `src/gpu/mod.rs` — adds four `#[cfg(all(target_os = "macos", feature = "metal"))]` dispatcher arms (one per public dispatcher), feeding `metal::*` return values into the existing `GpuQueryResult` / `ProcessGpuInfo` / `Vec<GpuProcessEntry>` contracts
**Consistency Checks**: `cargo build --target aarch64-apple-darwin --features metal && cargo build --target x86_64-pc-windows-msvc && cargo build --target x86_64-unknown-linux-gnu` (expected: PASS)
**Commit**: `feat(gpu): route dispatchers to metal backend on macOS`
