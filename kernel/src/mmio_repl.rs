//! Interactive MMIO peek/poke REPL for the fallback terminal.
//!
//! The real-hardware dev loop is flash-USB → boot → photograph → reset. Every
//! "what's in register X?" question costs a full flash cycle. This REPL turns
//! the boot into an interactive session: the operator drives register reads
//! and writes from the keyboard while looking at results on the framebuffer.
//!
//! Commands (case-insensitive):
//! ```text
//!   r <hex>                  read BAR2 + offset, 32-bit
//!   w <hex> <hex>            write (requires prior `unlock` in this session)
//!   dump <start> <end>       dump a range, 32-bit words
//!   scan                     bulk-probe the whole BAR, skipping sentinels
//!   bar                      show BAR base + window length
//!   chip                     show chip info from the Wi-Fi probe
//!   unlock                   enable `w` (off by default; arms write gate)
//!   lock                     disable `w`
//!   help / ?                 command list
//!   clear                    clear screen and reprint banner
//! ```
//!
//! Offsets and values are interpreted as hex whether `0x`-prefixed or not.

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

/// Set to true by `unlock`; required for `w` to actually write. Prevents
/// finger-slip writes from hard-faulting the laptop. A fresh boot starts
/// locked.
static WRITE_UNLOCKED: AtomicBool = AtomicBool::new(false);

/// Maximum addresses printed by `dump` / `scan` to stop a stray `dump 0 fffff`
/// from rendering for minutes. Bumpable if needed.
const MAX_DUMP_WORDS: u32 = 512;

/// One rendered line of REPL output.
pub type OutputLine = String;

/// Dispatch a user-entered command line. Returns the lines to print.
///
/// The REPL is stateless except for the `WRITE_UNLOCKED` flag; all state
/// lives on the hardware or in `wifi_init`'s BAR2 window.
pub fn handle(line: &str) -> Vec<OutputLine> {
    let line = line.trim();
    if line.is_empty() {
        return Vec::new();
    }
    let mut parts = line.split_whitespace();
    let cmd = parts.next().unwrap_or("").to_ascii_lowercase();
    let args: Vec<&str> = parts.collect();

    match cmd.as_str() {
        "r" | "read" => cmd_read(&args),
        "w" | "write" => cmd_write(&args),
        "dump" | "d" => cmd_dump(&args),
        "scan" => cmd_scan(),
        "bar" => cmd_bar(),
        "chip" => cmd_chip(),
        "unlock" => {
            WRITE_UNLOCKED.store(true, Ordering::Release);
            alloc::vec!["\x1b[93m[REPL] write gate UNLOCKED — w now active\x1b[0m".into()]
        }
        "lock" => {
            WRITE_UNLOCKED.store(false, Ordering::Release);
            alloc::vec!["\x1b[90m[REPL] write gate locked\x1b[0m".into()]
        }
        "help" | "?" => cmd_help(),
        "clear" | "cls" => alloc::vec!["\x1b[2J\x1b[H".into()],
        _ => alloc::vec![alloc::format!(
            "\x1b[91m[REPL] unknown: '{}' — try 'help'\x1b[0m",
            cmd
        )],
    }
}

fn parse_hex(s: &str) -> Option<u32> {
    let s = s.trim().trim_start_matches("0x").trim_start_matches("0X");
    u32::from_str_radix(s, 16).ok()
}

fn cmd_read(args: &[&str]) -> Vec<OutputLine> {
    let Some(off_raw) = args.first() else {
        return alloc::vec!["\x1b[91musage: r <hex-offset>\x1b[0m".into()];
    };
    let Some(off) = parse_hex(off_raw) else {
        return alloc::vec![alloc::format!(
            "\x1b[91m[REPL] bad hex: '{}'\x1b[0m", off_raw
        )];
    };
    match crate::wifi_init::mmio_read32(off) {
        Some(v) => alloc::vec![alloc::format!(
            "\x1b[96m  0x{:04x} = 0x{:08x}\x1b[0m", off, v
        )],
        None => alloc::vec![alloc::format!(
            "\x1b[91m[REPL] 0x{:04x} out of range or BAR2 unmapped\x1b[0m", off
        )],
    }
}

