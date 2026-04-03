//! Windows Audio Session API (WASAPI) implementation.
//!
//! Maps IAudioClient, IAudioRenderClient, IMMDeviceEnumerator, and IMMDevice
//! to ClaudioOS HDA audio PCM playback.

use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;
use spin::Mutex;

use crate::ole32::{self, HResult, S_OK, E_INVALIDARG, E_FAIL, E_OUTOFMEMORY, E_NOTIMPL};
use crate::unicode::*;

// =============================================================================
// GUIDs
// =============================================================================

/// CLSID_MMDeviceEnumerator
pub const CLSID_MM_DEVICE_ENUMERATOR: Guid = Guid {
    data1: 0xBCDE0395,
    data2: 0xE52F,
    data3: 0x467C,
    data4: [0x8E, 0x3D, 0xC4, 0x57, 0x92, 0x91, 0x69, 0x2E],
};

/// IID_IMMDeviceEnumerator
pub const IID_IMM_DEVICE_ENUMERATOR: Guid = Guid {
    data1: 0xA95664D2,
    data2: 0x9614,
    data3: 0x4F35,
    data4: [0xA7, 0x46, 0xDE, 0x8D, 0xB6, 0x36, 0x17, 0xE6],
};

/// IID_IAudioClient
pub const IID_IAUDIO_CLIENT: Guid = Guid {
    data1: 0x1CB9AD4C,
    data2: 0xDBFA,
    data3: 0x4C32,
    data4: [0xB1, 0x78, 0xC2, 0xF5, 0x68, 0xA7, 0x03, 0xB2],
};

/// IID_IAudioRenderClient
pub const IID_IAUDIO_RENDER_CLIENT: Guid = Guid {
    data1: 0xF294ACFC,
    data2: 0x3146,
    data3: 0x4483,
    data4: [0xA7, 0xBF, 0xAD, 0xDC, 0xA7, 0xC2, 0x60, 0xE2],
};

// =============================================================================
// Constants & enums
// =============================================================================

/// AUDCLNT_SHAREMODE
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShareMode {
    Shared = 0,
    Exclusive = 1,
}

/// EDataFlow
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataFlow {
    Render = 0,
    Capture = 1,
    All = 2,
}

/// ERole
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Console = 0,
    Multimedia = 1,
    Communications = 2,
}

/// AUDCLNT_BUFFERFLAGS
pub const AUDCLNT_BUFFERFLAGS_SILENT: DWord = 0x2;

/// AUDCLNT error codes.
pub const AUDCLNT_E_NOT_INITIALIZED: HResult = 0x88890001_u32 as i32;
pub const AUDCLNT_E_ALREADY_INITIALIZED: HResult = 0x88890002_u32 as i32;
pub const AUDCLNT_E_WRONG_ENDPOINT_TYPE: HResult = 0x88890003_u32 as i32;
pub const AUDCLNT_E_DEVICE_INVALIDATED: HResult = 0x88890004_u32 as i32;
pub const AUDCLNT_E_NOT_STOPPED: HResult = 0x88890005_u32 as i32;
pub const AUDCLNT_E_BUFFER_TOO_LARGE: HResult = 0x88890006_u32 as i32;
pub const AUDCLNT_E_OUT_OF_ORDER: HResult = 0x88890007_u32 as i32;
pub const AUDCLNT_E_UNSUPPORTED_FORMAT: HResult = 0x88890008_u32 as i32;
pub const AUDCLNT_E_DEVICE_IN_USE: HResult = 0x8889000A_u32 as i32;

/// Reference time: 100-nanosecond units.
pub type ReferenceTime = i64;

/// 10 million 100-ns units = 1 second.
pub const REFTIMES_PER_SEC: ReferenceTime = 10_000_000;
pub const REFTIMES_PER_MILLISEC: ReferenceTime = 10_000;

// =============================================================================
// WAVEFORMATEX
// =============================================================================

