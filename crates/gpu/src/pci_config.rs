//! PCI configuration space access for NVIDIA GPUs.
//!
//! NVIDIA GPUs are PCI devices with vendor ID 0x10DE. We enumerate the PCI bus,
//! find the GPU, read its BARs, enable bus mastering and memory space, and
//! determine the GPU family from the device ID.
//!
//! Reference: envytools — https://envytools.readthedocs.io/en/latest/hw/bus/pci.html

use alloc::string::String;
use alloc::format;
use core::fmt;

/// NVIDIA PCI vendor ID.
pub const NVIDIA_VENDOR_ID: u16 = 0x10DE;

/// PCI configuration space register offsets.
pub const PCI_VENDOR_ID: u8 = 0x00;
pub const PCI_DEVICE_ID: u8 = 0x02;
pub const PCI_COMMAND: u8 = 0x04;
pub const PCI_STATUS: u8 = 0x06;
pub const PCI_REVISION: u8 = 0x08;
pub const PCI_CLASS_CODE: u8 = 0x09;
pub const PCI_BAR0: u8 = 0x10;
pub const PCI_BAR1: u8 = 0x14;
pub const PCI_BAR2: u8 = 0x18;
pub const PCI_BAR3: u8 = 0x1C;
pub const PCI_BAR4: u8 = 0x20;
pub const PCI_BAR5: u8 = 0x24;

/// PCI command register bits.
pub const PCI_CMD_IO_SPACE: u16 = 1 << 0;
pub const PCI_CMD_MEMORY_SPACE: u16 = 1 << 1;
pub const PCI_CMD_BUS_MASTER: u16 = 1 << 2;

/// GPU architecture families, from Kepler (2012) to Ada Lovelace (2022).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuFamily {
    /// GK1xx — Kepler (2012)
    Kepler,
    /// GM1xx/GM2xx — Maxwell (2014)
    Maxwell,
    /// GP1xx — Pascal (2016)
    Pascal,
    /// GV1xx — Volta (2017)
    Volta,
    /// TU1xx — Turing (2018)
    Turing,
    /// GA1xx — Ampere (2020). RTX 3070 Ti = GA104.
    Ampere,
    /// AD1xx — Ada Lovelace (2022)
    Ada,
    /// Unrecognized GPU family
    Unknown,
}

impl fmt::Display for GpuFamily {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GpuFamily::Kepler => write!(f, "Kepler"),
            GpuFamily::Maxwell => write!(f, "Maxwell"),
            GpuFamily::Pascal => write!(f, "Pascal"),
            GpuFamily::Volta => write!(f, "Volta"),
            GpuFamily::Turing => write!(f, "Turing"),
            GpuFamily::Ampere => write!(f, "Ampere"),
            GpuFamily::Ada => write!(f, "Ada Lovelace"),
            GpuFamily::Unknown => write!(f, "Unknown"),
        }
    }
}

/// Detected NVIDIA GPU PCI device with mapped BAR addresses.
#[derive(Debug)]
pub struct GpuPciDevice {
    /// PCI bus number
    pub bus: u8,
    /// PCI device number
    pub device: u8,
    /// PCI function number
    pub function: u8,
    /// PCI device ID (determines GPU model)
    pub device_id: u16,
    /// PCI revision
    pub revision: u8,
    /// GPU architecture family
    pub family: GpuFamily,
    /// BAR0: MMIO registers (typically 16-32 MiB)
    pub bar0_base: u64,
    pub bar0_size: u64,
    /// BAR1: VRAM aperture for CPU access (typically 256 MiB - 16 GiB)
    pub bar1_base: u64,
    pub bar1_size: u64,
    /// BAR2/3: I/O port or additional MMIO (may not be present)
    pub bar2_base: u64,
    pub bar2_size: u64,
    /// Human-readable GPU name
    pub name: String,
}

