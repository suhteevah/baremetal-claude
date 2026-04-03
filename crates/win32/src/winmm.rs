//! Windows Multimedia (winmm.dll) API implementation.
//!
//! Legacy audio API: waveOut*, timeGetTime, PlaySoundW.
//! Maps to ClaudioOS HDA audio PCM playback.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;
use spin::Mutex;

use crate::unicode::*;
use crate::wasapi::WaveFormatEx;

// =============================================================================
// Constants & error codes
// =============================================================================

/// MMSYSERR error codes.
pub const MMSYSERR_NOERROR: u32 = 0;
pub const MMSYSERR_ERROR: u32 = 1;
pub const MMSYSERR_BADDEVICEID: u32 = 2;
pub const MMSYSERR_NOTENABLED: u32 = 3;
pub const MMSYSERR_ALLOCATED: u32 = 4;
pub const MMSYSERR_INVALHANDLE: u32 = 5;
pub const MMSYSERR_NODRIVER: u32 = 6;
pub const MMSYSERR_NOMEM: u32 = 7;
pub const MMSYSERR_INVALFLAG: u32 = 10;
pub const MMSYSERR_INVALPARAM: u32 = 11;

/// WAVERR error codes.
pub const WAVERR_BADFORMAT: u32 = 32;
pub const WAVERR_STILLPLAYING: u32 = 33;
pub const WAVERR_UNPREPARED: u32 = 34;

/// WAVE_MAPPER — default device.
pub const WAVE_MAPPER: u32 = 0xFFFFFFFF;

/// Callback types.
pub const CALLBACK_NULL: DWord = 0x00000000;
pub const CALLBACK_WINDOW: DWord = 0x00010000;
pub const CALLBACK_THREAD: DWord = 0x00020000;
pub const CALLBACK_FUNCTION: DWord = 0x00030000;
pub const CALLBACK_EVENT: DWord = 0x00050000;

/// WOM messages.
pub const WOM_OPEN: u32 = 0x3BB;
pub const WOM_CLOSE: u32 = 0x3BC;
pub const WOM_DONE: u32 = 0x3BD;

/// PlaySound flags.
pub const SND_SYNC: DWord = 0x0000;
pub const SND_ASYNC: DWord = 0x0001;
pub const SND_NODEFAULT: DWord = 0x0002;
pub const SND_MEMORY: DWord = 0x0004;
pub const SND_LOOP: DWord = 0x0008;
pub const SND_NOSTOP: DWord = 0x0010;
pub const SND_FILENAME: DWord = 0x00020000;
pub const SND_RESOURCE: DWord = 0x00040004;

// =============================================================================
// Structures
// =============================================================================

/// HWAVEOUT handle type.
pub type HWaveOut = u64;

/// WAVEHDR — wave audio buffer header.
#[repr(C)]
pub struct WaveHdr {
    pub data: *mut u8,
    pub buffer_length: DWord,
    pub bytes_recorded: DWord,
    pub user: u64,
    pub flags: DWord,
    pub loops: DWord,
    pub next: *mut WaveHdr,
    pub reserved: u64,
}

/// WAVEHDR flags.
pub const WHDR_DONE: DWord = 0x00000001;
pub const WHDR_PREPARED: DWord = 0x00000002;
pub const WHDR_BEGINLOOP: DWord = 0x00000004;
pub const WHDR_ENDLOOP: DWord = 0x00000008;
pub const WHDR_INQUEUE: DWord = 0x00000010;

/// WAVEOUTCAPSW — wave output device capabilities.
#[repr(C)]
pub struct WaveOutCapsW {
    pub manufacturer_id: u16,
    pub product_id: u16,
    pub driver_version: u32,
    pub product_name: [u16; 32],
    pub formats: DWord,
    pub channels: u16,
    pub reserved: u16,
    pub support: DWord,
}

