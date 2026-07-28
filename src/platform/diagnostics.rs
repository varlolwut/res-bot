use std::collections::VecDeque;
use std::ffi::c_void;
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::Instant;

use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{COLOR_WINDOW, GetSysColorBrush, InvalidateRect};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    BS_PUSHBUTTON, CREATESTRUCTW, CreateWindowExW, DefWindowProcW, DestroyWindow, ES_AUTOVSCROLL,
    ES_MULTILINE, ES_READONLY, ES_WANTRETURN, GWLP_USERDATA, GetClientRect, GetWindowLongPtrW,
    HICON, HMENU, MoveWindow, SW_SHOWNOACTIVATE, SWP_NOACTIVATE, SWP_NOZORDER, SendMessageW,
    SetWindowLongPtrW, SetWindowPos, SetWindowTextW, ShowWindow, WINDOW_EX_STYLE, WINDOW_STYLE,
    WM_CLOSE, WM_COMMAND, WM_COPY, WM_DESTROY, WM_DPICHANGED, WM_NCCREATE, WM_SIZE, WS_BORDER,
    WS_CHILD, WS_EX_NOACTIVATE, WS_EX_TOPMOST, WS_OVERLAPPEDWINDOW, WS_VISIBLE, WS_VSCROLL,
};
use windows::core::{PCWSTR, w};

use crate::error::{AppError, AppResult};
use crate::platform::write_debug_warning;

const CLASS_NAME: PCWSTR = w!("res-bot-diagnostics-window");
const WINDOW_NAME: PCWSTR = w!("res-bot — диагностика (запись включена)");
const EDIT_CLASS: PCWSTR = w!("EDIT");
const BUTTON_CLASS: PCWSTR = w!("BUTTON");
const CLEAR_LABEL: PCWSTR = w!("Очистить");
const COPY_LABEL: PCWSTR = w!("Копировать");
const CLEAR_BUTTON_ID: usize = 1;
const COPY_BUTTON_ID: usize = 2;
const MAX_LINES: usize = 500;
const WINDOW_WIDTH: i32 = 780;
const WINDOW_HEIGHT: i32 = 480;
const PADDING: i32 = 12;
const BUTTON_WIDTH: i32 = 126;
const BUTTON_HEIGHT: i32 = 30;
const DEFAULT_DPI: u32 = 96;
const EM_SETSEL: u32 = 0x00B1;
const EM_SCROLLCARET: u32 = 0x00B7;

pub struct DiagnosticWindow {
    enabled: Arc<AtomicBool>,
    receiver: Receiver<String>,
    window: HWND,
    edit: HWND,
    clear_button: HWND,
    copy_button: HWND,
    lines: VecDeque<String>,
    session_started: Option<Instant>,
    dpi: u32,
}

impl DiagnosticWindow {
    pub fn new(enabled: Arc<AtomicBool>, receiver: Receiver<String>) -> Self {
        Self {
            enabled,
            receiver,
            window: null_window(),
            edit: null_window(),
            clear_button: null_window(),
            copy_button: null_window(),
            lines: VecDeque::new(),
            session_started: None,
            dpi: DEFAULT_DPI,
        }
    }

    pub fn register_class(instance: HINSTANCE, icon: HICON) -> AppResult<()> {
        let class = windows::Win32::UI::WindowsAndMessaging::WNDCLASSW {
            lpfnWndProc: Some(diagnostic_window_procedure),
            hInstance: instance,
            hIcon: icon,
            hbrBackground: unsafe { GetSysColorBrush(COLOR_WINDOW) },
            lpszClassName: CLASS_NAME,
            ..Default::default()
        };
        let atom = unsafe { windows::Win32::UI::WindowsAndMessaging::RegisterClassW(&class) };
        if atom == 0 {
            return Err(last_win32_error("RegisterClassW diagnostics"));
        }
        Ok(())
    }

    pub fn unregister_class(instance: HINSTANCE) -> AppResult<()> {
        unsafe {
            windows::Win32::UI::WindowsAndMessaging::UnregisterClassW(CLASS_NAME, Some(instance))
        }
        .map_err(|source| AppError::Windows {
            operation: "UnregisterClassW diagnostics",
            source,
        })
    }

