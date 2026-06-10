// SPDX-License-Identifier: MIT OR Apache-2.0

//! `hmn` — GPU memory CLI for `hypomnesis` (built only when the
//! default-off `cli` feature is enabled).
//!
//! Subcommands:
//!
//! - `hmn` (default) — one line per visible GPU with free / total `VRAM`.
//!   Uses [`hypomnesis::Snapshot::all`], so on Windows the AMD / Intel
//!   `iGPU` surfaces alongside the NVIDIA dGPU(s); on macOS the Apple
//!   Silicon `SoC` surfaces as a single `UMA` device.
//! - `hmn ps` — list processes holding GPU memory across one or all
//!   visible devices. On Linux (`NVML`) the list is compute-only; on
//!   Windows (`PDH`, `WDDM 2.0`+) the list includes every GPU memory
//!   holder (compositor, browsers, games, compute); on macOS (Metal
//!   ledger) the list enumerates every same-user PID holding
//!   `graphics_footprint` bytes. See the `--help` Limitations text and
//!   the rustdoc for [`hypomnesis::gpu_processes`] for the
//!   per-platform breakdown.
//!
//! Install with `cargo install hypomnesis --features cli`.

use std::fmt::Write as _;

use clap::{Parser, Subcommand};
use hypomnesis::{Result, Snapshot, device_count, device_info, gpu_processes};

/// `hmn` CLI: device summary plus GPU-process listing.
#[derive(Parser, Debug)]
#[command(
    name = "hmn",
    version,
    about = "GPU memory CLI: device summary (default) + GPU-process listing (`hmn ps`).",
    long_about = "GPU memory CLI for hypomnesis.\n\
                  \n\
                  Default subcommand: prints one line per visible GPU with free / total VRAM.\n\
                  \n\
                  `hmn ps`: lists processes holding GPU memory.\n\
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
                  - `?` in the NAME column on Windows means the calling user cannot resolve \
                  the process's name via `OpenProcess`. Most cases (system services, \
                  other-user processes like `dwm.exe`, `csrss.exe`) resolve when `hmn ps` \
                  is run as Administrator. The Windows kernel itself (PID 4) is rendered \
                  as `[kernel]`, not `?`, so it does not pollute the suspicious-`?` set. \
                  PPL-protected processes (Windows Defender, anti-cheat engines) would \
                  remain `?` even elevated, but typically do not appear in `hmn ps` output \
                  unless they are actively holding GPU memory.\n\
                  - Security note: a `?` row holding substantial VRAM that does not resolve \
                  under elevation is worth investigating — by construction it is either a \
                  process owned by another user / running as SYSTEM, a PPL-protected process, \
                  or a transient race. None of these are intrinsically malicious, but on a \
                  single-user desktop none of them should hold large amounts of GPU memory \
                  unexpectedly. The protected-count parenthetical on the summary line is \
                  intentionally surfaced because this distinction matters.\n\
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
        /// Emit a JSON array (one object per row) instead of the
        /// default text table. Each object has fields `pid` (number),
        /// `name` (string or null), `used_bytes` (number),
        /// `device_index` (number), `device_name` (string or null).
        #[arg(long)]
        json: bool,
    },
}

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    let outcome = match cli.command {
        None => run_summary(),
        Some(Commands::Ps { pid, device, json }) => run_ps(pid, device, json),
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

/// Run the default subcommand: print one line per visible GPU.
fn run_summary() -> Result<()> {
    let snaps = Snapshot::all()?;
    if snaps.is_empty() {
        println!("hmn: no visible GPUs.");
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
        // `writeln!` into a String never fails — the writes-to-String
        // impl returns Ok(()). Same for every other write!/writeln! in
        // this file.
        let _ = writeln!(
            out,
            "GPU {}{name_suffix}: free {free_mib} MiB / {total_mib} MiB",
            dev.index,
        );
    }
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
    /// GPU memory used by this process in bytes.
    used_bytes: u64,
    /// Zero-based device index (NVML-canonical).
    device_index: u32,
    /// Friendly device name (e.g. `RTX 5060 Ti`); `None` when
    /// `device_info` failed for this index.
    device_name: Option<String>,
}

/// Run the `ps` subcommand: collect process rows for the selected
/// device(s), apply the `--pid` filter, then emit either a text table
/// or JSON.
//
// Returns `Result<()>` for symmetry with `run_summary` so `main` can
// dispatch through one match arm. The body never produces an `Err` (per-device
// failures are swallowed via `continue` so one broken device doesn't kill the
// whole listing); the lint is allowed for that reason.
#[allow(clippy::unnecessary_wraps)]
fn run_ps(pid_filter: Option<u32>, device_filter: Option<u32>, json: bool) -> Result<()> {
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
                device_index: idx,
                // BORROW: clone — device_name is shared across all
                // rows for this device.
                device_name: device_name.clone(),
            });
        }
    }

    // Human-facing display order: VRAM descending so the biggest
    // consumers land at the top (the row a user asking "what's
    // eating my GPU memory?" wants to see first), name ascending
    // for tie-break grouping (duplicate-name processes like
    // `msedgewebview2.exe` cluster together), then PID ascending
    // for stable order across runs when name + bytes tie. The
    // library's `gpu_processes()` returns rows PID-sorted; this
    // overrides that for display only.
    rows.sort_by(|a, b| {
        b.used_bytes
            .cmp(&a.used_bytes)
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.pid.cmp(&b.pid))
    });

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
/// `<N> GPU process[es] found[ matching <filters>][ (<M> protected — re-run elevated for names)].`
///
/// Three appendices, each elided when not applicable:
///
/// - **Filter clause** (` matching pid=N device=M`): appended only
///   when at least one filter is active.
/// - **Protected clause** (` (<M> protected — re-run elevated for names)`):
///   appended only when at least one row has `name: None`. Surfaces
///   the actionable hint that Administrator-level access would
///   resolve those names — most are system services or other-user
///   processes the calling user can't `OpenProcess` against. PID 4
///   (the Windows kernel pseudo-process) is special-cased to render
///   as `[kernel]` rather than `?` and therefore does **not**
///   contribute to this count; the remaining count is genuinely
///   foreign-user / `SYSTEM` / `PPL`-protected / transient-race
///   processes that elevation can actually help with.
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
    let protected = rows.iter().filter(|r| r.name.is_none()).count();
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
    let device_header = "DEVICE";

    let pid_cells: Vec<String> = rows.iter().map(|r| r.pid.to_string()).collect();
    let name_cells: Vec<&str> = rows
        .iter()
        .map(|r| r.name.as_deref().unwrap_or("?"))
        .collect();
    let vram_cells: Vec<String> = rows.iter().map(|r| format_vram(r.used_bytes)).collect();
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
    let device_w = column_width(device_header, device_cells.iter().map(String::as_str));

    let mut out = String::new();
    let _ = writeln!(
        out,
        "{pid_header:<pid_w$}  {name_header:<name_w$}  {vram_header:<vram_w$}  {device_header:<device_w$}",
    );
    for (((pid, name), vram), device) in pid_cells
        .iter()
        .zip(&name_cells)
        .zip(&vram_cells)
        .zip(&device_cells)
    {
        let _ = writeln!(
            out,
            "{pid:<pid_w$}  {name:<name_w$}  {vram:<vram_w$}  {device:<device_w$}",
        );
    }
    out
}

