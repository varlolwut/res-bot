use std::ffi::c_void;
use std::path::PathBuf;
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::thread::{self, JoinHandle};

use windows::Win32::Foundation::{GetLastError, HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CREATESTRUCTW, CreateIconFromResourceEx, CreatePopupMenu, CreateWindowExW,
    DefWindowProcW, DestroyIcon, DestroyMenu, DestroyWindow, DispatchMessageW, GWLP_USERDATA,
    GetCursorPos, GetMessageW, GetWindowLongPtrW, HICON, IDOK, LR_DEFAULTCOLOR, MB_ICONINFORMATION,
    MB_OK, MF_CHECKED, MF_POPUP, MF_SEPARATOR, MF_STRING, MF_UNCHECKED, MSG, MessageBoxW,
    PostMessageW, PostQuitMessage, RegisterClassW, RegisterWindowMessageW, SetForegroundWindow,
    SetWindowLongPtrW, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenu, TranslateMessage,
    UnregisterClassW, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLOSE, WM_CONTEXTMENU, WM_DESTROY,
    WM_LBUTTONUP, WM_NCCREATE, WM_RBUTTONUP, WM_USER, WNDCLASSW,
};
use windows::core::{PCWSTR, w};

use crate::config::{Config, TargetMode};
use crate::error::{AppError, AppResult};
use crate::platform::{
    diagnostics::DiagnosticWindow, settings::SettingsWindow, tray_icon::ICON_BYTES,
    write_debug_warning,
};

const TRAY_ICON_ID: u32 = 1;
const SETTINGS_MENU_ID: usize = 1;
const DIAGNOSTICS_MENU_ID: usize = 2;
const SELF_CHECK_MENU_ID: usize = 3;
const EXIT_MENU_ID: usize = 4;
const LINEAGE_MODE_MENU_ID: usize = 5;
const PARSEC_MODE_MENU_ID: usize = 6;
const TRAY_CALLBACK_MESSAGE: u32 = WM_USER + 1;
const DIAGNOSTIC_EVENT_MESSAGE: u32 = WM_USER + 2;
const SELF_CHECK_REPORT_MESSAGE: u32 = WM_USER + 3;
const ICON_SIZE: i32 = 16;
const ICON_RESOURCE_VERSION: u32 = 0x0003_0000;
const CLASS_NAME: PCWSTR = w!("res-bot-tray-window");
const WINDOW_NAME: PCWSTR = w!("res-bot");
const SETTINGS_LABEL: PCWSTR = w!("Настройки");
const DIAGNOSTICS_LABEL: PCWSTR = w!("Диагностика");
const SELF_CHECK_LABEL: PCWSTR = w!("Самопроверка");
const TARGET_MODE_LABEL: PCWSTR = w!("Источник изображения");
const LINEAGE_MODE_LABEL: PCWSTR = w!("Lineage II");
const PARSEC_MODE_LABEL: PCWSTR = w!("Parsec");
const EXIT_LABEL: PCWSTR = w!("Выход");
const TASKBAR_CREATED: PCWSTR = w!("TaskbarCreated");

pub struct TrayConnector {
    exit_requested: Arc<AtomicBool>,
    diagnostics_enabled: Arc<AtomicBool>,
    diagnostics_sender: Sender<String>,
    self_check_requested: Arc<AtomicBool>,
    self_check_sender: Sender<String>,
    settings_receiver: Receiver<Config>,
    window: usize,
    thread: Option<JoinHandle<AppResult<()>>>,
}

