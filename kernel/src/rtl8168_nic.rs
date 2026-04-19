//! Realtek RTL8168 NIC integration for ClaudioOS.
//!
//! Mirrors [`crate::intel_nic`] but backs the smoltcp Device with the
//! `claudio-rtl8168` driver. Targets the RTL8168 found on the HP Victus
//! (PCI 04:00.0, vendor 0x10EC, device 0x8168) — the ethernet dev-loop NIC.

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;
use core::sync::atomic::Ordering;

use claudio_rtl8168::{DiagRegs, Rtl8168, REALTEK_VENDOR_ID, RTL8168_DEVICE_ID};
use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::socket::dhcpv4;
use smoltcp::time::Instant;
use smoltcp::wire::{EthernetAddress, IpCidr, Ipv4Address, Ipv4Cidr};

use crate::pci::{BarKind, PciDevice};

use alloc::string::{String, ToString};
use alloc::format;
use spin::Mutex;

// ---------------------------------------------------------------------------
// Status snapshot for fallback terminal
// ---------------------------------------------------------------------------

/// Captured result of the RTL8168 probe + init + DHCP attempt. The fallback
/// keyboard-echo terminal reads this via [`snapshot`] so we can see what
/// happened on real hardware without a serial cable.
#[derive(Debug, Clone, Default)]
pub struct Rtl8168Status {
    pub found: bool,
    pub bdf: String,
    pub bar2_phys: u64,
    pub bar2_kind: String,
    pub mac: Option<[u8; 6]>,
    pub init_err: Option<String>,
    pub dhcp_err: Option<String>,
    pub ipv4: Option<String>,
    pub gateway: Option<String>,
    /// Diagnostic register snapshot taken after DHCP timeout.
    pub diag: Option<DiagRegs>,
}

static STATUS: Mutex<Option<Rtl8168Status>> = Mutex::new(None);

pub fn snapshot() -> Option<Rtl8168Status> {
    STATUS.lock().clone()
}

fn set(st: Rtl8168Status) {
    *STATUS.lock() = Some(st);
}

// ---------------------------------------------------------------------------
// PCI detection
// ---------------------------------------------------------------------------

pub fn find_rtl8168() -> Option<PciDevice> {
    crate::pci::find_device(REALTEK_VENDOR_ID, RTL8168_DEVICE_ID)
}

/// Initialize the RTL8168 driver from a PCI device descriptor.
///
/// Maps BAR2 MMIO, enables bus mastering + memory space, and calls
/// [`Rtl8168::init`]. BAR2 is used because the RTL8168 in PCIe mode exposes
/// its 256-byte register space there; BAR0 is legacy I/O and is ignored.
pub unsafe fn init_rtl8168(pci_dev: &PciDevice) -> Result<Rtl8168, &'static str> {
    let bar2 = pci_dev.bars[2];
    match bar2.kind {
        BarKind::Mmio32 | BarKind::Mmio64 => {}
        _ => {
            log::error!(
                "[rtl8168-nic] BAR2 kind {:?} is not MMIO (addr={:#x})",
                bar2.kind, bar2.addr,
            );
            return Err("RTL8168 BAR2 is not MMIO");
        }
    }
    if bar2.addr == 0 {
        log::error!("[rtl8168-nic] BAR2 addr is zero");
        return Err("RTL8168 BAR2 is zero");
    }

    let phys_mem_offset = crate::PHYS_MEM_OFFSET.load(Ordering::Relaxed);
    let mmio_virt = phys_mem_offset + bar2.addr;

    log::info!(
        "[rtl8168-nic] RTL8168 at PCI {:02x}:{:02x}.{} BAR2 phys={:#x} virt={:#x} IRQ={}",
        pci_dev.bus, pci_dev.device, pci_dev.function,
        bar2.addr, mmio_virt, pci_dev.irq_line,
    );

    crate::pci::enable_mem_and_bus_master(pci_dev.bus, pci_dev.device, pci_dev.function);

    unsafe {
        Rtl8168::init(mmio_virt, pci_dev.irq_line, virt_to_phys)
            .map_err(|e| {
                log::error!("[rtl8168-nic] driver init failed: {}", e);
                "rtl8168 init failed"
            })
    }
}

// ---------------------------------------------------------------------------
// Virtual-to-physical translation (identical to intel_nic.rs)
// ---------------------------------------------------------------------------

