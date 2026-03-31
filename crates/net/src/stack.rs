//! smoltcp network stack integration.
//!
//! Wraps a [`VirtioNet`] NIC driver in smoltcp's [`Device`] trait so the TCP/IP
//! stack can send and receive Ethernet frames.  Provides a [`NetworkStack`]
//! that owns the smoltcp [`Interface`], [`SocketSet`], and device, and drives
//! DHCP lease acquisition and periodic polling.

extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;

use smoltcp::iface::{Config, Interface, PollResult, SocketHandle, SocketSet};
use smoltcp::phy::{Device, DeviceCapabilities, Medium, RxToken, TxToken};
use smoltcp::socket::dhcpv4;
use smoltcp::time::Instant;
use smoltcp::wire::{EthernetAddress, IpCidr, Ipv4Address, Ipv4Cidr};

use crate::nic::VirtioNet;
use crate::NicDriver;

// ---------------------------------------------------------------------------
// smoltcp Device adapter
// ---------------------------------------------------------------------------

/// Wrapper around [`VirtioNet`] that implements smoltcp's [`Device`] trait.
pub struct SmoltcpDevice {
    nic: VirtioNet,
    /// Temporary receive buffer.  We pull one frame at a time from the NIC and
    /// hand it to smoltcp via an [`RxToken`].
    rx_buf: Vec<u8>,
}

impl SmoltcpDevice {
    pub fn new(nic: VirtioNet) -> Self {
        Self {
            nic,
            rx_buf: vec![0u8; 2048],
        }
    }
}

/// Receive token — hands one Ethernet frame to smoltcp.
pub struct SmoltcpRxToken {
    /// The received frame bytes (no VirtIO header — the NIC driver strips it).
    frame: Vec<u8>,
}

impl RxToken for SmoltcpRxToken {
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&[u8]) -> R,
    {
        f(&self.frame)
    }
}

/// Transmit token — smoltcp calls `consume` with a closure that fills the
/// frame buffer, and we push it to the NIC.
pub struct SmoltcpTxToken<'a> {
    nic: &'a mut VirtioNet,
}

impl<'a> TxToken for SmoltcpTxToken<'a> {
    fn consume<R, F>(self, len: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        // Allocate a temporary buffer for smoltcp to fill.
        let mut buf = vec![0u8; len];
        let result = f(&mut buf);

        log::trace!("[net] TX: sending {} byte Ethernet frame to NIC", len);
        if let Err(e) = self.nic.transmit(&buf) {
            log::warn!("[net] TX failed: {:?} (frame len: {})", e, len);
        }

        result
    }
}

impl Device for SmoltcpDevice {
    type RxToken<'a> = SmoltcpRxToken;
    type TxToken<'a> = SmoltcpTxToken<'a>;

    fn receive(&mut self, _timestamp: Instant) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        // Try to pull one frame from the NIC.
        match self.nic.receive(&mut self.rx_buf) {
            Ok(Some(len)) => {
                let frame = self.rx_buf[..len].to_vec();
                let rx = SmoltcpRxToken { frame };
                let tx = SmoltcpTxToken { nic: &mut self.nic };
                Some((rx, tx))
            }
            Ok(None) => None,
            Err(e) => {
                log::trace!("[net] RX error: {:?}", e);
                None
            }
        }
    }

    fn transmit(&mut self, _timestamp: Instant) -> Option<Self::TxToken<'_>> {
        Some(SmoltcpTxToken { nic: &mut self.nic })
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
// NetworkStack
// ---------------------------------------------------------------------------

/// High-level network stack combining NIC driver, smoltcp interface, and
/// socket set.
///
/// The caller is expected to call [`NetworkStack::poll`] frequently (on timer
/// interrupts, NIC interrupts, or whenever work might be available).
pub struct NetworkStack {
    pub iface: Interface,
    pub sockets: SocketSet<'static>,
    pub device: SmoltcpDevice,
    dhcp_handle: SocketHandle,
    /// Set to `true` once DHCP assigns an IP address.
    pub has_ip: bool,
    /// The gateway address from DHCP, if any.
    pub gateway: Option<Ipv4Address>,
    /// DNS server addresses from DHCP.
    pub dns_servers: Vec<Ipv4Address>,
}

impl NetworkStack {
    /// Create a new network stack around the given NIC.
    ///
    /// Initializes the smoltcp interface with the NIC's MAC address and adds a
    /// DHCP socket for automatic IP configuration.
    pub fn new(nic: VirtioNet) -> Self {
        let mac = nic.mac_address();
        let ethernet_addr = EthernetAddress(mac);
        log::info!(
            "[net] creating smoltcp interface, MAC {}",
            ethernet_addr
        );

        let config = Config::new(ethernet_addr.into());
        let mut device = SmoltcpDevice::new(nic);

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
    /// This must be called regularly.  It processes incoming frames, handles
    /// DHCP state transitions, and advances TCP/UDP socket state machines.
    ///
    /// Returns `true` if socket state may have changed (useful for deciding
    /// whether to sleep).
    pub fn poll(&mut self, timestamp: Instant) -> bool {
        let result = self.iface.poll(timestamp, &mut self.device, &mut self.sockets);

        // Process DHCP events.
        self.process_dhcp();

        result == PollResult::SocketStateChanged
    }

    /// Check the DHCP socket for configuration events and apply them.
    fn process_dhcp(&mut self) {
        let socket = self.sockets.get_mut::<dhcpv4::Socket>(self.dhcp_handle);
        let event = socket.poll();

        match event {
            None => {}
            Some(dhcpv4::Event::Configured(config)) => {
                let addr = config.address;
                log::info!("[dhcp] acquired IP: {}", addr);

                self.iface.update_ip_addrs(|addrs| {
                    // Remove any previous addresses.
                    addrs.clear();
                    addrs.push(IpCidr::Ipv4(addr)).ok();
                });

                if let Some(router) = config.router {
                    self.iface
                        .routes_mut()
                        .add_default_ipv4_route(router)
                        .ok();
                    self.gateway = Some(router);
                    log::info!("[dhcp] gateway: {}", router);
                }

                // Store DNS servers.
                self.dns_servers.clear();
                for dns in config.dns_servers.iter() {
                    self.dns_servers.push(*dns);
                    log::info!("[dhcp] DNS server: {}", dns);
                }

                self.has_ip = true;
            }
            Some(dhcpv4::Event::Deconfigured) => {
                log::warn!("[dhcp] lease lost, deconfigured");
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
            _ => None,
        })
    }

    /// Provide mutable access to the underlying NIC (e.g. for interrupt
    /// acknowledgment).
    pub fn nic_mut(&mut self) -> &mut VirtioNet {
        &mut self.device.nic
    }
}
