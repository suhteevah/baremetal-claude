# ClaudioOS Hardware Drivers

## Supported Hardware Overview

| Driver | Crate | Lines | Status | Hardware |
|--------|-------|-------|--------|----------|
| AHCI/SATA | `claudio-ahci` | 2,139 | Complete | Any AHCI controller (Intel PCH, AMD) |
| NVMe | `claudio-nvme` | 2,563 | Complete | NVMe 1.4+ SSDs (Samsung, WD, Intel) |
| Intel NIC | `claudio-intel-nic` | 1,986 | Complete | e1000, e1000e (I219-V), igc (I225-V) |
| VirtIO-net | `claudio-net` | 3,172 | Complete | QEMU virtio-net-pci (legacy 0.9.5) |
| xHCI USB | `claudio-xhci` | 4,204 | Complete | USB 3.0 host controllers + HID keyboard |
| HDA Audio | `claudio-hda` | 2,631 | Complete | Intel HD Audio (Realtek, etc.) |
| NVIDIA GPU | `claudio-gpu` | 3,392 | Complete | NVIDIA GPUs (Falcon, FIFO, tensor ops) |
| SMP | `claudio-smp` | 3,391 | Complete | Multi-core x86_64 (APIC, trampoline) |
| ACPI | `claudio-acpi` | 2,433 | Complete | RSDP/MADT/FADT/MCFG/HPET parsing |
| PS/2 Keyboard | kernel | -- | Complete | PS/2 via IRQ1 (8042 controller) |
| PIT Timer | kernel | -- | Complete | 8253/8254 at 18.2 Hz |
| Serial UART | kernel | -- | Complete | 16550 at 0x3F8, 115200 baud |

**Wiring status**: The drivers are implemented as standalone crates with clean APIs.
Wiring them into the kernel boot sequence is tracked in `docs/ROADMAP.md` under
"TODO -- Critical".

---

## AHCI/SATA (`crates/ahci/`)

AHCI (Advanced Host Controller Interface) provides a standard register-level
interface to SATA drives. ClaudioOS detects AHCI controllers via PCI class
0x01/subclass 0x06.

### Module Structure

| Module | Purpose |
|--------|---------|
| `hba.rs` | HBA (Host Bus Adapter) registers: global regs, port regs, volatile MMIO |
| `port.rs` | Per-port state machine: idle, BSY/DRQ wait, command slot management |
| `command.rs` | Command table construction: CFIS (H2D Register FIS), PRDT entries |
| `identify.rs` | ATA IDENTIFY DEVICE parsing: model, serial, capacity, features |
| `driver.rs` | High-level `AhciController` + `AhciDisk` with sector read/write |

### Register Layout

```
ABAR (BAR5 from PCI config)
  +0x00  GHC: Global Host Control (CAP, GHC, IS, PI, VS)
  +0x100 Port 0 registers (CLB, FB, IS, IE, CMD, TFD, SIG, SSTS, SCTL, SERR, CI)
  +0x180 Port 1 registers
  ...up to 32 ports
```

### Usage

```rust
use claudio_ahci::AhciController;

let abar: u64 = /* PCI BAR5 */;
let mut ctrl = AhciController::init(abar);
for disk in ctrl.disks() {
    let mut buf = [0u8; 512];
    disk.read_sectors(0, 1, &mut buf).unwrap();
}
```

---

## NVMe (`crates/nvme/`)

NVMe provides high-performance access to solid-state storage via PCIe memory-mapped
I/O. Queue pairs (submission + completion) with doorbell registers enable concurrent
sector I/O.

### Module Structure

| Module | Purpose |
|--------|---------|
| `registers.rs` | Controller registers: CAP, VS, CC, CSTS, AQA, ASQ, ACQ, doorbells |
| `queue.rs` | Submission/Completion queue pair: ring buffer, phase bit tracking |
| `admin.rs` | Admin commands: Identify Controller, Identify Namespace, Create I/O Queue |
| `io.rs` | I/O commands: Read, Write, Flush with scatter-gather PRP lists |
| `driver.rs` | `NvmeController` + `NvmeDisk` with sector-level API |

### Queue Architecture

```
Host Memory:
  Admin Submission Queue (ASQ) --doorbell--> Controller
  Admin Completion Queue (ACQ) <--interrupt-- Controller

  I/O Submission Queue 1 (IOSQ) --doorbell--> Controller
  I/O Completion Queue 1 (IOCQ) <--interrupt-- Controller

Each queue pair:
  - Submission: array of 64-byte command entries
  - Completion: array of 16-byte completion entries
  - Phase bit flips each wrap to distinguish new from old
  - Doorbell registers: BAR0 + 0x1000 + (queue_id * doorbell_stride)
```

