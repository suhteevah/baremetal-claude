//! Intel NIC integration for ClaudioOS.
//!
//! Detects Intel e1000/e1000e/igc NICs via PCI, initializes the E1000 driver,
//! and provides a smoltcp [`Device`] adapter so the NIC can be used with the
//! full smoltcp TCP/IP stack (DHCP, DNS, TCP, TLS) — identical to VirtIO-net.
//!
//! # Architecture
//!
//! ```text
//!   smoltcp Interface
//!       ↕ Device trait
//!   IntelSmoltcpDevice (this module)
//!       ↕ E1000::transmit / E1000::receive
//!   claudio-intel-nic crate
//!       ↕ MMIO registers + DMA descriptor rings
//!   Intel NIC hardware
//! ```

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::Ordering;

use claudio_intel_nic::{E1000, NicVariant};
use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::socket::dhcpv4;
use smoltcp::time::Instant;
use smoltcp::wire::{EthernetAddress, IpCidr, Ipv4Address, Ipv4Cidr};

use crate::pci::PciDevice;

// ---------------------------------------------------------------------------
// PCI detection
// ---------------------------------------------------------------------------

/// Scan stored PCI devices for a supported Intel NIC.
///
/// Checks vendor 0x8086 against all device IDs known to `NicVariant::from_pci_ids`.
/// Returns the first match along with its NIC variant.
pub fn find_intel_nic() -> Option<(PciDevice, NicVariant)> {
    // Iterate all PCI devices and check for Intel NIC vendor/device combos.
    // We use the existing find infrastructure — scan each stored device.
    crate::pci::find_by_predicate(|dev| {
        NicVariant::from_pci_ids(dev.vendor_id, dev.device_id).map(|v| (dev, v))
    })
}

/// Initialize an Intel NIC from its PCI device descriptor.
///
/// Handles BAR0 MMIO mapping (32-bit or 64-bit BAR), provides the
/// virt-to-phys translation function, and calls `E1000::init`.
///
/// # Safety
/// PCI bus mastering must be enabled before calling this.
pub unsafe fn init_e1000(pci_dev: &PciDevice, variant: NicVariant) -> Result<E1000, &'static str> {
    let phys_mem_offset = crate::PHYS_MEM_OFFSET.load(Ordering::Relaxed);

    // Decode BAR0 — Intel NICs use memory-mapped I/O.
    let bar0_raw = pci_dev.bar0;
    if bar0_raw & 1 != 0 {
        log::error!("[intel-nic] BAR0 is I/O-space ({:#x}), expected MMIO", bar0_raw);
        return Err("BAR0 is I/O-space, expected MMIO");
    }

    let mmio_phys: u64 = if (bar0_raw >> 1) & 0x3 == 0x2 {
        // 64-bit BAR: read BAR1 (offset 0x14) for upper 32 bits.
        let bar1 = crate::pci::read_config_pub(
            pci_dev.bus, pci_dev.device, pci_dev.function, 0x14,
        );
        ((bar1 as u64) << 32) | ((bar0_raw & !0xF) as u64)
    } else {
        // 32-bit BAR.
        (bar0_raw & !0xF) as u64
    };

    log::info!(
        "[intel-nic] {} at PCI {:02x}:{:02x}.{}, MMIO phys={:#x}, IRQ={}",
        variant.name(),
        pci_dev.bus,
        pci_dev.device,
        pci_dev.function,
        mmio_phys,
        pci_dev.irq_line,
    );

    // Map physical MMIO address to virtual address via the bootloader's
    // identity-offset mapping.
    let mmio_virt = phys_mem_offset + mmio_phys;
    log::info!("[intel-nic] MMIO virtual address: {:#x}", mmio_virt);

    // Initialize the E1000 driver.
    let e1000 = unsafe {
        E1000::init(
            mmio_virt as *mut u8,
            pci_dev.irq_line,
            variant,
            virt_to_phys,
        )
    }.map_err(|e| {
        log::error!("[intel-nic] E1000 init failed: {:?}", e);
        "E1000 initialization failed"
    })?;

    log::info!(
        "[intel-nic] initialized: MAC={:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        e1000.mac_address()[0], e1000.mac_address()[1], e1000.mac_address()[2],
        e1000.mac_address()[3], e1000.mac_address()[4], e1000.mac_address()[5],
    );

    Ok(e1000)
}

