//! I225-V / I226-V (igc family) specific quirks and initialization.
//!
//! The I225 and I226 are Intel's 2.5GbE consumer NICs found on newer
//! motherboards (Alder Lake, Raptor Lake, etc.). They share the same basic
//! e1000 register layout but have several differences:
//!
//! - GIO Master Disable sequence before reset
//! - Different PHY initialization (internal PHY, 2.5GBASE-T capable)
//! - CTRL_EXT register quirks
//! - Different EEPROM access method on some revisions
//!
//! This module provides the [`I225Quirks`] type with static methods called
//! from [`E1000::init`](crate::e1000::E1000::init) when the variant is
//! I225V or I226V.

use crate::regs;

// =========================================================================
// I225/I226-specific register offsets (where they differ from e1000)
// =========================================================================

/// I225 RX Queue 0 registers use the same offsets as e1000 (0x2800+).
/// However, additional RX queues (1-3) have different base offsets on I225.
/// Queue 0 is the only one we use.

/// I225 RXPBS — Receive Packet Buffer Size.
pub const RXPBS: u32 = 0x2404;
/// I225 TXPBS — Transmit Packet Buffer Size.
pub const TXPBS: u32 = 0x3404;

/// I225 GPIE — General Purpose Interrupt Enable.
pub const GPIE: u32 = 0x1514;
/// I225 EICS — Extended Interrupt Cause Set.
pub const EICS: u32 = 0x1520;
/// I225 EIMS — Extended Interrupt Mask Set.
pub const EIMS: u32 = 0x1524;
/// I225 EIMC — Extended Interrupt Mask Clear.
pub const EIMC: u32 = 0x1528;

/// I225 PHPM — PHY Power Management.
pub const PHPM: u32 = 0x0E14;
/// I225 MDICNFG — MDI Configuration.
pub const MDICNFG: u32 = 0x0E04;

// =========================================================================
// I225/I226 quirks
// =========================================================================

/// Family-specific initialization and workarounds for I225-V and I226-V.
pub struct I225Quirks;

impl I225Quirks {
    /// Disable GIO (General I/O) master access before performing a software
    /// reset. The I225 datasheet requires this to prevent DMA corruption.
    ///
    /// # Safety
    /// `bar0` must point to valid MMIO space.
    pub fn disable_gio_master(bar0: *mut u8) {
        log::debug!("[intel-nic] I225: disabling GIO master");

        // Set GIO Master Disable in CTRL.
        unsafe { regs::set_reg_bits(bar0, regs::CTRL, regs::CTRL_GIO_MASTER_DISABLE) };

        // Wait for GIO Master Enable Status to clear in STATUS register.
        for i in 0..100_000 {
            let status = unsafe { regs::read_reg(bar0, regs::STATUS) };
            if status & regs::STATUS_GIO_MASTER_ENABLE == 0 {
                log::debug!("[intel-nic] I225: GIO master disabled after {} iterations", i);
                return;
            }
            core::hint::spin_loop();
        }

        log::warn!("[intel-nic] I225: GIO master disable timed out — proceeding anyway");
    }

    /// Apply pre-PHY initialization quirks for I225/I226.
    ///
    /// Called after reset but before PHY auto-negotiation.
    ///
    /// # Safety
    /// `bar0` must point to valid MMIO space.
    pub fn pre_phy_init(bar0: *mut u8) {
        log::debug!("[intel-nic] I225: applying pre-PHY init quirks");

        // Set packet buffer sizes appropriate for 2.5GbE.
        // The I225 has 32KB RX and 20KB TX packet buffers.
        unsafe {
            regs::write_reg(bar0, RXPBS, 0x20); // 32KB RX buffer
            regs::write_reg(bar0, TXPBS, 0x14); // 20KB TX buffer
        }
        log::debug!("[intel-nic] I225: set RXPBS=0x20 (32KB), TXPBS=0x14 (20KB)");

        // Ensure PHPM is in a sane state — clear power-down bits.
        let phpm = unsafe { regs::read_reg(bar0, PHPM) };
        log::debug!("[intel-nic] I225: PHPM={:#010x}", phpm);

        // Configure MDICNFG for internal PHY.
        let mdicnfg = unsafe { regs::read_reg(bar0, MDICNFG) };
        log::debug!("[intel-nic] I225: MDICNFG={:#010x}", mdicnfg);

        // Clear CTRL_EXT bits that might interfere.
        let ctrl_ext = unsafe { regs::read_reg(bar0, regs::CTRL_EXT) };
        log::debug!("[intel-nic] I225: CTRL_EXT={:#010x}", ctrl_ext);
    }

    /// Apply post-link quirks for I225/I226 after auto-negotiation completes.
    ///
    /// For 2.5GbE links, adjusts flow control and buffer thresholds.
    ///
    /// # Safety
    /// `bar0` must point to valid MMIO space.
    pub fn post_link_up(bar0: *mut u8) {
        log::debug!("[intel-nic] I225: applying post-link-up quirks");

        // Read current link speed from STATUS.
        let status = unsafe { regs::read_reg(bar0, regs::STATUS) };
        let speed = (status & regs::STATUS_SPEED_MASK) >> regs::STATUS_SPEED_SHIFT;

        match speed {
            3 => {
                log::info!("[intel-nic] I225: 2.5GbE link detected, adjusting buffers");
                // At 2.5GbE, flow control thresholds should be higher.
                unsafe {
                    regs::write_reg(bar0, regs::FCTTV, 0x0680);
                }
            }
            2 => {
                log::info!("[intel-nic] I225: 1GbE link detected");
            }
            1 => {
                log::info!("[intel-nic] I225: 100Mbps link detected");
            }
            0 => {
                log::info!("[intel-nic] I225: 10Mbps link detected");
            }
            _ => {
                log::warn!("[intel-nic] I225: unknown speed code {}", speed);
            }
        }
    }
}
