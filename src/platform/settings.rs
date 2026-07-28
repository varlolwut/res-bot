use std::ffi::c_void;
use std::path::PathBuf;
use std::ptr;
use std::sync::mpsc::Sender;

use windows::Win32::Foundation::{GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{COLOR_WINDOW, GetSysColorBrush, InvalidateRect};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    BM_GETCHECK, BM_SETCHECK, BS_AUTOCHECKBOX, BS_DEFPUSHBUTTON, BS_PUSHBUTTON, CREATESTRUCTW,
    CreateWindowExW, DefWindowProcW, DestroyWindow, ES_AUTOHSCROLL, GWLP_USERDATA, GetClientRect,
    GetWindowLongPtrW, GetWindowTextLengthW, GetWindowTextW, HICON, HMENU, MINMAXINFO, MessageBoxW,
    MoveWindow, SW_SHOW, SWP_NOACTIVATE, SWP_NOZORDER, SendMessageW, SetForegroundWindow,
    SetWindowLongPtrW, SetWindowPos, SetWindowTextW, ShowWindow, WINDOW_EX_STYLE, WINDOW_STYLE,
    WM_CLOSE, WM_COMMAND, WM_DESTROY, WM_DPICHANGED, WM_GETMINMAXINFO, WM_NCCREATE, WM_SIZE,
    WS_BORDER, WS_CHILD, WS_OVERLAPPEDWINDOW, WS_TABSTOP, WS_VISIBLE,
};
use windows::core::{PCWSTR, w};

use crate::config::{Config, TargetMode};
use crate::error::{AppError, AppResult};
use crate::platform::write_debug_warning;

const CLASS_NAME: PCWSTR = w!("res-bot-settings-window");
const WINDOW_NAME: PCWSTR = w!("res-bot — настройки");
const STATIC_CLASS: PCWSTR = w!("STATIC");
const EDIT_CLASS: PCWSTR = w!("EDIT");
const BUTTON_CLASS: PCWSTR = w!("BUTTON");
const SAVE_LABEL: &str = "Сохранить";
const APPLY_LABEL: &str = "Применить";
const CANCEL_LABEL: &str = "Отмена";
const RELOCATE_CURSOR_LABEL: &str = "Случайно перемещать курсор после клика";
const SAVE_BUTTON_ID: usize = 1;
const APPLY_BUTTON_ID: usize = 2;
const CANCEL_BUTTON_ID: usize = 3;
const DEFAULT_DPI: u32 = 96;
const WINDOW_WIDTH: i32 = 720;
const WINDOW_HEIGHT: i32 = 610;
const PADDING: i32 = 16;
const ROW_HEIGHT: i32 = 36;
const LABEL_WIDTH: i32 = 310;
const BUTTON_WIDTH: i32 = 130;
const BUTTON_HEIGHT: i32 = 32;
const CHECKED_STATE: usize = 1;

const FIELD_LABELS: [&str; 10] = [
    "Фрагменты заголовка через запятую",
    "Интервал проверки, секунд",
    "Минимальная длительность движения, мс",
    "Максимальная длительность движения, мс",
    "Минимальная задержка перед проверкой, мс",
    "Максимальная задержка перед проверкой, мс",
    "Количество подтверждающих кадров (2–5)",
    "Интервал между кадрами, мс",
    "Необходимый простой мыши, мс",
    "Максимальное ожидание мыши, мс",
];

pub struct SettingsWindow {
    path: PathBuf,
    sender: Sender<Config>,
    current: Config,
    window: HWND,
    labels: Vec<HWND>,
    edits: Vec<HWND>,
    relocate_checkbox: HWND,
    save_button: HWND,
    apply_button: HWND,
    cancel_button: HWND,
    dpi: u32,
}

impl SettingsWindow {
    pub fn new(path: PathBuf, sender: Sender<Config>, current: Config) -> Self {
        Self {
            path,
            sender,
            current,
            window: null_window(),
            labels: Vec::new(),
            edits: Vec::new(),
            relocate_checkbox: null_window(),
            save_button: null_window(),
            apply_button: null_window(),
            cancel_button: null_window(),
            dpi: DEFAULT_DPI,
        }
    }