/// Supported format flags.
pub const WAVE_FORMAT_1M08: DWord = 0x00000001; // 11.025 kHz, Mono, 8-bit
pub const WAVE_FORMAT_1M16: DWord = 0x00000004; // 11.025 kHz, Mono, 16-bit
pub const WAVE_FORMAT_1S08: DWord = 0x00000002; // 11.025 kHz, Stereo, 8-bit
pub const WAVE_FORMAT_1S16: DWord = 0x00000008; // 11.025 kHz, Stereo, 16-bit
pub const WAVE_FORMAT_2M08: DWord = 0x00000010; // 22.05 kHz, Mono, 8-bit
pub const WAVE_FORMAT_2M16: DWord = 0x00000040; // 22.05 kHz, Mono, 16-bit
pub const WAVE_FORMAT_2S08: DWord = 0x00000020; // 22.05 kHz, Stereo, 8-bit
pub const WAVE_FORMAT_2S16: DWord = 0x00000080; // 22.05 kHz, Stereo, 16-bit
pub const WAVE_FORMAT_4M08: DWord = 0x00000100; // 44.1 kHz, Mono, 8-bit
pub const WAVE_FORMAT_4M16: DWord = 0x00000400; // 44.1 kHz, Mono, 16-bit
pub const WAVE_FORMAT_4S08: DWord = 0x00000200; // 44.1 kHz, Stereo, 8-bit
pub const WAVE_FORMAT_4S16: DWord = 0x00000800; // 44.1 kHz, Stereo, 16-bit
pub const WAVE_FORMAT_44S16: DWord = WAVE_FORMAT_4S16;

/// WAVECAPS support flags.
pub const WAVECAPS_PITCH: DWord = 0x0001;
pub const WAVECAPS_PLAYBACKRATE: DWord = 0x0002;
pub const WAVECAPS_VOLUME: DWord = 0x0004;
pub const WAVECAPS_LRVOLUME: DWord = 0x0008;
pub const WAVECAPS_SYNC: DWord = 0x0010;
pub const WAVECAPS_SAMPLEACCURATE: DWord = 0x0020;

/// MMTIME structure for waveOutGetPosition.
#[repr(C)]
pub struct MmTime {
    pub time_type: u32,
    pub u: MmTimeUnion,
}

#[repr(C)]
pub union MmTimeUnion {
    pub ms: u32,
    pub sample: u32,
    pub cb: u32,
    pub ticks: u32,
}

pub const TIME_MS: u32 = 0x0001;
pub const TIME_SAMPLES: u32 = 0x0002;
pub const TIME_BYTES: u32 = 0x0004;
pub const TIME_TICKS: u32 = 0x0008;

// =============================================================================
// Internal state
// =============================================================================

struct WaveOutState {
    format: WaveFormatEx,
    open: bool,
    paused: bool,
    /// Total bytes written (for position tracking).
    bytes_written: u64,
    callback: u64,
    callback_type: DWord,
    volume: u32,
}

static WAVEOUT_TABLE: Mutex<Option<BTreeMap<u64, WaveOutState>>> = Mutex::new(None);
static NEXT_WAVEOUT: Mutex<u64> = Mutex::new(0xAA00_0000);

// Work around: use valid hex
const WAVEOUT_BASE: u64 = 0xAA00_0000;

// =============================================================================
// Public API — waveOut*
// =============================================================================

/// waveOutGetNumDevs — get the number of waveform output devices.
pub fn wave_out_get_num_devs() -> u32 {
    1 // We always report one audio device (HDA)
}

/// waveOutGetDevCapsW — get device capabilities.
pub fn wave_out_get_dev_caps_w(
    device_id: u32,
    caps: *mut WaveOutCapsW,
    size: u32,
) -> u32 {
    if caps.is_null() || size < core::mem::size_of::<WaveOutCapsW>() as u32 {
        return MMSYSERR_INVALPARAM;
    }

    if device_id != 0 && device_id != WAVE_MAPPER {
        return MMSYSERR_BADDEVICEID;
    }

    let name = "ClaudioOS HDA Audio";
    let name_utf16 = crate::unicode::utf8_to_utf16(name);

    unsafe {
        (*caps).manufacturer_id = 0x0045; // Microsoft
        (*caps).product_id = 0x0001;
        (*caps).driver_version = 0x0100;
        (*caps).product_name = [0u16; 32];
        let copy_len = name_utf16.len().min(31);
        for (i, &ch) in name_utf16.iter().take(copy_len).enumerate() {
            (*caps).product_name[i] = ch;
        }
        (*caps).formats = WAVE_FORMAT_1M08 | WAVE_FORMAT_1S08
            | WAVE_FORMAT_1M16 | WAVE_FORMAT_1S16
            | WAVE_FORMAT_2M08 | WAVE_FORMAT_2S08
            | WAVE_FORMAT_2M16 | WAVE_FORMAT_2S16
            | WAVE_FORMAT_4M08 | WAVE_FORMAT_4S08
            | WAVE_FORMAT_4M16 | WAVE_FORMAT_4S16;
        (*caps).channels = 2;
        (*caps).reserved = 0;
        (*caps).support = WAVECAPS_VOLUME | WAVECAPS_LRVOLUME;
    }

    MMSYSERR_NOERROR
}