impl TrayConnector {
    pub fn start(config: Config, config_path: PathBuf) -> AppResult<Self> {
        let exit_requested = Arc::new(AtomicBool::new(false));
        let thread_exit_requested = Arc::clone(&exit_requested);
        let diagnostics_enabled = Arc::new(AtomicBool::new(false));
        let thread_diagnostics_enabled = Arc::clone(&diagnostics_enabled);
        let (diagnostics_sender, diagnostics_receiver) = mpsc::channel::<String>();
        let self_check_requested = Arc::new(AtomicBool::new(false));
        let thread_self_check_requested = Arc::clone(&self_check_requested);
        let (self_check_sender, self_check_receiver) = mpsc::channel::<String>();
        let (settings_sender, settings_receiver) = mpsc::channel::<Config>();
        let (ready_sender, ready_receiver) = mpsc::sync_channel::<AppResult<usize>>(1);
        let inputs = TrayThreadInputs {
            exit_requested: thread_exit_requested,
            diagnostics_enabled: thread_diagnostics_enabled,
            diagnostics_receiver,
            self_check_requested: thread_self_check_requested,
            self_check_receiver,
            settings_sender,
            config,
            config_path,
        };
        let thread = thread::spawn(move || tray_thread(inputs, ready_sender));
        let window = ready_receiver
            .recv()
            .map_err(|source| AppError::TrayThread {
                reason: format!("tray startup channel closed: {source}"),
            })??;

        Ok(Self {
            exit_requested,
            diagnostics_enabled,
            diagnostics_sender,
            self_check_requested,
            self_check_sender,
            settings_receiver,
            window,
            thread: Some(thread),
        })
    }

    pub fn exit_requested(&self) -> bool {
        self.exit_requested.load(Ordering::Acquire)
    }

    pub fn take_self_check_requested(&self) -> bool {
        self.self_check_requested.swap(false, Ordering::AcqRel)
    }

    pub fn complete_self_check(&self, report: String) -> AppResult<()> {
        self.self_check_sender
            .send(report)
            .map_err(|_| AppError::SelfCheckChannel)?;
        let window = HWND(self.window as *mut c_void);
        unsafe {
            PostMessageW(
                Some(window),
                SELF_CHECK_REPORT_MESSAGE,
                WPARAM(0),
                LPARAM(0),
            )
        }
        .map_err(|source| AppError::Windows {
            operation: "PostMessageW self-check report",
            source,
        })
    }

    pub fn take_config_update(&self) -> AppResult<Option<Config>> {
        let mut latest = None;
        loop {
            match self.settings_receiver.try_recv() {
                Ok(config) => latest = Some(config),
                Err(std::sync::mpsc::TryRecvError::Empty) => return Ok(latest),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    return Err(AppError::SettingsChannel);
                }
            }
        }
    }

    #[cfg(test)]
    pub fn disabled_for_test() -> Self {
        let (diagnostics_sender, _diagnostics_receiver) = mpsc::channel::<String>();
        let (self_check_sender, _self_check_receiver) = mpsc::channel::<String>();
        let (_settings_sender, settings_receiver) = mpsc::channel::<Config>();
        Self {
            exit_requested: Arc::new(AtomicBool::new(false)),
            diagnostics_enabled: Arc::new(AtomicBool::new(false)),
            diagnostics_sender,
            self_check_requested: Arc::new(AtomicBool::new(false)),
            self_check_sender,
            settings_receiver,
            window: 0,
            thread: None,
        }
    }

    pub fn log_diagnostic<F>(&self, create_message: F) -> AppResult<()>
    where
        F: FnOnce() -> String,
    {
        if !self.diagnostics_enabled.load(Ordering::Acquire) {
            return Ok(());
        }
        let message = create_message();
        self.diagnostics_sender
            .send(message)
            .map_err(|source| AppError::DiagnosticChannel { event: source.0 })?;
        let window = HWND(self.window as *mut c_void);
        unsafe { PostMessageW(Some(window), DIAGNOSTIC_EVENT_MESSAGE, WPARAM(0), LPARAM(0)) }
            .map_err(|source| AppError::Windows {
                operation: "PostMessageW diagnostics",
                source,
            })
    }

    pub fn shutdown(mut self) -> AppResult<()> {
        self.stop_thread();
        let thread = self
            .thread
            .take()
            .expect("tray thread exists until explicit shutdown");
        thread.join().map_err(|payload| AppError::TrayThread {
            reason: panic_description(payload),
        })?
    }

    fn stop_thread(&self) {
        let thread_finished = self.thread.as_ref().is_none_or(JoinHandle::is_finished);
        if thread_finished {
            return;
        }
        let window = HWND(self.window as *mut c_void);
        if let Err(error) = unsafe { PostMessageW(Some(window), WM_CLOSE, WPARAM(0), LPARAM(0)) } {
            write_debug_warning(&format!("failed to stop tray thread: error={error}"));
        }
    }
}