/// Map a PCI device ID to its GPU family.
///
/// Device IDs sourced from envytools and nouveau:
/// https://envytools.readthedocs.io/en/latest/hw/pciid.html
pub fn device_id_to_family(device_id: u16) -> GpuFamily {
    match device_id >> 4 {
        // Kepler: GK104, GK106, GK107, GK110, GK208, etc.
        // Device IDs: 0x11xx, 0x0Fxx range
        0x0E0..=0x0EF => GpuFamily::Kepler,  // GK104
        0x0F0..=0x0FF => GpuFamily::Kepler,  // GK107/GK208
        0x100..=0x10F => GpuFamily::Kepler,  // GK110
        0x118..=0x11F => GpuFamily::Kepler,  // GK106

        // Maxwell: GM107, GM108, GM200, GM204, GM206
        // Device IDs: 0x13xx, 0x17xx range
        0x130..=0x13F => GpuFamily::Maxwell, // GM107/GM108
        0x140..=0x14F => GpuFamily::Maxwell, // GM204/GM206
        0x170..=0x17F => GpuFamily::Maxwell, // GM200

        // Pascal: GP100, GP102, GP104, GP106, GP107, GP108
        // Device IDs: 0x15xx, 0x1Bxx, 0x1Cxx, 0x1Dxx range
        0x150..=0x15F => GpuFamily::Pascal,  // GP100
        0x1B0..=0x1BF => GpuFamily::Pascal,  // GP102
        0x1C0..=0x1CF => GpuFamily::Pascal,  // GP104/GP106/GP107
        0x1D0..=0x1DF => GpuFamily::Pascal,  // GP108

        // Volta: GV100
        // Device IDs: 0x1D8x range
        0x1D8..=0x1DF => GpuFamily::Volta,   // GV100

        // Turing: TU102, TU104, TU106, TU116, TU117
        // Device IDs: 0x1E0x, 0x1F0x, 0x2180x range
        0x1E0..=0x1EF => GpuFamily::Turing,  // TU102/TU104
        0x1F0..=0x1FF => GpuFamily::Turing,  // TU106/TU116/TU117
        0x218..=0x21F => GpuFamily::Turing,  // TU116 mobile

        // Ampere: GA102, GA103, GA104, GA106, GA107
        // Device IDs: 0x2200-0x27FF range
        // RTX 3070 Ti = GA104 = device ID 0x2482, 0x2484
        0x220..=0x22F => GpuFamily::Ampere,  // GA102 (RTX 3090/3080)
        0x230..=0x23F => GpuFamily::Ampere,  // GA103
        0x240..=0x24F => GpuFamily::Ampere,  // GA104 (RTX 3070 Ti, RTX 3070)
        0x250..=0x25F => GpuFamily::Ampere,  // GA106 (RTX 3060)
        0x260..=0x27F => GpuFamily::Ampere,  // GA107 (RTX 3050)

        // Ada Lovelace: AD102, AD103, AD104, AD106, AD107
        // Device IDs: 0x2600-0x2800 range
        0x260..=0x26F => GpuFamily::Ada,     // AD102 (RTX 4090)
        0x270..=0x27F => GpuFamily::Ada,     // AD103/AD104 (RTX 4080/4070 Ti)
        0x280..=0x28F => GpuFamily::Ada,     // AD106/AD107 (RTX 4060)

        _ => GpuFamily::Unknown,
    }
}

/// Map known device IDs to human-readable GPU names.
pub fn device_id_to_name(device_id: u16) -> String {
    match device_id {
        // Ampere — GA104
        0x2482 => String::from("NVIDIA GeForce RTX 3070 Ti"),
        0x2484 => String::from("NVIDIA GeForce RTX 3070 Ti"),
        0x2486 => String::from("NVIDIA GeForce RTX 3070 Ti Mobile"),
        0x2488 => String::from("NVIDIA GeForce RTX 3070 Ti Laptop GPU"),
        // Ampere — GA102
        0x2204 => String::from("NVIDIA GeForce RTX 3090"),
        0x2206 => String::from("NVIDIA GeForce RTX 3080"),
        0x2208 => String::from("NVIDIA GeForce RTX 3080 Ti"),
        // Ampere — GA106
        0x2503 => String::from("NVIDIA GeForce RTX 3060"),
        0x2504 => String::from("NVIDIA GeForce RTX 3060 Ti"),
        // Turing
        0x1E04 => String::from("NVIDIA GeForce RTX 2080 Ti"),
        0x1E07 => String::from("NVIDIA GeForce RTX 2080"),
        0x1F02 => String::from("NVIDIA GeForce RTX 2070"),
        0x1F08 => String::from("NVIDIA GeForce RTX 2060"),
        // Ada Lovelace
        0x2684 => String::from("NVIDIA GeForce RTX 4090"),
        0x2704 => String::from("NVIDIA GeForce RTX 4080"),
        0x2782 => String::from("NVIDIA GeForce RTX 4070 Ti"),
        _ => format!("NVIDIA GPU (device_id=0x{:04X})", device_id),
    }
}

// ---------------------------------------------------------------------------
// PCI config space access via I/O ports 0xCF8/0xCFC
// ---------------------------------------------------------------------------

