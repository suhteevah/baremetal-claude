//! PCI bus enumeration — discovers NIC and block devices.
//!
//! Scans PCI config space for known device classes:
//! - Network controller (class 0x02): VirtIO-net or Intel e1000
//! - Mass storage (class 0x01): VirtIO-blk or AHCI/NVMe

use x86_64::instructions::port::Port;

const PCI_CONFIG_ADDR: u16 = 0xCF8;
const PCI_CONFIG_DATA: u16 = 0xCFC;

pub fn enumerate() {
    log::info!("[pci] scanning bus 0...");

    // Only scan bus 0 for now — QEMU places all devices there.
    // Scanning all 256 buses with interrupts enabled can overflow the
    // interrupt + kernel stack with all the port I/O and log formatting.
    for device in 0..32u8 {
        let vendor = read_config(0, device, 0, 0x00) as u16;
        if vendor == 0xFFFF {
            continue;
        }
        let device_id = (read_config(0, device, 0, 0x00) >> 16) as u16;
        let class = (read_config(0, device, 0, 0x08) >> 24) as u8;
        let subclass = (read_config(0, device, 0, 0x08) >> 16) as u8;

        log::info!(
            "[pci] 00:{:02x}.0 vendor={:#06x} device={:#06x} class={:#04x}/{:#04x}",
            device, vendor, device_id, class, subclass
        );

        match (vendor, device_id) {
            (0x1AF4, 0x1000) => log::info!("[pci]   -> VirtIO network device"),
            (0x1AF4, 0x1001) => log::info!("[pci]   -> VirtIO block device"),
            (0x8086, 0x100E) => log::info!("[pci]   -> Intel 82540EM (e1000)"),
            (0x8086, 0x10D3) => log::info!("[pci]   -> Intel 82574L"),
            _ => {}
        }
    }
    log::info!("[pci] scan complete");
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
