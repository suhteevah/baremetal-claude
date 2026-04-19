//! RTL8168 driver — init + MMIO helpers. TX/RX are stubs (Phase 2).

extern crate alloc;
use alloc::boxed::Box;
use alloc::vec::Vec;

use crate::descriptors::{
    alloc_buffers, alloc_ring, Descriptor, VirtToPhysFn,
    BUF_SIZE, EOR, OWN, RING_SIZE,
};
use crate::regs;

#[inline(always)]
unsafe fn r8(base: *const u8, off: u32) -> u8 {
    unsafe { core::ptr::read_volatile(base.add(off as usize)) }
}
#[inline(always)]
unsafe fn r16(base: *const u8, off: u32) -> u16 {
    unsafe { core::ptr::read_volatile(base.add(off as usize) as *const u16) }
}
#[inline(always)]
unsafe fn r32(base: *const u8, off: u32) -> u32 {
    unsafe { core::ptr::read_volatile(base.add(off as usize) as *const u32) }
}
#[inline(always)]
unsafe fn w8(base: *mut u8, off: u32, v: u8) {
    unsafe { core::ptr::write_volatile(base.add(off as usize), v) }
}
#[inline(always)]
unsafe fn w16(base: *mut u8, off: u32, v: u16) {
    unsafe { core::ptr::write_volatile(base.add(off as usize) as *mut u16, v) }
}
#[inline(always)]
unsafe fn w32(base: *mut u8, off: u32, v: u32) {
    unsafe { core::ptr::write_volatile(base.add(off as usize) as *mut u32, v) }
}

#[derive(Debug, Clone, Copy)]
pub struct DiagRegs {
    pub cr: u8,
    pub isr: u16,
    pub imr: u16,
    pub phy_status: u8,
    pub rxcr: u32,
    pub txcr: u32,
    pub cplus_cmd: u16,
    pub max_tx_pkt: u8,
    pub tx_tail: u16,
    pub rx_head: u16,
    pub rx_head_opts1: u32,
    pub tx_head_opts1: u32,
}

#[derive(Debug)]
pub enum Rtl8168InitError {
    NullBase,
    ResetTimeout,
    AllocFailed,
}

impl core::fmt::Display for Rtl8168InitError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NullBase     => write!(f, "BAR2 MMIO base is null"),
            Self::ResetTimeout => write!(f, "reset timeout (CR.RST stuck)"),
            Self::AllocFailed  => write!(f, "descriptor ring allocation failed"),
        }
    }
}

pub struct Rtl8168 {
    mmio:         *mut u8,
    mac:          [u8; 6],
    rx_ring:      Box<[Descriptor; RING_SIZE]>,
    tx_ring:      Box<[Descriptor; RING_SIZE]>,
    rx_bufs:      Vec<Box<[u8; BUF_SIZE]>>,
    tx_bufs:      Vec<Box<[u8; BUF_SIZE]>>,
    rx_head:      usize,
    tx_tail:      usize,
    virt_to_phys: VirtToPhysFn,
}

unsafe impl Send for Rtl8168 {}