/// waveOutOpen — open a waveform output device.
pub fn wave_out_open(
    phwo: *mut HWaveOut,
    device_id: u32,
    format: *const WaveFormatEx,
    callback: u64,
    instance: u64,
    flags: DWord,
) -> u32 {
    if format.is_null() { return MMSYSERR_INVALPARAM; }

    let fmt = unsafe { *format };

    // Copy packed fields to locals to avoid unaligned references
    let fmt_tag = { fmt.format_tag };
    let fmt_channels = { fmt.channels };
    let fmt_samples_per_sec = { fmt.samples_per_sec };
    let fmt_bits_per_sample = { fmt.bits_per_sample };

    // Validate format
    if fmt_tag != crate::wasapi::WAVE_FORMAT_PCM
        && fmt_tag != crate::wasapi::WAVE_FORMAT_IEEE_FLOAT
    {
        return WAVERR_BADFORMAT;
    }

    log::info!(
        "[winmm] waveOutOpen: {}ch {}Hz {}bit fmt=0x{:X}",
        fmt_channels, fmt_samples_per_sec, fmt_bits_per_sample, fmt_tag
    );

    let mut counter = NEXT_WAVEOUT.lock();
    let handle = WAVEOUT_BASE + (*counter - WAVEOUT_BASE);
    *counter += 1;

    let mut table = WAVEOUT_TABLE.lock();
    if table.is_none() { *table = Some(BTreeMap::new()); }
    if let Some(ref mut map) = *table {
        map.insert(handle, WaveOutState {
            format: fmt,
            open: true,
            paused: false,
            bytes_written: 0,
            callback,
            callback_type: flags & 0x00070000,
            volume: 0xFFFFFFFF, // Max volume
        });
    }

    if !phwo.is_null() {
        unsafe { *phwo = handle; }
    }

    MMSYSERR_NOERROR
}

/// waveOutPrepareHeader — prepare a buffer for playback.
pub fn wave_out_prepare_header(
    hwo: HWaveOut,
    header: *mut WaveHdr,
    size: u32,
) -> u32 {
    if header.is_null() { return MMSYSERR_INVALPARAM; }

    unsafe {
        (*header).flags |= WHDR_PREPARED;
    }

    MMSYSERR_NOERROR
}

/// waveOutUnprepareHeader — unprepare a previously prepared buffer.
pub fn wave_out_unprepare_header(
    hwo: HWaveOut,
    header: *mut WaveHdr,
    size: u32,
) -> u32 {
    if header.is_null() { return MMSYSERR_INVALPARAM; }

    unsafe {
        if (*header).flags & WHDR_INQUEUE != 0 {
            return WAVERR_STILLPLAYING;
        }
        (*header).flags &= !WHDR_PREPARED;
    }

    MMSYSERR_NOERROR
}

/// waveOutWrite — send a buffer to the device for playback.
pub fn wave_out_write(
    hwo: HWaveOut,
    header: *mut WaveHdr,
    size: u32,
) -> u32 {
    if header.is_null() { return MMSYSERR_INVALPARAM; }

    let buf_len = unsafe { (*header).buffer_length };

    unsafe {
        if (*header).flags & WHDR_PREPARED == 0 {
            return WAVERR_UNPREPARED;
        }
        (*header).flags |= WHDR_INQUEUE;
    }

    log::trace!("[winmm] waveOutWrite: hwo=0x{:X} {} bytes", hwo, buf_len);

    // In a real implementation, we'd copy the PCM data to the HDA ring buffer.
    // For now, simulate immediate completion.
    let mut table = WAVEOUT_TABLE.lock();
    if let Some(ref mut map) = *table {
        if let Some(state) = map.get_mut(&hwo) {
            state.bytes_written += buf_len as u64;
        }
    }

    // Mark buffer as done immediately
    unsafe {
        (*header).flags &= !WHDR_INQUEUE;
        (*header).flags |= WHDR_DONE;
    }

    MMSYSERR_NOERROR
}

/// waveOutReset — stop playback and reset the device.
pub fn wave_out_reset(hwo: HWaveOut) -> u32 {
    log::debug!("[winmm] waveOutReset: hwo=0x{:X}", hwo);

    let mut table = WAVEOUT_TABLE.lock();
    if let Some(ref mut map) = *table {
        if let Some(state) = map.get_mut(&hwo) {
            state.paused = false;
            state.bytes_written = 0;
            return MMSYSERR_NOERROR;
        }
    }
    MMSYSERR_INVALHANDLE
}

/// waveOutPause — pause playback.
pub fn wave_out_pause(hwo: HWaveOut) -> u32 {
    let mut table = WAVEOUT_TABLE.lock();
    if let Some(ref mut map) = *table {
        if let Some(state) = map.get_mut(&hwo) {
            state.paused = true;
            return MMSYSERR_NOERROR;
        }
    }
    MMSYSERR_INVALHANDLE
}