// ---------------------------------------------------------------------------
// Virtual-to-physical address translation
// ---------------------------------------------------------------------------

/// Translate a virtual address to a physical address by walking the page table.
///
/// This is the `VirtToPhysFn` callback provided to the E1000 driver for DMA
/// descriptor ring and buffer address translation.
fn virt_to_phys(virt_addr: usize) -> u64 {
    let phys_mem_offset = crate::PHYS_MEM_OFFSET.load(Ordering::Relaxed);
    let virt = x86_64::VirtAddr::new(virt_addr as u64);
    let offset_virt = x86_64::VirtAddr::new(phys_mem_offset);

    // Read CR3 to get the level-4 page table physical address.
    let (l4_frame, _) = x86_64::registers::control::Cr3::read();
    let l4_phys = l4_frame.start_address();
    let l4_virt = offset_virt + l4_phys.as_u64();
    let l4_table = unsafe {
        &*(l4_virt.as_ptr() as *const x86_64::structures::paging::PageTable)
    };

    // Level 4
    let l4_entry = &l4_table[virt.p4_index()];
    if l4_entry.is_unused() {
        panic!("[intel-nic] virt_to_phys: L4 entry unused for {:#x}", virt_addr);
    }

    let l3_virt = offset_virt + l4_entry.addr().as_u64();
    let l3_table = unsafe {
        &*(l3_virt.as_ptr() as *const x86_64::structures::paging::PageTable)
    };

    // Level 3
    let l3_entry = &l3_table[virt.p3_index()];
    if l3_entry.is_unused() {
        panic!("[intel-nic] virt_to_phys: L3 entry unused for {:#x}", virt_addr);
    }
    if l3_entry.flags().contains(x86_64::structures::paging::PageTableFlags::HUGE_PAGE) {
        let base = l3_entry.addr().as_u64();
        return base + (virt_addr as u64 & 0x3FFF_FFFF);
    }

    let l2_virt = offset_virt + l3_entry.addr().as_u64();
    let l2_table = unsafe {
        &*(l2_virt.as_ptr() as *const x86_64::structures::paging::PageTable)
    };

    // Level 2
    let l2_entry = &l2_table[virt.p2_index()];
    if l2_entry.is_unused() {
        panic!("[intel-nic] virt_to_phys: L2 entry unused for {:#x}", virt_addr);
    }
    if l2_entry.flags().contains(x86_64::structures::paging::PageTableFlags::HUGE_PAGE) {
        let base = l2_entry.addr().as_u64();
        return base + (virt_addr as u64 & 0x1F_FFFF);
    }

    let l1_virt = offset_virt + l2_entry.addr().as_u64();
    let l1_table = unsafe {
        &*(l1_virt.as_ptr() as *const x86_64::structures::paging::PageTable)
    };

    // Level 1
    let l1_entry = &l1_table[virt.p1_index()];
    if l1_entry.is_unused() {
        panic!("[intel-nic] virt_to_phys: L1 entry unused for {:#x}", virt_addr);
    }
    let frame_phys = l1_entry.addr().as_u64();
    frame_phys + (virt_addr as u64 & 0xFFF)
}

// ---------------------------------------------------------------------------
// smoltcp Device adapter
// ---------------------------------------------------------------------------

/// smoltcp [`Device`] implementation wrapping an Intel E1000 NIC.
///
/// This provides the same interface as `SmoltcpDevice` in `claudio-net` but
/// backed by the Intel NIC driver instead of VirtIO-net.
pub struct IntelSmoltcpDevice {
    /// The underlying E1000 driver instance.
    pub nic: E1000,
    /// Temporary receive buffer (2048 bytes, matching E1000 descriptor buffers).
    rx_buf: Vec<u8>,
}

