//! User32.dll API implementation — window management and message loop.
//!
//! Windows are mapped to framebuffer panes in ClaudioOS. Each window has its own
//! message queue. The message loop pumps keyboard/mouse events to the focused window.

use alloc::collections::BTreeMap;
use alloc::collections::VecDeque;
use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;

use crate::unicode::*;
use crate::handles::{self, HandleObject};
use crate::teb_peb;

// =============================================================================
// Window class registration
// =============================================================================

/// WNDCLASSW structure (simplified).
#[repr(C)]
pub struct WndClassW {
    pub style: UInt,
    pub wnd_proc: u64,       // WNDPROC function pointer
    pub cls_extra: i32,
    pub wnd_extra: i32,
    pub instance: HInstance,
    pub icon: u64,           // HICON
    pub cursor: u64,         // HCURSOR
    pub background: HBrush,
    pub menu_name: LpcWStr,
    pub class_name: LpcWStr,
}

/// Registered window class info.
#[derive(Clone)]
struct RegisteredClass {
    name: String,
    style: UInt,
    wnd_proc: u64,
    instance: HInstance,
}

/// Class registration table.
static CLASSES: Mutex<Option<BTreeMap<String, RegisteredClass>>> = Mutex::new(None);

/// RegisterClassW — register a window class.
pub fn register_class_w(wnd_class: *const WndClassW) -> Atom {
    if wnd_class.is_null() {
        teb_peb::set_last_error(87); // ERROR_INVALID_PARAMETER
        return 0;
    }

    let (name, style, wnd_proc, instance) = unsafe {
        let cls = &*wnd_class;
        let name = utf16_to_utf8(cls.class_name);
        (name, cls.style, cls.wnd_proc, cls.instance)
    };

    log::debug!("[user32] RegisterClassW: '{}'", name);

    let mut classes = CLASSES.lock();
    if classes.is_none() {
        *classes = Some(BTreeMap::new());
    }

    static NEXT_ATOM: spin::Mutex<u16> = spin::Mutex::new(0xC000);
    let atom = {
        let mut a = NEXT_ATOM.lock();
        let val = *a;
        *a += 1;
        val
    };

    if let Some(ref mut map) = *classes {
        map.insert(name.clone(), RegisteredClass {
            name,
            style,
            wnd_proc,
            instance,
        });
    }

    atom
}

// =============================================================================
// Window creation and management
// =============================================================================

/// Window styles.
pub const WS_OVERLAPPED: DWord = 0x00000000;
pub const WS_POPUP: DWord = 0x80000000;
pub const WS_CHILD: DWord = 0x40000000;
pub const WS_VISIBLE: DWord = 0x10000000;
pub const WS_CAPTION: DWord = 0x00C00000;
pub const WS_SYSMENU: DWord = 0x00080000;
pub const WS_MINIMIZEBOX: DWord = 0x00020000;
pub const WS_MAXIMIZEBOX: DWord = 0x00010000;
pub const WS_OVERLAPPEDWINDOW: DWord = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX;

/// Extended window styles.
pub const WS_EX_APPWINDOW: DWord = 0x00040000;
pub const WS_EX_WINDOWEDGE: DWord = 0x00000100;
pub const WS_EX_CLIENTEDGE: DWord = 0x00000200;

/// Default window position.
pub const CW_USEDEFAULT: i32 = 0x80000000_u32 as i32;

/// Show window commands.
pub const SW_HIDE: i32 = 0;
pub const SW_SHOWNORMAL: i32 = 1;
pub const SW_SHOWMINIMIZED: i32 = 2;
pub const SW_SHOWMAXIMIZED: i32 = 3;
pub const SW_SHOW: i32 = 5;
pub const SW_RESTORE: i32 = 9;

