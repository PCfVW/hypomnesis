// SPDX-License-Identifier: MIT OR Apache-2.0

//! `nvidia-smi` subprocess fallback (device-wide).
//!
//! Spawns
//! `nvidia-smi --query-gpu=memory.used,memory.total --format=csv,noheader,nounits --id=N`
//! and parses the single CSV line. Slower than `NVML` / `DXGI`
//! (subprocess fork + driver query) and reports device-wide totals
//! only — but works whenever the NVIDIA driver is installed even if
//! `NVML` cannot be loaded (broken `libnvidia-ml.so.1`, missing
//! `nvml.dll`, version skew). The dispatcher in `src/gpu/mod.rs`
//! treats the value as `is_per_process = false`.

use std::process::Command;

/// Successful result of an `nvidia-smi` query for a single device.
///
/// Returning `Option<NvidiaSmiResult>` from [`query`] (rather than the
/// previous `(Option<u64>, Option<u64>)` tuple) makes the all-or-nothing
/// invariant explicit at the type level: a successful query always
/// produces both `used_bytes` and `total_bytes`; any failure path
/// returns `None`. Callers no longer need to handle logically-impossible
/// mixed `(Some, None)` / `(None, Some)` states.
pub(super) struct NvidiaSmiResult {
    /// Device-wide used memory in bytes (sum across all processes).
    pub used_bytes: u64,
    /// Total dedicated memory on the device in bytes.
    pub total_bytes: u64,
}

/// Query `nvidia-smi` for device-wide memory at adapter index `idx`.
///
/// Returns `Some(NvidiaSmiResult)` on success; `None` if `nvidia-smi`
/// could not be spawned, exited non-zero, or produced unparseable
/// output.
///
/// `nvidia-smi` reports in `MiB`; values are converted to bytes via
/// saturating multiplication (overflow at the `u64` ceiling is
/// physically unreachable but defended against anyway).
pub(super) fn query(idx: u32) -> Option<NvidiaSmiResult> {
    let cmd_result = Command::new("nvidia-smi")
        .args([
            "--query-gpu=memory.used,memory.total",
            "--format=csv,noheader,nounits",
        ])
        .arg(format!("--id={idx}"))
        .output();

    let output = match cmd_result {
        Ok(o) if o.status.success() => o,
        #[cfg(feature = "debug-output")]
        Ok(o) => {
            // BORROW: explicit String::from_utf8_lossy — stderr is best-effort
            // diagnostic text and may not be UTF-8 on weird locales.
            let stderr = String::from_utf8_lossy(&o.stderr);
            eprintln!(
                "[nvidia-smi debug] subprocess for idx={idx} exited with {} \
                 (stderr trimmed: {:?})",
                o.status,
                stderr.trim(),
            );
            return None;
        }
        #[cfg(not(feature = "debug-output"))]
        Ok(_) => return None,
        #[cfg(feature = "debug-output")]
        Err(e) => {
            eprintln!("[nvidia-smi debug] failed to spawn for idx={idx}: {e}");
            return None;
        }
        #[cfg(not(feature = "debug-output"))]
        Err(_) => return None,
    };

    // BORROW: explicit String::from_utf8_lossy — `nvidia-smi --format=csv,nounits`
    // output is ASCII numerals + commas, but be defensive against locale drift.
    let stdout = String::from_utf8_lossy(&output.stdout);
    // The else block carries a cfg-gated `eprintln!` for the `debug-output`
    // feature; with that feature off, the body collapses to a bare
    // `return None` and `clippy::question_mark` (under `-D warnings` on
    // MSRV 1.88) wants `?` instead. We keep the let-else so the
    // diagnostic-on path stays consistent with the surrounding error
    // sites (spawn fail / non-zero exit / parse fail), all of which
    // also use let-else with cfg-gated debug prints.
    #[allow(clippy::question_mark)]
    let Some(line_raw) = stdout.lines().next() else {
        #[cfg(feature = "debug-output")]
        eprintln!("[nvidia-smi debug] empty stdout for idx={idx}");
        return None;
    };
    let line = line_raw.trim();

    let mut parts = line.split(',');
    let used_str = parts.next().map(str::trim)?;
    let total_str = parts.next().map(str::trim)?;

    let used_mb: u64 = match used_str.parse() {
        Ok(v) => v,
        #[cfg(feature = "debug-output")]
        Err(e) => {
            eprintln!("[nvidia-smi debug] failed to parse used '{used_str}' for idx={idx}: {e}");
            return None;
        }
        #[cfg(not(feature = "debug-output"))]
        Err(_) => return None,
    };
    let total_mb: u64 = match total_str.parse() {
        Ok(v) => v,
        #[cfg(feature = "debug-output")]
        Err(e) => {
            eprintln!("[nvidia-smi debug] failed to parse total '{total_str}' for idx={idx}: {e}");
            return None;
        }
        #[cfg(not(feature = "debug-output"))]
        Err(_) => return None,
    };

    // 1 MiB = 1_048_576 bytes; saturating in case of absurd inputs.
    let used_bytes = used_mb.saturating_mul(1_048_576);
    let total_bytes = total_mb.saturating_mul(1_048_576);

    #[cfg(feature = "debug-output")]
    eprintln!(
        "[nvidia-smi debug] idx={idx}: used={used_mb}MiB total={total_mb}MiB \
         ({used_bytes} / {total_bytes} bytes)"
    );

    Some(NvidiaSmiResult {
        used_bytes,
        total_bytes,
    })
}

