//! XInput game controller input API implementation.
//!
//! Maps XInputGetState, XInputSetState, XInputGetCapabilities to USB HID
//! gamepad if detected, otherwise returns ERROR_DEVICE_NOT_CONNECTED.

use spin::Mutex;

use crate::unicode::DWord;

// =============================================================================
// Constants
// =============================================================================

/// Maximum number of XInput controllers.
pub const XUSER_MAX_COUNT: DWord = 4;

/// Error codes.
pub const ERROR_SUCCESS: DWord = 0;
pub const ERROR_DEVICE_NOT_CONNECTED: DWord = 1167;

/// XINPUT_GAMEPAD button flags.
pub const XINPUT_GAMEPAD_DPAD_UP: u16 = 0x0001;
pub const XINPUT_GAMEPAD_DPAD_DOWN: u16 = 0x0002;
pub const XINPUT_GAMEPAD_DPAD_LEFT: u16 = 0x0004;
pub const XINPUT_GAMEPAD_DPAD_RIGHT: u16 = 0x0008;
pub const XINPUT_GAMEPAD_START: u16 = 0x0010;
pub const XINPUT_GAMEPAD_BACK: u16 = 0x0020;
pub const XINPUT_GAMEPAD_LEFT_THUMB: u16 = 0x0040;
pub const XINPUT_GAMEPAD_RIGHT_THUMB: u16 = 0x0080;
pub const XINPUT_GAMEPAD_LEFT_SHOULDER: u16 = 0x0100;
pub const XINPUT_GAMEPAD_RIGHT_SHOULDER: u16 = 0x0200;
pub const XINPUT_GAMEPAD_A: u16 = 0x1000;
pub const XINPUT_GAMEPAD_B: u16 = 0x2000;
pub const XINPUT_GAMEPAD_X: u16 = 0x4000;
pub const XINPUT_GAMEPAD_Y: u16 = 0x8000;

/// XINPUT_DEVTYPE
pub const XINPUT_DEVTYPE_GAMEPAD: u8 = 0x01;

/// XINPUT_DEVSUBTYPE
pub const XINPUT_DEVSUBTYPE_GAMEPAD: u8 = 0x01;
pub const XINPUT_DEVSUBTYPE_WHEEL: u8 = 0x02;
pub const XINPUT_DEVSUBTYPE_ARCADE_STICK: u8 = 0x03;
pub const XINPUT_DEVSUBTYPE_FLIGHT_STICK: u8 = 0x04;
pub const XINPUT_DEVSUBTYPE_DANCE_PAD: u8 = 0x05;
pub const XINPUT_DEVSUBTYPE_GUITAR: u8 = 0x06;
pub const XINPUT_DEVSUBTYPE_DRUM_KIT: u8 = 0x08;

/// XINPUT_CAPS flags.
pub const XINPUT_CAPS_FFB_SUPPORTED: u16 = 0x0001;
pub const XINPUT_CAPS_WIRELESS: u16 = 0x0002;
pub const XINPUT_CAPS_PMD_SUPPORTED: u16 = 0x0008;
pub const XINPUT_CAPS_NO_NAVIGATION: u16 = 0x0010;

/// Deadzone thresholds.
pub const XINPUT_GAMEPAD_LEFT_THUMB_DEADZONE: i16 = 7849;
pub const XINPUT_GAMEPAD_RIGHT_THUMB_DEADZONE: i16 = 8689;
pub const XINPUT_GAMEPAD_TRIGGER_THRESHOLD: u8 = 30;

/// XInputGetState flag for guide button.
pub const XINPUT_FLAG_GAMEPAD: DWord = 0x00000001;

// =============================================================================
// Structures
// =============================================================================

/// XINPUT_GAMEPAD
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct XInputGamepad {
    pub w_buttons: u16,
    pub b_left_trigger: u8,
    pub b_right_trigger: u8,
    pub s_thumb_lx: i16,
    pub s_thumb_ly: i16,
    pub s_thumb_rx: i16,
    pub s_thumb_ry: i16,
}

/// XINPUT_STATE
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct XInputState {
    pub dw_packet_number: DWord,
    pub gamepad: XInputGamepad,
}

/// XINPUT_VIBRATION
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct XInputVibration {
    pub w_left_motor_speed: u16,
    pub w_right_motor_speed: u16,
}

