//! HDA audio stream management (DMA, BDL, format, start/stop).
//!
//! Each stream uses a Buffer Descriptor List (BDL) pointing to DMA buffers
//! for cyclic audio playback. Per Intel HDA spec 1.0a, Section 4.5.

use alloc::vec;
use alloc::vec::Vec;
use log::{debug, info, trace, warn};

use crate::registers::*;

// =============================================================================
// Buffer Descriptor List (BDL) entry (Section 3.6.3)
// =============================================================================

/// A single BDL entry: 16 bytes, 128-bit aligned.
///
/// ```text
/// Offset 0x00: Address (64-bit) — physical address of data buffer
/// Offset 0x08: Length  (32-bit) — buffer length in bytes
/// Offset 0x0C: IOC     (32-bit) — bit 0 = Interrupt on Completion
/// ```
#[repr(C, align(16))]
#[derive(Clone, Copy, Debug)]
pub struct BdlEntry {
    /// Physical address of the data buffer.
    pub address: u64,
    /// Length of the data buffer in bytes.
    pub length: u32,
    /// Flags: bit 0 = IOC (Interrupt on Completion).
    pub flags: u32,
}

/// Maximum number of BDL entries per stream.
pub const BDL_MAX_ENTRIES: usize = 256;

/// IOC flag for BDL entry (bit 0).
pub const BDL_IOC: u32 = 1 << 0;

// =============================================================================
// Stream Format encoding (Section 3.7.1)
// =============================================================================

/// Decoded stream format.
#[derive(Debug, Clone, Copy)]
pub struct StreamFormat {
    /// Sample base rate: false = 48kHz, true = 44.1kHz
    pub base_44k1: bool,
    /// Sample rate multiplier (0=x1, 1=x2, 2=x3, 3=x4)
    pub rate_mult: u8,
    /// Sample rate divisor (0=/1, 1=/2, 2=/3, 3=/4, 4=/5, 5=/6, 6=/7, 7=/8)
    pub rate_div: u8,
    /// Bits per sample (0=8, 1=16, 2=20, 3=24, 4=32)
    pub bits: u8,
    /// Number of channels minus 1 (0=mono, 1=stereo, ...)
    pub channels_minus_1: u8,
}

impl StreamFormat {
    /// 48kHz, 16-bit, stereo (the most common PCM format).
    pub const PCM_48K_16BIT_STEREO: Self = StreamFormat {
        base_44k1: false,
        rate_mult: 0,
        rate_div: 0,
        bits: 1, // 16-bit
        channels_minus_1: 1,
    };

    /// 44.1kHz, 16-bit, stereo (CD quality).
    pub const PCM_44K1_16BIT_STEREO: Self = StreamFormat {
        base_44k1: true,
        rate_mult: 0,
        rate_div: 0,
        bits: 1,
        channels_minus_1: 1,
    };

    /// 48kHz, 16-bit, mono.
    pub const PCM_48K_16BIT_MONO: Self = StreamFormat {
        base_44k1: false,
        rate_mult: 0,
        rate_div: 0,
        bits: 1,
        channels_minus_1: 0,
    };

    /// 96kHz, 16-bit, stereo.
    pub const PCM_96K_16BIT_STEREO: Self = StreamFormat {
        base_44k1: false,
        rate_mult: 1, // x2
        rate_div: 0,
        bits: 1,
        channels_minus_1: 1,
    };

    /// 48kHz, 24-bit, stereo.
    pub const PCM_48K_24BIT_STEREO: Self = StreamFormat {
        base_44k1: false,
        rate_mult: 0,
        rate_div: 0,
        bits: 3, // 24-bit
        channels_minus_1: 1,
    };

    /// Encode to the 16-bit SD_FMT register value.
    ///
    /// ```text
    /// Bit 15:    Base (0=48kHz, 1=44.1kHz)
    /// Bits 13:11: Mult (0=x1, 1=x2, 2=x3, 3=x4)
    /// Bits 10:8:  Div  (0=/1, 1=/2, ..., 7=/8)
    /// Bits 6:4:   Bits (0=8, 1=16, 2=20, 3=24, 4=32)
    /// Bits 3:0:   Chan (number of channels - 1)
    /// ```
    pub fn encode(&self) -> u16 {
        let mut val: u16 = 0;
        if self.base_44k1 {
            val |= 1 << 14;
        }
        val |= ((self.rate_mult as u16) & 0x07) << 11;
        val |= ((self.rate_div as u16) & 0x07) << 8;
        val |= ((self.bits as u16) & 0x07) << 4;
        val |= (self.channels_minus_1 as u16) & 0x0F;
        val
    }

