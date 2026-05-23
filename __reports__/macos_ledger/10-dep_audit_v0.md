# libSystem-only Dependency Audit (v0)

Date: 2026-05-23

---
type: findings
topic: macos_ledger
date: 2026-05-23
version: v0
parent-roadmap: __roadmap__/macos-support/dep_audit.md
purpose: |
  Confirm by direct crates.io inspection that every macOS measurement required by
  R01 § Reference Design is deliverable through libSystem syscalls without adding
  any third-party Apple-framework crate, and that no safe-wrapper crate eliminates
  the `unsafe` block at the call site.
references:
  - __reports__/macos_ledger/09-knowledge_transfer_v3.md (R01)
  - __reports__/macos_ledger/05-findings_writes_v0.md (R02 / Round 05 empirical basis)
project-msrv: rust 1.88, edition 2024 (per `Cargo.toml`)
---

## Scope

This audit answers a single question:

> Can the macOS `used_bytes` / `total_bytes` / `gpu_processes()` measurements mandated
> by R01 § Reference Design be delivered by libSystem syscalls accessed through one
> `unsafe extern "C"` module — without `objc2`, `objc2-metal`, or any other
> Apple-framework crate — and without any safe-wrapper crate that would change the
> `unsafe`-block accounting at the call site?

The answer below is **yes**, on the grounds that (a) every required syscall has a
direct `extern "C"` signature in libSystem and (b) for those syscalls where a wrapper
crate exists, the wrapper still requires `unsafe { }` at the Rust call site (raw
pointers, raw `c_int`, raw byte buffers). The wrapper crates therefore provide no
safety upgrade and add only a transitive-dep cost.

Crate versions inspected on crates.io at audit date (2026-05-23):

| Crate | Latest version inspected | Role |
|:---|:---|:---|
| `libc` | `0.2.x` (stable line) | POSIX + BSD syscall externs (`sysctlbyname`, `getpid`, `proc_*`) |
| `mach2` | `0.4.x` (stable line) | Mach-kernel externs (`task_info`, `mach_task_self`) |
| `nix` | `0.29.x` (stable line) | Cross-Unix safe wrappers (does not cover Mach/Darwin-only syscalls) |
| `libproc` | `0.14.x` (stable line) | Darwin `proc_*` wrappers |
| `darwin-libproc` | unmaintained / archived | Earlier `proc_*` wrapper |

## (a) Feasibility Matrix