impl Rtl8168 {
    pub unsafe fn init(
        mmio_virt: u64,
        _irq: u8,
        virt_to_phys: VirtToPhysFn,
    ) -> Result<Self, Rtl8168InitError> {
        log::info!("[rtl8168] init at MMIO virt={:#x}", mmio_virt);
        if mmio_virt == 0 { return Err(Rtl8168InitError::NullBase); }
        let mmio = mmio_virt as *mut u8;

        log::debug!("[rtl8168] software reset");
        unsafe { w8(mmio, regs::CR, regs::CR_RST) };
        let mut ok = false;
        for i in 0..200_000u32 {
            if unsafe { r8(mmio, regs::CR) } & regs::CR_RST == 0 {
                log::debug!("[rtl8168] reset done after {} polls", i);
                ok = true; break;
            }
        }
        if !ok { return Err(Rtl8168InitError::ResetTimeout); }

        let w0 = unsafe { r32(mmio, regs::IDR0) };
        let w4 = unsafe { r32(mmio, regs::IDR4) };
        let mac = [
            (w0 & 0xFF) as u8, ((w0 >> 8) & 0xFF) as u8,
            ((w0 >> 16) & 0xFF) as u8, ((w0 >> 24) & 0xFF) as u8,
            (w4 & 0xFF) as u8, ((w4 >> 8) & 0xFF) as u8,
        ];
        log::info!("[rtl8168] MAC {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            mac[0],mac[1],mac[2],mac[3],mac[4],mac[5]);

        unsafe { w8(mmio, regs::CR9346, regs::CR9346_UNLOCK) };

        // Re-write MAC to IDR0/IDR4 — some rtl8168 variants lose the MAC after
        // software reset and APM/unicast filter won't match without it. With
        // promiscuous mode this is belt-and-suspenders, but it's free.
        let mac_lo = (mac[0] as u32)
            | ((mac[1] as u32) << 8)
            | ((mac[2] as u32) << 16)
            | ((mac[3] as u32) << 24);
        let mac_hi = (mac[4] as u32) | ((mac[5] as u32) << 8);
        unsafe {
            w32(mmio, regs::IDR0, mac_lo);
            w32(mmio, regs::IDR4, mac_hi);
        }
        log::debug!("[rtl8168] MAC re-written to IDR0/IDR4");

        // Clear CPlusCmd — Linux r8169 does this before any TX/RX setup.
        // Leaving it at reset default has caused silent TX corruption in the
        // wild. We'll later OR in checksum/VLAN bits once the basic path works.
        unsafe { w16(mmio, regs::CPLUS_CMD, 0x0000) };
        log::debug!("[rtl8168] CPlusCmd cleared");

        // Set max TX packet size (unit: 128 bytes). 0x3F = 8064 bytes ceiling.
        unsafe { w8(mmio, regs::MAX_TX_PKT_SIZE, 0x3F) };
        log::debug!("[rtl8168] MaxTxPacketSize set to 0x3F (8064 bytes)");

        let (mut rx_ring, rx_phys) = alloc_ring(virt_to_phys).ok_or(Rtl8168InitError::AllocFailed)?;
        let rx_bufs = alloc_buffers();
        let (mut tx_ring, tx_phys) = alloc_ring(virt_to_phys).ok_or(Rtl8168InitError::AllocFailed)?;
        let tx_bufs = alloc_buffers();

        for i in 0..RING_SIZE {
            let phys = virt_to_phys(rx_bufs[i].as_ptr() as usize);
            rx_ring[i].set_addr(phys);
            let mut opts1 = OWN | (BUF_SIZE as u32 & 0x3FFF);
            if i == RING_SIZE - 1 { opts1 |= EOR; }
            core::sync::atomic::fence(core::sync::atomic::Ordering::Release);
            rx_ring[i].opts1 = opts1;
        }
        tx_ring[RING_SIZE - 1].opts1 = EOR;

        log::debug!("[rtl8168] RDSAR={:#x} TNPDS={:#x}", rx_phys, tx_phys);
        unsafe {
            w32(mmio, regs::RDSAR_LOW,  (rx_phys & 0xFFFF_FFFF) as u32);
            w32(mmio, regs::RDSAR_HIGH, (rx_phys >> 32) as u32);
            w32(mmio, regs::TNPDS_LOW,  (tx_phys & 0xFFFF_FFFF) as u32);
            w32(mmio, regs::TNPDS_HIGH, (tx_phys >> 32) as u32);
            w32(mmio, regs::THPDS_LOW,  0);
            w32(mmio, regs::THPDS_HIGH, 0);
        }

        unsafe {
            w32(mmio, regs::RXCR, regs::RXCR_DEFAULT);
            w32(mmio, regs::TXCR, regs::TXCR_DEFAULT);
            w16(mmio, regs::RXMPS, regs::RXMPS_DEFAULT);
        }

        unsafe { w8(mmio, regs::CR, regs::CR_RE | regs::CR_TE) };
        log::info!("[rtl8168] RE+TE enabled");

        unsafe { w16(mmio, regs::IMR,
            regs::INT_ROK | regs::INT_TOK | regs::INT_RER | regs::INT_TER | regs::INT_LINK) };

        unsafe { w8(mmio, regs::CR9346, regs::CR9346_LOCK) };
        log::info!("[rtl8168] init complete");

        Ok(Self { mmio, mac, rx_ring, tx_ring, rx_bufs, tx_bufs,
                  rx_head: 0, tx_tail: 0, virt_to_phys })
    }

    pub fn mac_address(&self) -> [u8; 6] { self.mac }

    /// Snapshot of key registers for diagnostics. Read-only; safe at any time.
    pub fn diag_regs(&self) -> DiagRegs {
        unsafe {
            DiagRegs {
                cr:          r8(self.mmio, regs::CR),
                isr:         r16(self.mmio, regs::ISR),
                imr:         r16(self.mmio, regs::IMR),
                phy_status:  r8(self.mmio, regs::PHY_STATUS),
                rxcr:        r32(self.mmio, regs::RXCR),
                txcr:        r32(self.mmio, regs::TXCR),
                cplus_cmd:   r16(self.mmio, regs::CPLUS_CMD),
                max_tx_pkt:  r8(self.mmio, regs::MAX_TX_PKT_SIZE),
                tx_tail:     self.tx_tail as u16,
                rx_head:     self.rx_head as u16,
                rx_head_opts1: self.rx_ring[self.rx_head].opts1,
                tx_head_opts1: self.tx_ring[self.tx_tail].opts1,
            }
        }
    }

    pub fn ack_interrupt(&self) -> u16 {
        let isr = unsafe { r16(self.mmio as *const u8, regs::ISR) };
        if isr != 0 { unsafe { w16(self.mmio, regs::ISR, isr) }; }
        isr
    }

    /// Transmit a single Ethernet frame.
    ///
    /// Copies `frame` into the next TX descriptor's DMA buffer, sets
    /// OWN|FS|LS|len (preserving EOR on the last descriptor), and kicks
    /// the NIC via TPPOLL.NPQ. Returns `Err` if the ring is full.
    pub fn transmit(&mut self, frame: &[u8]) -> Result<(), &'static str> {
        if frame.len() > BUF_SIZE {
            log::warn!("[rtl8168] TX frame too big ({} > {})", frame.len(), BUF_SIZE);
            return Err("rtl8168: frame exceeds buffer size");
        }

        let idx = self.tx_tail;
        let desc_opts1 = self.tx_ring[idx].opts1;

        // If OWN=1, NIC still owns this descriptor → ring full.
        if desc_opts1 & OWN != 0 {
            log::trace!("[rtl8168] TX ring full at idx={}", idx);
            return Err("rtl8168: TX ring full");
        }

        // Copy frame into DMA buffer.
        self.tx_bufs[idx][..frame.len()].copy_from_slice(frame);

        // Set buffer physical address (re-set every time in case allocator moved — safe).
        let buf_phys = (self.virt_to_phys)(self.tx_bufs[idx].as_ptr() as usize);
        self.tx_ring[idx].set_addr(buf_phys);

        // Compose opts1: OWN | FS | LS | len, preserving EOR on last entry.
        let eor = if idx == RING_SIZE - 1 { EOR } else { 0 };
        let opts1 = OWN | crate::descriptors::TX_FS | crate::descriptors::TX_LS
                  | eor | (frame.len() as u32 & 0x3FFF);

        // Release fence: ensure buffer + addr writes are visible before OWN is set.
        core::sync::atomic::fence(core::sync::atomic::Ordering::Release);
        self.tx_ring[idx].opts1 = opts1;

        // Kick the NIC to process normal-priority TX queue.
        unsafe { w8(self.mmio, regs::TPPOLL, regs::TPPOLL_NPQ) };

        // Advance tx_tail with wrap.
        self.tx_tail = (idx + 1) % RING_SIZE;

        log::trace!("[rtl8168] TX idx={} len={} kicked", idx, frame.len());
        Ok(())
    }