impl Drop for TrayConnector {
    fn drop(&mut self) {
        self.stop_thread();
        let Some(thread) = self.thread.take() else {
            return;
        };
        match thread.join() {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                write_debug_warning(&format!("tray shutdown failed: error={error}"));
            }
            Err(payload) => {
                write_debug_warning(&format!(
                    "tray thread panicked: reason={}",
                    panic_description(payload)
                ));
            }
        }
    }
}

struct TrayThreadState {
    exit_requested: Arc<AtomicBool>,
    taskbar_created_message: u32,
    icon: HICON,
    instance: HINSTANCE,
    diagnostics: DiagnosticWindow,
    settings: SettingsWindow,
    self_check_requested: Arc<AtomicBool>,
    self_check_receiver: Receiver<String>,
    error: Option<AppError>,
}

struct TrayThreadInputs {
    exit_requested: Arc<AtomicBool>,
    diagnostics_enabled: Arc<AtomicBool>,
    diagnostics_receiver: Receiver<String>,
    self_check_requested: Arc<AtomicBool>,
    self_check_receiver: Receiver<String>,
    settings_sender: Sender<Config>,
    config: Config,
    config_path: PathBuf,
}

fn tray_thread(
    inputs: TrayThreadInputs,
    ready_sender: SyncSender<AppResult<usize>>,
) -> AppResult<()> {
    match initialize_tray(inputs) {
        Ok(resources) => {
            ready_sender
                .send(Ok(resources.window.0 as usize))
                .map_err(|source| AppError::TrayThread {
                    reason: format!("tray startup receiver closed: {source}"),
                })?;
            run_message_loop(resources)
        }
        Err(error) => {
            ready_sender
                .send(Err(error))
                .map_err(|source| AppError::TrayThread {
                    reason: format!("tray startup receiver closed: {source}"),
                })?;
            Ok(())
        }
    }
}

struct TrayResources {
    window: HWND,
    icon: HICON,
    instance: HINSTANCE,
    state: *mut TrayThreadState,
}

fn initialize_tray(inputs: TrayThreadInputs) -> AppResult<TrayResources> {
    let TrayThreadInputs {
        exit_requested,
        diagnostics_enabled,
        diagnostics_receiver,
        self_check_requested,
        self_check_receiver,
        settings_sender,
        config,
        config_path,
    } = inputs;
    let module = unsafe { GetModuleHandleW(None) }.map_err(|source| AppError::Windows {
        operation: "GetModuleHandleW",
        source,
    })?;
    let instance = HINSTANCE(module.0);
    let icon = load_embedded_icon(ICON_SIZE)?;
    let taskbar_created_message = unsafe { RegisterWindowMessageW(TASKBAR_CREATED) };
    if taskbar_created_message == 0 {
        return Err(last_win32_error("RegisterWindowMessageW"));
    }

    let class = WNDCLASSW {
        lpfnWndProc: Some(tray_window_procedure),
        hInstance: instance,
        hIcon: icon,
        lpszClassName: CLASS_NAME,
        ..Default::default()
    };
    let atom = unsafe { RegisterClassW(&class) };
    if atom == 0 {
        return Err(last_win32_error("RegisterClassW"));
    }
    DiagnosticWindow::register_class(instance, icon)?;
    SettingsWindow::register_class(instance, icon)?;

    let state = Box::into_raw(Box::new(TrayThreadState {
        exit_requested,
        taskbar_created_message,
        icon,
        instance,
        diagnostics: DiagnosticWindow::new(diagnostics_enabled, diagnostics_receiver),
        settings: SettingsWindow::new(config_path, settings_sender, config),
        self_check_requested,
        self_check_receiver,
        error: None,
    }));
    let window = unsafe {
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS_NAME,
            WINDOW_NAME,
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            None,
            None,
            Some(instance),
            Some(state.cast_const().cast()),
        )
    };
    let window = match window {
        Ok(window) => window,
        Err(source) => {
            unsafe {
                drop(Box::from_raw(state));
            }
            return Err(AppError::Windows {
                operation: "CreateWindowExW",
                source,
            });
        }
    };
    add_tray_icon(window, icon)?;

    Ok(TrayResources {
        window,
        icon,
        instance,
        state,
    })
}

