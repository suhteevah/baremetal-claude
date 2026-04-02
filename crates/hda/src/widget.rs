//! HDA widget (node) discovery and configuration.
//!
//! The HDA codec is organized as a tree: root node (NID 0) contains
//! function group nodes, each of which contains widget nodes.
//! Per Intel HDA spec 1.0a, Section 7.

use alloc::vec::Vec;
use log::{debug, info, trace, warn};

use crate::codec::*;

// =============================================================================
// Parameter IDs (Section 7.3.4)
// =============================================================================

/// Vendor ID (32-bit: vendor[31:16] | device[15:0])
pub const PARAM_VENDOR_ID: u8 = 0x00;
/// Revision ID
pub const PARAM_REVISION_ID: u8 = 0x02;
/// Subordinate Node Count: start_nid[23:16] | total_nodes[7:0]
pub const PARAM_SUBORDINATE_NODE_COUNT: u8 = 0x04;
/// Function Group Type
pub const PARAM_FUNCTION_GROUP_TYPE: u8 = 0x05;
/// Audio Function Group Capabilities
pub const PARAM_AUDIO_FG_CAP: u8 = 0x08;
/// Audio Widget Capabilities
pub const PARAM_AUDIO_WIDGET_CAP: u8 = 0x09;
/// Supported PCM Size/Rates
pub const PARAM_SUPPORTED_PCM: u8 = 0x0A;
/// Supported Stream Formats
pub const PARAM_SUPPORTED_STREAM_FORMATS: u8 = 0x0B;
/// Pin Capabilities
pub const PARAM_PIN_CAP: u8 = 0x0C;
/// Input Amplifier Capabilities
pub const PARAM_AMP_IN_CAP: u8 = 0x0D;
/// Output Amplifier Capabilities
pub const PARAM_AMP_OUT_CAP: u8 = 0x0E;
/// Connection List Length
pub const PARAM_CONN_LIST_LENGTH: u8 = 0x0F;
/// Supported Power States
pub const PARAM_SUPPORTED_POWER_STATES: u8 = 0x10;
/// Processing Capabilities
pub const PARAM_PROCESSING_CAP: u8 = 0x11;
/// GPIO Count
pub const PARAM_GPIO_COUNT: u8 = 0x12;
/// Volume Knob Capabilities
pub const PARAM_VOLUME_KNOB_CAP: u8 = 0x13;

// =============================================================================
// Widget types (Audio Widget Capabilities bits 23:20, Section 7.3.4.6)
// =============================================================================

/// Widget type enumeration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum WidgetType {
    AudioOutput = 0x0,
    AudioInput = 0x1,
    AudioMixer = 0x2,
    AudioSelector = 0x3,
    PinComplex = 0x4,
    PowerWidget = 0x5,
    VolumeKnob = 0x6,
    BeepGenerator = 0x7,
    VendorDefined = 0xF,
    Unknown = 0xFF,
}

impl WidgetType {
    /// Decode widget type from the Audio Widget Capabilities parameter.
    pub fn from_caps(caps: u32) -> Self {
        match (caps >> 20) & 0x0F {
            0x0 => WidgetType::AudioOutput,
            0x1 => WidgetType::AudioInput,
            0x2 => WidgetType::AudioMixer,
            0x3 => WidgetType::AudioSelector,
            0x4 => WidgetType::PinComplex,
            0x5 => WidgetType::PowerWidget,
            0x6 => WidgetType::VolumeKnob,
            0x7 => WidgetType::BeepGenerator,
            0xF => WidgetType::VendorDefined,
            _ => WidgetType::Unknown,
        }
    }
}

// =============================================================================
// Audio Widget Capabilities bit fields (Section 7.3.4.6)
// =============================================================================

