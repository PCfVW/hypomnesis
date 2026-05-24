# Create src/gpu/metal.rs

**Goal**: Implement the macOS GPU backend module: libSystem-only FFI for `task_info` / `ledger` / `sysctlbyname` / `proc_listpids` / `proc_pidpath`, plus the four public functions (`query`, `device_count`, `process_gpu_info`, `list_compute_processes`) that the dispatchers in `src/gpu/mod.rs` will call.
**Pre-conditions**:
- [ ] `dep_audit.md` is `done`
**Success Gates**:
- ⬜ [static] `src/gpu/metal.rs` exists with module-level `//!` doc citing R01 + R02 once, and explicitly stating: resident-byte semantics, entry-name lookup at init (no hardcoded index)
- ⬜ [static] `src/gpu/metal.rs` contains a `mod libsystem_ffi` submodule with the five required `unsafe extern "C"` declarations (`getpid`, `ledger`, `sysctlbyname`, `proc_listpids`, `proc_pidpath`)
- ⬜ [static] `src/gpu/mod.rs` includes the line `#[cfg(all(target_os = "macos", feature = "metal"))] mod metal;`
- ⬜ [static] Every `unsafe` block has a `// SAFETY:` comment; every numeric cast has a `// CAST:` annotation; every direct slice index has an `// INDEX:` annotation, per [CONVENTIONS.md §Annotation rules]
- ⬜ [static] `GRAPHICS_FOOTPRINT_INDEX` is a `OnceLock<i32>` resolved via `LEDGER_TEMPLATE_INFO` by name match on `b"graphics_footprint"` — index 36 must NOT appear as a literal in source
- ⬜ [run] `cargo check --target aarch64-apple-darwin --features metal` succeeds (parses, type-checks, links libSystem)
- ⬜ [run] `cargo clippy --target aarch64-apple-darwin --features metal -- -D warnings` is clean
**References**: [R01 §Reference Design](../../__reports__/macos_ledger/09-knowledge_transfer_v3.md), [R02 Appendix A](../../__reports__/macos_ledger/05-findings_writes_v0.md) (Swift+C probe — the C externs map 1:1 to the Rust ones), [src/gpu/dxgi.rs](../../src/gpu/dxgi.rs) and [src/gpu/nvml.rs](../../src/gpu/nvml.rs) as template patterns

## Step 1: Scaffold the module — FFI declarations, constants, structs, and module gate

**Goal**: Create `src/gpu/metal.rs` with the libSystem FFI surface and the `repr(C)` types it needs; declare the module in `src/gpu/mod.rs`. No business logic yet — just the substrate.
**Implementation Logic**:
Create `src/gpu/metal.rs` with: (a) module-level `//!` doc — three short paragraphs: purpose (per-process GPU memory on Apple Silicon UMA), source map (`ledger`, `sysctlbyname`, `task_info`), semantic equivalence (resident bytes — matches Windows `WorkingSetSize` and Linux `VmRSS`), one citation each to [R01] and [R02]. (b) `mod libsystem_ffi` with `unsafe extern "C"` declaring: `getpid() -> i32`, `ledger(cmd: i32, arg1: i32, arg2: *mut c_void, arg3: *mut c_void) -> i32`, `sysctlbyname(name: *const c_char, oldp: *mut c_void, oldlenp: *mut usize, newp: *mut c_void, newlen: usize) -> i32`, `proc_listpids(type_: u32, typeinfo: u32, buffer: *mut c_void, buffersize: i32) -> i32`, `proc_pidpath(pid: i32, buffer: *mut c_void, buffersize: u32) -> i32`. (c) `repr(C)` structs `LedgerTemplateInfo` (fields per `<sys/kern_memorystatus.h>` / `<sys/proc_info.h>` — `lti_name: [c_char; 32]`, `lti_group: [c_char; 32]`, `lti_units: [c_char; 32]`) and `LedgerEntryInfo` (`lei_balance: i64`, `lei_peak: i64`, `lei_credit: i64`, `lei_debit: i64`, `lei_limit: i64`, `lei_refill_period: u64`, `lei_last_refill: u64`, `lei_warn_period: u64`, `lei_last_warn: u64`, `lei_actions: i32`, `lei_flags: i32`). (d) Constants: `LEDGER_INFO = 0x4`, `LEDGER_TEMPLATE_INFO = 0x5`, `LEDGER_ENTRY_INFO_V2 = 0x6` (verify exact values against open-source XNU `osfmk/kern/ledger.h` at the MSRV-compatible tag — values cited in the rustdoc with link). `PROC_ALL_PIDS = 1`. (e) `pub(crate)` empty function signatures `pub(crate) fn query() -> Option<...>`, etc. — bodies in subsequent steps. Then in `src/gpu/mod.rs`, add `#[cfg(all(target_os = "macos", feature = "metal"))] mod metal;` next to the existing module declarations around line 20.
**Deliverables**: `src/gpu/metal.rs` (new) with module `//!` doc, `mod libsystem_ffi` (5 `extern "C"` fn signatures: `getpid`, `ledger`, `sysctlbyname`, `proc_listpids`, `proc_pidpath`), structs `LedgerTemplateInfo` + `LedgerEntryInfo`, constants `LEDGER_INFO` + `LEDGER_TEMPLATE_INFO` + `LEDGER_ENTRY_INFO_V2` + `PROC_ALL_PIDS`, stub fn signatures `query` + `device_count` + `process_gpu_info` + `list_compute_processes`; `src/gpu/mod.rs` (modify) adds `#[cfg(all(target_os = "macos", feature = "metal"))] mod metal;`
**Consistency Checks**: `cargo check --target aarch64-apple-darwin --features metal && cargo clippy --target aarch64-apple-darwin --features metal -- -D warnings` (expected: PASS)
**Commit**: `feat(metal): scaffold src/gpu/metal.rs with libSystem FFI`

