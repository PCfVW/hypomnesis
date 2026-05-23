# macOS GPU + RAM Measurement — Knowledge Transfer (v3)

Date: 2026-05-22

---
type: knowledge-transfer
topic: macos_ledger
date: 2026-05-22
version: v3
prior-version: 08-knowledge_transfer_v2.md
revision-reason: |
  (1) Drop `objc2-metal` and `objc2` dependencies entirely — the libSystem-only path
  (`sysctl` + `ledger` + `task_info`) delivers every required measurement, removes the
  dep-identification pain experienced in the prior cycle, and mirrors the Linux
  pattern of "no third-party FFI crate, just a thin `unsafe extern "C"` module."
  (2) Replace `MTLDevice.recommendedMaxWorkingSetSize` (dynamic OS budget; inconsistent
  with the maintainer's cross-platform fixed-capacity contract for `total_bytes`) with
  `sysctl hw.memsize` (fixed system DRAM; semantically consistent with Windows
  `DedicatedVideoMemory` + Linux `nvmlDeviceGetMemoryInfo.total`).
  (3) Update the target-release framing from "v0.3.0" to "v0.2.2" per the maintainer's
  direct guidance. The PR does not modify `Cargo.toml`'s version field; the maintainer
  applies the bump after PR acceptance.
purpose: hand off to a fresh implementation cycle starting from `main` (v0.2.1) in a new worktree
audience: the next implementer (human or agent) tasked with adding macOS RAM + GPU measurement to `hypomnesis`
---

## Executive Summary

- **Decision**: the in-progress `claude/compassionate-moore-709120` integration branch is abandoned. The next implementation cycle restarts from `main` (v0.2.1) in a fresh worktree. This report is the single hand-off artifact.
- **Target release**: **v0.2.2** (per maintainer directive). The PR does NOT modify `Cargo.toml`'s `version = "0.2.1"` field. The maintainer applies the version bump after accepting the PR. Discussions, commit-message bodies, and PR text use the "v0.2.2" framing for anchoring; the source-of-truth version edit is a maintainer-only operation.
- **Mandated macOS GPU `used_bytes` metric**: `ledger(LEDGER_ENTRY_INFO_V2, pid, …).graphics_footprint`. Not `MTLDevice.currentAllocatedSize`. Justified by the cross-platform-consistency contract (Windows `WorkingSetSize`, Linux `VmRSS` — both resident-tracked). See § Why `graphics_footprint`.
- **Mandated macOS GPU `total_bytes` metric**: `sysctl hw.memsize` (fixed system DRAM). Not `MTLDevice.recommendedMaxWorkingSetSize`. Justified by the same cross-platform-consistency contract — Windows `DedicatedVideoMemory` and Linux `nvmlDeviceGetMemoryInfo.total` both report fixed physical capacity, not dynamic budgets.
- **Dependencies**: **no `objc2-metal`, no `objc2`, no third-party Apple framework crates**. The macOS path is libSystem-only — `sysctl`, `ledger`, `task_info` — accessed through a single `unsafe extern "C"` module mirroring the existing `src/ram.rs::mach_ffi` pattern. See § Dependencies.
- **What is empirically established**: macOS per-process RAM and Metal GPU memory CAN be measured for arbitrary same-user PIDs via libSystem syscalls. Cross-user PIDs require root. Per-process Metal allocations DO enter the kernel's `graphics_footprint` ledger entry — once they are written (residency-tracked, not allocator-tracked). See § Empirical Foundations.
- **Net knowledge gained**: the architecture described in § Reference Design is empirically validated, cross-platform-coherent, and dep-free on macOS. The next cycle ships a focused v0.2.2 from a clean worktree.

---

## Target Release: v0.2.2

The maintainer (`PCfVW <ejacopin@ymail.com>`) directly specified v0.2.2 as the release target for this work. Implications for the next cycle:

| Item | Behaviour |
|:---|:---|
| `Cargo.toml` `version` field | **Stays at `"0.2.1"`** during the implementation. The PR does not touch it |
| Discussions, commit bodies, PR title/description | Use "v0.2.2" framing for anchoring (e.g., "Adds macOS support targeting v0.2.2") |
| Version bump itself | Maintainer-only operation, applied after PR acceptance |
| Cross-cutting docs (README, lib.rs `//!`) | Reference "v0.2.x" or "current" rather than naming a specific tag |

This separation keeps version-semantics decisions in the maintainer's hands and prevents the kind of "v0.4 framing drift" that contaminated the prior cycle.

---

## Dependencies — libSystem-only

The macOS path requires no third-party Apple-framework dependencies. Every measurement is achievable via libSystem syscalls accessed through a single `unsafe extern "C"` module mirroring `src/ram.rs::mach_ffi` on `main`.

| Required measurement | Source | Crate dep needed? |
|:---|:---|:---|
| Calling-process RAM | `task_info(TASK_VM_INFO).phys_footprint` | No (same `unsafe extern "C"` pattern as existing `mach_ffi`) |
| Calling-process GPU `used_bytes` | `ledger(LEDGER_ENTRY_INFO_V2, getpid(), …).graphics_footprint` | No (libSystem syscall) |
| Cross-PID GPU `used_bytes` | Same with target PID | No |
| Device `total_bytes` | `sysctlbyname("hw.memsize", …)` | No (libSystem syscall) |
| Device name | `sysctlbyname("machdep.cpu.brand_string", …)` → "Apple M3 Pro" or similar | No |
| Device count | Hardcoded `1` for Apple Silicon | No |
| (Optional) PID enumeration for `gpu_processes()` | `proc_listpids` via libSystem extern, OR the `libproc` crate IF it provides a safe wrapper (see § Open Questions) | At most one safe-wrapper crate, NOT `objc2`-family |

**Concretely: the `metal` Cargo feature on macOS becomes `metal = []`** (no `dep:` lines). The `[target.'cfg(target_os = "macos")'.dependencies]` block in `Cargo.toml` disappears entirely. This was the same dep block that caused the prior cycle's version-identification pain (`objc2 = "0.6"` vs `"0.5"`, `objc2-metal = "0.3"` vs `"0.2"`, the `default-features = false, features = ["MTLDevice"]` incantation). None of it is required.

**This mirrors the maintainer's Linux pattern**: `src/ram.rs` on `main` reads `/proc/self/status` via `std::fs::read_to_string` with no third-party crate dep at all. The macOS analog under this proposal is the existing `unsafe extern "C"` style already in `src/ram.rs::mach_ffi` — one dedicated module, one extern block, zero third-party FFI crates.

The trade-offs of going dep-free (vs keeping `objc2-metal` minimally):

| Trade-off | With libSystem-only | With `objc2-metal` |
|:---|:---|:---|
| Device count on Mac Pro with eGPU (rare) | Wrong (reports 1; could undercount) | Correct |
| Device name string | "Apple M3 Pro" from CPU brand sysctl (identical on Apple Silicon since GPU is on the same die) | "Apple M3 Pro" from `MTLDevice.name` (canonical) |
| Compile time | Smaller | Includes the Apple Objective-C runtime bindings |
| Dep version pain | None | Recurring across `objc2-metal` 0.x releases |

Both trade-offs are display quality, not measurement accuracy. The next implementer can elect to add `objc2-metal` if Mac Pro eGPU support becomes a stated requirement, but the default position should be libSystem-only.

---

## Why `graphics_footprint` — Cross-Platform Consistency

The original maintainer (`PCfVW <ejacopin@ymail.com>`) chose **resident-byte semantics** for both pre-existing platforms. This is not an arbitrary decision; it is the public-API contract of `hypomnesis`.

| Platform | RAM metric | GPU `used_bytes` metric | GPU `total_bytes` metric | Semantics |
|:---|:---|:---|:---|:---|
| Windows | `K32GetProcessMemoryInfo` → **`WorkingSetSize`** | `IDXGIAdapter3::QueryVideoMemoryInfo` → `DXGI_QUERY_VIDEO_MEMORY_INFO.CurrentUsage` | `DXGI_ADAPTER_DESC.DedicatedVideoMemory` | `used`: resident. `total`: fixed physical capacity |
| Linux | `/proc/self/status` → **`VmRSS`** | `nvmlDeviceGetMemoryInfo.used`, `nvmlDeviceGetComputeRunningProcesses_v3.usedGpuMemory` | `nvmlDeviceGetMemoryInfo.total` | `used`: resident. `total`: fixed physical capacity |
| macOS (the only consistent choice) | `task_info(TASK_VM_INFO)` → **`phys_footprint`** | `ledger(…, pid, graphics_footprint)` | **`sysctl hw.memsize`** | `used`: resident. `total`: fixed physical capacity (system DRAM, which on UMA IS the GPU memory pool) |

The original code is explicit about the resident-bytes choice. `src/ram.rs:387` on `main` literally calls `line.strip_prefix("VmRSS:")`, bypassing the available `VmSize` (virtual reservation) on the same file. The deliberateness is documented in the module-level `//!` comment: "exact, per-process." The same deliberateness must carry to macOS.

The macos-silicon campaign shipped:
- `MTLDevice.currentAllocatedSize` for `used_bytes` → allocator-tracked (virtual reservation). Inconsistent: would report 256 MiB for a never-written allocation while Windows/Linux peers report ~0.
- `MTLDevice.recommendedMaxWorkingSetSize` for `total_bytes` → dynamic OS budget (varies under memory pressure). Inconsistent: Windows + Linux peers report static physical capacity.

Both choices silently re-defined the public-API contract. The corrected v3 path (`graphics_footprint` + `hw.memsize`) restores it.

Round 05 demonstrated the residency-tracking behavior directly:

| Buffer state | `currentAllocatedSize` reports | `graphics_footprint` reports | Equivalent on Windows / Linux |
|:---|:---|:---|:---|
| Allocated 256 MiB, never written | 256 MiB (lies — no physical pages backing it) | 0 (no resident pages) | A `VirtualAlloc(MEM_RESERVE)` region (Windows) or anonymous `mmap` (Linux) with no faults — reported as 0 in `WorkingSetSize` and `VmRSS`. Match |
| Allocated 256 MiB, wrote every byte | 256 MiB | 256 MiB exactly | A committed + touched allocation — reported as 256 MiB in `WorkingSetSize` and `VmRSS`. Match |

`graphics_footprint` is the mandated `used_bytes` metric, not a preferred one. `hw.memsize` is the mandated `total_bytes` metric on the same grounds.

---

## The `CONVENTIONS.md` Rule (Project Discipline)

`CONVENTIONS.md`'s safety/performance-critical sections — the `Feature-Gating Policy for unsafe` table, the `unsafe`-annotation rules, the numeric/SIMD discipline if applicable to a future feature — are **immutable by default**. They may be modified ONLY when:

1. There is **absolutely no alternative path** that achieves the same engineering result without modifying them, AND
2. The necessity is **experimentally proven** and the proof is attached to the PR.

The default first question is not "what should the new policy row say?" — it is "is there a path that does not require modifying these sections at all?" If a no-modification path exists, take it, even if it is more code to write.

A modification to `CONVENTIONS.md` is the **exception**, not the registration step for new code. Treating it as "documentation to update when you add new unsafe" is the failure mode that this cycle demonstrated. The correct framing: the rules in `CONVENTIONS.md` define the engineering envelope; the implementation must fit inside the envelope by default, and the envelope is only expanded when the implementation has proven that fitting inside it is impossible.

---

## Reference Design (Empirically Validated, Cross-Platform-Coherent, Dep-Free)

The next implementer should build this. Every claim below is backed by Round 05 (`__reports__/macos_ledger/05-findings_writes_v0.md`) and re-verifiable in ~30 minutes by re-running the probe.

| Measurement | macOS source | Cross-platform peer | Notes |
|:---|:---|:---|:---|
| Calling-process RAM (resident) | `task_info(mach_task_self_, TASK_VM_INFO, …).phys_footprint` | Windows `WorkingSetSize`, Linux `VmRSS` | All three are resident-page counts. Already exists in `src/ram.rs::mach_ffi` on the integration branch; technique is correct |
| Calling-process GPU memory (resident) | `ledger(LEDGER_ENTRY_INFO_V2, getpid(), &arg, …)` with template entry named `graphics_footprint` (index 36 on macOS 26.x; **enumerate by name** via `LEDGER_TEMPLATE_INFO` at init, do not hardcode) | Windows `DXGI_QUERY_VIDEO_MEMORY_INFO.CurrentUsage`, Linux `nvmlDeviceGetMemoryInfo.used` | All three report what is actually committed to GPU-attributed physical memory right now |
| Any same-user process's GPU memory | Same as above with target PID instead of `getpid()` | Linux `nvmlDeviceGetComputeRunningProcesses_v3` per-process entry | Takes a **bare PID**; no `task_for_pid` needed. Verified empirically |
| Any same-user process's RAM | Same syscall, template entry `phys_footprint` | Linux `/proc/<pid>/status` `VmRSS` | Same-user PIDs work without `sudo` |
| Cross-user processes (e.g. `WindowServer`) | Same syscall, requires `sudo` or `com.apple.security.cs.debugger` entitlement | Same constraint exists on Linux (`/proc/<pid>/status` of other users) | Out of scope for the unprivileged default path; degrade gracefully |
| Device `total_bytes` (fixed physical capacity) | `sysctlbyname("hw.memsize", …)` → system DRAM | Windows `DXGI_ADAPTER_DESC.DedicatedVideoMemory`, Linux `nvmlDeviceGetMemoryInfo.total` | All three are fixed physical capacity available to the GPU. On UMA this is system DRAM because the GPU has no separate memory pool |
| Device name | `sysctlbyname("machdep.cpu.brand_string", …)` → "Apple M3 Pro" | Windows `DXGI_ADAPTER_DESC.Description`, Linux `nvmlDeviceGetName` | On Apple Silicon the CPU brand string identifies the GPU because they share the die |
| Device count | Hardcoded `1` for Apple Silicon | Windows DXGI adapter enumeration, Linux NVML device-count | Multi-GPU Mac Pro + eGPU is out of scope for libSystem-only path; revisit if/when stated requirement |

What is **NOT** the right source on macOS (and why, anchored to the cross-platform contract):

| Anti-source | Why it's wrong |
|:---|:---|
| `MTLDevice.currentAllocatedSize` for `used_bytes` | Allocator-tracked (virtual reservation), not resident-tracked. Inconsistent with Windows `WorkingSetSize` and Linux `VmRSS` — those report resident bytes |
| `MTLDevice.recommendedMaxWorkingSetSize` for `total_bytes` | Dynamic OS-managed cap; varies under memory pressure. Inconsistent with Windows `DedicatedVideoMemory` and Linux `nvmlDeviceGetMemoryInfo.total` — those report fixed physical capacity. Also requires `objc2-metal` which the libSystem-only path eliminates |
| `task_for_pid()` for cross-PID work | Not needed. `ledger(LEDGER_ENTRY_INFO_V2, pid, …)` takes a bare PID |
| `MetalNoCrossPidSupport` error variant | Premised on a false claim that cross-PID Metal accounting doesn't exist on macOS. It does, via the kernel ledger |
| `objc2-metal` / `objc2` Cargo deps | Not required. Every measurement achievable via libSystem syscalls; see § Dependencies |

---

## Empirical Foundations

The single empirical artifact worth preserving is **Round 05** (`__reports__/macos_ledger/05-findings_writes_v0.md`). Headline measurement on Apple M3 Pro, macOS 26.3.1:

| Phase transition | `graphics_footprint` delta | `MTLDevice.currentAllocatedSize` delta | `phys_footprint` delta |
|:---|:---|:---|:---|
| Allocate 256 MiB `MTLBuffer` (no write) | 0 | +256 MiB | ~0 |
| Write every byte (`for i in 0..<len { ptr[i] = … }`) | **+256 MiB exactly** | 0 | +256.1 MiB |
| `didModifyRange` | 0 | 0 | 0 |
| Private-mode + blit copy | +328 MiB | counted | +328 MiB |

Two independent kernel-side read paths agreed numerically: the BSD `ledger()` syscall and the Mach `task_info(…REV3).ledger_tag_graphics_footprint` field returned identical values. The probe source (Swift + a C bridging header) is preserved verbatim in Round 05 Appendix A.

The 256 MiB write was a simple per-byte assignment loop (`ptr[i] = UInt8(i & 0xff)`). No memset, no compute kernel — the simplest possible write to force page residency.

The residency-vs-allocator divergence empirically demonstrates that `graphics_footprint` behaves on Apple Silicon UMA the way `WorkingSetSize` and `VmRSS` behave on their respective platforms. The choice of `graphics_footprint` for `used_bytes` is forced by this consistency requirement.

---

## Wins

- **Empirical methodology, once corrected**: the writes-corrected probe (Round 05) settled the residency question with measurable phase-by-phase deltas, two independent read paths, and a clean verifier audit. Reusable as a regression test.
- **Cross-PID without `task_for_pid`**: `ledger()` takes a bare PID for same-user reads, removing the entitlement requirement that was being assumed earlier.
- **Three orthogonal accounting universes named**: `currentAllocatedSize` (Metal allocator, virtual), `graphics_footprint` (kernel ledger, resident), `phys_footprint` (task physical footprint, resident, includes Metal pages on UMA once written).
- **Cross-platform contract anchored on both `used_bytes` and `total_bytes`**: the original maintainer's resident-byte semantics for `used_bytes` and fixed-physical-capacity semantics for `total_bytes` are reflected in the new macOS metrics. The public API means the same thing on all three platforms.
- **Dep simplification**: libSystem-only macOS path eliminates `objc2-metal` and `objc2`, removing the version-identification pain experienced in the prior cycle and matching the maintainer's "minimal-dep" instinct (Linux has no FFI crate dep at all).

---

## Pain Points

- **Wrong contract shipped on integration branch.** The `macos-silicon` campaign shipped `currentAllocatedSize` for `used_bytes` and `recommendedMaxWorkingSetSize` for `total_bytes` — both diverge from the maintainer's pre-existing cross-platform contract. The `MetalNoCrossPidSupport` error variant compounds this with a false-cross-PID-impossible premise. None of these should be carried forward.
- **Demand-paging methodology gap.** The first empirical probe allocated `MTLBuffer` objects but never wrote to them. Kernel resident-page counters showed zero because nothing was resident. The team lead initially endorsed the wrong verdict.
- **Dep selection was painful and avoidable.** Identifying the correct versions of `objc2` and `objc2-metal` consumed multiple rounds (initial proposal said `0.5`/`0.2`, corrected to `0.6`/`0.3`; the `default-features = false, features = ["MTLDevice"]` incantation took another pass). All of this work was wasted because the deps are not needed for the libSystem-only path. The cycle never asked "do we need these deps at all?"
- **`CONVENTIONS.md` was treated as a registry to extend** rather than a rule-set that resists modification. Speculative rows were added for unsafe surfaces that did not exist in the code.
- **Provenance confusion**: agent-authored CONVENTIONS rows were repeatedly treated as "existing accepted precedent" when only the Windows row is maintainer-authored.
- **Cross-platform contract not consulted.** The macos-silicon campaign chose `currentAllocatedSize` and `recommendedMaxWorkingSetSize` without comparing semantics against the existing Windows + Linux paths. The Linux code's deliberate `VmRSS` (not `VmSize`) choice was the precedent that should have informed the macOS choice. It did not.
- **Version-number framing drift.** The team lead referred to the planned work as a "v0.4 follow-on" implying a v0.3.0 with the wrong contract would ship first. The maintainer has since clarified the target release as v0.2.2 — applied by the maintainer, not by the implementation PR.
- **Report sprawl.** Nine rounds of reports accumulated (00–08) plus this revision.

---

## Root Causes

- **No "is the proposed metric semantically consistent with the existing platforms?" gate.** The macos-silicon plan picked metrics that did not match the maintainer's resident-byte / fixed-capacity contract. The cross-platform contract should be the first check on any proposed measurement.
- **No "is this dependency strictly required?" gate.** The cycle added `objc2-metal` because the spec said "use Metal," not because the libSystem-only path had been ruled out. Every Apple-framework dep should be auditied against the libSystem alternative before being committed to.
- **No "verify the methodology before declaring conclusions" gate.** The first probe was run, results were taken at face value, and a migration strategy was synthesised — all without internal validation of probe design.
- **The default question about `CONVENTIONS.md` was "how do we update it to accept this new code?"** rather than "is there a path that does not require modifying it at all?" Inverted the project rule.
- **Specs drove unsafe declarations rather than code requirements driving them.** A row was added for a `metal` unsafe surface that does not exist in `src/gpu/metal.rs`.
- **Roadmap depth was used to encode sequencing without forcing "is this leaf justified?"** Eleven leaves shipped because the plan had eleven leaves.
- **Cross-report concept-mixing.** Agent-authored CONVENTIONS rows were repeatedly conflated with maintainer-authored ones.

---

## Next-Cycle Changes

- **Instruction changes**:
  - Every new measurement in the public API must include, in its design documentation, a row of the § Reference Design cross-platform table showing that the macOS semantic matches the Windows + Linux semantic. If the new measurement diverges, the divergence must be justified intrinsically (e.g., the device count under multi-GPU Mac Pro — but that is out of scope for v0.2.2).
  - Every proposed Cargo dependency must be audited against a libSystem-only alternative before being added. If libSystem alone can deliver the measurement, use libSystem and do not add the dep. The dep audit is the first leaf of the next campaign.
  - Every implementation leaf that touches `unsafe` must include, in its `Pre-conditions`, a sentence stating which Rust language rule mandates the `unsafe`. If no such rule applies, the leaf must not add `unsafe`.
  - `CONVENTIONS.md`'s safety/performance-critical sections are **immutable by default**. Modifications require (a) absolutely no alternative path AND (b) experimental proof. The default question is "is there a path that does not require modifying these sections at all?"
  - Empirical probes must include a residency / behaviour-coverage rationale before reading kernel counters. Allocate-but-no-write probes are rejected at design.
  - The PR does NOT modify `Cargo.toml`'s `version` field. The maintainer applies the v0.2.2 bump after acceptance.
- **Workflow changes**:
  - Start the next cycle from a worktree off `main` (v0.2.1). Do not rebase any of `claude/compassionate-moore-709120`'s commits. Cherry-pick nothing.
  - Carry forward exactly one report: `__reports__/macos_ledger/05-findings_writes_v0.md`. Optionally also this knowledge-transfer report.
  - Use a small number of leaves (≤ 5 estimated under the libSystem-only path) reflecting the actual code surface.
  - First leaf: dep audit (libSystem-only feasibility confirmation). Second leaf: extend `mach_ffi` (or sibling `libsystem_ffi`) with `ledger`, `sysctl`, `proc_listpids` externs. Subsequent leaves: dispatcher wiring + tests + docs.
- **Review process changes**:
  - The PR opened from the next cycle must include, in its description:
    1. The § Reference Design cross-platform table showing semantic consistency.
    2. Round 05 measurements as the empirical basis for `graphics_footprint`.
    3. crates.io evidence that no safe wrapper exists for each FFI call requiring `unsafe` (`libc::ledger`, `nix::ledger`, `mach2::ledger`, etc.).
    4. Per-`unsafe`-block citation of which Rust language rule mandates it.
    5. **Explicit proof that no alternative path exists which would avoid modifying safety-critical `CONVENTIONS.md` sections at all.**
    6. PR title and description anchored on "v0.2.2"; PR does not modify `Cargo.toml` version field.

---

## Artifacts to Preserve

| Artifact | Path on integration branch | Why it matters |
|:---|:---|:---|
| Writes-corrected probe findings (Round 05) | `__reports__/macos_ledger/05-findings_writes_v0.md` | Empirical basis for the architecture. Swift + C probe source verbatim in Appendix A. Re-runnable in ~30 min |
| This knowledge-transfer report (v3) | `__reports__/macos_ledger/09-knowledge_transfer_v3.md` | The hand-off; self-contained; supersedes v0, v1, v2 |
| Original maintainer's CONVENTIONS.md Windows row | `CONVENTIONS.md` line 245, commit `8785ade` by `PCfVW <ejacopin@ymail.com>` | The only maintainer-authored unsafe-policy row. Structural template for any new row text, ONLY if § The CONVENTIONS.md Rule conditions are both satisfied |
| Original maintainer's Linux `VmRSS` choice | `src/ram.rs:387` on `main`, commit `8785ade` by `PCfVW <ejacopin@ymail.com>` | The line `line.strip_prefix("VmRSS:")` — deliberate choice of resident over virtual. Same intent on macOS |

The next cycle should retain Round 05's commit-level provenance for the PR description: commit `6c4abd7` contains the probe source, the C bridging header, and the raw output transcripts.

---

## Open Questions for the Next Cycle

| Question | How to answer | Effort |
|:---|:---|:---|
| **Is there a path that delivers the macOS feature without modifying any safety/performance-critical section of `CONVENTIONS.md`?** | For each required syscall, classify as "safe-wrapped crate exists" or "requires `unsafe extern \"C\"` block." Only if **every** syscall falls in the second class should a `CONVENTIONS.md` modification be considered | ~30 min upfront |
| If a `CONVENTIONS.md` modification appears required, has the necessity been experimentally proven? | Side-by-side comparison: (a) attempt to use safe-wrapped path with the specific failure mode; (b) corresponding `unsafe extern "C"` block | ~1 hr |
| Does the `libproc` crate provide a safe `proc_listpids` wrapper? | `cargo new --bin /tmp/libproc-check && cargo add libproc`; check whether the call requires `unsafe { }` | ~10 min |
| What is the earliest macOS version where `LEDGER_ENTRY_INFO_V2` and `graphics_footprint` are present? | Inspect older SDKs' `Kernel.framework/Headers/kern/ledger.h`; project MSRV is 10.15. Round 05 verified on 26.x only | ~20 min |
| Is `sysctlbyname` accessible via plain `unsafe extern "C"` declaration, or does it benefit from a `libc` crate dep? | Inspect the `libc` crate's `sysctlbyname` binding — confirm it is a thin `extern` wrapper. If the wrapper has no Rust-side safety added, prefer the inline `unsafe extern "C"` to avoid the dep | ~10 min |

---

## What the Next Cycle Should NOT Do

- Do not read the integration branch's source under `src/gpu/metal.rs`, `src/gpu/mod.rs`, or `src/error.rs`. Mirror the `main`-branch Windows + Linux patterns, not the abandoned macOS code.
- Do not use `MTLDevice.currentAllocatedSize` for `used_bytes`. Use `graphics_footprint` from the BSD kernel ledger.
- Do not use `MTLDevice.recommendedMaxWorkingSetSize` for `total_bytes`. Use `sysctl hw.memsize`.
- Do not add `objc2-metal` or `objc2` as Cargo dependencies. The libSystem-only path is sufficient and matches the maintainer's "minimal-dep" pattern.
- Do not propagate the `MetalNoCrossPidSupport` error variant. Cross-PID Metal accounting IS supported via the kernel ledger.
- Do not modify `Cargo.toml`'s `version` field. The maintainer applies the v0.2.2 bump after PR acceptance.
- **Do not modify safety/performance-critical sections of `CONVENTIONS.md`** unless every alternative has been experimentally exhausted AND the necessity is documented with proof. If a modification is proposed: the row to add must document an `unsafe` block that already exists in code from a prior leaf; the PR must include the no-alternative proof artifact; the modification must be a separate commit the maintainer can revert independently.
- Do not bundle architectural decisions into the same PR commit as the implementation. Each substantive decision should be expressible as a one-sentence justification in the commit message body with a link to Round 05 and to this report.

---

## Closing Note

The empirical question — "can `hypomnesis` measure per-process Metal memory on macOS?" — is answered: yes, via `ledger(LEDGER_ENTRY_INFO_V2, pid, graphics_footprint_tag)`. Round 05 is the proof.

The semantic question — "should the measurement be allocator-tracked or resident-tracked?" — is also answered: resident-tracked, because that is what Windows `WorkingSetSize` and Linux `VmRSS` already report.

The `total_bytes` question — "fixed capacity or dynamic budget?" — is answered: fixed capacity (`sysctl hw.memsize`), because that is what Windows `DedicatedVideoMemory` and Linux `nvmlDeviceGetMemoryInfo.total` already report.

The dependency question — "do we need `objc2-metal` and `objc2`?" — is answered: no. libSystem syscalls (`task_info`, `ledger`, `sysctl`) deliver every required measurement. The `metal` Cargo feature becomes `metal = []` with no third-party deps.

The release-framing question — "v0.3.0 or something else?" — is answered: v0.2.2, applied by the maintainer after PR acceptance. The PR itself does not modify `Cargo.toml`'s `version` field.

The engineering question — "can the implementation be delivered without touching `CONVENTIONS.md`?" — is the gating question for the next cycle. The default answer must be assumed to be "yes, find that path" until the no-alternative proof says otherwise.

Everything else in this cycle is friction the next implementer should not inherit. The next worktree starts from `main`, reads this report and Round 05, and ships a focused v0.2.2 release.