    pub fn open(&mut self, instance: HINSTANCE) -> AppResult<()> {
        if !self.window.0.is_null() {
            unsafe {
                let _ = ShowWindow(self.window, SW_SHOWNOACTIVATE);
            }
            return Ok(());
        }

        self.discard_pending_events();
        self.lines.clear();
        self.session_started = Some(Instant::now());
        let state_pointer = self as *mut DiagnosticWindow;
        let window = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(WS_EX_NOACTIVATE.0 | WS_EX_TOPMOST.0),
                CLASS_NAME,
                WINDOW_NAME,
                WS_OVERLAPPEDWINDOW,
                120,
                120,
                WINDOW_WIDTH,
                WINDOW_HEIGHT,
                None,
                None,
                Some(instance),
                Some(state_pointer.cast_const().cast()),
            )
        }
        .map_err(|source| AppError::Windows {
            operation: "CreateWindowExW diagnostics",
            source,
        })?;
        self.window = window;
        self.dpi = unsafe { GetDpiForWindow(window) };

        let edit_style = WINDOW_STYLE(
            WS_CHILD.0
                | WS_VISIBLE.0
                | WS_BORDER.0
                | WS_VSCROLL.0
                | ES_MULTILINE as u32
                | ES_AUTOVSCROLL as u32
                | ES_READONLY as u32
                | ES_WANTRETURN as u32,
        );
        self.edit = create_control(instance, window, EDIT_CLASS, PCWSTR::null(), edit_style, 0)?;
        self.clear_button = create_control(
            instance,
            window,
            BUTTON_CLASS,
            CLEAR_LABEL,
            WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | BS_PUSHBUTTON as u32),
            CLEAR_BUTTON_ID,
        )?;
        self.copy_button = create_control(
            instance,
            window,
            BUTTON_CLASS,
            COPY_LABEL,
            WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | BS_PUSHBUTTON as u32),
            COPY_BUTTON_ID,
        )?;
        self.layout()?;
        self.enabled.store(true, Ordering::Release);
        self.append_line(
            "Диагностика включена. Вернитесь в Lineage II; закрытие этого окна остановит запись."
                .to_owned(),
        )?;
        unsafe {
            let _ = ShowWindow(window, SW_SHOWNOACTIVATE);
        }
        Ok(())
    }

    pub fn close(&mut self) -> AppResult<()> {
        self.enabled.store(false, Ordering::Release);
        if self.window.0.is_null() {
            return Ok(());
        }
        unsafe { DestroyWindow(self.window) }.map_err(|source| AppError::Windows {
            operation: "DestroyWindow diagnostics",
            source,
        })
    }

    pub fn drain_events(&mut self) -> AppResult<()> {
        loop {
            match self.receiver.try_recv() {
                Ok(message) => self.append_line(message)?,
                Err(TryRecvError::Empty) => return Ok(()),
                Err(TryRecvError::Disconnected) => {
                    self.enabled.store(false, Ordering::Release);
                    return Ok(());
                }
            }
        }
    }

    fn discard_pending_events(&self) {
        loop {
            match self.receiver.try_recv() {
                Ok(_) => {}
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => return,
            }
        }
    }

    fn append_line(&mut self, message: String) -> AppResult<()> {
        let elapsed = self
            .session_started
            .map(|started| started.elapsed())
            .unwrap_or_default();
        self.lines.push_back(format!(
            "[+{:04}.{:03}] {message}",
            elapsed.as_secs(),
            elapsed.subsec_millis()
        ));
        if self.lines.len() > MAX_LINES {
            self.lines.pop_front();
        }
        self.render()
    }

    fn render(&self) -> AppResult<()> {
        if self.edit.0.is_null() {
            return Ok(());
        }
        let text = self
            .lines
            .iter()
            .cloned()
            .collect::<Vec<String>>()
            .join("\r\n");
        let wide = wide_string(&text);
        unsafe { SetWindowTextW(self.edit, PCWSTR(wide.as_ptr())) }.map_err(|source| {
            AppError::Windows {
                operation: "SetWindowTextW diagnostics",
                source,
            }
        })?;
        let end = text.encode_utf16().count();
        unsafe {
            SendMessageW(
                self.edit,
                EM_SETSEL,
                Some(WPARAM(end)),
                Some(LPARAM(end as isize)),
            );
            SendMessageW(self.edit, EM_SCROLLCARET, None, None);
        }
        Ok(())
    }

    fn clear(&mut self) -> AppResult<()> {
        self.lines.clear();
        self.session_started = Some(Instant::now());
        self.append_line("Журнал очищен.".to_owned())
    }

    fn copy_all(&self) {
        if self.edit.0.is_null() {
            return;
        }
        unsafe {
            SendMessageW(self.edit, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));
            SendMessageW(self.edit, WM_COPY, None, None);
            SendMessageW(
                self.edit,
                EM_SETSEL,
                Some(WPARAM(usize::MAX)),
                Some(LPARAM(-1)),
            );
        }
    }

    fn layout(&self) -> AppResult<()> {
        if self.window.0.is_null() {
            return Ok(());
        }
        let mut client = RECT::default();
        unsafe { GetClientRect(self.window, &mut client) }.map_err(|source| AppError::Windows {
            operation: "GetClientRect diagnostics",
            source,
        })?;
        let padding = scale_dimension(PADDING, self.dpi);
        let button_width = scale_dimension(BUTTON_WIDTH, self.dpi);
        let button_height = scale_dimension(BUTTON_HEIGHT, self.dpi);
        let minimum_edit_height = scale_dimension(40, self.dpi);
        let width = (client.right - client.left).max(padding * 2 + button_width * 2);
        let height =
            (client.bottom - client.top).max(padding * 3 + button_height + minimum_edit_height);
        let edit_height = height - padding * 3 - button_height;
        move_control(
            self.edit,
            padding,
            padding,
            width - padding * 2,
            edit_height,
        )?;
        let buttons_y = padding * 2 + edit_height;
        move_control(
            self.copy_button,
            width - padding - button_width,
            buttons_y,
            button_width,
            button_height,
        )?;
        move_control(
            self.clear_button,
            width - padding * 2 - button_width * 2,
            buttons_y,
            button_width,
            button_height,
        )?;
        if unsafe { InvalidateRect(Some(self.window), None, true) }.as_bool() {
            Ok(())
        } else {
            Err(last_win32_error("InvalidateRect diagnostics"))
        }
    }

    fn apply_dpi_change(&mut self, dpi: u32, suggested_rect: isize) -> AppResult<()> {
        if dpi == 0 || suggested_rect == 0 {
            return Err(AppError::InvalidDpiChange {
                dpi,
                suggested_rect,
            });
        }
        let rect = unsafe { &*(suggested_rect as *const RECT) };
        self.dpi = dpi;
        unsafe {
            SetWindowPos(
                self.window,
                None,
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
                SWP_NOACTIVATE | SWP_NOZORDER,
            )
        }
        .map_err(|source| AppError::Windows {
            operation: "SetWindowPos diagnostics DPI change",
            source,
        })?;
        self.layout()
    }

    fn handle_destroyed(&mut self) {
        self.enabled.store(false, Ordering::Release);
        self.window = null_window();
        self.edit = null_window();
        self.clear_button = null_window();
        self.copy_button = null_window();
        self.session_started = None;
    }
}

