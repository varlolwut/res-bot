use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("configuration file could not be read: path={path}, reason={source}")]
    ConfigRead {
        path: String,
        source: std::io::Error,
    },

    #[error("configuration file is invalid: path={path}, reason={source}")]
    ConfigParse {
        path: String,
        source: toml::de::Error,
    },

    #[error("configuration value is invalid: field={field}, value={value}, reason={reason}")]
    ConfigValue {
        field: &'static str,
        value: String,
        reason: &'static str,
    },

    #[error("Windows API call failed: operation={operation}, reason={source}")]
    Windows {
        operation: &'static str,
        source: windows::core::Error,
    },

    #[error("Windows API call failed: operation={operation}, code={code}")]
    Win32 { operation: &'static str, code: u32 },

    #[error("captured image is invalid: width={width}, height={height}, bytes={bytes}")]
    InvalidImage {
        width: u32,
        height: u32,
        bytes: usize,
    },

    #[error(
        "image rectangle is outside the captured frame: x={x}, y={y}, width={width}, height={height}"
    )]
    InvalidCrop {
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    },

    #[error(
        "Russian Windows OCR is unavailable; install the Russian language OCR component in Windows Settings"
    )]
    OcrLanguageUnavailable,

    #[error("OCR returned conflicting resurrection percentages: text={text}")]
    ConflictingPercentage { text: String },

    #[error("mouse input was only partially sent: requested={requested}, sent={sent}")]
    PartialInput { requested: u32, sent: u32 },
}

pub type AppResult<T> = Result<T, AppError>;
