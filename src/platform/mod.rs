mod capture;
mod diagnostics;
mod input;
mod notify;
mod ocr;
mod tray;
mod tray_icon;
mod window;

pub use capture::capture_window;
pub use input::{click_human_like, stop_shortcut_pressed};
pub use notify::{show_fatal_error, write_debug_warning};
pub use ocr::OcrConnector;
pub use tray::TrayConnector;
pub use window::{GameWindow, initialize_dpi_awareness, matching_foreground_window};

