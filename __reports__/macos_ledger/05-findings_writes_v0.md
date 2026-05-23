# macOS Ledger — Writes-Corrected Probe Findings (v0)

Date: 2026-05-22

---
type: findings
topic: macos_ledger
date: 2026-05-22
version: v0
prior-version: 00-findings_v0.md
key-metric: graphics_footprint delta per 256 MiB MTLBuffer (shared, written): +268,435,456 B = +256 MiB (prior: +32,768 B = 0 pages after allocation without writes, delta: +268,418,048 B)
decision-required: intervene
---

## Executive Summary

- **Hypothesis H1 is confirmed**: writing every byte of a 256 MiB `storageModeShared` `MTLBuffer` causes `graphics_footprint` (ledger entry [36]) to increase by exactly +256 MiB (+268,435,456 B). The Strand A v0 negative result was an artefact of demand-paging, not a real kernel-side ignoring of Metal allocations. The `03-observation_v0.md` methodology gap is now closed.
- **The v0 verdict in `02-migration_strategy_v0.md` is superseded.** `graphics_footprint` does track Metal-allocated memory once pages are touched. A v1 migration strategy is needed.
- **`phys_footprint` also tracks Metal-written memory**: after the write, `phys_footprint` grows by ~256 MiB, confirming that written Metal shared buffers enter the task physical footprint on Apple Silicon UMA. This directly affects the docs claim that `phys_footprint` "matches Activity Monitor" — for processes that hold unwritten Metal buffers, it will under-report.
- **`currentAllocatedSize` is allocator-tracked, not resident-tracked**: it jumped from 81,920 B to 268,517,376 B at Phase 1 (allocation without writing) and did not change at Phase 2 (after writes). This confirms `currentAllocatedSize` counts virtual Metal allocations regardless of page residency, while `graphics_footprint` counts resident pages.
- Phases 4–6 (storageModePrivate + blit) were executed. The blit copy caused `graphics_footprint` to grow by an additional +344,326,144 B (~328 MiB), consistent with the GPU driver mapping the private buffer's pages as it copies the data.

---

## Headline Result

```
metric:    graphics_footprint delta Phase 1 -> Phase 2 (write every byte)
value:     +268,435,456
unit:      bytes
prior:     +32,768 B (Strand A probe, Phase baseline -> +256MiB allocated, no write)
direction: up
```

---

## Results Tables

### Table 1 — Summary: All Four Key Metrics at Each Phase

| Phase | `currentAllocatedSize` (B) | `phys_footprint` (B) | `ledger_tag_graphics` (B) | `graphics_footprint` ledger[36] (B) |
|---|---|---|---|---|
| Phase 0 — baseline | 81,920 | 3,981,864 | 16,384 | 16,384 |
| Phase 1 — alloc 256 MiB shared, no write | 268,517,376 | 3,998,248 | 16,384 | 16,384 |
| Phase 2 — write every byte (for-loop) | 268,517,376 | 272,581,376 | 268,451,840 | 268,451,840 |
| Phase 3 — `didModifyRange(0..<256MiB)` | 268,517,376 | 272,597,760 | 268,451,840 | 268,451,840 |
| Phase 4 — alloc 256 MiB private, no blit | 537,346,048 | 272,958,208 | 268,517,376 | 268,517,376 |
| Phase 5 — blit shared→private committed | 537,346,048 | 618,464,048 | 612,843,520 | 612,843,520 |
| Phase 6 — post-blit steady state | 537,346,048 | 618,464,048 | 612,843,520 | 612,843,520 |

### Table 2 — Phase-by-Phase Delta of Key Metrics

| Transition | `currentAllocatedSize` delta | `phys_footprint` delta | `ledger_tag_graphics` delta | `graphics_footprint`[36] delta |
|---|---|---|---|---|
| Ph0→Ph1 (allocate, no write) | **+268,435,456 B (+256 MiB)** | +16,384 B (+1 page) | 0 | 0 |
| Ph1→Ph2 (write every byte) | **0** | +268,583,128 B (+256 MiB) | **+268,435,456 B (+256 MiB)** | **+268,435,456 B (+256 MiB)** |
| Ph2→Ph3 (didModifyRange) | 0 | +16,384 B (+1 page) | 0 | 0 |
| Ph3→Ph4 (alloc private, no blit) | +268,828,672 B (+256 MiB) | +360,448 B (~0) | +65,536 B (4 pages) | +65,536 B (4 pages) |
| Ph4→Ph5 (blit copy committed) | 0 | +345,505,840 B (+329 MiB) | **+344,326,144 B (+328 MiB)** | **+344,326,144 B (+328 MiB)** |
| Ph5→Ph6 (steady state) | 0 | 0 | 0 | 0 |

### Table 3 — Ledger Entries That Moved ≥ 1 Page on Phase 1→2 (write every byte)

| Index | Entry name | Before (B) | After (B) | Delta (B) | MiB |
|---|---|---|---|---|---|
| 1 | `tkm_private` | 376,832 | 524,288 | +147,456 | +0.141 |
| 3 | `phys_mem` | 11,616,256 | 280,051,712 | +268,435,456 | **+256.000** |
| 6 | `internal` | 3,522,560 | 271,958,016 | +268,435,456 | **+256.000** |
| 8 | `alternate_accounting` | 81,920 | 268,517,376 | +268,435,456 | **+256.000** |
| 10 | `page_table` | 393,768 | 541,440 | +147,672 | +0.141 |
| 11 | `phys_footprint` | 3,998,248 | 272,581,376 | +268,583,128 | **+256.141** |
| **36** | **`graphics_footprint`** | **16,384** | **268,451,840** | **+268,435,456** | **+256.000** |

### Table 4 — `currentAllocatedSize` Semantics

