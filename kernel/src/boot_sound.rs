//! PC speaker boot chime — plays tones via PIT channel 2 + port 0x61.
//!
//! The PC speaker is universally available on x86 systems (real and QEMU).
//! We program PIT channel 2 as a square-wave generator and gate its output
//! to the speaker via port 0x61 bits 0-1.

use x86_64::instructions::port::Port;

/// Program PIT channel 2 to produce a square wave at `freq_hz` and enable
/// the PC speaker. The tone plays until `stop_tone()` is called.
fn start_tone(freq_hz: u32) {
    if freq_hz == 0 {
        return;
    }
    let divisor = 1_193_182u32 / freq_hz;
    let divisor = if divisor > 0xFFFF { 0xFFFF } else if divisor == 0 { 1 } else { divisor };

    unsafe {
        // PIT command: channel 2, access lo/hi, mode 3 (square wave)
        let mut cmd: Port<u8> = Port::new(0x43);
        cmd.write(0xB6);

        // Load divisor into channel 2 (port 0x42)
        let mut ch2: Port<u8> = Port::new(0x42);
        ch2.write((divisor & 0xFF) as u8);
        ch2.write(((divisor >> 8) & 0xFF) as u8);

        // Enable speaker: set bits 0 (gate) and 1 (speaker enable) of port 0x61
        let mut speaker: Port<u8> = Port::new(0x61);
        let val = speaker.read();
        speaker.write(val | 0x03);
    }
}

/// Disable the PC speaker by clearing bits 0-1 of port 0x61.
fn stop_tone() {
    unsafe {
        let mut speaker: Port<u8> = Port::new(0x61);
        let val = speaker.read();
        speaker.write(val & !0x03);
    }
}

/// Busy-wait for approximately `ms` milliseconds using PIT channel 0 counting.
/// This is a rough delay — good enough for audible tone durations.
fn delay_ms(ms: u32) {
    // PIT channel 0 ticks at 1,193,182 Hz. We count ticks via the timer
    // interrupt (each tick ~55ms at 18.2 Hz), but for short sub-tick delays
    // we spin-loop reading the PIT counter directly.
    //
    // For simplicity, use a calibrated spin loop. Each iteration of the
    // inner loop takes roughly the same time on QEMU. We read PIT channel 0
    // to measure elapsed ticks.
    let target_ticks = (ms as u64) * 1_193; // ~1193 PIT ticks per ms

    unsafe {
        // Latch channel 0 count
        let mut cmd: Port<u8> = Port::new(0x43);
        let mut ch0: Port<u8> = Port::new(0x40);

        cmd.write(0x00); // latch channel 0
        let lo = ch0.read() as u16;
        let hi = ch0.read() as u16;
        let start = (hi << 8) | lo;

        let mut elapsed: u64 = 0;
        let mut prev = start;

        while elapsed < target_ticks {
            // Brief spin to avoid hammering the port too fast
            for _ in 0..100 {
                core::hint::spin_loop();
            }

            cmd.write(0x00); // latch channel 0
            let lo = ch0.read() as u16;
            let hi = ch0.read() as u16;
            let current = (hi << 8) | lo;

            // PIT counts DOWN — if current > prev, counter wrapped around
            let delta = if current <= prev {
                (prev - current) as u64
            } else {
                (prev as u64) + (0x10000 - current as u64)
            };
            elapsed += delta;
            prev = current;
        }
    }
}

/// Play a single tone at the given frequency for the given duration.
pub fn play_tone(freq_hz: u32, duration_ms: u32) {
    start_tone(freq_hz);
    delay_ms(duration_ms);
    stop_tone();
}

/// Play the ClaudioOS boot chime: ascending C5-E5-G5 triad.
///
/// Frequencies: C5=523 Hz, E5=659 Hz, G5=784 Hz
/// Each note is 100ms with a 20ms gap between notes.
pub fn boot_chime() {
    log::info!("[sound] playing boot chime");
    play_tone(523, 100);  // C5
    delay_ms(20);
    play_tone(659, 100);  // E5
    delay_ms(20);
    play_tone(784, 150);  // G5 — held slightly longer for resolution
    log::info!("[sound] boot chime complete");
}
