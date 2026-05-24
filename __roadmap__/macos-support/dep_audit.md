# libSystem-only dep audit

**Goal**: Confirm by direct inspection that every macOS measurement can be delivered via libSystem syscalls (`task_info`, `ledger`, `sysctlbyname`, `proc_listpids`, `proc_pidpath`) without adding any third-party Apple-framework crate, and capture the audit evidence for the PR description.
**Pre-conditions**:
- [ ] Worktree is on `claude/charming-moore-e60495` (off `main` at v0.2.1)
- [ ] [R01 Knowledge Transfer v3](../../__reports__/macos_ledger/09-knowledge_transfer_v3.md) and [R02 Round 05](../../__reports__/macos_ledger/05-findings_writes_v0.md) read
**Success Gates**:
- ⬜ [static] A short markdown note (committed under `__reports__/macos_ledger/10-dep_audit_v0.md`) lists each required syscall, its libSystem availability, and the crates.io result for any safe-wrapper alternative. The note concludes with the explicit statement "no `objc2`, no `objc2-metal`, no Apple-framework crate required for v0.2.2".
- ⬜ [static] The note answers the five [R01 Open Questions for the Next Cycle] inline (libSystem-only sufficiency, no-`CONVENTIONS.md`-modification path, `libproc` crate evaluation, MSRV-vs-`LEDGER_ENTRY_INFO_V2` compatibility, `sysctlbyname` access mode).
**References**: [R01 §Dependencies — libSystem-only](../../__reports__/macos_ledger/09-knowledge_transfer_v3.md), [R01 §Open Questions for the Next Cycle](../../__reports__/macos_ledger/09-knowledge_transfer_v3.md)

## Step 1: Audit each required syscall against crates.io safe wrappers

**Goal**: Produce a one-row-per-syscall feasibility matrix proving that every measurement is reachable through `unsafe extern "C"` from libSystem and that no safe-wrapper crate eliminates the `unsafe` block.
**Implementation Logic**:
For each syscall used in [R01 §Reference Design] — `task_info(TASK_VM_INFO)`, `ledger(LEDGER_INFO|LEDGER_TEMPLATE_INFO|LEDGER_ENTRY_INFO_V2)`, `sysctlbyname("hw.memsize"|"machdep.cpu.brand_string")`, `proc_listpids(PROC_ALL_PIDS)`, `proc_pidpath` — open crates.io and check `libc`, `mach2`, `nix`, `libproc`, `darwin-libproc`. Record for each: (a) is a safe wrapper available; (b) if yes, does it still require `unsafe { }` at the call site; (c) cite the crate version inspected. The libSystem-only verdict holds if and only if every row in column (b) is "still requires `unsafe`" OR no wrapper exists.
**Deliverables**: `__reports__/macos_ledger/10-dep_audit_v0.md` containing: (a) feasibility matrix with columns `syscall`, `libSystem extern signature`, `safe-wrapper crate (if any)`, `still requires unsafe?`, `verdict`; (b) §`CONVENTIONS.md` impact section answering the alternative-path question; (c) §`LEDGER_ENTRY_INFO_V2` availability section citing open-source XNU `osfmk/kern/ledger.h` at the MSRV-compatible tag; (d) final verdict line "no `objc2`, no `objc2-metal`, no Apple-framework crate required for v0.2.2"
**Consistency Checks**: `test -s __reports__/macos_ledger/10-dep_audit_v0.md && grep -q "still requires unsafe" __reports__/macos_ledger/10-dep_audit_v0.md` (expected: PASS)
**Commit**: `docs(macos): audit libSystem-only feasibility for macOS measurements`