/// Window messages.
pub const WM_NULL: UInt = 0x0000;
pub const WM_CREATE: UInt = 0x0001;
pub const WM_DESTROY: UInt = 0x0002;
pub const WM_MOVE: UInt = 0x0003;
pub const WM_SIZE: UInt = 0x0005;
pub const WM_ACTIVATE: UInt = 0x0006;
pub const WM_SETFOCUS: UInt = 0x0007;
pub const WM_KILLFOCUS: UInt = 0x0008;
pub const WM_CLOSE: UInt = 0x0010;
pub const WM_QUIT: UInt = 0x0012;
pub const WM_PAINT: UInt = 0x000F;
pub const WM_ERASEBKGND: UInt = 0x0014;
pub const WM_KEYDOWN: UInt = 0x0100;
pub const WM_KEYUP: UInt = 0x0101;
pub const WM_CHAR: UInt = 0x0102;
pub const WM_COMMAND: UInt = 0x0111;
pub const WM_TIMER: UInt = 0x0113;
pub const WM_MOUSEMOVE: UInt = 0x0200;
pub const WM_LBUTTONDOWN: UInt = 0x0201;
pub const WM_LBUTTONUP: UInt = 0x0202;
pub const WM_RBUTTONDOWN: UInt = 0x0204;
pub const WM_RBUTTONUP: UInt = 0x0205;
pub const WM_MOUSEWHEEL: UInt = 0x020A;

/// MSG structure.
#[repr(C)]
pub struct Msg {
    pub hwnd: HWnd,
    pub message: UInt,
    pub wparam: WParam,
    pub lparam: LParam,
    pub time: DWord,
    pub pt_x: i32,
    pub pt_y: i32,
}

/// Per-window message queue.
struct WindowState {
    class_name: String,
    title: String,
    wnd_proc: u64,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    visible: bool,
    queue: VecDeque<QueuedMsg>,
}

struct QueuedMsg {
    message: UInt,
    wparam: WParam,
    lparam: LParam,
}

/// Global window state table.
static WINDOWS: Mutex<Option<BTreeMap<u64, WindowState>>> = Mutex::new(None);

/// Thread-level quit flag.
static QUIT_POSTED: Mutex<bool> = Mutex::new(false);
static QUIT_CODE: Mutex<i32> = Mutex::new(0);

/// CreateWindowExW — create a window (framebuffer pane).
pub fn create_window_ex_w(
    ex_style: DWord,
    class_name: LpcWStr,
    window_name: LpcWStr,
    style: DWord,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    parent: HWnd,
    menu: u64,
    instance: HInstance,
    param: u64,
) -> HWnd {
    let cls = unsafe { utf16_to_utf8(class_name) };
    let title = unsafe { utf16_to_utf8(window_name) };

    log::debug!(
        "[user32] CreateWindowExW: class='{}', title='{}', {}x{} at ({},{})",
        cls, title, width, height, x, y
    );

    // Look up the window class for its wnd_proc
    let wnd_proc = {
        let classes = CLASSES.lock();
        classes.as_ref()
            .and_then(|m| m.get(&cls))
            .map(|c| c.wnd_proc)
            .unwrap_or(0)
    };

    // Resolve CW_USEDEFAULT
    let real_x = if x == CW_USEDEFAULT { 100 } else { x };
    let real_y = if y == CW_USEDEFAULT { 100 } else { y };
    let real_w = if width == CW_USEDEFAULT { 800 } else { width };
    let real_h = if height == CW_USEDEFAULT { 600 } else { height };

    // Allocate a handle
    let handle = handles::alloc_handle(HandleObject::Window {
        class_name: cls.clone(),
        title: title.clone(),
        x: real_x,
        y: real_y,
        width: real_w,
        height: real_h,
        visible: (style & WS_VISIBLE) != 0,
        parent,
    });

    // Store window state
    let mut windows = WINDOWS.lock();
    if windows.is_none() {
        *windows = Some(BTreeMap::new());
    }
    if let Some(ref mut map) = *windows {
        map.insert(handle, WindowState {
            class_name: cls,
            title,
            wnd_proc,
            x: real_x,
            y: real_y,
            width: real_w,
            height: real_h,
            visible: (style & WS_VISIBLE) != 0,
            queue: VecDeque::new(),
        });
    }

    // Send WM_CREATE
    post_message_to(handle, WM_CREATE, 0, 0);

    handle
}