fn virt_to_phys(virt_addr: usize) -> u64 {
    let phys_mem_offset = crate::PHYS_MEM_OFFSET.load(Ordering::Relaxed);
    let virt = x86_64::VirtAddr::new(virt_addr as u64);
    let offset_virt = x86_64::VirtAddr::new(phys_mem_offset);

    let (l4_frame, _) = x86_64::registers::control::Cr3::read();
    let l4_virt = offset_virt + l4_frame.start_address().as_u64();
    let l4_table = unsafe {
        &*(l4_virt.as_ptr() as *const x86_64::structures::paging::PageTable)
    };

    let l4_entry = &l4_table[virt.p4_index()];
    if l4_entry.is_unused() {
        panic!("[rtl8168-nic] virt_to_phys: L4 entry unused for {:#x}", virt_addr);
    }

    let l3_virt = offset_virt + l4_entry.addr().as_u64();
    let l3_table = unsafe {
        &*(l3_virt.as_ptr() as *const x86_64::structures::paging::PageTable)
    };
    let l3_entry = &l3_table[virt.p3_index()];
    if l3_entry.is_unused() {
        panic!("[rtl8168-nic] virt_to_phys: L3 entry unused for {:#x}", virt_addr);
    }
    if l3_entry.flags().contains(x86_64::structures::paging::PageTableFlags::HUGE_PAGE) {
        return l3_entry.addr().as_u64() + (virt_addr as u64 & 0x3FFF_FFFF);
    }

    let l2_virt = offset_virt + l3_entry.addr().as_u64();
    let l2_table = unsafe {
        &*(l2_virt.as_ptr() as *const x86_64::structures::paging::PageTable)
    };
    let l2_entry = &l2_table[virt.p2_index()];
    if l2_entry.is_unused() {
        panic!("[rtl8168-nic] virt_to_phys: L2 entry unused for {:#x}", virt_addr);
    }
    if l2_entry.flags().contains(x86_64::structures::paging::PageTableFlags::HUGE_PAGE) {
        return l2_entry.addr().as_u64() + (virt_addr as u64 & 0x1F_FFFF);
    }

    let l1_virt = offset_virt + l2_entry.addr().as_u64();
    let l1_table = unsafe {
        &*(l1_virt.as_ptr() as *const x86_64::structures::paging::PageTable)
    };
    let l1_entry = &l1_table[virt.p1_index()];
    if l1_entry.is_unused() {
        panic!("[rtl8168-nic] virt_to_phys: L1 entry unused for {:#x}", virt_addr);
    }
    l1_entry.addr().as_u64() + (virt_addr as u64 & 0xFFF)
}

// ---------------------------------------------------------------------------
// smoltcp Device adapter
// ---------------------------------------------------------------------------

pub struct Rtl8168SmoltcpDevice {
    pub nic: Rtl8168,
    rx_buf: Vec<u8>,
}

impl Rtl8168SmoltcpDevice {
    pub fn new(nic: Rtl8168) -> Self {
        Self { nic, rx_buf: vec![0u8; 2048] }
    }
}

pub struct RtlRxToken {
    frame: Vec<u8>,
}

impl RxToken for RtlRxToken {
    fn consume<R, F: FnOnce(&[u8]) -> R>(self, f: F) -> R {
        f(&self.frame)
    }
}

pub struct RtlTxToken<'a> {
    nic: &'a mut Rtl8168,
}

impl<'a> TxToken for RtlTxToken<'a> {
    fn consume<R, F: FnOnce(&mut [u8]) -> R>(self, len: usize, f: F) -> R {
        let mut buf = vec![0u8; len];
        let result = f(&mut buf);
        log::trace!("[rtl8168-nic] TX: sending {} byte Ethernet frame", len);
        if let Err(e) = self.nic.transmit(&buf) {
            log::warn!("[rtl8168-nic] TX failed: {} (frame len: {})", e, len);
        }
        result
    }
}

impl Device for Rtl8168SmoltcpDevice {
    type RxToken<'a> = RtlRxToken;
    type TxToken<'a> = RtlTxToken<'a>;

    fn receive(&mut self, _ts: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        match self.nic.receive(&mut self.rx_buf) {
            Ok(Some(len)) => {
                let frame = self.rx_buf[..len].to_vec();
                Some((RtlRxToken { frame }, RtlTxToken { nic: &mut self.nic }))
            }
            Ok(None) => None,
            Err(e) => {
                log::trace!("[rtl8168-nic] RX error: {}", e);
                None
            }
        }
    }

    fn transmit(&mut self, _ts: Instant) -> Option<Self::TxToken<'_>> {
        Some(RtlTxToken { nic: &mut self.nic })
    }

    fn capabilities(&self) -> DeviceCapabilities {
        let mut caps = DeviceCapabilities::default();
        caps.medium = Medium::Ethernet;
        caps.max_transmission_unit = 1514;
        caps.max_burst_size = Some(1);
        caps
    }
}

// ---------------------------------------------------------------------------
// Network stack
// ---------------------------------------------------------------------------

pub struct Rtl8168NetworkStack {
    pub iface: Interface,
    pub sockets: SocketSet<'static>,
    pub device: Rtl8168SmoltcpDevice,
    dhcp_handle: SocketHandle,
    pub has_ip: bool,
    pub gateway: Option<Ipv4Address>,
    pub dns_servers: Vec<Ipv4Address>,
}

