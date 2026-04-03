//! AP (Application Processor) boot trampoline for ClaudioOS.
//!
//! The trampoline is a small block of real-mode code that must reside at a
//! physical address below 1 MiB (the STARTUP IPI vector field is only 8 bits,
//! encoding a 4 KiB page number). The AP begins execution in 16-bit real mode
//! at `trampoline_page << 12`, transitions through protected mode to long mode,
//! and finally jumps to the Rust AP entry point.
//!
//! ## Mode Transitions
//!
//! The AP wakes up in 16-bit real mode with no GDT, no paging, and no stack.
//! The trampoline code performs three mode transitions:
//!
//! 1. **Real mode -> Protected mode**: Load a minimal GDT, set CR0.PE, far jump
//!    to flush the pipeline and enter 32-bit code.
//!
//! 2. **Protected mode -> Long mode**: Enable PAE (CR4.PAE), load CR3 with PML4
//!    from TrampolineData, enable long mode (IA32_EFER.LME), then enable paging
//!    (CR0.PG) which activates IA-32e mode.
//!
//! 3. **32-bit compatibility -> 64-bit mode**: Far jump to a 64-bit code segment,
//!    load 64-bit data segments, set up the stack from TrampolineData, signal
//!    AP ready, and call the Rust entry point.
//!
//! ## Memory Layout at the trampoline page (e.g., physical 0x8000)
//!
//! ```text
//! +0x000  Trampoline code (real -> protected -> long mode, ~150 bytes)
//! +0x080  GDT descriptor (6 bytes: limit + base)
//! +0x088  GDT data (5 entries x 8 bytes = 40 bytes)
//! +0xF00  TrampolineData struct (shared BSP <-> AP data: PML4, stack, entry, etc.)
//! ```

use core::ptr;
use core::sync::atomic::{AtomicU32, Ordering};

/// Physical page number where the trampoline will be placed.
/// 0x08 => physical address 0x8000.
pub const TRAMPOLINE_PAGE: u8 = 0x08;

/// Physical address of the trampoline code.
pub const TRAMPOLINE_PHYS: u64 = (TRAMPOLINE_PAGE as u64) << 12;

/// Offset within the trampoline page where `TrampolineData` is placed.
pub const TRAMPOLINE_DATA_OFFSET: u64 = 0xF00;

/// Physical address of the trampoline data block.
pub const TRAMPOLINE_DATA_PHYS: u64 = TRAMPOLINE_PHYS + TRAMPOLINE_DATA_OFFSET;

// ---------------------------------------------------------------------------
// Shared data between BSP and AP
// ---------------------------------------------------------------------------

/// Data block shared between BSP and AP during AP boot.
///
/// The BSP writes this before sending INIT+SIPI. The AP reads it from a
/// known physical address to obtain its stack, page table, and entry point.
#[repr(C, align(16))]
pub struct TrampolineData {
    /// CR3 value — physical address of PML4 page table.
    pub pml4_addr: u64,
    /// Stack pointer for the AP (top of its allocated stack).
    pub stack_top: u64,
    /// 64-bit entry point function address the AP will call.
    pub entry_point: u64,
    /// CPU index assigned to this AP (set by BSP before each SIPI).
    pub cpu_index: u32,
    /// AP sets this to 1 when it has reached long mode and is running.
    pub ap_ready: AtomicU32,
    /// GDT pointer (limit:16 + base:64) for the AP to load.
    pub gdt_limit: u16,
    pub _pad: u16,
    pub gdt_base: u64,
}

