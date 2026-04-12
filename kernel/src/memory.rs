//! Physical frame allocator + kernel heap.
//!
//! Uses the UEFI memory map to find usable frames, then maps a heap region
//! and initializes linked_list_allocator as the global allocator.

use bootloader_api::info::{MemoryRegionKind, MemoryRegions};
use core::sync::atomic::{AtomicBool, Ordering};
use linked_list_allocator::LockedHeap;
use x86_64::structures::paging::{
    FrameAllocator, Mapper, OffsetPageTable, Page, PageTable, PageTableFlags, PhysFrame, Size4KiB,
};
use x86_64::{PhysAddr, VirtAddr};

/// Kernel heap: 64 MiB — needs room for VirtIO queues, smoltcp buffers,
/// log formatting, BTreeMap allocations, double-buffered framebuffer
/// (2560x1600x4x2 = 32 MiB at high res), and the 4 MiB kernel stack.
pub const HEAP_START: usize = 0x_4444_4444_0000;
pub const HEAP_SIZE: usize = 48 * 1024 * 1024; // 48 MiB (double-buffered 2560x1600 = ~32 MiB + kernel stack + buffers)

/// Set to `true` once the heap allocator has been initialized.
/// Logger checks this to avoid `format!` allocations before the heap exists.
pub static HEAP_READY: AtomicBool = AtomicBool::new(false);

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

/// Return (used, total) heap bytes from the allocator.
pub fn heap_stats() -> (usize, usize) {
    let allocator = ALLOCATOR.lock();
    let free = allocator.free();
    let used = HEAP_SIZE - free;
    (used, HEAP_SIZE)
}

/// Initialize the heap: map virtual pages at HEAP_START and init the allocator.
pub fn init(phys_mem_offset: u64, memory_regions: &'static MemoryRegions) {
    let phys_mem_offset = VirtAddr::new(phys_mem_offset);

    // Create OffsetPageTable from the active level 4 page table
    let mut mapper = unsafe {
        let level_4_table = active_level_4_table(phys_mem_offset);
        OffsetPageTable::new(level_4_table, phys_mem_offset)
    };

    let mut frame_allocator = unsafe { BootInfoFrameAllocator::init(memory_regions) };

    // Map heap pages
    let heap_start = VirtAddr::new(HEAP_START as u64);
    let heap_end = heap_start + HEAP_SIZE as u64 - 1u64;
    let heap_start_page: Page<Size4KiB> = Page::containing_address(heap_start);
    let heap_end_page: Page<Size4KiB> = Page::containing_address(heap_end);

    for page in Page::range_inclusive(heap_start_page, heap_end_page) {
        let frame = frame_allocator
            .allocate_frame()
            .expect("out of physical memory for heap");
        let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;
        unsafe {
            mapper
                .map_to(page, frame, flags, &mut frame_allocator)
                .expect("heap page mapping failed")
                .flush();
        }
    }

    unsafe {
        ALLOCATOR.lock().init(HEAP_START as *mut u8, HEAP_SIZE);
    }
    HEAP_READY.store(true, Ordering::Release);

    log::info!(
        "[mem] heap initialized at {:#x}, size {} KiB",
        HEAP_START,
        HEAP_SIZE / 1024
    );
}

/// Get a mutable reference to the active level 4 page table.
///
/// # Safety
/// The caller must ensure that the complete physical memory is mapped at the
/// given `phys_mem_offset`. Also, this function must only be called once to
/// avoid aliasing `&mut` references (which is undefined behavior).
unsafe fn active_level_4_table(phys_mem_offset: VirtAddr) -> &'static mut PageTable {
    use x86_64::registers::control::Cr3;

    let (level_4_table_frame, _) = Cr3::read();
    let phys = level_4_table_frame.start_address();
    let virt = phys_mem_offset + phys.as_u64();
    let page_table_ptr: *mut PageTable = virt.as_mut_ptr();
    unsafe { &mut *page_table_ptr }
}

/// A frame allocator that returns usable frames from the bootloader's memory map.
pub struct BootInfoFrameAllocator {
    memory_regions: &'static MemoryRegions,
    next: usize,
}

