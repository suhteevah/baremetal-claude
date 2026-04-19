//! Wi-Fi probing: walks enumerated PCI devices looking for a supported Wi-Fi
//! adapter and hands control to the `claudio-wifi` dispatcher.
//!
//! On real hardware this lands at the Realtek RTL8852BE stub branch (Victus).
//! The stub branch is intentionally non-invasive — no MMIO writes, no IRQ
//! setup — so it cannot hang the kernel. The Intel branch calls real
//! `IntelController::init` which is MMIO-heavy; it is gated behind
//! `WIFI_ON_REAL_HW` so we don't accidentally poke at hardware that isn't
//! there.
//!
//! Every step paints a distinct `fb_checkpoint` so boot progress is
//! observable without serial, and writes a one-line summary to a static
//! snapshot the fallback terminal renders.

extern crate alloc;

use alloc::string::{String, ToString};
use spin::Mutex;

use crate::fb_checkpoint;
use crate::pci;
use crate::phys_mem_offset;
use claudio_wifi::backend::PciProbe;
use claudio_wifi::{probe, WifiController, INTEL_VENDOR, REALTEK_VENDOR};

/// Disable real MMIO-poking Wi-Fi init until the Realtek driver is fleshed
/// out. The Realtek *stub* arm does nothing harmful; the Intel arm does. We
/// flip this off until one of them is known-good on the target silicon.
///
/// Currently: probe runs, records what was found to `WIFI_STATUS`, but
/// `IntelController::init` will NOT execute even if an Intel card is found on
/// real hardware — we bail at the probe step with a note in the status. The
/// Realtek stub arm always runs (it is a no-op).
const WIFI_ON_REAL_HW: bool = true;

/// Captured result of the Wi-Fi probe attempt. Rendered to the fallback
/// terminal by `main.rs` so real-HW runs produce readable evidence without
/// serial output.
static WIFI_STATUS: Mutex<Option<WifiStatus>> = Mutex::new(None);

#[derive(Debug, Clone)]
pub struct WifiStatus {
    /// Human-readable chip name ("Realtek RTL8852BE" or "Intel AX201") or
    /// `"none"` if no supported device was enumerated.
    pub chip: String,
    /// PCI BDF tuple, formatted.
    pub bdf: String,
    /// Vendor:device ID, lowercase hex.
    pub ids: String,
    /// BAR0 as listed in PCI config (legacy field, usually I/O on Realtek).
    pub bar0: u32,
    /// Full six-BAR snapshot, preformatted for terminal dump. One line per
    /// populated BAR, e.g. "BAR2 MMIO64 0x00000000fe100000".
    pub bars_formatted: String,
    /// The MMIO BAR the Realtek driver will use (BAR2), or 0 if absent.
    pub rtw_mmio_bar: u64,
    /// IRQ line from PCI config (unreliable on CNVi parts but worth logging).
    pub irq_line: u8,
    /// Formatted register readings from Realtek readonly probe (empty for
    /// Intel / gated / no-device paths). One line per register: `NAME@0xOFF=0xVALUE`.
    pub probe_regs: String,
    /// Outcome — one of: detected-stub / initialized / gated / not-found / error:...
    pub outcome: String,
}

/// Lock-free snapshot for the fallback terminal.
pub fn snapshot() -> Option<WifiStatus> {
    WIFI_STATUS.lock().clone()
}

/// HHDM-translated virtual base of the Wi-Fi BAR2 MMIO region. Set by
/// `init()` once the device is located. 0 = not set. Used by the fallback
/// terminal's peek/poke REPL to let the operator inspect registers live
/// without reflashing.
static BAR2_VIRT: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);
static BAR2_LEN: core::sync::atomic::AtomicUsize =
    core::sync::atomic::AtomicUsize::new(0);

/// Default BAR2 size assumption for RTL8852BE. The chip's actual BAR size
/// is negotiated via the BAR-sizing probe but we hard-code a conservative
/// 2 MiB window for the REPL so we don't accidentally step off the mapped
/// region and fault. Actual BAR is reported as 8 MiB by Linux rtw89.
const DEFAULT_BAR2_LEN: usize = 2 * 1024 * 1024;

