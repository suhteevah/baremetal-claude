//! PHY (physical layer transceiver) management via the MDIC register.
//!
//! All Intel e1000/e1000e/igc NICs access the PHY through the MDI (Management
//! Data Interface) control register at offset 0x0020. This module provides
//! read/write access to PHY registers and common operations like reset,
//! auto-negotiation, and link detection.

use crate::regs;
use crate::NicVariant;

// =========================================================================
// Standard MII PHY Register Addresses (IEEE 802.3)
// =========================================================================

/// PHY Control Register.
pub const PHY_CTRL: u32 = 0;
/// PHY Status Register.
pub const PHY_STATUS: u32 = 1;
/// PHY Identifier 1 (OUI bits [3:18]).
pub const PHY_ID1: u32 = 2;
/// PHY Identifier 2 (OUI bits [19:24], model, revision).
pub const PHY_ID2: u32 = 3;
/// Auto-Negotiation Advertisement Register.
pub const PHY_AUTONEG_ADV: u32 = 4;
/// Auto-Negotiation Link Partner Ability Register.
pub const PHY_AUTONEG_LP: u32 = 5;
/// Auto-Negotiation Expansion Register.
pub const PHY_AUTONEG_EXP: u32 = 6;
/// 1000BASE-T Control Register.
pub const PHY_1000T_CTRL: u32 = 9;
/// 1000BASE-T Status Register.
pub const PHY_1000T_STATUS: u32 = 10;

// =========================================================================
// PHY Control Register Bits (register 0)
// =========================================================================

/// Collision Test.
pub const PHY_CTRL_COLL_TEST: u16 = 1 << 7;
/// Full Duplex.
pub const PHY_CTRL_FULL_DUPLEX: u16 = 1 << 8;
/// Restart Auto-Negotiation.
pub const PHY_CTRL_RESTART_AUTONEG: u16 = 1 << 9;
/// Isolate PHY from MII.
pub const PHY_CTRL_ISOLATE: u16 = 1 << 10;
/// Power Down.
pub const PHY_CTRL_POWER_DOWN: u16 = 1 << 11;
/// Enable Auto-Negotiation.
pub const PHY_CTRL_AUTONEG_EN: u16 = 1 << 12;
/// Speed Selection MSB (bit 6 is LSB).
pub const PHY_CTRL_SPEED_MSB: u16 = 1 << 13;
/// Loopback.
pub const PHY_CTRL_LOOPBACK: u16 = 1 << 14;
/// PHY Reset — self-clearing.
pub const PHY_CTRL_RESET: u16 = 1 << 15;

// =========================================================================
// PHY Status Register Bits (register 1)
// =========================================================================

/// Extended Capabilities supported.
pub const PHY_STATUS_EXT_CAP: u16 = 1 << 0;
/// Jabber condition detected.
pub const PHY_STATUS_JABBER: u16 = 1 << 1;
/// Link Status — 1 = link established.
pub const PHY_STATUS_LINK: u16 = 1 << 2;
/// Auto-Negotiation Ability.
pub const PHY_STATUS_AUTONEG_CAP: u16 = 1 << 3;
/// Remote Fault detected.
pub const PHY_STATUS_REMOTE_FAULT: u16 = 1 << 4;
/// Auto-Negotiation Complete.
pub const PHY_STATUS_AUTONEG_DONE: u16 = 1 << 5;

// =========================================================================
// Auto-Negotiation Advertisement Bits (register 4)
// =========================================================================

/// 10BASE-T Half Duplex.
pub const AUTONEG_10_HD: u16 = 1 << 5;
/// 10BASE-T Full Duplex.
pub const AUTONEG_10_FD: u16 = 1 << 6;
/// 100BASE-TX Half Duplex.
pub const AUTONEG_100_HD: u16 = 1 << 7;
/// 100BASE-TX Full Duplex.
pub const AUTONEG_100_FD: u16 = 1 << 8;
/// Pause capability (flow control).
pub const AUTONEG_PAUSE: u16 = 1 << 10;
/// Asymmetric Pause.
pub const AUTONEG_ASYM_PAUSE: u16 = 1 << 11;