/// WAVEFORMATEX structure.
#[repr(C, packed)]
#[derive(Debug, Clone, Copy)]
pub struct WaveFormatEx {
    pub format_tag: u16,
    pub channels: u16,
    pub samples_per_sec: u32,
    pub avg_bytes_per_sec: u32,
    pub block_align: u16,
    pub bits_per_sample: u16,
    pub cb_size: u16,
}

/// WAVE_FORMAT_PCM
pub const WAVE_FORMAT_PCM: u16 = 0x0001;
/// WAVE_FORMAT_IEEE_FLOAT
pub const WAVE_FORMAT_IEEE_FLOAT: u16 = 0x0003;
/// WAVE_FORMAT_EXTENSIBLE
pub const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;

impl Default for WaveFormatEx {
    fn default() -> Self {
        Self {
            format_tag: WAVE_FORMAT_PCM,
            channels: 2,
            samples_per_sec: 44100,
            avg_bytes_per_sec: 44100 * 2 * 2,
            block_align: 4,
            bits_per_sample: 16,
            cb_size: 0,
        }
    }
}

// =============================================================================
// Internal state
// =============================================================================

/// Audio client state.
struct AudioClientState {
    initialized: bool,
    running: bool,
    share_mode: ShareMode,
    format: WaveFormatEx,
    buffer_frames: u32,
    /// PCM sample ring buffer.
    buffer: Vec<u8>,
    /// Write position in the buffer.
    write_pos: usize,
    /// Frames currently filled but not yet played.
    pending_frames: u32,
}

/// Audio render client state (linked to an audio client).
struct AudioRenderClientState {
    audio_client: u64,
    /// Pointer to the buffer region returned by GetBuffer.
    current_buffer: Option<Vec<u8>>,
    current_frames: u32,
}

/// Device handle state.
struct DeviceState {
    data_flow: DataFlow,
    active: bool,
}

static DEVICE_TABLE: Mutex<Option<BTreeMap<u64, DeviceState>>> = Mutex::new(None);
static NEXT_DEVICE: Mutex<u64> = Mutex::new(0xA0D0_0000);

static CLIENT_TABLE: Mutex<Option<BTreeMap<u64, AudioClientState>>> = Mutex::new(None);
static NEXT_CLIENT: Mutex<u64> = Mutex::new(0xA0D1_0000);

static RENDER_TABLE: Mutex<Option<BTreeMap<u64, AudioRenderClientState>>> = Mutex::new(None);
static NEXT_RENDER: Mutex<u64> = Mutex::new(0xA0D2_0000);

/// Enumerator handle (singleton).
static ENUMERATOR_HANDLE: Mutex<u64> = Mutex::new(0);

const ENUM_BASE: u64 = 0xA0DE_0000;
const DEV_BASE: u64 = 0xA0D0_0000;
const CLI_BASE: u64 = 0xA0D1_0000;
const REN_BASE: u64 = 0xA0D2_0000;

// =============================================================================
// IMMDeviceEnumerator
// =============================================================================

/// Create the device enumerator (called via CoCreateInstance).
pub fn create_device_enumerator(ppv: *mut *mut core::ffi::c_void) -> HResult {
    if ppv.is_null() { return E_INVALIDARG; }

    let handle = {
        let mut h = ENUMERATOR_HANDLE.lock();
        if *h == 0 {
            *h = ENUM_BASE;
        }
        *h
    };

    log::info!("[wasapi] IMMDeviceEnumerator created: 0x{:X}", handle);
    unsafe { *ppv = handle as *mut core::ffi::c_void; }
    S_OK
}

