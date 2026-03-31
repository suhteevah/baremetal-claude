# CLAUDE.md — ClaudioOS Build Instructions

## What This Is

ClaudioOS is a bare-metal Rust operating system that boots via UEFI and provides a
purpose-built environment for running multiple AI coding agents (Anthropic Claude)
simultaneously. It has NO Linux kernel, NO POSIX layer, NO JavaScript runtime. It is
a single-address-space async Rust application that manages its own hardware.

**Owner**: Matt Gates (suhteevah) — Ridge Cell Repair LLC
**Target hardware**: x86_64 UEFI machines (dev on QEMU, prod on i9-11900K/RTX 3070 Ti,
Supermicro SYS-4028GR-TRT, HP Victus laptop, Arch Linux box)

## Architecture Overview

```
┌─────────────────────────────────────────────────────┐
│                  Agent Dashboard                     │
│  ┌──────────┐ ┌──────────┐ ┌──────────┐            │
│  │ Agent 1  │ │ Agent 2  │ │ Agent 3  │  ...       │
│  │ (pane)   │ │ (pane)   │ │ (pane)   │            │
│  └──────────┘ └──────────┘ └──────────┘            │
├─────────────────────────────────────────────────────┤
│              Agent Session Manager                   │
│         (async tasks, one per agent)                 │
├──────────────┬──────────────┬───────────────────────┤
│  API Client  │     Auth     │   Terminal Renderer   │
│  (Messages   │  (OAuth      │   (framebuffer +      │
│   API + SSE) │   device     │    ANSI + split       │
│              │   flow)      │    panes)             │
├──────────────┴──────────────┴───────────────────────┤
│              Async Executor (interrupt-driven)        │
├─────────────────────────────────────────────────────┤
│     Net (smoltcp + TLS)    │    FS (FAT32 persist)  │
├────────────────────────────┴────────────────────────┤
│   NIC Driver (virtio-net / e1000)  │  PS/2 Keyboard │
├────────────────────────────────────┴────────────────┤
│              x86_64 Kernel Core                      │
│   (paging, heap, GDT/IDT, interrupts, PCI)          │
├─────────────────────────────────────────────────────┤
│              UEFI Boot (bootloader crate)             │
└─────────────────────────────────────────────────────┘
```

## Crate Structure

- **`kernel/`** — Binary entry point. Boots, inits hardware, starts async executor,
  launches auth gate then agent dashboard. This is `#![no_std]` + `#![no_main]`.
- **`crates/api-client/`** — Anthropic Messages API client. Pure `no_std` + `alloc`.
  Handles streaming SSE, tool use protocol, conversation state. NO reqwest, NO hyper.
  Raw HTTP/1.1 over a TLS byte stream.
- **`crates/auth/`** — OAuth 2.0 Device Authorization Grant (RFC 8628). Token persist
  to FAT32, background refresh task, credential store shared across all agents.
- **`crates/terminal/`** — Framebuffer terminal renderer with split-pane support.
  Uses `os-terminal` or custom `vte` + `noto-sans-mono-bitmap`. Each pane is a
  viewport into the GOP framebuffer with independent scroll state.
- **`crates/net/`** — smoltcp integration, DHCP, DNS, TLS wrapper. Provides a
  high-level `TlsStream` type that the API client consumes. NIC driver abstraction.
- **`crates/agent/`** — Agent session lifecycle. Each session is an async task with
  its own conversation history, tool execution, and terminal pane. The dashboard
  manages creation/destruction/focus of sessions.
- **`crates/fs-persist/`** — FAT32 persistence layer. Config, tokens, agent state,
  conversation logs. Wraps `fatfs` with typed accessors.

## Build & Run

### Prerequisites
```bash
rustup target add x86_64-unknown-none
cargo install bootimage  # or use bootloader's disk image builder
# QEMU for testing:
sudo apt install qemu-system-x86 ovmf
```

### Development cycle
```bash
# Build the kernel image
cargo build --target x86_64-unknown-none

# Create bootable disk image (bootloader crate handles this)
# The exact command depends on bootloader v0.11 disk image builder

# Run in QEMU with UEFI
qemu-system-x86_64 \
    -bios /usr/share/OVMF/OVMF_CODE.fd \
    -drive format=raw,file=target/x86_64-unknown-none/debug/boot-bios-claudio-os.img \
    -device virtio-net-pci,netdev=net0 \
    -netdev user,id=net0,hostfwd=tcp::5555-:5555 \
    -serial stdio \
    -m 512M \
    -smp 4
```

### QEMU networking note
`-netdev user` provides SLIRP NAT. The guest gets DHCP (10.0.2.x), DNS (10.0.2.3),
and outbound internet including HTTPS to api.anthropic.com. No bridging needed for dev.

## Critical Design Decisions

1. **NO JavaScript runtime.** We call the Anthropic API directly from Rust via
   HTTP/1.1 POST + SSE streaming. This eliminates 50+ syscalls worth of compat work.

2. **Single address space.** No kernel/user boundary, no syscalls, no process
   isolation. Every agent session is an async task. We trust our own code.