/// waveOutRestart — resume paused playback.
pub fn wave_out_restart(hwo: HWaveOut) -> u32 {
    let mut table = WAVEOUT_TABLE.lock();
    if let Some(ref mut map) = *table {
        if let Some(state) = map.get_mut(&hwo) {
            state.paused = false;
            return MMSYSERR_NOERROR;
        }
    }
    MMSYSERR_INVALHANDLE
}

/// waveOutClose — close the device.
pub fn wave_out_close(hwo: HWaveOut) -> u32 {
    log::debug!("[winmm] waveOutClose: hwo=0x{:X}", hwo);

    let mut table = WAVEOUT_TABLE.lock();
    if let Some(ref mut map) = *table {
        if map.remove(&hwo).is_some() {
            return MMSYSERR_NOERROR;
        }
    }
    MMSYSERR_INVALHANDLE
}

/// waveOutGetVolume — get the current volume.
pub fn wave_out_get_volume(hwo: HWaveOut, volume: *mut DWord) -> u32 {
    if volume.is_null() { return MMSYSERR_INVALPARAM; }

    let table = WAVEOUT_TABLE.lock();
    if let Some(state) = table.as_ref().and_then(|m| m.get(&hwo)) {
        unsafe { *volume = state.volume; }
        return MMSYSERR_NOERROR;
    }

    // Default max volume if device not found
    unsafe { *volume = 0xFFFFFFFF; }
    MMSYSERR_NOERROR
}

/// waveOutSetVolume — set the volume.
pub fn wave_out_set_volume(hwo: HWaveOut, volume: DWord) -> u32 {
    let mut table = WAVEOUT_TABLE.lock();
    if let Some(ref mut map) = *table {
        if let Some(state) = map.get_mut(&hwo) {
            state.volume = volume;
            return MMSYSERR_NOERROR;
        }
    }
    MMSYSERR_INVALHANDLE
}

/// waveOutGetPosition — get the current playback position.
pub fn wave_out_get_position(hwo: HWaveOut, mmt: *mut MmTime, size: u32) -> u32 {
    if mmt.is_null() { return MMSYSERR_INVALPARAM; }

    let table = WAVEOUT_TABLE.lock();
    if let Some(state) = table.as_ref().and_then(|m| m.get(&hwo)) {
        unsafe {
            let time_type = (*mmt).time_type;
            match time_type {
                TIME_BYTES => {
                    (*mmt).u.cb = state.bytes_written as u32;
                }
                TIME_SAMPLES => {
                    let block_align = state.format.block_align.max(1) as u64;
                    (*mmt).u.sample = (state.bytes_written / block_align) as u32;
                }
                _ => {
                    // Default to milliseconds
                    let bytes_per_sec = state.format.avg_bytes_per_sec.max(1) as u64;
                    (*mmt).u.ms = ((state.bytes_written * 1000) / bytes_per_sec) as u32;
                    (*mmt).time_type = TIME_MS;
                }
            }
        }
        return MMSYSERR_NOERROR;
    }
    MMSYSERR_INVALHANDLE
}

// =============================================================================
// Public API — Time
// =============================================================================

/// timeGetTime — get the system time in milliseconds.
pub fn time_get_time() -> DWord {
    crate::kernel32::get_tick_count()
}

/// timeBeginPeriod — request minimum timer resolution.
pub fn time_begin_period(period: u32) -> u32 {
    log::trace!("[winmm] timeBeginPeriod: {} ms", period);
    MMSYSERR_NOERROR // Always succeed
}

/// timeEndPeriod — release minimum timer resolution.
pub fn time_end_period(period: u32) -> u32 {
    log::trace!("[winmm] timeEndPeriod: {} ms", period);
    MMSYSERR_NOERROR
}

// =============================================================================
// Public API — PlaySound
// =============================================================================

/// PlaySoundW — play a sound resource or file.
pub fn play_sound_w(
    sound: LpcWStr,
    hmod: u64,
    flags: DWord,
) -> Bool {
    let sound_str = if !sound.is_null() && (flags & SND_MEMORY == 0) {
        unsafe { utf16_to_utf8(sound) }
    } else {
        String::from("<memory/null>")
    };

    log::info!("[winmm] PlaySoundW: '{}' flags=0x{:X}", sound_str, flags);

    // In a real implementation, we'd decode the WAV file and play through HDA.
    // For now, return success (the sound just won't be audible).
    TRUE
}