    /// Decode from a 16-bit SD_FMT register value.
    pub fn decode(val: u16) -> Self {
        StreamFormat {
            base_44k1: (val >> 14) & 1 != 0,
            rate_mult: ((val >> 11) & 0x07) as u8,
            rate_div: ((val >> 8) & 0x07) as u8,
            bits: ((val >> 4) & 0x07) as u8,
            channels_minus_1: (val & 0x0F) as u8,
        }
    }

    /// Get the effective sample rate in Hz.
    pub fn sample_rate_hz(&self) -> u32 {
        let base = if self.base_44k1 { 44100 } else { 48000 };
        let mult = (self.rate_mult as u32) + 1;
        let div = (self.rate_div as u32) + 1;
        base * mult / div
    }

    /// Get the number of channels.
    pub fn channels(&self) -> u16 {
        (self.channels_minus_1 as u16) + 1
    }

    /// Get the bits per sample.
    pub fn bits_per_sample(&self) -> u16 {
        match self.bits {
            0 => 8,
            1 => 16,
            2 => 20,
            3 => 24,
            4 => 32,
            _ => 16, // fallback
        }
    }

    /// Bytes per frame (channels * bytes_per_sample).
    pub fn frame_size(&self) -> usize {
        let bytes_per_sample = (self.bits_per_sample() as usize + 7) / 8;
        self.channels() as usize * bytes_per_sample
    }
}

// =============================================================================
// Stream descriptor management
// =============================================================================

/// An HDA output stream backed by DMA buffers and a BDL.
pub struct HdaStream {
    /// BAR0 MMIO base.
    bar0: usize,
    /// Stream descriptor index (0-based, includes input streams offset).
    sd_index: usize,
    /// Stream number (1-15, programmed into CTL2 and codec converters).
    stream_number: u8,
    /// The BDL entries (heap-allocated, physically contiguous in our kernel).
    bdl: Vec<BdlEntry>,
    /// Physical address of the BDL.
    bdl_phys: u64,
    /// DMA audio buffer.
    dma_buffer: Vec<u8>,
    /// Physical address of the DMA buffer.
    dma_phys: u64,
    /// Stream format.
    format: StreamFormat,
    /// Whether the stream is currently running.
    running: bool,
}

impl HdaStream {
    /// Create and configure a new output stream.
    ///
    /// `sd_index` is the stream descriptor index in the register space
    /// (input streams first, then output streams).
    /// `stream_number` is 1-15 and must match what's programmed on the codec DAC.
    ///
    /// # Safety
    /// `bar0` must be a valid mapped HDA BAR0.
    pub unsafe fn new(
        bar0: usize,
        sd_index: usize,
        stream_number: u8,
        format: StreamFormat,
        buffer_size: usize,
    ) -> Self {
        info!(
            "hda: creating stream: sd_index={}, stream_number={}, format={:?}, buffer_size={}",
            sd_index, stream_number, format, buffer_size
        );

        let sd_base = stream_desc_offset(sd_index);

        // Reset the stream descriptor
        unsafe {
            Self::reset_stream(bar0, sd_base);
        }

        // Allocate DMA buffer (zeroed)
        let dma_buffer = vec![0u8; buffer_size];
        let dma_phys = dma_buffer.as_ptr() as u64;
        debug!(
            "hda: stream {}: DMA buffer at phys=0x{:016x}, size={}",
            sd_index, dma_phys, buffer_size
        );

        // Build BDL with a single entry pointing to the entire buffer.
        // For larger buffers or cyclic playback, split into multiple entries.
        let bdl_entry = BdlEntry {
            address: dma_phys,
            length: buffer_size as u32,
            flags: BDL_IOC, // interrupt when this buffer completes
        };
        let bdl = vec![bdl_entry];
        let bdl_phys = bdl.as_ptr() as u64;
        debug!(
            "hda: stream {}: BDL at phys=0x{:016x}, 1 entry",
            sd_index, bdl_phys
        );

        // Configure stream descriptor registers
        unsafe {
            // Set BDL address
            write32(bar0, sd_base + SD_BDPL, bdl_phys as u32);
            write32(bar0, sd_base + SD_BDPU, (bdl_phys >> 32) as u32);
            trace!("hda: stream {}: BDL pointer set", sd_index);

            // Set Cyclic Buffer Length (total bytes in all BDL entries)
            write32(bar0, sd_base + SD_CBL, buffer_size as u32);
            trace!("hda: stream {}: CBL={}", sd_index, buffer_size);

            // Set Last Valid Index (0-based, so for 1 entry it's 0)
            write16(bar0, sd_base + SD_LVI, 0);
            trace!("hda: stream {}: LVI=0", sd_index);

            // Set stream format
            let fmt = format.encode();
            write16(bar0, sd_base + SD_FMT, fmt);
            debug!(
                "hda: stream {}: format=0x{:04x} ({}Hz, {}bit, {}ch)",
                sd_index,
                fmt,
                format.sample_rate_hz(),
                format.bits_per_sample(),
                format.channels()
            );

            // Set stream number and direction (output) in CTL byte 2
            let ctl2 = (stream_number << SD_CTL2_STRM_SHIFT) | SD_CTL2_DIR;
            write8(bar0, sd_base + SD_CTL + 2, ctl2);
            trace!(
                "hda: stream {}: CTL2=0x{:02x} (stream_num={}, dir=output)",
                sd_index, ctl2, stream_number
            );

            // Enable interrupts: IOC + FIFO error + descriptor error
            let ctl0 = read8(bar0, sd_base + SD_CTL);
            write8(
                bar0,
                sd_base + SD_CTL,
                ctl0 | SD_CTL_IOCE | SD_CTL_FEIE | SD_CTL_DEIE,
            );
            trace!("hda: stream {}: interrupts enabled", sd_index);
        }

        HdaStream {
            bar0,
            sd_index,
            stream_number,
            bdl,
            bdl_phys,
            dma_buffer,
            dma_phys,
            format,
            running: false,
        }
    }

