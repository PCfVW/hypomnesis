// SPDX-License-Identifier: MIT OR Apache-2.0

//! `MemoryReport` printing demo — exercises `print_delta` and
//! `print_before_after` against live measurements.
//!
//! Run with:
//! ```sh
//! cargo run --features report --example print_demo
//! ```
//!
//! Or to also see raw NVML / DXGI / nvidia-smi diagnostic output:
//! ```sh
//! cargo run --features "report debug-output" --example print_demo
//! ```

use hypomnesis::{MemoryReport, Snapshot};

#[allow(clippy::expect_used)]
fn main() {
    println!("--- hypomnesis print_demo ---");

    let before = Snapshot::now(0).expect("Snapshot::now failed");

    // Allocate ~50 MiB on the heap to produce a visible RAM delta.
    // Use vec![0_u8; ...] (zeroed allocation) so the OS commits pages.
    let hold: Vec<u8> = vec![0_u8; 50 * 1024 * 1024];

    let after = Snapshot::now(0).expect("Snapshot::now failed");
    let report = MemoryReport::new(before, after);

    println!("--- print_delta ---");
    report.print_delta("alloc 50 MiB");

    println!("--- print_before_after ---");
    report.print_before_after("alloc 50 MiB");

    println!("--- format_delta (returned as String, no newline added by us) ---");
    print!("{}", report.format_delta("alloc 50 MiB"));

    println!("--- format_before_after (returned as String, no newline added by us) ---");
    print!("{}", report.format_before_after("alloc 50 MiB"));

    // Keep the allocation alive until here so the after-snapshot still
    // reflects it (otherwise the optimizer could elide `hold`).
    drop(hold);
}