| Event | `currentAllocatedSize` before | After | Delta | Interpretation |
|---|---|---|---|---|
| `makeBuffer(256 MiB, .storageModeShared)` | 81,920 B | 268,517,376 B | +256 MiB | Allocator-tracked: jumps on allocation |
| Write every byte (for-loop) | 268,517,376 B | 268,517,376 B | **0** | Resident-independent: does not change on write |
| `didModifyRange` | 268,517,376 B | 268,517,376 B | 0 | No effect on `currentAllocatedSize` |

---

## Observations

| Signal | Baseline / Expected | Observed [source] | Interpretation |
|---|---|---|---|
| `graphics_footprint` on write | H2 expected: stays 0; H1 expected: +256 MiB | +268,435,456 B = +256 MiB at Phase 2 [probe output Phase 1→2 delta] | H1 confirmed: Metal pages enter `graphics_footprint` once written/resident. Strand A's zero was demand-paging, not real absence of tracking. |
| `graphics_footprint` on allocation (no write) | Neutral: untouched pages are non-resident | Delta = 0 (Phase 0→1) [probe output Phase 0→1 delta] | Kernel tracks resident pages, not virtual allocations. `makeBuffer` alone does not charge the ledger. |
| `phys_footprint` on write | H2: stays unchanged; H1/H3: grows with buffer | +268,583,128 B ≈ +256 MiB at Phase 2 [probe output task_info phys_footprint] | Metal-written shared buffers enter `phys_footprint`. Docs claim "matches Activity Monitor" stands for actively-written buffers but fails for unwritten ones. |
| `currentAllocatedSize` allocator vs resident | H4 predicted it would grow on write; Apple docs lean allocator | Does not change on write (Phase 1→2 delta = 0); grew on allocation (Phase 0→1) [probe output summary table] | `currentAllocatedSize` is allocator-tracked (virtual reservation). `graphics_footprint` is resident-tracked. The 10,000× divergence in Strand A was allocator vs resident, not allocator vs zero. |
| `ledger_tag_graphics_footprint` via `task_vm_info` | Should mirror ledger entry [36] | Identical to `graphics_footprint` at all phases [probe summary table] | Two read paths (Mach `task_vm_info` and BSD `ledger()`) share the same kernel counter, confirming REV3 field reliability. |
| `storageModePrivate` blit tracking | Private mode uses GPU-only memory | Phase 4→5 blit: `graphics_footprint` +344 MiB (shared already counted from Phase 2, so cumulative = 256+328 = 584 MiB at Phase 5) [probe Phase 5 absolute: 612,843,520 B] | Private-mode GPU memory is also tracked by `graphics_footprint` when pages become resident via a blit. |

---

## Charts & Visualizations

```
graphics_footprint vs currentAllocatedSize — writes-corrected probe
Apple M3 Pro, macOS 26.3.1, page size 16,384 B

currentAllocatedSize (MiB, upper axis):
 537 |                                        ●─────●─────●─────●
 268 |              ●─────●─────●─────●
  82 | ●
   0 +─────────────────────────────────────────────────────────
      Ph0   Ph1   Ph2   Ph3   Ph4   Ph5   Ph6
             (alloc) (write) (dMR) (alloc) (blit) (ss)

graphics_footprint (MiB, lower axis):
 613 |                                               ●─────●
 268 |                    ●─────●─────●
   0 | ●─────●
     +─────────────────────────────────────────────────────────
      Ph0   Ph1   Ph2   Ph3   Ph4   Ph5   Ph6

Key observations:
- currentAllocatedSize jumps at allocation (Ph0→Ph1, Ph3→Ph4), not at write
- graphics_footprint jumps at write (Ph1→Ph2), not at allocation
- Both metrics track the same total memory but through different residency states
```

---

## Contradictions & Surprises

- Strand A's observation that `graphics_footprint ≈ 49 KiB` with 512 MiB allocated was entirely explained by demand-paging: the probe never wrote, so no pages were ever resident. The counter was correct all along — it tracks resident GPU memory, not virtual reservations.
- `phys_footprint` growing by ~256 MiB on write (shared buffer) is a significant secondary finding: the docs' claim that `phys_footprint` "matches Activity Monitor" is only correct for processes that actively write to their Metal buffers. A process holding large allocated-but-untouched Metal heaps will be under-reported by `phys_footprint` vs Activity Monitor.
- `internal` and `alternate_accounting` entries each grew by exactly +256 MiB simultaneously with `graphics_footprint` on Phase 1→2. This indicates Metal's shared buffer memory is classified as `VM_LEDGER_TAG_GRAPHICS` (`graphics_footprint`) and also counted in the `internal` (anonymous/private VM) and `alternate_accounting` categories — consistent with Metal using a unified physical backing that is tagged to multiple accounting buckets.

---

## Hypothesis Verdict

**Hypothesis H1 (from `04-open_question_v0.md`) is supported by the data.**

H1 states: "`graphics_footprint` and/or `iokit_mapped` increases by approximately the written buffer size after the probe writes every byte."

Evidence chain:

1. Phase 1→2 `graphics_footprint` delta: +268,435,456 B = exactly 256 MiB = 16,384 pages.
2. Phase 1→2 `task_vm_info.ledger_tag_graphics_footprint` delta: identical (+268,435,456 B), confirming both read paths.
3. Phase 1→2 `currentAllocatedSize` delta: **0** — confirming this is not re-allocation, but page-residency arrival.
4. Phase 0→1 (allocate without write) delta: `graphics_footprint` = 0, `currentAllocatedSize` = +256 MiB — confirming allocation alone does not charge the ledger.
5. Phase 5 `graphics_footprint` absolute value: 612,843,520 B = 584 MiB = shared (256) + private-after-blit (~328). Private mode is also tracked.