    /// Reset a stream descriptor.
    ///
    /// # Safety
    /// BAR0 must be valid. `sd_base` must be the correct stream descriptor offset.
    unsafe fn reset_stream(bar0: usize, sd_base: usize) {
        trace!("hda: resetting stream at offset 0x{:03x}", sd_base);

        // Assert SRST
        unsafe {
            let ctl0 = read8(bar0, sd_base + SD_CTL);
            write8(bar0, sd_base + SD_CTL, ctl0 | SD_CTL_SRST);
        }

        // Wait for SRST to be acknowledged
        let mut attempts = 0;
        loop {
            let ctl0 = unsafe { read8(bar0, sd_base + SD_CTL) };
            if ctl0 & SD_CTL_SRST != 0 {
                break;
            }
            attempts += 1;
            if attempts > 10_000 {
                warn!("hda: stream reset assert timeout at offset 0x{:03x}", sd_base);
                break;
            }
            core::hint::spin_loop();
        }

        // Deassert SRST
        unsafe {
            let ctl0 = read8(bar0, sd_base + SD_CTL);
            write8(bar0, sd_base + SD_CTL, ctl0 & !SD_CTL_SRST);
        }

        // Wait for SRST to clear
        attempts = 0;
        loop {
            let ctl0 = unsafe { read8(bar0, sd_base + SD_CTL) };
            if ctl0 & SD_CTL_SRST == 0 {
                break;
            }
            attempts += 1;
            if attempts > 10_000 {
                warn!("hda: stream reset deassert timeout at offset 0x{:03x}", sd_base);
                break;
            }
            core::hint::spin_loop();
        }

        trace!("hda: stream reset complete at offset 0x{:03x}", sd_base);
    }

    /// Fill the DMA buffer with PCM sample data.
    ///
    /// `samples` are interleaved signed 16-bit PCM samples.
    /// Returns the number of bytes written.
    pub fn fill_buffer(&mut self, samples: &[i16]) -> usize {
        let byte_len = samples.len() * 2;
        let copy_len = byte_len.min(self.dma_buffer.len());
        let sample_count = copy_len / 2;

        trace!(
            "hda: stream {}: filling buffer with {} samples ({} bytes)",
            self.sd_index,
            sample_count,
            copy_len
        );

        for (i, &sample) in samples[..sample_count].iter().enumerate() {
            let bytes = sample.to_le_bytes();
            self.dma_buffer[i * 2] = bytes[0];
            self.dma_buffer[i * 2 + 1] = bytes[1];
        }

        // Zero remaining buffer
        for byte in &mut self.dma_buffer[copy_len..] {
            *byte = 0;
        }

        // Update CBL to reflect actual data length
        let sd_base = stream_desc_offset(self.sd_index);
        unsafe {
            write32(self.bar0, sd_base + SD_CBL, copy_len as u32);
        }

        // Update BDL entry length
        if !self.bdl.is_empty() {
            self.bdl[0].length = copy_len as u32;
        }

        debug!(
            "hda: stream {}: buffer filled, CBL={}",
            self.sd_index, copy_len
        );

        copy_len
    }

    /// Start the stream (begin DMA playback).
    ///
    /// # Safety
    /// The DMA buffer must contain valid audio data and the codec path
    /// must be configured to receive from this stream number.
    pub unsafe fn start(&mut self) {
        if self.running {
            warn!("hda: stream {}: already running", self.sd_index);
            return;
        }

        let sd_base = stream_desc_offset(self.sd_index);
        info!("hda: starting stream {} (sd_offset=0x{:03x})", self.sd_index, sd_base);

        // Clear any pending status bits
        unsafe {
            write8(
                self.bar0,
                sd_base + SD_STS,
                SD_STS_BCIS | SD_STS_FIFOE | SD_STS_DESE,
            );
        }

        // Set RUN bit
        unsafe {
            let ctl0 = read8(self.bar0, sd_base + SD_CTL);
            write8(self.bar0, sd_base + SD_CTL, ctl0 | SD_CTL_RUN);
        }

        self.running = true;
        info!("hda: stream {} started", self.sd_index);
    }