/// Stereo: widget supports stereo (bit 0)
pub const WIDGET_CAP_STEREO: u32 = 1 << 0;
/// InAmpPresent: input amplifier present (bit 1)
pub const WIDGET_CAP_IN_AMP: u32 = 1 << 1;
/// OutAmpPresent: output amplifier present (bit 2)
pub const WIDGET_CAP_OUT_AMP: u32 = 1 << 2;
/// AmpParamOverride: amp param override (bit 3)
pub const WIDGET_CAP_AMP_OVERRIDE: u32 = 1 << 3;
/// FormatOverride: format override (bit 4)
pub const WIDGET_CAP_FORMAT_OVERRIDE: u32 = 1 << 4;
/// Stripe: stripe support (bit 5)
pub const WIDGET_CAP_STRIPE: u32 = 1 << 5;
/// ProcWidget: processing widget (bit 6)
pub const WIDGET_CAP_PROC: u32 = 1 << 6;
/// Unsolicited: unsolicited response capable (bit 7)
pub const WIDGET_CAP_UNSOL: u32 = 1 << 7;
/// ConnList: has connection list (bit 8)
pub const WIDGET_CAP_CONN_LIST: u32 = 1 << 8;
/// Digital: digital capable (bit 9)
pub const WIDGET_CAP_DIGITAL: u32 = 1 << 9;
/// Power Control: power state control (bit 10)
pub const WIDGET_CAP_POWER: u32 = 1 << 10;
/// L-R Swap: left-right swap (bit 11)
pub const WIDGET_CAP_LR_SWAP: u32 = 1 << 11;
/// CP Caps: content protection (bit 12)
pub const WIDGET_CAP_CP: u32 = 1 << 12;
/// Channel Count extension (bits 15:13)
pub const WIDGET_CAP_CHAN_EXT_SHIFT: u32 = 13;
/// Delay (bits 19:16)
pub const WIDGET_CAP_DELAY_SHIFT: u32 = 16;
/// Type (bits 23:20)
pub const WIDGET_CAP_TYPE_SHIFT: u32 = 20;

// =============================================================================
// Pin Capabilities (Section 7.3.4.9)
// =============================================================================

/// Impedance Sense Capable (bit 0)
pub const PIN_CAP_IMPEDANCE: u32 = 1 << 0;
/// Trigger Required (bit 1)
pub const PIN_CAP_TRIGGER: u32 = 1 << 1;
/// Presence Detect Capable (bit 2)
pub const PIN_CAP_PRESENCE: u32 = 1 << 2;
/// Headphone Drive Capable (bit 3)
pub const PIN_CAP_HP_DRIVE: u32 = 1 << 3;
/// Output Capable (bit 4)
pub const PIN_CAP_OUTPUT: u32 = 1 << 4;
/// Input Capable (bit 5)
pub const PIN_CAP_INPUT: u32 = 1 << 5;
/// Balanced I/O (bit 6)
pub const PIN_CAP_BALANCED: u32 = 1 << 6;
/// HDMI (bit 7)
pub const PIN_CAP_HDMI: u32 = 1 << 7;
/// EAPD Capable (bit 16)
pub const PIN_CAP_EAPD: u32 = 1 << 16;

// =============================================================================
// Pin Configuration Default (Section 7.3.3.31)
// =============================================================================

/// Decoded pin configuration default register.
#[derive(Debug, Clone)]
pub struct PinConfig {
    /// Sequence (bits 3:0)
    pub sequence: u8,
    /// Default Association (bits 7:4)
    pub association: u8,
    /// Misc (bit 8)
    pub misc: u8,
    /// Color (bits 15:12)
    pub color: u8,
    /// Connection Type (bits 19:16)
    pub connection_type: u8,
    /// Default Device (bits 23:20)
    pub default_device: u8,
    /// Location (bits 29:24)
    pub location: u8,
    /// Port Connectivity (bits 31:30)
    pub port_connectivity: u8,
}

impl PinConfig {
    /// Decode a 32-bit pin configuration default value.
    pub fn decode(val: u32) -> Self {
        PinConfig {
            sequence: (val & 0x0F) as u8,
            association: ((val >> 4) & 0x0F) as u8,
            misc: ((val >> 8) & 0x0F) as u8,
            color: ((val >> 12) & 0x0F) as u8,
            connection_type: ((val >> 16) & 0x0F) as u8,
            default_device: ((val >> 20) & 0x0F) as u8,
            location: ((val >> 24) & 0x3F) as u8,
            port_connectivity: ((val >> 30) & 0x03) as u8,
        }
    }

    /// Returns true if this pin is not connected (port connectivity = 0x01 no connection).
    pub fn is_no_connection(&self) -> bool {
        self.port_connectivity == 0x01
    }

    /// Default device type constants.
    pub fn device_name(&self) -> &'static str {
        match self.default_device {
            0x0 => "Line Out",
            0x1 => "Speaker",
            0x2 => "HP Out",
            0x3 => "CD",
            0x4 => "SPDIF Out",
            0x5 => "Digital Other Out",
            0x6 => "Modem Line Side",
            0x7 => "Modem Handset Side",
            0x8 => "Line In",
            0x9 => "AUX",
            0xA => "Mic In",
            0xB => "Telephony",
            0xC => "SPDIF In",
            0xD => "Digital Other In",
            0xF => "Other",
            _ => "Unknown",
        }
    }
}