H2 is **not supported** (writes did restore the ledger signal). H3 is **not the best fit** (it was `graphics_footprint` specifically, not an unexpected entry, that responded). H4 is **not supported** (`currentAllocatedSize` did not grow on write).

---

## Implications for Round 02 (Migration Strategy)

**Strand D's v0 verdict in `02-migration_strategy_v0.md` is superseded; a v1 synthesis is needed.**

The v0 verdict ("Option C — leave-as-is + minor polish") was grounded in the Round 00 finding that `graphics_footprint` did not track Metal allocations. That finding is now corrected: the tracking exists, but only once pages are written/resident. The pivot question re-opens:

- `ledger(LEDGER_ENTRY_INFO_V2, pid)` does provide cross-process, unprivileged access to `graphics_footprint` for any same-user process.
- `graphics_footprint` does track Metal-allocated memory — once pages are resident.
- The key caveat: **it does not track unwritten pages**. For processes that hold large Metal allocations without writing (e.g., on-demand GPU workloads), `graphics_footprint` will under-report vs `currentAllocatedSize`. The two metrics remain complementary, not interchangeable.
- `02-migration_strategy_v1.md` must re-examine whether the cross-process ledger path has sufficient coverage for hypomnesis's use cases, given this residency dependency.

---

## Cross-link to Round 03

`03-observation_v0.md` framed this re-probe by identifying that Strand A's `metal_probe_hold.swift` allocated `MTLBuffer` objects but never wrote to them. The v0 verdict in `00-findings_v0.md` and `02-migration_strategy_v0.md` were explicitly marked provisional pending this corrected test. This report (`05-findings_writes_v0.md`) closes that provisional status: the corrected test has been run and the verdict has flipped from H2 to H1.

---

## Open Follow-ups

- **`phys_footprint` vs Activity Monitor (Round 02 secondary finding)**: This data illuminates the question. `phys_footprint` grows by ~256 MiB when Metal shared buffers are written, which means it does track written Metal memory. However, it does not track unwritten allocations (Phase 0→1 delta was only 1 page). Activity Monitor reports resident physical memory, which should also exclude unwritten pages. This is consistent — but it means a Metal-heavy process that allocates and then writes all its buffers (most real workloads) will be correctly reported by `phys_footprint`. A process using Metal for lazy allocation (uncommitted reserves) will be under-reported. No new campaign is scoped here; this nuance should be documented in `02-migration_strategy_v1.md`.
- **`graphics_footprint` for unwritten-page scenarios**: A cross-process reader of `graphics_footprint` will under-report GPU memory for target processes that hold unwritten Metal buffers. Whether this gap matters for hypomnesis's cross-process use case depends on the target workloads. The `02-migration_strategy_v1.md` synthesis should address this.

---

## Steering Questions

- **[now]** Scope and write `02-migration_strategy_v1.md`: the ledger pivot is viable for written-page use cases; the v0 "leave-as-is" verdict is superseded. What is the new recommended migration option given the residency caveat?
- **[now]** Update hypomnesis docs to qualify the `phys_footprint` "matches Activity Monitor" claim with the residency caveat for unwritten Metal allocations.
- **[next run]** Determine empirically whether a real-world target workload (LM Studio, MLX inference) writes its Metal buffers before hypomnesis reads the ledger. If yes, `graphics_footprint` is a reliable metric; if not, a residency-trigger (read-then-write or wait-until-completed) may be needed in the measurement path.
- **[later]** Confirm whether the ~328 MiB Phase 4→5 delta (private blit) is genuinely new physical pages or GPU MMU remapping of the shared pages. The two buffers may share the same UMA pages and the +328 MiB could double-count the shared buffer's resident pages.

---

## Pointers

- This report: `__reports__/macos_ledger/05-findings_writes_v0.md`
- Prior findings (superseded): [`00-findings_v0.md`](00-findings_v0.md)
- Methodology gap that drove this re-probe: [`03-observation_v0.md`](03-observation_v0.md)
- Open question framing (hypotheses H1–H4, test design): [`04-open_question_v0.md`](04-open_question_v0.md)
- Superseded migration strategy: [`02-migration_strategy_v0.md`](02-migration_strategy_v0.md)
- Probe binary (ephemeral): `/tmp/ledger-probe-writes/metal_probe_writes`
- Probe source (ephemeral): `/tmp/ledger-probe-writes/metal_probe_writes.swift`, `/tmp/ledger-probe-writes/bridge.h`
- Raw probe output (ephemeral): `/tmp/ledger-probe-writes/output.txt`

---

## Appendix A — Probe Source

### `bridge.h`

```c
#ifndef BRIDGE_H
#define BRIDGE_H

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <errno.h>
#include <mach/mach.h>
#include <mach/task_info.h>
#include <sys/types.h>

/* ------------------------------------------------------------------ *
 * ledger() syscall declarations — no user-space header ships these.
 * ABI is stable (SYS_ledger = 373). Constants from kern/ledger.h.
 * ------------------------------------------------------------------ */

#define LEDGER_INFO             0
#define LEDGER_ENTRY_INFO       1
#define LEDGER_TEMPLATE_INFO    2
#define LEDGER_LIMIT            3
#define LEDGER_ENTRY_INFO_V2    4
#define LEDGER_NAME_MAX         32

struct ledger_info {
    char    li_name[LEDGER_NAME_MAX];
    int64_t li_id;
    int64_t li_entries;
};

struct ledger_template_info {
    char lti_name[LEDGER_NAME_MAX];
    char lti_group[LEDGER_NAME_MAX];
    char lti_units[LEDGER_NAME_MAX];
};

struct ledger_entry_info_v2 {
    int64_t  lei_balance;
    int64_t  lei_credit;
    int64_t  lei_debit;
    uint64_t lei_limit;
    uint64_t lei_refill_period;
    uint64_t lei_last_refill;
    int64_t  lei_lifetime_max;
    uint64_t lei_reserved[4];
};  /* sizeof = 88 bytes */

extern int ledger(int cmd, caddr_t arg1, caddr_t arg2, caddr_t arg3);

#endif /* BRIDGE_H */
```