    /// Stop the stream (halt DMA).
    pub unsafe fn stop(&mut self) {
        if !self.running {
            trace!("hda: stream {}: already stopped", self.sd_index);
            return;
        }

        let sd_base = stream_desc_offset(self.sd_index);
        info!("hda: stopping stream {}", self.sd_index);

        // Clear RUN bit
        unsafe {
            let ctl0 = read8(self.bar0, sd_base + SD_CTL);
            write8(self.bar0, sd_base + SD_CTL, ctl0 & !SD_CTL_RUN);
        }

        // Wait for stream to stop
        let mut attempts = 0;
        loop {
            let ctl0 = unsafe { read8(self.bar0, sd_base + SD_CTL) };
            if ctl0 & SD_CTL_RUN == 0 {
                break;
            }
            attempts += 1;
            if attempts > 10_000 {
                warn!("hda: stream {} stop timeout", self.sd_index);
                break;
            }
            core::hint::spin_loop();
        }

        self.running = false;
        info!("hda: stream {} stopped", self.sd_index);
    }

    /// Read the current Link Position In Buffer (byte offset into the cyclic buffer).
    pub fn position(&self) -> u32 {
        let sd_base = stream_desc_offset(self.sd_index);
        let lpib = unsafe { read32(self.bar0, sd_base + SD_LPIB) };
        trace!("hda: stream {}: LPIB={}", self.sd_index, lpib);
        lpib
    }

    /// Read the stream status register.
    pub fn status(&self) -> u8 {
        let sd_base = stream_desc_offset(self.sd_index);
        unsafe { read8(self.bar0, sd_base + SD_STS) }
    }

    /// Check if the buffer completion interrupt fired.
    pub fn buffer_complete(&self) -> bool {
        self.status() & SD_STS_BCIS != 0
    }

    /// Clear the buffer completion interrupt status bit.
    pub unsafe fn clear_buffer_complete(&self) {
        let sd_base = stream_desc_offset(self.sd_index);
        unsafe {
            write8(self.bar0, sd_base + SD_STS, SD_STS_BCIS);
        }
    }

    /// Get the stream number (for programming the codec converter).
    pub fn stream_number(&self) -> u8 {
        self.stream_number
    }

    /// Get the format register value.
    pub fn format_register(&self) -> u16 {
        self.format.encode()
    }

    /// Check if the stream is currently running.
    pub fn is_running(&self) -> bool {
        self.running
    }
}

/// Generate a sine wave tone as 16-bit PCM samples.
///
/// Returns a Vec of interleaved i16 samples for the given parameters.
pub fn generate_tone(
    frequency_hz: u16,
    sample_rate: u32,
    channels: u16,
    duration_ms: u32,
) -> Vec<i16> {
    let total_frames = (sample_rate * duration_ms / 1000) as usize;
    let total_samples = total_frames * channels as usize;
    let mut samples = Vec::with_capacity(total_samples);

    debug!(
        "hda: generating tone: {}Hz, {}Hz sample rate, {} channels, {}ms ({} frames)",
        frequency_hz, sample_rate, channels, duration_ms, total_frames
    );

    // Fixed-point sine approximation (no libm dependency).
    // We use a simple parabolic approximation of sine.
    let period_samples = sample_rate / frequency_hz as u32;

    for frame in 0..total_frames {
        // Phase in range [0, period_samples)
        let phase = (frame as u32) % period_samples;
        // Normalize to [-1.0, 1.0] range using fixed-point
        // phase_norm = (phase * 2 / period) - 1, scaled to i32
        let phase_scaled = (phase as i64 * 2 * 32768) / period_samples as i64 - 32768;

        // Parabolic sine approximation: sin(x) ~= 4x(1-|x|) for x in [-1,1]
        // x is phase_scaled / 32768
        let x = phase_scaled;
        let abs_x = if x < 0 { -x } else { x };
        // 4 * x * (32768 - abs_x) / 32768, result scaled to [-32767, 32767]
        let sine = (4 * x * (32768 - abs_x)) / (32768 * 32768 / 32767);
        let sample = sine.clamp(-32767, 32767) as i16;

        // Write same sample to all channels (mono content on all channels)
        for _ in 0..channels {
            samples.push(sample);
        }
    }

    trace!(
        "hda: generated {} PCM samples ({} bytes)",
        samples.len(),
        samples.len() * 2
    );
    samples
}
