use windows::Win32::System::Diagnostics::Debug::OutputDebugStringW;
use windows::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MB_OK, MessageBoxW};
use windows::core::PCWSTR;

pub fn write_debug_warning(message: &str) {
    let wide = wide_string(&format!("FarNav warning: {message}"));
    unsafe { OutputDebugStringW(PCWSTR(wide.as_ptr())) };
}

pub fn show_fatal_error(message: &str) {
    let body = wide_string(message);
    let title = wide_string("FarNav — ошибка");
    let _ = unsafe {
        MessageBoxW(
            None,
            PCWSTR(body.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OK | MB_ICONERROR,
        )
    };
}

fn wide_string(value: &str) -> Vec<u16> {
    value.encode_utf16().chain([0]).collect()
}