### `metal_probe_writes.swift`

```swift
// metal_probe_writes.swift
// Writes-corrected Metal ledger probe — Phases 0–3 (required) + 4–6 (optional).
// Closes the Strand A methodology gap: forces page residency before reading counters.
//
// Compile:
//   xcrun -sdk macosx swiftc -framework Metal -framework Foundation \
//       -o /tmp/ledger-probe-writes/metal_probe_writes \
//       /tmp/ledger-probe-writes/metal_probe_writes.swift \
//       -import-objc-header /tmp/ledger-probe-writes/bridge.h

import Metal
import Foundation

// ─────────────────────────────────────────────────────────────────────────────
// Constants (must match bridge.h)
// ─────────────────────────────────────────────────────────────────────────────

let LEDGER_TEMPLATE_INFO_CMD: Int32 = 2
let LEDGER_ENTRY_INFO_V2_CMD: Int32 = 4
let LEDGER_NAME_MAX_INT: Int = 32
let PAGE_SIZE: Int64 = 16_384
let EXPECTED_ENTRY_COUNT = 68  // confirmed by Strand A on this host

// ─────────────────────────────────────────────────────────────────────────────
// Helpers
// ─────────────────────────────────────────────────────────────────────────────

func mib(_ bytes: Int64) -> String { return String(format: "%.3f", Double(bytes) / (1024.0*1024.0)) }
func mibI(_ bytes: Int) -> String  { return String(format: "%.3f", Double(bytes) / (1024.0*1024.0)) }

// ─────────────────────────────────────────────────────────────────────────────
// Ledger snapshot
// ─────────────────────────────────────────────────────────────────────────────

struct LedgerSnapshot {
    var entries: [(name: String, balance: Int64)]
    var currentAllocatedSize: Int
    var physFootprint: Int64
    var machGraphicsFootprint: Int64
}

func readLedger(pid: pid_t, templateNames: [String], device: MTLDevice) -> LedgerSnapshot {
    // ── LEDGER_ENTRY_INFO_V2 ────────────────────────────────────────────────
    let ev2buf = UnsafeMutablePointer<ledger_entry_info_v2>.allocate(capacity: EXPECTED_ENTRY_COUNT)
    defer { ev2buf.deallocate() }
    var ev2count: Int32 = Int32(EXPECTED_ENTRY_COUNT)

    let _ = ev2buf.withMemoryRebound(to: CChar.self, capacity: EXPECTED_ENTRY_COUNT * MemoryLayout<ledger_entry_info_v2>.stride) { ev2ptr in
        withUnsafeMutablePointer(to: &ev2count) { countPtr in
            countPtr.withMemoryRebound(to: CChar.self, capacity: 1) { countCptr in
                ledger(LEDGER_ENTRY_INFO_V2_CMD,
                       caddr_t(bitPattern: Int(pid)),
                       ev2ptr,
                       countCptr)
            }
        }
    }

    var entries: [(name: String, balance: Int64)] = []
    let returned = Int(ev2count)
    for i in 0..<min(returned, EXPECTED_ENTRY_COUNT) {
        let balance = ev2buf[i].lei_balance
        let name = i < templateNames.count ? templateNames[i] : "entry\(i)"
        entries.append((name: name, balance: balance))
    }

    // ── task_vm_info ─────────────────────────────────────────────────────────
    var vmInfo = task_vm_info_data_t()
    var vmCount = mach_msg_type_number_t(MemoryLayout<task_vm_info_data_t>.size / MemoryLayout<natural_t>.size)
    let kr = withUnsafeMutablePointer(to: &vmInfo) {
        $0.withMemoryRebound(to: integer_t.self, capacity: Int(vmCount)) { ptr in
            task_info(mach_task_self_, task_flavor_t(TASK_VM_INFO), ptr, &vmCount)
        }
    }
    var physFootprint: Int64 = -1
    var machGraphics: Int64 = -1
    if kr == KERN_SUCCESS {
        physFootprint = Int64(vmInfo.phys_footprint)
        let rev3count = mach_msg_type_number_t(84)
        if vmCount >= rev3count {
            machGraphics = Int64(vmInfo.ledger_tag_graphics_footprint)
        }
    }

    return LedgerSnapshot(
        entries: entries,
        currentAllocatedSize: device.currentAllocatedSize,
        physFootprint: physFootprint,
        machGraphicsFootprint: machGraphics
    )
}

func loadTemplateNames() -> [String] {
    let maxSlots = 128
    var count: Int32 = Int32(maxSlots)
    let stride = MemoryLayout<ledger_template_info>.stride
    let rawBuf = UnsafeMutableRawPointer.allocate(byteCount: maxSlots * stride, alignment: MemoryLayout<ledger_template_info>.alignment)
    defer { rawBuf.deallocate() }

    // Convention from Strand A probe2.c: ledger(LEDGER_TEMPLATE_INFO, tmpl_buf, &count, NULL)
    // arg1 = template buffer, arg2 = count pointer, arg3 = NULL
    let _ = withUnsafeMutablePointer(to: &count) { countPtr in
        countPtr.withMemoryRebound(to: CChar.self, capacity: 1) { countCptr in
            ledger(LEDGER_TEMPLATE_INFO_CMD,
                   rawBuf.assumingMemoryBound(to: CChar.self),
                   countCptr,
                   nil)
        }
    }
    let actualCount = min(Int(count), EXPECTED_ENTRY_COUNT)
    var names: [String] = []
    for i in 0..<actualCount {
        let namePtr = rawBuf.advanced(by: i * stride).assumingMemoryBound(to: CChar.self)
        var nameBuf = [CChar](repeating: 0, count: LEDGER_NAME_MAX_INT + 1)
        for j in 0..<LEDGER_NAME_MAX_INT {
            nameBuf[j] = namePtr[j]
            if namePtr[j] == 0 { break }
        }
        names.append(String(cString: nameBuf))
    }
    return names
}

// ─────────────────────────────────────────────────────────────────────────────
// Delta printing — uses only Swift string interpolation (no %s format specs)
// ─────────────────────────────────────────────────────────────────────────────

func printAbsolute(label: String, snap: LedgerSnapshot) {
    print("  [\(label)] currentAllocatedSize=\(snap.currentAllocatedSize) B (\(mibI(snap.currentAllocatedSize)) MiB)")
    print("  [\(label)] phys_footprint=\(snap.physFootprint) B (\(mib(snap.physFootprint)) MiB)")
    print("  [\(label)] ledger_tag_graphics_footprint=\(snap.machGraphicsFootprint) B (\(mib(snap.machGraphicsFootprint)) MiB)")
}

func printDeltas(label: String, before: LedgerSnapshot, after: LedgerSnapshot) {
    print("")
    print("DELTA: \(label)")
    print("---------------------------------------------------------------")

    var anyLedgerMoved = false
    // Skip index 0 (cpu_time in nanoseconds — not a memory counter)
    for i in 1..<min(before.entries.count, after.entries.count) {
        let bBal = before.entries[i].balance
        let aBal = after.entries[i].balance
        let delta = aBal - bBal
        if abs(delta) >= PAGE_SIZE {
            let name = before.entries[i].name
            print("  [\(i)] \(name): before=\(bBal)  after=\(aBal)  delta=\(delta)  (\(mib(delta)) MiB)")
            anyLedgerMoved = true
        }
    }
    if !anyLedgerMoved {
        print("  (no ledger entry moved by >= 1 page = \(PAGE_SIZE) bytes)")
    }

    let casDelta = after.currentAllocatedSize - before.currentAllocatedSize
    print("  currentAllocatedSize:  before=\(before.currentAllocatedSize)  after=\(after.currentAllocatedSize)  delta=\(casDelta)  (\(mibI(casDelta)) MiB)")

    let pfDelta = after.physFootprint - before.physFootprint
    print("  task_info phys_footprint:  before=\(before.physFootprint)  after=\(after.physFootprint)  delta=\(pfDelta)  (\(mib(pfDelta)) MiB)")

    let mgDelta = after.machGraphicsFootprint - before.machGraphicsFootprint
    print("  task_vm_info ledger_tag_graphics_footprint:  before=\(before.machGraphicsFootprint)  after=\(after.machGraphicsFootprint)  delta=\(mgDelta)  (\(mib(mgDelta)) MiB)")
}

// ─────────────────────────────────────────────────────────────────────────────
// Main
// ─────────────────────────────────────────────────────────────────────────────

guard let device = MTLCreateSystemDefaultDevice() else {
    fputs("FATAL: MTLCreateSystemDefaultDevice() returned nil\n", stderr)
    exit(1)
}

let pid = getpid()
print("PID=\(pid)  device=\(device.name)")
print("Host page size: \(PAGE_SIZE) bytes")

let templateNames = loadTemplateNames()
print("Ledger template entries (clamped to \(EXPECTED_ENTRY_COUNT)): \(templateNames.count)")
print("Template names:")
for (i, n) in templateNames.enumerated() {
    print("  [\(i)] \(n)")
}

// ── Phase 0: Baseline ─────────────────────────────────────────────────────────
let snap0 = readLedger(pid: pid, templateNames: templateNames, device: device)
print("\n== Phase 0 (baseline) ==")
printAbsolute(label: "phase0", snap: snap0)

// ── Phase 1: Allocate 256 MiB storageModeShared (no write) ───────────────────
let bufLen = 256 << 20
let bufShared = device.makeBuffer(length: bufLen, options: .storageModeShared)!
let snap1 = readLedger(pid: pid, templateNames: templateNames, device: device)
print("\n== Phase 1 (allocated 256 MiB storageModeShared, no write) ==")
printAbsolute(label: "phase1", snap: snap1)
printDeltas(label: "Phase 0 -> Phase 1 (allocate shared, no write)", before: snap0, after: snap1)

// ── Phase 2: Write every byte (force page residency) ─────────────────────────
let ptr = bufShared.contents().bindMemory(to: UInt8.self, capacity: bufLen)
for i in 0..<bufLen {
    ptr[i] = UInt8(i & 0xff)
}
let snap2 = readLedger(pid: pid, templateNames: templateNames, device: device)
print("\n== Phase 2 (wrote every byte of 256 MiB shared buffer via Swift for-loop) ==")
printAbsolute(label: "phase2", snap: snap2)
printDeltas(label: "Phase 1 -> Phase 2 (write every byte via Swift for-loop)", before: snap1, after: snap2)
printDeltas(label: "Phase 0 -> Phase 2 (allocate + write, cumulative)", before: snap0, after: snap2)

// ── Phase 3: didModifyRange ────────────────────────────────────────────────────
bufShared.didModifyRange(0..<bufLen)
let snap3 = readLedger(pid: pid, templateNames: templateNames, device: device)
print("\n== Phase 3 (buf.didModifyRange(0..<256MiB) called) ==")
printAbsolute(label: "phase3", snap: snap3)
printDeltas(label: "Phase 2 -> Phase 3 (didModifyRange)", before: snap2, after: snap3)

// ── Phases 4–6: storageModePrivate + blit copy ────────────────────────────────
print("\n== Phase 4 (allocate 256 MiB storageModePrivate) ==")
guard let commandQueue = device.makeCommandQueue() else {
    print("SKIPPED Phases 4-6: could not create MTLCommandQueue")
    print("\nDone (Phases 4-6 skipped).")
    exit(0)
}
let bufPrivate = device.makeBuffer(length: bufLen, options: .storageModePrivate)!
let snap4 = readLedger(pid: pid, templateNames: templateNames, device: device)
printAbsolute(label: "phase4", snap: snap4)
printDeltas(label: "Phase 3 -> Phase 4 (allocate private buffer, no blit yet)", before: snap3, after: snap4)

print("\n== Phase 5 (blit copy shared -> private, forces private residency) ==")
guard let cmdBuf = commandQueue.makeCommandBuffer(),
      let blitEnc = cmdBuf.makeBlitCommandEncoder() else {
    print("SKIPPED Phases 5-6: could not create command buffer or blit encoder")
    print("\nDone (Phases 5-6 skipped).")
    exit(0)
}
blitEnc.copy(from: bufShared, sourceOffset: 0, to: bufPrivate, destinationOffset: 0, size: bufLen)
blitEnc.endEncoding()
cmdBuf.commit()
cmdBuf.waitUntilCompleted()

let snap5 = readLedger(pid: pid, templateNames: templateNames, device: device)
printAbsolute(label: "phase5", snap: snap5)
printDeltas(label: "Phase 4 -> Phase 5 (blit copy forces private residency)", before: snap4, after: snap5)

print("\n== Phase 6 (post-blit snapshot; shared buffer still in ARC scope) ==")
let snap6 = readLedger(pid: pid, templateNames: templateNames, device: device)
printAbsolute(label: "phase6", snap: snap6)
printDeltas(label: "Phase 5 -> Phase 6 (steady state)", before: snap5, after: snap6)

// ── Summary table ──────────────────────────────────────────────────────────────
print("\n== SUMMARY TABLE ==")
print("Phase | currentAllocatedSize | phys_footprint | ledger_tag_graphics | gfx_ledger[36]")
let gfxIdx = 36   // confirmed by Strand A; index is stable
let allPhases: [(String, LedgerSnapshot)] = [
    ("Phase0_baseline",            snap0),
    ("Phase1_alloc_shared_nowrite", snap1),
    ("Phase2_write_every_byte",    snap2),
    ("Phase3_didModifyRange",      snap3),
    ("Phase4_alloc_private",       snap4),
    ("Phase5_blit_committed",      snap5),
    ("Phase6_post_blit",           snap6),
]
for (name, snap) in allPhases {
    let gfxBal = gfxIdx < snap.entries.count ? snap.entries[gfxIdx].balance : -1
    print("\(name) | \(snap.currentAllocatedSize) | \(snap.physFootprint) | \(snap.machGraphicsFootprint) | \(gfxBal)")
}

print("\n== All nonzero ledger entries at Phase 2 (after write) ==")
print("(Index 0 = cpu_time in ns, skipped)")
for (i, e) in snap2.entries.enumerated() where e.balance != 0 && i > 0 {
    print("  [\(i)] \(e.name): balance=\(e.balance)")
}

print("\nDone (exit 0).")
```

