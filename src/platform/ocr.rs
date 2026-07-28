use std::thread;
use std::time::Duration;

use windows::Globalization::Language;
use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
use windows::Media::Ocr::OcrEngine;
use windows::Storage::Streams::DataWriter;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
use windows::core::HSTRING;

use crate::error::{AppError, AppResult};
use crate::image::Frame;
use crate::platform::write_debug_warning;

pub struct OcrConnector {
    engine: OcrEngine,
    _apartment: ComApartment,
}

impl OcrConnector {
    pub fn new(language_tag: &str) -> AppResult<Self> {
        let apartment = ComApartment::initialize()?;
        let language =
            Language::CreateLanguage(&HSTRING::from(language_tag)).map_err(|source| {
                AppError::Windows {
                    operation: "Language::CreateLanguage",
                    source,
                }
            })?;
        let supported =
            OcrEngine::IsLanguageSupported(&language).map_err(|source| AppError::Windows {
                operation: "OcrEngine::IsLanguageSupported",
                source,
            })?;
        if !supported {
            return Err(AppError::OcrLanguageUnavailable);
        }
        let engine =
            OcrEngine::TryCreateFromLanguage(&language).map_err(|source| AppError::Windows {
                operation: "OcrEngine::TryCreateFromLanguage",
                source,
            })?;
        Ok(Self {
            engine,
            _apartment: apartment,
        })
    }

    pub fn recognize(&self, frame: &Frame) -> AppResult<String> {
        let mut last_error = None::<AppError>;
        for attempt in 1..=3 {
            match self.recognize_once(frame) {
                Ok(text) => return Ok(text),
                Err(error) => {
                    write_debug_warning(&format!(
                        "OCR failed; attempt={attempt}, width={}, height={}, error={error}",
                        frame.width, frame.height
                    ));
                    last_error = Some(error);
                    thread::sleep(Duration::from_millis(80 * attempt));
                }
            }
        }
        Err(last_error.expect("OCR retry loop always records an error"))
    }

    fn recognize_once(&self, frame: &Frame) -> AppResult<String> {
        let writer = DataWriter::new().map_err(|source| AppError::Windows {
            operation: "DataWriter::new",
            source,
        })?;
        writer
            .WriteBytes(&frame.bgra)
            .map_err(|source| AppError::Windows {
                operation: "DataWriter::WriteBytes",
                source,
            })?;
        let buffer = writer.DetachBuffer().map_err(|source| AppError::Windows {
            operation: "DataWriter::DetachBuffer",
            source,
        })?;
        let bitmap = SoftwareBitmap::CreateCopyFromBuffer(
            &buffer,
            BitmapPixelFormat::Bgra8,
            frame.width as i32,
            frame.height as i32,
        )
        .map_err(|source| AppError::Windows {
            operation: "SoftwareBitmap::CreateCopyFromBuffer",
            source,
        })?;
        let result = self
            .engine
            .RecognizeAsync(&bitmap)
            .map_err(|source| AppError::Windows {
                operation: "OcrEngine::RecognizeAsync",
                source,
            })?
            .join()
            .map_err(|source| AppError::Windows {
                operation: "OcrEngine::RecognizeAsync completion",
                source,
            })?;
        result
            .Text()
            .map(|text| text.to_string())
            .map_err(|source| AppError::Windows {
                operation: "OcrResult::Text",
                source,
            })
    }
}

struct ComApartment;

impl ComApartment {
    fn initialize() -> AppResult<Self> {
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
            .ok()
            .map_err(|source| AppError::Windows {
                operation: "CoInitializeEx",
                source,
            })?;
        Ok(Self)
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}