/// Format `ps` rows as a JSON array, one object per row. Hand-rolled
/// (no `serde` dep — keeps the `cli` feature lean for v0.2). Each
/// object: `{"pid":N,"name":<string|null>,"used_bytes":N,"device_index":N,"device_name":<string|null>}`.
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
            r#"{{"pid":{},"name":{name_json},"used_bytes":{},"device_index":{},"device_name":{device_name_json}}}"#,
            row.pid, row.used_bytes, row.device_index,
        );
    }
    out.push_str("]\n");
    out
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
    clippy::missing_docs_in_private_items
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
            device_index,
            device_name: device_name.map(str::to_owned),
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
        assert_eq!(s, "PID  NAME  VRAM  DEVICE\n");
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
        let expected = "PID    NAME        VRAM     DEVICE     \n\
                        12345  python.exe  8.0 GiB  RTX 5060 Ti\n";
        assert_eq!(s, expected);
    }

    #[test]
    fn format_ps_table_protected_name_renders_question_mark() {
        // Column widths: PID=3 (header), NAME=4 (header), VRAM=7
        // ("256 MiB"), DEVICE=11 ("RTX 5060 Ti"). Two-space separators.
        let r = row(99, Some("?"), 268_435_456, 0, Some("RTX 5060 Ti"));
        let s = format_ps_table(&[r]);
        let expected = "PID  NAME  VRAM     DEVICE     \n\
                        99   ?     256 MiB  RTX 5060 Ti\n";
        assert_eq!(s, expected);
    }

    #[test]
    fn format_ps_table_missing_name_renders_question_mark() {
        // Missing name (None) renders identically to the protected `?`
        // case — both go through the `unwrap_or("?")` path.
        let r = row(99, None, 268_435_456, 0, Some("RTX 5060 Ti"));
        let s = format_ps_table(&[r]);
        let expected = "PID  NAME  VRAM     DEVICE     \n\
                        99   ?     256 MiB  RTX 5060 Ti\n";
        assert_eq!(s, expected);
    }

    #[test]
    fn format_ps_table_falls_back_to_gpu_n_when_no_device_name() {
        let r = row(99, Some("python.exe"), 268_435_456, 3, None);
        let s = format_ps_table(&[r]);
        assert!(s.contains("python.exe  256 MiB  GPU 3"));
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
            "[{\"pid\":12345,\"name\":\"python.exe\",\"used_bytes\":8388608,\"device_index\":0,\"device_name\":\"RTX 5060 Ti\"}]\n"
        );
    }

    #[test]
    fn format_ps_json_null_name() {
        let r = row(42, None, 0, 0, None);
        let s = format_ps_json(&[r]);
        assert_eq!(
            s,
            "[{\"pid\":42,\"name\":null,\"used_bytes\":0,\"device_index\":0,\"device_name\":null}]\n"
        );
    }

    #[test]
    fn format_ps_json_two_rows_comma_separated() {
        let a = row(1, Some("a.exe"), 1_048_576, 0, Some("GPU"));
        let b = row(2, Some("b.exe"), 2_097_152, 0, Some("GPU"));
        let s = format_ps_json(&[a, b]);
        assert_eq!(
            s,
            "[{\"pid\":1,\"name\":\"a.exe\",\"used_bytes\":1048576,\"device_index\":0,\"device_name\":\"GPU\"},\
             {\"pid\":2,\"name\":\"b.exe\",\"used_bytes\":2097152,\"device_index\":0,\"device_name\":\"GPU\"}]\n"
        );
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
}