3. **OAuth device flow at boot.** The auth module gates agent session startup.
   Token persists to FAT32. Background refresh runs as an executor task.

4. **Interrupt-driven async executor.** Hardware interrupts (NIC rx, keyboard,
   timer) wake futures. No polling. `hlt` when idle for power savings.

5. **Split-pane terminal natively.** No tmux dependency. The terminal crate manages
   a layout tree of viewports over the GOP framebuffer.

6. **FAT32 only for persistence.** No ext4, no journaling. Simple, works, the
   `fatfs` crate handles it. Config + tokens + logs.

## Development Phases

### Phase 1: Boot to terminal (PRIORITY — do this first)
- [ ] Kernel boots via bootloader crate on QEMU with UEFI
- [ ] GOP framebuffer initialized, can draw pixels
- [ ] Serial debug output working (0x3F8)
- [ ] GDT + IDT + interrupts configured
- [ ] Heap allocator working (linked_list_allocator)
- [ ] PS/2 keyboard input via IRQ1
- [ ] Basic terminal: type characters, see them on screen
- [ ] ANSI escape sequence support via vte

### Phase 2: Networking + TLS
- [ ] VirtIO-net driver initialized via PCI enumeration
- [ ] smoltcp interface with DHCP obtaining IP + DNS
- [ ] TCP connection to a known IP (test with httpbin.org)
- [ ] DNS resolution working (resolve api.anthropic.com)
- [ ] TLS handshake (embedded-tls initially)
- [ ] HTTPS GET to verify connectivity
- [ ] HTTPS POST with JSON body

### Phase 3: API client + Auth
- [ ] OAuth device flow: display code, poll for token
- [ ] Token persistence to FAT32 image
- [ ] Anthropic Messages API: send prompt, receive response
- [ ] SSE streaming: parse `event: content_block_delta` etc.
- [ ] Tool use protocol: parse tool_use blocks, return tool_result
- [ ] Conversation state management

### Phase 4: Multi-agent dashboard
- [ ] Split-pane layout tree (horizontal/vertical splits)
- [ ] Per-pane terminal instances with independent scroll
- [ ] Keyboard shortcuts: Ctrl+B prefix (tmux-style) for pane mgmt
- [ ] Agent session creation/destruction
- [ ] Focus switching between panes
- [ ] Status bar: agent states, token usage, network status

### Phase 5: Real hardware + hardening
- [ ] Boot on physical hardware (test on Arch box first)
- [ ] e1000/I219-V NIC driver for real Intel NICs
- [ ] USB keyboard via xHCI (CrabUSB) or rely on PS/2 emulation
- [ ] LUKS-like encryption for persist partition
- [ ] Graceful shutdown / token revocation
- [ ] USB boot image generation

## Key Crate Versions & Docs

| Crate | Version | Docs |
|-------|---------|------|
| bootloader | 0.11 | https://docs.rs/bootloader/0.11 |
| bootloader_api | 0.11 | https://docs.rs/bootloader_api/0.11 |
| x86_64 | 0.15 | https://docs.rs/x86_64/0.15 |
| smoltcp | 0.12 | https://docs.rs/smoltcp/0.12 |
| vte | 0.15 | https://docs.rs/vte/0.15 |
| pc-keyboard | 0.8 | https://docs.rs/pc-keyboard/0.8 |
| fatfs | 0.4 | https://docs.rs/fatfs/0.4 |
| embedded-tls | 0.17 | https://docs.rs/embedded-tls/0.17 |
| linked_list_allocator | 0.10 | https://docs.rs/linked_list_allocator/0.10 |
| spin | 0.9 | https://docs.rs/spin/0.9 |
| serde_json (no_std) | 1.x | features = ["alloc"], default-features = false |

## Reference Projects

- **blog_os**: https://os.phil-opp.com — THE tutorial. Follow for kernel basics.
- **MOROS**: https://moros.cc — Hobby Rust OS with smoltcp networking. Great driver ref.
- **Motor OS**: https://motor-os.org — Rust microkernel that serves its own website.
- **Redox OS drivers**: https://gitlab.redox-os.org/redox-os/drivers — NIC/NVMe/USB ref.
- **Hermit OS**: https://hermit-os.org — Unikernel with smoltcp, good kernel structure.
- **os-terminal**: https://lib.rs/crates/os-terminal — Turnkey bare-metal terminal.

## Conventions

- All crates are `#![no_std]` with `extern crate alloc` where heap is needed
- Use `log` crate macros everywhere, kernel provides serial + framebuffer log sinks
- Async where possible, `spin::Mutex` for shared state (no std Mutex available)
- Verbose logging: every network event, every API call, every auth state change
- Test in QEMU first, always. `cargo test` runs host-side unit tests for pure logic.
- Kernel panics print a red backtrace to framebuffer + serial before halting

## Environment Variables (build-time)

- `CLAUDIO_API_KEY` — Optional baked-in API key for development (skips OAuth)
- `CLAUDIO_LOG_LEVEL` — trace/debug/info/warn/error (default: info)
- `CLAUDIO_QEMU` — Set to 1 to use QEMU-friendly defaults (VirtIO, SLIRP)
