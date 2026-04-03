//! DLL function dispatcher — resolves "dll_name!function_name" to Rust fn pointers.
//!
//! When the PE loader encounters an import like "kernel32.dll!CreateFileW",
//! it calls `resolve_import` which returns the address of our Rust implementation.
//! This is the core of Win32 compatibility: the IAT (Import Address Table) of
//! loaded PE binaries points directly to these functions.

use alloc::collections::BTreeMap;
use alloc::string::String;
use spin::Mutex;

/// Type alias for a resolved function pointer.
pub type FnPtr = u64;

/// Global dispatch table: maps "DLL_NAME!FunctionName" -> function pointer.
static DISPATCH: Mutex<Option<BTreeMap<String, FnPtr>>> = Mutex::new(None);

/// Module base addresses for GetModuleHandle/GetProcAddress.
static MODULE_MAP: Mutex<Option<BTreeMap<u64, String>>> = Mutex::new(None);

/// Initialize the dispatcher and register all implemented Win32 functions.
pub fn init() {
    let mut table = BTreeMap::new();
    let mut modules = BTreeMap::new();

    // === kernel32.dll ===
    let k32 = "kernel32.dll";
    modules.insert(0x7FF8_0000_0000u64, String::from(k32));

    register(&mut table, k32, "CreateFileW", crate::kernel32::create_file_w as FnPtr);
    register(&mut table, k32, "ReadFile", crate::kernel32::read_file as FnPtr);
    register(&mut table, k32, "WriteFile", crate::kernel32::write_file as FnPtr);
    register(&mut table, k32, "CloseHandle", crate::kernel32::close_handle as FnPtr);
    register(&mut table, k32, "GetFileSize", crate::kernel32::get_file_size as FnPtr);
    register(&mut table, k32, "SetFilePointer", crate::kernel32::set_file_pointer as FnPtr);
    register(&mut table, k32, "DeleteFileW", crate::kernel32::delete_file_w as FnPtr);
    register(&mut table, k32, "CreateDirectoryW", crate::kernel32::create_directory_w as FnPtr);
    register(&mut table, k32, "FindFirstFileW", crate::kernel32::find_first_file_w as FnPtr);
    register(&mut table, k32, "FindNextFileW", crate::kernel32::find_next_file_w as FnPtr);
    register(&mut table, k32, "FindClose", crate::kernel32::find_close as FnPtr);
    register(&mut table, k32, "CreateProcessW", crate::kernel32::create_process_w as FnPtr);
    register(&mut table, k32, "ExitProcess", crate::kernel32::exit_process as FnPtr);
    register(&mut table, k32, "GetCurrentProcess", crate::kernel32::get_current_process as FnPtr);
    register(&mut table, k32, "GetCurrentProcessId", crate::kernel32::get_current_process_id as FnPtr);
    register(&mut table, k32, "TerminateProcess", crate::kernel32::terminate_process as FnPtr);
    register(&mut table, k32, "CreateThread", crate::kernel32::create_thread as FnPtr);
    register(&mut table, k32, "ExitThread", crate::kernel32::exit_thread as FnPtr);
    register(&mut table, k32, "GetCurrentThread", crate::kernel32::get_current_thread as FnPtr);
    register(&mut table, k32, "GetCurrentThreadId", crate::kernel32::get_current_thread_id as FnPtr);
    register(&mut table, k32, "Sleep", crate::kernel32::sleep as FnPtr);
    register(&mut table, k32, "SleepEx", crate::kernel32::sleep_ex as FnPtr);
    register(&mut table, k32, "VirtualAlloc", crate::kernel32::virtual_alloc as FnPtr);
    register(&mut table, k32, "VirtualFree", crate::kernel32::virtual_free as FnPtr);
    register(&mut table, k32, "VirtualProtect", crate::kernel32::virtual_protect as FnPtr);
    register(&mut table, k32, "HeapCreate", crate::kernel32::heap_create as FnPtr);
    register(&mut table, k32, "HeapAlloc", crate::kernel32::heap_alloc as FnPtr);
    register(&mut table, k32, "HeapFree", crate::kernel32::heap_free as FnPtr);
    register(&mut table, k32, "HeapReAlloc", crate::kernel32::heap_realloc as FnPtr);
    register(&mut table, k32, "GetProcessHeap", crate::kernel32::get_process_heap as FnPtr);
    register(&mut table, k32, "CreateMutexW", crate::kernel32::create_mutex_w as FnPtr);
    register(&mut table, k32, "CreateEventW", crate::kernel32::create_event_w as FnPtr);
    register(&mut table, k32, "WaitForSingleObject", crate::kernel32::wait_for_single_object as FnPtr);
    register(&mut table, k32, "WaitForMultipleObjects", crate::kernel32::wait_for_multiple_objects as FnPtr);
    register(&mut table, k32, "ReleaseMutex", crate::kernel32::release_mutex as FnPtr);
    register(&mut table, k32, "SetEvent", crate::kernel32::set_event as FnPtr);
    register(&mut table, k32, "ResetEvent", crate::kernel32::reset_event as FnPtr);
    register(&mut table, k32, "InitializeCriticalSection", crate::kernel32::initialize_critical_section as FnPtr);
    register(&mut table, k32, "EnterCriticalSection", crate::kernel32::enter_critical_section as FnPtr);
    register(&mut table, k32, "LeaveCriticalSection", crate::kernel32::leave_critical_section as FnPtr);
    register(&mut table, k32, "GetStdHandle", crate::kernel32::get_std_handle as FnPtr);
    register(&mut table, k32, "WriteConsoleW", crate::kernel32::write_console_w as FnPtr);
    register(&mut table, k32, "ReadConsoleW", crate::kernel32::read_console_w as FnPtr);
    register(&mut table, k32, "SetConsoleTitleW", crate::kernel32::set_console_title_w as FnPtr);
    register(&mut table, k32, "GetConsoleMode", crate::kernel32::get_console_mode as FnPtr);
    register(&mut table, k32, "SetConsoleMode", crate::kernel32::set_console_mode as FnPtr);
    register(&mut table, k32, "GetLastError", crate::kernel32::get_last_error as FnPtr);
    register(&mut table, k32, "SetLastError", crate::kernel32::set_last_error as FnPtr);
    register(&mut table, k32, "GetTickCount", crate::kernel32::get_tick_count as FnPtr);
    register(&mut table, k32, "GetTickCount64", crate::kernel32::get_tick_count64 as FnPtr);
    register(&mut table, k32, "QueryPerformanceCounter", crate::kernel32::query_performance_counter as FnPtr);
    register(&mut table, k32, "QueryPerformanceFrequency", crate::kernel32::query_performance_frequency as FnPtr);
    register(&mut table, k32, "GetSystemTimeAsFileTime", crate::kernel32::get_system_time_as_file_time as FnPtr);
    register(&mut table, k32, "GetModuleHandleW", crate::kernel32::get_module_handle_w as FnPtr);
    register(&mut table, k32, "GetProcAddress", crate::kernel32::get_proc_address as FnPtr);
    register(&mut table, k32, "LoadLibraryW", crate::kernel32::load_library_w as FnPtr);
    register(&mut table, k32, "FreeLibrary", crate::kernel32::free_library as FnPtr);
    register(&mut table, k32, "GetCommandLineW", crate::kernel32::get_command_line_w as FnPtr);
    register(&mut table, k32, "GetEnvironmentVariableW", crate::kernel32::get_environment_variable_w as FnPtr);
    register(&mut table, k32, "SetEnvironmentVariableW", crate::kernel32::set_environment_variable_w as FnPtr);
    register(&mut table, k32, "GetSystemInfo", crate::kernel32::get_system_info as FnPtr);
    register(&mut table, k32, "GlobalAlloc", crate::kernel32::global_alloc as FnPtr);
    register(&mut table, k32, "GlobalFree", crate::kernel32::global_free as FnPtr);
    register(&mut table, k32, "OutputDebugStringW", crate::kernel32::output_debug_string_w as FnPtr);
    register(&mut table, k32, "FormatMessageW", crate::kernel32::format_message_w as FnPtr);

    // === user32.dll ===
    let u32_dll = "user32.dll";
    modules.insert(0x7FF8_2000_0000u64, String::from(u32_dll));

    register(&mut table, u32_dll, "RegisterClassW", crate::user32::register_class_w as FnPtr);
    register(&mut table, u32_dll, "CreateWindowExW", crate::user32::create_window_ex_w as FnPtr);
    register(&mut table, u32_dll, "DestroyWindow", crate::user32::destroy_window as FnPtr);
    register(&mut table, u32_dll, "ShowWindow", crate::user32::show_window as FnPtr);
    register(&mut table, u32_dll, "GetMessageW", crate::user32::get_message_w as FnPtr);
    register(&mut table, u32_dll, "PeekMessageW", crate::user32::peek_message_w as FnPtr);
    register(&mut table, u32_dll, "TranslateMessage", crate::user32::translate_message as FnPtr);
    register(&mut table, u32_dll, "DispatchMessageW", crate::user32::dispatch_message_w as FnPtr);
    register(&mut table, u32_dll, "PostQuitMessage", crate::user32::post_quit_message as FnPtr);
    register(&mut table, u32_dll, "DefWindowProcW", crate::user32::def_window_proc_w as FnPtr);
    register(&mut table, u32_dll, "MessageBoxW", crate::user32::message_box_w as FnPtr);
    register(&mut table, u32_dll, "GetKeyState", crate::user32::get_key_state as FnPtr);
    register(&mut table, u32_dll, "GetAsyncKeyState", crate::user32::get_async_key_state as FnPtr);
    register(&mut table, u32_dll, "GetCursorPos", crate::user32::get_cursor_pos as FnPtr);
    register(&mut table, u32_dll, "SetCursorPos", crate::user32::set_cursor_pos as FnPtr);

    // === gdi32.dll ===
    let gdi = "gdi32.dll";
    modules.insert(0x7FF8_3000_0000u64, String::from(gdi));

    register(&mut table, gdi, "CreateDCW", crate::gdi32::create_dc_w as FnPtr);
    register(&mut table, gdi, "CreateCompatibleDC", crate::gdi32::create_compatible_dc as FnPtr);
    register(&mut table, gdi, "DeleteDC", crate::gdi32::delete_dc as FnPtr);
    register(&mut table, gdi, "GetDC", crate::gdi32::get_dc as FnPtr);
    register(&mut table, gdi, "ReleaseDC", crate::gdi32::release_dc as FnPtr);
    register(&mut table, gdi, "CreateCompatibleBitmap", crate::gdi32::create_compatible_bitmap as FnPtr);
    register(&mut table, gdi, "BitBlt", crate::gdi32::bit_blt as FnPtr);
    register(&mut table, gdi, "StretchBlt", crate::gdi32::stretch_blt as FnPtr);
    register(&mut table, gdi, "SelectObject", crate::gdi32::select_object as FnPtr);
    register(&mut table, gdi, "DeleteObject", crate::gdi32::delete_object as FnPtr);
    register(&mut table, gdi, "Rectangle", crate::gdi32::rectangle as FnPtr);
    register(&mut table, gdi, "Ellipse", crate::gdi32::ellipse as FnPtr);
    register(&mut table, gdi, "LineTo", crate::gdi32::line_to as FnPtr);
    register(&mut table, gdi, "MoveToEx", crate::gdi32::move_to_ex as FnPtr);
    register(&mut table, gdi, "TextOutW", crate::gdi32::text_out_w as FnPtr);
    register(&mut table, gdi, "SetPixel", crate::gdi32::set_pixel as FnPtr);
    register(&mut table, gdi, "GetPixel", crate::gdi32::get_pixel as FnPtr);
    register(&mut table, gdi, "CreateFontW", crate::gdi32::create_font_w as FnPtr);

    // === ws2_32.dll ===
    let ws2 = "ws2_32.dll";
    modules.insert(0x7FF8_4000_0000u64, String::from(ws2));

    register(&mut table, ws2, "WSAStartup", crate::ws2_32::wsa_startup as FnPtr);
    register(&mut table, ws2, "WSACleanup", crate::ws2_32::wsa_cleanup as FnPtr);
    register(&mut table, ws2, "socket", crate::ws2_32::socket as FnPtr);
    register(&mut table, ws2, "bind", crate::ws2_32::bind as FnPtr);
    register(&mut table, ws2, "listen", crate::ws2_32::listen as FnPtr);
    register(&mut table, ws2, "accept", crate::ws2_32::accept as FnPtr);
    register(&mut table, ws2, "connect", crate::ws2_32::connect as FnPtr);
    register(&mut table, ws2, "send", crate::ws2_32::send as FnPtr);
    register(&mut table, ws2, "recv", crate::ws2_32::recv as FnPtr);
    register(&mut table, ws2, "closesocket", crate::ws2_32::closesocket as FnPtr);
    register(&mut table, ws2, "select", crate::ws2_32::select as FnPtr);
    register(&mut table, ws2, "getaddrinfo", crate::ws2_32::getaddrinfo as FnPtr);
    register(&mut table, ws2, "freeaddrinfo", crate::ws2_32::freeaddrinfo as FnPtr);
    register(&mut table, ws2, "gethostbyname", crate::ws2_32::gethostbyname as FnPtr);
    register(&mut table, ws2, "ioctlsocket", crate::ws2_32::ioctlsocket as FnPtr);
    register(&mut table, ws2, "setsockopt", crate::ws2_32::setsockopt as FnPtr);
    register(&mut table, ws2, "getsockopt", crate::ws2_32::getsockopt as FnPtr);

    // === advapi32.dll ===
    let adv = "advapi32.dll";
    modules.insert(0x7FF8_5000_0000u64, String::from(adv));

    register(&mut table, adv, "RegOpenKeyExW", crate::advapi32::reg_open_key_ex_w as FnPtr);
    register(&mut table, adv, "RegQueryValueExW", crate::advapi32::reg_query_value_ex_w as FnPtr);
    register(&mut table, adv, "RegSetValueExW", crate::advapi32::reg_set_value_ex_w as FnPtr);
    register(&mut table, adv, "RegCloseKey", crate::advapi32::reg_close_key as FnPtr);
    register(&mut table, adv, "RegCreateKeyExW", crate::advapi32::reg_create_key_ex_w as FnPtr);
    register(&mut table, adv, "RegDeleteKeyW", crate::advapi32::reg_delete_key_w as FnPtr);
    register(&mut table, adv, "RegEnumKeyExW", crate::advapi32::reg_enum_key_ex_w as FnPtr);
    register(&mut table, adv, "RegEnumValueW", crate::advapi32::reg_enum_value_w as FnPtr);
    register(&mut table, adv, "CryptAcquireContextW", crate::advapi32::crypt_acquire_context_w as FnPtr);
    register(&mut table, adv, "CryptGenRandom", crate::advapi32::crypt_gen_random as FnPtr);

    // === ole32.dll ===
    let ole = "ole32.dll";
    modules.insert(0x7FF8_7000_0000u64, String::from(ole));

    register(&mut table, ole, "CoInitialize", crate::ole32::co_initialize as FnPtr);
    register(&mut table, ole, "CoInitializeEx", crate::ole32::co_initialize_ex as FnPtr);
    register(&mut table, ole, "CoUninitialize", crate::ole32::co_uninitialize as FnPtr);
    register(&mut table, ole, "CoCreateInstance", crate::ole32::co_create_instance as FnPtr);

    // === msvcrt.dll ===
    let crt = "msvcrt.dll";
    modules.insert(0x7FF8_6000_0000u64, String::from(crt));

    register(&mut table, crt, "malloc", crate::msvcrt::malloc as FnPtr);
    register(&mut table, crt, "free", crate::msvcrt::free as FnPtr);
    register(&mut table, crt, "realloc", crate::msvcrt::realloc as FnPtr);
    register(&mut table, crt, "calloc", crate::msvcrt::calloc as FnPtr);
    register(&mut table, crt, "printf", crate::msvcrt::printf as FnPtr);
    register(&mut table, crt, "sprintf", crate::msvcrt::sprintf as FnPtr);
    register(&mut table, crt, "fopen", crate::msvcrt::fopen as FnPtr);
    register(&mut table, crt, "fclose", crate::msvcrt::fclose as FnPtr);
    register(&mut table, crt, "fread", crate::msvcrt::fread as FnPtr);
    register(&mut table, crt, "fwrite", crate::msvcrt::fwrite as FnPtr);
    register(&mut table, crt, "strlen", crate::msvcrt::strlen as FnPtr);
    register(&mut table, crt, "strcpy", crate::msvcrt::strcpy as FnPtr);
    register(&mut table, crt, "strcmp", crate::msvcrt::strcmp as FnPtr);
    register(&mut table, crt, "memcpy", crate::msvcrt::memcpy as FnPtr);
    register(&mut table, crt, "memset", crate::msvcrt::memset as FnPtr);
    register(&mut table, crt, "atoi", crate::msvcrt::atoi as FnPtr);
    register(&mut table, crt, "time", crate::msvcrt::time as FnPtr);
    register(&mut table, crt, "clock", crate::msvcrt::clock as FnPtr);

    // === dwrite.dll (DirectWrite) ===
    let dw = "dwrite.dll";
    modules.insert(0x7FF8_8000_0000u64, String::from(dw));

    register(&mut table, dw, "DWriteCreateFactory", crate::directwrite::dwrite_create_factory as FnPtr);

    // === d2d1.dll (Direct2D) ===
    let d2d = "d2d1.dll";
    modules.insert(0x7FF8_9000_0000u64, String::from(d2d));

    register(&mut table, d2d, "D2D1CreateFactory", crate::direct2d::d2d1_create_factory as FnPtr);

    // === wasapi (via ole32 CoCreateInstance) ===
    // WASAPI functions are accessed through COM interfaces, not direct DLL exports.
    // The following are registered for GetProcAddress fallback resolution.
    let wasapi_dll = "audioses.dll";
    modules.insert(0x7FF8_A000_0000u64, String::from(wasapi_dll));

    register(&mut table, wasapi_dll, "CreateDeviceEnumerator", crate::wasapi::create_device_enumerator as FnPtr);
    register(&mut table, wasapi_dll, "GetDefaultAudioEndpoint", crate::wasapi::get_default_audio_endpoint as FnPtr);
    register(&mut table, wasapi_dll, "DeviceActivate", crate::wasapi::device_activate as FnPtr);
    register(&mut table, wasapi_dll, "AudioClientInitialize", crate::wasapi::audio_client_initialize as FnPtr);
    register(&mut table, wasapi_dll, "AudioClientGetBufferSize", crate::wasapi::audio_client_get_buffer_size as FnPtr);
    register(&mut table, wasapi_dll, "AudioClientGetCurrentPadding", crate::wasapi::audio_client_get_current_padding as FnPtr);
    register(&mut table, wasapi_dll, "AudioClientGetMixFormat", crate::wasapi::audio_client_get_mix_format as FnPtr);
    register(&mut table, wasapi_dll, "AudioClientGetService", crate::wasapi::audio_client_get_service as FnPtr);
    register(&mut table, wasapi_dll, "AudioClientStart", crate::wasapi::audio_client_start as FnPtr);
    register(&mut table, wasapi_dll, "AudioClientStop", crate::wasapi::audio_client_stop as FnPtr);
    register(&mut table, wasapi_dll, "RenderClientGetBuffer", crate::wasapi::render_client_get_buffer as FnPtr);
    register(&mut table, wasapi_dll, "RenderClientReleaseBuffer", crate::wasapi::render_client_release_buffer as FnPtr);

    // === xinput1_4.dll (XInput) ===
    let xi = "xinput1_4.dll";
    modules.insert(0x7FF8_B000_0000u64, String::from(xi));

    register(&mut table, xi, "XInputGetState", crate::xinput::xinput_get_state as FnPtr);
    register(&mut table, xi, "XInputSetState", crate::xinput::xinput_set_state as FnPtr);
    register(&mut table, xi, "XInputGetCapabilities", crate::xinput::xinput_get_capabilities as FnPtr);
    register(&mut table, xi, "XInputGetBatteryInformation", crate::xinput::xinput_get_battery_information as FnPtr);
    register(&mut table, xi, "XInputEnable", crate::xinput::xinput_enable as FnPtr);

    // Also register under xinput1_3.dll and xinput9_1_0.dll for compatibility
    let xi3 = "xinput1_3.dll";
    modules.insert(0x7FF8_B100_0000u64, String::from(xi3));
    register(&mut table, xi3, "XInputGetState", crate::xinput::xinput_get_state as FnPtr);
    register(&mut table, xi3, "XInputSetState", crate::xinput::xinput_set_state as FnPtr);
    register(&mut table, xi3, "XInputGetCapabilities", crate::xinput::xinput_get_capabilities as FnPtr);

    // === windowscodecs.dll (WIC) ===
    // WIC is primarily COM-based, but we register for GetProcAddress.
    let wic_dll = "windowscodecs.dll";
    modules.insert(0x7FF8_C000_0000u64, String::from(wic_dll));

    register(&mut table, wic_dll, "CreateImagingFactory", crate::wic::create_imaging_factory as FnPtr);

    // === shell32.dll ===
    let sh = "shell32.dll";
    modules.insert(0x7FF8_D000_0000u64, String::from(sh));

    register(&mut table, sh, "SHGetFolderPathW", crate::shell32::sh_get_folder_path_w as FnPtr);
    register(&mut table, sh, "SHGetSpecialFolderPathW", crate::shell32::sh_get_special_folder_path_w as FnPtr);
    register(&mut table, sh, "ShellExecuteW", crate::shell32::shell_execute_w as FnPtr);
    register(&mut table, sh, "SHGetFileInfoW", crate::shell32::sh_get_file_info_w as FnPtr);
    register(&mut table, sh, "SHGetKnownFolderPath", crate::shell32::sh_get_known_folder_path as FnPtr);
    register(&mut table, sh, "PathCombineW", crate::shell32::path_combine_w as FnPtr);

    // === winmm.dll ===
    let wmm = "winmm.dll";
    modules.insert(0x7FF8_E000_0000u64, String::from(wmm));

    register(&mut table, wmm, "waveOutGetNumDevs", crate::winmm::wave_out_get_num_devs as FnPtr);
    register(&mut table, wmm, "waveOutGetDevCapsW", crate::winmm::wave_out_get_dev_caps_w as FnPtr);
    register(&mut table, wmm, "waveOutOpen", crate::winmm::wave_out_open as FnPtr);
    register(&mut table, wmm, "waveOutPrepareHeader", crate::winmm::wave_out_prepare_header as FnPtr);
    register(&mut table, wmm, "waveOutUnprepareHeader", crate::winmm::wave_out_unprepare_header as FnPtr);
    register(&mut table, wmm, "waveOutWrite", crate::winmm::wave_out_write as FnPtr);
    register(&mut table, wmm, "waveOutReset", crate::winmm::wave_out_reset as FnPtr);
    register(&mut table, wmm, "waveOutPause", crate::winmm::wave_out_pause as FnPtr);
    register(&mut table, wmm, "waveOutRestart", crate::winmm::wave_out_restart as FnPtr);
    register(&mut table, wmm, "waveOutClose", crate::winmm::wave_out_close as FnPtr);
    register(&mut table, wmm, "waveOutGetVolume", crate::winmm::wave_out_get_volume as FnPtr);
    register(&mut table, wmm, "waveOutSetVolume", crate::winmm::wave_out_set_volume as FnPtr);
    register(&mut table, wmm, "waveOutGetPosition", crate::winmm::wave_out_get_position as FnPtr);
    register(&mut table, wmm, "timeGetTime", crate::winmm::time_get_time as FnPtr);
    register(&mut table, wmm, "timeBeginPeriod", crate::winmm::time_begin_period as FnPtr);
    register(&mut table, wmm, "timeEndPeriod", crate::winmm::time_end_period as FnPtr);
    register(&mut table, wmm, "PlaySoundW", crate::winmm::play_sound_w as FnPtr);

    let count = table.len();
    *DISPATCH.lock() = Some(table);
    *MODULE_MAP.lock() = Some(modules);

    log::info!("[dispatcher] Registered {} Win32 API functions", count);
}