/// One row of an `nvidia-smi --query-compute-apps` listing.
///
/// Returned by [`query_compute_apps`] for use by the
/// `crate::gpu_processes` dispatcher on Windows (and as a Linux
/// fallback when `NVML` is unavailable).
pub(super) struct ComputeApp {
    /// OS process ID.
    pub pid: u32,
    /// Process name as reported by `nvidia-smi`. `None` when the field
    /// is empty after trimming; `Some("?")` when `nvidia-smi` could not
    /// read the image name (protected process). Other values are the
    /// trimmed name verbatim.
    pub name: Option<String>,
    /// GPU memory used by this process in bytes. `nvidia-smi` reports
    /// `MiB`; converted via saturating multiplication.
    pub used_bytes: u64,
}

/// Query `nvidia-smi` for every compute process on adapter `idx`.
///
/// Spawns
/// `nvidia-smi --query-compute-apps=pid,process_name,used_memory --format=csv,noheader,nounits --id={idx}`
/// and parses each output line as one [`ComputeApp`]. Returns `None` if
/// the subprocess could not be spawned, exited non-zero, or produced
/// completely unparseable output. An empty stdout (no rows) is **not**
/// an error — returns `Some(Vec::new())`, since "no compute apps on
/// this device" is a valid state.
///
/// **Compute-only.** `--query-compute-apps` enumerates only processes
/// with an active `CUDA` context. Browsers using GPU compositing,
/// games, and pure-graphics apps do not appear.
///
/// # CSV parsing
///
/// Each non-empty line is split as
/// `<pid>,<process_name>,<used_memory_mib>`. Names may contain commas
/// in unusual cases (Windows accepts comma in filenames), so the
/// parser splits on the **last** comma first to isolate `used_memory`,
/// then on the **first** comma of the remainder to isolate `pid` from
/// `name`. Lines that don't split into three parts, or whose `pid` /
/// `used_memory` don't parse, are skipped (with a `debug-output`
/// trace).
pub(super) fn query_compute_apps(idx: u32) -> Option<Vec<ComputeApp>> {
    let cmd_result = Command::new("nvidia-smi")
        .args([
            "--query-compute-apps=pid,process_name,used_memory",
            "--format=csv,noheader,nounits",
        ])
        .arg(format!("--id={idx}"))
        .output();

    let output = match cmd_result {
        Ok(o) if o.status.success() => o,
        #[cfg(feature = "debug-output")]
        Ok(o) => {
            // BORROW: explicit String::from_utf8_lossy — stderr is best-effort
            // diagnostic text and may not be UTF-8 on weird locales.
            let stderr = String::from_utf8_lossy(&o.stderr);
            eprintln!(
                "[nvidia-smi debug] --query-compute-apps for idx={idx} exited with {} \
                 (stderr trimmed: {:?})",
                o.status,
                stderr.trim(),
            );
            return None;
        }
        #[cfg(not(feature = "debug-output"))]
        Ok(_) => return None,
        #[cfg(feature = "debug-output")]
        Err(e) => {
            eprintln!("[nvidia-smi debug] failed to spawn --query-compute-apps for idx={idx}: {e}");
            return None;
        }
        #[cfg(not(feature = "debug-output"))]
        Err(_) => return None,
    };

    // BORROW: explicit String::from_utf8_lossy — `nvidia-smi --format=csv,nounits`
    // is ASCII numerals + commas + process names; defensive against locale drift.
    let stdout = String::from_utf8_lossy(&output.stdout);

    let rows: Vec<ComputeApp> = stdout
        .lines()
        .filter_map(|line_raw| parse_compute_app_line(line_raw, idx))
        .collect();

    #[cfg(feature = "debug-output")]
    eprintln!(
        "[nvidia-smi debug] query_compute_apps(idx={idx}): {} row(s)",
        rows.len()
    );

    Some(rows)
}