impl IntelSmoltcpDevice {
    pub fn new(nic: E1000) -> Self {
        Self {
            nic,
            rx_buf: vec![0u8; 2048],
        }
    }
}

/// Receive token — delivers one Ethernet frame to smoltcp.
pub struct IntelRxToken {
    frame: Vec<u8>,
}

impl RxToken for IntelRxToken {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        f(&self.frame)
    }
}

/// Transmit token — smoltcp fills the frame and we push it to the E1000.
pub struct IntelTxToken<'a> {
    nic: &'a mut E1000,
}

impl<'a> TxToken for IntelTxToken<'a> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let mut buf = vec![0u8; len];
        let result = f(&mut buf);

        log::trace!("[intel-nic] TX: sending {} byte Ethernet frame", len);
        if let Err(e) = self.nic.transmit(&buf) {
            log::warn!("[intel-nic] TX failed: {:?} (frame len: {})", e, len);
        }

        result
    }
}

impl Device for IntelSmoltcpDevice {
    type RxToken<'a> = IntelRxToken;
    type TxToken<'a> = IntelTxToken<'a>;

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        match self.nic.receive(&mut self.rx_buf) {
            Ok(Some(len)) => {
                let frame = self.rx_buf[..len].to_vec();
                let rx = IntelRxToken { frame };
                let tx = IntelTxToken { nic: &mut self.nic };
                Some((rx, tx))
            }
            Ok(None) => None,
            Err(e) => {
                log::trace!("[intel-nic] RX error: {:?}", e);
                None
            }
        }
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        Some(IntelTxToken { nic: &mut self.nic })
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ethernet;
        caps.max_transmission_unit = 1514; // Standard Ethernet MTU
        caps.max_burst_size = Some(1);
        caps
    }
}

// ---------------------------------------------------------------------------
// Intel Network Stack — mirrors claudio_net::NetworkStack for Intel NICs
// ---------------------------------------------------------------------------

/// Network stack for Intel NICs.
///
/// Provides the same interface as `claudio_net::NetworkStack` but uses the
/// Intel E1000 driver instead of VirtIO-net. This allows the kernel's main
/// loop to use either stack type transparently.
pub struct IntelNetworkStack {
    pub iface: Interface,
    pub sockets: SocketSet<'static>,
    pub device: IntelSmoltcpDevice,
    dhcp_handle: SocketHandle,
    /// Set to `true` once DHCP assigns an IP address.
    pub has_ip: bool,
    /// The gateway address from DHCP, if any.
    pub gateway: Option<Ipv4Address>,
    /// DNS server addresses from DHCP.
    pub dns_servers: Vec<Ipv4Address>,
}

impl IntelNetworkStack {
    /// Create a new network stack around the given Intel NIC.
    pub fn new(nic: E1000) -> Self {
        let mac = nic.mac_address();
        let ethernet_addr = EthernetAddress(mac);
        log::info!(
            "[intel-net] creating smoltcp interface, MAC {}",
            ethernet_addr
        );

        let config = Config::new(ethernet_addr.into());
        let mut device = IntelSmoltcpDevice::new(nic);
        let iface = Interface::new(config, &mut device, Instant::ZERO);
        let mut sockets = SocketSet::new(vec![]);

        // Add a DHCP client socket.
        let dhcp_socket = dhcpv4::Socket::new();
        let dhcp_handle = sockets.add(dhcp_socket);

        Self {
            iface,
            sockets,
            device,
            dhcp_handle,
            has_ip: false,
            gateway: None,
            dns_servers: Vec::new(),
        }
    }

    /// Drive the network stack forward.
    ///
    /// Must be called regularly. Processes incoming frames, handles DHCP,
    /// and advances TCP/UDP state machines.
    pub fn poll(&mut self, timestamp: Instant) -> bool {
        let result = self.iface.poll(timestamp, &mut self.device, &mut self.sockets);
        self.process_dhcp();
        result == smoltcp::iface::PollResult::SocketStateChanged
    }