/// Register a single function in the dispatch table.
fn register(table: &mut BTreeMap<String, FnPtr>, dll: &str, name: &str, ptr: FnPtr) {
    let key = alloc::format!("{}!{}", dll.to_ascii_lowercase(), name);
    table.insert(key, ptr);
}

/// Resolve a DLL import to a function pointer.
///
/// `dll_name` is e.g. "kernel32.dll", `func_name` is e.g. "CreateFileW".
/// Returns the address of the Rust implementation, or None if not found.
pub fn resolve_import(dll_name: &str, func_name: &str) -> Option<FnPtr> {
    let key = alloc::format!("{}!{}", dll_name.to_ascii_lowercase(), func_name);
    let table = DISPATCH.lock();
    table.as_ref().and_then(|t| t.get(&key).copied())
}

/// Resolve a function by module base address and function name.
/// Used by GetProcAddress.
pub fn resolve_function_by_module(module_base: u64, func_name: &str) -> Option<FnPtr> {
    let dll_name = {
        let modules = MODULE_MAP.lock();
        modules.as_ref().and_then(|m| m.get(&module_base).cloned())
    };

    if let Some(dll) = dll_name {
        resolve_import(&dll, func_name)
    } else {
        // Try all DLLs
        let key_suffix = alloc::format!("!{}", func_name);
        let table = DISPATCH.lock();
        table.as_ref().and_then(|t| {
            for (k, &v) in t.iter() {
                if k.ends_with(&key_suffix) {
                    return Some(v);
                }
            }
            None
        })
    }
}

/// Get the total number of registered functions.
pub fn function_count() -> usize {
    DISPATCH.lock().as_ref().map(|t| t.len()).unwrap_or(0)
}

/// List all registered DLLs with their function counts.
pub fn list_dlls() -> alloc::vec::Vec<(String, usize)> {
    let table = DISPATCH.lock();
    if let Some(ref t) = *table {
        let mut dlls = BTreeMap::new();
        for key in t.keys() {
            if let Some(bang) = key.find('!') {
                let dll = &key[..bang];
                *dlls.entry(String::from(dll)).or_insert(0usize) += 1;
            }
        }
        dlls.into_iter().collect()
    } else {
        alloc::vec::Vec::new()
    }
}