/// DestroyWindow.
pub fn destroy_window(hwnd: HWnd) -> Bool {
    log::debug!("[user32] DestroyWindow: hwnd=0x{:X}", hwnd);

    // Send WM_DESTROY
    post_message_to(hwnd, WM_DESTROY, 0, 0);

    // Remove from window state table
    let mut windows = WINDOWS.lock();
    if let Some(ref mut map) = *windows {
        map.remove(&hwnd);
    }
    handles::close_handle(hwnd);
    TRUE
}

/// ShowWindow — show or hide a window.
pub fn show_window(hwnd: HWnd, cmd_show: i32) -> Bool {
    log::trace!("[user32] ShowWindow: hwnd=0x{:X}, cmd={}", hwnd, cmd_show);

    let was_visible = {
        let mut windows = WINDOWS.lock();
        if let Some(ref mut map) = *windows {
            if let Some(win) = map.get_mut(&hwnd) {
                let prev = win.visible;
                win.visible = cmd_show != SW_HIDE;
                prev
            } else {
                false
            }
        } else {
            false
        }
    };

    if was_visible { TRUE } else { FALSE }
}

// =============================================================================
// Message loop
// =============================================================================

/// GetMessageW — retrieve a message, blocking until one is available.
/// Returns FALSE (0) for WM_QUIT, TRUE otherwise.
pub fn get_message_w(
    msg: *mut Msg,
    hwnd: HWnd,
    msg_filter_min: UInt,
    msg_filter_max: UInt,
) -> Bool {
    if msg.is_null() {
        return FALSE;
    }

    // Check if quit was posted
    {
        let quit = *QUIT_POSTED.lock();
        if quit {
            unsafe {
                (*msg).hwnd = 0;
                (*msg).message = WM_QUIT;
                (*msg).wparam = *QUIT_CODE.lock() as WParam;
                (*msg).lparam = 0;
                (*msg).time = crate::kernel32::get_tick_count();
                (*msg).pt_x = 0;
                (*msg).pt_y = 0;
            }
            return FALSE;
        }
    }

    // Try to dequeue a message
    let mut windows = WINDOWS.lock();
    if let Some(ref mut map) = *windows {
        // If hwnd is 0, check all windows
        let queued = if hwnd == 0 {
            let mut found = None;
            for (&wh, state) in map.iter_mut() {
                if let Some(qm) = state.queue.pop_front() {
                    found = Some((wh, qm));
                    break;
                }
            }
            found
        } else {
            map.get_mut(&hwnd).and_then(|state| {
                state.queue.pop_front().map(|qm| (hwnd, qm))
            })
        };

        if let Some((wh, qm)) = queued {
            let in_range = msg_filter_min == 0 && msg_filter_max == 0
                || (qm.message >= msg_filter_min && qm.message <= msg_filter_max);

            if in_range {
                unsafe {
                    (*msg).hwnd = wh;
                    (*msg).message = qm.message;
                    (*msg).wparam = qm.wparam;
                    (*msg).lparam = qm.lparam;
                    (*msg).time = crate::kernel32::get_tick_count();
                    (*msg).pt_x = 0;
                    (*msg).pt_y = 0;
                }
                return TRUE;
            }
        }
    }

    // No messages — synthesize an idle paint
    unsafe {
        (*msg).hwnd = hwnd;
        (*msg).message = WM_PAINT;
        (*msg).wparam = 0;
        (*msg).lparam = 0;
        (*msg).time = crate::kernel32::get_tick_count();
        (*msg).pt_x = 0;
        (*msg).pt_y = 0;
    }
    TRUE
}

