//! PCI bus enumeration — discovers NIC and block devices.
//!
//! Scans PCI config space for known device classes:
//! - Network controller (class 0x02): VirtIO-net or Intel e1000
//! - Mass storage (class 0x01): VirtIO-blk or AHCI/NVMe
//!
//! After enumeration, discovered devices are stored in a static array so
//! other subsystems (e.g. the network stack) can look them up by
//! vendor/device ID.

extern crate alloc;

use spin::Mutex;
use x86_64::instructions::port::Port;

const PCI_CONFIG_ADDR: u16 = 0xCF8;
const PCI_CONFIG_DATA: u16 = 0xCFC;

/// Maximum number of PCI devices we track.
const MAX_DEVICES: usize = 32;

/// Information about a discovered PCI device.
#[derive(Debug, Clone, Copy)]
pub struct PciDevice {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub class: u8,
    pub subclass: u8,
    /// Programming interface byte from the PCI class register.
    pub prog_if: u8,
    /// BAR0 value (I/O port base for legacy VirtIO, MMIO base for others).
    /// For I/O space BARs, bit 0 is set; the actual port base is `bar0 & !0x3`.
    pub bar0: u32,
    /// All six BARs resolved to full 64-bit addresses (adjacent BARs are paired
    /// for 64-bit MMIO BARs). `bars[i].kind == Unused` for the high half of a
    /// 64-bit pair or an empty slot. See [`BarKind`] for the encoding.
    ///
    /// Realtek Wi-Fi exposes MMIO at BAR2 on PCIe (BAR0 is legacy I/O); Intel
    /// AHCI exposes MMIO at BAR5; virtio-legacy uses BAR0 I/O. Drivers should
    /// select the BAR index they need and check `kind` before dereferencing.
    pub bars: [Bar; 6],
    /// The PCI interrupt line (from config register 0x3C, bits 7:0).
    pub irq_line: u8,
}

/// A single PCI Base Address Register, resolved to kind + address.
#[derive(Debug, Clone, Copy)]
pub struct Bar {
    pub kind: BarKind,
    /// For MMIO: the 64-bit physical base address, with type/flag bits masked
    /// off. For I/O: the 16-bit port base zero-extended. 0 when unused.
    pub addr: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarKind {
    /// Slot unused (reported 0 / 0xFFFFFFFF, or the upper half of a 64-bit BAR).
    Unused,
    /// Memory-mapped I/O, 32-bit address space.
    Mmio32,
    /// Memory-mapped I/O, 64-bit address space (occupies this slot + next).
    Mmio64,
    /// Legacy I/O port space.
    Io,
}

impl Bar {
    pub const fn unused() -> Self {
        Self { kind: BarKind::Unused, addr: 0 }
    }
}

impl PciDevice {
    /// Return the I/O port base from BAR0 (strips the I/O space indicator bits).
    pub fn io_base(&self) -> u16 {
        (self.bar0 & !0x3) as u16
    }
}

/// Static storage for discovered PCI devices.
struct PciDevices {
    devices: [Option<PciDevice>; MAX_DEVICES],
    count: usize,
}

impl PciDevices {
    const fn new() -> Self {
        Self {
            devices: [None; MAX_DEVICES],
            count: 0,
        }
    }

    fn push(&mut self, dev: PciDevice) {
        if self.count < MAX_DEVICES {
            self.devices[self.count] = Some(dev);
            self.count += 1;
        }
    }