unsafe extern "system" fn diagnostic_window_procedure(
    window: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        let create = unsafe { &*(lparam.0 as *const CREATESTRUCTW) };
        unsafe {
            SetWindowLongPtrW(window, GWLP_USERDATA, create.lpCreateParams as isize);
        }
        return LRESULT(1);
    }

    let state_pointer =
        unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) } as *mut DiagnosticWindow;
    if !state_pointer.is_null() {
        let state = unsafe { &mut *state_pointer };
        match message {
            WM_COMMAND => {
                match wparam.0 & 0xffff {
                    CLEAR_BUTTON_ID => {
                        if let Err(error) = state.clear() {
                            write_debug_warning(&format!(
                                "failed to clear diagnostics: error={error}"
                            ));
                        }
                    }
                    COPY_BUTTON_ID => state.copy_all(),
                    _ => {}
                }
                return LRESULT(0);
            }
            WM_SIZE => {
                if let Err(error) = state.layout() {
                    write_debug_warning(&format!("failed to resize diagnostics: error={error}"));
                }
                return LRESULT(0);
            }
            WM_DPICHANGED => {
                let dpi = (wparam.0 & 0xffff) as u32;
                if let Err(error) = state.apply_dpi_change(dpi, lparam.0) {
                    write_debug_warning(&format!(
                        "failed to apply diagnostics DPI change: error={error}"
                    ));
                }
                return LRESULT(0);
            }
            WM_CLOSE => {
                state.enabled.store(false, Ordering::Release);
                if let Err(error) = unsafe { DestroyWindow(window) } {
                    write_debug_warning(&format!("failed to close diagnostics: error={error}"));
                }
                return LRESULT(0);
            }
            WM_DESTROY => {
                state.handle_destroyed();
                return LRESULT(0);
            }
            _ => {}
        }
    }

    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}