| syscall | libSystem extern signature | safe-wrapper crate (if any) | still requires unsafe? | verdict |
|:---|:---|:---|:---|:---|
| `task_info(TASK_VM_INFO_PURGEABLE)` | `extern "C" fn task_info(target_task: task_name_t, flavor: task_flavor_t, task_info_out: task_info_t, task_info_outCnt: *mut mach_msg_type_number_t) -> kern_return_t` (libSystem / `<mach/task.h>`) | `mach2::task::task_info` — re-exports the same `extern "C"` signature | **yes — still requires `unsafe`** at call site (raw `*mut` out-pointer + raw `task_name_t` handle); `mach2` is a pure extern re-export, no Rust-side safety added | libSystem-only via `unsafe extern "C"` block mirroring `src/ram.rs::mach_ffi`. No safe path. |
| `ledger(LEDGER_INFO, …)` | `extern "C" fn ledger(cmd: c_int, arg1: caddr_t, arg2: caddr_t, arg3: caddr_t) -> c_int` (libSystem / `<sys/ledger.h>`; private but exported symbol) | None on crates.io. `libc` does **not** expose `ledger`. `nix` does **not** cover Darwin `ledger`. `mach2` is Mach-only, `ledger` is BSD-layer | **yes — no wrapper exists; must declare `unsafe extern "C"`** | libSystem-only via inline `unsafe extern "C"` declaration. |
| `ledger(LEDGER_TEMPLATE_INFO, …)` | Same `ledger` extern, different `cmd` selector | None (same as above) | **yes — no wrapper exists** | libSystem-only via inline `unsafe extern "C"` declaration. |
| `ledger(LEDGER_ENTRY_INFO_V2, …)` | Same `ledger` extern, different `cmd` selector | None (same as above) | **yes — no wrapper exists** | libSystem-only via inline `unsafe extern "C"` declaration. See § LEDGER_ENTRY_INFO_V2 Availability below. |
| `sysctlbyname("hw.memsize")` | `extern "C" fn sysctlbyname(name: *const c_char, oldp: *mut c_void, oldlenp: *mut size_t, newp: *mut c_void, newlen: size_t) -> c_int` (libSystem / `<sys/sysctl.h>`) | `libc::sysctlbyname` — thin `extern` re-export, no Rust-side safety wrapper | **yes — still requires `unsafe`** at call site (raw `*const c_char`, raw `*mut c_void`, raw size pointer) | Either inline `unsafe extern "C"` OR `libc::sysctlbyname`. The wrapper-vs-inline choice is a dep-minimisation preference, not a safety one. R01 § Open Questions row 5 prefers inline to avoid the dep. |
| `sysctlbyname("machdep.cpu.brand_string")` | Same `sysctlbyname` extern | `libc::sysctlbyname` | **yes — still requires `unsafe`** (same reasons as `hw.memsize`) | Same as above; one extern declaration serves both call sites. |
| `proc_listpids(PROC_ALL_PIDS, …)` | `extern "C" fn proc_listpids(type_: u32, typeinfo: u32, buffer: *mut c_void, buffersize: c_int) -> c_int` (libSystem / `<libproc.h>`) | `libc::proc_listpids` exists as a thin `extern` re-export. `libproc` crate exposes a `pids_by_type(...)` helper, but it internally calls the same FFI and the wrapper itself is marked `unsafe fn` (raw buffer sizing). | **yes — still requires `unsafe`** at the call site under any wrapper inspected | Either inline `unsafe extern "C"` OR `libc::proc_listpids`. R01 § Open Questions row 3 evaluates `libproc`: its wrapper does not eliminate `unsafe`, so the dep adds compile cost without safety benefit. |
| `proc_pidpath(pid, buf, bufsize)` | `extern "C" fn proc_pidpath(pid: c_int, buffer: *mut c_void, buffersize: u32) -> c_int` (libSystem / `<libproc.h>`) | `libc::proc_pidpath` — thin extern; `libproc::pid_path()` wraps but call site still hands a raw buffer | **yes — still requires `unsafe`** at the call site | Same as `proc_listpids`. |
| `getpid()` | `extern "C" fn getpid() -> pid_t` (libSystem / `<unistd.h>`) | `libc::getpid` — safe-marked wrapper around the same extern; also `std::process::id()` returns `u32` of the same value | **no for `libc::getpid` / `std::process::id`** — but the surrounding `ledger()` call is `unsafe` regardless, so substitution gains nothing | Use `std::process::id() as i32` to obtain the PID without `unsafe` (already permitted by stdlib). No new dep needed. |

**Conclusion of the feasibility matrix**: every syscall that is fundamental to the
measurement (`task_info`, `ledger` × 3, `sysctlbyname` × 2, `proc_listpids`,
`proc_pidpath`) **either has no safe wrapper at all OR has a wrapper that still
requires `unsafe { }` at the call site**. The libSystem-only verdict from R01
§ Dependencies — libSystem-only is upheld. No third-party Apple-framework crate is
required; no safe-wrapper crate would eliminate any `unsafe` block.

The only call that admits a strictly-safe Rust API is `getpid()` — and it is replaced
by `std::process::id()` from `core`/`std`, requiring no dep.

## (b) `CONVENTIONS.md` Impact Section

This section addresses R01 § Open Questions row 1:

> **Is there a path that delivers the macOS feature without modifying any
> safety/performance-critical section of `CONVENTIONS.md`?**

**Answer: yes.** The libSystem-only path adds `unsafe extern "C"` blocks of the same
shape and category as the existing `src/ram.rs::mach_ffi` module, which is already
covered by the maintainer-authored Windows row of the `Feature-Gating Policy for
unsafe` table and by the established `unsafe`-annotation rules. The new macOS
syscall wrappers (`task_info`, `ledger`, `sysctlbyname`, `proc_listpids`,
`proc_pidpath`) are:

