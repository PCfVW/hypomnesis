# hypomnesis examples

Runnable example binaries living under `examples/`. Each is feature-gated
via `[[example]] required-features = [...]` in [`Cargo.toml`](../Cargo.toml).

## `print_demo` — `MemoryReport` printing helpers

Exercises [`MemoryReport::print_delta`](../src/report.rs) and
[`MemoryReport::print_before_after`](../src/report.rs) (and their
`format_*` String-returning siblings) against live snapshots. Useful as
a smoke test, a copy-paste starting point for new consumers, and the
fastest way to *see* what the formatted output looks like on your
machine.

```sh
cargo run --features report --example print_demo
```

To also see the raw `NVML` / `DXGI` / `nvidia-smi` diagnostic traces:

```sh
cargo run --features "report debug-output" --example print_demo
```

Expected output (your numbers will differ):

```
--- hypomnesis print_demo ---
--- print_delta ---
  alloc 50 MiB: RAM +19 MB  |  VRAM +0 MB [per-process]
--- print_before_after ---
  alloc 50 MiB: RAM 6 MB → 25 MB (+19 MB)
  alloc 50 MiB: VRAM 0 MB → 0 MB (+0 MB / 16311 MB) [per-process] [NVIDIA GeForce RTX 5060 Ti]
--- format_delta (returned as String, no newline added by us) ---
  alloc 50 MiB: RAM +19 MB  |  VRAM +0 MB [per-process]
--- format_before_after (returned as String, no newline added by us) ---
  alloc 50 MiB: RAM 6 MB → 25 MB (+19 MB)
  alloc 50 MiB: VRAM 0 MB → 0 MB (+0 MB / 16311 MB) [per-process] [NVIDIA GeForce RTX 5060 Ti]
```

Notes on the output:

- The 50 MiB allocation typically shows up as a smaller RAM delta because
  the OS commits pages lazily — the `Vec<u8>` zeroing forces commit on
  Linux but Windows is more relaxed.
- `VRAM +0 MB` is correct for a non-CUDA test process: `DXGI`'s
  `CurrentUsage` is 0 when this process has no `D3D` / `DXGI` allocations.
  On a process that *does* allocate VRAM (e.g. a candle inference run),
  the delta would be the real per-process VRAM increase.
- The adapter name `NVIDIA GeForce RTX 5060 Ti` comes from `DXGI`'s
  `DXGI_ADAPTER_DESC.Description` on Windows; on Linux it comes from
  `nvmlDeviceGetName`.
- `[per-process]` means the source backend is `DXGI` or `NVML` (true
  per-process tracking). On Linux, a non-CUDA test binary falls back to
  `nvidia-smi` and shows `[device-wide]` instead.
