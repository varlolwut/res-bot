use std::ffi::c_void;
use std::ptr;
use std::slice;
use std::thread;
use std::time::Duration;

use windows::Win32::Foundation::GetLastError;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BitBlt, CreateCompatibleDC, CreateDIBSection,
    DIB_RGB_COLORS, DeleteDC, DeleteObject, GetDC, HBITMAP, HDC, HGDIOBJ, ReleaseDC, SRCCOPY,
    SelectObject,
};

use crate::error::{AppError, AppResult};
use crate::image::Frame;
use crate::platform::GameWindow;
use crate::platform::write_debug_warning;

pub fn capture_window(window: &GameWindow) -> AppResult<Frame> {
    let mut last_error = None::<AppError>;
    for attempt in 1..=3 {
        match capture_once(window) {
            Ok(frame) => return Ok(frame),
            Err(error) => {
                write_debug_warning(&format!(
                    "screen capture failed; attempt={attempt}, error={error}"
                ));
                last_error = Some(error);
                thread::sleep(Duration::from_millis(80 * attempt));
            }
        }
    }
    Err(last_error.expect("capture retry loop always records an error"))
}

fn capture_once(window: &GameWindow) -> AppResult<Frame> {
    let bounds = window.screen_bounds;
    if bounds.width == 0 || bounds.height == 0 {
        return Err(AppError::InvalidImage {
            width: bounds.width,
            height: bounds.height,
            bytes: 0,
        });
    }

    let screen_dc = ScreenDc::acquire()?;
    let memory_dc = MemoryDc::create(screen_dc.handle)?;
    let bitmap = DibSection::create(memory_dc.handle, bounds.width, bounds.height)?;
    let previous = unsafe { SelectObject(memory_dc.handle, HGDIOBJ(bitmap.handle.0)) };
    if previous.0.is_null() {
        return Err(last_win32_error("SelectObject"));
    }

    let copied = unsafe {
        BitBlt(
            memory_dc.handle,
            0,
            0,
            bounds.width as i32,
            bounds.height as i32,
            Some(screen_dc.handle),
            bounds.x,
            bounds.y,
            SRCCOPY,
        )
    };
    let _ = unsafe { SelectObject(memory_dc.handle, previous) };
    copied.map_err(|source| AppError::Windows {
        operation: "BitBlt",
        source,
    })?;

    let length = bounds.width as usize * bounds.height as usize * 4;
    let pixels = unsafe { slice::from_raw_parts(bitmap.bits.cast::<u8>(), length) }.to_vec();
    Frame::new(window.frame_origin(), bounds.width, bounds.height, pixels)
}

struct ScreenDc {
    handle: HDC,
}

impl ScreenDc {
    fn acquire() -> AppResult<Self> {
        let handle = unsafe { GetDC(None) };
        if handle.0.is_null() {
            return Err(last_win32_error("GetDC"));
        }
        Ok(Self { handle })
    }
}

impl Drop for ScreenDc {
    fn drop(&mut self) {
        let _ = unsafe { ReleaseDC(None, self.handle) };
    }
}

struct MemoryDc {
    handle: HDC,
}

impl MemoryDc {
    fn create(source: HDC) -> AppResult<Self> {
        let handle = unsafe { CreateCompatibleDC(Some(source)) };
        if handle.0.is_null() {
            return Err(last_win32_error("CreateCompatibleDC"));
        }
        Ok(Self { handle })
    }
}

impl Drop for MemoryDc {
    fn drop(&mut self) {
        let _ = unsafe { DeleteDC(self.handle) };
    }
}

struct DibSection {
    handle: HBITMAP,
    bits: *mut c_void,
}

impl DibSection {
    fn create(device_context: HDC, width: u32, height: u32) -> AppResult<Self> {
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width as i32,
                biHeight: -(height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = ptr::null_mut::<c_void>();
        let handle = unsafe {
            CreateDIBSection(
                Some(device_context),
                &info,
                DIB_RGB_COLORS,
                &mut bits,
                None,
                0,
            )
        }
        .map_err(|source| AppError::Windows {
            operation: "CreateDIBSection",
            source,
        })?;
        if bits.is_null() {
            return Err(last_win32_error("CreateDIBSection bits"));
        }
        Ok(Self { handle, bits })
    }
}

impl Drop for DibSection {
    fn drop(&mut self) {
        let _ = unsafe { DeleteObject(HGDIOBJ(self.handle.0)) };
    }
}

fn last_win32_error(operation: &'static str) -> AppError {
    AppError::Win32 {
        operation,
        code: unsafe { GetLastError() }.0,
    }
}