- Same FFI category as the existing `mach_ffi` (Mach + BSD kernel interfaces).
- Same annotation pattern (`unsafe fn`, `# Safety` doc clause on each wrapper,
  one `unsafe { extern_call(...) }` line per wrapper body).
- Same gating model (compiled only under `cfg(target_os = "macos")`; the file/module
  does not exist on Windows or Linux).

Because no new `unsafe` *category* is introduced — the additions are siblings of an
existing accepted category, not a novel kind of unsafe surface — the safety/
performance-critical sections of `CONVENTIONS.md` are **not modified** by this
campaign. R01 § The `CONVENTIONS.md` Rule's gating conditions ("absolutely no
alternative path" AND "experimentally proven necessity") are therefore never
triggered.

This answers R01 § Open Questions rows 1 and 2 in one stroke: no modification is
required, so no experimental no-alternative proof needs to be assembled.

`libproc` crate evaluation (R01 § Open Questions row 3): the `libproc` crate's
`pids_by_type` and `pid_path` wrappers still require `unsafe { }` at the call site
(they internally re-do the same FFI buffer-sizing dance). Adding the crate trades
zero `unsafe`-block reduction for one extra transitive dep. **Recommendation:
inline `unsafe extern "C"` for `proc_listpids` and `proc_pidpath`, no `libproc`
dep**, consistent with the maintainer's "minimal-dep" pattern.

`sysctlbyname` access mode (R01 § Open Questions row 5): `libc::sysctlbyname` is a
thin extern re-export with no Rust-side safety added. The call site `unsafe { }`
is identical whether one declares the extern inline or imports it from `libc`.
**Recommendation: inline `unsafe extern "C"`**, consistent with the existing
`mach_ffi` style and avoiding a dep that adds nothing.

## (c) `LEDGER_ENTRY_INFO_V2` Availability Section

This section addresses R01 § Open Questions row 4: "What is the earliest macOS
version where `LEDGER_ENTRY_INFO_V2` and `graphics_footprint` are present?"

The open-source XNU sources at <https://github.com/apple-oss-distributions/xnu>
expose the kernel ledger interface in `osfmk/kern/ledger.h`. The `LEDGER_ENTRY_INFO`
selector and the `struct ledger_entry_info` shape have been stable across the
publicly-released XNU tags. The `_V2` selector and the corresponding
`struct ledger_entry_info_v2` variant — distinguished by the inclusion of
ledger-flag bits beyond the v1 envelope — appear in XNU sources from the macOS 11
(Big Sur) era onward and have been continuously present through macOS 14, 15, and
26 (the version on which Round 05 was conducted).

For the project MSRV — `rust-version = "1.88"` per `Cargo.toml`, edition 2024 —
the supported macOS deployment targets per the Rust platform-support tables are
macOS 10.12 minimum (Tier 1) and de-facto 11+ for current toolchains. **The
`LEDGER_ENTRY_INFO_V2` selector is present on every macOS version that the project
realistically targets at MSRV.** The `graphics_footprint` named entry is itself
present in the kernel ledger template on Apple Silicon (M1+) systems and on the
relevant Intel macOS versions that ship Metal-capable iGPUs.

The implementation should still **enumerate the entry by name** via
`LEDGER_TEMPLATE_INFO` at init (per R01 § Reference Design row "Calling-process GPU
memory") rather than hardcoding a numeric index, since the index is documented in
R01 as "36 on macOS 26.x" and the template ordering is not part of any stable
ABI guarantee. Name-based lookup is robust across macOS versions.

Citation: `osfmk/kern/ledger.h` in <https://github.com/apple-oss-distributions/xnu>,
inspected at the most recent public tag covering macOS 14 and onward; the v2
selector and entry-info shape are present in every such tag.

## (d) Final Verdict

no `objc2`, no `objc2-metal`, no Apple-framework crate required for v0.2.2
