// SPDX-License-Identifier: MIT OR Apache-2.0

//! `hmn` — GPU memory CLI for `hypomnesis`, built via the (since v0.2.8)
//! default-on `cli` feature.
//!
//! Subcommands:
//!
//! - `hmn` (default) — one line per visible GPU with free / total `VRAM`.
//!   Uses [`hypomnesis::Snapshot::all`], so on Windows the AMD / Intel
//!   `iGPU` surfaces alongside the NVIDIA dGPU(s); on macOS the Apple
//!   Silicon `SoC` surfaces as a single `UMA` device. `--json` emits the
//!   same data as a JSON array instead of text.
//! - `hmn ps` — list processes holding GPU memory across one or all
//!   visible devices. On Linux (`NVML`) the list is compute-only; on
//!   Windows (`PDH`, `WDDM 2.0`+) the list includes every GPU memory
//!   holder (compositor, browsers, games, compute); on macOS (Metal
//!   ledger) the list enumerates every same-user PID holding
//!   `graphics_footprint` bytes. See the `--help` Limitations text and
//!   the rustdoc for [`hypomnesis::gpu_processes`] for the
//!   per-platform breakdown.
//! - `hmn spill -- <command>` — run a command while polling
//!   [`hypomnesis::SpillTracker`] (`time(1)`-style wrapper, default
//!   100 ms interval), print a `SpillReport` to stderr when the
//!   command exits, and pass its exit code through. Windows /
//!   `WDDM`-only measurement; on other platforms the command still
//!   runs and the report says "spill not measurable".
//! - `hmn watch [PID...]` — attach to already-running PID(s) (or
//!   auto-select the top-N by committed VRAM) and sample
//!   [`hypomnesis::SpillTracker`] plus per-PID committed/shared VRAM on a
//!   timer, printing one row per PID per interval with deltas and the
//!   shipped v0.2.5 SPILL condition. `time(1)`-style scrolling sampler —
//!   not a TUI, same discipline as `hmn spill`. Exit code conveys whether
//!   spill was observed during the watch, for scripts/watchdogs.
//!
//! Install with `cargo install hypomnesis` (the `cli` feature is
//! default-on since v0.2.8; `--features cli` is still accepted but
//! redundant).

use std::collections::HashMap;
use std::fmt::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use clap::{Parser, Subcommand, ValueEnum};
use hypomnesis::{
    GpuProcessEntry, Result, Snapshot, SpillEpisode, SpillReport, SpillTracker, device_count,
    device_info, gpu_processes,
};

/// `hmn` CLI: device summary plus GPU-process listing.
#[derive(Parser, Debug)]
#[command(
    name = "hmn",
    version,
    about = "GPU memory CLI: device summary (default) + GPU-process listing (`hmn ps`).",
    long_about = "GPU memory CLI for hypomnesis.\n\
                  \n\
                  Default subcommand: prints one line per visible GPU with free / total VRAM. \
                  `--json` emits the same data as a JSON array instead (fields: index, name, \
                  total_bytes, free_bytes, used_bytes, reserved_bytes, driver_version).\n\
                  \n\
                  `hmn ps`: lists processes holding GPU memory.\n\
                  \n\
                  `hmn spill -- <command>`: runs a command while sampling WDDM spill state \
                  (resident shared-memory growth under dedicated-VRAM saturation), prints a \
                  SpillReport to stderr on exit, and passes the command's exit code through.\n\
                  \n\
                  `hmn watch [PID...]`: attaches to already-running PID(s) (or auto-selects the \
                  top `--top` processes by committed VRAM when none are given) and samples spill \
                  state plus per-PID VRAM on a timer, printing one row per PID per interval with \
                  deltas. Not a TUI — a scrolling time(1)-style sampler, same discipline as \
                  `hmn spill`. Exit code conveys whether spill was observed, for scripts/watchdogs.\n\
                  \n\
                  Limitations (per-platform):\n\
                  - Linux / NVML backend is compute-only — only processes with an active CUDA \
                  context appear. Browsers using GPU compositing, games, and pure-graphics \
                  apps do not.\n\
                  - Windows / PDH backend (consumer WDDM 2.0+) lists EVERY GPU memory holder: \
                  the desktop compositor, browsers, games, and CUDA / compute alongside. The \
                  semantic shift from the Linux compute-only list is intentional and reflects \
                  what `VidMm` actually accounts for.\n\
                  - Windows `used_bytes` reflects WDDM's dedicated commit, not resident set. \
                  Under WDDM a process can commit GPU allocations exceeding physical VRAM — \
                  the kernel pages them via the shared system memory budget. Numbers \
                  exceeding the device's total VRAM are real, not bugs; they match Task \
                  Manager's `Dedicated GPU memory` column.\n\
                  - The SHARED column (Windows / PDH only) shows resident shared-system-memory \
                  bytes — the WDDM spill signal, matching Task Manager's `Shared GPU memory` \
                  column. A benign baseline (staging/upload heaps) is normal; growth while \
                  dedicated VRAM saturates is spill. Always 0 on Linux and macOS (no \
                  shared-residency counter exists there).\n\
                  - On Windows, `?` in the NAME column is now rare (since v0.2.8): a \
                  `CreateToolhelp32Snapshot` fallback (the same mechanism `Get-Process`/Task \
                  Manager use) resolves most PIDs `OpenProcess` can't, including ordinary \
                  foreign-user/SYSTEM processes like `dwm.exe`/`csrss.exe`, non-elevated. \
                  What remains renders as `[exited]` (the process exited between the VRAM \
                  sample and the name lookup — elevation would not help) or `[protected]` \
                  (the snapshot fallback itself could not be taken — very rare; re-run \
                  elevated). The Windows kernel itself (PID 4) renders as `[kernel]`, not \
                  `?` or `[protected]`. This distinction is Windows-only; Linux/macOS \
                  unresolved rows remain a bare `?`/absent name — run as the owning user \
                  or with `sudo` there.\n\
                  - Security note: a `[protected]` row (or a bare `?` on Linux/macOS) that \
                  does not resolve under elevation is worth investigating — by construction \
                  it is either a process owned by another user, a process running as \
                  SYSTEM/LOCAL SERVICE/NETWORK SERVICE, a PPL-protected process, or (rarely) \
                  the snapshot API itself failing. None of these are intrinsically \
                  malicious, but on a single-user desktop an unexpected one holding \
                  substantial VRAM is worth investigating. The summary line's protected-count \
                  parenthetical counts `[protected]`/absent-name/the rare nvidia-smi-fallback \
                  literal `?` — not `[exited]`, since elevation can't help a process that's \
                  already gone.\n\
                  - Pre-WDDM-2.0 Windows falls back to `nvidia-smi --query-compute-apps`, \
                  which is compute-only and may show `[N/A]` memory under consumer WDDM \
                  (parser drops those rows).\n\
                  - The R570 u64::MAX sentinel and used > total checks are applied per-row \
                  on NVIDIA backends; affected rows are dropped rather than reported as \
                  garbage.\n\
                  - macOS: `used_bytes` reflects currently-resident GPU pages \
                  (`graphics_footprint` ledger entry); the kernel evicts idle Metal pages, \
                  so the same PID may report different values across calls. Same \
                  resident-bytes semantics as Windows `WorkingSetSize` and Linux `VmRSS`.\n\
                  - macOS: cross-user PIDs are silently skipped — the per-PID `ledger` \
                  syscall returns `EPERM` for processes owned by another user. To list \
                  every PID on the system, run elevated (`sudo hmn ps`)."
)]
struct Cli {
    /// Subcommand. Omitted for the default device-summary view.
    #[command(subcommand)]
    command: Option<Commands>,
    /// Emit the default device-summary view as a JSON array instead of
    /// text. Only meaningful with no subcommand — each subcommand
    /// (`ps`/`spill`/`watch`) already has its own `--json`, and combining
    /// this with one (e.g. `hmn --json ps`) is a hard error (exit `2`)
    /// rather than being silently ignored. One object per visible GPU:
    /// `index` (number), `name` (string or null),
    /// `total_bytes`/`free_bytes`/`used_bytes` (numbers), `reserved_bytes`
    /// (number or null), `driver_version` (string or null).
    #[arg(long)]
    json: bool,
}

/// Subcommand tree for `hmn`.
#[derive(Subcommand, Debug)]
enum Commands {
    /// List processes holding GPU memory. On Linux: compute-only via
    /// NVML. On Windows / WDDM 2.0+: every GPU memory holder via PDH
    /// (compositor, browsers, compute, etc.). On macOS: every
    /// same-user PID holding `graphics_footprint` ledger bytes; run
    /// elevated (`sudo`) to include cross-user PIDs. See `hmn --help`
    /// Limitations for the full per-platform breakdown.
    Ps {
        /// Filter to processes whose PID matches.
        #[arg(long, value_name = "PID")]
        pid: Option<u32>,
        /// Filter to a single GPU index. Default: every device reported
        /// by `device_count()`.
        #[arg(long, value_name = "INDEX")]
        device: Option<u32>,
        /// Display order: `dedicated` ("who do I kill to free VRAM?",
        /// the default), `shared` ("who is currently being paged out?"
        /// — a symptom, not a cause; always a no-op ordering on Linux
        /// and macOS, where `shared_used_bytes` is always 0), or
        /// `total` (dedicated + shared, "who is the biggest GPU-memory
        /// citizen overall?"). Tie-breaks (name ascending, then PID
        /// ascending) are unchanged by this flag.
        #[arg(long, value_name = "KEY", default_value = "dedicated")]
        sort: SortKey,
        /// Emit a JSON array (one object per row) instead of the
        /// default text table. Each object has fields `pid` (number),
        /// `name` (string or null), `used_bytes` (number),
        /// `shared_used_bytes` (number — resident shared bytes, the
        /// WDDM spill signal; 0 off-Windows), `device_index` (number),
        /// `device_name` (string or null). Row order follows `--sort`.
        #[arg(long)]
        json: bool,
    },
    /// Run a command while sampling WDDM spill state; print a
    /// `SpillReport` to stderr when it exits (stdout stays the
    /// wrapped command's). The wrapped command's exit code passes
    /// through.
    ///
    /// Spill = resident shared-system-memory growth while dedicated
    /// VRAM saturates — measurable on Windows / WDDM 2.0+ only. On
    /// Linux and macOS the command still runs, and the report is
    /// replaced by a "spill not measurable on this platform" note.
    ///
    /// Ctrl+C reaches the whole process group: hmn dies with the
    /// wrapped command and the report is lost (recording a partial
    /// report on interrupt is deliberately out of scope for now).
    Spill {
        /// Polling interval in milliseconds (minimum 1). Values below
        /// ~50 ms add PDH query cost without extra resolution (the
        /// GPU counters update on driver cadence) — documented, not
        /// clamped beyond the zero-floor.
        #[arg(long, value_name = "MS", default_value_t = 100, value_parser = clap::value_parser!(u64).range(1..))]
        interval: u64,
        /// GPU index to watch (NVML-canonical ordering).
        #[arg(long, value_name = "INDEX", default_value_t = 0)]
        device: u32,
        /// Also emit the `SpillReport` as a JSON object on stdout
        /// (after the wrapped command's own output; stderr keeps the
        /// human-readable block). Fields: `measurable`, `spilled`,
        /// `observations`, `baseline_shared_bytes`,
        /// `peak_shared_bytes`, `peak_dedicated_bytes`,
        /// `dedicated_limit_bytes`, `total_spill_duration_ms`,
        /// `episodes[]` (`start_label`, `end_label` or null,
        /// `peak_shared_bytes`, `observations`, `duration_ms`).
        /// Check `measurable` before trusting `spilled: false`.
        #[arg(long)]
        json: bool,
        /// The command to run (everything after `--`).
        #[arg(
            trailing_var_arg = true,
            allow_hyphen_values = true,
            required = true,
            value_name = "COMMAND"
        )]
        command: Vec<String>,
    },
    /// Attach to already-running PID(s) and sample WDDM spill state plus
    /// per-PID VRAM on a timer; one row per watched PID per interval,
    /// with deltas and the shipped v0.2.5 SPILL condition. Not a TUI —
    /// a scrolling time(1)-style sampler, same discipline as `hmn spill`.
    ///
    /// With no PID given, auto-selects the top `--top` processes by
    /// committed VRAM from the first sample and keeps that fixed set for
    /// the run (or re-selects every interval with `--follow-new`). A
    /// watched PID that stops appearing in the per-process listing
    /// (exited, or simply holds no GPU memory right now) renders as 0
    /// bytes each interval — `hmn watch` does not distinguish the two;
    /// it does not auto-stop on this basis, use `--duration` or Ctrl+C.
    /// If the OS recycles a watched PID onto a different process
    /// mid-watch, a resolved-name change is used as a best-effort signal
    /// to reset that row's baseline rather than mixing two processes'
    /// readings.
    ///
    /// Runs until `--duration` elapses or Ctrl+C, printing a closing
    /// summary (adapter-level `SpillReport` plus per-PID peak/baseline) and
    /// exiting `0` if spill was never observed, `1` if it was at least
    /// once, `2` on a hard error (bad device, nothing to auto-select, or
    /// `--follow-new` combined with explicit PID(s)).
    Watch {
        /// Explicit PID(s) to watch. When omitted, auto-selects the top
        /// `--top` processes by committed VRAM from the first sample.
        #[arg(value_name = "PID")]
        pids: Vec<u32>,
        /// Sampling interval: digits followed by an optional unit (`ms`,
        /// `s`, `m`, `h`); bare digits are seconds. Shorter intervals
        /// catch brief flicker episodes at the cost of more PDH queries.
        #[arg(long, value_name = "DUR", default_value = "5s", value_parser = parse_duration)]
        interval: Duration,
        /// Stop after this long and print the closing summary (same
        /// duration-string format as `--interval`). Omitted: run until
        /// Ctrl+C.
        #[arg(long, value_name = "DUR", value_parser = parse_duration)]
        duration: Option<Duration>,
        /// Number of processes to auto-select by committed VRAM when no
        /// PID is given. Ignored when explicit PID(s) are passed.
        #[arg(long, value_name = "N", default_value_t = 5)]
        top: usize,
        /// Auto-select mode only: re-run the top-`--top` selection every
        /// interval instead of once at attach. A PID entering the
        /// followed set starts fresh (baseline = first sighting); a PID
        /// dropping out (exited, or fell below rank `--top`) simply
        /// stops appearing in the live rows and is finalized into the
        /// closing summary's `per_pid[]` with its peak/baseline, instead
        /// of rendering `0` rows forever. An empty first sample is not
        /// an error under this flag — the watch just starts empty and
        /// picks up work as it appears, which is the point. Combining
        /// this with explicit PID(s) on the command line is a hard
        /// error (exit `2`): there is no top-N to re-run against a fixed
        /// list, and explicit PIDs are watched exactly as given.
        #[arg(long)]
        follow_new: bool,
        /// GPU index to watch (NVML-canonical ordering).
        #[arg(long, value_name = "INDEX", default_value_t = 0)]
        device: u32,
        /// Emit JSON Lines to stdout instead of a text table: one
        /// `{"kind":"sample",...}` object per PID per interval as it
        /// happens, plus a final `{"kind":"summary",...}` object (the
        /// adapter `SpillReport` fields plus a `per_pid[]` peak/baseline
        /// array) when the watch ends. Pipeable to `jq -c` live.
        #[arg(long)]
        json: bool,
    },
}

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    // `--json` before a subcommand parses cleanly (clap has no opinion on
    // the combination) but would otherwise be silently dropped: only the
    // `None` arm below reads `cli.json`, so `hmn --json ps` would print
    // plain text with no warning — a script relying on `--json` landing
    // wherever it's typed would silently get prose instead of JSON.
    // Reject the combination loudly instead, same "hard error, exit 2"
    // convention as `watch --follow-new` + explicit PIDs below.
    if cli.json && cli.command.is_some() {
        eprintln!(
            "hmn: --json before a subcommand is ignored, not applied — each subcommand has \
             its own --json (e.g. `hmn ps --json`, not `hmn --json ps`)"
        );
        return std::process::ExitCode::from(2);
    }
    let outcome = match cli.command {
        None => run_summary(cli.json),
        Some(Commands::Ps {
            pid,
            device,
            sort,
            json,
        }) => run_ps(pid, device, sort, json),
        // `spill` bypasses the Ok/Err fold below: its exit code is the
        // wrapped command's, passed through — not hmn's own
        // success/failure.
        Some(Commands::Spill {
            interval,
            device,
            json,
            command,
        }) => return run_spill(interval, device, json, &command),
        // `watch` also bypasses the Ok/Err fold: its exit code conveys
        // whether spill was observed, not hmn's own success/failure.
        Some(Commands::Watch {
            pids,
            interval,
            duration,
            top,
            follow_new,
            device,
            json,
        }) => return run_watch(&pids, interval, duration, top, follow_new, device, json),
    };
    match outcome {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("hmn: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

// -----------------------------------------------------------------------------
// Summary subcommand
// -----------------------------------------------------------------------------

/// Whether `snaps` is non-empty but every entry's `gpu_device` is
/// `None` — devices enumerated, but `device_info` failed for each (e.g.
/// a partial driver install). Shared by both [`run_summary`] paths:
/// [`format_summary`] and [`format_summary_json`] both silently skip
/// exactly these entries, so `[]`/no-output is otherwise indistinguishable
/// from genuine zero-device enumeration.
fn all_gpu_devices_unreadable(snaps: &[Snapshot]) -> bool {
    !snaps.is_empty() && snaps.iter().all(|s| s.gpu_device.is_none())
}

/// The "devices enumerated but none readable" diagnostic line, shared by
/// both [`run_summary`] call sites (stdout in text mode, stderr in
/// `--json` mode) so the wording can't drift between the two.
fn format_none_readable_message(count: usize) -> String {
    format!("hmn: {count} GPU(s) enumerated but none readable.")
}

/// Run the default subcommand: print one line per visible GPU (or, with
/// `--json`, a JSON array).
fn run_summary(json: bool) -> Result<()> {
    let snaps = Snapshot::all()?;
    if json {
        // The `[]` shape is identical whether zero devices were visible
        // or every visible device failed to read — distinguish the two
        // on stderr (mirrors `hmn ps`'s always-on stderr summary line)
        // without changing the documented JSON array shape on stdout.
        if all_gpu_devices_unreadable(&snaps) {
            eprintln!("{}", format_none_readable_message(snaps.len()));
        }
        print!("{}", format_summary_json(&snaps));
        return Ok(());
    }
    if snaps.is_empty() {
        println!("hmn: no visible GPUs.");
        return Ok(());
    }
    if all_gpu_devices_unreadable(&snaps) {
        // Printing nothing here would exit `0` with empty stdout,
        // indistinguishable from a working "nothing to show" success —
        // say so explicitly instead.
        println!("{}", format_none_readable_message(snaps.len()));
        return Ok(());
    }
    print!("{}", format_summary(&snaps));
    Ok(())
}

/// Format the device summary, one line per snapshot that has a populated
/// `gpu_device`. Snapshots without a `gpu_device` (e.g. RAM-only entries)
/// are skipped.
#[allow(clippy::missing_panics_doc)] // writes to a String; cannot fail in practice
fn format_summary(snaps: &[Snapshot]) -> String {
    let mut out = String::new();
    for snap in snaps {
        let Some(dev) = &snap.gpu_device else {
            continue;
        };
        let free_mib = bytes_to_mib(dev.free_bytes);
        let total_mib = bytes_to_mib(dev.total_bytes);
        // BORROW: explicit Option::as_deref + map_or — name is
        // Option<String>; we need an owned suffix String.
        let name_suffix = dev
            .name
            .as_deref()
            .map_or(String::new(), |n| format!(" [{n}]"));
        // Driver/firmware carve-out, when the backend surfaced it (NVML
        // R510+). It is a *subset* of `total_mib` (NVML's
        // `total = reserved + free + used`), so the parenthetical reads as
        // "of which N is reserved", not an addition on top — matching
        // `nvidia-smi -q -d MEMORY`'s separate `Total` / `Reserved` lines.
        // Elided on backends that report `None` (DXGI, nvidia-smi, Metal,
        // pre-R510).
        let reserved_suffix = dev.reserved_bytes.map_or(String::new(), |r| {
            format!(" ({} MiB reserved)", bytes_to_mib(r))
        });
        // NVIDIA driver version (NVML or nvidia-smi fallback). Elided on
        // backends that don't expose an NVIDIA driver string (DXGI,
        // Metal, non-NVIDIA adapters).
        let driver_suffix = dev
            .driver_version
            .as_deref()
            .map_or(String::new(), |v| format!(", driver {v}"));
        // `writeln!` into a String never fails — the writes-to-String
        // impl returns Ok(()). Same for every other write!/writeln! in
        // this file.
        let _ = writeln!(
            out,
            "GPU {}{name_suffix}: free {free_mib} MiB / {total_mib} MiB{reserved_suffix}{driver_suffix}",
            dev.index,
        );
    }
    out
}

/// Format the device summary as a JSON array, one object per snapshot
/// that has a populated `gpu_device` (mirrors [`format_summary`]'s
/// skip rule for snapshots without one). Hand-rolled (no `serde` dep —
/// same policy as [`format_ps_json`]). Each object:
/// `{"index":N,"name":<string|null>,"total_bytes":N,"free_bytes":N,"used_bytes":N,"reserved_bytes":<number|null>,"driver_version":<string|null>}`.
/// String values are JSON-escaped via [`json_escape`].
#[allow(clippy::missing_panics_doc)] // writes to a String; cannot fail in practice
fn format_summary_json(snaps: &[Snapshot]) -> String {
    let mut out = String::from("[");
    let mut first = true;
    for snap in snaps {
        let Some(dev) = &snap.gpu_device else {
            continue;
        };
        if first {
            first = false;
        } else {
            out.push(',');
        }
        let name_json = dev.name.as_deref().map_or_else(
            || String::from("null"),
            |n| format!("\"{}\"", json_escape(n)),
        );
        let reserved_json = dev
            .reserved_bytes
            .map_or_else(|| "null".to_owned(), |r| r.to_string());
        let driver_json = dev.driver_version.as_deref().map_or_else(
            || String::from("null"),
            |v| format!("\"{}\"", json_escape(v)),
        );
        let _ = write!(
            out,
            r#"{{"index":{},"name":{name_json},"total_bytes":{},"free_bytes":{},"used_bytes":{},"reserved_bytes":{reserved_json},"driver_version":{driver_json}}}"#,
            dev.index, dev.total_bytes, dev.free_bytes, dev.used_bytes,
        );
    }
    out.push_str("]\n");
    out
}

// -----------------------------------------------------------------------------
// `ps` subcommand
// -----------------------------------------------------------------------------

/// One row of `hmn ps` output (binary-internal — not part of the
/// library's public API).
#[derive(Debug, Clone)]
struct PsRow {
    /// Process ID.
    pid: u32,
    /// Process name. `None` when no name source produced one.
    name: Option<String>,
    /// GPU memory used by this process in bytes (`WDDM` dedicated
    /// commit on the Windows `PDH` path).
    used_bytes: u64,
    /// Resident shared-system-memory bytes — the `WDDM` spill signal.
    /// `0` on non-Windows backends (no shared-residency counter).
    shared_used_bytes: u64,
    /// Zero-based device index (NVML-canonical).
    device_index: u32,
    /// Friendly device name (e.g. `RTX 5060 Ti`); `None` when
    /// `device_info` failed for this index.
    device_name: Option<String>,
}

/// Display-order key for `hmn ps --sort` (and, always pinned to
/// [`Self::Dedicated`], for [`select_top_n_pids`]'s auto-selection).
///
/// Binary-internal dispatch enum, not a library type — matched
/// exhaustively by [`ps_row_comparator`], the sole place that
/// interprets it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum SortKey {
    /// `used_bytes` (`WDDM` dedicated commit) descending — "who do I
    /// kill to free VRAM?". The default; matches `hmn ps`'s pre-v0.2.7
    /// fixed order exactly. Also accepts `vram` (the column header and
    /// the word the rest of the tool's help text uses for this
    /// quantity) and `committed` (the word `hmn watch`'s `COMMITTED`
    /// column uses for the same quantity) as aliases — same ordering,
    /// different vocabulary entry points, so users don't have to learn
    /// `hmn`-internal naming to reach for the default sort.
    #[value(alias = "vram", alias = "committed")]
    Dedicated,
    /// `shared_used_bytes` (resident shared-system-memory, the spill
    /// signal) descending — "who is currently being paged out?". A
    /// symptom, not a cause: a process high in SHARED has already lost
    /// the fight for dedicated VRAM. Always a no-op ordering on Linux
    /// and macOS, where `shared_used_bytes` is always `0`.
    Shared,
    /// `used_bytes + shared_used_bytes` descending — "who is the
    /// biggest GPU-memory citizen overall?". Outweighs `Dedicated` for
    /// processes that hold meaningful shared residency alongside their
    /// dedicated commit.
    Total,
}

/// Build the row comparator for a given [`SortKey`], shared by `hmn ps`
/// (user-selectable via `--sort`) and [`select_top_n_pids`] (always
/// [`SortKey::Dedicated`]) so the two orderings cannot silently drift
/// apart. Tie-breaks (name ascending, then PID ascending — stable
/// output across runs, and clusters duplicate-name processes like
/// `msedgewebview2.exe`) are identical regardless of the primary key.
const fn ps_row_comparator(key: SortKey) -> impl Fn(&PsRow, &PsRow) -> std::cmp::Ordering {
    move |a, b| {
        let primary = match key {
            SortKey::Dedicated => b.used_bytes.cmp(&a.used_bytes),
            SortKey::Shared => b.shared_used_bytes.cmp(&a.shared_used_bytes),
            SortKey::Total => b
                .used_bytes
                .saturating_add(b.shared_used_bytes)
                .cmp(&a.used_bytes.saturating_add(a.shared_used_bytes)),
        };
        primary
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.pid.cmp(&b.pid))
    }
}