/// Parse one `--query-compute-apps` CSV line into a [`ComputeApp`].
///
/// Returns `None` for empty / unparseable lines. Extracted for unit-testability;
/// the parser is exercised by inline tests with hand-crafted fixtures.
///
/// The `idx` parameter is only consumed by the `debug-output` traces; the
/// `cfg_attr` lets the parameter be unused when the feature is off without
/// tripping `unused_variables`.
#[cfg_attr(not(feature = "debug-output"), allow(unused_variables))]
fn parse_compute_app_line(line_raw: &str, idx: u32) -> Option<ComputeApp> {
    let line = line_raw.trim();
    if line.is_empty() {
        return None;
    }

    // Split on the LAST comma first (used_memory is the rightmost
    // field), then on the FIRST comma of the remainder (pid is leftmost).
    // This is robust to `process_name` values containing commas, which
    // Windows technically allows in filenames.
    let (rest, used_str) = line.rsplit_once(',')?;
    let (pid_str, name_str) = rest.split_once(',')?;

    let pid: u32 = match pid_str.trim().parse() {
        Ok(v) => v,
        #[cfg(feature = "debug-output")]
        Err(e) => {
            eprintln!(
                "[nvidia-smi debug] --query-compute-apps idx={idx}: failed to parse pid {pid_str:?}: {e}"
            );
            return None;
        }
        #[cfg(not(feature = "debug-output"))]
        Err(_) => return None,
    };

    let used_mb: u64 = match used_str.trim().parse() {
        Ok(v) => v,
        #[cfg(feature = "debug-output")]
        Err(e) => {
            eprintln!(
                "[nvidia-smi debug] --query-compute-apps idx={idx}: failed to parse used_memory \
                 {used_str:?} for pid {pid}: {e}"
            );
            return None;
        }
        #[cfg(not(feature = "debug-output"))]
        Err(_) => return None,
    };

    let trimmed_name = name_str.trim();
    // BORROW: explicit to_owned — name_str is a &str borrowed from
    // stdout; the returned ComputeApp must own its name.
    let name = if trimmed_name.is_empty() {
        None
    } else {
        Some(trimmed_name.to_owned())
    };

    // 1 MiB = 1_048_576 bytes; saturating in case of absurd inputs.
    let used_bytes = used_mb.saturating_mul(1_048_576);

    Some(ComputeApp {
        pid,
        name,
        used_bytes,
    })
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::missing_docs_in_private_items
)]
mod tests {
    use super::*;

    #[test]
    fn parse_compute_app_basic() {
        let row = parse_compute_app_line("12345, python.exe, 1024", 0).unwrap();
        assert_eq!(row.pid, 12345);
        assert_eq!(row.name.as_deref(), Some("python.exe"));
        assert_eq!(row.used_bytes, 1024 * 1_048_576);
    }

    #[test]
    fn parse_compute_app_protected_name() {
        // `nvidia-smi` writes `?` literally for protected processes whose
        // image name it cannot read. Preserved as-is.
        let row = parse_compute_app_line("999, ?, 256", 0).unwrap();
        assert_eq!(row.pid, 999);
        assert_eq!(row.name.as_deref(), Some("?"));
        assert_eq!(row.used_bytes, 256 * 1_048_576);
    }

    #[test]
    fn parse_compute_app_name_with_comma() {
        // Windows technically allows comma in filenames. The
        // last-comma-first split policy isolates used_memory robustly
        // even when the name contains a comma.
        let row = parse_compute_app_line("42, weird,name.exe, 8", 0).unwrap();
        assert_eq!(row.pid, 42);
        assert_eq!(row.name.as_deref(), Some("weird,name.exe"));
        assert_eq!(row.used_bytes, 8 * 1_048_576);
    }

    #[test]
    fn parse_compute_app_empty_line() {
        assert!(parse_compute_app_line("", 0).is_none());
        assert!(parse_compute_app_line("   ", 0).is_none());
    }

    #[test]
    fn parse_compute_app_unparseable_pid() {
        assert!(parse_compute_app_line("notanumber, python.exe, 1024", 0).is_none());
    }

    #[test]
    fn parse_compute_app_unparseable_memory() {
        assert!(parse_compute_app_line("123, python.exe, notanumber", 0).is_none());
    }

    #[test]
    fn parse_compute_app_too_few_fields() {
        // Only one comma → only two fields; the dispatcher should drop.
        assert!(parse_compute_app_line("123, python.exe", 0).is_none());
        // No commas at all → can't split.
        assert!(parse_compute_app_line("12345", 0).is_none());
    }
}
