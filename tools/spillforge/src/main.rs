//! `spillforge` — forced-`WDDM`-spill fixture for validating hypomnesis
//! spill detection. **Windows-only**; not part of the published crate.
//!
//! Allocates `D3D11` default-heap buffers *with initial-data uploads*
//! past the card's dedicated `VRAM`, then keeps the whole working set
//! **hot** (round-robin 64 KiB touches) so `VidMm` must keep resources
//! resident — pegging dedicated-resident at its true budget ceiling and
//! pushing the overflow into shared-system-memory residency: a real,
//! reproducible spill.
//!
//! Two lessons this tool encodes (learned during v0.2.5 validation —
//! see `docs/roadmap-v0.2.5.md`, "Implementation notes (as shipped)"):
//!
//! 1. **Commit alone produces no spill.** Buffers created without
//!    initial data are committed but never resident — dedicated usage
//!    barely moves and shared stays flat. The upload is what makes
//!    residency real.
//! 2. **An idle working set gets evicted to backing store, not to
//!    shared residency.** Without the touch loop, `VidMm` demotes
//!    untouched resources out of GPU visibility entirely and the
//!    shared gauge under-reports. The churn keeps the demand honest.
//!
//! Usage (run under `hmn spill` to watch the detection fire):
//!
//! ```sh
//! cargo build --release --manifest-path tools/spillforge/Cargo.toml
//! hmn spill -- tools/spillforge/target/release/spillforge.exe [TARGET_GIB] [HOLD_SECS]
//! ```
//!
//! `TARGET_GIB` defaults to 20 (comfortably past a 16 GiB card — pick
//! ~1.25× your dedicated `VRAM`); `HOLD_SECS` defaults to 10. Expect
//! the desktop to get sluggish during the churn; everything releases on
//! exit. Reference-card result (RTX 5060 Ti 16 GiB, 2026-07-22): one
//! 13.1 s episode, peak shared 3.1 GiB over a 163 MiB baseline, peak
//! dedicated 91.3% of `DXGI` capacity.

use windows::Win32::Foundation::HMODULE;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Graphics::Direct3D11::{
    D3D11_BIND_SHADER_RESOURCE, D3D11_BOX, D3D11_BUFFER_DESC, D3D11_CREATE_DEVICE_FLAG,
    D3D11_SDK_VERSION, D3D11_SUBRESOURCE_DATA, D3D11_USAGE_DEFAULT, D3D11CreateDevice,
    ID3D11Buffer, ID3D11Device, ID3D11DeviceContext,
};

/// Bytes per buffer. 256 MiB keeps each allocation well inside D3D11's
/// per-resource limits while reaching the target in few enough chunks.
const CHUNK: u32 = 256 * 1024 * 1024;

fn main() {
    let mut args = std::env::args().skip(1);
    let target_gib: u64 = args
        .next()
        .and_then(|a| a.parse().ok())
        .unwrap_or(20);
    let hold_secs: u64 = args
        .next()
        .and_then(|a| a.parse().ok())
        .unwrap_or(10);

    let mut device: Option<ID3D11Device> = None;
    let mut context: Option<ID3D11DeviceContext> = None;
    unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_FLAG(0),
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )
    }
    .expect("D3D11CreateDevice failed");
    let device = device.expect("no device");
    let context = context.expect("no context");

    let desc = D3D11_BUFFER_DESC {
        ByteWidth: CHUNK,
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
        StructureByteStride: 0,
    };

    // Initial data forces a real upload: the resource becomes RESIDENT
    // (not merely committed) — without this, VidMm never pages anything
    // and there is honestly no spill to detect (lesson 1).
    let seed: Vec<u8> = vec![0xA5; CHUNK as usize];
    let init = D3D11_SUBRESOURCE_DATA {
        pSysMem: seed.as_ptr().cast(),
        SysMemPitch: 0,
        SysMemSlicePitch: 0,
    };

    let target_chunks = (target_gib * 1024 * 1024 * 1024 / CHUNK as u64) as usize;
    let mut buffers: Vec<ID3D11Buffer> = Vec::with_capacity(target_chunks);
    for i in 0..target_chunks {
        let mut buf: Option<ID3D11Buffer> = None;
        match unsafe { device.CreateBuffer(&desc, Some(&init), Some(&mut buf)) } {
            Ok(()) => {
                if let Some(b) = buf {
                    buffers.push(b);
                }
            }
            Err(e) => {
                eprintln!("spillforge: CreateBuffer #{i} failed ({e}); stopping allocation");
                break;
            }
        }
        // Flush so the upload actually executes rather than queuing.
        unsafe { context.Flush() };
        if (i + 1) % 8 == 0 {
            eprintln!(
                "spillforge: allocated+uploaded {} GiB",
                (i + 1) as u64 * CHUNK as u64 / (1024 * 1024 * 1024)
            );
        }
    }

    // Hold phase: keep the whole working set HOT by round-robin touching
    // every buffer (64 KiB UpdateSubresource each) — forces VidMm to
    // keep resources resident, pegging dedicated at its true budget
    // ceiling and pushing the overflow into shared residency (lesson 2).
    eprintln!(
        "spillforge: churning {} GiB working set for {hold_secs} s...",
        buffers.len() as u64 * CHUNK as u64 / (1024 * 1024 * 1024)
    );
    let patch = vec![0x5Au8; 64 * 1024];
    // Destination box: only the first 64 KiB of each buffer — without
    // it, UpdateSubresource copies the WHOLE 256 MiB resource from the
    // 64 KiB source (out-of-bounds read, instant crash).
    let dst_box = D3D11_BOX {
        left: 0,
        top: 0,
        front: 0,
        right: 64 * 1024,
        bottom: 1,
        back: 1,
    };
    let hold_until = std::time::Instant::now() + std::time::Duration::from_secs(hold_secs);
    let mut i = 0usize;
    while std::time::Instant::now() < hold_until {
        let b = &buffers[i % buffers.len()];
        unsafe {
            context.UpdateSubresource(b, 0, Some(&dst_box), patch.as_ptr().cast(), 0, 0);
            context.Flush();
        }
        i += 1;
    }
    eprintln!("spillforge: touched {i} buffers total; releasing and exiting");
}
