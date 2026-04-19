//! RTL8168 DMA descriptor rings — 16-byte descriptors, 256-entry, 256-byte aligned.

extern crate alloc;
use alloc::boxed::Box;
use alloc::vec::Vec;

pub const RING_SIZE: usize = 256;
pub const BUF_SIZE:  usize = 2048;

pub const OWN:   u32 = 1 << 31;
pub const EOR:   u32 = 1 << 30;
pub const TX_FS: u32 = 1 << 29;
pub const TX_LS: u32 = 1 << 28;
pub const RX_FS: u32 = 1 << 29;
pub const RX_LS: u32 = 1 << 28;

pub type VirtToPhysFn = fn(usize) -> u64;

#[repr(C, align(16))]
#[derive(Clone, Copy, Default, Debug)]
pub struct Descriptor {
    pub opts1: u32,
    pub opts2: u32,
    pub buf_lo: u32,
    pub buf_hi: u32,
}

impl Descriptor {
    #[inline]
    pub fn set_addr(&mut self, phys: u64) {
        self.buf_lo = phys as u32;
        self.buf_hi = (phys >> 32) as u32;
    }
}

pub fn alloc_ring(virt_to_phys: VirtToPhysFn) -> Option<(Box<[Descriptor; RING_SIZE]>, u64)> {
    let layout = alloc::alloc::Layout::from_size_align(
        core::mem::size_of::<Descriptor>() * RING_SIZE,
        256,
    ).ok()?;
    let ptr = unsafe { alloc::alloc::alloc_zeroed(layout) } as *mut [Descriptor; RING_SIZE];
    if ptr.is_null() {
        log::error!("[rtl8168] alloc_ring: OOM");
        return None;
    }
    let phys = virt_to_phys(ptr as usize);
    let boxed = unsafe { Box::from_raw(ptr) };
    Some((boxed, phys))
}

pub fn alloc_buffers() -> Vec<Box<[u8; BUF_SIZE]>> {
    (0..RING_SIZE).map(|_| Box::new([0u8; BUF_SIZE])).collect()
}
