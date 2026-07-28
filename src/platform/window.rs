use windows::Win32::Foundation::{GetLastError, HWND, POINT, RECT};
use windows::Win32::Graphics::Gdi::ClientToScreen;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClientRect, GetForegroundWindow, GetWindowTextLengthW, GetWindowTextW,
};

use crate::error::{AppError, AppResult};
use crate::image::{Point, Rect};

#[derive(Clone, Debug)]
pub struct GameWindow {
    pub handle: HWND,
    pub screen_bounds: Rect,
    pub title: String,
}

pub fn initialize_dpi_awareness() -> AppResult<()> {
    unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) }.map_err(
        |source| AppError::Windows {
            operation: "SetProcessDpiAwarenessContext",
            source,
        },
    )
}

pub fn matching_foreground_window(title_fragments: &[String]) -> AppResult<Option<GameWindow>> {
    let handle = unsafe { GetForegroundWindow() };
    if handle.0.is_null() {
        return Ok(None);
    }

    let title = window_title(handle)?;
    let normalized = title.to_lowercase();
    if !title_fragments
        .iter()
        .any(|fragment| normalized.contains(&fragment.to_lowercase()))
    {
        return Ok(None);
    }

    Ok(Some(GameWindow {
        handle,
        screen_bounds: client_screen_bounds(handle)?,
        title,
    }))
}

fn window_title(handle: HWND) -> AppResult<String> {
    let length = unsafe { GetWindowTextLengthW(handle) };
    if length == 0 {
        let code = unsafe { GetLastError() }.0;
        if code != 0 {
            return Err(AppError::Win32 {
                operation: "GetWindowTextLengthW",
                code,
            });
        }
        return Ok(String::new());
    }

    let mut buffer = vec![0_u16; length as usize + 1];
    let copied = unsafe { GetWindowTextW(handle, &mut buffer) };
    if copied == 0 {
        return Err(AppError::Win32 {
            operation: "GetWindowTextW",
            code: unsafe { GetLastError() }.0,
        });
    }
    Ok(String::from_utf16_lossy(&buffer[..copied as usize]))
}

fn client_screen_bounds(handle: HWND) -> AppResult<Rect> {
    let mut client = RECT::default();
    unsafe { GetClientRect(handle, &mut client) }.map_err(|source| AppError::Windows {
        operation: "GetClientRect",
        source,
    })?;
    let mut origin = POINT { x: 0, y: 0 };
    unsafe { ClientToScreen(handle, &mut origin) }
        .ok()
        .map_err(|source| AppError::Windows {
            operation: "ClientToScreen",
            source,
        })?;

    Ok(Rect {
        x: origin.x,
        y: origin.y,
        width: (client.right - client.left).max(0) as u32,
        height: (client.bottom - client.top).max(0) as u32,
    })
}

impl GameWindow {
    pub fn frame_origin(&self) -> Point {
        Point {
            x: self.screen_bounds.x,
            y: self.screen_bounds.y,
        }
    }
}