/// PeekMessageW — check for a message without blocking.
pub fn peek_message_w(
    msg: *mut Msg,
    hwnd: HWnd,
    msg_filter_min: UInt,
    msg_filter_max: UInt,
    remove_msg: UInt,
) -> Bool {
    if msg.is_null() {
        return FALSE;
    }

    let pm_remove = (remove_msg & 0x0001) != 0; // PM_REMOVE

    // Check quit flag
    {
        let quit = *QUIT_POSTED.lock();
        if quit {
            unsafe {
                (*msg).hwnd = 0;
                (*msg).message = WM_QUIT;
                (*msg).wparam = *QUIT_CODE.lock() as WParam;
                (*msg).lparam = 0;
                (*msg).time = crate::kernel32::get_tick_count();
                (*msg).pt_x = 0;
                (*msg).pt_y = 0;
            }
            if pm_remove {
                *QUIT_POSTED.lock() = false;
            }
            return TRUE;
        }
    }

    let mut windows = WINDOWS.lock();
    if let Some(ref mut map) = *windows {
        let queued = if hwnd == 0 {
            let mut found = None;
            for (&wh, state) in map.iter_mut() {
                if pm_remove {
                    if let Some(qm) = state.queue.pop_front() {
                        found = Some((wh, qm));
                        break;
                    }
                } else if let Some(qm) = state.queue.front() {
                    found = Some((wh, QueuedMsg {
                        message: qm.message,
                        wparam: qm.wparam,
                        lparam: qm.lparam,
                    }));
                    break;
                }
            }
            found
        } else {
            map.get_mut(&hwnd).and_then(|state| {
                if pm_remove {
                    state.queue.pop_front().map(|qm| (hwnd, qm))
                } else {
                    state.queue.front().map(|qm| (hwnd, QueuedMsg {
                        message: qm.message,
                        wparam: qm.wparam,
                        lparam: qm.lparam,
                    }))
                }
            })
        };

        if let Some((wh, qm)) = queued {
            unsafe {
                (*msg).hwnd = wh;
                (*msg).message = qm.message;
                (*msg).wparam = qm.wparam;
                (*msg).lparam = qm.lparam;
                (*msg).time = crate::kernel32::get_tick_count();
                (*msg).pt_x = 0;
                (*msg).pt_y = 0;
            }
            return TRUE;
        }
    }

    FALSE
}

/// TranslateMessage — translate virtual-key messages into character messages.
pub fn translate_message(msg: *const Msg) -> Bool {
    if msg.is_null() {
        return FALSE;
    }

    let message = unsafe { (*msg).message };
    if message == WM_KEYDOWN {
        // In a full implementation, we'd translate VK codes to WM_CHAR messages.
        // For now, just post a WM_CHAR with the wparam as the character code.
        let hwnd = unsafe { (*msg).hwnd };
        let wparam = unsafe { (*msg).wparam };
        if wparam < 128 {
            post_message_to(hwnd, WM_CHAR, wparam, 0);
        }
    }
    TRUE
}

/// DispatchMessageW — dispatch a message to the window's WndProc.
pub fn dispatch_message_w(msg: *const Msg) -> LResult {
    if msg.is_null() {
        return 0;
    }

    let (hwnd, message, wparam, lparam) = unsafe {
        ((*msg).hwnd, (*msg).message, (*msg).wparam, (*msg).lparam)
    };

    // Look up the window's WndProc
    let wnd_proc = {
        let windows = WINDOWS.lock();
        windows.as_ref()
            .and_then(|map| map.get(&hwnd))
            .map(|w| w.wnd_proc)
            .unwrap_or(0)
    };

    if wnd_proc != 0 {
        // Call the WndProc — in practice this would be an indirect call
        // through the function pointer. Since we're in a bare-metal OS,
        // we invoke it via the PE's IAT-resolved function.
        log::trace!(
            "[user32] DispatchMessage: hwnd=0x{:X}, msg=0x{:X} -> wndproc=0x{:X}",
            hwnd, message, wnd_proc
        );

        // Safety: calling into loaded PE code
        unsafe {
            let func: extern "system" fn(HWnd, UInt, WParam, LParam) -> LResult =
                core::mem::transmute(wnd_proc);
            func(hwnd, message, wparam, lparam)
        }
    } else {
        def_window_proc_w(hwnd, message, wparam, lparam)
    }
}

