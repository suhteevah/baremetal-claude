//! Core e1000/e1000e/igc driver.
//!
//! The [`E1000`] struct manages a single Intel Ethernet NIC. It handles:
//! - Hardware reset and initialization
//! - MAC address reading (from EEPROM or RAL/RAH registers)
//! - RX/TX descriptor ring setup
//! - Frame transmission and reception
//! - Interrupt handling
//!
//! The same struct is used for all three families (e1000, e1000e/I219, igc/I225).
//! Family-specific quirks are handled via the [`NicVariant`] field and the
//! [`I225Quirks`](crate::i225::I225Quirks) module.

extern crate alloc;

use crate::descriptors::{
    self, RxRing, TxRing, RxDescriptor, TxDescriptor,
    RING_SIZE, BUF_SIZE,
    RX_STATUS_DD, RX_STATUS_EOP,
    TX_CMD_EOP, TX_CMD_IFCS, TX_CMD_RS, TX_STATUS_DD,
};
use crate::phy::PhyManager;
use crate::regs;
use crate::{IntelNicError, NicVariant};

/// Virtual-to-physical address translation function type.
///
/// The kernel must provide this when initializing the driver, since address
/// translation depends on the page table configuration (which the NIC crate
/// does not own).
pub type VirtToPhysFn = fn(virt_addr: usize) -> u64;

/// Intel e1000/e1000e/igc NIC driver.
pub struct E1000 {
    /// MMIO base address (BAR0 mapped into virtual memory).
    bar0: *mut u8,
    /// IRQ line for this device.
    irq: u8,
    /// NIC variant (e1000, I219-V, I225-V, I226-V).
    variant: NicVariant,
    /// MAC address (6 bytes).
    mac: [u8; 6],
    /// Receive descriptor ring.
    rx_ring: RxRing,
    /// Transmit descriptor ring.
    tx_ring: TxRing,
    /// PHY manager for link configuration.
    phy: PhyManager,
    /// Virtual-to-physical address translation.
    virt_to_phys: VirtToPhysFn,
}

// SAFETY: E1000 contains raw pointers to MMIO space and heap-allocated
// descriptor rings. These are only accessed through &mut self methods and
// the driver is designed to be owned by a single task (the network poller).
unsafe impl Send for E1000 {}