    pub fn register_class(instance: HINSTANCE, icon: HICON) -> AppResult<()> {
        let class = windows::Win32::UI::WindowsAndMessaging::WNDCLASSW {
            lpfnWndProc: Some(settings_window_procedure),
            hInstance: instance,
            hIcon: icon,
            hbrBackground: unsafe { GetSysColorBrush(COLOR_WINDOW) },
            lpszClassName: CLASS_NAME,
            ..Default::default()
        };
        let atom = unsafe { windows::Win32::UI::WindowsAndMessaging::RegisterClassW(&class) };
        if atom == 0 {
            return Err(last_win32_error("RegisterClassW settings"));
        }
        Ok(())
    }

    pub fn unregister_class(instance: HINSTANCE) -> AppResult<()> {
        unsafe {
            windows::Win32::UI::WindowsAndMessaging::UnregisterClassW(CLASS_NAME, Some(instance))
        }
        .map_err(|source| AppError::Windows {
            operation: "UnregisterClassW settings",
            source,
        })
    }

    pub fn open(&mut self, instance: HINSTANCE) -> AppResult<()> {
        if !self.window.0.is_null() {
            unsafe {
                let _ = ShowWindow(self.window, SW_SHOW);
                let _ = SetForegroundWindow(self.window);
            }
            return Ok(());
        }

        let state_pointer = self as *mut SettingsWindow;
        let window = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                CLASS_NAME,
                WINDOW_NAME,
                WS_OVERLAPPEDWINDOW,
                140,
                100,
                WINDOW_WIDTH,
                WINDOW_HEIGHT,
                None,
                None,
                Some(instance),
                Some(state_pointer.cast_const().cast()),
            )
        }
        .map_err(|source| AppError::Windows {
            operation: "CreateWindowExW settings",
            source,
        })?;
        self.window = window;
        self.dpi = unsafe { GetDpiForWindow(window) };
        self.create_controls(instance)?;
        self.populate()?;
        self.layout()?;
        unsafe {
            let _ = ShowWindow(window, SW_SHOW);
            let _ = SetForegroundWindow(window);
        }
        Ok(())
    }

    pub fn close(&mut self) -> AppResult<()> {
        if self.window.0.is_null() {
            return Ok(());
        }
        unsafe { DestroyWindow(self.window) }.map_err(|source| AppError::Windows {
            operation: "DestroyWindow settings",
            source,
        })
    }

    pub fn target_mode(&self) -> Option<TargetMode> {
        self.current.target_mode()
    }

    pub fn select_target_mode(&mut self, mode: TargetMode) -> AppResult<()> {
        let config = self.current.for_target_mode(mode);
        config.save(&self.path)?;
        self.sender
            .send(config.clone())
            .map_err(|_| AppError::SettingsChannel)?;
        self.current = config;
        if !self.window.0.is_null() {
            self.populate()?;
        }
        Ok(())
    }

    fn create_controls(&mut self, instance: HINSTANCE) -> AppResult<()> {
        self.labels = FIELD_LABELS
            .iter()
            .map(|label| {
                create_control(
                    instance,
                    self.window,
                    STATIC_CLASS,
                    &wide_string(label),
                    WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0),
                    0,
                )
            })
            .collect::<AppResult<Vec<HWND>>>()?;
        self.edits = FIELD_LABELS
            .iter()
            .map(|_| {
                create_control(
                    instance,
                    self.window,
                    EDIT_CLASS,
                    &[0],
                    WINDOW_STYLE(
                        WS_CHILD.0
                            | WS_VISIBLE.0
                            | WS_BORDER.0
                            | WS_TABSTOP.0
                            | ES_AUTOHSCROLL as u32,
                    ),
                    0,
                )
            })
            .collect::<AppResult<Vec<HWND>>>()?;
        self.relocate_checkbox = create_control(
            instance,
            self.window,
            BUTTON_CLASS,
            &wide_string(RELOCATE_CURSOR_LABEL),
            WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | BS_AUTOCHECKBOX as u32),
            0,
        )?;
        self.save_button = create_control(
            instance,
            self.window,
            BUTTON_CLASS,
            &wide_string(SAVE_LABEL),
            WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | BS_PUSHBUTTON as u32),
            SAVE_BUTTON_ID,
        )?;
        self.apply_button = create_control(
            instance,
            self.window,
            BUTTON_CLASS,
            &wide_string(APPLY_LABEL),
            WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | BS_DEFPUSHBUTTON as u32),
            APPLY_BUTTON_ID,
        )?;
        self.cancel_button = create_control(
            instance,
            self.window,
            BUTTON_CLASS,
            &wide_string(CANCEL_LABEL),
            WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | WS_TABSTOP.0 | BS_PUSHBUTTON as u32),
            CANCEL_BUTTON_ID,
        )?;
        Ok(())
    }

    fn populate(&self) -> AppResult<()> {
        let values = [
            self.current.window_title_fragments.join(", "),
            self.current.poll_interval_seconds.to_string(),
            self.current.click_min_duration_ms.to_string(),
            self.current.click_max_duration_ms.to_string(),
            self.current.pre_click_min_delay_ms.to_string(),
            self.current.pre_click_max_delay_ms.to_string(),
            self.current.confirmation_frame_count.to_string(),
            self.current.confirmation_interval_ms.to_string(),
            self.current.mouse_idle_required_ms.to_string(),
            self.current.mouse_idle_timeout_ms.to_string(),
        ];
        for (edit, value) in self.edits.iter().zip(values) {
            set_control_text(*edit, &value)?;
        }
        let checked = usize::from(self.current.relocate_cursor_after_click);
        unsafe {
            SendMessageW(
                self.relocate_checkbox,
                BM_SETCHECK,
                Some(WPARAM(checked)),
                None,
            );
        }
        Ok(())
    }

    fn read_config(&self) -> AppResult<Config> {
        let values = self
            .edits
            .iter()
            .map(|edit| control_text(*edit))
            .collect::<AppResult<Vec<String>>>()?;
        let title_fragments = values[0]
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .collect::<Vec<String>>();
        let config = Config {
            window_title_fragments: title_fragments,
            poll_interval_seconds: parse_u64("poll_interval_seconds", &values[1])?,
            click_min_duration_ms: parse_u64("click_min_duration_ms", &values[2])?,
            click_max_duration_ms: parse_u64("click_max_duration_ms", &values[3])?,
            pre_click_min_delay_ms: parse_u64("pre_click_min_delay_ms", &values[4])?,
            pre_click_max_delay_ms: parse_u64("pre_click_max_delay_ms", &values[5])?,
            confirmation_frame_count: parse_u32("confirmation_frame_count", &values[6])?,
            confirmation_interval_ms: parse_u64("confirmation_interval_ms", &values[7])?,
            mouse_idle_required_ms: parse_u64("mouse_idle_required_ms", &values[8])?,
            mouse_idle_timeout_ms: parse_u64("mouse_idle_timeout_ms", &values[9])?,
            relocate_cursor_after_click: checkbox_checked(self.relocate_checkbox),
        };
        config.validate()?;
        Ok(config)
    }

    fn apply(&mut self) -> AppResult<()> {
        let config = self.read_config()?;
        self.sender
            .send(config.clone())
            .map_err(|_| AppError::SettingsChannel)?;
        self.current = config;
        self.close()
    }

    fn save(&mut self) -> AppResult<()> {
        let config = self.read_config()?;
        config.save(&self.path)?;
        self.sender
            .send(config.clone())
            .map_err(|_| AppError::SettingsChannel)?;
        self.current = config;
        self.close()
    }

    fn layout(&self) -> AppResult<()> {
        if self.window.0.is_null() {
            return Ok(());
        }
        let mut client = RECT::default();
        unsafe { GetClientRect(self.window, &mut client) }.map_err(|source| AppError::Windows {
            operation: "GetClientRect settings",
            source,
        })?;
        let padding = scale_dimension(PADDING, self.dpi);
        let row_height = scale_dimension(ROW_HEIGHT, self.dpi);
        let label_width = scale_dimension(LABEL_WIDTH, self.dpi);
        let button_width = scale_dimension(BUTTON_WIDTH, self.dpi);
        let button_height = scale_dimension(BUTTON_HEIGHT, self.dpi);
        let width = client.right - client.left;

        for (index, (label, edit)) in self.labels.iter().zip(&self.edits).enumerate() {
            let y = padding + index as i32 * row_height;
            move_control(*label, padding, y + 6, label_width, row_height - 6)?;
            move_control(
                *edit,
                padding + label_width,
                y,
                width - padding * 2 - label_width,
                row_height - 6,
            )?;
        }
        let checkbox_y = padding + self.edits.len() as i32 * row_height;
        move_control(
            self.relocate_checkbox,
            padding,
            checkbox_y,
            width - padding * 2,
            row_height,
        )?;
        let button_y = client.bottom - padding - button_height;
        move_control(
            self.cancel_button,
            width - padding - button_width,
            button_y,
            button_width,
            button_height,
        )?;
        move_control(
            self.save_button,
            width - padding * 3 - button_width * 3,
            button_y,
            button_width,
            button_height,
        )?;
        move_control(
            self.apply_button,
            width - padding * 2 - button_width * 2,
            button_y,
            button_width,
            button_height,
        )?;
        if unsafe { InvalidateRect(Some(self.window), None, true) }.as_bool() {
            Ok(())
        } else {
            Err(last_win32_error("InvalidateRect settings"))
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
            operation: "SetWindowPos settings DPI change",
            source,
        })?;
        self.layout()
    }

    fn handle_destroyed(&mut self) {
        self.window = null_window();
        self.labels.clear();
        self.edits.clear();
        self.relocate_checkbox = null_window();
        self.save_button = null_window();
        self.apply_button = null_window();
        self.cancel_button = null_window();
    }
}