/// Run the `ps` subcommand: collect process rows for the selected
/// device(s), apply the `--pid` filter, sort per `--sort`, then emit
/// either a text table or JSON.
//
// Returns `Result<()>` for symmetry with `run_summary` so `main` can
// dispatch through one match arm. The body never produces an `Err` (per-device
// failures are swallowed via `continue` so one broken device doesn't kill the
// whole listing); the lint is allowed for that reason.
#[allow(clippy::unnecessary_wraps)]
fn run_ps(
    pid_filter: Option<u32>,
    device_filter: Option<u32>,
    sort: SortKey,
    json: bool,
) -> Result<()> {
    // device_count returning Err here means no enumeration backend is
    // enabled / every backend failed; treat as zero NVIDIA devices and
    // let the empty Vec fall through to the formatter (which prints
    // a header-only table or `[]`).
    let device_indices: Vec<u32> = device_filter.map_or_else(
        || (0..device_count().unwrap_or(0)).collect(),
        |idx| vec![idx],
    );

    let mut rows: Vec<PsRow> = Vec::new();
    for &idx in &device_indices {
        // Look up the device name once per device for the DEVICE column.
        // Failure here is non-fatal: row's `device_name` falls back to
        // None and the formatter renders `GPU N` instead.
        let device_name = device_info(idx).ok().and_then(|d| d.name);
        let Ok(entries) = gpu_processes(idx) else {
            continue;
        };
        for entry in entries {
            if let Some(want) = pid_filter
                && entry.pid != want
            {
                continue;
            }
            rows.push(PsRow {
                pid: entry.pid,
                name: entry.name,
                used_bytes: entry.used_bytes,
                shared_used_bytes: entry.shared_used_bytes,
                device_index: idx,
                // BORROW: clone — device_name is shared across all
                // rows for this device.
                device_name: device_name.clone(),
            });
        }
    }

    // Human-facing display order, per `--sort` (default: VRAM descending
    // so the biggest consumers land at the top — the row a user asking
    // "what's eating my GPU memory?" wants to see first). Tie-breaks
    // (name ascending for grouping duplicate-name processes like
    // `msedgewebview2.exe`, then PID ascending for stable order across
    // runs) are identical regardless of key — see `ps_row_comparator`.
    // The library's `gpu_processes()` returns rows PID-sorted; this
    // overrides that for display only.
    rows.sort_by(ps_row_comparator(sort));

    if json {
        print!("{}", format_ps_json(&rows));
    } else {
        print!("{}", format_ps_table(&rows));
    }
    // Human-readable summary on stderr — preserves stdout's scriptability
    // (header-only table or `[]` for empty) while giving interactive
    // users an unambiguous "command worked, here's the count" line.
    // Always printed, even when rows is non-empty, so the message is a
    // consistent confirmation rather than an error indicator. Redirect
    // 2>/dev/null to suppress.
    eprintln!(
        "hmn: {}",
        format_ps_summary(&rows, pid_filter, device_filter)
    );
    Ok(())
}

/// Build the stderr summary string for `hmn ps`. Format:
/// `<N> GPU process[es] found[ matching <filters>][ (<X.Y> <unit> committed total[; <M> protected — re-run elevated for names)].`
///
/// Two appendices after the noun, each elided when not applicable:
///
/// - **Filter clause** (` matching pid=N device=M`): appended only
///   when at least one filter is active. Supports any combination of
///   `--pid` and `--device`.
/// - **Committed-total parenthetical** (` (X.Y unit committed total)`,
///   formatted via [`format_vram`] so it renders as `MiB` below 1
///   `GiB` and `GiB` to one decimal place otherwise): appended only
///   when `count > 0`. The word "committed" hints at the `WDDM`
///   commit-vs-resident distinction the Windows `PDH` backend
///   exposes — summing `used_bytes` across processes can exceed
///   physical `VRAM` under `WDDM` (a real `WDDM` property, not a
///   bug), so naming the figure "committed total" prevents that from
///   reading as broken when a Windows user sees, say, 32 `GiB`
///   committed on a 16 `GiB` card. Elided entirely when `count == 0`
///   because a zero-bytes total carries no information.
///
///   When at least one row is genuinely unresolvable, the parenthetical
///   carries a **protected continuation**
///   (`; M protected — re-run elevated for names`) joined by `; `. A row
///   counts as protected when `name.is_none()` (`NVML`'s
///   `/proc/<pid>/comm` unreadable on Linux; macOS cross-user PIDs whose
///   `ledger` syscall returned `EPERM` — `sudo hmn ps` is the equivalent
///   elevation there); when `name` is exactly `Some("[protected]")` (the
///   Windows-only bracket meaning the `Toolhelp32Snapshot` fallback could
///   not be taken at all — see `hypomnesis::gpu_processes`'s Windows
///   path); or when `name` is the literal `Some("?")` string the
///   pre-`WDDM 2.0` `nvidia-smi` fallback writes for a row it couldn't
///   name itself (same "might resolve under elevation" meaning as the
///   other two — pre-existing, but not previously counted here). `PID 4`
///   (`[kernel]`) and `[exited]` rows deliberately
///   do **not** contribute to this count: `[kernel]` has no executable
///   image to resolve regardless of privilege, and `[exited]` means the
///   process was already gone by the time of the name lookup — elevation
///   would not have helped either case, so counting them would overstate
///   what re-running elevated could actually buy. As of v0.2.8, most
///   Windows `?` rows resolve to a real name via the snapshot fallback
///   before this function ever sees them; the count that remains is
///   genuinely foreign-user / `SYSTEM` / `PPL`-protected processes, or —
///   on Linux/macOS, where the fallback doesn't apply — any unresolved
///   row at all.
///
/// "GPU process" / "GPU processes" (not the previous-release
/// "compute process" / "compute processes") because on the `PDH`
/// Windows path the list includes every GPU memory holder
/// (compositor, browsers, games, compute), not just `CUDA` contexts.
fn format_ps_summary(
    rows: &[PsRow],
    pid_filter: Option<u32>,
    device_filter: Option<u32>,
) -> String {
    let count = rows.len();
    let protected = rows
        .iter()
        .filter(|r| {
            r.name.is_none()
                || r.name.as_deref() == Some("[protected]")
                || r.name.as_deref() == Some("?")
        })
        .count();
    let committed_total: u64 = rows.iter().map(|r| r.used_bytes).sum();

    let noun = if count == 1 {
        "GPU process"
    } else {
        "GPU processes"
    };

    let mut out = format!("{count} {noun} found");

    let filter_clause = match (pid_filter, device_filter) {
        (Some(p), Some(d)) => Some(format!("pid={p} device={d}")),
        (Some(p), None) => Some(format!("pid={p}")),
        (None, Some(d)) => Some(format!("device={d}")),
        (None, None) => None,
    };
    if let Some(clause) = filter_clause {
        let _ = write!(out, " matching {clause}");
    }

    // Committed-total + protected parenthetical. The word "committed"
    // hints at the WDDM commit-vs-resident distinction the Windows
    // backend exposes — summing `used_bytes` across processes can
    // exceed physical VRAM under WDDM (a real WDDM property, not a
    // bug), so naming the figure "committed total" prevents that from
    // reading as broken. Elided entirely when `count == 0` because
    // "0 MiB committed total" carries no information.
    match (count, protected) {
        (0, _) => {}
        (_, 0) => {
            let _ = write!(out, " ({} committed total)", format_vram(committed_total));
        }
        (_, p) => {
            let _ = write!(
                out,
                " ({} committed total; {p} protected — re-run elevated for names)",
                format_vram(committed_total)
            );
        }
    }

    out.push('.');
    out
}

/// Format `ps` rows as a fixed-column text table. Always prints the
/// header, even when `rows` is empty.
#[allow(clippy::missing_panics_doc)] // writes to a String; cannot fail in practice
fn format_ps_table(rows: &[PsRow]) -> String {
    let pid_header = "PID";
    let name_header = "NAME";
    let vram_header = "VRAM";
    let shared_header = "SHARED";
    let device_header = "DEVICE";

    let pid_cells: Vec<String> = rows.iter().map(|r| r.pid.to_string()).collect();
    let name_cells: Vec<&str> = rows
        .iter()
        .map(|r| r.name.as_deref().unwrap_or("?"))
        .collect();
    let vram_cells: Vec<String> = rows.iter().map(|r| format_vram(r.used_bytes)).collect();
    let shared_cells: Vec<String> = rows
        .iter()
        .map(|r| format_vram(r.shared_used_bytes))
        .collect();
    let device_cells: Vec<String> = rows
        .iter()
        .map(|r| {
            r.device_name
                .clone()
                .unwrap_or_else(|| format!("GPU {}", r.device_index))
        })
        .collect();

    let pid_w = column_width(pid_header, pid_cells.iter().map(String::as_str));
    let name_w = column_width(name_header, name_cells.iter().copied());
    let vram_w = column_width(vram_header, vram_cells.iter().map(String::as_str));
    let shared_w = column_width(shared_header, shared_cells.iter().map(String::as_str));
    let device_w = column_width(device_header, device_cells.iter().map(String::as_str));

    let mut out = String::new();
    let _ = writeln!(
        out,
        "{pid_header:<pid_w$}  {name_header:<name_w$}  {vram_header:<vram_w$}  {shared_header:<shared_w$}  {device_header:<device_w$}",
    );
    for ((((pid, name), vram), shared), device) in pid_cells
        .iter()
        .zip(&name_cells)
        .zip(&vram_cells)
        .zip(&shared_cells)
        .zip(&device_cells)
    {
        let _ = writeln!(
            out,
            "{pid:<pid_w$}  {name:<name_w$}  {vram:<vram_w$}  {shared:<shared_w$}  {device:<device_w$}",
        );
    }
    out
}