// =============================================================================
// Widget node descriptor
// =============================================================================

/// Describes a single widget node in the codec tree.
#[derive(Debug, Clone)]
pub struct Widget {
    /// Node ID within the codec.
    pub nid: u8,
    /// Widget type.
    pub widget_type: WidgetType,
    /// Raw Audio Widget Capabilities parameter.
    pub caps: u32,
    /// Pin configuration (only for PinComplex widgets).
    pub pin_config: Option<PinConfig>,
    /// Pin capabilities (only for PinComplex widgets).
    pub pin_caps: u32,
    /// Connection list (NIDs this widget can receive from).
    pub connections: Vec<u8>,
    /// Output amplifier capabilities.
    pub amp_out_caps: u32,
    /// Input amplifier capabilities.
    pub amp_in_caps: u32,
}

// =============================================================================
// Codec descriptor
// =============================================================================

/// Describes a discovered codec and its widget tree.
#[derive(Debug, Clone)]
pub struct Codec {
    /// Codec address (0-14).
    pub address: u8,
    /// Vendor ID.
    pub vendor_id: u16,
    /// Device ID.
    pub device_id: u16,
    /// All widgets in this codec.
    pub widgets: Vec<Widget>,
    /// The audio function group NID (usually 0x01).
    pub afg_nid: u8,
}

// =============================================================================
// Widget tree traversal
// =============================================================================

/// Discover all codecs connected to the controller.
///
/// Checks STATESTS to find which codec addresses responded during controller reset.
///
/// # Safety
/// `corb_rirb` must have a valid BAR0 and be fully initialized.
pub unsafe fn discover_codecs(corb_rirb: &mut CorbRirb, bar0: usize) -> Vec<Codec> {
    let statests = unsafe { crate::registers::read16(bar0, crate::registers::STATESTS) };
    info!("hda: STATESTS=0x{:04x} (codec presence)", statests);

    let mut codecs = Vec::new();

    for addr in 0u8..15 {
        if statests & (1 << addr) == 0 {
            continue;
        }

        info!("hda: codec {} detected, enumerating...", addr);

        // Get vendor/device ID from root node (NID 0)
        let vendor_device = unsafe { corb_rirb.get_parameter(addr, 0, PARAM_VENDOR_ID) };
        let (vendor_id, device_id) = match vendor_device {
            Some(val) => {
                let vid = (val >> 16) as u16;
                let did = (val & 0xFFFF) as u16;
                info!(
                    "hda: codec {}: vendor=0x{:04x} device=0x{:04x}",
                    addr, vid, did
                );
                (vid, did)
            }
            None => {
                warn!("hda: codec {}: failed to read vendor ID, skipping", addr);
                continue;
            }
        };

        // Get subordinate node count from root node to find function groups
        let sub_nodes = unsafe {
            corb_rirb.get_parameter(addr, 0, PARAM_SUBORDINATE_NODE_COUNT)
        };
        let (fg_start, fg_count) = match sub_nodes {
            Some(val) => {
                let start = ((val >> 16) & 0xFF) as u8;
                let count = (val & 0xFF) as u8;
                debug!(
                    "hda: codec {}: function groups start={} count={}",
                    addr, start, count
                );
                (start, count)
            }
            None => {
                warn!("hda: codec {}: failed to read subordinate nodes", addr);
                continue;
            }
        };

        let mut afg_nid = 0u8;
        let mut widgets = Vec::new();

        // Iterate function groups
        for fg_idx in 0..fg_count {
            let fg_nid = fg_start + fg_idx;
            let fg_type = unsafe {
                corb_rirb.get_parameter(addr, fg_nid, PARAM_FUNCTION_GROUP_TYPE)
            };

            match fg_type {
                Some(val) => {
                    let node_type = val & 0xFF;
                    debug!(
                        "hda: codec {}: function group NID={} type=0x{:02x}",
                        addr, fg_nid, node_type
                    );
                    if node_type == 0x01 {
                        // Audio Function Group
                        afg_nid = fg_nid;
                        info!(
                            "hda: codec {}: Audio Function Group at NID={}",
                            addr, fg_nid
                        );

                        // Power on the AFG
                        unsafe {
                            corb_rirb.set_verb(addr, fg_nid, VERB_SET_POWER_STATE, 0x00);
                        }
                        debug!("hda: codec {}: AFG powered on (D0)", addr);

                        // Enumerate widgets in this function group
                        let w = unsafe {
                            enumerate_widgets(corb_rirb, addr, fg_nid)
                        };
                        widgets = w;
                    }
                }
                None => {
                    warn!(
                        "hda: codec {}: failed to read FG type for NID={}",
                        addr, fg_nid
                    );
                }
            }
        }

        codecs.push(Codec {
            address: addr,
            vendor_id,
            device_id,
            widgets,
            afg_nid,
        });
    }

    info!("hda: discovered {} codec(s)", codecs.len());
    codecs
}