/// XINPUT_CAPABILITIES
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct XInputCapabilities {
    pub dev_type: u8,
    pub dev_sub_type: u8,
    pub flags: u16,
    pub gamepad: XInputGamepad,
    pub vibration: XInputVibration,
}

/// XINPUT_BATTERY_INFORMATION
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct XInputBatteryInformation {
    pub battery_type: u8,
    pub battery_level: u8,
}

/// XINPUT_KEYSTROKE
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct XInputKeystroke {
    pub virtual_key: u16,
    pub unicode: u16,
    pub flags: u16,
    pub user_index: u8,
    pub hid_code: u8,
}

// =============================================================================
// Internal state
// =============================================================================

/// Per-controller state.
struct ControllerState {
    connected: bool,
    packet_number: DWord,
    gamepad: XInputGamepad,
    vibration: XInputVibration,
}

impl Default for ControllerState {
    fn default() -> Self {
        Self {
            connected: false,
            packet_number: 0,
            gamepad: XInputGamepad::default(),
            vibration: XInputVibration::default(),
        }
    }
}

/// Global controller states for up to 4 controllers.
static CONTROLLERS: Mutex<[ControllerState; 4]> = Mutex::new([
    ControllerState { connected: false, packet_number: 0, gamepad: XInputGamepad { w_buttons: 0, b_left_trigger: 0, b_right_trigger: 0, s_thumb_lx: 0, s_thumb_ly: 0, s_thumb_rx: 0, s_thumb_ry: 0 }, vibration: XInputVibration { w_left_motor_speed: 0, w_right_motor_speed: 0 } },
    ControllerState { connected: false, packet_number: 0, gamepad: XInputGamepad { w_buttons: 0, b_left_trigger: 0, b_right_trigger: 0, s_thumb_lx: 0, s_thumb_ly: 0, s_thumb_rx: 0, s_thumb_ry: 0 }, vibration: XInputVibration { w_left_motor_speed: 0, w_right_motor_speed: 0 } },
    ControllerState { connected: false, packet_number: 0, gamepad: XInputGamepad { w_buttons: 0, b_left_trigger: 0, b_right_trigger: 0, s_thumb_lx: 0, s_thumb_ly: 0, s_thumb_rx: 0, s_thumb_ry: 0 }, vibration: XInputVibration { w_left_motor_speed: 0, w_right_motor_speed: 0 } },
    ControllerState { connected: false, packet_number: 0, gamepad: XInputGamepad { w_buttons: 0, b_left_trigger: 0, b_right_trigger: 0, s_thumb_lx: 0, s_thumb_ly: 0, s_thumb_rx: 0, s_thumb_ry: 0 }, vibration: XInputVibration { w_left_motor_speed: 0, w_right_motor_speed: 0 } },
]);

// =============================================================================
// Public API
// =============================================================================

/// XInputGetState — get the current state of a controller.
pub fn xinput_get_state(dw_user_index: DWord, state: *mut XInputState) -> DWord {
    if dw_user_index >= XUSER_MAX_COUNT || state.is_null() {
        return ERROR_DEVICE_NOT_CONNECTED;
    }

    let controllers = CONTROLLERS.lock();
    let ctrl = &controllers[dw_user_index as usize];

    if !ctrl.connected {
        return ERROR_DEVICE_NOT_CONNECTED;
    }

    unsafe {
        (*state).dw_packet_number = ctrl.packet_number;
        (*state).gamepad = ctrl.gamepad;
    }

    log::trace!(
        "[xinput] GetState({}): buttons=0x{:04X} LT={} RT={} LX={} LY={} RX={} RY={}",
        dw_user_index,
        ctrl.gamepad.w_buttons,
        ctrl.gamepad.b_left_trigger,
        ctrl.gamepad.b_right_trigger,
        ctrl.gamepad.s_thumb_lx,
        ctrl.gamepad.s_thumb_ly,
        ctrl.gamepad.s_thumb_rx,
        ctrl.gamepad.s_thumb_ry,
    );

    ERROR_SUCCESS
}