/// IMMDeviceEnumerator::GetDefaultAudioEndpoint
pub fn get_default_audio_endpoint(
    _enumerator: u64,
    data_flow: u32,
    role: u32,
    device: *mut u64,
) -> HResult {
    if device.is_null() { return E_INVALIDARG; }

    let flow = match data_flow {
        0 => DataFlow::Render,
        1 => DataFlow::Capture,
        _ => DataFlow::All,
    };

    log::debug!("[wasapi] GetDefaultAudioEndpoint: flow={:?} role={}", flow, role);

    let mut counter = NEXT_DEVICE.lock();
    let handle = DEV_BASE + (*counter - DEV_BASE);
    *counter += 1;

    let mut table = DEVICE_TABLE.lock();
    if table.is_none() { *table = Some(BTreeMap::new()); }
    if let Some(ref mut map) = *table {
        map.insert(handle, DeviceState {
            data_flow: flow,
            active: true,
        });
    }

    unsafe { *device = handle; }
    S_OK
}

// =============================================================================
// IMMDevice
// =============================================================================

/// IMMDevice::Activate — activate an interface on the device.
pub fn device_activate(
    device: u64,
    iid: *const Guid,
    _cls_ctx: DWord,
    _activation_params: *mut core::ffi::c_void,
    interface: *mut *mut core::ffi::c_void,
) -> HResult {
    if iid.is_null() || interface.is_null() { return E_INVALIDARG; }

    let riid = unsafe { &*iid };

    // Check if requesting IAudioClient
    if ole32::guid_eq(riid, &IID_IAUDIO_CLIENT) {
        let mut counter = NEXT_CLIENT.lock();
        let handle = CLI_BASE + (*counter - CLI_BASE);
        *counter += 1;

        let mut table = CLIENT_TABLE.lock();
        if table.is_none() { *table = Some(BTreeMap::new()); }
        if let Some(ref mut map) = *table {
            map.insert(handle, AudioClientState {
                initialized: false,
                running: false,
                share_mode: ShareMode::Shared,
                format: WaveFormatEx::default(),
                buffer_frames: 0,
                buffer: Vec::new(),
                write_pos: 0,
                pending_frames: 0,
            });
        }

        log::debug!("[wasapi] Device::Activate IAudioClient -> 0x{:X}", handle);
        unsafe { *interface = handle as *mut core::ffi::c_void; }
        return S_OK;
    }

    log::warn!(
        "[wasapi] Device::Activate: unsupported IID {{{:08X}-{:04X}-{:04X}}}",
        riid.data1, riid.data2, riid.data3
    );
    E_NOTIMPL
}

// =============================================================================
// IAudioClient
// =============================================================================

/// IAudioClient::Initialize — set up the audio stream.
pub fn audio_client_initialize(
    client: u64,
    share_mode: u32,
    stream_flags: DWord,
    buffer_duration: ReferenceTime,
    periodicity: ReferenceTime,
    format: *const WaveFormatEx,
    _audio_session_guid: *const Guid,
) -> HResult {
    if format.is_null() { return E_INVALIDARG; }

    let fmt = unsafe { *format };

    // Validate format
    if fmt.format_tag != WAVE_FORMAT_PCM
        && fmt.format_tag != WAVE_FORMAT_IEEE_FLOAT
        && fmt.format_tag != WAVE_FORMAT_EXTENSIBLE
    {
        return AUDCLNT_E_UNSUPPORTED_FORMAT;
    }

    let mode = if share_mode == 0 { ShareMode::Shared } else { ShareMode::Exclusive };

    // Copy packed fields to locals to avoid unaligned references
    let fmt_tag = { fmt.format_tag };
    let fmt_channels = { fmt.channels };
    let fmt_samples_per_sec = { fmt.samples_per_sec };
    let fmt_bits_per_sample = { fmt.bits_per_sample };
    let fmt_block_align = { fmt.block_align };

    // Calculate buffer size in frames
    let frames = ((fmt_samples_per_sec as i64 * buffer_duration) / REFTIMES_PER_SEC) as u32;
    let frames = frames.max(256); // Minimum buffer size
    let buffer_bytes = frames as usize * fmt_block_align as usize;

    log::info!(
        "[wasapi] IAudioClient::Initialize: mode={:?} fmt=0x{:X} {}ch {}Hz {}bit buf={}frames",
        mode, fmt_tag, fmt_channels, fmt_samples_per_sec, fmt_bits_per_sample, frames
    );

    let mut table = CLIENT_TABLE.lock();
    if let Some(ref mut map) = *table {
        if let Some(state) = map.get_mut(&client) {
            if state.initialized {
                return AUDCLNT_E_ALREADY_INITIALIZED;
            }
            state.initialized = true;
            state.share_mode = mode;
            state.format = fmt;
            state.buffer_frames = frames;
            state.buffer = vec![0u8; buffer_bytes];
            state.write_pos = 0;
            state.pending_frames = 0;
            return S_OK;
        }
    }
    E_FAIL
}