impl BootInfoFrameAllocator {
    /// Create a new allocator from the bootloader memory regions.
    ///
    /// # Safety
    /// The caller must guarantee that the memory regions are valid and that
    /// usable frames are not already in use.
    pub unsafe fn init(memory_regions: &'static MemoryRegions) -> Self {
        Self {
            memory_regions,
            next: 0,
        }
    }

    /// Returns an iterator over usable physical frames.
    fn usable_frames(&self) -> impl Iterator<Item = PhysFrame> + '_ {
        self.memory_regions
            .iter()
            .filter(|r| r.kind == MemoryRegionKind::Usable)
            .map(|r| r.start..r.end)
            .flat_map(|r| r.step_by(4096))
            .map(|addr| PhysFrame::containing_address(PhysAddr::new(addr)))
    }
}

unsafe impl FrameAllocator<Size4KiB> for BootInfoFrameAllocator {
    fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        let frame = self.usable_frames().nth(self.next);
        self.next += 1;
        frame
    }
}

/// Translate a virtual address to its physical address by walking the CR3
/// page tables.
///
/// This is used by DMA-capable drivers (AHCI, NVMe) to convert heap virtual
/// addresses into physical addresses the hardware can DMA to/from. Heap pages
/// are NOT identity-mapped — they live at `0x4444_4444_0000+` with physical
/// frames assigned by `BootInfoFrameAllocator`.
///
/// Panics if the address is not mapped (unmapped pages are a kernel bug).
pub fn virt_to_phys(virt_addr: usize) -> u64 {
    let phys_mem_offset = crate::PHYS_MEM_OFFSET.load(Ordering::Relaxed);
    let virt = VirtAddr::new(virt_addr as u64);
    let offset_virt = VirtAddr::new(phys_mem_offset);

    // Read CR3 to get the level-4 page table physical address.
    let (l4_frame, _) = x86_64::registers::control::Cr3::read();
    let l4_phys = l4_frame.start_address();
    let l4_virt = offset_virt + l4_phys.as_u64();
    let l4_table = unsafe {
        &*(l4_virt.as_ptr() as *const PageTable)
    };

    // Level 4
    let l4_entry = &l4_table[virt.p4_index()];
    if l4_entry.is_unused() {
        panic!("[memory] virt_to_phys: L4 entry unused for {:#x}", virt_addr);
    }

    let l3_virt = offset_virt + l4_entry.addr().as_u64();
    let l3_table = unsafe {
        &*(l3_virt.as_ptr() as *const PageTable)
    };

    // Level 3
    let l3_entry = &l3_table[virt.p3_index()];
    if l3_entry.is_unused() {
        panic!("[memory] virt_to_phys: L3 entry unused for {:#x}", virt_addr);
    }
    if l3_entry.flags().contains(PageTableFlags::HUGE_PAGE) {
        let base = l3_entry.addr().as_u64();
        return base + (virt_addr as u64 & 0x3FFF_FFFF);
    }

    let l2_virt = offset_virt + l3_entry.addr().as_u64();
    let l2_table = unsafe {
        &*(l2_virt.as_ptr() as *const PageTable)
    };

    // Level 2
    let l2_entry = &l2_table[virt.p2_index()];
    if l2_entry.is_unused() {
        panic!("[memory] virt_to_phys: L2 entry unused for {:#x}", virt_addr);
    }
    if l2_entry.flags().contains(PageTableFlags::HUGE_PAGE) {
        let base = l2_entry.addr().as_u64();
        return base + (virt_addr as u64 & 0x1F_FFFF);
    }

    let l1_virt = offset_virt + l2_entry.addr().as_u64();
    let l1_table = unsafe {
        &*(l1_virt.as_ptr() as *const PageTable)
    };

    // Level 1
    let l1_entry = &l1_table[virt.p1_index()];
    if l1_entry.is_unused() {
        panic!("[memory] virt_to_phys: L1 entry unused for {:#x}", virt_addr);
    }
    let frame_phys = l1_entry.addr().as_u64();
    frame_phys + (virt_addr as u64 & 0xFFF)
}