impl E1000 {
    /// Initialize the Intel NIC at the given MMIO base address.
    ///
    /// This performs the full hardware initialization sequence:
    /// 1. Software reset
    /// 2. Read MAC address (EEPROM or RAL/RAH fallback)
    /// 3. Initialize PHY and start auto-negotiation
    /// 4. Set up RX descriptor ring
    /// 5. Set up TX descriptor ring
    /// 6. Enable interrupts
    /// 7. Enable RX and TX
    ///
    /// # Arguments
    /// * `bar0` — Virtual address of BAR0 MMIO region
    /// * `irq` — PCI interrupt line
    /// * `variant` — NIC family variant
    /// * `virt_to_phys` — Address translation function
    ///
    /// # Safety
    /// `bar0` must point to a valid, mapped MMIO region for the NIC.
    /// PCI bus mastering must already be enabled.
    pub unsafe fn init(
        bar0: *mut u8,
        irq: u8,
        variant: NicVariant,
        virt_to_phys: VirtToPhysFn,
    ) -> Result<Self, IntelNicError> {
        log::info!("[intel-nic] initializing {} at BAR0={:p}, IRQ={}",
            variant.name(), bar0, irq);

        // -- Step 1: Software reset --
        Self::software_reset(bar0, variant)?;

        // -- Step 2: Disable interrupts during setup --
        log::debug!("[intel-nic] disabling all interrupts during setup");
        unsafe { regs::write_reg(bar0, regs::IMC, 0xFFFF_FFFF) };
        // Read ICR to clear any pending interrupts.
        let icr = unsafe { regs::read_reg(bar0, regs::ICR) };
        log::debug!("[intel-nic] cleared pending interrupts: ICR={:#010x}", icr);

        // -- Step 3: Read MAC address --
        let mac = Self::read_mac(bar0, variant)?;
        log::info!("[intel-nic] MAC address: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]);

        // Program the MAC address into RAL0/RAH0.
        Self::write_mac_to_ral(bar0, &mac);

        // -- Step 4: Clear the Multicast Table Array --
        log::debug!("[intel-nic] clearing multicast table array ({} entries)", regs::MTA_COUNT);
        for i in 0..regs::MTA_COUNT {
            unsafe { regs::write_reg(bar0, regs::MTA_BASE + (i as u32) * 4, 0) };
        }

        // -- Step 5: Initialize PHY --
        let phy = unsafe { PhyManager::new(bar0, variant) };
        if let Ok((oui, model, rev)) = phy.read_phy_id() {
            log::info!("[intel-nic] PHY detected: OUI={:#08x} model={:#04x} rev={:#04x}",
                oui, model, rev);
        } else {
            log::warn!("[intel-nic] could not read PHY ID (may be internal PHY)");
        }

        // Apply I225-specific quirks if needed.
        if matches!(variant, NicVariant::I225V | NicVariant::I226V) {
            log::debug!("[intel-nic] applying I225/I226 quirks");
            crate::i225::I225Quirks::pre_phy_init(bar0);
        }

        // Start auto-negotiation.
        if let Err(()) = phy.start_autoneg() {
            log::error!("[intel-nic] PHY auto-negotiation start failed");
            return Err(IntelNicError::PhyError);
        }

        // -- Step 6: Set up RX ring --
        let mut rx_ring = descriptors::alloc_rx_ring();
        Self::setup_rx_ring(bar0, &mut rx_ring, virt_to_phys);

        // -- Step 7: Set up TX ring --
        let mut tx_ring = descriptors::alloc_tx_ring();
        Self::setup_tx_ring(bar0, &mut tx_ring, virt_to_phys);

        // -- Step 8: Set up CTRL register --
        log::debug!("[intel-nic] configuring CTRL register");
        let mut ctrl = unsafe { regs::read_reg(bar0, regs::CTRL) };
        ctrl |= regs::CTRL_SLU;    // Set Link Up
        ctrl |= regs::CTRL_ASDE;   // Auto-Speed Detection Enable
        ctrl &= !regs::CTRL_LRST;  // Clear Link Reset
        ctrl &= !regs::CTRL_PHY_RST; // Clear PHY Reset
        ctrl &= !regs::CTRL_FRCSPD;  // Let auto-negotiation determine speed
        ctrl &= !regs::CTRL_FRCDPLX; // Let auto-negotiation determine duplex
        unsafe { regs::write_reg(bar0, regs::CTRL, ctrl) };
        log::debug!("[intel-nic] CTRL={:#010x}", ctrl);

        // -- Step 9: Enable interrupts --
        log::debug!("[intel-nic] enabling interrupts");
        let int_mask = regs::ICR_LSC      // Link Status Change
            | regs::ICR_RXT0              // Receive Timer (packet arrived)
            | regs::ICR_RXDMT0            // RX Descriptor Minimum Threshold
            | regs::ICR_RXO              // Receiver Overrun
            | regs::ICR_TXDW;            // TX Descriptor Written Back
        unsafe { regs::write_reg(bar0, regs::IMS, int_mask) };
        log::debug!("[intel-nic] IMS={:#010x}", int_mask);

        // -- Step 10: Enable RX and TX --
        Self::enable_rx(bar0);
        Self::enable_tx(bar0);

        // Log final status.
        let status = unsafe { regs::read_reg(bar0, regs::STATUS) };
        log::info!("[intel-nic] initialization complete: STATUS={:#010x}", status);
        if status & regs::STATUS_LU != 0 {
            log::info!("[intel-nic] link is UP");
        } else {
            log::info!("[intel-nic] link is DOWN (will come up after auto-negotiation)");
        }

        Ok(Self {
            bar0,
            irq,
            variant,
            mac,
            rx_ring,
            tx_ring,
            phy,
            virt_to_phys,
        })
    }

    // =====================================================================
    // Software Reset
    // =====================================================================

    fn software_reset(bar0: *mut u8, variant: NicVariant) -> Result<(), IntelNicError> {
        log::info!("[intel-nic] performing software reset");

        // For I225/I226, disable GIO master first.
        if matches!(variant, NicVariant::I225V | NicVariant::I226V) {
            crate::i225::I225Quirks::disable_gio_master(bar0);
        }

        // Set the RST bit in CTRL.
        unsafe { regs::set_reg_bits(bar0, regs::CTRL, regs::CTRL_RST) };
        log::debug!("[intel-nic] CTRL.RST set, waiting for self-clear");

        // Wait for RST bit to self-clear.
        for i in 0..1_000_000 {
            let ctrl = unsafe { regs::read_reg(bar0, regs::CTRL) };
            if ctrl & regs::CTRL_RST == 0 {
                log::info!("[intel-nic] software reset complete after {} iterations", i);

                // Post-reset delay — Intel datasheets recommend waiting after reset.
                for _ in 0..10_000 {
                    core::hint::spin_loop();
                }

                // Disable interrupts again after reset (reset may re-enable them).
                unsafe { regs::write_reg(bar0, regs::IMC, 0xFFFF_FFFF) };
                let _ = unsafe { regs::read_reg(bar0, regs::ICR) };

                return Ok(());
            }

            if i % 100_000 == 0 && i > 0 {
                log::debug!("[intel-nic] still waiting for reset ({} iterations)", i);
            }

            core::hint::spin_loop();
        }

        log::error!("[intel-nic] software reset timeout — CTRL.RST did not self-clear");
        Err(IntelNicError::ResetTimeout)
    }

    // =====================================================================
    // MAC Address
    // =====================================================================

    /// Read the MAC address. Tries EEPROM first, falls back to RAL0/RAH0.
    fn read_mac(bar0: *mut u8, variant: NicVariant) -> Result<[u8; 6], IntelNicError> {
        log::debug!("[intel-nic] reading MAC address");

        // Try EEPROM read first.
        match Self::read_mac_from_eeprom(bar0, variant) {
            Ok(mac) => {
                log::debug!("[intel-nic] MAC from EEPROM: {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                    mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]);
                if mac != [0xFF; 6] && mac != [0x00; 6] {
                    return Ok(mac);
                }
                log::warn!("[intel-nic] EEPROM MAC is invalid ({:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}), trying RAL/RAH",
                    mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]);
            }
            Err(()) => {
                log::warn!("[intel-nic] EEPROM read failed, trying RAL/RAH registers");
            }
        }

        // Fallback: read from RAL0/RAH0 (may have been programmed by firmware).
        let ral = unsafe { regs::read_reg(bar0, regs::RAL0) };
        let rah = unsafe { regs::read_reg(bar0, regs::RAH0) };
        log::debug!("[intel-nic] RAL0={:#010x}, RAH0={:#010x}", ral, rah);

        let mac = [
            (ral & 0xFF) as u8,
            ((ral >> 8) & 0xFF) as u8,
            ((ral >> 16) & 0xFF) as u8,
            ((ral >> 24) & 0xFF) as u8,
            (rah & 0xFF) as u8,
            ((rah >> 8) & 0xFF) as u8,
        ];

        if mac == [0xFF; 6] || mac == [0x00; 6] {
            log::error!("[intel-nic] no valid MAC address found in EEPROM or RAL/RAH");
            return Err(IntelNicError::EepromError);
        }

        Ok(mac)
    }

    /// Read MAC address from EEPROM using the EERD register.
    ///
    /// The MAC address is stored in EEPROM words 0, 1, 2 (6 bytes total).
    /// The EERD access method differs between e1000 and e1000e/igc:
    /// - e1000: address in bits [15:8], done bit is bit 4
    /// - e1000e/igc: address in bits [15:2], done bit is bit 1
    fn read_mac_from_eeprom(bar0: *mut u8, variant: NicVariant) -> Result<[u8; 6], ()> {
        let mut mac = [0u8; 6];

        let (addr_shift, done_bit) = match variant {
            NicVariant::E1000 => (regs::EERD_ADDR_SHIFT_E1000, regs::EERD_DONE_E1000),
            NicVariant::I219V | NicVariant::I225V | NicVariant::I226V => {
                (regs::EERD_ADDR_SHIFT_E1000E, regs::EERD_DONE_E1000E)
            }
        };

        for word_idx in 0u32..3 {
            let eerd_val = regs::EERD_START | (word_idx << addr_shift);
            unsafe { regs::write_reg(bar0, regs::EERD, eerd_val) };

            log::trace!("[intel-nic] EEPROM read: word={}, EERD={:#010x}", word_idx, eerd_val);

            // Wait for done bit.
            let mut data: u16 = 0;
            let mut done = false;
            for _ in 0..100_000 {
                let val = unsafe { regs::read_reg(bar0, regs::EERD) };
                if val & done_bit != 0 {
                    data = ((val & regs::EERD_DATA_MASK) >> regs::EERD_DATA_SHIFT) as u16;
                    done = true;
                    log::trace!("[intel-nic] EEPROM word {} = {:#06x}", word_idx, data);
                    break;
                }
                core::hint::spin_loop();
            }

            if !done {
                log::error!("[intel-nic] EEPROM read timeout: word={}", word_idx);
                return Err(());
            }

            mac[(word_idx as usize) * 2] = (data & 0xFF) as u8;
            mac[(word_idx as usize) * 2 + 1] = ((data >> 8) & 0xFF) as u8;
        }

        Ok(mac)
    }

    /// Write the MAC address into RAL0/RAH0 registers.
    fn write_mac_to_ral(bar0: *mut u8, mac: &[u8; 6]) {
        let ral = (mac[0] as u32)
            | ((mac[1] as u32) << 8)
            | ((mac[2] as u32) << 16)
            | ((mac[3] as u32) << 24);
        // RAH bit 31 (AV = Address Valid) must be set.
        let rah = (mac[4] as u32)
            | ((mac[5] as u32) << 8)
            | (1 << 31); // AV bit

        unsafe {
            regs::write_reg(bar0, regs::RAL0, ral);
            regs::write_reg(bar0, regs::RAH0, rah);
        }
        log::debug!("[intel-nic] wrote MAC to RAL0={:#010x}, RAH0={:#010x}", ral, rah);
    }

    // =====================================================================
    // RX Ring Setup
    // =====================================================================

    fn setup_rx_ring(bar0: *mut u8, ring: &mut RxRing, virt_to_phys: VirtToPhysFn) {
        log::info!("[intel-nic] setting up RX descriptor ring ({} descriptors)", RING_SIZE);

        // Point each descriptor's buffer_addr to the physical address of its buffer.
        for i in 0..RING_SIZE {
            let buf_virt = ring.buffers[i].as_ptr() as usize;
            let buf_phys = virt_to_phys(buf_virt);
            ring.descriptors[i] = RxDescriptor {
                buffer_addr: buf_phys,
                length: 0,
                checksum: 0,
                status: 0,
                errors: 0,
                special: 0,
            };
            if i < 4 || i == RING_SIZE - 1 {
                log::trace!("[intel-nic] RX desc[{}]: buf_phys={:#x}", i, buf_phys);
            }
        }

        // Program the descriptor ring base address.
        let ring_virt = ring.descriptors.as_ptr() as usize;
        let ring_phys = virt_to_phys(ring_virt);
        log::debug!("[intel-nic] RX ring phys={:#x}, size={} bytes",
            ring_phys, RING_SIZE * core::mem::size_of::<RxDescriptor>());

        unsafe {
            regs::write_reg(bar0, regs::RDBAL, ring_phys as u32);
            regs::write_reg(bar0, regs::RDBAH, (ring_phys >> 32) as u32);
            regs::write_reg(bar0, regs::RDLEN, (RING_SIZE * core::mem::size_of::<RxDescriptor>()) as u32);

            // Head = 0, Tail = RING_SIZE - 1 (NIC owns all descriptors).
            regs::write_reg(bar0, regs::RDH, 0);
            regs::write_reg(bar0, regs::RDT, (RING_SIZE - 1) as u32);
        }

        ring.head = 0;
        ring.tail = (RING_SIZE - 1) % RING_SIZE;

        // Set receive delay timer for interrupt coalescing.
        unsafe {
            regs::write_reg(bar0, regs::RDTR, 0); // No delay — immediate interrupt.
            regs::write_reg(bar0, regs::RADV, 0); // No absolute delay.
        }

        log::info!("[intel-nic] RX ring configured: RDBAL={:#010x}, RDLEN={}, RDH=0, RDT={}",
            ring_phys as u32, RING_SIZE * 16, RING_SIZE - 1);
    }

    fn enable_rx(bar0: *mut u8) {
        log::debug!("[intel-nic] enabling receiver");

        let rctl = regs::RCTL_EN         // Receiver Enable
            | regs::RCTL_BAM            // Broadcast Accept Mode
            | regs::RCTL_BSIZE_2048     // 2048-byte receive buffers
            | regs::RCTL_SECRC          // Strip CRC
            | regs::RCTL_LBM_NONE;      // No loopback

        unsafe { regs::write_reg(bar0, regs::RCTL, rctl) };
        log::debug!("[intel-nic] RCTL={:#010x}", rctl);
    }

    // =====================================================================
    // TX Ring Setup
    // =====================================================================

    fn setup_tx_ring(bar0: *mut u8, ring: &mut TxRing, virt_to_phys: VirtToPhysFn) {
        log::info!("[intel-nic] setting up TX descriptor ring ({} descriptors)", RING_SIZE);

        // Zero out all TX descriptors.
        for i in 0..RING_SIZE {
            ring.descriptors[i] = TxDescriptor::zeroed();
        }

        // Program the descriptor ring base address.
        let ring_virt = ring.descriptors.as_ptr() as usize;
        let ring_phys = virt_to_phys(ring_virt);
        log::debug!("[intel-nic] TX ring phys={:#x}, size={} bytes",
            ring_phys, RING_SIZE * core::mem::size_of::<TxDescriptor>());

        unsafe {
            regs::write_reg(bar0, regs::TDBAL, ring_phys as u32);
            regs::write_reg(bar0, regs::TDBAH, (ring_phys >> 32) as u32);
            regs::write_reg(bar0, regs::TDLEN, (RING_SIZE * core::mem::size_of::<TxDescriptor>()) as u32);

            // Head = 0, Tail = 0 (ring starts empty).
            regs::write_reg(bar0, regs::TDH, 0);
            regs::write_reg(bar0, regs::TDT, 0);
        }

        ring.head = 0;
        ring.tail = 0;

        // Set inter-packet gap (standard IEEE 802.3 values).
        unsafe { regs::write_reg(bar0, regs::TIPG, regs::TIPG_DEFAULT) };

        // Set transmit interrupt delay.
        unsafe {
            regs::write_reg(bar0, regs::TIDV, 0); // No delay.
            regs::write_reg(bar0, regs::TADV, 0); // No absolute delay.
        }

        log::info!("[intel-nic] TX ring configured: TDBAL={:#010x}, TDLEN={}, TDH=0, TDT=0",
            ring_phys as u32, RING_SIZE * 16);
    }

    fn enable_tx(bar0: *mut u8) {
        log::debug!("[intel-nic] enabling transmitter");

        let tctl = regs::TCTL_EN                           // Transmitter Enable
            | regs::TCTL_PSP                               // Pad Short Packets
            | (15 << regs::TCTL_CT_SHIFT)                  // Collision Threshold = 15
            | regs::TCTL_COLD_FD                           // Full-duplex collision distance
            | regs::TCTL_RTLC;                             // Re-transmit on Late Collision

        unsafe { regs::write_reg(bar0, regs::TCTL, tctl) };
        log::debug!("[intel-nic] TCTL={:#010x}", tctl);
    }

    // =====================================================================
    // Transmit
    // =====================================================================

    /// Transmit an Ethernet frame.
    ///
    /// The frame should be a complete Ethernet frame (dest MAC + src MAC +
    /// ethertype + payload). The NIC will append the CRC.
    ///
    /// Returns `Ok(())` on success or an error if the TX ring is full.
    pub fn transmit(&mut self, frame: &[u8]) -> Result<(), IntelNicError> {
        if frame.len() > BUF_SIZE {
            log::error!("[intel-nic] TX: frame too large ({} > {})", frame.len(), BUF_SIZE);
            return Err(IntelNicError::TxRingFull);
        }

        // Reclaim completed TX descriptors.
        self.reclaim_tx();

        if self.tx_ring.is_full() {
            log::warn!("[intel-nic] TX: ring is full ({} in use)", self.tx_ring.in_use());
            return Err(IntelNicError::TxRingFull);
        }

        let idx = self.tx_ring.tail;

        // Copy the frame into the DMA buffer.
        self.tx_ring.buffers[idx][..frame.len()].copy_from_slice(frame);

        // Set up the descriptor.
        let buf_virt = self.tx_ring.buffers[idx].as_ptr() as usize;
        let buf_phys = (self.virt_to_phys)(buf_virt);

        self.tx_ring.descriptors[idx] = TxDescriptor {
            buffer_addr: buf_phys,
            length: frame.len() as u16,
            cso: 0,
            cmd: TX_CMD_EOP | TX_CMD_IFCS | TX_CMD_RS,
            status: 0,
            css: 0,
            special: 0,
        };

        // Advance tail.
        self.tx_ring.tail = descriptors::DescriptorRing::<TxDescriptor>::wrap_next(idx);

        // Write the new tail to TDT — this kicks the NIC to start transmitting.
        unsafe { regs::write_reg(self.bar0, regs::TDT, self.tx_ring.tail as u32) };

        log::trace!("[intel-nic] TX: queued {} byte frame at desc[{}], phys={:#x}, new_tail={}",
            frame.len(), idx, buf_phys, self.tx_ring.tail);

        Ok(())
    }

    /// Reclaim completed TX descriptors by checking the DD (Descriptor Done) bit.
    fn reclaim_tx(&mut self) {
        let mut reclaimed = 0u32;

        while self.tx_ring.head != self.tx_ring.tail {
            let idx = self.tx_ring.head;
            let status = self.tx_ring.descriptors[idx].status;

            if status & TX_STATUS_DD == 0 {
                break; // This descriptor hasn't been processed yet.
            }

            // Clear the descriptor.
            self.tx_ring.descriptors[idx] = TxDescriptor::zeroed();
            self.tx_ring.head = descriptors::DescriptorRing::<TxDescriptor>::wrap_next(idx);
            reclaimed += 1;
        }

        if reclaimed > 0 {
            log::trace!("[intel-nic] TX: reclaimed {} descriptors, head now {}",
                reclaimed, self.tx_ring.head);
        }
    }

    // =====================================================================
    // Receive
    // =====================================================================

    /// Receive an Ethernet frame.
    ///
    /// Checks the next RX descriptor for the DD (Descriptor Done) bit. If a
    /// frame is available, copies it into `out` and returns `Ok(Some(len))`.
    /// Returns `Ok(None)` if no frame is available.
    pub fn receive(&mut self, out: &mut [u8]) -> Result<Option<usize>, IntelNicError> {
        // The hardware RDH register tracks which descriptor the NIC will write
        // next. We track our own read position. Check the descriptor at
        // (tail + 1) % RING_SIZE — that's the oldest unprocessed descriptor.
        let check_idx = descriptors::DescriptorRing::<RxDescriptor>::wrap_next(self.rx_ring.tail);

        let desc = &self.rx_ring.descriptors[check_idx];

        if desc.status & RX_STATUS_DD == 0 {
            return Ok(None); // No frame available.
        }

        let length = desc.length as usize;
        let is_eop = desc.status & RX_STATUS_EOP != 0;

        if desc.errors != 0 {
            log::warn!("[intel-nic] RX: descriptor[{}] has errors: {:#04x}", check_idx, desc.errors);
        }

        if !is_eop {
            log::warn!("[intel-nic] RX: multi-descriptor frame (not EOP) at desc[{}], dropping",
                check_idx);
            // Recycle and skip.
            self.recycle_rx_desc(check_idx);
            return Ok(None);
        }

        if length > out.len() {
            log::warn!("[intel-nic] RX: frame too large ({} > {}), dropping", length, out.len());
            self.recycle_rx_desc(check_idx);
            return Err(IntelNicError::RxBufferTooSmall);
        }

        // Copy the received frame from the DMA buffer.
        out[..length].copy_from_slice(&self.rx_ring.buffers[check_idx][..length]);

        log::trace!("[intel-nic] RX: received {} byte frame from desc[{}]", length, check_idx);

        // Recycle the descriptor.
        self.recycle_rx_desc(check_idx);

        Ok(Some(length))
    }

    /// Recycle an RX descriptor: reset its buffer pointer, clear status, and
    /// advance the tail (making it available to the NIC again).
    fn recycle_rx_desc(&mut self, idx: usize) {
        let buf_virt = self.rx_ring.buffers[idx].as_ptr() as usize;
        let buf_phys = (self.virt_to_phys)(buf_virt);

        self.rx_ring.descriptors[idx] = RxDescriptor {
            buffer_addr: buf_phys,
            length: 0,
            checksum: 0,
            status: 0,
            errors: 0,
            special: 0,
        };

        // Advance tail — write the new tail to RDT.
        self.rx_ring.tail = idx;
        unsafe { regs::write_reg(self.bar0, regs::RDT, self.rx_ring.tail as u32) };

        log::trace!("[intel-nic] RX: recycled desc[{}], new RDT={}", idx, self.rx_ring.tail);
    }

    // =====================================================================
    // Interrupt Handling
    // =====================================================================

    /// Handle a NIC interrupt.
    ///
    /// Reads the ICR register (which auto-clears on read) and processes the
    /// interrupt cause bits. Returns the raw ICR value for the caller to
    /// decide whether to wake network tasks.
    pub fn handle_interrupt(&mut self) -> u32 {
        let icr = unsafe { regs::read_reg(self.bar0, regs::ICR) };

        if icr == 0 {
            log::trace!("[intel-nic] spurious interrupt (ICR=0)");
            return 0;
        }

        log::debug!("[intel-nic] interrupt: ICR={:#010x}", icr);

        if icr & regs::ICR_LSC != 0 {
            let status = unsafe { regs::read_reg(self.bar0, regs::STATUS) };
            if status & regs::STATUS_LU != 0 {
                log::info!("[intel-nic] link status change: LINK UP");
            } else {
                log::warn!("[intel-nic] link status change: LINK DOWN");
            }
        }

        if icr & regs::ICR_RXT0 != 0 {
            log::trace!("[intel-nic] RX timer interrupt (packet received)");
        }

        if icr & regs::ICR_RXDMT0 != 0 {
            log::debug!("[intel-nic] RX descriptor minimum threshold reached");
        }

        if icr & regs::ICR_RXO != 0 {
            log::warn!("[intel-nic] receiver overrun!");
        }

        if icr & regs::ICR_TXDW != 0 {
            log::trace!("[intel-nic] TX descriptor written back");
            self.reclaim_tx();
        }

        icr
    }

    // =====================================================================
    // Accessors
    // =====================================================================

    /// Return the 6-byte MAC address.
    pub fn mac_address(&self) -> [u8; 6] {
        self.mac
    }

    /// Return the IRQ line.
    pub fn irq(&self) -> u8 {
        self.irq
    }

    /// Return the NIC variant.
    pub fn variant(&self) -> NicVariant {
        self.variant
    }

    /// Check if the link is currently up by reading the STATUS register.
    pub fn link_up(&self) -> bool {
        let status = unsafe { regs::read_reg(self.bar0, regs::STATUS) };
        status & regs::STATUS_LU != 0
    }

    /// Get detailed link status via PHY.
    pub fn link_status(&self) -> Result<crate::phy::LinkStatus, ()> {
        self.phy.link_status()
    }
}
