# Dogfooding report (from askesis/canvas): `cargo install hypomnesis` installs no binary, and most `?` rows are nameable without elevation

**Date:** 2026-08-03
**Reporter:** askesis `canvas` — the 38M scale-up (12L/512d/8H, fp32) on a local RTX 5060 Ti 16 GiB (Windows 11 / WDDM) and a rented RTX 5090 32 GiB (Linux, vast.ai), `hmn 0.2.7`
**Severity:** One **install-path defect** that silenced the tool on a box billing by the hour, one **name-resolution** finding with a concrete non-elevated fix, one alias request — plus field validation of `watch` deciding a real engineering question in one command
**Affected area:** `Cargo.toml` feature defaults; `hmn ps` / `hmn watch` name resolution (`OpenProcess`); `hmn ps --sort` value set
**Status:** Proposed — v0.2.8 candidates

---

## TL;DR

`hmn watch` did the job it exists for: it settled a **leak-versus-ceiling** question about a
38M-parameter training run in a single command, and the model built from its numbers then
predicted a VRAM spill to the gigabyte. That part is unqualified success and is written up in
§4.

The report exists for what happened *around* it. **`cargo install hypomnesis` completes
successfully and installs no binary at all** — `hmn` is gated behind a default-off `cli`
feature — so a rented 5090 ran a multi-hour training job with no VRAM census, and the fact
that hypomnesis has a perfectly good Linux/`NVML` per-process path never came into play. This
is the second time the same default-off feature has cost us something; the first produced a
stale `0.2.6` binary at home while the source said `0.2.7`.

And the long-standing `?` rows turn out to be **mostly avoidable without elevation**: a
non-elevated `Get-Process` named the very PID `hmn ps` rendered as `?`, which points at the
access right rather than at privilege.

---

## 1. `cargo install hypomnesis` exits 0 and installs nothing

Observed on the rented box while provisioning it for a training run. The deploy script ran
`cargo install hypomnesis`, the compile succeeded, and `hmn --version` then failed. The
install log says exactly why:

```
   Compiling hypomnesis v0.2.7
    Finished `release` profile [optimized] target(s) in 2.61s
warning: none of the package's binaries are available for install using the selected features
  bin "hmn" requires the features: `cli`
```

`Cargo.toml` confirms it: `default = ["nvml", "nvidia-smi-fallback", "dxgi", "pdh", "metal"]`
— every data source is on by default, and the program that reads them is not.

**Consequences we actually paid.** The run's own launcher logs
`run_stages.sh: line 96: hmn: command not found`, and the job proceeded — correctly, because
we guard that call — but blind: no per-PID residency, no spill census, for a job whose whole
point was to be watched. Worse, it hid a capability: `nvmlDeviceGetComputeRunningProcesses_v3`
is documented in `gpu/mod.rs` as the **Linux primary** source, so `hmn ps` should work well on
exactly this class of rented box.

**Resolved mid-run, and the Linux path is excellent.** Installed with
`cargo install hypomnesis --features cli` while the training job kept running:

```
$ hmn
GPU 0 [NVIDIA GeForce RTX 5090]: free 20592 MiB / 32607 MiB (487 MiB reserved)
$ hmn ps --sort dedicated
PID    NAME    VRAM      SHARED  DEVICE
14374  canvas  11.2 GiB  0 MiB   NVIDIA GeForce RTX 5090
hmn: 1 GPU process found (11.2 GiB committed total).
```

One process, correctly named, correct footprint — and it cross-validates the Windows reading
of the *same* configuration (10.9 GiB at batch 64 locally, 11.2 GiB here), which is a
reassuring agreement between two entirely different data sources (`PDH`/`DXGI` vs `NVML`).
This is now the evidence behind our `remote.sh status`, which reports per-PID residency as
positive proof of a live CUDA context rather than inferring it from a process list.

**One deployment note worth a FAQ entry.** `cargo install` puts `hmn` in `~/.cargo/bin`, which
a **non-interactive** `ssh` does not have on `PATH` — so a remote `command -v hmn` fails on a
box where hmn is installed and working, and any script that probes that way will report the
tool absent. The caller must `. "$HOME/.cargo/env"` first. Not a hypomnesis defect, but it is
the second time on this box that "missing" actually meant "off `PATH` in this shell" (the
first being torch inside `/venv/main`), so it is worth one line in the docs for anyone
scripting hmn on rented hardware.

**Proposal (v0.2.8): make `cli` a default feature.** Library-only consumers pass
`--no-default-features` or select sources explicitly, which is the conventional polarity for a
crate that ships a tool — and it matches the crate's own instincts everywhere else, since the
*sources* are all default-on. A crate whose purpose is observability should not have a silent
no-op as its most obvious install command.

*Secondary:* `cargo install` warns and exits `0`. If defaults are not changed, consider a
`build.rs` note or a README banner directly above the install line, because the warning is
printed before a wall of compile output and is read by nobody.

---

## 2. Most `?` rows are nameable without elevation — and the residue is a display question

`hmn ps --sort dedicated` on the local box:

