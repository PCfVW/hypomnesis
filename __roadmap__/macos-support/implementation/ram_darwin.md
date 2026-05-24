# Add macOS RAM path

**Goal**: Extend `src/ram.rs` with a `darwin_ffi` submodule and a `macos_rss()` function so `process_rss()` returns the calling process's `phys_footprint` on macOS — semantically equivalent to Windows `WorkingSetSize` and Linux `VmRSS`.
**Pre-conditions**:
- [ ] `dep_audit.md` is `done`
**Success Gates**:
- ⬜ [static] `src/ram.rs` contains a `mod darwin_ffi` submodule with an `unsafe extern "C"` block declaring `task_info` and `mach_task_self`
- ⬜ [static] `src/ram.rs::process_rss()` has a `#[cfg(target_os = "macos")]` arm returning a `u64` from `macos_rss()`
- ⬜ [static] Every `unsafe` block in the new code carries a `// SAFETY:` comment per [CONVENTIONS.md §FFI Patterns]
- ⬜ [static] Every numeric cast carries a `// CAST:` annotation
- ⬜ [run] `cargo test --target aarch64-apple-darwin process_rss` passes the existing `process_rss_returns_positive` smoke test
- ⬜ [behavioral] Writing a 256 MiB `Vec<u8>` and re-reading `process_rss()` shows a delta of +256 MiB ± 1 page (16 KiB) — verified in `verification/cli_output_verifier.md`
**References**: [R01 §Reference Design](../../__reports__/macos_ledger/09-knowledge_transfer_v3.md), [src/ram.rs:42](../../src/ram.rs) for the `win_ffi` template pattern to mirror

## Step 1: Add `darwin_ffi` submodule and `macos_rss()` function; wire into `process_rss()` dispatcher

**Goal**: Implement the macOS branch of the existing RAM dispatcher using `task_info(TASK_VM_INFO_PURGEABLE).phys_footprint`.
**Implementation Logic**:
Mirror the `win_ffi` pattern at `src/ram.rs:42`. Declare a private `mod darwin_ffi` submodule containing: (a) the `task_vm_info_data_t` `#[repr(C)]` struct laid out per `<mach/task_info.h>` — important fields: `phys_footprint: u64` near the end; (b) a `pub const TASK_VM_INFO_PURGEABLE: u32 = 22;` constant; (c) the `task_vm_info_purgeable_count: u32` constant equal to `std::mem::size_of::<task_vm_info_data_t>() / std::mem::size_of::<u32>()`; (d) an `unsafe extern "C"` block declaring `mach_task_self() -> u32` (safe per Apple's docs — always returns the calling task port) and `task_info(target_task: u32, flavor: u32, task_info_out: *mut u32, task_info_outCnt: *mut u32) -> i32`. Then a `fn macos_rss() -> Result<u64, HypomnesisError>` that: stack-allocates the struct zero-initialised, calls `task_info` with `TASK_VM_INFO_PURGEABLE`, checks the return code (`KERN_SUCCESS == 0`), returns `phys_footprint`. Extend the dispatcher at `src/ram.rs:21` with a `#[cfg(target_os = "macos")] { macos_rss() }` arm. Update the module-level `//!` doc to mention the macOS source.
**Deliverables**: `src/ram.rs` — adds `mod darwin_ffi` with `struct TaskVmInfo`, `const TASK_VM_INFO_PURGEABLE`, `const TASK_VM_INFO_PURGEABLE_COUNT`, `extern "C" fn mach_task_self`, `extern "C" fn task_info`; module-level `fn macos_rss() -> Result<u64, HypomnesisError>`; `#[cfg(target_os = "macos")]` arm in `process_rss()`; module `//!` doc updated to mention `task_info(TASK_VM_INFO_PURGEABLE).phys_footprint`
**Consistency Checks**: `cargo test --target aarch64-apple-darwin --test smoke process_rss_returns_positive` (expected: PASS)
**Commit**: `feat(ram): add macOS phys_footprint via task_info`
