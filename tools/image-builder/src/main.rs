//! Disk image builder for ClaudioOS.
//!
//! Takes the compiled kernel binary and creates bootable UEFI and BIOS disk images
//! using the `bootloader` crate's image builder.
//!
//! Usage:
//!   cargo run --package claudio-image-builder -- <path-to-kernel-binary>
//!
//! Example:
//!   cargo run --package claudio-image-builder -- target/x86_64-unknown-none/debug/claudio-os

use std::path::{Path, PathBuf};

fn main() {
    let kernel_path = std::env::args()
        .nth(1)
        .expect("usage: claudio-image-builder <kernel-binary-path>");

    let kernel_path = Path::new(&kernel_path);
    if !kernel_path.exists() {
        eprintln!("error: kernel binary not found at {:?}", kernel_path);
        eprintln!("hint: run `cargo build` first to compile the kernel");
        std::process::exit(1);
    }

    let out_dir = kernel_path
        .parent()
        .unwrap_or(Path::new("."));

    // Create UEFI disk image
    let uefi_path = out_dir.join("claudio-os-uefi.img");
    println!("[image] creating UEFI disk image at {:?}", uefi_path);
    let uefi_builder = bootloader::UefiBoot::new(kernel_path);
    uefi_builder
        .create_disk_image(&uefi_path)
        .expect("failed to create UEFI disk image");
    println!("[image] UEFI image: {:?} ({} bytes)", uefi_path, std::fs::metadata(&uefi_path).map(|m| m.len()).unwrap_or(0));

    // Create BIOS disk image (for legacy boot / simpler QEMU invocation)
    let bios_path = out_dir.join("claudio-os-bios.img");
    println!("[image] creating BIOS disk image at {:?}", bios_path);
    let bios_builder = bootloader::BiosBoot::new(kernel_path);
    bios_builder
        .create_disk_image(&bios_path)
        .expect("failed to create BIOS disk image");
    println!("[image] BIOS image: {:?} ({} bytes)", bios_path, std::fs::metadata(&bios_path).map(|m| m.len()).unwrap_or(0));

    println!();
    println!("[image] done! To boot in QEMU:");
    println!();
    println!("  UEFI boot (requires OVMF):");
    println!("    qemu-system-x86_64 \\");
    println!("      -bios /usr/share/OVMF/OVMF_CODE.fd \\");
    println!("      -drive format=raw,file={} \\", uefi_path.display());
    println!("      -serial stdio -m 512M", );
    println!();
    println!("  BIOS boot (no OVMF needed):");
    println!("    qemu-system-x86_64 \\");
    println!("      -drive format=raw,file={} \\", bios_path.display());
    println!("      -serial stdio -m 512M");
}
