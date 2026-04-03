//! Real-Time Clock (RTC) driver — reads wall-clock time from CMOS RTC.
//!
//! The MC146818 RTC (or compatible) is accessed via I/O ports 0x70 (address)
//! and 0x71 (data). It provides seconds, minutes, hours, day, month, year,
//! and optionally century. Values may be in BCD or binary depending on
//! status register B.
//!
//! This module reads the RTC at boot, stores it as a global boot timestamp,
//! and combines it with PIT elapsed ticks to provide a wall clock.

use alloc::format;
use alloc::string::String;
use core::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use x86_64::instructions::port::Port;

/// Date/time components read from the CMOS RTC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DateTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

impl DateTime {
    /// Format as "YYYY-MM-DD HH:MM:SS".
    pub fn format(&self) -> String {
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        )
    }

    /// Approximate Unix timestamp (seconds since 1970-01-01 00:00:00 UTC).
    ///
    /// Uses a simplified calculation that is accurate for dates from
    /// 1970 through 2099 (no leap-second correction, but handles leap years).
    pub fn to_unix_timestamp(&self) -> i64 {
        let y = self.year as i64;
        let m = self.month as i64;
        let d = self.day as i64;

        // Days from 1970-01-01 to the start of the given year
        let mut days: i64 = 0;
        for yr in 1970..y {
            days += if is_leap_year(yr as u16) { 366 } else { 365 };
        }

        // Days from start of year to start of given month
        let month_days: [i64; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
        for mi in 0..(m - 1) as usize {
            days += month_days[mi];
            if mi == 1 && is_leap_year(self.year) {
                days += 1; // February in a leap year
            }
        }

        days += d - 1; // day of month is 1-based

        days * 86400 + self.hour as i64 * 3600 + self.minute as i64 * 60 + self.second as i64
    }
}

fn is_leap_year(y: u16) -> bool {
    (y % 4 == 0 && y % 100 != 0) || (y % 400 == 0)
}

/// Convert a Unix timestamp back to a DateTime (UTC).
fn unix_to_datetime(ts: i64) -> DateTime {
    let mut remaining = ts;

    let second = (remaining % 60) as u8;
    remaining /= 60;
    let minute = (remaining % 60) as u8;
    remaining /= 60;
    let hour = (remaining % 24) as u8;
    remaining /= 24;

    // remaining is now days since epoch
    let mut year: u16 = 1970;
    loop {
        let days_in_year: i64 = if is_leap_year(year) { 366 } else { 365 };
        if remaining < days_in_year {
            break;
        }
        remaining -= days_in_year;
        year += 1;
    }

    let month_days: [i64; 12] = [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31];
    let mut month: u8 = 1;
    for mi in 0..12usize {
        let mut md = month_days[mi];
        if mi == 1 && is_leap_year(year) {
            md += 1;
        }
        if remaining < md {
            break;
        }
        remaining -= md;
        month += 1;
    }

    let day = remaining as u8 + 1;

    DateTime {
        year,
        month,
        day,
        hour,
        minute,
        second,
    }
}

// ── CMOS register access ─────────────────────────────────────────────

/// Read a CMOS register via the standard MC146818 two-port interface.
///
/// Port 0x70 is the address/control register. Bit 7 controls NMI masking:
/// 0 = NMI enabled (normal operation), 1 = NMI disabled. We always leave
/// NMI enabled (bit 7 = 0) by ANDing with 0x7F.
///
/// Port 0x71 is the data register -- reading it returns the value of the
/// CMOS register selected by the last write to port 0x70.
///
/// # Safety
/// Accesses I/O ports 0x70 and 0x71. Must be called from ring 0.
unsafe fn cmos_read(reg: u8) -> u8 {
    unsafe {
        let mut addr_port: Port<u8> = Port::new(0x70);
        let mut data_port: Port<u8> = Port::new(0x71);
        addr_port.write(reg & 0x7F); // bit 7 = 0 => NMI enabled
        data_port.read()
    }
}

/// Check if the RTC update-in-progress flag is set (status register A, bit 7).
unsafe fn update_in_progress() -> bool {
    unsafe { (cmos_read(0x0A) & 0x80) != 0 }
}

/// Convert a BCD (Binary-Coded Decimal) byte to binary.
///
/// BCD encodes each decimal digit in 4 bits: the byte 0x59 means decimal 59.
/// High nibble (val >> 4) is the tens digit, low nibble (val & 0x0F) is the
/// ones digit. Most CMOS RTCs default to BCD mode (status register B bit 2 = 0).
fn bcd_to_bin(val: u8) -> u8 {
    (val >> 4) * 10 + (val & 0x0F)
}

/// Read the raw RTC register set (may be BCD, may be 12h).
unsafe fn read_raw_rtc() -> (u8, u8, u8, u8, u8, u8, u8) {
    unsafe {
        let second = cmos_read(0x00);
        let minute = cmos_read(0x02);
        let hour = cmos_read(0x04);
        let day = cmos_read(0x07);
        let month = cmos_read(0x08);
        let year = cmos_read(0x09);
        let century = cmos_read(0x32);
        (second, minute, hour, day, month, year, century)
    }
}