/// Read a 32-bit value from PCI configuration space.
///
/// Uses the legacy x86 PCI configuration mechanism 1 (I/O ports 0xCF8/0xCFC).
pub fn pci_config_read32(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    let address: u32 = 0x8000_0000
        | ((bus as u32) << 16)
        | ((device as u32) << 11)
        | ((function as u32) << 8)
        | ((offset as u32) & 0xFC);

    unsafe {
        // Write address to CONFIG_ADDRESS (0xCF8)
        core::arch::asm!(
            "out dx, eax",
            in("dx") 0xCF8u16,
            in("eax") address,
            options(nostack, preserves_flags)
        );
        // Read data from CONFIG_DATA (0xCFC)
        let value: u32;
        core::arch::asm!(
            "in eax, dx",
            in("dx") 0xCFCu16,
            out("eax") value,
            options(nostack, preserves_flags)
        );
        value
    }
}

/// Write a 32-bit value to PCI configuration space.
pub fn pci_config_write32(bus: u8, device: u8, function: u8, offset: u8, value: u32) {
    let address: u32 = 0x8000_0000
        | ((bus as u32) << 16)
        | ((device as u32) << 11)
        | ((function as u32) << 8)
        | ((offset as u32) & 0xFC);

    unsafe {
        core::arch::asm!(
            "out dx, eax",
            in("dx") 0xCF8u16,
            in("eax") address,
            options(nostack, preserves_flags)
        );
        core::arch::asm!(
            "out dx, eax",
            in("dx") 0xCFCu16,
            in("eax") value,
            options(nostack, preserves_flags)
        );
    }
}

/// Read a 16-bit value from PCI configuration space.
pub fn pci_config_read16(bus: u8, device: u8, function: u8, offset: u8) -> u16 {
    let val32 = pci_config_read32(bus, device, function, offset & 0xFC);
    ((val32 >> ((offset & 2) * 8)) & 0xFFFF) as u16
}

/// Write a 16-bit value to PCI configuration space.
pub fn pci_config_write16(bus: u8, device: u8, function: u8, offset: u8, value: u16) {
    let val32 = pci_config_read32(bus, device, function, offset & 0xFC);
    let shift = (offset & 2) * 8;
    let mask = !(0xFFFF << shift);
    let new_val = (val32 & mask) | ((value as u32) << shift);
    pci_config_write32(bus, device, function, offset & 0xFC, new_val);
}

/// Decode a 64-bit BAR from two consecutive 32-bit BAR registers.
fn decode_bar64(bar_lo: u32, bar_hi: u32) -> (u64, bool) {
    let is_64bit = (bar_lo & 0x06) == 0x04;
    let is_mmio = (bar_lo & 0x01) == 0;
    let base = if is_64bit {
        ((bar_hi as u64) << 32) | ((bar_lo as u64) & !0xF)
    } else {
        (bar_lo as u64) & !0xF
    };
    (base, is_mmio)
}

/// Determine BAR size by writing all 1s and reading back.
fn probe_bar_size(bus: u8, device: u8, function: u8, bar_offset: u8) -> u64 {
    let original = pci_config_read32(bus, device, function, bar_offset);
    pci_config_write32(bus, device, function, bar_offset, 0xFFFF_FFFF);
    let size_mask = pci_config_read32(bus, device, function, bar_offset);
    pci_config_write32(bus, device, function, bar_offset, original);

    if size_mask == 0 || size_mask == 0xFFFF_FFFF {
        return 0;
    }

    // Mask out type bits for MMIO BARs
    let mask = size_mask & !0xF;
    if mask == 0 {
        return 0;
    }
    // Size = ~mask + 1
    ((!mask).wrapping_add(1)) as u64
}