/// IAudioClient::GetBufferSize — get the buffer capacity in frames.
pub fn audio_client_get_buffer_size(client: u64, num_frames: *mut u32) -> HResult {
    if num_frames.is_null() { return E_INVALIDARG; }

    let table = CLIENT_TABLE.lock();
    if let Some(state) = table.as_ref().and_then(|m| m.get(&client)) {
        if !state.initialized { return AUDCLNT_E_NOT_INITIALIZED; }
        unsafe { *num_frames = state.buffer_frames; }
        S_OK
    } else {
        E_FAIL
    }
}

/// IAudioClient::GetCurrentPadding — get frames of data currently buffered.
pub fn audio_client_get_current_padding(client: u64, padding: *mut u32) -> HResult {
    if padding.is_null() { return E_INVALIDARG; }

    let table = CLIENT_TABLE.lock();
    if let Some(state) = table.as_ref().and_then(|m| m.get(&client)) {
        if !state.initialized { return AUDCLNT_E_NOT_INITIALIZED; }
        unsafe { *padding = state.pending_frames; }
        S_OK
    } else {
        E_FAIL
    }
}

/// IAudioClient::GetMixFormat — get the device's preferred format.
pub fn audio_client_get_mix_format(client: u64, format: *mut *mut WaveFormatEx) -> HResult {
    if format.is_null() { return E_INVALIDARG; }

    // Return a standard 44100 Hz stereo 16-bit PCM format
    let fmt = alloc::boxed::Box::new(WaveFormatEx {
        format_tag: WAVE_FORMAT_PCM,
        channels: 2,
        samples_per_sec: 44100,
        avg_bytes_per_sec: 44100 * 4,
        block_align: 4,
        bits_per_sample: 16,
        cb_size: 0,
    });

    unsafe { *format = alloc::boxed::Box::into_raw(fmt); }
    S_OK
}

/// IAudioClient::GetService — get an audio service interface.
pub fn audio_client_get_service(
    client: u64,
    riid: *const Guid,
    ppv: *mut *mut core::ffi::c_void,
) -> HResult {
    if riid.is_null() || ppv.is_null() { return E_INVALIDARG; }

    let iid = unsafe { &*riid };

    if ole32::guid_eq(iid, &IID_IAUDIO_RENDER_CLIENT) {
        let mut counter = NEXT_RENDER.lock();
        let handle = REN_BASE + (*counter - REN_BASE);
        *counter += 1;

        let mut table = RENDER_TABLE.lock();
        if table.is_none() { *table = Some(BTreeMap::new()); }
        if let Some(ref mut map) = *table {
            map.insert(handle, AudioRenderClientState {
                audio_client: client,
                current_buffer: None,
                current_frames: 0,
            });
        }

        log::debug!("[wasapi] IAudioClient::GetService IAudioRenderClient -> 0x{:X}", handle);
        unsafe { *ppv = handle as *mut core::ffi::c_void; }
        return S_OK;
    }

    log::warn!(
        "[wasapi] IAudioClient::GetService: unsupported IID {{{:08X}-{:04X}-{:04X}}}",
        iid.data1, iid.data2, iid.data3
    );
    E_NOTIMPL
}

/// IAudioClient::Start — start the audio stream.
pub fn audio_client_start(client: u64) -> HResult {
    let mut table = CLIENT_TABLE.lock();
    if let Some(ref mut map) = *table {
        if let Some(state) = map.get_mut(&client) {
            if !state.initialized { return AUDCLNT_E_NOT_INITIALIZED; }
            state.running = true;
            log::info!("[wasapi] IAudioClient::Start");
            return S_OK;
        }
    }
    E_FAIL
}

