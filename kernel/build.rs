//! Kernel build script — hooks the Limine linker script into rustc.
//!
//! Without this, the kernel would use the default rust-lld script and fail to
//! place the `.requests` section + higher-half virtual addresses expected by
//! the Limine Boot Protocol.

fn main() {
    // Pass the linker script to rust-lld. The script lives next to Cargo.toml.
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let linker_script = format!("{}/linker.ld", manifest_dir);
    println!("cargo:rerun-if-changed={}", linker_script);
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rustc-link-arg=-T{}", linker_script);
    // Disable PIE — the linker script hard-codes the kernel load address at
    // 0xffffffff80000000. rust-lld would otherwise emit a DYN ELF that Limine
    // relocates; this is fine but produces noisier diagnostics. Force static.
    println!("cargo:rustc-link-arg=--no-dynamic-linker");
    println!("cargo:rustc-link-arg=-static");
    // Ensure the bootloader can find the requests by keeping .requests markers.
    println!("cargo:rustc-link-arg=-z");
    println!("cargo:rustc-link-arg=max-page-size=0x1000");
}