/// PostQuitMessage — post WM_QUIT to the thread message queue.
pub fn post_quit_message(exit_code: i32) {
    log::debug!("[user32] PostQuitMessage: code={}", exit_code);
    *QUIT_POSTED.lock() = true;
    *QUIT_CODE.lock() = exit_code;
}

/// DefWindowProcW — default window message handler.
pub fn def_window_proc_w(hwnd: HWnd, msg: UInt, wparam: WParam, lparam: LParam) -> LResult {
    match msg {
        WM_CLOSE => {
            destroy_window(hwnd);
            0
        }
        WM_DESTROY => {
            0
        }
        WM_ERASEBKGND => {
            1 // We handled it
        }
        WM_PAINT => {
            0
        }
        _ => 0,
    }
}

/// MessageBoxW — display a modal dialog (logged to serial, returns IDOK).
pub fn message_box_w(hwnd: HWnd, text: LpcWStr, caption: LpcWStr, mb_type: UInt) -> i32 {
    let text_str = unsafe { utf16_to_utf8(text) };
    let caption_str = unsafe { utf16_to_utf8(caption) };
    log::info!("[user32] MessageBox [{}]: {}", caption_str, text_str);
    1 // IDOK
}

// =============================================================================
// Keyboard and mouse state
// =============================================================================

/// Virtual key state table (256 entries, one per VK code).
static KEY_STATE: Mutex<[u8; 256]> = Mutex::new([0u8; 256]);

/// GetKeyState — get the state of a virtual key.
pub fn get_key_state(vk: i32) -> i16 {
    let state = KEY_STATE.lock();
    let idx = (vk & 0xFF) as usize;
    // High bit = pressed, low bit = toggled
    state[idx] as i16
}

/// GetAsyncKeyState — get the async state of a virtual key.
pub fn get_async_key_state(vk: i32) -> i16 {
    // Same as GetKeyState in our single-threaded model
    get_key_state(vk)
}

/// POINT structure.
#[repr(C)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

/// Cursor position.
static CURSOR_POS: Mutex<(i32, i32)> = Mutex::new((0, 0));

/// GetCursorPos.
pub fn get_cursor_pos(point: *mut Point) -> Bool {
    if point.is_null() {
        return FALSE;
    }
    let (x, y) = *CURSOR_POS.lock();
    unsafe {
        (*point).x = x;
        (*point).y = y;
    }
    TRUE
}

/// SetCursorPos.
pub fn set_cursor_pos(x: i32, y: i32) -> Bool {
    *CURSOR_POS.lock() = (x, y);
    TRUE
}

// =============================================================================
// Internal helpers
// =============================================================================

/// Post a message to a specific window's queue.
fn post_message_to(hwnd: HWnd, message: UInt, wparam: WParam, lparam: LParam) {
    let mut windows = WINDOWS.lock();
    if let Some(ref mut map) = *windows {
        if let Some(state) = map.get_mut(&hwnd) {
            state.queue.push_back(QueuedMsg { message, wparam, lparam });
        }
    }
}

/// Update key state (called from keyboard interrupt handler).
pub fn set_key_state(vk: u8, pressed: bool) {
    let mut state = KEY_STATE.lock();
    if pressed {
        state[vk as usize] |= 0x80;
    } else {
        state[vk as usize] &= !0x80;
    }
}

/// Update cursor position (called from mouse handler).
pub fn update_cursor(x: i32, y: i32) {
    *CURSOR_POS.lock() = (x, y);
}
