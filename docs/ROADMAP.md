---
name: ClaudioOS feature roadmap
description: Complete feature checklist for making ClaudioOS a real OS — track what's done and what's next
type: project
---

# ClaudioOS Feature Roadmap

## DONE
- [x] UEFI boot
- [x] GDT/IDT/PIC interrupts
- [x] Heap allocator (48 MiB)
- [x] PS/2 keyboard
- [x] Framebuffer (2560x1600, Terminus font, double-buffered, dirty regions)
- [x] VirtIO-net driver
- [x] smoltcp TCP/IP (DHCP, DNS)
- [x] TLS 1.3 (embedded-tls, AES-128-GCM)
- [x] HTTP/HTTPS client
- [x] claude.ai OAuth (email + code, source:"claude", anthropic-* headers)
- [x] claude.ai Max subscription chat API
- [x] Session persistence (fw_cfg, 28-day sessionKey)
- [x] Conversation reuse across reboots
- [x] Multi-agent dashboard (tmux-style split panes)
- [x] Agent tool loop (20 rounds max)
- [x] Python interpreter (python-lite)
- [x] JavaScript interpreter (js-lite)
- [x] Rust compiler (rustc-lite + Cranelift JIT)
- [x] Text editor (nano-like)
- [x] Wraith browser (DOM parser, transport, text renderer)
- [x] Cloudflare challenge solver
- [x] ext4 filesystem
- [x] btrfs filesystem
- [x] NTFS filesystem
- [x] AHCI/SATA driver
- [x] NVMe driver
- [x] Intel NIC driver (e1000/I219/I225)
- [x] xHCI USB 3.0 + HID keyboard
- [x] ACPI (RSDP/MADT/FADT/MCFG/HPET, shutdown, reboot)
- [x] HDA audio (PCM playback)
- [x] SMP multi-core (APIC, trampoline, work-stealing scheduler)
- [x] GPU compute (NVIDIA, Falcon, FIFO, tensor ops)
- [x] VFS layer (mount table, GPT/MBR, POSIX file API)
- [x] AI-native shell (28 builtins + natural language → Claude)
- [x] Post-quantum SSH daemon (ML-KEM-768 + X25519, ML-DSA-65)

## TODO — Critical (can't ship without)
- [ ] Wire VFS to real storage drivers (AHCI/NVMe + ext4/btrfs)
- [ ] Wire shell to dashboard (shell as a pane type)
- [ ] Wire SSH daemon to smoltcp TCP + terminal panes
- [ ] Wire xHCI USB keyboard to dashboard (replace PS/2 for real hardware)
- [ ] Wire Intel NIC to smoltcp (replace VirtIO-net for real hardware)
- [ ] Wire ACPI to boot sequence (hardware discovery, APIC setup)
- [ ] Wire SMP to agent sessions (one agent per core)
- [ ] Boot on real hardware (i9-11900K test first)
- [ ] RTC — real-time clock (CMOS or HPET) for wall clock time
- [ ] Fix keyboard input in QEMU graphical mode

## TODO — Important (makes it usable daily)
- [ ] Mouse support (USB HID mouse via xHCI)
- [ ] Pipes/IPC between agents
- [ ] Agent naming (Ctrl+B , to rename)
- [ ] init system (boot sequence configuration)
- [ ] User accounts + login (for SSH)
- [ ] Authorized keys management
- [ ] Config file persistence (FAT32 or ext4 on disk)
- [ ] Log file output (serial to file, or disk-backed)
- [ ] Session token auto-refresh before expiry
- [ ] Conversation management (list, select, delete old ones)

## TODO — Killer features (differentiators)
- [ ] Claude as shell (natural language → executed commands)
- [ ] GPU LLM inference (run local models on RTX 3070 Ti)
- [ ] Live code reload (Cranelift JIT hot-swap)
- [ ] Agent collaboration (agents talking to each other)
- [ ] Web browser in a pane (wraith text-mode rendering)
- [ ] File manager (visual directory browser)
- [ ] System monitor (CPU/memory/network dashboard)
- [ ] Boot from USB stick
- [ ] PXE network boot
- [ ] Wi-Fi driver (Intel AX201)

## TODO — Polish
- [ ] Better font rendering (anti-aliased if GPU available)
- [ ] Color themes
- [ ] Startup animation / splash screen
- [ ] Sound effects (boot chime via HDA)
- [ ] Screensaver (like TempleOS flight sim)

## Published Open-Source Crates (19)
1. ext4-rw
2. btrfs-nostd
3. ntfs-rw
4. js-lite
5. python-lite
6. rustc-lite
7. wraith-dom
8. wraith-render
9. wraith-transport
10. ahci-nostd
11. nvme-nostd
12. intel-nic-nostd
13. xhci-nostd
14. acpi-nostd
15. hda-nostd
16. smp-nostd
17. gpu-compute-nostd
18. sshd-pqc
19. editor-nostd