fn run_message_loop(resources: TrayResources) -> AppResult<()> {
    let mut message = MSG::default();
    loop {
        let status = unsafe { GetMessageW(&mut message, None, 0, 0) };
        if status.0 == -1 {
            return Err(last_win32_error("GetMessageW"));
        }
        if status.0 == 0 {
            break;
        }
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }

    let state = unsafe { Box::from_raw(resources.state) };
    let state_error = state.error;
    SettingsWindow::unregister_class(resources.instance)?;
    DiagnosticWindow::unregister_class(resources.instance)?;
    unsafe { UnregisterClassW(CLASS_NAME, Some(resources.instance)) }.map_err(|source| {
        AppError::Windows {
            operation: "UnregisterClassW",
            source,
        }
    })?;
    unsafe { DestroyIcon(resources.icon) }.map_err(|source| AppError::Windows {
        operation: "DestroyIcon",
        source,
    })?;
    state_error.map_or(Ok(()), Err)
}

unsafe extern "system" fn tray_window_procedure(
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

    let state_pointer = unsafe { GetWindowLongPtrW(window, GWLP_USERDATA) } as *mut TrayThreadState;
    if !state_pointer.is_null() {
        let state = unsafe { &mut *state_pointer };
        if message == state.taskbar_created_message {
            if let Err(error) = add_tray_icon(window, state.icon) {
                close_after_error(window, state, error);
            }
            return LRESULT(0);
        }

        match message {
            SELF_CHECK_REPORT_MESSAGE => {
                if let Err(error) = show_pending_self_check_report(window, state) {
                    close_after_error(window, state, error);
                }
                return LRESULT(0);
            }
            DIAGNOSTIC_EVENT_MESSAGE => {
                if let Err(error) = state.diagnostics.drain_events() {
                    close_after_error(window, state, error);
                }
                return LRESULT(0);
            }
            TRAY_CALLBACK_MESSAGE => {
                let event = lparam.0 as u32;
                if event == WM_RBUTTONUP || event == WM_CONTEXTMENU || event == WM_LBUTTONUP {
                    match show_context_menu(window, state.settings.target_mode()) {
                        Ok(TrayCommand::SelectTargetMode(mode)) => {
                            if let Err(error) = state.settings.select_target_mode(mode) {
                                close_after_error(window, state, error);
                            }
                        }
                        Ok(TrayCommand::OpenSettings) => {
                            if let Err(error) = state.settings.open(state.instance) {
                                close_after_error(window, state, error);
                            }
                        }
                        Ok(TrayCommand::OpenDiagnostics) => {
                            if let Err(error) = state.diagnostics.open(state.instance) {
                                close_after_error(window, state, error);
                            }
                        }
                        Ok(TrayCommand::StartSelfCheck) => {
                            if show_self_check_instructions(window) {
                                state.self_check_requested.store(true, Ordering::Release);
                            }
                        }
                        Ok(TrayCommand::Exit) => request_exit(window, state),
                        Ok(TrayCommand::None) => {}
                        Err(error) => close_after_error(window, state, error),
                    }
                }
                return LRESULT(0);
            }
            WM_CLOSE => {
                request_exit(window, state);
                return LRESULT(0);
            }
            WM_DESTROY => {
                delete_tray_icon(window);
                unsafe { PostQuitMessage(0) };
                return LRESULT(0);
            }
            _ => {}
        }
    }

    unsafe { DefWindowProcW(window, message, wparam, lparam) }
}