```
PID    NAME                         VRAM     SHARED  DEVICE
16044  ?                            888 MiB  4 MiB   NVIDIA GeForce RTX 5060 Ti
17700  Zed.exe                      218 MiB  4 MiB   NVIDIA GeForce RTX 5060 Ti
...
hmn: 18 GPU processes found (1.7 GiB committed total; 2 protected — re-run elevated for names).
```

The top VRAM holder on the machine was anonymous. From the **same non-elevated shell**, a
plain `Get-Process -Id 16044` returned `dwm` immediately. So the name was available; the tool
declined to fetch it.

The cause looks like the access right, not privilege. `hmn.rs` resolves names via
`OpenProcess`, whose `PROCESS_QUERY_INFORMATION` right is denied for other-session, `SYSTEM`
and `PPL` processes. Two lighter routes exist, both non-elevated:

1. **Process-enumeration snapshot** — `CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS)` +
   `Process32FirstW/NextW` yields `szExeFile` for **every** process with no per-process handle
   at all. This is how `tasklist` and `Get-Process` name protected processes.
2. **`OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION)` + `QueryFullProcessImageNameW`** —
   documented to succeed where the full query right is refused.

Either would collapse most `?` rows. Elevation would remain relevant only for full image
*paths*, which `hmn ps` does not display.

**The Linux side is a control experiment for this claim.** On the rented box, `hmn ps` named
our process (`canvas`) with no privilege at all, because `/proc/<pid>/comm` is world-readable
and needs no handle. Same tool, same release, same question — the difference is purely which
API the platform path uses to fetch a name. That is a second argument that the Windows `?` is
a choice of access right rather than a privilege wall.

**On the display question, which is the part that survives the fix.** Some rows will always
resist: PIDs that exited between the VRAM sample and the name lookup, and genuinely opaque
system pseudo-processes. hypomnesis already has the right convention for this and uses it —
`PID 4` renders as `[kernel]`, not `?`. Extending that bracket idiom keeps the `ps` look while
never lying:

```
PID    NAME              VRAM
16044  dwm                888 MiB     <- resolved by snapshot, no elevation
    4  [kernel]             4 MiB     <- existing convention
23108  [exited]            61 MiB     <- sampled, then gone before lookup
 9002  [protected]         12 MiB     <- genuinely refused; elevation would help
```

The brackets say "this is a state, not a name", which is exactly what `ps` does with
`[kthreadd]`. The footer hint stays, but it should then count only true `[protected]` rows —
today it counts everything unresolved and so overstates what elevation would buy.

---

## 3. `hmn ps --sort vram` should be an alias

```
error: invalid value 'vram' for '--sort <KEY>'
  [possible values: dedicated, shared, total]
```

The error is good — it lists the valid values — but `vram` is the word the rest of the tool
uses (`VRAM` is the column header, and the help text says "per-PID VRAM"). Accepting it as an
alias for `dedicated` costs one match arm and removes a round trip. Same argument for
`committed`, which is the word `watch` prints.

---

## 4. What worked: `watch` decided a real question in one command

The 38M model spilled at batch 96 on the 16 GiB card. The question that mattered was whether
the fused optimizer and EMA paths we had just written were **leaking**, or whether the model
simply did not fit — a leak would have invalidated a day of measurements and a correctness
claim, not just an estimate.

```
hmn watch --follow-new --interval 3s --duration 95s
```

Twenty-one samples over a 200-step run at batch 64: `COMMITTED 10.9 GiB`, **`ΔCOMMIT +0 B` on
every single sample**, `SHARED 78 MiB` flat, `SPILL no`, closing summary `episodes 0 — no
spill observed`. Flat is a much stronger statement than "looks fine", and it took one command
and ninety-five seconds to make it.

The numbers then did second duty. Subtracting the known fixed state (params + grads + Adam
moments + EMA shadow = 0.71 GiB) from 10.9 GiB gives **163 MiB of activations per row**, which
predicts 16.0 GiB at batch 96 against ~15.0 GiB free — i.e. exactly the spill observed, and
21.1 GiB at batch 128, i.e. the OOM observed. An instrument that supports arithmetic like that
is doing more than reporting.

Two smaller things that earned their design:

- **`--follow-new` announced its own set change** (`followed set changed: entered pid=10216;
  left pid=27620 (canvas.exe)`) rather than silently swapping rows. That is the v0.2.7 feature
  requested in the previous report, and it behaved.
- **The help text refuses to overclaim**: "a watched PID that stops appearing renders as 0
  bytes each interval — `hmn watch` does not distinguish the two". Tools that name their own
  ambiguities are the ones worth trusting with a measurement.

---

## Suggested priority for v0.2.8

| | Item | Effort | Why it ranks here |
|---|---|---|---|
| 1 | `cli` becomes a default feature | trivial | The install path is currently a silent no-op; it has now cost two sessions |
| 2 | Name resolution via process snapshot | small | Removes most `?` rows with no elevation; turns a privilege story into a display story |
| 3 | `[exited]` / `[protected]` bracket rendering + honest footer count | small | Keeps the `ps` look, never lies, and follows the existing `[kernel]` precedent |
| 4 | `--sort vram` / `committed` aliases | trivial | The tool's own vocabulary should be accepted |

Nothing here touches the measurement core, which has now been correct in every field use we
have put it to.