// =========================================================================
// 1000BASE-T Control Bits (register 9)
// =========================================================================

/// Advertise 1000BASE-T Half Duplex.
pub const GBCR_1000T_HD: u16 = 1 << 8;
/// Advertise 1000BASE-T Full Duplex.
pub const GBCR_1000T_FD: u16 = 1 << 9;

// =========================================================================
// Link speed/duplex result
// =========================================================================

/// Detected link speed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkSpeed {
    Speed10,
    Speed100,
    Speed1000,
    Speed2500,
}

/// Detected duplex mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Duplex {
    Half,
    Full,
}

/// Link status information.
#[derive(Debug, Clone, Copy)]
pub struct LinkStatus {
    pub up: bool,
    pub speed: LinkSpeed,
    pub duplex: Duplex,
}

// =========================================================================
// PHY Manager
// =========================================================================

/// Manages PHY access and link configuration for Intel NICs.
pub struct PhyManager {
    /// MMIO base address (BAR0 mapped into virtual memory).
    bar0: *mut u8,
    /// PHY address on the MDIO bus (usually 1).
    phy_addr: u32,
    /// NIC variant for family-specific behavior.
    variant: NicVariant,
}

// SAFETY: PhyManager holds a raw pointer to MMIO space which is a fixed
// hardware address. It is only accessed through &mut self methods.
unsafe impl Send for PhyManager {}

impl PhyManager {
    /// Create a new PHY manager.
    ///
    /// # Safety
    /// `bar0` must point to valid MMIO space for the NIC.
    pub unsafe fn new(bar0: *mut u8, variant: NicVariant) -> Self {
        // Default PHY address is 1 for most Intel NICs.
        let phy_addr = 1;
        log::debug!("[intel-nic] PHY manager created: variant={:?}, phy_addr={}", variant, phy_addr);
        Self { bar0, phy_addr, variant }
    }

    /// Read a PHY register via the MDIC register.
    ///
    /// Performs a single MDI read cycle and spins until the READY bit is set
    /// or a timeout is reached.
    pub fn read(&self, reg: u32) -> Result<u16, ()> {
        log::trace!("[intel-nic] PHY read: reg={}", reg);

        let mdic_val = regs::MDIC_OP_READ
            | (self.phy_addr << regs::MDIC_PHYADD_SHIFT)
            | (reg << regs::MDIC_REGADD_SHIFT);

        unsafe { regs::write_reg(self.bar0, regs::MDIC, mdic_val) };

        // Spin until READY or ERROR — typical completion is <64 us.
        for _ in 0..10_000 {
            let val = unsafe { regs::read_reg(self.bar0, regs::MDIC) };
            if val & regs::MDIC_ERROR != 0 {
                log::error!("[intel-nic] PHY read error: reg={}, MDIC={:#010x}", reg, val);
                return Err(());
            }
            if val & regs::MDIC_READY != 0 {
                let data = (val & regs::MDIC_DATA_MASK) as u16;
                log::trace!("[intel-nic] PHY read complete: reg={}, data={:#06x}", reg, data);
                return Ok(data);
            }
            // Small delay — volatile read acts as memory fence.
            core::hint::spin_loop();
        }

        log::error!("[intel-nic] PHY read timeout: reg={}", reg);
        Err(())
    }