### Usage

```rust
use claudio_nvme::NvmeController;

let mut ctrl = NvmeController::init(bar0_addr).unwrap();
let mut disk = ctrl.namespace(1).unwrap();
let mut buf = [0u8; 512];
disk.read_sectors(0, 1, &mut buf).unwrap();
```

---

## Intel NIC (`crates/intel-nic/`)

Supports the Intel e1000 family of Ethernet controllers for real hardware
(the VirtIO-net driver is used in QEMU).

### Supported Controllers

| PCI Device ID | Controller | Common Hardware |
|---------------|-----------|-----------------|
| 0x100E | e1000 (82540EM) | QEMU fallback, older servers |
| 0x15B8 | e1000e (I219-V) | Desktop Intel LAN (i9-11900K) |
| 0x15F3 | igc (I225-V) | 2.5GbE desktop LAN |

### Module Structure

| Module | Purpose |
|--------|---------|
| `regs.rs` | Register definitions: CTRL, STATUS, RCTL, TCTL, RDBAL/H, TDBAL/H |
| `rx.rs` | Receive descriptor ring: DMA buffers, head/tail management |
| `tx.rs` | Transmit descriptor ring: DMA buffers, RS/EOP flags |
| `phy.rs` | PHY configuration: MDIO register access, link speed/duplex |
| `driver.rs` | `IntelNic` with init, send_packet, recv_packet, link_status |

### DMA Ring Architecture

```
Host Memory:                        NIC Hardware:
  RX Descriptor Ring (256 entries)    RDH (head) -- NIC writes here
    [addr | length | status | ...]    RDT (tail) -- driver advances here
  RX Packet Buffers (2 KiB each)

  TX Descriptor Ring (256 entries)    TDH (head) -- NIC reads here
    [addr | length | cmd | status]    TDT (tail) -- driver writes here
  TX Packet Buffers (2 KiB each)
```

---

## xHCI USB 3.0 (`crates/xhci/`)

xHCI (eXtensible Host Controller Interface) provides USB 1.1/2.0/3.0 support
through a unified register interface. ClaudioOS uses it primarily for USB
keyboard input on real hardware (replacing PS/2).

### Module Structure

| Module | Purpose |
|--------|---------|
| `registers.rs` | Capability, Operational, Runtime, Doorbell register sets |
| `trb.rs` | Transfer Request Block types: Normal, Setup, Data, Status, Event, Link |
| `ring.rs` | TRB ring management: enqueue, dequeue, cycle bit, link TRBs |
| `context.rs` | Device/Endpoint context structures for slot assignment |
| `device.rs` | USB device enumeration: address, configure, interface/endpoint discovery |
| `hid.rs` | HID keyboard driver: report descriptor parsing, scancode translation |
| `driver.rs` | `XhciController` with init, poll, and keyboard event retrieval |

### TRB Ring Architecture

```
Command Ring (host -> controller):
  [TRB 0] [TRB 1] ... [Link TRB] -> wraps to start
  Doorbell write triggers controller to process

Event Ring (controller -> host):
  [Event TRB 0] [Event TRB 1] ...
  Interrupt or poll to check new events

Transfer Rings (per-endpoint):
  [Setup TRB] [Data TRB] [Status TRB]  -- control transfers
  [Normal TRB] [Normal TRB]            -- bulk/interrupt transfers
```

---

## HDA Audio (`crates/hda/`)

Intel High Definition Audio (HDA) provides audio playback through a codec
discovery + command/response protocol.

### Module Structure

| Module | Purpose |
|--------|---------|
| `registers.rs` | HDA controller registers: GCAP, GCTL, CORBBASE, RIRBBASE, stream regs |
| `corb.rs` | Command Outbound Ring Buffer: send verb commands to codecs |
| `rirb.rs` | Response Inbound Ring Buffer: receive codec responses |
| `codec.rs` | Codec discovery: widget tree walk, pin config, DAC/ADC routing |
| `stream.rs` | Stream descriptor setup: BDL (Buffer Descriptor List), format, DMA |
| `driver.rs` | `HdaController` with init, discover_codecs, play_pcm |

### CORB/RIRB Protocol

```
Host sends verb to codec:
  CORB[write_ptr] = (codec_id << 28) | (nid << 20) | verb
  Write CORBWP to advance

Codec responds:
  RIRB[read_ptr] = response (32 bits) + solicited flag
  Read RIRBWP to check for new responses

Stream playback:
  BDL: array of (buffer_addr, buffer_length) entries
  Stream registers: CTL, STS, LPIB, CBL, FMT
  DMA reads PCM samples from BDL buffers
```