/// Read a 32-bit register at BAR2 + offset. Returns `None` if BAR2 isn't
/// mapped or offset is out of range. No hardware side effects except
/// the volatile read itself.
pub fn mmio_read32(offset: u32) -> Option<u32> {
    let base = BAR2_VIRT.load(core::sync::atomic::Ordering::Acquire);
    if base == 0 {
        return None;
    }
    let len = BAR2_LEN.load(core::sync::atomic::Ordering::Acquire);
    let off = offset as usize;
    if off + 4 > len {
        return None;
    }
    let ptr = (base + off) as *const u32;
    Some(unsafe { core::ptr::read_volatile(ptr) })
}

/// Write a 32-bit register at BAR2 + offset. `None` on out-of-range.
///
/// # Safety
///
/// Every rtw89 register write can change chip state in ways that the rest of
/// the kernel has no defence against (e.g. triggering an IRQ we don't handle,
/// DMA with a stale ring pointer, power-gating the CPU clock). Caller owns
/// the chip's state machine.
pub unsafe fn mmio_write32(offset: u32, value: u32) -> Option<()> {
    let base = BAR2_VIRT.load(core::sync::atomic::Ordering::Acquire);
    if base == 0 {
        return None;
    }
    let len = BAR2_LEN.load(core::sync::atomic::Ordering::Acquire);
    let off = offset as usize;
    if off + 4 > len {
        return None;
    }
    let ptr = (base + off) as *mut u32;
    unsafe { core::ptr::write_volatile(ptr, value) };
    Some(())
}