    /// Process DHCP events and apply configuration.
    fn process_dhcp(&mut self) {
        let socket = self.sockets.get_mut::<dhcpv4::Socket>(self.dhcp_handle);
        let event = socket.poll();

        match event {
            None => {}
            Some(dhcpv4::Event::Configured(config)) => {
                let addr = config.address;
                log::info!("[intel-dhcp] acquired IP: {}", addr);

                self.iface.update_ip_addrs(|addrs| {
                    addrs.clear();
                    addrs.push(IpCidr::Ipv4(addr)).ok();
                });

                if let Some(router) = config.router {
                    self.iface
                        .routes_mut()
                        .add_default_ipv4_route(router)
                        .ok();
                    self.gateway = Some(router);
                    log::info!("[intel-dhcp] gateway: {}", router);
                }

                self.dns_servers.clear();
                for dns in config.dns_servers.iter() {
                    self.dns_servers.push(*dns);
                    log::info!("[intel-dhcp] DNS server: {}", dns);
                }

                self.has_ip = true;
            }
            Some(dhcpv4::Event::Deconfigured) => {
                log::warn!("[intel-dhcp] lease lost, deconfigured");
                self.iface.update_ip_addrs(|addrs| addrs.clear());
                self.iface.routes_mut().remove_default_ipv4_route();
                self.has_ip = false;
                self.gateway = None;
                self.dns_servers.clear();
            }
        }
    }

    /// Get the currently assigned IPv4 address, if any.
    pub fn ipv4_addr(&self) -> Option<Ipv4Cidr> {
        self.iface.ip_addrs().iter().find_map(|cidr| match cidr {
            IpCidr::Ipv4(v4) => Some(*v4),
            #[allow(unreachable_patterns)]
            _ => None,
        })
    }

    /// Provide mutable access to the underlying E1000 driver.
    pub fn nic_mut(&mut self) -> &mut E1000 {
        &mut self.device.nic
    }
}

/// Maximum number of poll iterations while waiting for DHCP.
const DHCP_TIMEOUT_POLLS: usize = 200_000;

/// Initialize the Intel NIC network stack end-to-end.
///
/// 1. Detects an Intel NIC via PCI enumeration.
/// 2. Initializes the E1000 driver.
/// 3. Wraps it in a smoltcp Interface with DHCP.
/// 4. Polls until a DHCP lease is acquired.
///
/// Returns `None` if no Intel NIC is found, or `Err` on init/DHCP failure.
pub fn init_intel_network(
    now: impl Fn() -> Instant,
) -> Option<Result<IntelNetworkStack, &'static str>> {
    let (pci_dev, variant) = find_intel_nic()?;

    log::info!(
        "[intel-nic] detected {} at PCI {:02x}:{:02x}.{}",
        variant.name(),
        pci_dev.bus,
        pci_dev.device,
        pci_dev.function,
    );

    // Ensure bus mastering is enabled (should already be from PCI enum, but
    // be safe).
    crate::pci::enable_bus_master_for(pci_dev.bus, pci_dev.device, pci_dev.function);

    let e1000 = match unsafe { init_e1000(&pci_dev, variant) } {
        Ok(nic) => nic,
        Err(e) => return Some(Err(e)),
    };

    let mut stack = IntelNetworkStack::new(e1000);

    // Poll until DHCP assigns an IP address.
    log::info!("[intel-net] waiting for DHCP lease...");
    for i in 0..DHCP_TIMEOUT_POLLS {
        stack.poll(now());

        if stack.has_ip {
            if let Some(addr) = stack.ipv4_addr() {
                log::info!("[intel-net] network ready: IP {}", addr);
            }
            return Some(Ok(stack));
        }

        if i > 0 && i % 10_000 == 0 {
            log::debug!("[intel-net] still waiting for DHCP ({} polls)...", i);
        }
    }

    log::error!("[intel-net] DHCP timed out after {} polls", DHCP_TIMEOUT_POLLS);
    Some(Err("DHCP timeout"))
}