fn cmd_write(args: &[&str]) -> Vec<OutputLine> {
    if !WRITE_UNLOCKED.load(Ordering::Acquire) {
        return alloc::vec![
            "\x1b[91m[REPL] writes are locked — type 'unlock' first\x1b[0m".into()
        ];
    }
    if args.len() < 2 {
        return alloc::vec!["\x1b[91musage: w <hex-offset> <hex-value>\x1b[0m".into()];
    }
    let Some(off) = parse_hex(args[0]) else {
        return alloc::vec![alloc::format!("\x1b[91m[REPL] bad offset '{}'\x1b[0m", args[0])];
    };
    let Some(val) = parse_hex(args[1]) else {
        return alloc::vec![alloc::format!("\x1b[91m[REPL] bad value '{}'\x1b[0m", args[1])];
    };
    // SAFETY: the operator has explicitly unlocked writes; they own the
    // consequences (hard fault, PCI abort, etc.).
    let wrote = unsafe { crate::wifi_init::mmio_write32(off, val) };
    match wrote {
        Some(()) => {
            let after = crate::wifi_init::mmio_read32(off).unwrap_or(0xDEAD_BEEF);
            alloc::vec![alloc::format!(
                "\x1b[92m  w 0x{:04x} <- 0x{:08x}  (readback 0x{:08x})\x1b[0m",
                off, val, after
            )]
        }
        None => alloc::vec![alloc::format!(
            "\x1b[91m[REPL] 0x{:04x} out of range or BAR2 unmapped\x1b[0m", off
        )],
    }
}

fn cmd_dump(args: &[&str]) -> Vec<OutputLine> {
    if args.len() < 2 {
        return alloc::vec!["\x1b[91musage: dump <start-hex> <end-hex>\x1b[0m".into()];
    }
    let (Some(start), Some(end)) = (parse_hex(args[0]), parse_hex(args[1])) else {
        return alloc::vec!["\x1b[91m[REPL] bad hex\x1b[0m".into()];
    };
    if end <= start {
        return alloc::vec!["\x1b[91m[REPL] end must be > start\x1b[0m".into()];
    }
    // Word-align + cap.
    let start = start & !0x3;
    let words = ((end - start) / 4).min(MAX_DUMP_WORDS);
    let mut out = Vec::with_capacity(words as usize + 1);
    for i in 0..words {
        let off = start + i * 4;
        let v = crate::wifi_init::mmio_read32(off).unwrap_or(0xFFFF_FFFF);
        out.push(alloc::format!("\x1b[96m  0x{:04x} = 0x{:08x}\x1b[0m", off, v));
    }
    out.push(alloc::format!(
        "\x1b[90m  [{} words, 0x{:04x}..0x{:04x}]\x1b[0m",
        words, start, start + words * 4
    ));
    out
}

fn cmd_scan() -> Vec<OutputLine> {
    // Scans 0x100..0x2000 looking for non-sentinel values. Same heuristic as
    // the boot-time probe; quick way to see whole-BAR live registers.
    let start = 0x0100u32;
    let end = 0x2000u32;
    let mut out = Vec::new();
    let mut live = 0usize;
    let mut off = start;
    while off < end {
        if let Some(v) = crate::wifi_init::mmio_read32(off) {
            let is_sentinel = v == 0 || v == 0xFFFF_FFFF || v == 0xDEAD_BEEF;
            if !is_sentinel {
                out.push(alloc::format!("\x1b[96m  0x{:04x} = 0x{:08x}\x1b[0m", off, v));
                live += 1;
                if live >= MAX_DUMP_WORDS as usize {
                    break;
                }
            }
        }
        off += 4;
    }
    out.push(alloc::format!(
        "\x1b[90m  [scan 0x{:04x}..0x{:04x}: {} live regs]\x1b[0m",
        start, off, live
    ));
    out
}

fn cmd_bar() -> Vec<OutputLine> {
    let snap = crate::wifi_init::snapshot();
    match snap {
        Some(w) => alloc::vec![
            alloc::format!("\x1b[96m  chip: {}  ids: {}\x1b[0m", w.chip, w.ids),
            alloc::format!("\x1b[96m  BDF:  {}    irq: {}\x1b[0m", w.bdf, w.irq_line),
            alloc::format!("\x1b[96m  BAR2 phys: 0x{:016x}\x1b[0m", w.rtw_mmio_bar),
        ],
        None => alloc::vec!["\x1b[91m[REPL] wifi_init did not run\x1b[0m".into()],
    }
}

fn cmd_chip() -> Vec<OutputLine> {
    // Alias for `bar` for now; in future, runs probe routines to decode
    // chip ID, revision, etc.
    cmd_bar()
}

fn cmd_help() -> Vec<OutputLine> {
    alloc::vec![
        "\x1b[93mMMIO REPL — BAR2 peek/poke\x1b[0m".into(),
        "\x1b[90m  r <off>           read 32-bit\x1b[0m".into(),
        "\x1b[90m  w <off> <val>     write (after 'unlock')\x1b[0m".into(),
        "\x1b[90m  dump <s> <e>      dump range (hex)\x1b[0m".into(),
        "\x1b[90m  scan              probe 0x100..0x2000 for live regs\x1b[0m".into(),
        "\x1b[90m  bar / chip        show BAR + chip info\x1b[0m".into(),
        "\x1b[90m  unlock / lock     arm or disarm the write gate\x1b[0m".into(),
        "\x1b[90m  clear             clear the screen\x1b[0m".into(),
        "\x1b[90m  help / ?          this list\x1b[0m".into(),
    ]
}