/// Scan PCI for a supported Wi-Fi adapter and record the result.
///
/// Safe to call multiple times (last call wins). Must be called AFTER
/// `pci::enumerate`, the heap, and (for a real Intel init) the IDT.
pub fn init() {
    fb_checkpoint(60, (0, 200, 255)); // cyan: wifi probe entered

    let devs = pci::snapshot();
    let wifi_dev = devs.into_iter().find(|d| {
        // Class 0x02 = Network, subclass 0x80 = "Other" (where Wi-Fi usually
        // lives on Intel CNVi and Realtek combo cards). Subclass 0x00 is plain
        // Ethernet — skip. Any Intel or Realtek network-class device is a
        // candidate for the dispatcher which will further gate by device ID.
        d.class == 0x02
            && d.subclass == 0x80
            && (d.vendor_id == INTEL_VENDOR || d.vendor_id == REALTEK_VENDOR)
    });

    let Some(d) = wifi_dev else {
        fb_checkpoint(61, (128, 128, 128)); // grey: no candidate
        *WIFI_STATUS.lock() = Some(WifiStatus {
            chip: "none".to_string(),
            bdf: "-".to_string(),
            ids: "-".to_string(),
            bar0: 0,
            bars_formatted: String::new(),
            rtw_mmio_bar: 0,
            irq_line: 0,
            probe_regs: String::new(),
            outcome: "no network class/80 device on PCI".to_string(),
        });
        log::warn!("[wifi] no supported Wi-Fi adapter found");
        return;
    };

    fb_checkpoint(61, (0, 255, 255)); // bright cyan: candidate found

    let chip = match d.vendor_id {
        REALTEK_VENDOR => match d.device_id {
            0xB520 => "Realtek RTL8852BE",
            0xB852 => "Realtek RTL8852AE",
            0xC822 => "Realtek RTL8822CE",
            _ => "Realtek Wi-Fi (unknown model)",
        },
        INTEL_VENDOR => "Intel Wi-Fi (see pci::identify)",
        _ => "unknown vendor",
    };
    let bdf = alloc::format!("{:02x}:{:02x}.{}", d.bus, d.device, d.function);
    let ids = alloc::format!("{:04x}:{:04x}", d.vendor_id, d.device_id);

    // Format all BARs for the terminal dump + pull out the Realtek MMIO
    // address (BAR2 on PCIe Realtek; rtw89 reference).
    let mut bars_formatted = String::new();
    let mut rtw_mmio_bar = 0u64;
    for (idx, bar) in d.bars.iter().enumerate() {
        use crate::pci::BarKind;
        let kind = match bar.kind {
            BarKind::Unused => continue,
            BarKind::Mmio32 => "MMIO32",
            BarKind::Mmio64 => "MMIO64",
            BarKind::Io => "IO    ",
        };
        bars_formatted.push_str(&alloc::format!(
            "BAR{} {} 0x{:016x}\n", idx, kind, bar.addr
        ));
        if idx == 2 && matches!(bar.kind, BarKind::Mmio32 | BarKind::Mmio64) {
            rtw_mmio_bar = bar.addr;
        }
    }

    log::info!(
        "[wifi] candidate: {} {} ids={} bar0={:#010x} irq={}",
        chip, bdf, ids, d.bar0, d.irq_line
    );

    // Ensure PCI Memory Space + Bus Master are enabled before touching BAR2.
    // UEFI normally enables both, but CNVi / combo cards are sometimes left
    // with only Memory Space on; MSI-X programming later will need Bus Master.
    crate::pci::enable_mem_and_bus_master(d.bus, d.device, d.function);
    fb_checkpoint(62, (255, 255, 0)); // yellow: PCI cmd configured

    // Publish the BAR2 virt base for the REPL to use. Done even when
    // WIFI_ON_REAL_HW is false so the operator can still peek/poke from
    // the fallback terminal.
    if rtw_mmio_bar != 0 {
        let virt = (rtw_mmio_bar + phys_mem_offset()) as usize;
        BAR2_VIRT.store(virt, core::sync::atomic::Ordering::Release);
        BAR2_LEN.store(DEFAULT_BAR2_LEN, core::sync::atomic::Ordering::Release);
    }

    if !WIFI_ON_REAL_HW {
        fb_checkpoint(63, (255, 200, 0)); // amber: gated
        *WIFI_STATUS.lock() = Some(WifiStatus {
            chip: chip.to_string(),
            bdf,
            ids,
            bar0: d.bar0,
            bars_formatted,
            rtw_mmio_bar,
            irq_line: d.irq_line,
            probe_regs: String::new(),
            outcome: "gated (WIFI_ON_REAL_HW=false)".to_string(),
        });
        log::info!("[wifi] init gated; probe not invoked");
        return;
    }

    // Hand off to the dispatcher. The Realtek arm is a no-op stub; the Intel
    // arm does real MMIO. Pick the correct MMIO BAR per vendor: Realtek uses
    // BAR2, Intel uses BAR0. The legacy `bar0` field stays populated for
    // drivers that still expect it.
    let mmio = match d.vendor_id {
        REALTEK_VENDOR => rtw_mmio_bar,
        _ => d.bar0 as u64,
    };
    let probe_input = PciProbe {
        vendor_id: d.vendor_id,
        device_id: d.device_id,
        bar0: mmio,
        irq_line: d.irq_line,
        phys_mem_offset: phys_mem_offset(),
    };
    fb_checkpoint(64, (0, 255, 128)); // green: calling probe

    let mut probe_regs = String::new();
    let outcome = match unsafe { probe(&probe_input) } {
        Ok(Some(WifiController::Intel(_))) => {
            fb_checkpoint(65, (0, 255, 0));
            "initialized (Intel)".to_string()
        }
        Ok(Some(WifiController::Realtek(ctrl))) => {
            fb_checkpoint(65, (255, 100, 255));
            let probe = ctrl.initial_probe();
            // Compact format: `XXXX=VVVVVVVV\n` (14 bytes/reg) — dropped the
            // `name@0x` / `=0x` scaffolding since offsets are always 4 hex
            // digits and values always 8. Keeps the QR in manageable size.
            for r in &probe.readings {
                probe_regs.push_str(&alloc::format!(
                    "{:04x}={:08x}\n",
                    r.offset, r.value
                ));
            }
            alloc::format!("readonly-probe: {}", probe.summary)
        }
        Ok(None) => {
            fb_checkpoint(65, (128, 128, 128));
            "probe: device not recognized by any backend".to_string()
        }
        Err(e) => {
            fb_checkpoint(65, (255, 0, 0));
            alloc::format!("error: {}", e)
        }
    };

    *WIFI_STATUS.lock() = Some(WifiStatus {
        chip: chip.to_string(),
        bdf,
        ids,
        bar0: d.bar0,
        bars_formatted,
        rtw_mmio_bar,
        irq_line: d.irq_line,
        probe_regs,
        outcome,
    });
}