impl TrampolineData {
    /// Zero-initialize the trampoline data.
    pub const fn zeroed() -> Self {
        Self {
            pml4_addr: 0,
            stack_top: 0,
            entry_point: 0,
            cpu_index: 0,
            ap_ready: AtomicU32::new(0),
            gdt_limit: 0,
            _pad: 0,
            gdt_base: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// Trampoline code (hand-assembled x86 machine code)
// ---------------------------------------------------------------------------

/// Hand-assembled AP trampoline code.
///
/// This code runs at physical address `TRAMPOLINE_PHYS` in 16-bit real mode.
/// It performs:
///   1. Disable interrupts (cli)
///   2. Load a minimal GDT for 32-bit protected mode
///   3. Set CR0.PE to enter protected mode
///   4. Far jump to flush pipeline and enter 32-bit code
///   5. Set up 32-bit segments
///   6. Enable PAE (CR4.PAE = bit 5)
///   7. Load CR3 with PML4 from TrampolineData
///   8. Enable long mode (IA32_EFER.LME = bit 8, MSR 0xC0000080)
///   9. Enable paging (CR0.PG = bit 31) — enters IA-32e mode
///  10. Far jump to 64-bit code segment
///  11. Load stack pointer from TrampolineData
///  12. Signal AP ready
///  13. Call the Rust entry point
///
/// The code is position-dependent on `TRAMPOLINE_PHYS`.
/// TrampolineData is at `TRAMPOLINE_PHYS + 0xF00`.
///
/// Machine code generated for TRAMPOLINE_PHYS = 0x8000:
pub const TRAMPOLINE_CODE: &[u8] = &[
    // =======================================================================
    // 16-bit real mode (org 0x8000, CS:IP = 0x0800:0x0000)
    // =======================================================================
    // 0x00: cli
    0xFA,
    // 0x01: xor ax, ax
    0x31, 0xC0,
    // 0x03: mov ds, ax
    0x8E, 0xD8,
    // 0x05: mov es, ax
    0x8E, 0xC0,
    // 0x07: mov ss, ax
    0x8E, 0xD0,
    //
    // Load GDT for protected mode transition (GDT is embedded at offset 0x80)
    // 0x09: lgdt [0x8080]  (absolute address of GDT descriptor)
    0x0F, 0x01, 0x16, 0x80, 0x80,
    //
    // Enable protected mode: set CR0.PE (bit 0)
    // 0x0E: mov eax, cr0
    0x0F, 0x20, 0xC0,
    // 0x11: or al, 1
    0x0C, 0x01,
    // 0x13: mov cr0, eax
    0x0F, 0x22, 0xC0,
    //
    // Far jump to 32-bit protected mode code at offset 0x20
    // 0x16: jmp 0x08:0x8020  (code32 selector 0x08, abs address)
    0x66, 0xEA, 0x20, 0x80, 0x00, 0x00, 0x08, 0x00,
    //
    // Padding to offset 0x20
    0x90, 0x90,
    //
    // =======================================================================
    // 32-bit protected mode (offset 0x20 from page base)
    // =======================================================================
    // 0x20: mov ax, 0x10  (data segment selector)
    0x66, 0xB8, 0x10, 0x00,
    // 0x24: mov ds, ax
    0x8E, 0xD8,
    // 0x26: mov es, ax
    0x8E, 0xC0,
    // 0x28: mov fs, ax
    0x8E, 0xE0,
    // 0x2A: mov gs, ax
    0x8E, 0xE8,
    // 0x2C: mov ss, ax
    0x8E, 0xD0,
    //
    // Enable PAE (CR4.PAE = bit 5)
    // 0x2E: mov eax, cr4
    0x0F, 0x20, 0xE0,
    // 0x31: or eax, 0x20
    0x83, 0xC8, 0x20,
    // 0x34: mov cr4, eax
    0x0F, 0x22, 0xE0,
    //
    // Load CR3 with PML4 address from TrampolineData (offset +0x00 at 0x8F00)
    // 0x37: mov eax, [0x8F00]
    0xA1, 0x00, 0x8F, 0x00, 0x00,
    // 0x3C: mov cr3, eax
    0x0F, 0x22, 0xD8,
    //
    // Enable long mode: set IA32_EFER.LME (bit 8)
    // 0x3F: mov ecx, 0xC0000080  (IA32_EFER MSR)
    0xB9, 0x80, 0x00, 0x00, 0xC0,
    // 0x44: rdmsr
    0x0F, 0x32,
    // 0x46: or eax, 0x100  (LME bit)
    0x0D, 0x00, 0x01, 0x00, 0x00,
    // 0x4B: wrmsr
    0x0F, 0x30,
    //
    // Enable paging: set CR0.PG (bit 31) — activates IA-32e mode
    // 0x4D: mov eax, cr0
    0x0F, 0x20, 0xC0,
    // 0x50: or eax, 0x80000000
    0x0D, 0x00, 0x00, 0x00, 0x80,
    // 0x55: mov cr0, eax
    0x0F, 0x22, 0xC0,
    //
    // Far jump to 64-bit long mode code at offset 0x60
    // 0x58: jmp 0x18:0x8060  (code64 selector 0x18, abs address)
    0xEA, 0x60, 0x80, 0x00, 0x00, 0x18, 0x00,
    //
    // Padding to offset 0x60
    0x90,
    //
    // =======================================================================
    // 64-bit long mode (offset 0x60 from page base)
    // NOTE: from here all instructions are 64-bit encoded
    // =======================================================================
    // 0x60: mov ax, 0x20  (64-bit data segment selector)
    0x66, 0xB8, 0x20, 0x00,
    // 0x64: mov ds, ax
    0x8E, 0xD8,
    // 0x66: mov es, ax
    0x8E, 0xC0,
    // 0x68: mov ss, ax
    0x8E, 0xD0,
    // 0x6A: xor ax, ax
    0x66, 0x31, 0xC0,
    // 0x6D: mov fs, ax
    0x8E, 0xE0,
    // 0x6F: mov gs, ax
    0x8E, 0xE8,
    //
    // Load stack pointer from TrampolineData.stack_top (offset +0x08 at 0x8F08)
    // 0x71: mov rsp, [0x8F08]  (REX.W + mov rsp, [disp32])
    0x48, 0x8B, 0x24, 0x25, 0x08, 0x8F, 0x00, 0x00,
    //
    // Signal AP ready: write 1 to TrampolineData.ap_ready (offset +0x18 at 0x8F18)
    // 0x79: mov dword [0x8F18], 1
    0xC7, 0x04, 0x25, 0x18, 0x8F, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00,
    //
    // Load entry point from TrampolineData.entry_point (offset +0x10 at 0x8F10)
    // and CPU index from TrampolineData.cpu_index (offset +0x14 at 0x8F14)
    // 0x84: mov rdi, [0x8F14]  (first arg = cpu_index, zero-extended)
    0x48, 0x8B, 0x3C, 0x25, 0x14, 0x8F, 0x00, 0x00,
    // 0x8C: mov rax, [0x8F10]  (entry point address)
    0x48, 0x8B, 0x04, 0x25, 0x10, 0x8F, 0x00, 0x00,
    //
    // Call the Rust AP entry point: ap_entry(cpu_index: u64)
    // 0x94: call rax
    0xFF, 0xD0,
    //
    // If entry returns, halt
    // 0x96: cli; hlt; jmp $-2
    0xFA, 0xF4, 0xEB, 0xFD,
];

/// Minimal GDT for the trampoline (placed at offset 0x80 within the page).
///
/// Layout:
///   0x00: Null descriptor
///   0x08: 32-bit code segment (selector 0x08)
///   0x10: 32-bit data segment (selector 0x10)
///   0x18: 64-bit code segment (selector 0x18)
///   0x20: 64-bit data segment (selector 0x20)
pub const TRAMPOLINE_GDT: &[u8] = &[
    // Null descriptor
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    // 32-bit code: base=0, limit=0xFFFFF, type=0x9A (exec/read), granularity=4K, 32-bit
    0xFF, 0xFF, 0x00, 0x00, 0x00, 0x9A, 0xCF, 0x00,
    // 32-bit data: base=0, limit=0xFFFFF, type=0x92 (read/write), granularity=4K, 32-bit
    0xFF, 0xFF, 0x00, 0x00, 0x00, 0x92, 0xCF, 0x00,
    // 64-bit code: base=0, limit=0, type=0x9A, L=1 (long mode), D=0
    0x00, 0x00, 0x00, 0x00, 0x00, 0x9A, 0x20, 0x00,
    // 64-bit data: base=0, limit=0, type=0x92
    0x00, 0x00, 0x00, 0x00, 0x00, 0x92, 0x00, 0x00,
];

/// GDT descriptor (limit + base) for LGDT instruction, targeting the GDT
/// at `TRAMPOLINE_PHYS + 0x88` (the GDT data follows the 6-byte descriptor).
pub const TRAMPOLINE_GDT_DESC: &[u8] = &[
    // limit: 5 entries * 8 bytes - 1 = 39 = 0x27
    0x27, 0x00,
    // base: 0x00008088 (32-bit, within real-mode addressable range)
    0x88, 0x80, 0x00, 0x00,
];

// ---------------------------------------------------------------------------
// AP Trampoline manager
// ---------------------------------------------------------------------------

/// Manages the AP trampoline: copies code to low memory, configures per-AP data.
pub struct ApTrampoline {
    /// Virtual address corresponding to `TRAMPOLINE_PHYS` (identity-mapped).
    trampoline_vaddr: u64,
}

impl ApTrampoline {
    /// Create a new trampoline manager.
    ///
    /// `trampoline_vaddr` must be the virtual address that maps to `TRAMPOLINE_PHYS`.
    /// In ClaudioOS's identity-mapped lower memory, this is typically the same value.
    pub fn new(trampoline_vaddr: u64) -> Self {
        log::info!(
            "trampoline: creating ApTrampoline, vaddr={:#X}",
            trampoline_vaddr
        );
        Self { trampoline_vaddr }
    }

    /// Install the trampoline code and GDT into low memory.
    ///
    /// Copies the hand-assembled machine code, GDT descriptor, and GDT data
    /// into the trampoline page. The page is zeroed first to ensure no stale
    /// data could confuse an AP.
    ///
    /// # Safety
    /// The target physical page must be mapped writable and not in use by any
    /// other code. The memory must be below 1 MiB for the SIPI vector to reach it.
    pub unsafe fn install(&self) {
        log::info!(
            "trampoline: installing trampoline code at phys {:#X} ({} bytes)",
            TRAMPOLINE_PHYS,
            TRAMPOLINE_CODE.len()
        );

        let base = self.trampoline_vaddr as *mut u8;

        // Zero the entire page first
        ptr::write_bytes(base, 0, 4096);

        // Copy trampoline code at offset 0
        ptr::copy_nonoverlapping(TRAMPOLINE_CODE.as_ptr(), base, TRAMPOLINE_CODE.len());
        log::debug!(
            "trampoline: copied {} bytes of trampoline code",
            TRAMPOLINE_CODE.len()
        );

        // Copy GDT descriptor at offset 0x80
        let gdt_desc_ptr = base.add(0x80);
        ptr::copy_nonoverlapping(
            TRAMPOLINE_GDT_DESC.as_ptr(),
            gdt_desc_ptr,
            TRAMPOLINE_GDT_DESC.len(),
        );

        // Copy GDT data at offset 0x88 (right after the 6-byte descriptor)
        let gdt_data_ptr = base.add(0x88);
        ptr::copy_nonoverlapping(TRAMPOLINE_GDT.as_ptr(), gdt_data_ptr, TRAMPOLINE_GDT.len());
        log::debug!(
            "trampoline: GDT descriptor at {:#X}, GDT data at {:#X} ({} bytes)",
            TRAMPOLINE_PHYS + 0x80,
            TRAMPOLINE_PHYS + 0x88,
            TRAMPOLINE_GDT.len()
        );

        log::info!("trampoline: installation complete");
    }

    /// Set the trampoline data for a specific AP before sending INIT+SIPI.
    ///
    /// # Safety
    /// Must be called after `install()`. The page table, stack, and entry point
    /// must be valid and remain so until the AP signals ready.
    pub unsafe fn set_ap_data(
        &self,
        pml4_addr: u64,
        stack_top: u64,
        entry_point: u64,
        cpu_index: u32,
    ) {
        log::info!(
            "trampoline: setting AP data: cpu={}, pml4={:#X}, stack={:#X}, entry={:#X}",
            cpu_index,
            pml4_addr,
            stack_top,
            entry_point
        );

        let data_ptr = (self.trampoline_vaddr + TRAMPOLINE_DATA_OFFSET) as *mut TrampolineData;
        let data = &mut *data_ptr;

        data.pml4_addr = pml4_addr;
        data.stack_top = stack_top;
        data.entry_point = entry_point;
        data.cpu_index = cpu_index;
        data.ap_ready.store(0, Ordering::Release);

        log::debug!("trampoline: AP data written at phys {:#X}", TRAMPOLINE_DATA_PHYS);
    }

    /// Check if the AP has signaled ready.
    pub fn is_ap_ready(&self) -> bool {
        let data_ptr = (self.trampoline_vaddr + TRAMPOLINE_DATA_OFFSET) as *const TrampolineData;
        let ready = unsafe { (*data_ptr).ap_ready.load(Ordering::Acquire) };
        ready != 0
    }

    /// Wait for the AP to signal ready, with a spin timeout.
    ///
    /// `max_iterations` — maximum number of spin iterations before giving up.
    /// Returns `true` if the AP signaled ready, `false` on timeout.
    pub fn wait_ap_ready(&self, max_iterations: u64) -> bool {
        log::debug!(
            "trampoline: waiting for AP ready (max {} iterations)",
            max_iterations
        );
        for i in 0..max_iterations {
            if self.is_ap_ready() {
                log::info!("trampoline: AP signaled ready after {} iterations", i);
                return true;
            }
            core::hint::spin_loop();
        }
        log::warn!(
            "trampoline: AP did not signal ready within {} iterations",
            max_iterations
        );
        false
    }
}