/// Format `ps` rows as a JSON array, one object per row. Hand-rolled
/// (no `serde` dep — keeps the `cli` feature lean for v0.2). Each
/// object: `{"pid":N,"name":<string|null>,"used_bytes":N,"shared_used_bytes":N,"device_index":N,"device_name":<string|null>}`.
/// String values are JSON-escaped via [`json_escape`].
#[allow(clippy::missing_panics_doc)] // writes to a String; cannot fail in practice
fn format_ps_json(rows: &[PsRow]) -> String {
    let mut out = String::from("[");
    for (i, row) in rows.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let name_json = row.name.as_deref().map_or_else(
            || String::from("null"),
            |n| format!("\"{}\"", json_escape(n)),
        );
        let device_name_json = row.device_name.as_deref().map_or_else(
            || String::from("null"),
            |n| format!("\"{}\"", json_escape(n)),
        );
        let _ = write!(
            out,
            r#"{{"pid":{},"name":{name_json},"used_bytes":{},"shared_used_bytes":{},"device_index":{},"device_name":{device_name_json}}}"#,
            row.pid, row.used_bytes, row.shared_used_bytes, row.device_index,
        );
    }
    out.push_str("]\n");
    out
}

// -----------------------------------------------------------------------------
// `spill` subcommand
// -----------------------------------------------------------------------------

/// All-zeros JSON object emitted by `hmn spill --json` when no
/// `SpillTracker` could be constructed at all (hard error path), so
/// scripted consumers still receive a parseable object with
/// `measurable: false` rather than empty stdout.
const SPILL_JSON_UNMEASURABLE: &str = concat!(
    r#"{"measurable":false,"spilled":false,"observations":0,"#,
    r#""baseline_shared_bytes":0,"peak_shared_bytes":0,"#,
    r#""peak_dedicated_bytes":0,"dedicated_limit_bytes":0,"#,
    r#""total_spill_duration_ms":0,"episodes":[]}"#,
    "\n"
);

/// Run the `spill` subcommand: spawn the wrapped command with
/// inherited stdio, poll a [`SpillTracker`] every `interval_ms` until
/// the child exits, print the report (stderr human block; optional
/// stdout JSON), and pass the child's exit code through.
///
/// Measurement failures never stop the workload: a tracker that fails
/// to construct produces a stderr warning and the child runs
/// unmeasured. Only spawn/wait failures — where there is no child
/// outcome to pass through — return `hmn`'s own `FAILURE`.
fn run_spill(
    interval_ms: u64,
    device: u32,
    json: bool,
    command: &[String],
) -> std::process::ExitCode {
    // clap's `required = true` on the trailing arg makes an empty
    // command unreachable in practice; belt-and-braces for direct calls.
    let Some((program, args)) = command.split_first() else {
        eprintln!("hmn: spill requires a command to run (after `--`)");
        return std::process::ExitCode::FAILURE;
    };

    let mut tracker = match SpillTracker::new(device) {
        Ok(t) => Some(t),
        Err(e) => {
            eprintln!("hmn: spill tracking unavailable ({e}); running command unmeasured");
            None
        }
    };

    let mut child = match std::process::Command::new(program).args(args).spawn() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("hmn: failed to spawn {program:?}: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };

    let run_start = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if let Some(t) = tracker.as_mut() {
                    t.observe(format!("+{:.1}s", run_start.elapsed().as_secs_f64()));
                }
                std::thread::sleep(Duration::from_millis(interval_ms));
            }
            Err(e) => {
                eprintln!("hmn: failed to wait on wrapped command: {e}");
                return std::process::ExitCode::FAILURE;
            }
        }
    };

    // One final observation at exit so short-lived commands get at
    // least one sample, then the report.
    if let Some(t) = tracker.as_mut() {
        t.observe(format!("+{:.1}s", run_start.elapsed().as_secs_f64()));
    }
    if let Some(report) = tracker.map(SpillTracker::into_report) {
        if json {
            print!("{}", format_spill_json(&report));
        }
        if report.measurable {
            eprint!("{}", format_spill_report(&report));
        } else {
            eprintln!("hmn spill: spill not measurable on this platform");
        }
    } else {
        // Tracker construction failed (already warned above).
        if json {
            print!("{SPILL_JSON_UNMEASURABLE}");
        }
        eprintln!("hmn spill: spill not measurable (no tracker)");
    }

    std::process::ExitCode::from(exit_code_byte(status.code()))
}

/// Map a child's `ExitStatus::code()` to the byte `hmn` exits with.
///
/// `0..=255` passes through exactly. Codes outside that range —
/// negative Windows `NTSTATUS` values (e.g. `0xC0000005` as `i32`),
/// or >255 — map to `1` rather than being bit-truncated: truncation
/// could turn a failure like 256 into a false success. `None` (child
/// killed by a signal on Unix) also maps to `1`.
fn exit_code_byte(code: Option<i32>) -> u8 {
    code.map_or(1, |c| u8::try_from(c).unwrap_or(1))
}

/// Whole milliseconds of a [`Duration`] for JSON output (`u128`
/// clamped into `u64` — saturates at `u64::MAX`, unreachable for real
/// run lengths).
fn duration_ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// Seconds with one decimal place (`"3.8s"`) — the human-facing
/// duration rendering in the spill report.
fn format_secs(d: Duration) -> String {
    format!("{:.1}s", d.as_secs_f64())
}

/// Format the human-readable spill report block printed to stderr, under
/// a caller-chosen `prefix` (e.g. `"hmn spill"`, `"hmn watch"`).
///
/// Three aligned lines, continuation lines indented to `prefix.len() + 2`
/// spaces (matching `"<prefix>: "`'s width). The dedicated line elides
/// its `/ capacity` suffix when the capacity is unknown
/// (`dedicated_limit_bytes == 0`); the episodes line collapses to `no
/// spill observed` when no episode was recorded. The `first ... into
/// run` fragment reuses the episode start label, which callers stamp as
/// elapsed time (`"+12.4s"`).
#[allow(clippy::missing_panics_doc)] // writes to a String; cannot fail in practice
fn format_spill_report_with_prefix(prefix: &str, report: &SpillReport) -> String {
    let mut out = String::new();
    let indent = " ".repeat(prefix.len() + 2);
    let limit_suffix = if report.dedicated_limit_bytes > 0 {
        format!(" / {}", format_vram(report.dedicated_limit_bytes))
    } else {
        String::new()
    };
    let _ = writeln!(
        out,
        "{prefix}: peak dedicated {}{limit_suffix}",
        format_vram(report.peak_dedicated_bytes)
    );
    let _ = writeln!(
        out,
        "{indent}peak shared    {} (baseline {})",
        format_vram(report.peak_shared_bytes),
        format_vram(report.baseline_shared_bytes)
    );
    if report.spilled() {
        let total = format_secs(report.total_spill_duration());
        let longest = report
            .longest_episode()
            .map_or_else(String::new, |e| format_secs(e.duration));
        let first = report.first_spill_label().unwrap_or("?");
        let _ = writeln!(
            out,
            "{indent}episodes       {} — total {total}, longest {longest}, first {first} into run",
            report.episodes.len()
        );
    } else {
        let _ = writeln!(out, "{indent}episodes       0 — no spill observed");
    }
    out
}

/// [`format_spill_report_with_prefix`] under the `hmn spill` prefix —
/// the report block `hmn spill -- <command>` prints to stderr on exit.
fn format_spill_report(report: &SpillReport) -> String {
    format_spill_report_with_prefix("hmn spill", report)
}

/// Write a [`SpillReport`]'s episodes as a JSON array (`[...]`, no
/// trailing content) into `out`. Shared by [`format_spill_json`] and
/// `hmn watch`'s summary JSON so the episode shape and escaping
/// ([`json_escape`]) stay identical across both subcommands' `--json`
/// output.
#[allow(clippy::missing_panics_doc)] // writes to a String; cannot fail in practice
fn write_episodes_json(out: &mut String, episodes: &[SpillEpisode]) {
    out.push('[');
    for (i, ep) in episodes.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let end_label = ep.end_label.as_deref().map_or_else(
            || String::from("null"),
            |l| format!("\"{}\"", json_escape(l)),
        );
        let _ = write!(
            out,
            r#"{{"start_label":"{}","end_label":{end_label},"peak_shared_bytes":{},"observations":{},"duration_ms":{}}}"#,
            json_escape(&ep.start_label),
            ep.peak_shared_bytes,
            ep.observations,
            duration_ms(ep.duration),
        );
    }
    out.push(']');
}

/// Format a [`SpillReport`] as a single JSON object. Hand-rolled (no
/// `serde` dep — same policy as [`format_ps_json`]); labels are
/// escaped via [`json_escape`]; durations are integer milliseconds.
#[allow(clippy::missing_panics_doc)] // writes to a String; cannot fail in practice
fn format_spill_json(report: &SpillReport) -> String {
    let mut out = String::new();
    let _ = write!(
        out,
        r#"{{"measurable":{},"spilled":{},"observations":{},"baseline_shared_bytes":{},"peak_shared_bytes":{},"peak_dedicated_bytes":{},"dedicated_limit_bytes":{},"total_spill_duration_ms":{},"episodes":"#,
        report.measurable,
        report.spilled(),
        report.observations,
        report.baseline_shared_bytes,
        report.peak_shared_bytes,
        report.peak_dedicated_bytes,
        report.dedicated_limit_bytes,
        duration_ms(report.total_spill_duration()),
    );
    write_episodes_json(&mut out, &report.episodes);
    out.push_str("}\n");
    out
}

// -----------------------------------------------------------------------------
// `watch` subcommand
// -----------------------------------------------------------------------------

/// Minimum unresolved-PID cumulative growth (bytes, either committed or
/// shared) that triggers the one-shot "unresolved process grew" stderr
/// hint. Same magnitude as [`hypomnesis::spill`]'s
/// `DEFAULT_SHARED_GROWTH_BYTES` (256 MiB) — large enough to clear
/// ordinary counter jitter, small enough to fire well before a real
/// leak becomes a problem.
const UNRESOLVED_GROWTH_HINT_BYTES: u64 = 256 * 1024 * 1024;

/// How long a single sleep chunk in the watch loop lasts before
/// re-checking the Ctrl+C flag — keeps interrupt latency low even when
/// `--interval` is minutes long.
const WATCH_SLEEP_CHUNK: Duration = Duration::from_millis(200);

/// Per-watched-PID bookkeeping accumulated across the whole watch.
struct WatchedPidState {
    /// Committed bytes at the first sample — the baseline the closing
    /// summary reports growth against.
    baseline_used_bytes: u64,
    /// Shared-resident bytes at the first sample.
    baseline_shared_bytes: u64,
    /// Committed bytes at the previous sample — the per-interval delta
    /// is computed against this, then it is overwritten.
    prev_used_bytes: u64,
    /// Shared-resident bytes at the previous sample.
    prev_shared_bytes: u64,
    /// Highest committed reading seen across the watch.
    peak_used_bytes: u64,
    /// Highest shared-resident reading seen across the watch.
    peak_shared_bytes: u64,
    /// Name from the most recent sample that saw this PID; `None` once
    /// no sample has ever resolved one.
    last_name: Option<String>,
    /// Whether the unresolved-growth hint has already fired for this
    /// PID (fires at most once per watch).
    growth_hint_fired: bool,
}

impl WatchedPidState {
    /// Fresh state seeded from a PID's first sample: baseline, previous,
    /// and peak all start at the first reading, so the first interval's
    /// delta is `+0`.
    const fn new(used_bytes: u64, shared_bytes: u64) -> Self {
        Self {
            baseline_used_bytes: used_bytes,
            baseline_shared_bytes: shared_bytes,
            prev_used_bytes: used_bytes,
            prev_shared_bytes: shared_bytes,
            peak_used_bytes: used_bytes,
            peak_shared_bytes: shared_bytes,
            last_name: None,
            growth_hint_fired: false,
        }
    }
}

/// Accumulated per-PID watch state, plus the order PIDs were first seen
/// in.
///
/// A plain `HashMap<u32, WatchedPidState>` would lose insertion order —
/// fine when the watched PID set is fixed for the whole run (the
/// pre-`--follow-new` case, where the closing summary iterates the
/// original fixed list instead), but under `--follow-new` the closing
/// summary must instead walk *every* PID ever tracked (departed or
/// still active) in a deterministic, meaningful order. `seen_order`
/// records that order — chronological, first sighting — without pulling
/// in an ordered-map dependency: [`WatchState::track`] is the sole
/// insertion point and is the only place a PID is ever appended to it,
/// exactly once, the first time that PID is seen.
struct WatchState {
    /// Per-PID accumulated state, keyed by PID.
    by_pid: HashMap<u32, WatchedPidState>,
    /// PIDs in first-seen order. Never contains a duplicate — see
    /// [`WatchState::track`].
    seen_order: Vec<u32>,
}

impl WatchState {
    /// Fresh, empty state.
    fn new() -> Self {
        Self {
            by_pid: HashMap::new(),
            seen_order: Vec::new(),
        }
    }

    /// Get the existing entry for `pid`, or seed a fresh one from
    /// `(used_bytes, shared_bytes)` and record `pid` in
    /// [`Self::seen_order`] — but only on this, the *first* time `pid`
    /// is tracked. A PID that later drops out of the followed set and
    /// re-enters keeps its original `seen_order` position and its
    /// existing history (no reset; see [`process_sample`]'s doc comment
    /// for why re-entry is deliberately not treated as a new process).
    fn track(&mut self, pid: u32, used_bytes: u64, shared_bytes: u64) -> &mut WatchedPidState {
        // `self.seen_order` and `self.by_pid` are disjoint fields, so
        // borrowing the former up front and capturing it in the
        // `or_insert_with` closure below (which only runs, and so only
        // pushes, on an actual insertion) needs no unwrap/expect/entry
        // double-lookup to track first-seen order.
        let seen_order = &mut self.seen_order;
        self.by_pid.entry(pid).or_insert_with(|| {
            seen_order.push(pid);
            WatchedPidState::new(used_bytes, shared_bytes)
        })
    }
}

/// One PID's rendered row for one interval — output of [`process_sample`],
/// consumed by the text-table and JSONL formatters.
struct WatchSampleRow {
    /// Process ID.
    pid: u32,
    /// Process name from this sample; `None` renders as `?`.
    name: Option<String>,
    /// Committed bytes this sample.
    used_bytes: u64,
    /// Signed delta vs. the previous sample (negative = freed).
    used_delta: i64,
    /// Shared-resident bytes this sample.
    shared_bytes: u64,
    /// Signed delta vs. the previous sample.
    shared_delta: i64,
    /// Adapter-wide instantaneous spill state at this sample
    /// ([`SpillTracker::is_spilling`]) — the same value on every row
    /// sharing this interval's timestamp; spill is a device-level
    /// phenomenon, not a per-PID one. `false` when spill tracking is
    /// unavailable.
    spilling: bool,
}

/// End-of-watch peak/baseline summary for one watched PID.
struct WatchPidSummary {
    /// Process ID.
    pid: u32,
    /// Name from the most recent sample that resolved one.
    name: Option<String>,
    /// Committed bytes at the first sample.
    baseline_used_bytes: u64,
    /// Highest committed reading across the watch.
    peak_used_bytes: u64,
    /// Shared-resident bytes at the first sample.
    baseline_shared_bytes: u64,
    /// Highest shared-resident reading across the watch.
    peak_shared_bytes: u64,
}

/// Signed byte delta `current - previous`.
///
/// VRAM byte counts are far below `i64::MAX` (2^63) for any real GPU, so
/// the widening cast cannot lose information.
const fn signed_delta(current: u64, previous: u64) -> i64 {
    // CAST: u64 → i64, VRAM byte counts (< 2^53 in practice) fit
    // trivially; deltas can be negative (VRAM freed), which u64 cannot
    // represent.
    #[allow(clippy::as_conversions, clippy::cast_possible_wrap)]
    let (c, p) = (current as i64, previous as i64);
    c - p
}

/// Human-readable signed VRAM delta: `"+700 MiB"`, `"-1.2 GiB"`,
/// `"+0 B"` for an exact-zero delta (avoids a `"-0 …"` reading for
/// negative deltas that round to zero under [`format_vram`]'s MiB
/// granularity).
fn format_delta(bytes: i64) -> String {
    if bytes == 0 {
        return "+0 B".to_owned();
    }
    let sign = if bytes < 0 { '-' } else { '+' };
    format!("{sign}{}", format_vram(bytes.unsigned_abs()))
}

/// `u8` exit code conveying whether spill was observed during a watch:
/// `0` clean, `1` spill observed at least once. Hard-error paths (bad
/// device, nothing to auto-select, or `--follow-new` combined with
/// explicit PIDs) return `2` directly from [`run_watch`], bypassing
/// this mapping.
const fn watch_exit_code(spilled: bool) -> u8 {
    if spilled { 1 } else { 0 }
}

/// Select the PIDs to watch when none were given explicitly: sort by
/// [`ps_row_comparator`] under [`SortKey::Dedicated`] — always the
/// dedicated-descending key, sharing `run_ps`'s exact comparator
/// (including its name/PID tie-break chain) so the two orderings can't
/// silently drift — and take the first `n`. Pure — the auto-selection
/// policy is unit-testable without any FFI.
fn select_top_n_pids(rows: &[PsRow], n: usize) -> Vec<u32> {
    let mut sorted: Vec<&PsRow> = rows.iter().collect();
    let cmp = ps_row_comparator(SortKey::Dedicated);
    sorted.sort_by(|a, b| cmp(a, b));
    sorted.into_iter().take(n).map(|r| r.pid).collect()
}

/// Parse a `--interval` / `--duration` value: digits followed by an
/// optional unit (`ms`, `s`, `m`, `h`); bare digits mean seconds. Used
/// as a clap `value_parser`, so a parse failure surfaces as a normal
/// `--help`-style clap usage error (`String` satisfies clap's error
/// bound via the standard library's `impl From<String> for Box<dyn
/// Error + Send + Sync>`).
fn parse_duration(s: &str) -> std::result::Result<Duration, String> {
    let trimmed = s.trim();
    let split_at = trimmed
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(trimmed.len());
    let (digits, unit) = trimmed.split_at(split_at);
    if digits.is_empty() {
        return Err(format!(
            "invalid duration {s:?}: expected digits followed by an optional unit (ms, s, m, h)"
        ));
    }
    let value: u64 = digits
        .parse()
        .map_err(|_| format!("invalid duration {s:?}: {digits:?} is not a whole number"))?;
    let millis = match unit {
        "" | "s" => value.saturating_mul(1_000),
        "ms" => value,
        "m" => value.saturating_mul(60_000),
        "h" => value.saturating_mul(3_600_000),
        other => {
            return Err(format!(
                "invalid duration {s:?}: unknown unit {other:?} (expected ms, s, m, or h)"
            ));
        }
    };
    if millis == 0 {
        return Err(format!("invalid duration {s:?}: must be greater than zero"));
    }
    Ok(Duration::from_millis(millis))
}