/// Enumerate all widgets under a function group.
///
/// # Safety
/// `corb_rirb` must have a valid BAR0.
unsafe fn enumerate_widgets(
    corb_rirb: &mut CorbRirb,
    codec: u8,
    fg_nid: u8,
) -> Vec<Widget> {
    let sub_nodes = unsafe {
        corb_rirb.get_parameter(codec, fg_nid, PARAM_SUBORDINATE_NODE_COUNT)
    };

    let (start_nid, count) = match sub_nodes {
        Some(val) => {
            let start = ((val >> 16) & 0xFF) as u8;
            let count = (val & 0xFF) as u8;
            debug!(
                "hda: codec {}: AFG NID={} has {} widgets starting at NID={}",
                codec, fg_nid, count, start
            );
            (start, count)
        }
        None => {
            warn!(
                "hda: codec {}: failed to read widget count for AFG NID={}",
                codec, fg_nid
            );
            return Vec::new();
        }
    };

    let mut widgets = Vec::with_capacity(count as usize);

    for i in 0..count {
        let nid = start_nid + i;

        // Get Audio Widget Capabilities
        let caps = unsafe {
            corb_rirb.get_parameter(codec, nid, PARAM_AUDIO_WIDGET_CAP)
        }
        .unwrap_or(0);

        let widget_type = WidgetType::from_caps(caps);
        trace!(
            "hda: codec {}: NID={} type={:?} caps=0x{:08x}",
            codec, nid, widget_type, caps
        );

        // Get pin config for pin complex widgets
        let pin_config = if widget_type == WidgetType::PinComplex {
            let cfg = unsafe {
                corb_rirb.get_verb(codec, nid, VERB_GET_CONFIG_DEFAULT, 0)
            };
            match cfg {
                Some(val) => {
                    let pc = PinConfig::decode(val);
                    debug!(
                        "hda: codec {}: NID={} pin: device={}, connectivity={}, assoc={}, seq={}",
                        codec, nid, pc.device_name(), pc.port_connectivity,
                        pc.association, pc.sequence
                    );
                    Some(pc)
                }
                None => None,
            }
        } else {
            None
        };

        // Get pin capabilities for pin complex widgets
        let pin_caps = if widget_type == WidgetType::PinComplex {
            unsafe {
                corb_rirb.get_parameter(codec, nid, PARAM_PIN_CAP)
            }
            .unwrap_or(0)
        } else {
            0
        };

        // Get connection list
        let connections = if caps & WIDGET_CAP_CONN_LIST != 0 {
            unsafe { read_connection_list(corb_rirb, codec, nid) }
        } else {
            Vec::new()
        };

        // Get amp capabilities
        let amp_out_caps = if caps & WIDGET_CAP_OUT_AMP != 0 {
            if caps & WIDGET_CAP_AMP_OVERRIDE != 0 {
                unsafe {
                    corb_rirb.get_parameter(codec, nid, PARAM_AMP_OUT_CAP)
                }
                .unwrap_or(0)
            } else {
                // Use function group default
                unsafe {
                    corb_rirb.get_parameter(codec, fg_nid, PARAM_AMP_OUT_CAP)
                }
                .unwrap_or(0)
            }
        } else {
            0
        };

        let amp_in_caps = if caps & WIDGET_CAP_IN_AMP != 0 {
            if caps & WIDGET_CAP_AMP_OVERRIDE != 0 {
                unsafe {
                    corb_rirb.get_parameter(codec, nid, PARAM_AMP_IN_CAP)
                }
                .unwrap_or(0)
            } else {
                unsafe {
                    corb_rirb.get_parameter(codec, fg_nid, PARAM_AMP_IN_CAP)
                }
                .unwrap_or(0)
            }
        } else {
            0
        };

        widgets.push(Widget {
            nid,
            widget_type,
            caps,
            pin_config,
            pin_caps,
            connections,
            amp_out_caps,
            amp_in_caps,
        });
    }

    info!(
        "hda: codec {}: enumerated {} widgets",
        codec,
        widgets.len()
    );
    widgets
}

