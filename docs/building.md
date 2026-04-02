# Building ClaudioOS

Comprehensive guide for building, running, and debugging ClaudioOS on Windows,
Linux, and macOS.

---

## Table of Contents

- [Prerequisites](#prerequisites)
- [Two-Step Build Process](#two-step-build-process)
- [Running in QEMU](#running-in-qemu)
- [run.ps1 Explained](#runps1-explained)
- [Session Persistence](#session-persistence)
- [Troubleshooting](#troubleshooting)
- [The Stack Overflow Fix](#the-stack-overflow-fix)
- [Environment Variables](#environment-variables)
- [Project File Layout](#project-file-layout)

---

## Prerequisites

### Rust Toolchain

ClaudioOS requires **nightly Rust**. The `rust-toolchain.toml` in the repository root
automatically installs the correct toolchain on first build:

```toml
[toolchain]
channel = "nightly"
components = ["rust-src", "rustfmt", "clippy", "llvm-tools-preview"]
targets = ["x86_64-unknown-none"]
```

- **`rust-src`**: Required by `-Zbuild-std` for building `core` and `alloc` from
  source for the freestanding target
- **`llvm-tools-preview`**: Required by the bootloader's image builder for
  `llvm-objcopy` (converts ELF to raw binary)
- **`x86_64-unknown-none`**: The bare-metal target (no OS, no libc, no std)

You do **not** need to manually run `rustup target add` -- it happens automatically.

### Build Configuration

The `.cargo/config.toml` sets important build parameters:

```toml
[build]
target = "x86_64-unknown-none"

[unstable]
build-std = ["core", "alloc"]
build-std-features = ["compiler-builtins-mem"]
```

- **`target = "x86_64-unknown-none"`**: All `cargo build` commands default to the
  bare-metal target. The image builder is excluded from the workspace to avoid this.
- **`build-std = ["core", "alloc"]`**: Builds the standard library from source with
  our target's settings. Required because `x86_64-unknown-none` has no pre-built
  standard library.
- **`compiler-builtins-mem`**: Provides `memcpy`, `memset`, `memcmp` implementations
  that would normally come from libc.

### Platform-Specific Requirements

#### Windows

- **MSVC Build Tools**: The image builder (`tools/image-builder/`) is a host-side
  binary that links with the MSVC linker. Install "Desktop development with C++"
  from Visual Studio, or the MSVC Build Tools standalone installer.
- **LIB environment variable**: If the linker finds `link.exe` but not `kernel32.lib`,
  set the `LIB` variable to MSVC + Windows SDK lib directories:
  ```powershell
  $env:LIB = "C:\Program Files\Microsoft Visual Studio\2022\Community\VC\Tools\MSVC\14.39.33519\lib\x64;C:\Program Files (x86)\Windows Kits\10\Lib\10.0.22621.0\ucrt\x64;C:\Program Files (x86)\Windows Kits\10\Lib\10.0.22621.0\um\x64"
  ```
- **QEMU for Windows**: Download from https://www.qemu.org/download/#windows.
  Add the QEMU install directory to your PATH.
- **OVMF** (for UEFI boot): QEMU for Windows may not include OVMF firmware.
  Download from https://retrage.github.io/edk2-nightly/ or use BIOS boot instead.

#### Linux

```bash
# Debian/Ubuntu
sudo apt install qemu-system-x86 ovmf

# Arch Linux
sudo pacman -S qemu-system-x86 edk2-ovmf

# Fedora
sudo dnf install qemu-system-x86 edk2-ovmf
```

The Rust toolchain "just works" on Linux -- no special setup beyond having a C
linker available (for the image builder, which is a host-side binary).

#### macOS

```bash
brew install qemu
# OVMF firmware is included with Homebrew's QEMU package
```

---

## Two-Step Build Process

Building ClaudioOS is a two-step process because the bootloader crate's disk image
builder is a separate host-side tool.

### Step 1: Compile the Kernel

```bash
cargo build
```

This compiles the `claudio-os` kernel crate (and all 31 workspace dependency crates)
for the `x86_64-unknown-none` target. Output:

```
target/x86_64-unknown-none/debug/claudio-os
```

This is a bare ELF binary -- **not bootable on its own**. It needs to be wrapped
with the bootloader.

For release builds (smaller, optimized, LTO enabled):

```bash
cargo build --release
```

Release profile settings from `Cargo.toml`:
```toml
[profile.release]
panic = "abort"
lto = true
codegen-units = 1
opt-level = "z"    # Optimize for size
strip = true
```

### Step 2: Create Bootable Disk Images

```bash
cargo run --manifest-path tools/image-builder/Cargo.toml -- \
    target/x86_64-unknown-none/debug/claudio-os
```

The image builder (`tools/image-builder/src/main.rs`) uses the `bootloader` crate
(v0.11) to produce two disk images:

| Image | Path | Boot method |
|-------|------|-------------|
| UEFI | `target/.../claudio-os-uefi.img` | UEFI firmware (OVMF) |
| BIOS | `target/.../claudio-os-bios.img` | Legacy BIOS (simpler) |

**Why a separate tool?** The image builder runs on the host (`x86_64-pc-windows-msvc`
or `x86_64-unknown-linux-gnu`), not the bare-metal target. It is excluded from the
workspace (`exclude = ["tools/image-builder"]`) to prevent it from inheriting the
`x86_64-unknown-none` build target.

### Quick Build Script

For repeated builds during development:

```bash
# Build kernel + create images + run QEMU (BIOS mode)
cargo build && \
cargo run --manifest-path tools/image-builder/Cargo.toml -- \
    target/x86_64-unknown-none/debug/claudio-os && \
qemu-system-x86_64 \
    -drive format=raw,file=target/x86_64-unknown-none/debug/claudio-os-bios.img \
    -serial stdio -m 512M
```

---

## Running in QEMU

### BIOS Boot (Simplest)

No OVMF firmware needed. Works out of the box:

```bash
qemu-system-x86_64 \
    -drive format=raw,file=target/x86_64-unknown-none/debug/claudio-os-bios.img \
    -serial stdio \
    -m 512M
```

### UEFI Boot

Requires OVMF firmware. Path varies by platform:

```bash
# Linux (Debian/Ubuntu)
qemu-system-x86_64 \
    -bios /usr/share/OVMF/OVMF_CODE.fd \
    -drive format=raw,file=target/x86_64-unknown-none/debug/claudio-os-uefi.img \
    -serial stdio \
    -m 512M

# Windows
qemu-system-x86_64 \
    -drive if=pflash,format=raw,readonly=on,file="C:\Program Files\qemu\share\edk2-x86_64-code.fd" \
    -drive format=raw,file=target\x86_64-claudio\debug\claudio-os-uefi.img \
    -serial stdio \
    -m 512M
```

### With Networking + TLS (Full Stack)

Add VirtIO-net device with SLIRP user-mode networking. **`-cpu Haswell` is required**
for AES-NI instructions used by TLS 1.3:

```bash
qemu-system-x86_64 \
    -bios /usr/share/OVMF/OVMF_CODE.fd \
    -drive format=raw,file=target/x86_64-unknown-none/debug/claudio-os-uefi.img \
    -device virtio-net-pci,netdev=net0 \
    -netdev user,id=net0 \
    -serial stdio \
    -m 1G \
    -smp 4 \
    -cpu Haswell
```

SLIRP networking provides:
- **DHCP**: Guest gets 10.0.2.x automatically
- **DNS**: Available at 10.0.2.3
- **NAT**: Outbound TCP/UDP works (HTTPS to api.anthropic.com)
- **No host configuration**: No bridges, no tap devices, no root required

### Useful QEMU Flags

| Flag | Purpose |
|------|---------|
| `-serial stdio` | Route serial port to terminal (see log output) |
| `-m 1G` | 1 GiB RAM (recommended for full stack) |
| `-smp 4` | 4 CPU cores |
| `-cpu Haswell` | Enable AES-NI for TLS 1.3 (required) |
| `-display gtk,grab-on-hover=on` | Graphical window with keyboard capture |
| `-no-reboot` | Stop on triple fault instead of rebooting |
| `-no-shutdown` | Keep VM alive after power off |
| `-s -S` | Start GDB server on port 1234, wait for debugger |
| `-d int` | Log all interrupts to stderr (very verbose) |
| `-enable-kvm` | Use KVM acceleration (Linux, much faster) |

---

## run.ps1 Explained

The `run.ps1` PowerShell script is the primary launcher on Windows. It handles
session persistence automatically:

```powershell
$credFile = "target\session.txt"

$fwCfgArgs = @()
if ((Test-Path $credFile) -and ((Get-Content $credFile -Raw).Trim().Length -gt 0)) {
    Write-Host "[run] Loaded saved session from $credFile"
    $fwCfgArgs = @("-fw_cfg", "name=opt/claudio/session,file=$credFile")
}

& "C:\Program Files\qemu\qemu-system-x86_64.exe" `
    -cpu Haswell `
    -drive "if=pflash,format=raw,readonly=on,file=C:\Program Files\qemu\share\edk2-x86_64-code.fd" `
    -drive "format=raw,file=target\x86_64-claudio\debug\claudio-os-uefi.img" `
    -device virtio-net-pci,netdev=net0 `
    -netdev user,id=net0 `
    -serial stdio `
    -display gtk,grab-on-hover=on `
    -m 1G `
    -no-reboot `
    @fwCfgArgs
```

**Key features:**
- Checks for saved session in `target/session.txt`
- Passes session via QEMU `fw_cfg` for the kernel to read at boot
- Uses GTK display with keyboard grab for PS/2 input
- 1 GiB RAM, Haswell CPU, no-reboot for debugging

---

## Session Persistence

ClaudioOS persists authentication sessions across reboots using QEMU's `fw_cfg`
device.

### How It Works

1. **Authentication**: On first boot, ClaudioOS authenticates via claude.ai OAuth
   (email code) or API key
2. **Save**: The kernel writes the `sessionKey` cookie to serial output with a
   special marker
3. **Capture**: The host-side script captures the session data and writes it to
   `target/session.txt`
4. **Restore**: On next boot, `run.ps1` passes `-fw_cfg name=opt/claudio/session,file=target/session.txt`
5. **Read**: The kernel reads the `fw_cfg` item at boot and restores the session
   without re-authentication

### Session File Format

`target/session.txt` contains the raw `sessionKey` cookie value. The session is
valid for approximately 28 days.

### Manual Session Management

```powershell
# Clear saved session (force re-authentication on next boot)
Remove-Item target\session.txt

# Check if a session exists
if (Test-Path target\session.txt) { Get-Content target\session.txt }
```

---

## Troubleshooting

### Windows: MSVC Linker Errors

The image builder is a host-side binary requiring the MSVC linker. If you see:

```
error: linker `link.exe` not found
```

**Fix**: Install "Desktop development with C++" in Visual Studio.

### TLS Crashes (Illegal Instruction)

If the kernel crashes during TLS handshake with an `#UD` (Invalid Opcode):

1. Ensure `-cpu Haswell` (or later) is in the QEMU command
2. Check TLS buffer alignment (must be 16-byte aligned for AES-NI)
3. Verify the custom target `x86_64-claudio.json` is being used

### Boot Hangs (No Serial Output)

1. Verify `-serial stdio` is in the QEMU command
2. Check the disk image path is correct and the file exists
3. Try BIOS boot instead of UEFI
4. Add `-no-reboot` to catch triple faults

### Double Fault After Enabling Interrupts

Common causes:
1. **Missing data segment in GDT**: SS must be loaded with a valid data segment
2. **APIC not disabled**: UEFI enables Local APIC, conflicts with PIC
3. **Stack overflow**: Bootloader's 128 KiB stack exhausted (see below)

---

## The Stack Overflow Fix

### Symptom

After all init phases complete and interrupts are enabled, the first timer or
keyboard interrupt causes a double fault.

### Root Cause

The bootloader provides a 128 KiB kernel stack. During boot, every `log::info!()`
call performs `format_args!()` which pushes large stack frames. After 6 phases of
init with dozens of log calls plus PCI enumeration, the stack is nearly full.

### Solution

1. Request 128 KiB from the bootloader (up from default ~16 KiB)
2. Allocate a fresh 4 MiB stack on the heap after heap init
3. Switch RSP to the new stack before enabling interrupts

```rust
const NEW_STACK_SIZE: usize = 4 * 1024 * 1024;
let layout = alloc::alloc::Layout::from_size_align(NEW_STACK_SIZE, 16).unwrap();
let new_stack_ptr = unsafe { alloc::alloc::alloc_zeroed(layout) };
let new_stack_top = (new_stack_ptr as u64 + NEW_STACK_SIZE as u64) & !0xF;

unsafe {
    core::arch::asm!(
        "mov rsp, {stack}",
        "call {entry}",
        stack = in(reg) new_stack_top,
        entry = in(reg) post_stack_switch as *const (),
        options(noreturn)
    );
}
```

---

## Environment Variables

Build-time environment variables read by `env!()` or `option_env!()`:

| Variable | Default | Description |
|----------|---------|-------------|
| `CLAUDIO_API_KEY` | (none) | Baked-in Anthropic API key for dev (skips OAuth) |
| `CLAUDIO_LOG_LEVEL` | `info` | Log level: `trace`, `debug`, `info`, `warn`, `error` |
| `CLAUDIO_QEMU` | `0` | Set to `1` for QEMU-friendly defaults |

```bash
# Build with a dev API key
CLAUDIO_API_KEY=sk-ant-api03-xxx cargo build

# Windows PowerShell
$env:CLAUDIO_API_KEY = "sk-ant-api03-xxx"
cargo build
```

---

## Project File Layout

```
baremetal-claude/
|-- CLAUDE.md                  Project design document
|-- README.md                  User-facing README
|-- Cargo.toml                 Workspace root (33 crates)
|-- x86_64-claudio.json        Custom target with SSE+AES-NI
|-- rust-toolchain.toml        Nightly Rust + components
|-- run.ps1                    Windows QEMU launcher with session persistence
|-- .cargo/config.toml         Default target, build-std
|
|-- kernel/                    Kernel binary (4,537 lines)
|-- crates/                    31 library crates
|   |-- terminal/              Split-pane framebuffer terminal
|   |-- net/                   VirtIO-net + smoltcp + TLS + HTTP
|   |-- api-client/            Anthropic Messages API + SSE
|   |-- auth/                  OAuth device flow + API key
|   |-- agent/                 Agent session lifecycle
|   |-- shell/                 AI-native shell (28 builtins)
|   |-- vfs/                   Virtual filesystem layer
|   |-- ext4/                  ext4 filesystem
|   |-- btrfs/                 btrfs filesystem
|   |-- ntfs/                  NTFS filesystem
|   |-- ahci/                  AHCI/SATA driver
|   |-- nvme/                  NVMe driver
|   |-- intel-nic/             Intel NIC driver
|   |-- xhci/                  xHCI USB 3.0 + HID
|   |-- acpi/                  ACPI table parser
|   |-- hda/                   Intel HD Audio
|   |-- smp/                   SMP multi-core
|   |-- gpu/                   NVIDIA GPU compute
|   |-- sshd/                  Post-quantum SSH daemon
|   |-- editor/                Nano-like text editor
|   |-- python-lite/           Python interpreter
|   |-- js-lite/               JavaScript evaluator
|   |-- rustc-lite/            Rust compiler (Cranelift)
|   |-- wraith-dom/            HTML parser
|   |-- wraith-render/         HTML -> text renderer
|   |-- wraith-transport/      HTTP/HTTPS client
|   |-- fs-persist/            FAT32 persistence (stubbed)
|   +-- cranelift-*-nostd/     6 forked Cranelift crates
|
|-- tools/
|   |-- image-builder/         UEFI/BIOS disk image builder
|   |-- auth-relay.py          API key management proxy
|   |-- build-server.py        Host-side Rust compilation
|   +-- tls-proxy.py           TLS termination (debug)
|
+-- docs/                      Documentation
    |-- ARCHITECTURE.md        Full system architecture
    |-- HARDWARE.md            Hardware driver docs
    |-- NETWORKING.md          Network stack docs
    |-- FILESYSTEMS.md         Filesystem docs
    |-- SHELL.md               Shell docs
    |-- AGENTS.md              Multi-agent system docs
    |-- BUILDING.md            This file
    |-- OPEN-SOURCE-CRATES.md  Published crates catalog
    +-- ROADMAP.md             Feature roadmap
```