/// Sleep for `total`, checking `interrupted` every
/// [`WATCH_SLEEP_CHUNK`] so a Ctrl+C during a long `--interval` is
/// noticed promptly rather than only after the full sleep elapses.
fn sleep_interruptibly(total: Duration, interrupted: &AtomicBool) {
    let mut remaining = total;
    while remaining > Duration::ZERO && !interrupted.load(Ordering::Relaxed) {
        let step = remaining.min(WATCH_SLEEP_CHUNK);
        std::thread::sleep(step);
        remaining = remaining.saturating_sub(step);
    }
}

/// Filter out `name` values that don't represent a genuinely resolved
/// process identity for [`process_sample`]'s PID-reuse comparison:
/// `None` (unresolved) and the Windows-only `"[protected]"`/`"[exited]"`
/// synthetic brackets (still unresolved, just with more detail than a
/// bare `?`). `"[kernel]"` is deliberately *not* filtered — `PID 4` is
/// permanently the kernel and never flickers, so it is safe to treat as
/// a stable, comparable name.
#[must_use]
fn resolved_name(name: Option<&str>) -> Option<&str> {
    name.filter(|n| *n != "[protected]" && *n != "[exited]")
}

/// Fold one sample into `state`, observe the spill tracker, and return
/// one rendered [`WatchSampleRow`] per watched PID (in `watched`'s
/// order). A watched PID absent from `rows` renders as `0 B` / `0 B` for
/// this interval — `hmn watch` cannot distinguish "exited" from
/// "currently holds no GPU memory" and does not try to (see the `Watch`
/// subcommand's doc comment).
///
/// `watched` need not be the same slice across calls — under
/// `--follow-new` it's recomputed every interval — and a PID re-entering
/// `watched` after an absence resumes its existing [`WatchState`] entry
/// rather than starting fresh: [`WatchState::track`] only seeds a PID
/// once, the first time it's ever seen, by design (an OS process
/// legitimately dipping below rank `--top` for one interval and
/// recovering is not a new process; the name-change reset below is the
/// intentionally narrower signal for genuine OS PID reuse).
///
/// Emits the one-shot `?`-row growth hint to stderr the first interval
/// an unresolved watched PID's cumulative growth crosses
/// [`UNRESOLVED_GROWTH_HINT_BYTES`] — "unresolved" here means `name` is
/// `None` or (Windows-only, since v0.2.8) the `"[protected]"` bracket;
/// `"[exited]"` does not count, since a process already confirmed gone
/// cannot meaningfully "grow". Also detects a watched PID being recycled
/// by the OS mid-watch (its [`resolved_name`] changes between samples)
/// and resets that PID's baseline/peak so deltas describe the new
/// process rather than mixing two processes' readings — best-effort
/// (unresolved-name churn can't be distinguished from reuse).
fn process_sample(
    rows: &[GpuProcessEntry],
    state: &mut WatchState,
    watched: &[u32],
    elapsed: Duration,
    tracker: Option<&mut SpillTracker>,
) -> Vec<WatchSampleRow> {
    // EXPLICIT: `tracker: None` means no tracker was constructed for this
    // run (spill unmeasurable / construction failed) — every sample then
    // reports not-spilling, matching the honest "measurable: false"
    // contract elsewhere in the crate.
    let spilling = tracker.is_some_and(|t| {
        t.observe(format!("+{:.1}s", elapsed.as_secs_f64()));
        t.is_spilling()
    });

    let mut out = Vec::with_capacity(watched.len());
    for &pid in watched {
        let found = rows.iter().find(|r| r.pid == pid);
        let (name, used_bytes, shared_bytes) = found.map_or((None, 0, 0), |r| {
            (r.name.clone(), r.used_bytes, r.shared_used_bytes)
        });

        let entry = state.track(pid, used_bytes, shared_bytes);

        // The OS can recycle a PID mid-watch: a resolved name that
        // changes between samples is the only signal `hmn watch` has
        // that "pid" now names a different process than the one it
        // baselined against. Treat it as a fresh attach — reset
        // baseline/peak/prev to this sample so the closing summary and
        // this row's delta describe the *new* process, not a mix of
        // both. `None` on either side (still unresolved, or a
        // transient resolution race) is not treated as a change — nor
        // is the Windows-only `"[protected]"`/`"[exited]"` synthetic
        // brackets, which can flicker in and out for one interval (e.g.
        // a transient `Toolhelp32Snapshot` failure) without the
        // underlying process actually changing — see `resolved_name`
        // and the `last_name` update below, both of which stay sticky
        // across an unresolved sample.
        if let (Some(old), Some(new)) = (
            resolved_name(entry.last_name.as_deref()),
            resolved_name(name.as_deref()),
        ) && old != new
        {
            entry.baseline_used_bytes = used_bytes;
            entry.baseline_shared_bytes = shared_bytes;
            entry.prev_used_bytes = used_bytes;
            entry.prev_shared_bytes = shared_bytes;
            entry.peak_used_bytes = used_bytes;
            entry.peak_shared_bytes = shared_bytes;
            entry.growth_hint_fired = false;
            eprintln!(
                "hmn watch: pid={pid} name changed ({old} → {new}) — likely PID reuse by the OS; baseline reset"
            );
        }

        let used_delta = signed_delta(used_bytes, entry.prev_used_bytes);
        let shared_delta = signed_delta(shared_bytes, entry.prev_shared_bytes);
        entry.prev_used_bytes = used_bytes;
        entry.prev_shared_bytes = shared_bytes;
        entry.peak_used_bytes = entry.peak_used_bytes.max(used_bytes);
        entry.peak_shared_bytes = entry.peak_shared_bytes.max(shared_bytes);
        entry.last_name = resolved_name(name.as_deref())
            .map(ToOwned::to_owned)
            .or_else(|| entry.last_name.clone());

        let still_unresolved = name.is_none() || name.as_deref() == Some("[protected]");
        if !entry.growth_hint_fired && still_unresolved {
            let grown = used_bytes
                .saturating_sub(entry.baseline_used_bytes)
                .max(shared_bytes.saturating_sub(entry.baseline_shared_bytes));
            if grown >= UNRESOLVED_GROWTH_HINT_BYTES {
                entry.growth_hint_fired = true;
                eprintln!(
                    "hmn watch: unresolved pid={pid} grew +{} since attach — re-run elevated to identify",
                    format_vram(grown)
                );
            }
        }

        out.push(WatchSampleRow {
            pid,
            name,
            used_bytes,
            used_delta,
            shared_bytes,
            shared_delta,
            spilling,
        });
    }
    out
}

/// Format one interval's rows as a text table (no header — the caller
/// prints the column header once up front). Column widths are computed
/// per call from that interval's own cells, like [`format_ps_table`];
/// consecutive intervals may re-align slightly as values change width,
/// an acceptable trade-off for a continuously-appended stream (`--json`
/// is the stable-shape option for scripts).
#[allow(clippy::missing_panics_doc)] // writes to a String; cannot fail in practice
fn format_watch_rows_text(elapsed: Duration, rows: &[WatchSampleRow]) -> String {
    let time_label = format!("+{:.1}s", elapsed.as_secs_f64());
    let pid_header = "PID";
    let name_header = "NAME";
    let committed_header = "COMMITTED";
    let dcommit_header = "\u{394}COMMIT";
    let shared_header = "SHARED";
    let dshared_header = "\u{394}SHARED";
    let spill_header = "SPILL";

    let pid_cells: Vec<String> = rows.iter().map(|r| r.pid.to_string()).collect();
    let name_cells: Vec<&str> = rows
        .iter()
        .map(|r| r.name.as_deref().unwrap_or("?"))
        .collect();
    let committed_cells: Vec<String> = rows.iter().map(|r| format_vram(r.used_bytes)).collect();
    let dcommit_cells: Vec<String> = rows.iter().map(|r| format_delta(r.used_delta)).collect();
    let shared_cells: Vec<String> = rows.iter().map(|r| format_vram(r.shared_bytes)).collect();
    let dshared_cells: Vec<String> = rows.iter().map(|r| format_delta(r.shared_delta)).collect();
    let spill_cells: Vec<&str> = rows
        .iter()
        .map(|r| if r.spilling { "SPILL" } else { "no" })
        .collect();

    let pid_w = column_width(pid_header, pid_cells.iter().map(String::as_str));
    let name_w = column_width(name_header, name_cells.iter().copied());
    let committed_w = column_width(committed_header, committed_cells.iter().map(String::as_str));
    let dcommit_w = column_width(dcommit_header, dcommit_cells.iter().map(String::as_str));
    let shared_w = column_width(shared_header, shared_cells.iter().map(String::as_str));
    let dshared_w = column_width(dshared_header, dshared_cells.iter().map(String::as_str));
    let spill_w = column_width(spill_header, spill_cells.iter().copied());

    let mut out = String::new();
    for ((((((pid, name), committed), dcommit), shared), dshared), spill) in pid_cells
        .iter()
        .zip(&name_cells)
        .zip(&committed_cells)
        .zip(&dcommit_cells)
        .zip(&shared_cells)
        .zip(&dshared_cells)
        .zip(&spill_cells)
    {
        let _ = writeln!(
            out,
            "{time_label:<8}  {pid:<pid_w$}  {name:<name_w$}  {committed:<committed_w$}  \
             {dcommit:<dcommit_w$}  {shared:<shared_w$}  {dshared:<dshared_w$}  {spill:<spill_w$}",
        );
    }
    out
}

/// Format the watch column header line (text mode), printed once before
/// the loop starts.
fn format_watch_header_text() -> String {
    format!(
        "{:<8}  {:<6}  {:<12}  {:<9}  {:<9}  {:<9}  {:<9}  {:<5}\n",
        "TIME", "PID", "NAME", "COMMITTED", "\u{394}COMMIT", "SHARED", "\u{394}SHARED", "SPILL"
    )
}

/// Format one interval's rows as JSON Lines: one `"kind":"sample"`
/// object per row, newline-terminated, ready to pipe to `jq -c`.
#[allow(clippy::missing_panics_doc)] // writes to a String; cannot fail in practice
fn format_watch_rows_json(elapsed: Duration, rows: &[WatchSampleRow]) -> String {
    let mut out = String::new();
    for row in rows {
        let name_json = row.name.as_deref().map_or_else(
            || String::from("null"),
            |n| format!("\"{}\"", json_escape(n)),
        );
        let _ = writeln!(
            out,
            r#"{{"kind":"sample","t_ms":{},"pid":{},"name":{name_json},"used_bytes":{},"used_delta_bytes":{},"shared_used_bytes":{},"shared_delta_bytes":{},"spilling":{}}}"#,
            duration_ms(elapsed),
            row.pid,
            row.used_bytes,
            row.used_delta,
            row.shared_bytes,
            row.shared_delta,
            row.spilling,
        );
    }
    out
}

/// Format the end-of-watch per-PID peak/baseline block (text mode).
/// Empty `per_pid` renders as an empty string (nothing to show).
#[allow(clippy::missing_panics_doc)] // writes to a String; cannot fail in practice
fn format_watch_per_pid_block(per_pid: &[WatchPidSummary]) -> String {
    if per_pid.is_empty() {
        return String::new();
    }
    let pid_header = "PID";
    let name_header = "NAME";
    let baseline_committed_header = "BASELINE COMMIT";
    let peak_committed_header = "PEAK COMMIT";
    let baseline_shared_header = "BASELINE SHARED";
    let peak_shared_header = "PEAK SHARED";

    let pid_cells: Vec<String> = per_pid.iter().map(|p| p.pid.to_string()).collect();
    let name_cells: Vec<&str> = per_pid
        .iter()
        .map(|p| p.name.as_deref().unwrap_or("?"))
        .collect();
    let baseline_committed_cells: Vec<String> = per_pid
        .iter()
        .map(|p| format_vram(p.baseline_used_bytes))
        .collect();
    let peak_committed_cells: Vec<String> = per_pid
        .iter()
        .map(|p| format_vram(p.peak_used_bytes))
        .collect();
    let baseline_shared_cells: Vec<String> = per_pid
        .iter()
        .map(|p| format_vram(p.baseline_shared_bytes))
        .collect();
    let peak_shared_cells: Vec<String> = per_pid
        .iter()
        .map(|p| format_vram(p.peak_shared_bytes))
        .collect();

    let pid_w = column_width(pid_header, pid_cells.iter().map(String::as_str));
    let name_w = column_width(name_header, name_cells.iter().copied());
    let baseline_committed_w = column_width(
        baseline_committed_header,
        baseline_committed_cells.iter().map(String::as_str),
    );
    let peak_committed_w = column_width(
        peak_committed_header,
        peak_committed_cells.iter().map(String::as_str),
    );
    let baseline_shared_w = column_width(
        baseline_shared_header,
        baseline_shared_cells.iter().map(String::as_str),
    );
    let peak_shared_w = column_width(
        peak_shared_header,
        peak_shared_cells.iter().map(String::as_str),
    );

    let mut out = String::new();
    let _ = writeln!(
        out,
        "hmn watch: per-PID  {pid_header:<pid_w$}  {name_header:<name_w$}  \
         {baseline_committed_header:<baseline_committed_w$}  {peak_committed_header:<peak_committed_w$}  \
         {baseline_shared_header:<baseline_shared_w$}  {peak_shared_header:<peak_shared_w$}",
    );
    for (((((pid, name), bc), pc), bs), ps) in pid_cells
        .iter()
        .zip(&name_cells)
        .zip(&baseline_committed_cells)
        .zip(&peak_committed_cells)
        .zip(&baseline_shared_cells)
        .zip(&peak_shared_cells)
    {
        let _ = writeln!(
            out,
            "                    {pid:<pid_w$}  {name:<name_w$}  {bc:<baseline_committed_w$}  \
             {pc:<peak_committed_w$}  {bs:<baseline_shared_w$}  {ps:<peak_shared_w$}",
        );
    }
    out
}

/// Format the closing summary in text mode: the adapter-level report
/// (via [`format_spill_report_with_prefix`] under the `hmn watch`
/// prefix) or, when spill tracking was unavailable for this run, a
/// one-line notice — followed either way by the per-PID block.
#[allow(clippy::missing_panics_doc)] // writes to a String; cannot fail in practice
fn format_watch_summary_text(report: Option<&SpillReport>, per_pid: &[WatchPidSummary]) -> String {
    let mut out = report.map_or_else(
        || "hmn watch: spill tracking unavailable for this run; per-PID VRAM below\n".to_owned(),
        |r| format_spill_report_with_prefix("hmn watch", r),
    );
    out.push_str(&format_watch_per_pid_block(per_pid));
    out
}