    /// Receive a single Ethernet frame into `out`.
    ///
    /// Returns `Ok(Some(len))` if a frame was delivered, `Ok(None)` if the
    /// ring is empty (NIC still owns the head descriptor), or `Err` on
    /// descriptor-reported error (CRC, runt, etc.).
    pub fn receive(&mut self, out: &mut [u8]) -> Result<Option<usize>, &'static str> {
        let idx = self.rx_head;
        let opts1 = self.rx_ring[idx].opts1;

        // If OWN=1, NIC hasn't delivered a packet here yet.
        if opts1 & OWN != 0 {
            return Ok(None);
        }

        // Acquire fence: ensure we see buffer contents written before OWN was cleared.
        core::sync::atomic::fence(core::sync::atomic::Ordering::Acquire);

        // Error bits in opts1 (rtl8169 spec): bit 21 RES, 20 RUNT, 19 CRC.
        const RX_RES:  u32 = 1 << 21;
        const RX_RUNT: u32 = 1 << 20;
        const RX_CRC:  u32 = 1 << 19;
        let had_error = opts1 & (RX_RES | RX_RUNT | RX_CRC) != 0;

        // Length is low 14 bits; includes 4-byte FCS which we strip.
        let raw_len = (opts1 & 0x3FFF) as usize;
        let len = raw_len.saturating_sub(4);

        // Recycle descriptor before returning (whether or not error).
        let eor = if idx == RING_SIZE - 1 { EOR } else { 0 };
        // Reset buf addr in case it was clobbered (paranoia; NIC shouldn't touch it).
        let buf_phys = (self.virt_to_phys)(self.rx_bufs[idx].as_ptr() as usize);
        self.rx_ring[idx].set_addr(buf_phys);
        core::sync::atomic::fence(core::sync::atomic::Ordering::Release);
        self.rx_ring[idx].opts1 = OWN | eor | (BUF_SIZE as u32 & 0x3FFF);

        self.rx_head = (idx + 1) % RING_SIZE;

        if had_error {
            log::warn!("[rtl8168] RX error at idx={} opts1={:#010x}", idx, opts1);
            return Err("rtl8168: RX descriptor error");
        }

        if len == 0 || len > out.len() {
            log::warn!("[rtl8168] RX bad length: raw={} usable={} out_cap={}", raw_len, len, out.len());
            return Err("rtl8168: RX length out of range");
        }

        // Copy payload into caller's buffer.
        out[..len].copy_from_slice(&self.rx_bufs[idx][..len]);
        log::trace!("[rtl8168] RX idx={} len={}", idx, len);
        Ok(Some(len))
    }
}
