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

/// Query `nvidia-smi` for device-wide memory at adapter index `idx`.
///
/// Returns `(used_bytes, total_bytes)`, with each component `None`
/// if `nvidia-smi` could not be spawned, exited non-zero, or
/// produced unparseable output.
///
/// `nvidia-smi` reports in `MiB`; values are converted to bytes via
/// saturating multiplication (overflow at the `u64` ceiling is
/// physically unreachable but defended against anyway).
pub fn query(idx: u32) -> (Option<u64>, Option<u64>) {
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
            return (None, None);
        }
        #[cfg(not(feature = "debug-output"))]
        Ok(_) => return (None, None),
        #[cfg(feature = "debug-output")]
        Err(e) => {
            eprintln!("[nvidia-smi debug] failed to spawn for idx={idx}: {e}");
            return (None, None);
        }
        #[cfg(not(feature = "debug-output"))]
        Err(_) => return (None, None),
    };

    // BORROW: explicit String::from_utf8_lossy — `nvidia-smi --format=csv,nounits`
    // output is ASCII numerals + commas, but be defensive against locale drift.
    let stdout = String::from_utf8_lossy(&output.stdout);
    let Some(line_raw) = stdout.lines().next() else {
        #[cfg(feature = "debug-output")]
        eprintln!("[nvidia-smi debug] empty stdout for idx={idx}");
        return (None, None);
    };
    let line = line_raw.trim();

    let mut parts = line.split(',');
    let Some(used_str) = parts.next().map(str::trim) else {
        return (None, None);
    };
    let Some(total_str) = parts.next().map(str::trim) else {
        return (None, None);
    };

    let used_mb: u64 = match used_str.parse() {
        Ok(v) => v,
        #[cfg(feature = "debug-output")]
        Err(e) => {
            eprintln!("[nvidia-smi debug] failed to parse used '{used_str}' for idx={idx}: {e}");
            return (None, None);
        }
        #[cfg(not(feature = "debug-output"))]
        Err(_) => return (None, None),
    };
    let total_mb: u64 = match total_str.parse() {
        Ok(v) => v,
        #[cfg(feature = "debug-output")]
        Err(e) => {
            eprintln!("[nvidia-smi debug] failed to parse total '{total_str}' for idx={idx}: {e}");
            return (None, None);
        }
        #[cfg(not(feature = "debug-output"))]
        Err(_) => return (None, None),
    };

    // 1 MiB = 1_048_576 bytes; saturating in case of absurd inputs.
    let used_bytes = used_mb.saturating_mul(1_048_576);
    let total_bytes = total_mb.saturating_mul(1_048_576);

    #[cfg(feature = "debug-output")]
    eprintln!(
        "[nvidia-smi debug] idx={idx}: used={used_mb}MiB total={total_mb}MiB \
         ({used_bytes} / {total_bytes} bytes)"
    );

    (Some(used_bytes), Some(total_bytes))
}