/// IAudioClient::Stop — stop the audio stream.
pub fn audio_client_stop(client: u64) -> HResult {
    let mut table = CLIENT_TABLE.lock();
    if let Some(ref mut map) = *table {
        if let Some(state) = map.get_mut(&client) {
            if !state.initialized { return AUDCLNT_E_NOT_INITIALIZED; }
            state.running = false;
            log::info!("[wasapi] IAudioClient::Stop");
            return S_OK;
        }
    }
    E_FAIL
}

// =============================================================================
// IAudioRenderClient
// =============================================================================

/// IAudioRenderClient::GetBuffer — get a pointer to a buffer region.
pub fn render_client_get_buffer(
    render_client: u64,
    num_frames: u32,
    data: *mut *mut u8,
) -> HResult {
    if data.is_null() { return E_INVALIDARG; }

    // Get the associated audio client's block_align
    let block_align = {
        let rtable = RENDER_TABLE.lock();
        let client_handle = rtable.as_ref()
            .and_then(|m| m.get(&render_client))
            .map(|r| r.audio_client);

        if let Some(ch) = client_handle {
            let ctable = CLIENT_TABLE.lock();
            ctable.as_ref()
                .and_then(|m| m.get(&ch))
                .map(|c| c.format.block_align)
                .unwrap_or(4)
        } else {
            4
        }
    };

    let buffer_size = num_frames as usize * block_align as usize;
    let buffer = vec![0u8; buffer_size];
    let ptr = buffer.as_ptr() as *mut u8;

    let mut table = RENDER_TABLE.lock();
    if let Some(ref mut map) = *table {
        if let Some(state) = map.get_mut(&render_client) {
            state.current_buffer = Some(buffer);
            state.current_frames = num_frames;
            unsafe { *data = ptr; }
            return S_OK;
        }
    }
    E_FAIL
}

/// IAudioRenderClient::ReleaseBuffer — release the buffer and queue for playback.
pub fn render_client_release_buffer(
    render_client: u64,
    num_frames: u32,
    flags: DWord,
) -> HResult {
    let client_handle = {
        let mut table = RENDER_TABLE.lock();
        if let Some(ref mut map) = *table {
            if let Some(state) = map.get_mut(&render_client) {
                let ch = state.audio_client;
                // Take the buffer
                let _buffer = state.current_buffer.take();
                state.current_frames = 0;

                // If AUDCLNT_BUFFERFLAGS_SILENT, the buffer is all zeros (silence).
                if flags & AUDCLNT_BUFFERFLAGS_SILENT != 0 {
                    log::trace!("[wasapi] ReleaseBuffer: {} frames (silent)", num_frames);
                } else {
                    log::trace!("[wasapi] ReleaseBuffer: {} frames", num_frames);
                }

                // In a real implementation, we'd copy the audio data to the
                // HDA ring buffer for DMA playback. For now, we just update
                // the pending frames counter.
                Some(ch)
            } else {
                None
            }
        } else {
            None
        }
    };

    if let Some(ch) = client_handle {
        let mut table = CLIENT_TABLE.lock();
        if let Some(ref mut map) = *table {
            if let Some(state) = map.get_mut(&ch) {
                state.pending_frames = state.pending_frames.saturating_add(num_frames);
                // Simulate consumption: drain frames as if played
                if state.running && state.pending_frames > state.buffer_frames / 2 {
                    state.pending_frames = state.pending_frames.saturating_sub(num_frames);
                }
            }
        }
        S_OK
    } else {
        E_FAIL
    }
}

/// Check if the CLSID matches IMMDeviceEnumerator (for CoCreateInstance routing).
pub fn is_device_enumerator_clsid(clsid: &Guid) -> bool {
    ole32::guid_eq(clsid, &CLSID_MM_DEVICE_ENUMERATOR)
}
