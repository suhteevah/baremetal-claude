//! PCI bus enumeration — discovers NIC and block devices.
//!
//! Scans PCI config space for known device classes:
//! - Network controller (class 0x02): VirtIO-net or Intel e1000
//! - Mass storage (class 0x01): VirtIO-blk or AHCI/NVMe
//!
//! After enumeration, discovered devices are stored in a static array so
//! other subsystems (e.g. the network stack) can look them up by
//! vendor/device ID.

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
    /// BAR0 value (I/O port base for legacy VirtIO, MMIO base for others).
    /// For I/O space BARs, bit 0 is set; the actual port base is `bar0 & !0x3`.
    pub bar0: u32,
    /// The PCI interrupt line (from config register 0x3C, bits 7:0).
    pub irq_line: u8,
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

pub fn enumerate() {
    log::info!("[pci] scanning bus 0...");

    let mut devices = PCI_DEVICES.lock();

    // Only scan bus 0 for now — QEMU places all devices there.
    for device in 0..32u8 {
        let vendor = read_config(0, device, 0, 0x00) as u16;
        if vendor == 0xFFFF {
            continue;
        }
        let device_id = (read_config(0, device, 0, 0x00) >> 16) as u16;
        let class_reg = read_config(0, device, 0, 0x08);
        let class = (class_reg >> 24) as u8;
        let subclass = (class_reg >> 16) as u8;
        let bar0 = read_config(0, device, 0, 0x10);
        let irq_line = read_config(0, device, 0, 0x3C) as u8;

        log::info!(
            "[pci] 00:{:02x}.0 vendor={:#06x} device={:#06x} class={:#04x}/{:#04x} bar0={:#010x} irq={}",
            device, vendor, device_id, class, subclass, bar0, irq_line
        );

        let pci_dev = PciDevice {
            bus: 0,
            device,
            function: 0,
            vendor_id: vendor,
            device_id,
            class,
            subclass,
            bar0,
            irq_line,
        };

        match (vendor, device_id) {
            (0x1AF4, 0x1000) => {
                log::info!("[pci]   -> VirtIO network device (I/O base: {:#x})", pci_dev.io_base());
                // Enable bus mastering so the device can DMA to/from host memory.
                enable_bus_master(0, device, 0);
            }
            (0x1AF4, 0x1001) => log::info!("[pci]   -> VirtIO block device"),
            (0x8086, 0x100E) => {
                log::info!("[pci]   -> Intel 82540EM (e1000)");
                enable_bus_master(0, device, 0);
            }
            (0x8086, 0x10D3) => {
                log::info!("[pci]   -> Intel 82574L");
                enable_bus_master(0, device, 0);
            }
            _ => {}
        }

        devices.push(pci_dev);
    }
    log::info!("[pci] scan complete, {} devices found", devices.count);
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