/// Format the closing summary as one JSON object:
/// `{"kind":"summary",...adapter SpillReport fields...,"per_pid":[...]}`.
/// `report: None` (spill tracking unavailable) emits the same
/// all-zeros `"measurable":false` shape [`SPILL_JSON_UNMEASURABLE`]
/// uses, so scripted consumers always parse one shape either way.
#[allow(clippy::missing_panics_doc)] // writes to a String; cannot fail in practice
fn format_watch_summary_json(report: Option<&SpillReport>, per_pid: &[WatchPidSummary]) -> String {
    let mut out = String::from(r#"{"kind":"summary","#);
    match report {
        Some(r) => {
            let _ = write!(
                out,
                r#""measurable":{},"spilled":{},"observations":{},"baseline_shared_bytes":{},"peak_shared_bytes":{},"peak_dedicated_bytes":{},"dedicated_limit_bytes":{},"total_spill_duration_ms":{},"episodes":"#,
                r.measurable,
                r.spilled(),
                r.observations,
                r.baseline_shared_bytes,
                r.peak_shared_bytes,
                r.peak_dedicated_bytes,
                r.dedicated_limit_bytes,
                duration_ms(r.total_spill_duration()),
            );
            write_episodes_json(&mut out, &r.episodes);
        }
        None => {
            out.push_str(
                r#""measurable":false,"spilled":false,"observations":0,"baseline_shared_bytes":0,"peak_shared_bytes":0,"peak_dedicated_bytes":0,"dedicated_limit_bytes":0,"total_spill_duration_ms":0,"episodes":[]"#,
            );
        }
    }
    out.push_str(r#","per_pid":["#);
    for (i, p) in per_pid.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let name_json = p.name.as_deref().map_or_else(
            || String::from("null"),
            |n| format!("\"{}\"", json_escape(n)),
        );
        let _ = write!(
            out,
            r#"{{"pid":{},"name":{name_json},"baseline_used_bytes":{},"peak_used_bytes":{},"baseline_shared_bytes":{},"peak_shared_bytes":{}}}"#,
            p.pid,
            p.baseline_used_bytes,
            p.peak_used_bytes,
            p.baseline_shared_bytes,
            p.peak_shared_bytes,
        );
    }
    out.push_str("]}\n");
    out
}

/// Resolve which PIDs to watch from one sample: `explicit` unchanged
/// when non-empty (explicit PIDs are always watched exactly as given),
/// otherwise the top `top` by committed VRAM from `rows` — sharing
/// `hmn ps`'s own comparator via [`select_top_n_pids`], so the two
/// orderings can't drift apart.
///
/// Called once before the watch loop always, and again every interval
/// under `--follow-new` (cheap: `rows` numbers in the tens, and
/// `--interval` is 5s+ apart by default).
fn resolve_watched_pids(rows: &[GpuProcessEntry], explicit: &[u32], top: usize) -> Vec<u32> {
    if !explicit.is_empty() {
        return explicit.to_vec();
    }
    // device_index / device_name are unused by SortKey::Dedicated's
    // comparator (pid / used_bytes / name only) — defaulted rather than
    // threaded through from the caller, which has no device-name
    // context of its own to give.
    let ps_rows: Vec<PsRow> = rows
        .iter()
        .map(|e| PsRow {
            pid: e.pid,
            // BORROW: clone — e is borrowed from `rows`.
            name: e.name.clone(),
            used_bytes: e.used_bytes,
            shared_used_bytes: e.shared_used_bytes,
            device_index: 0,
            device_name: None,
        })
        .collect();
    select_top_n_pids(&ps_rows, top)
}

/// Build the stderr breadcrumb naming PIDs that entered or left the
/// followed set between two consecutive `--follow-new` intervals, or
/// `None` when the set didn't change. Entered-PID names come from the
/// current sample's `rows`; left-PID names come from `state`'s last
/// known name for that PID (it is already absent from `rows`, by
/// definition of having left). Purely cosmetic: doesn't affect the
/// JSONL stream shape or the closing summary.
///
/// `prev_watched` must be captured *before* the caller reassigns its
/// `watched` variable to the freshly `resolve_watched_pids`-computed
/// set — the two arguments have to actually differ for the diff to mean
/// anything. Call order relative to [`process_sample`] does not matter
/// on its own: `process_sample` only touches a PID present in the
/// `watched` slice it is given, so a departed PID's [`WatchState`] entry
/// is untouched that interval regardless of when this function runs
/// relative to it.
fn format_followed_set_change(
    prev_watched: &[u32],
    new_watched: &[u32],
    rows: &[GpuProcessEntry],
    state: &WatchState,
    elapsed: Duration,
) -> Option<String> {
    let entered: Vec<u32> = new_watched
        .iter()
        .copied()
        .filter(|p| !prev_watched.contains(p))
        .collect();
    let left: Vec<u32> = prev_watched
        .iter()
        .copied()
        .filter(|p| !new_watched.contains(p))
        .collect();
    if entered.is_empty() && left.is_empty() {
        return None;
    }

    let name_for = |pid: u32| -> String {
        let name = rows
            .iter()
            .find(|r| r.pid == pid)
            .and_then(|r| r.name.clone())
            .or_else(|| state.by_pid.get(&pid).and_then(|s| s.last_name.clone()));
        name.map_or_else(|| format!("pid={pid}"), |n| format!("pid={pid} ({n})"))
    };

    let mut clauses = Vec::new();
    if !entered.is_empty() {
        let list = entered
            .into_iter()
            .map(name_for)
            .collect::<Vec<_>>()
            .join(", ");
        clauses.push(format!("entered {list}"));
    }
    if !left.is_empty() {
        let list = left
            .into_iter()
            .map(name_for)
            .collect::<Vec<_>>()
            .join(", ");
        clauses.push(format!("left {list}"));
    }
    Some(format!(
        "hmn watch: +{:.1}s followed set changed: {}",
        elapsed.as_secs_f64(),
        clauses.join("; ")
    ))
}

/// Run the `watch` subcommand: resolve the watched PID set, sample it on
/// a timer against [`SpillTracker`] + [`gpu_processes`] until
/// `--duration` elapses or Ctrl+C, then print the closing summary.
///
/// Returns `2` immediately on a hard error (device unreachable, nothing
/// to auto-select without `--follow-new`, or `--follow-new` combined
/// with explicit PIDs); otherwise runs to completion and returns
/// [`watch_exit_code`] of whether spill was ever observed.
fn run_watch(
    pids: &[u32],
    interval: Duration,
    duration: Option<Duration>,
    top: usize,
    follow_new: bool,
    device: u32,
    json: bool,
) -> std::process::ExitCode {
    let mut seen = std::collections::HashSet::new();
    let explicit: Vec<u32> = pids.iter().copied().filter(|p| seen.insert(*p)).collect();
    let auto_selected = explicit.is_empty();

    // Checked first, before any backend call (including `device_info`
    // below, itself a real NVML/DXGI dispatch) — argument-validation
    // failures should fail fast without touching hardware at all.
    if follow_new && !auto_selected {
        eprintln!(
            "hmn: watch --follow-new only applies to auto-selection; drop --follow-new or \
             the explicit PID list"
        );
        return std::process::ExitCode::from(2);
    }

    let device_name = device_info(device).ok().and_then(|d| d.name);

    let first_rows = match gpu_processes(device) {
        Ok(rows) => rows,
        Err(e) => {
            eprintln!("hmn: watch failed to query device {device}: {e}");
            return std::process::ExitCode::from(2);
        }
    };

    let mut watched = resolve_watched_pids(&first_rows, &explicit, top);
    if watched.is_empty() {
        if follow_new {
            eprintln!(
                "hmn: watch found no GPU processes on device {device} yet (top {top} by \
                 committed); waiting for work to appear"
            );
        } else {
            eprintln!(
                "hmn: watch found no GPU processes on device {device} to auto-select \
                 (top {top}); re-run with an explicit PID once a workload is running"
            );
            return std::process::ExitCode::from(2);
        }
    }

    let mut tracker = match SpillTracker::new(device) {
        Ok(t) => Some(t),
        Err(e) => {
            eprintln!("hmn: watch spill tracking unavailable ({e}); showing per-PID VRAM only");
            None
        }
    };

    let interrupted = Arc::new(AtomicBool::new(false));
    {
        let interrupted = Arc::clone(&interrupted);
        if let Err(e) = ctrlc::set_handler(move || interrupted.store(true, Ordering::SeqCst)) {
            eprintln!(
                "hmn: watch failed to install Ctrl+C handler ({e}); interrupting will skip the closing summary"
            );
        }
    }

    let mode_clause = if follow_new {
        format!(
            "following top {top} by committed (re-selected every interval), {} initially",
            watched.len()
        )
    } else if auto_selected {
        format!("watching {} PID(s) (top {top} by committed)", watched.len())
    } else {
        format!("watching {} PID(s)", watched.len())
    };
    eprintln!(
        "hmn watch: device {device}{}, interval {:.1}s, {mode_clause}",
        device_name
            .as_deref()
            .map_or_else(String::new, |n| format!(" [{n}]")),
        interval.as_secs_f64(),
    );
    if !json {
        print!("{}", format_watch_header_text());
    }

    let mut state = WatchState::new();
    let start = std::time::Instant::now();

    let rows0 = process_sample(
        &first_rows,
        &mut state,
        &watched,
        Duration::ZERO,
        tracker.as_mut(),
    );
    if json {
        print!("{}", format_watch_rows_json(Duration::ZERO, &rows0));
    } else {
        print!("{}", format_watch_rows_text(Duration::ZERO, &rows0));
    }

    'watch: loop {
        if duration.is_some_and(|d| start.elapsed() >= d) || interrupted.load(Ordering::Relaxed) {
            break 'watch;
        }
        sleep_interruptibly(interval, &interrupted);
        if interrupted.load(Ordering::Relaxed) {
            break 'watch;
        }
        if duration.is_some_and(|d| start.elapsed() >= d) {
            break 'watch;
        }

        let elapsed = start.elapsed();
        let rows = match gpu_processes(device) {
            Ok(rows) => rows,
            Err(e) => {
                eprintln!(
                    "hmn watch: sample failed at +{:.1}s ({e}); skipping interval",
                    elapsed.as_secs_f64()
                );
                continue 'watch;
            }
        };

        if follow_new {
            let new_watched = resolve_watched_pids(&rows, &explicit, top);
            if let Some(msg) =
                format_followed_set_change(&watched, &new_watched, &rows, &state, elapsed)
            {
                eprintln!("{msg}");
            }
            watched = new_watched;
        }

        let sample = process_sample(&rows, &mut state, &watched, elapsed, tracker.as_mut());
        if json {
            print!("{}", format_watch_rows_json(elapsed, &sample));
        } else {
            print!("{}", format_watch_rows_text(elapsed, &sample));
        }
    }

    let report = tracker.map(SpillTracker::into_report);
    let per_pid: Vec<WatchPidSummary> = state
        .seen_order
        .iter()
        .map(|&pid| {
            let s = state.by_pid.get(&pid);
            WatchPidSummary {
                pid,
                name: s.and_then(|s| s.last_name.clone()),
                baseline_used_bytes: s.map_or(0, |s| s.baseline_used_bytes),
                peak_used_bytes: s.map_or(0, |s| s.peak_used_bytes),
                baseline_shared_bytes: s.map_or(0, |s| s.baseline_shared_bytes),
                peak_shared_bytes: s.map_or(0, |s| s.peak_shared_bytes),
            }
        })
        .collect();

    if json {
        print!("{}", format_watch_summary_json(report.as_ref(), &per_pid));
    } else {
        print!("{}", format_watch_summary_text(report.as_ref(), &per_pid));
    }

    std::process::ExitCode::from(watch_exit_code(
        report.as_ref().is_some_and(SpillReport::spilled),
    ))
}

// -----------------------------------------------------------------------------
// Formatting primitives
// -----------------------------------------------------------------------------

/// `MiB` (`bytes / 1_048_576`), rounded down. Used by the device-summary
/// formatter where `MiB` precision is sufficient.
const fn bytes_to_mib(bytes: u64) -> u64 {
    bytes / 1_048_576
}

/// Human-readable VRAM string. Renders `MiB` below 1 `GiB`, else `GiB`
/// to one decimal place.
fn format_vram(bytes: u64) -> String {
    const MIB: u64 = 1024 * 1024;
    const GIB: u64 = MIB * 1024;
    if bytes >= GIB {
        // CAST: u64 → f64, byte count and constant; fits in f64 mantissa
        // for any realistic VRAM size (< 2^53 bytes ≈ 8 PiB).
        #[allow(clippy::cast_precision_loss, clippy::as_conversions)]
        let g = (bytes as f64) / (GIB as f64);
        format!("{g:.1} GiB")
    } else {
        let mib = bytes / MIB;
        format!("{mib} MiB")
    }
}

/// Compute the width of a table column as `max(header.len(),
/// max(cell.len()))`.
fn column_width<'a>(header: &str, cells: impl IntoIterator<Item = &'a str>) -> usize {
    cells
        .into_iter()
        .map(str::len)
        .chain(std::iter::once(header.len()))
        .max()
        .unwrap_or(0)
}

/// Escape a string for JSON output. Hand-rolled to avoid pulling in
/// `serde_json` for the CLI feature.
#[allow(clippy::missing_panics_doc)] // writes to a String; cannot fail in practice
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                // CAST: char → u32, valid scalar values fit (≤ 0x10FFFF).
                #[allow(clippy::as_conversions)]
                let code = c as u32;
                let _ = write!(out, "\\u{code:04x}");
            }
            c => out.push(c),
        }
    }
    out
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::missing_docs_in_private_items,
    // EXPLICIT: panic! is the standard "unreachable pattern in a test"
    // signal for the spill arg-parse destructuring assertions.
    clippy::panic
)]
mod tests {
    use super::*;

    fn row(
        pid: u32,
        name: Option<&str>,
        used_bytes: u64,
        device_index: u32,
        device_name: Option<&str>,
    ) -> PsRow {
        PsRow {
            pid,
            name: name.map(str::to_owned),
            used_bytes,
            shared_used_bytes: 0,
            device_index,
            device_name: device_name.map(str::to_owned),
        }
    }

    /// Like [`row`] but with a non-zero `shared_used_bytes` — for the
    /// SHARED-column / spill-signal specific tests.
    fn row_shared(pid: u32, name: Option<&str>, used_bytes: u64, shared_used_bytes: u64) -> PsRow {
        PsRow {
            pid,
            name: name.map(str::to_owned),
            used_bytes,
            shared_used_bytes,
            device_index: 0,
            device_name: None,
        }
    }

    // --- format_vram ---

    #[test]
    fn format_vram_sub_gib() {
        assert_eq!(format_vram(0), "0 MiB");
        assert_eq!(format_vram(1024 * 1024), "1 MiB");
        assert_eq!(format_vram(512 * 1024 * 1024), "512 MiB");
    }

    #[test]
    fn format_vram_gib_one_decimal() {
        let one_gib = 1024_u64 * 1024 * 1024;
        assert_eq!(format_vram(one_gib), "1.0 GiB");
        // 1.5 GiB
        assert_eq!(format_vram(one_gib + one_gib / 2), "1.5 GiB");
        // ≈ 8.2 GiB (8 * 1024^3 + 200 * 1024^2 = 8 * GiB + 200 MiB).
        // 200 / 1024 = 0.1953... → renders as 8.2 GiB after one-decimal
        // rounding (matches the roadmap example output).
        let bytes_8_2_gib = 8 * one_gib + 200 * 1024 * 1024;
        assert_eq!(format_vram(bytes_8_2_gib), "8.2 GiB");
    }

    // --- bytes_to_mib ---

    #[test]
    fn bytes_to_mib_basic() {
        assert_eq!(bytes_to_mib(0), 0);
        assert_eq!(bytes_to_mib(1_048_576), 1);
        assert_eq!(bytes_to_mib(16_384 * 1_048_576), 16_384);
    }

    // --- column_width ---

    #[test]
    fn column_width_picks_max() {
        assert_eq!(column_width("PID", ["1", "12345"]), 5);
        assert_eq!(column_width("HEADER", ["a", "bc"]), 6);
        assert_eq!(column_width("PID", std::iter::empty::<&str>()), 3);
    }

    // --- json_escape ---

    #[test]
    fn json_escape_passthrough() {
        assert_eq!(json_escape("python.exe"), "python.exe");
    }

    #[test]
    fn json_escape_quotes_and_backslash() {
        assert_eq!(json_escape("a\"b\\c"), "a\\\"b\\\\c");
    }

    #[test]
    fn json_escape_control_chars() {
        assert_eq!(json_escape("a\nb"), "a\\nb");
        assert_eq!(json_escape("a\tb"), "a\\tb");
        // 0x01 is a control char without a short escape — 
        assert_eq!(json_escape("\u{0001}"), "\\u0001");
    }

    // --- format_ps_table ---

    #[test]
    fn format_ps_table_empty_prints_header_only() {
        let s = format_ps_table(&[]);
        // Header line ends with newline; widths default to header lengths.
        assert_eq!(s, "PID  NAME  VRAM  SHARED  DEVICE\n");
    }

    #[test]
    fn format_ps_table_single_row() {
        let r = row(
            12345,
            Some("python.exe"),
            8_589_934_592, // 8 GiB
            0,
            Some("RTX 5060 Ti"),
        );
        let s = format_ps_table(&[r]);
        let expected = "PID    NAME        VRAM     SHARED  DEVICE     \n\
                        12345  python.exe  8.0 GiB  0 MiB   RTX 5060 Ti\n";
        assert_eq!(s, expected);
    }

    #[test]
    fn format_ps_table_protected_name_renders_question_mark() {
        // Column widths: PID=3 (header), NAME=4 (header), VRAM=7
        // ("256 MiB"), SHARED=6 (header), DEVICE=11 ("RTX 5060 Ti").
        // Two-space separators.
        let r = row(99, Some("?"), 268_435_456, 0, Some("RTX 5060 Ti"));
        let s = format_ps_table(&[r]);
        let expected = "PID  NAME  VRAM     SHARED  DEVICE     \n\
                        99   ?     256 MiB  0 MiB   RTX 5060 Ti\n";
        assert_eq!(s, expected);
    }

    #[test]
    fn format_ps_table_missing_name_renders_question_mark() {
        // Missing name (None) renders identically to the protected `?`
        // case — both go through the `unwrap_or("?")` path.
        let r = row(99, None, 268_435_456, 0, Some("RTX 5060 Ti"));
        let s = format_ps_table(&[r]);
        let expected = "PID  NAME  VRAM     SHARED  DEVICE     \n\
                        99   ?     256 MiB  0 MiB   RTX 5060 Ti\n";
        assert_eq!(s, expected);
    }

    #[test]
    fn format_ps_table_falls_back_to_gpu_n_when_no_device_name() {
        let r = row(99, Some("python.exe"), 268_435_456, 3, None);
        let s = format_ps_table(&[r]);
        assert!(s.contains("python.exe  256 MiB  0 MiB   GPU 3"));
    }

    #[test]
    fn format_ps_table_shared_column_renders_nonzero_bytes() {
        // A genuinely spilling row: 16 GiB dedicated commit, 2 GiB
        // resident shared. The SHARED cell goes through the same
        // format_vram path as VRAM.
        let r = row_shared(
            77,
            Some("py.exe"),
            16 * 1024 * 1024 * 1024,
            2 * 1024 * 1024 * 1024,
        );
        let s = format_ps_table(&[r]);
        assert!(s.contains("16.0 GiB  2.0 GiB"));
    }

    // --- format_ps_json ---

    #[test]
    fn format_ps_json_empty() {
        assert_eq!(format_ps_json(&[]), "[]\n");
    }

    #[test]
    fn format_ps_json_single_row() {
        let r = row(
            12345,
            Some("python.exe"),
            8 * 1_048_576,
            0,
            Some("RTX 5060 Ti"),
        );
        let s = format_ps_json(&[r]);
        assert_eq!(
            s,
            "[{\"pid\":12345,\"name\":\"python.exe\",\"used_bytes\":8388608,\"shared_used_bytes\":0,\"device_index\":0,\"device_name\":\"RTX 5060 Ti\"}]\n"
        );
    }

    #[test]
    fn format_ps_json_null_name() {
        let r = row(42, None, 0, 0, None);
        let s = format_ps_json(&[r]);
        assert_eq!(
            s,
            "[{\"pid\":42,\"name\":null,\"used_bytes\":0,\"shared_used_bytes\":0,\"device_index\":0,\"device_name\":null}]\n"
        );
    }

    #[test]
    fn format_ps_json_two_rows_comma_separated() {
        let a = row(1, Some("a.exe"), 1_048_576, 0, Some("GPU"));
        let b = row(2, Some("b.exe"), 2_097_152, 0, Some("GPU"));
        let s = format_ps_json(&[a, b]);
        assert_eq!(
            s,
            "[{\"pid\":1,\"name\":\"a.exe\",\"used_bytes\":1048576,\"shared_used_bytes\":0,\"device_index\":0,\"device_name\":\"GPU\"},\
             {\"pid\":2,\"name\":\"b.exe\",\"used_bytes\":2097152,\"shared_used_bytes\":0,\"device_index\":0,\"device_name\":\"GPU\"}]\n"
        );
    }