---

## NVIDIA GPU (`crates/gpu/`)

Bare-metal NVIDIA GPU driver for compute workloads (not display). Based on
reverse-engineering from the nouveau project and envytools.

### Module Structure

| Module | Purpose |
|--------|---------|
| `pci_config.rs` | PCI vendor 0x10DE detection, BAR0/BAR1 mapping, GPU family detect |
| `mmio.rs` | MMIO register blocks: NV_PMC, PFIFO, PFB, PGRAPH, PTIMER |
| `memory.rs` | GPU VRAM management, GPU page tables, host-to-GPU DMA mapping |
| `falcon.rs` | Falcon microcontroller: firmware upload, PMU/SEC2/GSP-RM boot |
| `fifo.rs` | GPFIFO channels: push buffers, runlists, doorbell submission |
| `compute.rs` | Compute class setup: shader program load, grid/block dispatch |
| `tensor.rs` | Tensor operations: matmul, softmax, layernorm, GELU activation |
| `driver.rs` | `GpuDevice` high-level API: init, query capabilities, dispatch compute |

### Compute Dispatch Flow

```
1. Detect GPU via PCI (vendor 0x10DE)
2. Map BAR0 (MMIO registers) + BAR1 (VRAM aperture)
3. Boot Falcon microcontrollers (PMU, SEC2)
4. Create GPFIFO channel + push buffer
5. Load compute shader to GPU memory
6. Set up grid dimensions (blocks x threads)
7. Write method calls to push buffer
8. Ring doorbell to submit work
9. Poll for completion or wait for interrupt
```

---

## SMP Multi-Core (`crates/smp/`)

Symmetric Multi-Processing support enables running agent sessions across
multiple CPU cores.

### Module Structure

| Module | Purpose |
|--------|---------|
| `apic.rs` | Local APIC initialization, IPI (Inter-Processor Interrupt) sending |
| `trampoline.rs` | AP (Application Processor) boot: real-mode trampoline code at 0x8000 |
| `percpu.rs` | Per-CPU data structures: current task, local run queue, CPU ID |
| `scheduler.rs` | Work-stealing scheduler: per-core run queues, idle steal from neighbors |
| `driver.rs` | `SmpManager` with init, boot_aps, spawn_on_core |

### AP Boot Sequence

```
BSP (Boot Strap Processor):
  1. Parse ACPI MADT for AP APIC IDs
  2. Copy trampoline code to 0x8000 (below 1 MiB)
  3. Send INIT IPI to each AP
  4. Wait 10ms
  5. Send STARTUP IPI with vector 0x08 (-> 0x8000)
  6. AP wakes in real mode at 0x8000

AP (Application Processor):
  1. Execute trampoline: real mode -> protected mode -> long mode
  2. Load GDT, IDT, page tables (shared with BSP)
  3. Init local APIC
  4. Allocate per-CPU stack + data
  5. Enter scheduler idle loop
```

---

## ACPI (`crates/acpi/`)

ACPI table parsing provides hardware discovery and power management.

### Supported Tables

| Table | Purpose |
|-------|---------|
| RSDP | Root System Description Pointer -- entry point to ACPI tables |
| RSDT/XSDT | Root/Extended System Description Table -- table of table pointers |
| MADT | Multiple APIC Description Table -- APIC IDs, I/O APICs, interrupt overrides |
| FADT | Fixed ACPI Description Table -- PM timer, power management registers |
| MCFG | Memory-mapped Configuration Space -- PCIe ECAM base address |
| HPET | High Precision Event Timer -- nanosecond-resolution timer |

### Module Structure

| Module | Purpose |
|--------|---------|
| `rsdp.rs` | RSDP detection: scan EBDA + 0xE0000-0xFFFFF, validate checksum |
| `tables.rs` | Generic SDT header parsing, RSDT/XSDT traversal |
| `madt.rs` | MADT parsing: local APICs, I/O APICs, ISO, NMI entries |
| `fadt.rs` | FADT parsing: PM1a/PM1b control blocks, century register, boot flags |
| `mcfg.rs` | MCFG parsing: ECAM base, bus range for PCIe config space |
| `hpet.rs` | HPET parsing: base address, comparator count, min tick |
| `driver.rs` | `AcpiTables` with init, shutdown, reboot |

### Power Management

```rust
// Shutdown via ACPI PM1a control register:
// Write SLP_TYP | SLP_EN to PM1a_CNT
let pm1a_cnt = fadt.pm1a_control_block;
outw(pm1a_cnt, (slp_typ << 10) | (1 << 13));

// Reboot via keyboard controller (fallback):
outb(0x64, 0xFE);
```