impl Rtl8168NetworkStack {
    pub fn new(nic: Rtl8168) -> Self {
        let mac = nic.mac_address();
        let eth = EthernetAddress(mac);
        log::info!("[rtl8168-net] creating smoltcp interface, MAC {}", eth);

        let config = Config::new(eth.into());
        let mut device = Rtl8168SmoltcpDevice::new(nic);
        let iface = Interface::new(config, &mut device, Instant::ZERO);
        let mut sockets = SocketSet::new(vec![]);
        let dhcp_handle = sockets.add(dhcpv4::Socket::new());

        Self {
            iface, sockets, device, dhcp_handle,
            has_ip: false, gateway: None, dns_servers: Vec::new(),
        }
    }

    pub fn poll(&mut self, ts: Instant) -> bool {
        let result = self.iface.poll(ts, &mut self.device, &mut self.sockets);
        self.process_dhcp();
        result == smoltcp::iface::PollResult::SocketStateChanged
    }

    fn process_dhcp(&mut self) {
        let socket = self.sockets.get_mut::<dhcpv4::Socket>(self.dhcp_handle);
        match socket.poll() {
            None => {}
            Some(dhcpv4::Event::Configured(cfg)) => {
                let addr = cfg.address;
                log::info!("[rtl8168-dhcp] acquired IP: {}", addr);
                self.iface.update_ip_addrs(|addrs| {
                    addrs.clear();
                    addrs.push(IpCidr::Ipv4(addr)).ok();
                });
                if let Some(router) = cfg.router {
                    self.iface.routes_mut().add_default_ipv4_route(router).ok();
                    self.gateway = Some(router);
                    log::info!("[rtl8168-dhcp] gateway: {}", router);
                }
                self.dns_servers.clear();
                for dns in cfg.dns_servers.iter() {
                    self.dns_servers.push(*dns);
                    log::info!("[rtl8168-dhcp] DNS: {}", dns);
                }
                self.has_ip = true;
            }
            Some(dhcpv4::Event::Deconfigured) => {
                log::warn!("[rtl8168-dhcp] lease lost");
                self.iface.update_ip_addrs(|a| a.clear());
                self.iface.routes_mut().remove_default_ipv4_route();
                self.has_ip = false;
                self.gateway = None;
                self.dns_servers.clear();
            }
        }
    }

    pub fn ipv4_addr(&self) -> Option<Ipv4Cidr> {
        self.iface.ip_addrs().iter().find_map(|cidr| match cidr {
            IpCidr::Ipv4(v4) => Some(*v4),
            #[allow(unreachable_patterns)]
            _ => None,
        })
    }

    pub fn nic_mut(&mut self) -> &mut Rtl8168 {
        &mut self.device.nic
    }
}

const DHCP_TIMEOUT_POLLS: usize = 200_000;

pub fn init_rtl8168_network(
    now: impl Fn() -> Instant,
) -> Option<Result<Rtl8168NetworkStack, &'static str>> {
    let pci_dev = match find_rtl8168() {
        Some(d) => d,
        None => {
            set(Rtl8168Status { found: false, ..Default::default() });
            return None;
        }
    };

    let bar2 = pci_dev.bars[2];
    let mut st = Rtl8168Status {
        found: true,
        bdf: format!("{:02x}:{:02x}.{}", pci_dev.bus, pci_dev.device, pci_dev.function),
        bar2_phys: bar2.addr,
        bar2_kind: format!("{:?}", bar2.kind),
        ..Default::default()
    };
    set(st.clone());

    log::info!(
        "[rtl8168-nic] detected RTL8168 at PCI {}",
        st.bdf,
    );

    crate::pci::enable_bus_master_for(pci_dev.bus, pci_dev.device, pci_dev.function);

    let nic = match unsafe { init_rtl8168(&pci_dev) } {
        Ok(n) => n,
        Err(e) => {
            st.init_err = Some(e.to_string());
            set(st);
            return Some(Err(e));
        }
    };

    st.mac = Some(nic.mac_address());
    set(st.clone());

    let mut stack = Rtl8168NetworkStack::new(nic);

    log::info!("[rtl8168-net] waiting for DHCP lease...");
    for i in 0..DHCP_TIMEOUT_POLLS {
        stack.poll(now());
        if stack.has_ip {
            if let Some(addr) = stack.ipv4_addr() {
                log::info!("[rtl8168-net] network ready: IP {}", addr);
                st.ipv4 = Some(format!("{}", addr));
            }
            if let Some(gw) = stack.gateway {
                st.gateway = Some(format!("{}", gw));
            }
            set(st);
            return Some(Ok(stack));
        }
        if i > 0 && i % 10_000 == 0 {
            log::debug!("[rtl8168-net] still waiting for DHCP ({} polls)", i);
        }
    }

    log::error!("[rtl8168-net] DHCP timed out after {} polls", DHCP_TIMEOUT_POLLS);
    st.dhcp_err = Some(format!("DHCP timeout after {} polls", DHCP_TIMEOUT_POLLS));
    // Snapshot diagnostic registers so we can see why DHCP failed.
    st.diag = Some(stack.device.nic.diag_regs());
    set(st);
    Some(Err("DHCP timeout"))
}