    /// Write a PHY register via the MDIC register.
    pub fn write(&self, reg: u32, data: u16) -> Result<(), ()> {
        log::trace!("[intel-nic] PHY write: reg={}, data={:#06x}", reg, data);

        let mdic_val = regs::MDIC_OP_WRITE
            | (self.phy_addr << regs::MDIC_PHYADD_SHIFT)
            | (reg << regs::MDIC_REGADD_SHIFT)
            | (data as u32 & regs::MDIC_DATA_MASK);

        unsafe { regs::write_reg(self.bar0, regs::MDIC, mdic_val) };

        // Spin until READY or ERROR.
        for _ in 0..10_000 {
            let val = unsafe { regs::read_reg(self.bar0, regs::MDIC) };
            if val & regs::MDIC_ERROR != 0 {
                log::error!("[intel-nic] PHY write error: reg={}, MDIC={:#010x}", reg, val);
                return Err(());
            }
            if val & regs::MDIC_READY != 0 {
                log::trace!("[intel-nic] PHY write complete: reg={}", reg);
                return Ok(());
            }
            core::hint::spin_loop();
        }

        log::error!("[intel-nic] PHY write timeout: reg={}", reg);
        Err(())
    }

    /// Reset the PHY by setting the reset bit in PHY control register 0.
    /// Waits for the reset to self-clear.
    pub fn reset(&self) -> Result<(), ()> {
        log::info!("[intel-nic] PHY reset initiated");

        self.write(PHY_CTRL, PHY_CTRL_RESET)?;

        // Wait for reset bit to self-clear (typically <500ms).
        for i in 0..100_000 {
            let ctrl = self.read(PHY_CTRL)?;
            if ctrl & PHY_CTRL_RESET == 0 {
                log::info!("[intel-nic] PHY reset complete after {} iterations", i);
                return Ok(());
            }
            core::hint::spin_loop();
        }

        log::error!("[intel-nic] PHY reset timeout — reset bit did not clear");
        Err(())
    }

    /// Start auto-negotiation.
    ///
    /// Advertises all supported speeds/duplex modes and restarts the
    /// auto-negotiation process.
    pub fn start_autoneg(&self) -> Result<(), ()> {
        log::info!("[intel-nic] starting auto-negotiation");

        // Read current auto-neg advertisement.
        let mut adv = self.read(PHY_AUTONEG_ADV)?;
        log::debug!("[intel-nic] current AUTONEG_ADV: {:#06x}", adv);

        // Advertise 10/100 all modes + pause.
        adv |= AUTONEG_10_HD | AUTONEG_10_FD | AUTONEG_100_HD | AUTONEG_100_FD | AUTONEG_PAUSE;
        self.write(PHY_AUTONEG_ADV, adv)?;
        log::debug!("[intel-nic] updated AUTONEG_ADV: {:#06x}", adv);

        // Advertise 1000BASE-T full duplex.
        let mut gb_ctrl = self.read(PHY_1000T_CTRL)?;
        gb_ctrl |= GBCR_1000T_FD;
        // For I225/I226, also advertise 2.5GbE if supported.
        if matches!(self.variant, NicVariant::I225V | NicVariant::I226V) {
            log::debug!("[intel-nic] I225/I226: advertising 2.5GbE capability");
            // 2.5GBASE-T is advertised through vendor-specific PHY registers.
            // The I225 PHY (I225 internal PHY) handles this automatically
            // when auto-negotiation is enabled.
        }
        self.write(PHY_1000T_CTRL, gb_ctrl)?;
        log::debug!("[intel-nic] updated 1000T_CTRL: {:#06x}", gb_ctrl);

        // Enable auto-negotiation and restart it.
        let ctrl = PHY_CTRL_AUTONEG_EN | PHY_CTRL_RESTART_AUTONEG;
        self.write(PHY_CTRL, ctrl)?;
        log::info!("[intel-nic] auto-negotiation restarted");

        Ok(())
    }