fn request_exit(window: HWND, state: &mut TrayThreadState) {
    state.exit_requested.store(true, Ordering::Release);
    if let Err(error) = state.diagnostics.close() {
        record_error(state, error);
    }
    if let Err(error) = state.settings.close() {
        record_error(state, error);
    }
    if let Err(source) = unsafe { DestroyWindow(window) } {
        record_error(
            state,
            AppError::Windows {
                operation: "DestroyWindow",
                source,
            },
        );
        unsafe { PostQuitMessage(1) };
    }
}

fn close_after_error(window: HWND, state: &mut TrayThreadState, error: AppError) {
    record_error(state, error);
    request_exit(window, state);
}

fn record_error(state: &mut TrayThreadState, error: AppError) {
    if state.error.is_none() {
        state.error = Some(error);
    } else {
        write_debug_warning(&format!("additional tray error: error={error}"));
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TrayCommand {
    None,
    SelectTargetMode(TargetMode),
    OpenSettings,
    OpenDiagnostics,
    StartSelfCheck,
    Exit,
}

fn show_context_menu(window: HWND, target_mode: Option<TargetMode>) -> AppResult<TrayCommand> {
    let menu = Menu::create()?;
    let mode_menu = Menu::create()?;
    append_checked_menu_item(
        mode_menu.handle,
        LINEAGE_MODE_MENU_ID,
        LINEAGE_MODE_LABEL,
        target_mode == Some(TargetMode::Lineage),
        "AppendMenuW Lineage mode",
    )?;
    append_checked_menu_item(
        mode_menu.handle,
        PARSEC_MODE_MENU_ID,
        PARSEC_MODE_LABEL,
        target_mode == Some(TargetMode::Parsec),
        "AppendMenuW Parsec mode",
    )?;
    let mode_handle = mode_menu.handle;
    unsafe {
        AppendMenuW(
            menu.handle,
            MF_POPUP | MF_STRING,
            mode_handle.0 as usize,
            TARGET_MODE_LABEL,
        )
    }
    .map_err(|source| AppError::Windows {
        operation: "AppendMenuW target mode",
        source,
    })?;
    mode_menu.attach();
    unsafe { AppendMenuW(menu.handle, MF_SEPARATOR, 0, PCWSTR::null()) }.map_err(|source| {
        AppError::Windows {
            operation: "AppendMenuW target separator",
            source,
        }
    })?;
    unsafe { AppendMenuW(menu.handle, MF_STRING, SETTINGS_MENU_ID, SETTINGS_LABEL) }.map_err(
        |source| AppError::Windows {
            operation: "AppendMenuW settings",
            source,
        },
    )?;
    unsafe {
        AppendMenuW(
            menu.handle,
            MF_STRING,
            DIAGNOSTICS_MENU_ID,
            DIAGNOSTICS_LABEL,
        )
    }
    .map_err(|source| AppError::Windows {
        operation: "AppendMenuW diagnostics",
        source,
    })?;
    unsafe { AppendMenuW(menu.handle, MF_STRING, SELF_CHECK_MENU_ID, SELF_CHECK_LABEL) }.map_err(
        |source| AppError::Windows {
            operation: "AppendMenuW self-check",
            source,
        },
    )?;
    unsafe { AppendMenuW(menu.handle, MF_SEPARATOR, 0, PCWSTR::null()) }.map_err(|source| {
        AppError::Windows {
            operation: "AppendMenuW separator",
            source,
        }
    })?;
    unsafe { AppendMenuW(menu.handle, MF_STRING, EXIT_MENU_ID, EXIT_LABEL) }.map_err(|source| {
        AppError::Windows {
            operation: "AppendMenuW",
            source,
        }
    })?;
    let mut cursor = POINT::default();
    unsafe { GetCursorPos(&mut cursor) }.map_err(|source| AppError::Windows {
        operation: "GetCursorPos for tray menu",
        source,
    })?;
    if !unsafe { SetForegroundWindow(window) }.as_bool() {
        return Err(last_win32_error("SetForegroundWindow"));
    }
    let selected = unsafe {
        TrackPopupMenu(
            menu.handle,
            TPM_RIGHTBUTTON | TPM_RETURNCMD,
            cursor.x,
            cursor.y,
            None,
            window,
            None,
        )
    };
    match selected.0 as usize {
        LINEAGE_MODE_MENU_ID => Ok(TrayCommand::SelectTargetMode(TargetMode::Lineage)),
        PARSEC_MODE_MENU_ID => Ok(TrayCommand::SelectTargetMode(TargetMode::Parsec)),
        SETTINGS_MENU_ID => Ok(TrayCommand::OpenSettings),
        DIAGNOSTICS_MENU_ID => Ok(TrayCommand::OpenDiagnostics),
        SELF_CHECK_MENU_ID => Ok(TrayCommand::StartSelfCheck),
        EXIT_MENU_ID => Ok(TrayCommand::Exit),
        _ => Ok(TrayCommand::None),
    }
}

fn append_checked_menu_item(
    menu: windows::Win32::UI::WindowsAndMessaging::HMENU,
    identifier: usize,
    label: PCWSTR,
    checked: bool,
    operation: &'static str,
) -> AppResult<()> {
    let state = if checked { MF_CHECKED } else { MF_UNCHECKED };
    unsafe { AppendMenuW(menu, MF_STRING | state, identifier, label) }
        .map_err(|source| AppError::Windows { operation, source })
}

fn show_self_check_instructions(parent: HWND) -> bool {
    let text = wide_string(
        "После нажатия «ОК» у вас будет 3 секунды, чтобы вернуться в выбранное целевое окно.\r\n\r\nСамопроверка не управляет мышью и ничего не нажимает.",
    );
    let title = wide_string("res-bot — самопроверка");
    (unsafe {
        MessageBoxW(
            Some(parent),
            PCWSTR(text.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OK | MB_ICONINFORMATION,
        )
    }) == IDOK
}

fn show_pending_self_check_report(parent: HWND, state: &mut TrayThreadState) -> AppResult<()> {
    let mut latest = None::<String>;
    loop {
        match state.self_check_receiver.try_recv() {
            Ok(report) => latest = Some(report),
            Err(std::sync::mpsc::TryRecvError::Empty) => break,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                return Err(AppError::SelfCheckChannel);
            }
        }
    }
    let report = latest.ok_or(AppError::SelfCheckChannel)?;
    let text = wide_string(&report);
    let title = wide_string("res-bot — результат самопроверки");
    unsafe {
        MessageBoxW(
            Some(parent),
            PCWSTR(text.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OK | MB_ICONINFORMATION,
        );
    }
    Ok(())
}

struct Menu {
    handle: windows::Win32::UI::WindowsAndMessaging::HMENU,
    attached: bool,
}

impl Menu {
    fn create() -> AppResult<Self> {
        let handle = unsafe { CreatePopupMenu() }.map_err(|source| AppError::Windows {
            operation: "CreatePopupMenu",
            source,
        })?;
        Ok(Self {
            handle,
            attached: false,
        })
    }

    fn attach(mut self) {
        self.attached = true;
    }
}

impl Drop for Menu {
    fn drop(&mut self) {
        if self.attached {
            return;
        }
        if let Err(error) = unsafe { DestroyMenu(self.handle) } {
            write_debug_warning(&format!("failed to destroy tray menu: error={error}"));
        }
    }
}

fn add_tray_icon(window: HWND, icon: HICON) -> AppResult<()> {
    let mut data = tray_icon_data(window, icon);
    copy_wide_text("res-bot — помощник воскрешения", &mut data.szTip);
    if !unsafe { Shell_NotifyIconW(NIM_ADD, &data) }.as_bool() {
        return Err(last_win32_error("Shell_NotifyIconW NIM_ADD"));
    }
    Ok(())
}

fn delete_tray_icon(window: HWND) {
    let data = tray_icon_data(window, HICON(ptr::null_mut()));
    if !unsafe { Shell_NotifyIconW(NIM_DELETE, &data) }.as_bool() {
        write_debug_warning("failed to remove tray icon");
    }
}

fn tray_icon_data(window: HWND, icon: HICON) -> NOTIFYICONDATAW {
    NOTIFYICONDATAW {
        cbSize: size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: window,
        uID: TRAY_ICON_ID,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP,
        uCallbackMessage: TRAY_CALLBACK_MESSAGE,
        hIcon: icon,
        ..Default::default()
    }
}

fn load_embedded_icon(desired_size: i32) -> AppResult<HICON> {
    let image = select_icon_image(ICON_BYTES, desired_size as u32)?;
    unsafe {
        CreateIconFromResourceEx(
            image,
            true,
            ICON_RESOURCE_VERSION,
            desired_size,
            desired_size,
            LR_DEFAULTCOLOR,
        )
    }
    .map_err(|source| AppError::Windows {
        operation: "CreateIconFromResourceEx",
        source,
    })
}

fn select_icon_image(bytes: &[u8], desired_size: u32) -> AppResult<&[u8]> {
    if bytes.len() < 6 || read_u16(bytes, 0)? != 0 || read_u16(bytes, 2)? != 1 {
        return Err(AppError::InvalidIcon {
            reason: "ICO header is missing or unsupported".to_owned(),
        });
    }
    let count = read_u16(bytes, 4)? as usize;
    let directory_end = 6 + count * 16;
    if count == 0 || directory_end > bytes.len() {
        return Err(AppError::InvalidIcon {
            reason: format!(
                "ICO directory is invalid: count={count}, bytes={}",
                bytes.len()
            ),
        });
    }

    let mut selected = None::<(u32, usize, usize)>;
    for index in 0..count {
        let entry = 6 + index * 16;
        let width = if bytes[entry] == 0 {
            256
        } else {
            bytes[entry] as u32
        };
        let length = read_u32(bytes, entry + 8)? as usize;
        let offset = read_u32(bytes, entry + 12)? as usize;
        if offset
            .checked_add(length)
            .is_none_or(|end| end > bytes.len())
        {
            return Err(AppError::InvalidIcon {
                reason: format!(
                    "ICO entry is outside the file: index={index}, offset={offset}, length={length}"
                ),
            });
        }
        let distance = width.abs_diff(desired_size);
        if selected.is_none_or(|(best_distance, _, _)| distance < best_distance) {
            selected = Some((distance, offset, length));
        }
    }
    let (_, offset, length) = selected.ok_or_else(|| AppError::InvalidIcon {
        reason: "ICO contains no image entries".to_owned(),
    })?;
    Ok(&bytes[offset..offset + length])
}

fn read_u16(bytes: &[u8], offset: usize) -> AppResult<u16> {
    let value = bytes
        .get(offset..offset + 2)
        .ok_or_else(|| AppError::InvalidIcon {
            reason: format!("unexpected end of ICO at offset={offset}"),
        })?;
    Ok(u16::from_le_bytes([value[0], value[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> AppResult<u32> {
    let value = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| AppError::InvalidIcon {
            reason: format!("unexpected end of ICO at offset={offset}"),
        })?;
    Ok(u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
}

fn copy_wide_text(source: &str, destination: &mut [u16]) {
    let encoded = source
        .encode_utf16()
        .take(destination.len().saturating_sub(1))
        .collect::<Vec<u16>>();
    destination[..encoded.len()].copy_from_slice(&encoded);
    destination[encoded.len()] = 0;
}

fn wide_string(value: &str) -> Vec<u16> {
    value.encode_utf16().chain([0]).collect()
}

fn last_win32_error(operation: &'static str) -> AppError {
    AppError::Win32 {
        operation,
        code: unsafe { GetLastError() }.0,
    }
}

fn panic_description(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        return (*message).to_owned();
    }
    if let Some(message) = payload.downcast_ref::<String>() {
        return message.clone();
    }
    "unknown panic payload".to_owned()
}

#[cfg(test)]
mod tests {
    use super::{ICON_BYTES, copy_wide_text, select_icon_image};

    #[test]
    fn embedded_icon_contains_small_tray_image() {
        let image = select_icon_image(ICON_BYTES, 16).unwrap();

        assert!(!image.is_empty());
    }

    #[test]
    fn tooltip_is_null_terminated() {
        let mut destination = [55_u16; 8];

        copy_wide_text("abcdefghijk", &mut destination);

        assert_eq!(destination[7], 0);
    }
}