fn create_control(
    instance: HINSTANCE,
    parent: HWND,
    class_name: PCWSTR,
    text: PCWSTR,
    style: WINDOW_STYLE,
    identifier: usize,
) -> AppResult<HWND> {
    let menu = (identifier != 0).then_some(HMENU(identifier as *mut c_void));
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class_name,
            text,
            style,
            0,
            0,
            0,
            0,
            Some(parent),
            menu,
            Some(instance),
            None,
        )
    }
    .map_err(|source| AppError::Windows {
        operation: "CreateWindowExW diagnostic control",
        source,
    })
}

fn move_control(window: HWND, x: i32, y: i32, width: i32, height: i32) -> AppResult<()> {
    if window.0.is_null() {
        return Ok(());
    }
    unsafe { MoveWindow(window, x, y, width.max(1), height.max(1), true) }.map_err(|source| {
        AppError::Windows {
            operation: "MoveWindow diagnostic control",
            source,
        }
    })
}

fn scale_dimension(value: i32, dpi: u32) -> i32 {
    ((i64::from(value) * i64::from(dpi) + i64::from(DEFAULT_DPI / 2)) / i64::from(DEFAULT_DPI))
        as i32
}

fn last_win32_error(operation: &'static str) -> AppError {
    AppError::Win32 {
        operation,
        code: unsafe { windows::Win32::Foundation::GetLastError() }.0,
    }
}

fn wide_string(value: &str) -> Vec<u16> {
    value.encode_utf16().chain([0]).collect()
}

fn null_window() -> HWND {
    HWND(ptr::null_mut())
}

#[cfg(test)]
mod tests {
    use std::ptr;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::mpsc;

    use windows::Win32::Foundation::{HINSTANCE, HMODULE, RECT};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowRect, HICON};

    use super::DiagnosticWindow;
    use super::scale_dimension;

    #[test]
    fn dimensions_scale_for_monitor_dpi() {
        assert_eq!(scale_dimension(12, 96), 12);
        assert_eq!(scale_dimension(12, 144), 18);
        assert_eq!(scale_dimension(30, 192), 60);
    }

    #[test]
    fn diagnostic_window_opens_renders_event_and_closes() {
        let module: HMODULE = unsafe { GetModuleHandleW(None) }.unwrap();
        let instance = HINSTANCE(module.0);
        let enabled = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = mpsc::channel::<String>();
        let mut diagnostics = DiagnosticWindow::new(Arc::clone(&enabled), receiver);

        DiagnosticWindow::register_class(instance, HICON(ptr::null_mut())).unwrap();
        diagnostics.open(instance).unwrap();
        sender.send("test diagnostic event".to_owned()).unwrap();
        diagnostics.drain_events().unwrap();
        let suggested_rect = RECT {
            left: 160,
            top: 160,
            right: 760,
            bottom: 520,
        };
        diagnostics
            .apply_dpi_change(144, ptr::from_ref(&suggested_rect) as isize)
            .unwrap();

        assert!(enabled.load(Ordering::Acquire));
        assert_eq!(diagnostics.dpi, 144);
        assert!(
            diagnostics
                .lines
                .iter()
                .any(|line| line.contains("test diagnostic event"))
        );
        let mut window_rect = RECT::default();
        let mut clear_rect = RECT::default();
        let mut copy_rect = RECT::default();
        unsafe {
            GetWindowRect(diagnostics.window, &mut window_rect).unwrap();
            GetWindowRect(diagnostics.clear_button, &mut clear_rect).unwrap();
            GetWindowRect(diagnostics.copy_button, &mut copy_rect).unwrap();
        }
        assert!(clear_rect.right < copy_rect.left);
        assert!(clear_rect.bottom <= window_rect.bottom);
        assert!(copy_rect.bottom <= window_rect.bottom);

        diagnostics.close().unwrap();
        DiagnosticWindow::unregister_class(instance).unwrap();
        assert!(!enabled.load(Ordering::Acquire));
    }
}