## Step 2: Implement `query()` + `device_count()` + entry-index resolution

**Goal**: Wire `device_count` (returns `Some(1)` on Apple Silicon) and `query` (returns `GpuQueryResult` with self-PID `graphics_footprint`, `hw.memsize` total, CPU-brand-string name).
**Implementation Logic**:
Resolve the `graphics_footprint` ledger entry index ONCE at first call via `static GRAPHICS_FOOTPRINT_INDEX: OnceLock<i32>`: call `ledger(LEDGER_INFO, 0, …)` to get the entry count, then `ledger(LEDGER_TEMPLATE_INFO, 0, buf, …)` with a `Vec<LedgerTemplateInfo>` of that size, iterate, find the row whose `lti_name` C-string equals `b"graphics_footprint\0"`. Cache the index. `device_count`: call `sysctlbyname(b"hw.optional.arm64\0", …)` — if it returns 1, return `Some(1)`; else return `None` (Intel Macs aren't supported in v0.2.2). `query`: build a `GpuQueryResult` with `current_usage = read_graphics_footprint(getpid())`, `dedicated_video_memory = read_sysctl_u64(b"hw.memsize\0")`, `adapter_name = read_sysctl_string(b"machdep.cpu.brand_string\0")`. Cite [R02 Table 1] in a one-line code comment near the `graphics_footprint` read to anchor the semantic.
**Deliverables**: `src/gpu/metal.rs` (modify) — implements `static GRAPHICS_FOOTPRINT_INDEX: OnceLock<i32>`, helpers `resolve_graphics_footprint_index`, `read_graphics_footprint(pid)`, `read_sysctl_u64`, `read_sysctl_string`, `pub(crate) fn device_count() -> Option<u32>`, `pub(crate) fn query(idx: u32) -> Option<MetalQueryResult>` (shape mirrors `DxgiQueryResult` at src/gpu/dxgi.rs)
**Consistency Checks**: `cargo build --target aarch64-apple-darwin --features metal && cargo clippy --target aarch64-apple-darwin --features metal -- -D warnings` (expected: PASS)
**Commit**: `feat(metal): implement device_count and query via sysctl + ledger`

## Step 3: Implement `process_gpu_info()` for self-PID

**Goal**: Return per-process GPU info for the calling process — the self-PID surface that mirrors Windows DXGI's `process_gpu_info` at [src/gpu/dxgi.rs] and the Linux NVML self-PID path.
**Implementation Logic**:
`process_gpu_info(device_index: u32) -> Option<ProcessGpuInfo>` calls `getpid()`, then `read_graphics_footprint(self_pid)`, then constructs a `ProcessGpuInfo` with `current_usage = footprint`, `dedicated_video_memory = read_sysctl_u64(b"hw.memsize\0")`, `is_per_process = true`, `source = GpuQuerySource::Metal`. The `Metal` variant of `GpuQuerySource` is added in the dispatcher wiring leaf — this step uses the (not-yet-existing) variant; commit ordering at depth 3 fixes the cross-reference. (The `cfg`-gated `mod metal;` means this file is only compiled when the variant exists, after the dispatcher wiring commit.)
**Deliverables**: `src/gpu/metal.rs` (modify) — implements `pub(crate) fn process_gpu_info(device_index: u32) -> Option<ProcessGpuInfo>` reading self-PID `graphics_footprint`
**Consistency Checks**: `grep -q 'fn process_gpu_info' src/gpu/metal.rs && cargo check --target aarch64-apple-darwin --features metal` (expected: PASS)
**Commit**: `feat(metal): implement process_gpu_info via ledger graphics_footprint`

## Step 4: Implement `list_compute_processes()` via `proc_listpids` + per-PID `ledger`

**Goal**: Enumerate every same-user PID whose `graphics_footprint > 0` and return a `Vec<GpuProcessEntry>` — parity with the Linux NVML `nvmlDeviceGetComputeRunningProcesses_v3` path.
**Implementation Logic**:
Call `proc_listpids(PROC_ALL_PIDS, 0, null, 0)` to get the buffer-size hint, allocate a `Vec<i32>` of that size / 4, call again to fill. Iterate PIDs; for each, call `read_graphics_footprint(pid)`. If the syscall returns EPERM (cross-user without root), skip silently. If `footprint > 0`, call `proc_pidpath(pid, buf, …)` to get the executable path; extract the basename for the `name` field. Return `Vec<GpuProcessEntry>` (struct already exists in `src/gpu/mod.rs`). Cap the iteration at the buffer size to handle PID-count growth between the two `proc_listpids` calls.
**Deliverables**: `src/gpu/metal.rs` (modify) — implements `pub(crate) fn list_compute_processes(device_index: u32) -> Option<Vec<GpuProcessEntry>>` enumerating same-user PIDs via `proc_listpids` and reading per-PID `graphics_footprint` via `ledger`, with `proc_pidpath` for the `name` field; cross-user EPERM rows silently skipped
**Consistency Checks**: `cargo check --target aarch64-apple-darwin --features metal && cargo clippy --target aarch64-apple-darwin --features metal -- -D warnings` (expected: PASS)
**Commit**: `feat(metal): implement list_compute_processes via proc_listpids + ledger`
