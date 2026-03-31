//! Global Descriptor Table with Task State Segment.
//!
//! IST stacks:
//!   - IST[0]: Double fault handler (20 KiB)
//!   - IST[1]: Timer interrupt handler (16 KiB) — dedicated stack so IRQ0
//!             works regardless of the kernel stack state

use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;
use x86_64::VirtAddr;
use spin::Lazy;

pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;
pub const TIMER_IST_INDEX: u16 = 1;

const DOUBLE_FAULT_STACK_SIZE: usize = 4096 * 5;  // 20 KiB
const TIMER_STACK_SIZE: usize = 4096 * 4;          // 16 KiB

static TSS: Lazy<TaskStateSegment> = Lazy::new(|| {
    let mut tss = TaskStateSegment::new();

    // IST[0] — double fault handler stack
    tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = {
        static mut STACK: [u8; DOUBLE_FAULT_STACK_SIZE] = [0; DOUBLE_FAULT_STACK_SIZE];
        let stack_start = VirtAddr::from_ptr(&raw const STACK as *const u8);
        stack_start + DOUBLE_FAULT_STACK_SIZE as u64
    };

    // IST[1] — timer interrupt stack (IRQ0)
    // Gives the timer its own stack so it doesn't depend on the kernel stack.
    // This fixes the double fault that occurs when the kernel stack is deep
    // (e.g., inside the executor's BTreeMap operations or log formatting).
    tss.interrupt_stack_table[TIMER_IST_INDEX as usize] = {
        static mut STACK: [u8; TIMER_STACK_SIZE] = [0; TIMER_STACK_SIZE];
        let stack_start = VirtAddr::from_ptr(&raw const STACK as *const u8);
        stack_start + TIMER_STACK_SIZE as u64
    };

    tss
});

static GDT: Lazy<(GlobalDescriptorTable, Selectors)> = Lazy::new(|| {
    let mut gdt = GlobalDescriptorTable::new();
    let code_selector = gdt.append(Descriptor::kernel_code_segment());
    let data_selector = gdt.append(Descriptor::kernel_data_segment());
    let tss_selector = gdt.append(Descriptor::tss_segment(&TSS));
    (gdt, Selectors { code_selector, data_selector, tss_selector })
});

struct Selectors {
    code_selector: SegmentSelector,
    data_selector: SegmentSelector,
    tss_selector: SegmentSelector,
}

pub fn init() {
    use x86_64::instructions::segmentation::{CS, Segment};
    use x86_64::instructions::tables::load_tss;

    // Force TSS initialization and log IST addresses for debugging
    let ist0 = TSS.interrupt_stack_table[0];
    let ist1 = TSS.interrupt_stack_table[1];
    log::info!("[gdt] IST[0] (double fault) top: {:#x}", ist0.as_u64());
    log::info!("[gdt] IST[1] (timer)        top: {:#x}", ist1.as_u64());

    GDT.0.load();
    unsafe {
        CS::set_reg(GDT.1.code_selector);
        // Load data segment registers — needed for interrupt frame SS field
        use x86_64::instructions::segmentation::{DS, ES, SS, Segment};
        DS::set_reg(GDT.1.data_selector);
        ES::set_reg(GDT.1.data_selector);
        SS::set_reg(GDT.1.data_selector);
        load_tss(GDT.1.tss_selector);
    }
}