    /// Wait for auto-negotiation to complete.
    ///
    /// Polls the PHY status register until auto-negotiation is done or a
    /// timeout is reached. Returns `true` if auto-negotiation completed.
    pub fn wait_autoneg(&self, max_polls: u32) -> Result<bool, ()> {
        log::info!("[intel-nic] waiting for auto-negotiation to complete (max {} polls)", max_polls);

        for i in 0..max_polls {
            // Read PHY status twice — the link status bit is latched-low,
            // meaning the first read clears a stale "link down" condition.
            let _ = self.read(PHY_STATUS)?;
            let status = self.read(PHY_STATUS)?;

            if status & PHY_STATUS_AUTONEG_DONE != 0 {
                log::info!("[intel-nic] auto-negotiation complete after {} polls, status={:#06x}",
                    i, status);
                return Ok(true);
            }

            if i % 10_000 == 0 && i > 0 {
                log::debug!("[intel-nic] auto-negotiation still in progress ({} polls, status={:#06x})",
                    i, status);
            }

            core::hint::spin_loop();
        }

        log::warn!("[intel-nic] auto-negotiation did not complete within {} polls", max_polls);
        Ok(false)
    }

    /// Detect current link status, speed, and duplex from PHY registers.
    pub fn link_status(&self) -> Result<LinkStatus, ()> {
        // Read status twice (latched-low link bit).
        let _ = self.read(PHY_STATUS)?;
        let status = self.read(PHY_STATUS)?;

        let up = status & PHY_STATUS_LINK != 0;

        // Read the MAC STATUS register for resolved speed/duplex.
        let mac_status = unsafe { regs::read_reg(self.bar0, regs::STATUS) };

        let duplex = if mac_status & regs::STATUS_FD != 0 {
            Duplex::Full
        } else {
            Duplex::Half
        };

        let speed = match (mac_status & regs::STATUS_SPEED_MASK) >> regs::STATUS_SPEED_SHIFT {
            0 => LinkSpeed::Speed10,
            1 => LinkSpeed::Speed100,
            2 => LinkSpeed::Speed1000,
            3 => {
                // Speed 3 = 1000 on e1000, but on I225/I226 this may indicate 2.5GbE.
                if matches!(self.variant, NicVariant::I225V | NicVariant::I226V) {
                    LinkSpeed::Speed2500
                } else {
                    LinkSpeed::Speed1000
                }
            }
            _ => LinkSpeed::Speed10,
        };

        log::info!("[intel-nic] link status: up={}, speed={:?}, duplex={:?} (STATUS={:#010x})",
            up, speed, duplex, mac_status);

        Ok(LinkStatus { up, speed, duplex })
    }

    /// Poll for link up, returning `true` when the link comes up or `false`
    /// on timeout.
    pub fn wait_link_up(&self, max_polls: u32) -> Result<bool, ()> {
        log::info!("[intel-nic] waiting for link up (max {} polls)", max_polls);

        for i in 0..max_polls {
            let status = self.link_status()?;
            if status.up {
                log::info!("[intel-nic] link is UP: {:?} {:?}", status.speed, status.duplex);
                return Ok(true);
            }

            if i % 10_000 == 0 && i > 0 {
                log::debug!("[intel-nic] still waiting for link ({} polls)", i);
            }

            core::hint::spin_loop();
        }

        log::warn!("[intel-nic] link did not come up within {} polls", max_polls);
        Ok(false)
    }

    /// Read the PHY identifier (OUI, model, revision).
    pub fn read_phy_id(&self) -> Result<(u32, u8, u8), ()> {
        let id1 = self.read(PHY_ID1)?;
        let id2 = self.read(PHY_ID2)?;

        // OUI is spread across both registers.
        let oui = ((id1 as u32) << 6) | ((id2 as u32) >> 10);
        let model = ((id2 >> 4) & 0x3F) as u8;
        let revision = (id2 & 0x0F) as u8;

        log::info!("[intel-nic] PHY ID: OUI={:#08x}, model={:#04x}, rev={:#04x} (raw: {:#06x} {:#06x})",
            oui, model, revision, id1, id2);

        Ok((oui, model, revision))
    }
}
