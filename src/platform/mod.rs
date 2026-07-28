mod capture;
mod diagnostics;
mod input;
mod notify;
mod ocr;
mod settings;
mod tray;
mod tray_icon;
mod window;

pub use capture::capture_window;
pub use input::{
    ClickAttempt, ClickCancellation, ClickTiming, click_human_like, click_human_like_and_relocate,
    stop_shortcut_pressed, wait_for_mouse_idle,
};
pub use notify::{show_fatal_error, write_debug_warning};
pub use ocr::OcrConnector;
pub use tray::TrayConnector;
pub use window::{GameWindow, initialize_dpi_awareness, matching_foreground_window};