    #[test]
    fn format_ps_json_nonzero_shared_bytes() {
        let r = row_shared(7, Some("py.exe"), 1_048_576, 424_242);
        let s = format_ps_json(&[r]);
        assert!(s.contains("\"used_bytes\":1048576,\"shared_used_bytes\":424242,"));
    }

    #[test]
    fn format_ps_json_escapes_quotes_in_name() {
        let r = row(1, Some(r#"weird"name"#), 0, 0, None);
        let s = format_ps_json(&[r]);
        assert!(s.contains(r#""name":"weird\"name""#));
    }

    // --- format_summary ---
    //
    // `Snapshot` and `GpuDeviceInfo` are `#[non_exhaustive]` and the
    // binary is a separate crate from the library, so struct-literal
    // construction is forbidden here. We test the only case that
    // doesn't require one: an empty input.

    #[test]
    fn format_summary_empty_input() {
        assert_eq!(format_summary(&[]), "");
    }

    #[test]
    fn format_summary_json_empty_input() {
        // Unlike format_summary's "" on empty input, the JSON formatter
        // always emits a parseable array — mirrors format_ps_json's
        // empty-input behavior ("[]\n").
        assert_eq!(format_summary_json(&[]), "[]\n");
    }

    #[test]
    fn top_level_json_before_subcommand_parses_clean() {
        // clap itself has no opinion on this combination — `main` rejects
        // it at runtime (exit code 2, verified live/manually, not here:
        // it's a hard-error path before dispatch, same shape as
        // `watch_args_follow_new_with_explicit_pids_parses_clean` above).
        // This test only pins down that clap parsing itself doesn't
        // reject `hmn --json ps` — it has to reach `main`'s dispatch to
        // be caught.
        let cli = Cli::try_parse_from(["hmn", "--json", "ps"]).unwrap();
        assert!(cli.json);
        assert!(matches!(cli.command, Some(Commands::Ps { .. })));
    }

    // --- format_ps_summary (stderr count line) ---

    /// Build `n` `PsRow`s with resolved names — used by tests that
    /// focus on count and filter clauses, not the protected-count
    /// parenthetical (which is exercised separately).
    fn unprotected_rows(n: u32) -> Vec<PsRow> {
        (0..n)
            .map(|i| row(1000 + i, Some("test.exe"), 0, 0, None))
            .collect()
    }

    /// Build `n` `PsRow`s with `name: None` — used to exercise the
    /// protected-count parenthetical.
    fn protected_rows(n: u32) -> Vec<PsRow> {
        (0..n).map(|i| row(2000 + i, None, 0, 0, None)).collect()
    }

    #[test]
    fn format_ps_summary_zero_no_filters() {
        assert_eq!(
            format_ps_summary(&unprotected_rows(0), None, None),
            "0 GPU processes found."
        );
    }

    #[test]
    fn format_ps_summary_one_no_filters() {
        // Singular noun, no filter clause. `used_bytes: 0` rows still
        // get a committed-total parenthetical (the figure is 0 MiB —
        // honest, even when uninteresting).
        assert_eq!(
            format_ps_summary(&unprotected_rows(1), None, None),
            "1 GPU process found (0 MiB committed total)."
        );
    }

    #[test]
    fn format_ps_summary_many_no_filters() {
        assert_eq!(
            format_ps_summary(&unprotected_rows(7), None, None),
            "7 GPU processes found (0 MiB committed total)."
        );
    }

    #[test]
    fn format_ps_summary_with_pid_filter() {
        // Zero rows → no parenthetical at all (committed-total
        // elides; the filter clause still appears).
        assert_eq!(
            format_ps_summary(&unprotected_rows(0), Some(12345), None),
            "0 GPU processes found matching pid=12345."
        );
    }

    #[test]
    fn format_ps_summary_with_device_filter() {
        assert_eq!(
            format_ps_summary(&unprotected_rows(2), None, Some(0)),
            "2 GPU processes found matching device=0 (0 MiB committed total)."
        );
    }

    #[test]
    fn format_ps_summary_with_both_filters() {
        assert_eq!(
            format_ps_summary(&unprotected_rows(1), Some(99), Some(1)),
            "1 GPU process found matching pid=99 device=1 (0 MiB committed total)."
        );
    }

    // -- committed-total parenthetical (non-zero VRAM) --

    #[test]
    fn format_ps_summary_with_committed_total_gib() {
        // 3 rows at 4 GiB each → 12 GiB committed total, formatted
        // with one decimal place to match `format_vram`'s GiB output.
        const FOUR_GIB: u64 = 4 * 1024 * 1024 * 1024;
        let rows = vec![
            row(1001, Some("a.exe"), FOUR_GIB, 0, None),
            row(1002, Some("b.exe"), FOUR_GIB, 0, None),
            row(1003, Some("c.exe"), FOUR_GIB, 0, None),
        ];
        assert_eq!(
            format_ps_summary(&rows, None, None),
            "3 GPU processes found (12.0 GiB committed total)."
        );
    }

    #[test]
    fn format_ps_summary_with_committed_total_mib() {
        // 2 rows at 256 MiB each → 512 MiB, below 1 GiB threshold,
        // formatter renders as MiB.
        const QUARTER_GIB: u64 = 256 * 1024 * 1024;
        let rows = vec![
            row(1001, Some("a.exe"), QUARTER_GIB, 0, None),
            row(1002, Some("b.exe"), QUARTER_GIB, 0, None),
        ];
        assert_eq!(
            format_ps_summary(&rows, None, None),
            "2 GPU processes found (512 MiB committed total)."
        );
    }

    // -- protected-count parenthetical --

    #[test]
    fn format_ps_summary_one_protected_appends_parenthetical() {
        let mut rows = unprotected_rows(3);
        rows.extend(protected_rows(1));
        assert_eq!(
            format_ps_summary(&rows, None, None),
            "4 GPU processes found (0 MiB committed total; 1 protected — re-run elevated for names)."
        );
    }

    #[test]
    fn format_ps_summary_many_protected_appends_parenthetical() {
        let mut rows = unprotected_rows(28);
        rows.extend(protected_rows(4));
        assert_eq!(
            format_ps_summary(&rows, None, None),
            "32 GPU processes found (0 MiB committed total; 4 protected — re-run elevated for names)."
        );
    }

    #[test]
    fn format_ps_summary_all_protected() {
        let rows = protected_rows(3);
        assert_eq!(
            format_ps_summary(&rows, None, None),
            "3 GPU processes found (0 MiB committed total; 3 protected — re-run elevated for names)."
        );
    }

    #[test]
    fn format_ps_summary_zero_protected_elides_protected_part_keeps_total() {
        // No protected rows → no `M protected …` clause, but the
        // committed-total parenthetical still appears.
        assert_eq!(
            format_ps_summary(&unprotected_rows(5), None, None),
            "5 GPU processes found (0 MiB committed total)."
        );
    }

    #[test]
    fn format_ps_summary_protected_with_filters_both_appear() {
        let mut rows = unprotected_rows(2);
        rows.extend(protected_rows(1));
        assert_eq!(
            format_ps_summary(&rows, Some(42), Some(0)),
            "3 GPU processes found matching pid=42 device=0 (0 MiB committed total; 1 protected — re-run elevated for names)."
        );
    }

    #[test]
    fn format_ps_summary_bracket_protected_string_counts_as_protected() {
        // Windows-only `[protected]` synthetic name (the
        // Toolhelp32Snapshot fallback itself could not be taken) counts
        // toward the same "re-run elevated" hint as a bare `name: None`
        // row, even though `name` is `Some` here.
        let mut rows = unprotected_rows(2);
        rows.push(row(3000, Some("[protected]"), 0, 0, None));
        assert_eq!(
            format_ps_summary(&rows, None, None),
            "3 GPU processes found (0 MiB committed total; 1 protected — re-run elevated for names)."
        );
    }

    #[test]
    fn format_ps_summary_nvidia_smi_question_mark_counts_as_protected() {
        // Pre-existing (not v0.2.8-introduced) case: the pre-WDDM-2.0
        // `nvidia-smi` fallback writes a literal `"?"` name string rather
        // than `None` for a row it couldn't identify. This carries the
        // same "might resolve under elevation" meaning as `None`/
        // `[protected]` and must count toward the hint too — previously
        // it silently didn't, understating the count exactly the way
        // `[exited]` would have overstated it.
        let mut rows = unprotected_rows(2);
        rows.push(row(3002, Some("?"), 0, 0, None));
        assert_eq!(
            format_ps_summary(&rows, None, None),
            "3 GPU processes found (0 MiB committed total; 1 protected — re-run elevated for names)."
        );
    }

    #[test]
    fn format_ps_summary_bracket_exited_string_does_not_count_as_protected() {
        // `[exited]` means the process was already gone by the time of
        // the name lookup — elevation would not have helped, so it must
        // NOT inflate the protected count (this is the exact
        // overstatement the v0.2.8 dogfooding report flagged).
        let mut rows = unprotected_rows(2);
        rows.push(row(3001, Some("[exited]"), 0, 0, None));
        assert_eq!(
            format_ps_summary(&rows, None, None),
            "3 GPU processes found (0 MiB committed total)."
        );
    }

    #[test]
    fn format_ps_summary_kernel_bracket_does_not_count_as_protected() {
        // `[kernel]` (PID 4) has no executable image to resolve
        // regardless of privilege — unchanged pre-v0.2.8 behaviour,
        // re-asserted here alongside the new bracket-counting tests.
        let mut rows = unprotected_rows(2);
        rows.push(row(4, Some("[kernel]"), 0, 0, None));
        assert_eq!(
            format_ps_summary(&rows, None, None),
            "3 GPU processes found (0 MiB committed total)."
        );
    }

    // --- exit_code_byte (spill exit-code pass-through) ---

    #[test]
    fn exit_code_byte_zero_passes_through() {
        assert_eq!(exit_code_byte(Some(0)), 0);
    }

    #[test]
    fn exit_code_byte_passthrough_255() {
        assert_eq!(exit_code_byte(Some(7)), 7);
        assert_eq!(exit_code_byte(Some(255)), 255);
    }

    #[test]
    fn exit_code_byte_negative_is_one() {
        // Windows NTSTATUS codes surface as negative i32 (e.g. an
        // access violation 0xC0000005); never truncate.
        assert_eq!(exit_code_byte(Some(-1_073_741_819)), 1);
        assert_eq!(exit_code_byte(Some(-1)), 1);
    }

    #[test]
    fn exit_code_byte_overflow_is_one() {
        // Truncating 256 to u8 would yield 0 — a false success.
        assert_eq!(exit_code_byte(Some(256)), 1);
        assert_eq!(exit_code_byte(Some(i32::MAX)), 1);
    }

    #[test]
    fn exit_code_byte_none_is_one() {
        // Signal-killed child on Unix: no exit code.
        assert_eq!(exit_code_byte(None), 1);
    }

    // --- duration helpers ---

    #[test]
    fn duration_ms_and_format_secs() {
        assert_eq!(duration_ms(Duration::from_millis(3_800)), 3_800);
        assert_eq!(duration_ms(Duration::ZERO), 0);
        assert_eq!(format_secs(Duration::from_millis(3_800)), "3.8s");
        assert_eq!(format_secs(Duration::ZERO), "0.0s");
    }

    // --- ps argument parsing (--sort) ---

    #[test]
    fn ps_args_sort_defaults_to_dedicated() {
        let cli = Cli::try_parse_from(["hmn", "ps"]).unwrap();
        let Some(Commands::Ps { sort, .. }) = cli.command else {
            panic!("expected Ps subcommand");
        };
        assert_eq!(sort, SortKey::Dedicated);
    }

    #[test]
    fn ps_args_sort_accepts_each_key() {
        for (flag, expected) in [
            ("dedicated", SortKey::Dedicated),
            ("shared", SortKey::Shared),
            ("total", SortKey::Total),
        ] {
            let cli = Cli::try_parse_from(["hmn", "ps", "--sort", flag]).unwrap();
            let Some(Commands::Ps { sort, .. }) = cli.command else {
                panic!("expected Ps subcommand");
            };
            assert_eq!(sort, expected, "--sort {flag}");
        }
    }

    #[test]
    fn ps_args_sort_accepts_dedicated_aliases() {
        // `vram` and `committed` are the words the rest of the tool's
        // own vocabulary uses for this quantity (the `ps` column header
        // and `watch`'s `COMMITTED` column, respectively) — both should
        // resolve to the same ordering as the canonical `dedicated`.
        for flag in ["vram", "committed"] {
            let cli = Cli::try_parse_from(["hmn", "ps", "--sort", flag]).unwrap();
            let Some(Commands::Ps { sort, .. }) = cli.command else {
                panic!("expected Ps subcommand");
            };
            assert_eq!(sort, SortKey::Dedicated, "--sort {flag}");
        }
    }

    #[test]
    fn ps_args_sort_rejects_unknown_key() {
        assert!(Cli::try_parse_from(["hmn", "ps", "--sort", "bogus"]).is_err());
    }

    // --- spill argument parsing ---

    #[test]
    fn spill_args_parse_trailing_command_with_hyphen_values() {
        let cli = Cli::try_parse_from([
            "hmn",
            "spill",
            "--interval",
            "50",
            "--",
            "python",
            "train.py",
            "--lr",
            "0.1",
        ])
        .unwrap();
        let Some(Commands::Spill {
            interval,
            device,
            json,
            command,
        }) = cli.command
        else {
            panic!("expected Spill subcommand");
        };
        assert_eq!(interval, 50);
        assert_eq!(device, 0);
        assert!(!json);
        assert_eq!(command, ["python", "train.py", "--lr", "0.1"]);
    }

    #[test]
    fn spill_args_default_interval_100() {
        let cli = Cli::try_parse_from(["hmn", "spill", "--", "sleep", "1"]).unwrap();
        let Some(Commands::Spill { interval, .. }) = cli.command else {
            panic!("expected Spill subcommand");
        };
        assert_eq!(interval, 100);
    }

    #[test]
    fn spill_args_requires_command() {
        assert!(Cli::try_parse_from(["hmn", "spill"]).is_err());
    }

    #[test]
    fn spill_args_rejects_zero_interval() {
        // A 0 ms interval would busy-loop PDH collects against the
        // wrapped command; floored at 1 by the value_parser range.
        assert!(Cli::try_parse_from(["hmn", "spill", "--interval", "0", "--", "x"]).is_err());
        assert!(Cli::try_parse_from(["hmn", "spill", "--interval", "1", "--", "x"]).is_ok());
    }

    // --- spill report formatting (fixtures via the test-helpers
    //     builder: SpillReport is #[non_exhaustive], so the binary
    //     cannot struct-literal one — see SpillReportBuilder docs) ---

    #[cfg(feature = "test-helpers")]
    fn spilling_report() -> SpillReport {
        const GIB: u64 = 1024 * 1024 * 1024;
        SpillReport::builder()
            .measurable(true)
            .observations(200)
            .peak_dedicated_bytes(16 * GIB)
            .dedicated_limit_bytes(16 * GIB)
            .peak_shared_bytes(4 * GIB + 200 * 1024 * 1024) // 4.2 GiB
            .baseline_shared_bytes(300 * 1024 * 1024)
            .episode(
                "+12.4s",
                Some("+15.5s"),
                3 * GIB,
                31,
                Duration::from_millis(3_100),
            )
            .episode("+20.0s", None, 4 * GIB, 67, Duration::from_millis(6_700))
            .build()
    }

    #[cfg(feature = "test-helpers")]
    #[test]
    fn format_spill_report_episodes_line() {
        let s = format_spill_report(&spilling_report());
        let expected = "hmn spill: peak dedicated 16.0 GiB / 16.0 GiB\n           peak shared    4.2 GiB (baseline 300 MiB)\n           episodes       2 — total 9.8s, longest 6.7s, first +12.4s into run\n";
        assert_eq!(s, expected);
    }

    #[cfg(feature = "test-helpers")]
    #[test]
    fn format_spill_report_no_episodes() {
        const GIB: u64 = 1024 * 1024 * 1024;
        let report = SpillReport::builder()
            .measurable(true)
            .observations(50)
            .peak_dedicated_bytes(14 * GIB)
            .dedicated_limit_bytes(16 * GIB)
            .peak_shared_bytes(140 * 1024 * 1024)
            .baseline_shared_bytes(134 * 1024 * 1024)
            .build();
        let s = format_spill_report(&report);
        assert!(s.contains("episodes       0 — no spill observed"));
        assert!(s.contains("peak dedicated 14.0 GiB / 16.0 GiB"));
    }

    #[cfg(feature = "test-helpers")]
    #[test]
    fn format_spill_report_unknown_limit_elides_suffix() {
        let report = SpillReport::builder()
            .measurable(true)
            .peak_dedicated_bytes(1024 * 1024 * 1024)
            .build();
        let s = format_spill_report(&report);
        assert!(s.contains("peak dedicated 1.0 GiB\n"));
        assert!(!s.contains(" / "));
    }

    #[cfg(feature = "test-helpers")]
    #[test]
    fn format_spill_json_shape() {
        let s = format_spill_json(&spilling_report());
        assert!(s.starts_with("{\"measurable\":true,\"spilled\":true,\"observations\":200,"));
        assert!(s.contains("\"total_spill_duration_ms\":9800,"));
        assert!(s.contains("\"episodes\":[{\"start_label\":\"+12.4s\",\"end_label\":\"+15.5s\","));
        assert!(s.ends_with("]}\n"));
    }

    #[cfg(feature = "test-helpers")]
    #[test]
    fn format_spill_json_null_end_label() {
        let s = format_spill_json(&spilling_report());
        assert!(s.contains("\"start_label\":\"+20.0s\",\"end_label\":null,"));
    }

    #[cfg(feature = "test-helpers")]
    #[test]
    fn format_spill_json_escapes_labels() {
        let report = SpillReport::builder()
            .measurable(true)
            .episode("weird\"label", None, 0, 1, Duration::ZERO)
            .build();
        let s = format_spill_json(&report);
        assert!(s.contains(r#""start_label":"weird\"label""#));
    }

    #[test]
    fn spill_json_unmeasurable_constant_is_valid_shape() {
        // The hard-error fallback object mirrors format_spill_json's
        // field order so scripted consumers parse one shape.
        assert!(SPILL_JSON_UNMEASURABLE.starts_with("{\"measurable\":false,\"spilled\":false,"));
        assert!(SPILL_JSON_UNMEASURABLE.ends_with("\"episodes\":[]}\n"));
    }

    // --- format_spill_report_with_prefix (generalized under `hmn watch`) ---

    #[cfg(feature = "test-helpers")]
    #[test]
    fn format_spill_report_with_prefix_watch_matches_spill_shape() {
        // Same content as `format_spill_report`, just under the `hmn
        // watch` prefix — both prefixes are 9 chars, so the continuation
        // indent (11 spaces) is identical.
        let report = spilling_report();
        let s = format_spill_report_with_prefix("hmn watch", &report);
        let expected = "hmn watch: peak dedicated 16.0 GiB / 16.0 GiB\n           peak shared    4.2 GiB (baseline 300 MiB)\n           episodes       2 — total 9.8s, longest 6.7s, first +12.4s into run\n";
        assert_eq!(s, expected);
    }

    // --- write_episodes_json / format_spill_json parity after refactor ---

    #[cfg(feature = "test-helpers")]
    #[test]
    fn write_episodes_json_matches_format_spill_json_episodes() {
        let report = spilling_report();
        let mut direct = String::new();
        write_episodes_json(&mut direct, &report.episodes);
        let whole = format_spill_json(&report);
        assert!(whole.contains(&direct));
    }

    // --- parse_duration ---

    #[test]
    fn parse_duration_bare_number_is_seconds() {
        assert_eq!(parse_duration("30").unwrap(), Duration::from_secs(30));
    }

    #[test]
    fn parse_duration_seconds_suffix() {
        assert_eq!(parse_duration("30s").unwrap(), Duration::from_secs(30));
    }

    #[test]
    fn parse_duration_ms_suffix() {
        assert_eq!(parse_duration("500ms").unwrap(), Duration::from_millis(500));
    }

    #[test]
    fn parse_duration_minutes_suffix() {
        assert_eq!(parse_duration("5m").unwrap(), Duration::from_secs(300));
    }

    #[test]
    fn parse_duration_hours_suffix() {
        assert_eq!(parse_duration("1h").unwrap(), Duration::from_secs(3_600));
    }

    #[test]
    fn parse_duration_rejects_empty() {
        assert!(parse_duration("").is_err());
        assert!(parse_duration("s").is_err());
    }

    #[test]
    fn parse_duration_rejects_unknown_unit() {
        assert!(parse_duration("30x").is_err());
    }

    #[test]
    fn parse_duration_rejects_zero() {
        assert!(parse_duration("0").is_err());
        assert!(parse_duration("0s").is_err());
        assert!(parse_duration("0ms").is_err());
    }

    #[test]
    fn parse_duration_trims_whitespace() {
        assert_eq!(parse_duration(" 30s ").unwrap(), Duration::from_secs(30));
    }

    // --- format_delta ---

    #[test]
    fn format_delta_zero_is_plus_zero_bytes() {
        assert_eq!(format_delta(0), "+0 B");
    }

    #[test]
    fn format_delta_positive_mib() {
        assert_eq!(format_delta(700 * 1024 * 1024), "+700 MiB");
    }

    #[test]
    fn format_delta_negative_gib() {
        let one_point_two_gib = -(1024_i64 * 1024 * 1024 + 1024 * 1024 * 1024 / 5);
        assert_eq!(format_delta(one_point_two_gib), "-1.2 GiB");
    }

    // --- signed_delta ---

    #[test]
    fn signed_delta_basic() {
        assert_eq!(signed_delta(100, 40), 60);
        assert_eq!(signed_delta(40, 100), -60);
        assert_eq!(signed_delta(0, 0), 0);
    }

    // --- watch_exit_code ---

    #[test]
    fn watch_exit_code_clean_and_spilled() {
        assert_eq!(watch_exit_code(false), 0);
        assert_eq!(watch_exit_code(true), 1);
    }

    // --- ps_row_comparator / SortKey ---

    /// Like [`row`] but with an explicit `shared_used_bytes`, needed to
    /// exercise `SortKey::Shared` / `SortKey::Total`.
    fn row_full(pid: u32, name: &str, used_bytes: u64, shared_used_bytes: u64) -> PsRow {
        PsRow {
            pid,
            name: Some(name.to_owned()),
            used_bytes,
            shared_used_bytes,
            device_index: 0,
            device_name: None,
        }
    }

    fn sorted_pids(rows: &mut [PsRow], key: SortKey) -> Vec<u32> {
        rows.sort_by(ps_row_comparator(key));
        rows.iter().map(|r| r.pid).collect()
    }

    #[test]
    fn ps_row_comparator_dedicated_descending() {
        let mut rows = vec![
            row_full(1, "a.exe", 1_000, 9_000),
            row_full(2, "b.exe", 5_000, 0),
            row_full(3, "c.exe", 3_000, 0),
        ];
        assert_eq!(sorted_pids(&mut rows, SortKey::Dedicated), vec![2, 3, 1]);
    }

    #[test]
    fn ps_row_comparator_shared_descending() {
        let mut rows = vec![
            row_full(1, "a.exe", 1_000, 9_000),
            row_full(2, "b.exe", 5_000, 0),
            row_full(3, "c.exe", 3_000, 2_000),
        ];
        assert_eq!(sorted_pids(&mut rows, SortKey::Shared), vec![1, 3, 2]);
    }

    #[test]
    fn ps_row_comparator_total_descending_differs_from_dedicated_and_shared() {
        // pid 1: total 10_000 (highest) but neither dedicated- nor
        // shared-highest alone — only `total` puts it first.
        let mut rows = vec![
            row_full(1, "a.exe", 4_000, 6_000),
            row_full(2, "b.exe", 8_000, 0),
            row_full(3, "c.exe", 0, 7_000),
        ];
        assert_eq!(sorted_pids(&mut rows, SortKey::Total), vec![1, 2, 3]);
        // Confirms neither single-field key would have produced this order.
        assert_eq!(sorted_pids(&mut rows, SortKey::Dedicated), vec![2, 1, 3]);
        assert_eq!(sorted_pids(&mut rows, SortKey::Shared), vec![3, 1, 2]);
    }

    #[test]
    fn ps_row_comparator_tie_break_identical_across_keys() {
        // Two rows tied on every numeric field: every key must fall
        // through to the same name-then-PID tie-break.
        let mut rows = vec![
            row_full(20, "b.exe", 1_000, 1_000),
            row_full(10, "a.exe", 1_000, 1_000),
        ];
        for key in [SortKey::Dedicated, SortKey::Shared, SortKey::Total] {
            assert_eq!(sorted_pids(&mut rows, key), vec![10, 20], "key {key:?}");
        }
    }

    #[test]
    fn ps_row_comparator_total_saturates_instead_of_overflowing() {
        // Pathological but must not panic: plain `+` on two `u64::MAX`
        // values panics in debug builds (and silently wraps in
        // release); `saturating_add` does neither and still orders
        // this row correctly ahead of a small, unambiguous total.
        let mut rows = vec![
            row_full(1, "a.exe", u64::MAX, u64::MAX),
            row_full(2, "b.exe", 1_000, 0),
        ];
        assert_eq!(sorted_pids(&mut rows, SortKey::Total), vec![1, 2]);
    }

    // --- select_top_n_pids ---

    #[test]
    fn select_top_n_pids_orders_by_committed_descending() {
        let rows = vec![
            row(1, Some("a.exe"), 1_000, 0, None),
            row(2, Some("b.exe"), 5_000, 0, None),
            row(3, Some("c.exe"), 3_000, 0, None),
        ];
        assert_eq!(select_top_n_pids(&rows, 2), vec![2, 3]);
    }

    #[test]
    fn select_top_n_pids_n_larger_than_rows_returns_all() {
        let rows = vec![row(1, Some("a.exe"), 1_000, 0, None)];
        assert_eq!(select_top_n_pids(&rows, 5), vec![1]);
    }

    #[test]
    fn select_top_n_pids_empty_rows() {
        assert!(select_top_n_pids(&[], 5).is_empty());
    }

    #[test]
    fn select_top_n_pids_ties_break_by_pid_ascending() {
        let rows = vec![
            row(20, Some("b.exe"), 1_000, 0, None),
            row(10, Some("a.exe"), 1_000, 0, None),
        ];
        assert_eq!(select_top_n_pids(&rows, 2), vec![10, 20]);
    }

    #[test]
    fn select_top_n_pids_ties_break_by_name_before_pid() {
        // Shares ps_row_comparator with `hmn ps`: a tie on used_bytes
        // breaks by name first, PID only as the final fallback. Name
        // and PID order deliberately *disagree* here (the lower PID, 1,
        // carries the alphabetically-later name) so this fixture can
        // actually distinguish "name-then-PID" from the old "PID-only"
        // rule: a PID-only tie-break would produce `[1, 99]`; the real
        // (name-first) comparator produces `[99, 1]`.
        let rows = vec![
            row(1, Some("z.exe"), 1_000, 0, None),
            row(99, Some("a.exe"), 1_000, 0, None),
        ];
        assert_eq!(select_top_n_pids(&rows, 2), vec![99, 1]);
    }

    // --- format_watch_rows_text / format_watch_header_text ---

    fn watch_row(
        pid: u32,
        name: Option<&str>,
        used: u64,
        used_delta: i64,
        shared: u64,
        shared_delta: i64,
        spilling: bool,
    ) -> WatchSampleRow {
        WatchSampleRow {
            pid,
            name: name.map(str::to_owned),
            used_bytes: used,
            used_delta,
            shared_bytes: shared,
            shared_delta,
            spilling,
        }
    }

    #[test]
    fn format_watch_header_text_has_expected_columns() {
        let h = format_watch_header_text();
        assert!(h.contains("TIME"));
        assert!(h.contains("PID"));
        assert!(h.contains("NAME"));
        assert!(h.contains("COMMITTED"));
        assert!(h.contains("SHARED"));
        assert!(h.contains("SPILL"));
    }

    #[test]
    fn format_watch_rows_text_single_row() {
        let r = watch_row(
            12345,
            Some("python.exe"),
            8 * 1024 * 1024 * 1024,
            0,
            142 * 1024 * 1024,
            0,
            false,
        );
        let s = format_watch_rows_text(Duration::from_secs(5), &[r]);
        assert!(s.starts_with("+5.0s"));
        assert!(s.contains("12345"));
        assert!(s.contains("python.exe"));
        assert!(s.contains("8.0 GiB"));
        assert!(s.contains("142 MiB"));
        assert!(s.contains("+0 B"));
        assert!(s.contains("no"));
    }

    #[test]
    fn format_watch_rows_text_spilling_row() {
        let r = watch_row(
            12345,
            Some("python.exe"),
            16 * 1024 * 1024 * 1024,
            700 * 1024 * 1024,
            2 * 1024 * 1024 * 1024,
            576 * 1024 * 1024,
            true,
        );
        let s = format_watch_rows_text(Duration::from_secs(10), &[r]);
        assert!(s.contains("+700 MiB"));
        assert!(s.contains("+576 MiB"));
        assert!(s.contains("SPILL"));
    }

    #[test]
    fn format_watch_rows_text_missing_name_renders_question_mark() {
        let r = watch_row(99, None, 0, 0, 0, 0, false);
        let s = format_watch_rows_text(Duration::ZERO, &[r]);
        assert!(s.contains("99"));
        assert!(s.contains('?'));
    }

    // --- format_watch_rows_json ---

    #[test]
    fn format_watch_rows_json_shape() {
        let r = watch_row(
            7,
            Some("py.exe"),
            1_048_576,
            1_048_576,
            424_242,
            -1_000,
            true,
        );
        let s = format_watch_rows_json(Duration::from_millis(3_500), &[r]);
        assert!(s.starts_with(
            r#"{"kind":"sample","t_ms":3500,"pid":7,"name":"py.exe","used_bytes":1048576,"used_delta_bytes":1048576,"shared_used_bytes":424242,"shared_delta_bytes":-1000,"spilling":true}"#
        ));
        assert!(s.ends_with('\n'));
    }

    #[test]
    fn format_watch_rows_json_null_name() {
        let r = watch_row(7, None, 0, 0, 0, 0, false);
        let s = format_watch_rows_json(Duration::ZERO, &[r]);
        assert!(s.contains(r#""name":null,"#));
    }

    #[test]
    fn format_watch_rows_json_multiple_rows_multiple_lines() {
        let rows = vec![
            watch_row(1, Some("a.exe"), 0, 0, 0, 0, false),
            watch_row(2, Some("b.exe"), 0, 0, 0, 0, false),
        ];
        let s = format_watch_rows_json(Duration::ZERO, &rows);
        assert_eq!(s.lines().count(), 2);
    }

    // --- process_sample ---

    #[cfg(feature = "test-helpers")]
    fn entry(
        pid: u32,
        name: Option<&str>,
        used_bytes: u64,
        shared_used_bytes: u64,
    ) -> GpuProcessEntry {
        GpuProcessEntry::builder()
            .pid(pid)
            .name(name.map(str::to_owned))
            .used_bytes(used_bytes)
            .shared_used_bytes(shared_used_bytes)
            .build()
    }

    #[cfg(feature = "test-helpers")]
    #[test]
    fn process_sample_first_interval_zero_delta() {
        let mut state = WatchState::new();
        let rows = vec![entry(100, Some("python.exe"), 8_000, 100)];
        let out = process_sample(&rows, &mut state, &[100], Duration::ZERO, None);
        assert_eq!(out.len(), 1);
        let row = out.first().unwrap();
        assert_eq!(row.used_bytes, 8_000);
        assert_eq!(row.used_delta, 0);
        assert_eq!(row.shared_delta, 0);
        assert!(!row.spilling);
    }

    #[cfg(feature = "test-helpers")]
    #[test]
    fn process_sample_second_interval_computes_delta() {
        let mut state = WatchState::new();
        let rows0 = vec![entry(100, Some("python.exe"), 8_000, 100)];
        let _ = process_sample(&rows0, &mut state, &[100], Duration::ZERO, None);
        let rows1 = vec![entry(100, Some("python.exe"), 9_500, 300)];
        let out = process_sample(&rows1, &mut state, &[100], Duration::from_secs(5), None);
        let row = out.first().unwrap();
        assert_eq!(row.used_delta, 1_500);
        assert_eq!(row.shared_delta, 200);
    }

    #[cfg(feature = "test-helpers")]
    #[test]
    fn process_sample_missing_pid_renders_zero() {
        let mut state = WatchState::new();
        let rows: Vec<GpuProcessEntry> = vec![];
        let out = process_sample(&rows, &mut state, &[42], Duration::ZERO, None);
        assert_eq!(out.len(), 1);
        let row = out.first().unwrap();
        assert_eq!(row.pid, 42);
        assert_eq!(row.used_bytes, 0);
        assert_eq!(row.name, None);
    }

    #[cfg(feature = "test-helpers")]
    #[test]
    fn process_sample_peak_tracked_across_samples() {
        let mut state = WatchState::new();
        let rows0 = vec![entry(100, Some("python.exe"), 8_000, 100)];
        let _ = process_sample(&rows0, &mut state, &[100], Duration::ZERO, None);
        let rows1 = vec![entry(100, Some("python.exe"), 5_000, 50)];
        let _ = process_sample(&rows1, &mut state, &[100], Duration::from_secs(5), None);
        let s = state.by_pid.get(&100).unwrap();
        assert_eq!(s.peak_used_bytes, 8_000);
        assert_eq!(s.peak_shared_bytes, 100);
        assert_eq!(s.baseline_used_bytes, 8_000);
    }

    #[cfg(feature = "test-helpers")]
    #[test]
    fn process_sample_name_change_resets_baseline_and_peak() {
        // Simulates the OS recycling pid=100 mid-watch: python.exe ran
        // for a while (baseline/peak grow), then exits and the PID is
        // reassigned to an unrelated notepad.exe. The resolved-name
        // change must reset the row's baseline/peak to the new
        // process's reading rather than mixing the two.
        let mut state = WatchState::new();
        let rows0 = vec![entry(100, Some("python.exe"), 8_000, 100)];
        let _ = process_sample(&rows0, &mut state, &[100], Duration::ZERO, None);
        let rows1 = vec![entry(100, Some("python.exe"), 9_000, 500)];
        let _ = process_sample(&rows1, &mut state, &[100], Duration::from_secs(5), None);

        let rows2 = vec![entry(100, Some("notepad.exe"), 200, 10)];
        let out = process_sample(&rows2, &mut state, &[100], Duration::from_secs(10), None);
        let row = out.first().unwrap();
        assert_eq!(row.name.as_deref(), Some("notepad.exe"));
        // Delta is 0 on the reset sample — comparing against the
        // recycled PID's own (much larger) prior reading would be
        // meaningless.
        assert_eq!(row.used_delta, 0);
        assert_eq!(row.shared_delta, 0);

        let s = state.by_pid.get(&100).unwrap();
        assert_eq!(s.baseline_used_bytes, 200);
        assert_eq!(s.baseline_shared_bytes, 10);
        assert_eq!(s.peak_used_bytes, 200);
        assert_eq!(s.peak_shared_bytes, 10);
    }

    #[cfg(feature = "test-helpers")]
    #[test]
    fn process_sample_unresolved_name_does_not_trigger_reset() {
        // A `?` sample between two resolved samples of the SAME name
        // must not be treated as a reuse — last_name stays sticky
        // across the None sample, so the baseline is undisturbed.
        let mut state = WatchState::new();
        let rows0 = vec![entry(100, Some("python.exe"), 8_000, 100)];
        let _ = process_sample(&rows0, &mut state, &[100], Duration::ZERO, None);
        let rows1 = vec![entry(100, None, 8_500, 150)];
        let _ = process_sample(&rows1, &mut state, &[100], Duration::from_secs(5), None);
        let rows2 = vec![entry(100, Some("python.exe"), 9_000, 200)];
        let out = process_sample(&rows2, &mut state, &[100], Duration::from_secs(10), None);
        let row = out.first().unwrap();
        // Baseline never reset: delta is against the unresolved
        // sample's prev (8_500 / 150), not a fresh 0.
        assert_eq!(row.used_delta, 500);
        assert_eq!(row.shared_delta, 50);
        assert_eq!(state.by_pid.get(&100).unwrap().baseline_used_bytes, 8_000);
    }

    #[cfg(feature = "test-helpers")]
    #[test]
    fn process_sample_protected_bracket_flicker_does_not_trigger_reset() {
        // Same shape as the `None`-flicker test above, but for the
        // Windows-only `[protected]` synthetic bracket: a transient
        // Toolhelp32Snapshot failure on one interval must not look like
        // "the OS recycled this PID" and reset the baseline.
        let mut state = WatchState::new();
        let rows0 = vec![entry(100, Some("python.exe"), 8_000, 100)];
        let _ = process_sample(&rows0, &mut state, &[100], Duration::ZERO, None);
        let rows1 = vec![entry(100, Some("[protected]"), 8_500, 150)];
        let _ = process_sample(&rows1, &mut state, &[100], Duration::from_secs(5), None);
        let rows2 = vec![entry(100, Some("python.exe"), 9_000, 200)];
        let out = process_sample(&rows2, &mut state, &[100], Duration::from_secs(10), None);
        let row = out.first().unwrap();
        assert_eq!(row.used_delta, 500);
        assert_eq!(row.shared_delta, 50);
        assert_eq!(state.by_pid.get(&100).unwrap().baseline_used_bytes, 8_000);
    }

    #[cfg(feature = "test-helpers")]
    #[test]
    fn process_sample_reset_survives_a_protected_flicker_in_between() {
        // A `[protected]` flicker must not mask a GENUINE later name
        // change either: last_name should stay "python.exe" (sticky
        // across the flicker), so the real reuse at rows2 still resets.
        let mut state = WatchState::new();
        let rows0 = vec![entry(100, Some("python.exe"), 8_000, 100)];
        let _ = process_sample(&rows0, &mut state, &[100], Duration::ZERO, None);
        let rows1 = vec![entry(100, Some("[protected]"), 8_500, 150)];
        let _ = process_sample(&rows1, &mut state, &[100], Duration::from_secs(5), None);
        let rows2 = vec![entry(100, Some("notepad.exe"), 200, 10)];
        let out = process_sample(&rows2, &mut state, &[100], Duration::from_secs(10), None);
        let row = out.first().unwrap();
        assert_eq!(row.used_delta, 0);
        assert_eq!(row.shared_delta, 0);
        let s = state.by_pid.get(&100).unwrap();
        assert_eq!(s.baseline_used_bytes, 200);
        assert_eq!(s.baseline_shared_bytes, 10);
    }

    #[cfg(feature = "test-helpers")]
    #[test]
    fn process_sample_growth_hint_fires_for_protected_bracket() {
        // The one-shot "unresolved pid grew" stderr hint must still
        // fire for the Windows-only `[protected]` bracket, not just a
        // bare `None` — otherwise the hint would go silent on Windows
        // now that most `?` rows resolve via the Toolhelp32Snapshot
        // fallback and only genuinely-protected rows stay unresolved.
        let mut state = WatchState::new();
        let rows0 = vec![entry(100, Some("[protected]"), 0, 0)];
        let _ = process_sample(&rows0, &mut state, &[100], Duration::ZERO, None);
        let grown = UNRESOLVED_GROWTH_HINT_BYTES;
        let rows1 = vec![entry(100, Some("[protected]"), grown, 0)];
        let _ = process_sample(&rows1, &mut state, &[100], Duration::from_secs(5), None);
        assert!(state.by_pid.get(&100).unwrap().growth_hint_fired);
    }

    #[cfg(feature = "test-helpers")]
    #[test]
    fn process_sample_growth_hint_does_not_fire_for_exited_bracket() {
        // `[exited]` means the process was already confirmed gone —
        // "growth" on a gone process is meaningless, so the hint (whose
        // wording promises "re-run elevated to identify") must not fire
        // for it the way it does for `None`/`[protected]`.
        let mut state = WatchState::new();
        let rows0 = vec![entry(100, Some("[exited]"), 0, 0)];
        let _ = process_sample(&rows0, &mut state, &[100], Duration::ZERO, None);
        let grown = UNRESOLVED_GROWTH_HINT_BYTES;
        let rows1 = vec![entry(100, Some("[exited]"), grown, 0)];
        let _ = process_sample(&rows1, &mut state, &[100], Duration::from_secs(5), None);
        assert!(!state.by_pid.get(&100).unwrap().growth_hint_fired);
    }

    // --- resolved_name (PID-reuse comparison filter) ---

    #[test]
    fn resolved_name_passes_through_real_names() {
        assert_eq!(resolved_name(Some("python.exe")), Some("python.exe"));
    }

    #[test]
    fn resolved_name_passes_through_kernel_bracket() {
        // [kernel] (PID 4) is permanently stable and never flickers —
        // safe to treat as a comparable name, unlike [protected]/[exited].
        assert_eq!(resolved_name(Some("[kernel]")), Some("[kernel]"));
    }

    #[test]
    fn resolved_name_filters_protected_and_exited_brackets() {
        assert_eq!(resolved_name(Some("[protected]")), None);
        assert_eq!(resolved_name(Some("[exited]")), None);
    }

    #[test]
    fn resolved_name_filters_none() {
        assert_eq!(resolved_name(None), None);
    }

    // --- WatchState::track (--follow-new seen_order bookkeeping) ---

    #[test]
    fn watch_state_track_records_first_seen_order_once() {
        let mut state = WatchState::new();
        state.track(100, 1_000, 0);
        state.track(200, 2_000, 0);
        // Re-tracking an already-seen PID must not append a second
        // seen_order entry, nor reset its state.
        state.track(100, 9_000, 0);
        assert_eq!(state.seen_order, vec![100, 200]);
        assert_eq!(state.by_pid.get(&100).unwrap().baseline_used_bytes, 1_000);
    }

    #[test]
    fn watch_state_track_returns_mutable_existing_entry() {
        let mut state = WatchState::new();
        state.track(100, 1_000, 0).peak_used_bytes = 5_000;
        // The second `track` call for the same PID must see the
        // mutation the first call's caller made through the returned
        // reference, not a fresh `WatchedPidState`.
        assert_eq!(state.track(100, 1_000, 0).peak_used_bytes, 5_000);
    }

    #[cfg(feature = "test-helpers")]
    #[test]
    fn watch_state_seen_order_survives_pid_dropping_out_of_watched() {
        // Simulates --follow-new: pid 100 is watched for one interval,
        // genuinely excluded from `watched` the next (dropped below
        // top-N, but still alive and still present in `rows` — only
        // absent from the `watched` slice `process_sample` is given),
        // then re-enters. seen_order must record it exactly once, at
        // its first sighting, and its accumulated state must survive
        // the gap untouched rather than drifting or resetting.
        let mut state = WatchState::new();
        let rows0 = vec![
            entry(100, Some("a.exe"), 1_000, 0),
            entry(200, Some("b.exe"), 500, 0),
        ];
        let _ = process_sample(&rows0, &mut state, &[100, 200], Duration::ZERO, None);

        // pid 100 is genuinely excluded from `watched` this interval
        // even though it's still present in `rows` at a different
        // reading (9_000) — process_sample must not touch its state
        // at all while it's excluded.
        let rows1 = vec![
            entry(100, Some("a.exe"), 9_000, 0),
            entry(200, Some("b.exe"), 600, 0),
        ];
        let _ = process_sample(&rows1, &mut state, &[200], Duration::from_secs(5), None);
        assert_eq!(
            state.by_pid.get(&100).unwrap().peak_used_bytes,
            1_000,
            "excluded PID's state must be untouched while absent from `watched`"
        );

        // pid 100 re-enters `watched`; its delta is against its own
        // pre-gap prev (1_000), not the 9_000 it drifted to while
        // excluded — process_sample never saw that reading.
        let rows2 = vec![
            entry(100, Some("a.exe"), 1_200, 0),
            entry(200, Some("b.exe"), 600, 0),
        ];
        let out = process_sample(
            &rows2,
            &mut state,
            &[100, 200],
            Duration::from_secs(10),
            None,
        );
        let row100 = out.iter().find(|r| r.pid == 100).unwrap();
        assert_eq!(row100.used_delta, 200); // 1_200 - 1_000, not 1_200 - 9_000
        assert_eq!(state.by_pid.get(&100).unwrap().peak_used_bytes, 1_200);
        assert_eq!(state.seen_order, vec![100, 200]);
    }

    // --- resolve_watched_pids ---

    #[cfg(feature = "test-helpers")]
    #[test]
    fn resolve_watched_pids_explicit_passthrough_ignores_rows_and_top() {
        let rows = vec![entry(1, Some("a.exe"), 9_000, 0)];
        assert_eq!(resolve_watched_pids(&rows, &[42, 43], 1), vec![42, 43]);
    }

    #[cfg(feature = "test-helpers")]
    #[test]
    fn resolve_watched_pids_auto_selects_top_n_from_rows() {
        let rows = vec![
            entry(1, Some("a.exe"), 1_000, 0),
            entry(2, Some("b.exe"), 5_000, 0),
            entry(3, Some("c.exe"), 3_000, 0),
        ];
        assert_eq!(resolve_watched_pids(&rows, &[], 2), vec![2, 3]);
    }

    #[test]
    fn resolve_watched_pids_empty_rows_and_explicit_is_empty() {
        assert!(resolve_watched_pids(&[], &[], 5).is_empty());
    }

    // --- format_followed_set_change (--follow-new stderr breadcrumb) ---

    #[cfg(feature = "test-helpers")]
    #[test]
    fn format_followed_set_change_none_when_unchanged() {
        let rows = vec![entry(1, Some("a.exe"), 1_000, 0)];
        let state = WatchState::new();
        assert!(format_followed_set_change(&[1], &[1], &rows, &state, Duration::ZERO).is_none());
    }

    #[cfg(feature = "test-helpers")]
    #[test]
    fn format_followed_set_change_reports_entered_with_name_from_rows() {
        let rows = vec![entry(2, Some("new.exe"), 1_000, 0)];
        let state = WatchState::new();
        let msg = format_followed_set_change(&[1], &[1, 2], &rows, &state, Duration::from_secs(10))
            .unwrap();
        assert!(msg.contains("entered pid=2 (new.exe)"), "{msg}");
        assert!(!msg.contains("left"), "{msg}");
    }

    #[cfg(feature = "test-helpers")]
    #[test]
    fn format_followed_set_change_reports_left_with_name_from_state() {
        // The departed PID is, by construction, absent from the current
        // sample's `rows` — its name must come from `state`'s last
        // known reading instead.
        let rows: Vec<GpuProcessEntry> = vec![];
        let mut state = WatchState::new();
        state.track(1, 1_000, 0).last_name = Some("gone.exe".to_owned());
        let msg =
            format_followed_set_change(&[1], &[], &rows, &state, Duration::from_secs(10)).unwrap();
        assert!(msg.contains("left pid=1 (gone.exe)"), "{msg}");
        assert!(!msg.contains("entered"), "{msg}");
    }

    #[cfg(feature = "test-helpers")]
    #[test]
    fn format_followed_set_change_unresolved_name_renders_bare_pid() {
        let rows = vec![entry(2, None, 1_000, 0)];
        let state = WatchState::new();
        let msg = format_followed_set_change(&[1], &[1, 2], &rows, &state, Duration::ZERO).unwrap();
        assert!(msg.contains("entered pid=2"), "{msg}");
        assert!(!msg.contains("pid=2 ("), "{msg}");
    }

    #[cfg(feature = "test-helpers")]
    #[test]
    fn format_followed_set_change_reports_both_entered_and_left() {
        let rows = vec![entry(2, Some("new.exe"), 1_000, 0)];
        let state = WatchState::new();
        let msg = format_followed_set_change(&[1], &[2], &rows, &state, Duration::ZERO).unwrap();
        assert!(msg.contains("entered pid=2 (new.exe)"), "{msg}");
        assert!(msg.contains("left pid=1"), "{msg}");
    }

    // --- format_watch_per_pid_block / summary formatting ---

    fn pid_summary(
        pid: u32,
        name: Option<&str>,
        baseline_used: u64,
        peak_used: u64,
        baseline_shared: u64,
        peak_shared: u64,
    ) -> WatchPidSummary {
        WatchPidSummary {
            pid,
            name: name.map(str::to_owned),
            baseline_used_bytes: baseline_used,
            peak_used_bytes: peak_used,
            baseline_shared_bytes: baseline_shared,
            peak_shared_bytes: peak_shared,
        }
    }

    #[test]
    fn format_watch_per_pid_block_empty_is_empty_string() {
        assert_eq!(format_watch_per_pid_block(&[]), "");
    }

    #[test]
    fn format_watch_per_pid_block_single_pid() {
        let s = format_watch_per_pid_block(&[pid_summary(
            12345,
            Some("python.exe"),
            8 * 1024 * 1024 * 1024,
            9 * 1024 * 1024 * 1024,
            100 * 1024 * 1024,
            700 * 1024 * 1024,
        )]);
        assert!(s.contains("12345"));
        assert!(s.contains("python.exe"));
        assert!(s.contains("8.0 GiB"));
        assert!(s.contains("9.0 GiB"));
        assert!(s.contains("100 MiB"));
        assert!(s.contains("700 MiB"));
    }

    #[test]
    fn format_watch_summary_text_unmeasurable_notes_and_still_shows_per_pid() {
        let s = format_watch_summary_text(None, &[pid_summary(1, Some("a.exe"), 0, 0, 0, 0)]);
        assert!(s.contains("spill tracking unavailable"));
        assert!(s.contains("per-PID"));
        assert!(s.contains('1'));
    }

    #[cfg(feature = "test-helpers")]
    #[test]
    fn format_watch_summary_text_measurable_uses_watch_prefix() {
        let s = format_watch_summary_text(Some(&spilling_report()), &[]);
        assert!(s.starts_with("hmn watch: peak dedicated"));
    }

    #[test]
    fn format_watch_summary_json_unmeasurable_shape() {
        let s = format_watch_summary_json(None, &[pid_summary(1, Some("a.exe"), 10, 20, 0, 0)]);
        assert!(s.starts_with(
            r#"{"kind":"summary","measurable":false,"spilled":false,"observations":0,"#
        ));
        assert!(s.contains(r#""per_pid":[{"pid":1,"name":"a.exe","baseline_used_bytes":10,"peak_used_bytes":20,"baseline_shared_bytes":0,"peak_shared_bytes":0}]"#));
        assert!(s.ends_with("]}\n"));
    }

    #[cfg(feature = "test-helpers")]
    #[test]
    fn format_watch_summary_json_measurable_shape() {
        let s = format_watch_summary_json(Some(&spilling_report()), &[]);
        assert!(s.starts_with(r#"{"kind":"summary","measurable":true,"spilled":true,"#));
        assert!(s.contains(r#""per_pid":[]"#));
    }

    // --- Watch clap arg parsing ---

    #[test]
    fn watch_args_defaults() {
        let cli = Cli::try_parse_from(["hmn", "watch"]).unwrap();
        let Some(Commands::Watch {
            pids,
            interval,
            duration,
            top,
            follow_new,
            device,
            json,
        }) = cli.command
        else {
            panic!("expected Watch subcommand");
        };
        assert!(pids.is_empty());
        assert_eq!(interval, Duration::from_secs(5));
        assert_eq!(duration, None);
        assert_eq!(top, 5);
        assert!(!follow_new);
        assert_eq!(device, 0);
        assert!(!json);
    }

    #[test]
    fn watch_args_explicit_pids_and_overrides() {
        let cli = Cli::try_parse_from([
            "hmn",
            "watch",
            "1234",
            "5678",
            "--interval",
            "30s",
            "--duration",
            "10m",
            "--device",
            "1",
            "--json",
        ])
        .unwrap();
        let Some(Commands::Watch {
            pids,
            interval,
            duration,
            device,
            json,
            ..
        }) = cli.command
        else {
            panic!("expected Watch subcommand");
        };
        assert_eq!(pids, [1234, 5678]);
        assert_eq!(interval, Duration::from_secs(30));
        assert_eq!(duration, Some(Duration::from_secs(600)));
        assert_eq!(device, 1);
        assert!(json);
    }

    #[test]
    fn watch_args_rejects_bad_duration() {
        assert!(Cli::try_parse_from(["hmn", "watch", "--interval", "bogus"]).is_err());
        assert!(Cli::try_parse_from(["hmn", "watch", "--duration", "0"]).is_err());
    }

    #[test]
    fn watch_args_top_override() {
        let cli = Cli::try_parse_from(["hmn", "watch", "--top", "10"]).unwrap();
        let Some(Commands::Watch { top, .. }) = cli.command else {
            panic!("expected Watch subcommand");
        };
        assert_eq!(top, 10);
    }

    #[test]
    fn watch_args_follow_new_flag() {
        let cli = Cli::try_parse_from(["hmn", "watch", "--follow-new"]).unwrap();
        let Some(Commands::Watch { follow_new, .. }) = cli.command else {
            panic!("expected Watch subcommand");
        };
        assert!(follow_new);
    }

    #[test]
    fn watch_args_follow_new_with_explicit_pids_parses_clean() {
        // clap itself has no opinion on this combination — `run_watch`
        // rejects it at runtime (exit code 2, verified live/manually,
        // not here: it's a hard error path alongside the other early
        // hard-error returns in `run_watch`, none of which are unit
        // tested directly since they all require a live device query
        // or precede one). This test only pins down that clap parsing
        // itself doesn't reject the combination — it has to reach
        // `run_watch` to be caught.
        let cli = Cli::try_parse_from(["hmn", "watch", "1234", "--follow-new"]).unwrap();
        let Some(Commands::Watch {
            pids, follow_new, ..
        }) = cli.command
        else {
            panic!("expected Watch subcommand");
        };
        assert_eq!(pids, [1234]);
        assert!(follow_new);
    }
}