unsafe extern "system" fn settings_window_procedure(
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

    let state_pointer = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) } as *mut SettingsWindow;
    if !state_pointer.is_null() {
        let state = unsafe { &mut *state_pointer };
        match message {
            WM_COMMAND => {
                match wparam.0 & 0xffff {
                    SAVE_BUTTON_ID => {
                        if let Err(error) = state.save() {
                            show_settings_error(window, &error);
                        }
                    }
                    APPLY_BUTTON_ID => {
                        if let Err(error) = state.apply() {
                            show_settings_error(window, &error);
                        }
                    }
                    CANCEL_BUTTON_ID => {
                        if let Err(error) = state.close() {
                            write_debug_warning(&format!(
                                "failed to close settings: error={error}"
                            ));
                        }
                    }
                    _ => {}
                }
                return LRESULT(0);
            }
            WM_SIZE => {
                if let Err(error) = state.layout() {
                    write_debug_warning(&format!("failed to resize settings: error={error}"));
                }
                return LRESULT(0);
            }
            WM_DPICHANGED => {
                let dpi = (wparam.0 & 0xffff) as u32;
                if let Err(error) = state.apply_dpi_change(dpi, lparam.0) {
                    write_debug_warning(&format!(
                        "failed to apply settings DPI change: error={error}"
                    ));
                }
                return LRESULT(0);
            }
            WM_GETMINMAXINFO => {
                let limits = unsafe { &mut *(lparam.0 as *mut MINMAXINFO) };
                limits.ptMinTrackSize.x = scale_dimension(WINDOW_WIDTH, state.dpi);
                limits.ptMinTrackSize.y = scale_dimension(WINDOW_HEIGHT, state.dpi);
                return LRESULT(0);
            }
            WM_CLOSE => {
                if let Err(error) = unsafe { DestroyWindow(window) } {
                    write_debug_warning(&format!("failed to close settings: error={error}"));
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
    text: &[u16],
    style: WINDOW_STYLE,
    identifier: usize,
) -> AppResult<HWND> {
    let menu = (identifier != 0).then_some(HMENU(identifier as *mut c_void));
    unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            class_name,
            PCWSTR(text.as_ptr()),
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
        operation: "CreateWindowExW settings control",
        source,
    })
}

fn move_control(window: HWND, x: i32, y: i32, width: i32, height: i32) -> AppResult<()> {
    if window.0.is_null() {
        return Ok(());
    }
    unsafe { MoveWindow(window, x, y, width.max(1), height.max(1), true) }.map_err(|source| {
        AppError::Windows {
            operation: "MoveWindow settings control",
            source,
        }
    })
}

fn set_control_text(window: HWND, value: &str) -> AppResult<()> {
    let wide = wide_string(value);
    unsafe { SetWindowTextW(window, PCWSTR(wide.as_ptr())) }.map_err(|source| AppError::Windows {
        operation: "SetWindowTextW settings",
        source,
    })
}

fn control_text(window: HWND) -> AppResult<String> {
    let length = unsafe { GetWindowTextLengthW(window) };
    let mut buffer = vec![0_u16; length as usize + 1];
    let copied = unsafe { GetWindowTextW(window, &mut buffer) };
    if copied == 0 && length != 0 {
        return Err(last_win32_error("GetWindowTextW settings"));
    }
    Ok(String::from_utf16_lossy(&buffer[..copied as usize]))
}

fn checkbox_checked(window: HWND) -> bool {
    unsafe { SendMessageW(window, BM_GETCHECK, None, None) }.0 as usize == CHECKED_STATE
}

fn parse_u64(field: &'static str, value: &str) -> AppResult<u64> {
    value
        .trim()
        .parse::<u64>()
        .map_err(|_| AppError::ConfigValue {
            field,
            value: value.to_owned(),
            reason: "must be a non-negative integer",
        })
}

fn parse_u32(field: &'static str, value: &str) -> AppResult<u32> {
    value
        .trim()
        .parse::<u32>()
        .map_err(|_| AppError::ConfigValue {
            field,
            value: value.to_owned(),
            reason: "must be a non-negative integer",
        })
}

fn show_settings_error(parent: HWND, error: &AppError) {
    let text = wide_string(&format!(
        "Настройки не сохранены.\r\n\r\n{error}\r\n\r\nИсправьте значение и повторите."
    ));
    let title = wide_string("res-bot — ошибка настроек");
    unsafe {
        MessageBoxW(
            Some(parent),
            PCWSTR(text.as_ptr()),
            PCWSTR(title.as_ptr()),
            windows::Win32::UI::WindowsAndMessaging::MB_OK
                | windows::Win32::UI::WindowsAndMessaging::MB_ICONERROR,
        );
    }
}

fn scale_dimension(value: i32, dpi: u32) -> i32 {
    ((i64::from(value) * i64::from(dpi) + i64::from(DEFAULT_DPI / 2)) / i64::from(DEFAULT_DPI))
        as i32
}

fn last_win32_error(operation: &'static str) -> AppError {
    AppError::Win32 {
        operation,
        code: unsafe { GetLastError() }.0,
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
    use std::env;
    use std::ptr;
    use std::sync::mpsc;

    use windows::Win32::Foundation::{HINSTANCE, HMODULE};
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::UI::WindowsAndMessaging::HICON;

    use crate::config::Config;

    use super::{SettingsWindow, parse_u32, parse_u64};

    #[test]
    fn numeric_fields_require_integers() {
        assert_eq!(parse_u64("poll_interval_seconds", " 10 ").unwrap(), 10);
        assert_eq!(parse_u32("confirmation_frame_count", "3").unwrap(), 3);
        assert!(parse_u64("poll_interval_seconds", "1.5").is_err());
    }

    #[test]
    fn settings_window_opens_populates_and_closes() {
        let module: HMODULE = unsafe { GetModuleHandleW(None) }.unwrap();
        let instance = HINSTANCE(module.0);
        let (sender, _receiver) = mpsc::channel::<Config>();
        let path = env::temp_dir().join("res-bot-settings-test.toml");
        let mut settings = SettingsWindow::new(path, sender, Config::built_in());

        SettingsWindow::register_class(instance, HICON(ptr::null_mut())).unwrap();
        settings.open(instance).unwrap();

        assert_eq!(settings.edits.len(), 10);
        assert_eq!(settings.read_config().unwrap(), Config::built_in());

        settings.close().unwrap();
        SettingsWindow::unregister_class(instance).unwrap();
    }
}