/// Read the connection list for a widget.
///
/// # Safety
/// `corb_rirb` must have a valid BAR0.
unsafe fn read_connection_list(
    corb_rirb: &mut CorbRirb,
    codec: u8,
    nid: u8,
) -> Vec<u8> {
    let len_raw = unsafe {
        corb_rirb.get_parameter(codec, nid, PARAM_CONN_LIST_LENGTH)
    }
    .unwrap_or(0);

    let long_form = (len_raw & (1 << 7)) != 0;
    let count = (len_raw & 0x7F) as usize;

    if count == 0 {
        return Vec::new();
    }

    trace!(
        "hda: codec {}: NID={} connection list: count={}, long_form={}",
        codec, nid, count, long_form
    );

    let mut connections = Vec::with_capacity(count);

    if long_form {
        // Long form: 2 entries per response (16 bits each)
        let mut offset = 0u8;
        while connections.len() < count {
            let resp = unsafe {
                corb_rirb.get_verb(codec, nid, VERB_GET_CONN_LIST, offset)
            }
            .unwrap_or(0);

            let nid0 = (resp & 0xFFFF) as u8;
            let nid1 = ((resp >> 16) & 0xFFFF) as u8;

            connections.push(nid0);
            if connections.len() < count {
                connections.push(nid1);
            }
            offset += 2;
        }
    } else {
        // Short form: 4 entries per response (8 bits each)
        let mut offset = 0u8;
        while connections.len() < count {
            let resp = unsafe {
                corb_rirb.get_verb(codec, nid, VERB_GET_CONN_LIST, offset)
            }
            .unwrap_or(0);

            for shift in [0, 8, 16, 24] {
                if connections.len() >= count {
                    break;
                }
                connections.push(((resp >> shift) & 0xFF) as u8);
            }
            offset += 4;
        }
    }

    trace!(
        "hda: codec {}: NID={} connections: {:?}",
        codec,
        nid,
        connections
    );
    connections
}

/// Find an audio output path: DAC -> [Mixer/Selector] -> Pin (speaker/headphone).
///
/// Returns a list of (NID, WidgetType) from DAC to pin, or empty if not found.
pub fn find_output_path(codec: &Codec) -> Vec<(u8, WidgetType)> {
    info!(
        "hda: searching for audio output path in codec {}",
        codec.address
    );

    // Find output-capable pins (speakers, headphones, line out)
    let output_pins: Vec<&Widget> = codec
        .widgets
        .iter()
        .filter(|w| {
            w.widget_type == WidgetType::PinComplex
                && w.pin_caps & PIN_CAP_OUTPUT != 0
                && w.pin_config.as_ref().is_some_and(|pc| !pc.is_no_connection())
        })
        .collect();

    debug!(
        "hda: found {} output-capable pins",
        output_pins.len()
    );

    for pin in &output_pins {
        let device_name = pin
            .pin_config
            .as_ref()
            .map(|pc| pc.device_name())
            .unwrap_or("unknown");
        debug!(
            "hda: trying output pin NID={} ({})",
            pin.nid, device_name
        );

        // Walk backwards from pin through connection list to find a DAC
        let mut path = Vec::new();
        if trace_path_to_dac(codec, pin.nid, &mut path, 0) {
            path.reverse();
            info!("hda: found output path: {:?}", path);
            return path;
        }
    }

    warn!("hda: no audio output path found");
    Vec::new()
}

/// Recursively trace from a widget back to a DAC via connection lists.
///
/// Returns true if a DAC was found. `path` accumulates (nid, type) pairs
/// in reverse order (pin first, DAC last).
fn trace_path_to_dac(
    codec: &Codec,
    nid: u8,
    path: &mut Vec<(u8, WidgetType)>,
    depth: usize,
) -> bool {
    if depth > 16 {
        warn!("hda: path trace exceeded max depth at NID={}", nid);
        return false;
    }

    let widget = match codec.widgets.iter().find(|w| w.nid == nid) {
        Some(w) => w,
        None => return false,
    };

    path.push((nid, widget.widget_type));

    if widget.widget_type == WidgetType::AudioOutput {
        trace!("hda: reached DAC at NID={}", nid);
        return true;
    }

    // Follow connection list
    for &conn_nid in &widget.connections {
        if trace_path_to_dac(codec, conn_nid, path, depth + 1) {
            return true;
        }
    }

    path.pop();
    false
}