/// Read the CMOS RTC and return a [`DateTime`].
///
/// Waits for the update-in-progress flag to clear, then reads twice and
/// compares to avoid catching the RTC mid-update.
pub fn read_rtc() -> DateTime {
    unsafe {
        // Wait for any in-progress update to finish
        while update_in_progress() {}

        let (mut sec, mut min, mut hr, mut day, mut mon, mut yr, mut cen) = read_raw_rtc();

        // Read again until two consecutive reads match (guards against update race)
        loop {
            while update_in_progress() {}
            let (s2, m2, h2, d2, mo2, y2, c2) = read_raw_rtc();
            if sec == s2 && min == m2 && hr == h2 && day == d2 && mon == mo2 && yr == y2 && cen == c2
            {
                break;
            }
            sec = s2;
            min = m2;
            hr = h2;
            day = d2;
            mon = mo2;
            yr = y2;
            cen = c2;
        }

        // Read status register B to determine BCD vs binary and 12h vs 24h
        let status_b = cmos_read(0x0B);
        let is_binary = (status_b & 0x04) != 0;
        let is_24h = (status_b & 0x02) != 0;

        // Convert BCD to binary if needed
        if !is_binary {
            sec = bcd_to_bin(sec);
            min = bcd_to_bin(min);
            // For hours in 12h BCD mode, mask off the PM bit before BCD conversion
            hr = if !is_24h {
                let pm = hr & 0x80;
                let h = bcd_to_bin(hr & 0x7F);
                h | (pm >> 1) // preserve PM flag in bit 6 for now
            } else {
                bcd_to_bin(hr)
            };
            day = bcd_to_bin(day);
            mon = bcd_to_bin(mon);
            yr = bcd_to_bin(yr);
            cen = bcd_to_bin(cen);
        }

        // Handle 12h -> 24h conversion
        if !is_24h {
            let pm = if is_binary {
                (hr & 0x80) != 0
            } else {
                // We stored PM in bit 6 above
                (hr & 0x40) != 0
            };
            hr &= 0x3F; // mask off PM bits
            if pm && hr != 12 {
                hr += 12;
            } else if !pm && hr == 12 {
                hr = 0;
            }
        }

        // Compose full year from century + year
        let full_year = if cen > 0 {
            cen as u16 * 100 + yr as u16
        } else {
            // No century register — assume 2000s
            2000u16 + yr as u16
        };

        DateTime {
            year: full_year,
            month: mon,
            day,
            hour: hr,
            minute: min,
            second: sec,
        }
    }
}

// ── Global boot-time + wall clock ────────────────────────────────────

/// Unix timestamp at boot (seconds since epoch), captured from the RTC.
static BOOT_UNIX_TIMESTAMP: AtomicI64 = AtomicI64::new(0);

/// PIT tick count at the moment the RTC was read, so we can compute elapsed
/// time more precisely.
static BOOT_TICK_SNAPSHOT: AtomicU64 = AtomicU64::new(0);

/// Initialise the RTC subsystem: read the hardware clock and store the boot time.
/// Call this once during early boot, after interrupts::init() but before the
/// async executor starts.
pub fn init() {
    let dt = read_rtc();
    let unix = dt.to_unix_timestamp();
    let ticks = crate::interrupts::tick_count();

    BOOT_UNIX_TIMESTAMP.store(unix, Ordering::Relaxed);
    BOOT_TICK_SNAPSHOT.store(ticks, Ordering::Relaxed);

    log::info!(
        "[rtc] boot time: {} (unix {})",
        dt.format(),
        unix,
    );
}

/// Return the current wall-clock time as a [`DateTime`], computed from the
/// RTC boot reading plus PIT elapsed time.
pub fn wall_clock() -> DateTime {
    let boot_unix = BOOT_UNIX_TIMESTAMP.load(Ordering::Relaxed);
    let boot_ticks = BOOT_TICK_SNAPSHOT.load(Ordering::Relaxed);
    let current_ticks = crate::interrupts::tick_count();
    let elapsed_ticks = current_ticks.saturating_sub(boot_ticks);
    // Each PIT tick ≈ 55 ms
    let elapsed_secs = (elapsed_ticks * 55) / 1000;
    let now_unix = boot_unix + elapsed_secs as i64;
    unix_to_datetime(now_unix)
}

/// Return the current wall-clock time formatted as "YYYY-MM-DD HH:MM:SS".
pub fn wall_clock_formatted() -> String {
    wall_clock().format()
}

/// Return the Unix timestamp of boot time.
pub fn boot_timestamp() -> i64 {
    BOOT_UNIX_TIMESTAMP.load(Ordering::Relaxed)
}

/// Return uptime in seconds, derived from PIT ticks since boot.
pub fn uptime_seconds() -> u64 {
    let boot_ticks = BOOT_TICK_SNAPSHOT.load(Ordering::Relaxed);
    let current_ticks = crate::interrupts::tick_count();
    let elapsed = current_ticks.saturating_sub(boot_ticks);
    (elapsed * 55) / 1000
}