---

## Appendix B — Raw Probe Output

```
PID=52599  device=Apple M3 Pro
Host page size: 16384 bytes
Ledger template entries (clamped to 68): 68
Template names:
  [0] cpu_time
  [1] tkm_private
  [2] tkm_shared
  [3] phys_mem
  [4] wired_mem
  [5] conclave_mem
  [6] internal
  [7] iokit_mapped
  [8] alternate_accounting
  [9] alternate_accounting_compressed
  [10] page_table
  [11] phys_footprint
  [12] internal_compressed
  [13] reusable
  [14] external
  [15] purgeable_volatile
  [16] purgeable_nonvolatile
  [17] purgeable_volatile_compress
  [18] purgeable_nonvolatile_compress
  [19] pages_grabbed
  [20] pages_grabbed_kern
  [21] pages_grabbed_iopl
  [22] pages_grabbed_upl
  [23] tagged_nofootprint
  [24] tagged_footprint
  [25] tagged_nofootprint_compressed
  [26] tagged_footprint_compressed
  [27] network_volatile
  [28] network_nonvolatile
  [29] network_volatile_compressed
  [30] network_nonvolatile_compressed
  [31] media_nofootprint
  [32] media_footprint
  [33] media_nofootprint_compressed
  [34] media_footprint_compressed
  [35] graphics_nofootprint
  [36] graphics_footprint
  [37] graphics_nofootprint_compressed
  [38] graphics_footprint_compressed
  [39] neural_nofootprint
  [40] neural_footprint
  [41] neural_nofootprint_compressed
  [42] neural_footprint_compressed
  [43] neural_nofootprint_total
  [44] est_reclaimable
  [45] platform_idle_wakeups
  [46] interrupt_wakeups
  [47] SFI_CLASS_DARWIN_BG
  [48] SFI_CLASS_APP_NAP
  [49] SFI_CLASS_MANAGED
  [50] SFI_CLASS_DEFAULT
  [51] SFI_CLASS_OPTED_OUT
  [52] SFI_CLASS_UTILITY
  [53] SFI_CLASS_LEGACY
  [54] SFI_CLASS_USER_INITIATED
  [55] SFI_CLASS_USER_INTERACTIVE
  [56] SFI_CLASS_MAINTENANCE
  [57] SFI_CLASS_RUNAWAY_MITIGATION
  [58] cpu_time_billed_to_me
  [59] cpu_time_billed_to_others
  [60] physical_writes
  [61] logical_writes
  [62] logical_writes_to_external
  [63] fs_metadata_writes
  [64] energy_billed_to_me
  [65] energy_billed_to_others
  [66] memorystatus_dirty_time
  [67] swapins

== Phase 0 (baseline) ==
  [phase0] currentAllocatedSize=81920 B (0.078 MiB)
  [phase0] phys_footprint=3981864 B (3.797 MiB)
  [phase0] ledger_tag_graphics_footprint=16384 B (0.016 MiB)

== Phase 1 (allocated 256 MiB storageModeShared, no write) ==
  [phase1] currentAllocatedSize=268517376 B (256.078 MiB)
  [phase1] phys_footprint=3998248 B (3.813 MiB)
  [phase1] ledger_tag_graphics_footprint=16384 B (0.016 MiB)

DELTA: Phase 0 -> Phase 1 (allocate shared, no write)
---------------------------------------------------------------
  [2] tkm_shared: before=0  after=-147456  delta=-147456  (-0.141 MiB)
  [3] phys_mem: before=11517952  after=11616256  delta=98304  (0.094 MiB)
  [6] internal: before=3489792  after=3522560  delta=32768  (0.031 MiB)
  [8] alternate_accounting: before=65536  after=81920  delta=16384  (0.016 MiB)
  [11] phys_footprint: before=3981864  after=3998248  delta=16384  (0.016 MiB)
  [14] external: before=8028160  after=8093696  delta=65536  (0.062 MiB)
  currentAllocatedSize:  before=81920  after=268517376  delta=268435456  (256.000 MiB)
  task_info phys_footprint:  before=3981864  after=3998248  delta=16384  (0.016 MiB)
  task_vm_info ledger_tag_graphics_footprint:  before=16384  after=16384  delta=0  (0.000 MiB)

== Phase 2 (wrote every byte of 256 MiB shared buffer via Swift for-loop) ==
  [phase2] currentAllocatedSize=268517376 B (256.078 MiB)
  [phase2] phys_footprint=272581376 B (259.954 MiB)
  [phase2] ledger_tag_graphics_footprint=268451840 B (256.016 MiB)

DELTA: Phase 1 -> Phase 2 (write every byte via Swift for-loop)
---------------------------------------------------------------
  [1] tkm_private: before=376832  after=524288  delta=147456  (0.141 MiB)
  [3] phys_mem: before=11616256  after=280051712  delta=268435456  (256.000 MiB)
  [6] internal: before=3522560  after=271958016  delta=268435456  (256.000 MiB)
  [8] alternate_accounting: before=81920  after=268517376  delta=268435456  (256.000 MiB)
  [10] page_table: before=393768  after=541440  delta=147672  (0.141 MiB)
  [11] phys_footprint: before=3998248  after=272581376  delta=268583128  (256.141 MiB)
  [19] pages_grabbed: before=297  after=16692  delta=16395  (0.016 MiB)
  [36] graphics_footprint: before=16384  after=268451840  delta=268435456  (256.000 MiB)
  currentAllocatedSize:  before=268517376  after=268517376  delta=0  (0.000 MiB)
  task_info phys_footprint:  before=3998248  after=272581376  delta=268583128  (256.141 MiB)
  task_vm_info ledger_tag_graphics_footprint:  before=16384  after=268451840  delta=268435456  (256.000 MiB)

DELTA: Phase 0 -> Phase 2 (allocate + write, cumulative)
---------------------------------------------------------------
  [1] tkm_private: before=376832  after=524288  delta=147456  (0.141 MiB)
  [2] tkm_shared: before=0  after=-147456  delta=-147456  (-0.141 MiB)
  [3] phys_mem: before=11517952  after=280051712  delta=268533760  (256.094 MiB)
  [6] internal: before=3489792  after=271958016  delta=268468224  (256.031 MiB)
  [8] alternate_accounting: before=65536  after=268517376  delta=268451840  (256.016 MiB)
  [10] page_table: before=393768  after=541440  delta=147672  (0.141 MiB)
  [11] phys_footprint: before=3981864  after=272581376  delta=268599512  (256.156 MiB)
  [14] external: before=8028160  after=8093696  delta=65536  (0.062 MiB)
  [19] pages_grabbed: before=287  after=16692  delta=16405  (0.016 MiB)
  [36] graphics_footprint: before=16384  after=268451840  delta=268435456  (256.000 MiB)
  currentAllocatedSize:  before=81920  after=268517376  delta=268435456  (256.000 MiB)
  task_info phys_footprint:  before=3981864  after=272581376  delta=268599512  (256.156 MiB)
  task_vm_info ledger_tag_graphics_footprint:  before=16384  after=268451840  delta=268435456  (256.000 MiB)

== Phase 3 (buf.didModifyRange(0..<256MiB) called) ==
  [phase3] currentAllocatedSize=268517376 B (256.078 MiB)
  [phase3] phys_footprint=272597760 B (259.969 MiB)
  [phase3] ledger_tag_graphics_footprint=268451840 B (256.016 MiB)

DELTA: Phase 2 -> Phase 3 (didModifyRange)
---------------------------------------------------------------
  [3] phys_mem: before=280051712  after=280068096  delta=16384  (0.016 MiB)
  [6] internal: before=271958016  after=271974400  delta=16384  (0.016 MiB)
  [11] phys_footprint: before=272581376  after=272597760  delta=16384  (0.016 MiB)
  [61] logical_writes: before=12288  after=32768  delta=20480  (0.020 MiB)
  currentAllocatedSize:  before=268517376  after=268517376  delta=0  (0.000 MiB)
  task_info phys_footprint:  before=272581376  after=272597760  delta=16384  (0.016 MiB)
  task_vm_info ledger_tag_graphics_footprint:  before=268451840  after=268451840  delta=0  (0.000 MiB)

== Phase 4 (allocate 256 MiB storageModePrivate) ==
  [phase4] currentAllocatedSize=537346048 B (512.453 MiB)
  [phase4] phys_footprint=272958208 B (260.313 MiB)
  [phase4] ledger_tag_graphics_footprint=268517376 B (256.078 MiB)

DELTA: Phase 3 -> Phase 4 (allocate private buffer, no blit yet)
---------------------------------------------------------------
  [2] tkm_shared: before=-147456  after=-294912  delta=-147456  (-0.141 MiB)
  [3] phys_mem: before=280068096  after=281280512  delta=1212416  (1.156 MiB)
  [6] internal: before=271974400  after=272302080  delta=327680  (0.312 MiB)
  [7] iokit_mapped: before=98304  after=114688  delta=16384  (0.016 MiB)
  [8] alternate_accounting: before=268517376  after=268582912  delta=65536  (0.062 MiB)
  [11] phys_footprint: before=272597760  after=272941824  delta=344064  (0.328 MiB)
  [14] external: before=8093696  after=8978432  delta=884736  (0.844 MiB)
  [36] graphics_footprint: before=268451840  after=268517376  delta=65536  (0.062 MiB)
  currentAllocatedSize:  before=268517376  after=537346048  delta=268828672  (256.375 MiB)
  task_info phys_footprint:  before=272597760  after=272958208  delta=360448  (0.344 MiB)
  task_vm_info ledger_tag_graphics_footprint:  before=268451840  after=268517376  delta=65536  (0.062 MiB)

== Phase 5 (blit copy shared -> private, forces private residency) ==
  [phase5] currentAllocatedSize=537346048 B (512.453 MiB)
  [phase5] phys_footprint=618464048 B (589.813 MiB)
  [phase5] ledger_tag_graphics_footprint=612843520 B (584.453 MiB)

DELTA: Phase 4 -> Phase 5 (blit copy forces private residency)
---------------------------------------------------------------
  [1] tkm_private: before=524288  after=557056  delta=32768  (0.031 MiB)
  [3] phys_mem: before=281280512  after=283033600  delta=1753088  (1.672 MiB)
  [6] internal: before=272302080  after=273580032  delta=1277952  (1.219 MiB)
  [8] alternate_accounting: before=268582912  after=268697600  delta=114688  (0.109 MiB)
  [10] page_table: before=541440  after=574256  delta=32816  (0.031 MiB)
  [11] phys_footprint: before=272941824  after=618464048  delta=345522224  (329.516 MiB)
  [14] external: before=8978432  after=9453568  delta=475136  (0.453 MiB)
  [19] pages_grabbed: before=16726  after=38467  delta=21741  (0.021 MiB)
  [21] pages_grabbed_iopl: before=1  after=21551  delta=21550  (0.021 MiB)
  [36] graphics_footprint: before=268517376  after=612843520  delta=344326144  (328.375 MiB)
  [44] est_reclaimable: before=4194304  after=4145152  delta=-49152  (-0.047 MiB)
  [61] logical_writes: before=32768  after=155652  delta=122884  (0.117 MiB)
  currentAllocatedSize:  before=537346048  after=537346048  delta=0  (0.000 MiB)
  task_info phys_footprint:  before=272941824  after=618464048  delta=345505840  (329.500 MiB)
  task_vm_info ledger_tag_graphics_footprint:  before=268517376  after=612843520  delta=344326144  (328.375 MiB)

== Phase 6 (post-blit snapshot; shared buffer still in ARC scope) ==
  [phase6] currentAllocatedSize=537346048 B (512.453 MiB)
  [phase6] phys_footprint=618464048 B (589.813 MiB)
  [phase6] ledger_tag_graphics_footprint=612843520 B (584.453 MiB)

DELTA: Phase 5 -> Phase 6 (steady state)
---------------------------------------------------------------
  (no ledger entry moved by >= 1 page = 16384 bytes)
  currentAllocatedSize:  before=537346048  after=537346048  delta=0  (0.000 MiB)
  task_info phys_footprint:  before=618464048  after=618464048  delta=0  (0.000 MiB)
  task_vm_info ledger_tag_graphics_footprint:  before=612843520  after=612843520  delta=0  (0.000 MiB)

== SUMMARY TABLE ==
Phase | currentAllocatedSize | phys_footprint | ledger_tag_graphics | gfx_ledger[36]
Phase0_baseline | 81920 | 3981864 | 16384 | 16384
Phase1_alloc_shared_nowrite | 268517376 | 3998248 | 16384 | 16384
Phase2_write_every_byte | 268517376 | 272581376 | 268451840 | 268451840
Phase3_didModifyRange | 268517376 | 272597760 | 268451840 | 268451840
Phase4_alloc_private | 537346048 | 272958208 | 268517376 | 268517376
Phase5_blit_committed | 537346048 | 618464048 | 612843520 | 612843520
Phase6_post_blit | 537346048 | 618464048 | 612843520 | 612843520

== All nonzero ledger entries at Phase 2 (after write) ==
(Index 0 = cpu_time in ns, skipped)
  [1] tkm_private: balance=524288
  [2] tkm_shared: balance=-147456
  [3] phys_mem: balance=280051712
  [6] internal: balance=271958016
  [7] iokit_mapped: balance=98304
  [8] alternate_accounting: balance=268517376
  [10] page_table: balance=541440
  [11] phys_footprint: balance=272581376
  [14] external: balance=8093696
  [16] purgeable_nonvolatile: balance=49152
  [19] pages_grabbed: balance=16692
  [20] pages_grabbed_kern: balance=20
  [21] pages_grabbed_iopl: balance=1
  [22] pages_grabbed_upl: balance=1
  [36] graphics_footprint: balance=268451840
  [44] est_reclaimable: balance=4194304
  [46] interrupt_wakeups: balance=1
  [58] cpu_time_billed_to_me: balance=960812
  [61] logical_writes: balance=12288
  [64] energy_billed_to_me: balance=18695668

Done (exit 0).
```
