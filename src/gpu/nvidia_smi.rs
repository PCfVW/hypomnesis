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