/// Scan the PCI bus for an NVIDIA GPU.
///
/// Enumerates all PCI bus/device/function combinations looking for vendor 0x10DE.
/// Returns the first NVIDIA GPU found with its BARs decoded and bus mastering enabled.
pub fn scan_for_nvidia_gpu() -> Option<GpuPciDevice> {
    log::info!("gpu: scanning PCI bus for NVIDIA GPU (vendor 0x{:04X})...", NVIDIA_VENDOR_ID);

    for bus in 0..=255u8 {
        for device in 0..32u8 {
            let vendor_device = pci_config_read32(bus, device, 0, PCI_VENDOR_ID);
            let vendor_id = (vendor_device & 0xFFFF) as u16;
            let dev_id = ((vendor_device >> 16) & 0xFFFF) as u16;

            if vendor_id != NVIDIA_VENDOR_ID {
                continue;
            }

            // Check class code: 0x03 = display controller, 0x00 = VGA compatible
            let class_rev = pci_config_read32(bus, device, 0, PCI_CLASS_CODE);
            let class_code = ((class_rev >> 24) & 0xFF) as u8;
            let subclass = ((class_rev >> 16) & 0xFF) as u8;
            let revision = (class_rev & 0xFF) as u8;

            log::info!(
                "gpu: found NVIDIA device at PCI {:02X}:{:02X}.0 — device_id=0x{:04X} class={:02X}:{:02X} rev={}",
                bus, device, dev_id, class_code, subclass, revision
            );

            // We want display controllers (0x03) or processing accelerators (0x12)
            if class_code != 0x03 && class_code != 0x12 {
                log::warn!(
                    "gpu: NVIDIA device at {:02X}:{:02X}.0 has class {:02X}, not a GPU — skipping",
                    bus, device, class_code
                );
                continue;
            }

            let family = device_id_to_family(dev_id);
            let name = device_id_to_name(dev_id);
            log::info!("gpu: identified {} (family: {})", name, family);

            // --- Read BARs ---

            let bar0_raw = pci_config_read32(bus, device, 0, PCI_BAR0);
            let bar1_raw = pci_config_read32(bus, device, 0, PCI_BAR1);
            let bar2_raw = pci_config_read32(bus, device, 0, PCI_BAR2);
            let bar3_raw = pci_config_read32(bus, device, 0, PCI_BAR3);

            // BAR0 is typically 64-bit MMIO (registers)
            let (bar0_base, _) = decode_bar64(bar0_raw, bar1_raw);
            let bar0_size = probe_bar_size(bus, device, 0, PCI_BAR0);

            // BAR1 (or BAR2/3 if BAR0 is 64-bit) is VRAM aperture
            // If BAR0 is 64-bit, BAR1 is consumed, so VRAM starts at BAR2
            let is_bar0_64bit = (bar0_raw & 0x06) == 0x04;
            let (bar1_base, bar1_size, bar2_base, bar2_size);
            if is_bar0_64bit {
                let bar2_hi = pci_config_read32(bus, device, 0, PCI_BAR3);
                let (b1, _) = decode_bar64(bar2_raw, bar2_hi);
                bar1_base = b1;
                bar1_size = probe_bar_size(bus, device, 0, PCI_BAR2);
                let bar4_raw = pci_config_read32(bus, device, 0, PCI_BAR4);
                let bar5_raw = pci_config_read32(bus, device, 0, PCI_BAR5);
                let (b2, _) = decode_bar64(bar4_raw, bar5_raw);
                bar2_base = b2;
                bar2_size = probe_bar_size(bus, device, 0, PCI_BAR4);
            } else {
                let (b1, _) = decode_bar64(bar2_raw, bar3_raw);
                bar1_base = b1;
                bar1_size = probe_bar_size(bus, device, 0, PCI_BAR2);
                bar2_base = 0;
                bar2_size = 0;
            }

            log::info!(
                "gpu: BAR0 (MMIO regs) = 0x{:016X}, size = {} MiB",
                bar0_base, bar0_size / (1024 * 1024)
            );
            log::info!(
                "gpu: BAR1 (VRAM aperture) = 0x{:016X}, size = {} MiB",
                bar1_base, bar1_size / (1024 * 1024)
            );
            if bar2_size > 0 {
                log::info!(
                    "gpu: BAR2 (I/O or MMIO) = 0x{:016X}, size = {} KiB",
                    bar2_base, bar2_size / 1024
                );
            }

            // --- Enable bus mastering + memory space ---

            let cmd = pci_config_read16(bus, device, 0, PCI_COMMAND);
            let new_cmd = cmd | PCI_CMD_MEMORY_SPACE | PCI_CMD_BUS_MASTER;
            if cmd != new_cmd {
                log::info!(
                    "gpu: enabling bus mastering + memory space (cmd: 0x{:04X} -> 0x{:04X})",
                    cmd, new_cmd
                );
                pci_config_write16(bus, device, 0, PCI_COMMAND, new_cmd);
            } else {
                log::info!("gpu: bus mastering + memory space already enabled");
            }

            return Some(GpuPciDevice {
                bus,
                device,
                function: 0,
                device_id: dev_id,
                revision,
                family,
                bar0_base,
                bar0_size,
                bar1_base,
                bar1_size,
                bar2_base,
                bar2_size,
                name,
            });
        }
    }

    log::warn!("gpu: no NVIDIA GPU found on PCI bus");
    None
}