/// XInputSetState — set the vibration state of a controller.
pub fn xinput_set_state(dw_user_index: DWord, vibration: *mut XInputVibration) -> DWord {
    if dw_user_index >= XUSER_MAX_COUNT || vibration.is_null() {
        return ERROR_DEVICE_NOT_CONNECTED;
    }

    let mut controllers = CONTROLLERS.lock();
    let ctrl = &mut controllers[dw_user_index as usize];

    if !ctrl.connected {
        return ERROR_DEVICE_NOT_CONNECTED;
    }

    ctrl.vibration = unsafe { *vibration };

    log::trace!(
        "[xinput] SetState({}): left_motor={} right_motor={}",
        dw_user_index,
        ctrl.vibration.w_left_motor_speed,
        ctrl.vibration.w_right_motor_speed,
    );

    // In a real implementation, we'd send the vibration command to the
    // USB HID gamepad. For now, just store the value.
    ERROR_SUCCESS
}

/// XInputGetCapabilities — get the capabilities of a controller.
pub fn xinput_get_capabilities(
    dw_user_index: DWord,
    dw_flags: DWord,
    capabilities: *mut XInputCapabilities,
) -> DWord {
    if dw_user_index >= XUSER_MAX_COUNT || capabilities.is_null() {
        return ERROR_DEVICE_NOT_CONNECTED;
    }

    let controllers = CONTROLLERS.lock();
    let ctrl = &controllers[dw_user_index as usize];

    if !ctrl.connected {
        return ERROR_DEVICE_NOT_CONNECTED;
    }

    unsafe {
        (*capabilities).dev_type = XINPUT_DEVTYPE_GAMEPAD;
        (*capabilities).dev_sub_type = XINPUT_DEVSUBTYPE_GAMEPAD;
        (*capabilities).flags = XINPUT_CAPS_FFB_SUPPORTED;
        (*capabilities).gamepad = XInputGamepad {
            w_buttons: 0xFFFF, // All buttons supported
            b_left_trigger: 255,
            b_right_trigger: 255,
            s_thumb_lx: i16::MAX,
            s_thumb_ly: i16::MAX,
            s_thumb_rx: i16::MAX,
            s_thumb_ry: i16::MAX,
        };
        (*capabilities).vibration = XInputVibration {
            w_left_motor_speed: 65535,
            w_right_motor_speed: 65535,
        };
    }

    log::debug!("[xinput] GetCapabilities({}): gamepad", dw_user_index);
    ERROR_SUCCESS
}

/// XInputGetBatteryInformation — get battery info for a controller.
pub fn xinput_get_battery_information(
    dw_user_index: DWord,
    _dev_type: u8,
    battery_info: *mut XInputBatteryInformation,
) -> DWord {
    if dw_user_index >= XUSER_MAX_COUNT || battery_info.is_null() {
        return ERROR_DEVICE_NOT_CONNECTED;
    }

    let controllers = CONTROLLERS.lock();
    if !controllers[dw_user_index as usize].connected {
        return ERROR_DEVICE_NOT_CONNECTED;
    }

    // Report as wired (no battery)
    unsafe {
        (*battery_info).battery_type = 0x01; // BATTERY_TYPE_WIRED
        (*battery_info).battery_level = 0x03; // BATTERY_LEVEL_FULL
    }

    ERROR_SUCCESS
}

/// XInputEnable — enable or disable XInput reporting.
pub fn xinput_enable(enable: i32) {
    log::debug!("[xinput] XInputEnable: {}", enable != 0);
    // In our implementation this is a no-op; controllers always report.
}

// =============================================================================
// Internal helpers for USB HID gamepad integration
// =============================================================================

/// Connect a controller (called when USB HID gamepad is detected).
pub fn connect_controller(index: usize) {
    if index < 4 {
        let mut controllers = CONTROLLERS.lock();
        controllers[index].connected = true;
        log::info!("[xinput] Controller {} connected", index);
    }
}

/// Disconnect a controller.
pub fn disconnect_controller(index: usize) {
    if index < 4 {
        let mut controllers = CONTROLLERS.lock();
        controllers[index].connected = false;
        log::info!("[xinput] Controller {} disconnected", index);
    }
}

/// Update controller state from USB HID report.
pub fn update_controller_state(index: usize, gamepad: XInputGamepad) {
    if index < 4 {
        let mut controllers = CONTROLLERS.lock();
        if controllers[index].connected {
            controllers[index].gamepad = gamepad;
            controllers[index].packet_number = controllers[index].packet_number.wrapping_add(1);
        }
    }
}