    fn find(&self, vendor_id: u16, device_id: u16) -> Option<PciDevice> {
        for i in 0..self.count {
            if let Some(dev) = &self.devices[i] {
                if dev.vendor_id == vendor_id && dev.device_id == device_id {
                    return Some(*dev);
                }
            }
        }
        None
    }
}

static PCI_DEVICES: Mutex<PciDevices> = Mutex::new(PciDevices::new());

/// Find a VirtIO network device (vendor 0x1AF4, device 0x1000).
///
/// Returns `None` if no such device was found during enumeration.
pub fn find_nic() -> Option<PciDevice> {
    PCI_DEVICES.lock().find(0x1AF4, 0x1000)
}

/// Find any PCI device by vendor and device ID.
pub fn find_device(vendor_id: u16, device_id: u16) -> Option<PciDevice> {
    PCI_DEVICES.lock().find(vendor_id, device_id)
}

/// Find a PCI device by class, subclass, and programming interface.
pub fn find_by_class(class: u8, subclass: u8, prog_if: u8) -> Option<PciDevice> {
    let devices = PCI_DEVICES.lock();
    for i in 0..devices.count {
        if let Some(dev) = &devices.devices[i] {
            if dev.class == class && dev.subclass == subclass && dev.prog_if == prog_if {
                return Some(*dev);
            }
        }
    }
    None
}

/// Find a PCI device using a custom predicate.
///
/// The predicate receives each `PciDevice` and can return `Some(T)` for a
/// match or `None` to skip.  Returns the first match.
pub fn find_by_predicate<T, F>(predicate: F) -> Option<T>
where
    F: Fn(PciDevice) -> Option<T>,
{
    let devices = PCI_DEVICES.lock();
    for i in 0..devices.count {
        if let Some(dev) = &devices.devices[i] {
            if let Some(result) = predicate(*dev) {
                return Some(result);
            }
        }
    }
    None
}

pub fn enumerate() {
    log::info!("[pci] scanning all buses (0..=255)...");

    let mut devices = PCI_DEVICES.lock();

    // Brute-force scan every bus/slot/function. Intel CNVi Wi-Fi and many
    // NVMe/xHCI devices live on bus > 0 on real hardware (behind root ports).
    // Bus 0 only was a QEMU-only assumption that lost us every device behind
    // a PCIe root port on the Victus. Full scan costs a few thousand port
    // reads at boot and returns once 0xFFFF.
    for bus in 0u8..=255 {
        for device in 0u8..32 {
            // Probe function 0 first; if it's populated, check whether it's a
            // multi-function device (header_type bit 7 set) and iterate fns.
            let vendor0 = read_config(bus, device, 0, 0x00) as u16;
            if vendor0 == 0xFFFF {
                continue;
            }
            let header_type = (read_config(bus, device, 0, 0x0C) >> 16) as u8;
            let max_fn = if header_type & 0x80 != 0 { 8u8 } else { 1u8 };

            for func in 0u8..max_fn {
                let vendor = read_config(bus, device, func, 0x00) as u16;
                if vendor == 0xFFFF {
                    continue;
                }
                let device_id = (read_config(bus, device, func, 0x00) >> 16) as u16;
                let class_reg = read_config(bus, device, func, 0x08);
                let class = (class_reg >> 24) as u8;
                let subclass = (class_reg >> 16) as u8;
                let prog_if = (class_reg >> 8) as u8;
                let bar0 = read_config(bus, device, func, 0x10);
                let irq_line = read_config(bus, device, func, 0x3C) as u8;
                let bars = read_all_bars(bus, device, func);

                log::info!(
                    "[pci] {:02x}:{:02x}.{} vendor={:#06x} device={:#06x} class={:#04x}/{:#04x} bar0={:#010x} irq={}",
                    bus, device, func, vendor, device_id, class, subclass, bar0, irq_line
                );

                let pci_dev = PciDevice {
                    bus,
                    device,
                    function: func,
                    vendor_id: vendor,
                    device_id,
                    class,
                    subclass,
                    prog_if,
                    bar0,
                    bars,
                    irq_line,
                };

                match (vendor, device_id) {
                    (0x1AF4, 0x1000) => {
                        log::info!("[pci]   -> VirtIO network device (I/O base: {:#x})", pci_dev.io_base());
                        enable_bus_master(bus, device, func);
                    }
                    (0x1AF4, 0x1001) => log::info!("[pci]   -> VirtIO block device"),
                    (0x8086, did) => {
                        if let Some(variant) = claudio_intel_nic::NicVariant::from_pci_ids(0x8086, did) {
                            log::info!("[pci]   -> Intel NIC: {}", variant.name());
                            enable_bus_master(bus, device, func);
                        } else if did == 0x10D3 {
                            log::info!("[pci]   -> Intel 82574L (unsupported by driver)");
                            enable_bus_master(bus, device, func);
                        } else if class == 0x02 {
                            // Unknown Intel network controller — almost certainly
                            // a Wi-Fi adapter (AX200/AX201/AX210/AX211). Logged
                            // so the PCI-dump terminal line surfaces the device
                            // ID for driver probing.
                            log::info!(
                                "[pci]   -> Intel network class device 0x{:04x} (possible Wi-Fi)",
                                did
                            );
                        }
                    }
                    _ => {}
                }

                devices.push(pci_dev);
            }
        }
    }
    log::info!("[pci] scan complete, {} devices found", devices.count);
}

/// Return a snapshot of all enumerated PCI devices.
///
/// Used by the fallback terminal to render a human-readable dump so Matt can
/// photograph the Victus screen and read off Intel device IDs (no serial on
/// real HW). Caller must not hold `PCI_DEVICES` concurrently.
pub fn snapshot() -> alloc::vec::Vec<PciDevice> {
    let devs = PCI_DEVICES.lock();
    let mut out = alloc::vec::Vec::with_capacity(devs.count);
    for i in 0..devs.count {
        if let Some(dev) = devs.devices[i] {
            out.push(dev);
        }
    }
    out
}

/// Enable PCI bus mastering for a specific device (public interface).
///
/// Called by subsystem init code (e.g. USB) that discovers devices by class
/// rather than the fixed match table in `enumerate()`.
pub fn enable_bus_master_for(bus: u8, device: u8, func: u8) {
    enable_bus_master(bus, device, func);
}

/// Set both the Memory Space Enable (bit 1) and Bus Master Enable (bit 2)
/// bits in the PCI Command register. Needed before touching a device's MMIO
/// BAR — UEFI usually pre-enables both, but don't assume.
pub fn enable_mem_and_bus_master(bus: u8, device: u8, func: u8) {
    let cmd = read_config(bus, device, func, 0x04);
    let want = cmd | (1 << 1) | (1 << 2);
    if want != cmd {
        write_config(bus, device, func, 0x04, want);
        log::debug!(
            "[pci] enabled mem+bus-master for {:02x}:{:02x}.{} (was {:#06x} -> {:#06x})",
            bus, device, func, cmd & 0xFFFF, want & 0xFFFF
        );
    }
}

/// Read a PCI config register (public interface).
///
/// Used by subsystems that need to read additional BARs or capabilities
/// beyond what `PciDevice` stores.
pub fn read_config_pub(bus: u8, device: u8, func: u8, offset: u8) -> u32 {
    read_config(bus, device, func, offset)
}

/// Enable PCI bus mastering for a device.
///
/// Sets bit 2 (Bus Master) in the PCI Command register (offset 0x04).
/// This is required for any device that performs DMA (VirtIO, e1000, etc.).
fn enable_bus_master(bus: u8, device: u8, func: u8) {
    let cmd = read_config(bus, device, func, 0x04);
    // Bit 2 = Bus Master Enable
    if cmd & (1 << 2) == 0 {
        write_config(bus, device, func, 0x04, cmd | (1 << 2));
        log::debug!(
            "[pci] enabled bus mastering for {:02x}:{:02x}.{}",
            bus,
            device,
            func
        );
    }
}

/// Read all six BARs (config offsets 0x10..0x28) and decode kind + 64-bit
/// address. Handles 64-bit BAR pairing: if BAR N is a 64-bit MMIO BAR, its
/// upper half is in BAR N+1 and that slot is marked `Unused`.
fn read_all_bars(bus: u8, device: u8, func: u8) -> [Bar; 6] {
    let mut out = [Bar::unused(); 6];
    let mut i = 0usize;
    while i < 6 {
        let raw = read_config(bus, device, func, 0x10 + (i as u8) * 4);
        if raw == 0 || raw == 0xFFFF_FFFF {
            i += 1;
            continue;
        }
        if raw & 0x1 != 0 {
            // I/O BAR — always 32-bit, bits 2+ are the port base.
            out[i] = Bar { kind: BarKind::Io, addr: (raw & !0x3) as u64 };
            i += 1;
        } else {
            let ty = (raw >> 1) & 0x3; // 00 = 32-bit, 10 = 64-bit
            let base32 = (raw & 0xFFFF_FFF0) as u64;
            if ty == 0x2 && i + 1 < 6 {
                let hi = read_config(bus, device, func, 0x10 + ((i as u8) + 1) * 4) as u64;
                out[i] = Bar { kind: BarKind::Mmio64, addr: base32 | (hi << 32) };
                // upper half slot stays Unused
                i += 2;
            } else {
                out[i] = Bar { kind: BarKind::Mmio32, addr: base32 };
                i += 1;
            }
        }
    }
    out
}

fn read_config(bus: u8, device: u8, func: u8, offset: u8) -> u32 {
    let address: u32 = 0x8000_0000
        | ((bus as u32) << 16)
        | ((device as u32) << 11)
        | ((func as u32) << 8)
        | ((offset as u32) & 0xFC);

    unsafe {
        Port::new(PCI_CONFIG_ADDR).write(address);
        Port::new(PCI_CONFIG_DATA).read()
    }
}

fn write_config(bus: u8, device: u8, func: u8, offset: u8, value: u32) {
    let address: u32 = 0x8000_0000
        | ((bus as u32) << 16)
        | ((device as u32) << 11)
        | ((func as u32) << 8)
        | ((offset as u32) & 0xFC);

    unsafe {
        Port::new(PCI_CONFIG_ADDR).write(address);
        Port::<u32>::new(PCI_CONFIG_DATA).write(value);
    }
}
