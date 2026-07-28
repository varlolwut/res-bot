#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

#[cfg(not(target_os = "windows"))]
compile_error!("res-bot supports Windows only");

mod config;
mod decision;
mod detection;
mod error;
mod image;
mod platform;

use std::env;
use std::thread;
use std::time::{Duration, Instant};

use rand::Rng;

use crate::config::Config;
use crate::decision::{
    Action, ResurrectionPercentage, choose_action, has_numbered_nickname, parse_percentage,
};
use crate::detection::{DialogCandidate, find_player_panel_regions, find_resurrection_dialog};
use crate::error::{AppError, AppResult};
use crate::image::{Frame, Rect};
use crate::platform::{
    OcrConnector, TrayConnector, capture_window, click_human_like, initialize_dpi_awareness,
    matching_foreground_window, show_fatal_error, stop_shortcut_pressed, write_debug_warning,
};

fn main() {
    if let Err(error) = run() {
        show_fatal_error(&format!("{error}"));
    }
}

fn run() -> AppResult<()> {
    initialize_dpi_awareness()?;
    let executable = env::current_exe().map_err(|source| crate::error::AppError::ConfigRead {
        path: "<current executable>".to_owned(),
        source,
    })?;
    let config = Config::load(&executable)?;
    let ocr = OcrConnector::new("ru-RU")?;
    let tray = TrayConnector::start()?;

    while !stop_shortcut_pressed() && !tray.exit_requested() {
        run_cycle(&config, &ocr, &tray)?;
        wait_for_next_cycle(config.poll_interval_seconds, &tray);
    }
    tray.shutdown()
}

fn run_cycle(config: &Config, ocr: &OcrConnector, tray: &TrayConnector) -> AppResult<()> {
    tray.log_diagnostic(|| "Начат цикл проверки.".to_owned())?;
    let Some(window) = matching_foreground_window(&config.window_title_fragments)? else {
        tray.log_diagnostic(|| {
            "Пропуск: активное окно не соответствует заголовку Lineage II.".to_owned()
        })?;
        return Ok(());
    };
    tray.log_diagnostic(|| {
        format!(
            "Найдено окно: title={:?}, client={}.",
            window.title,
            format_rect(window.screen_bounds)
        )
    })?;
    let frame = capture_window(&window)?;
    tray.log_diagnostic(|| {
        format!(
            "Получен кадр: width={}, height={}.",
            frame.width, frame.height
        )
    })?;
    let Some(dialog) = find_resurrection_dialog(&frame) else {
        tray.log_diagnostic(|| "Пропуск: пара зелёной и красной кнопок не найдена.".to_owned())?;
        return Ok(());
    };
    tray.log_diagnostic(|| {
        format!(
            "Найден диалог: bounds={}, accept={}, reject={}.",
            format_rect(dialog.bounds),
            format_rect(dialog.accept_button),
            format_rect(dialog.reject_button)
        )
    })?;

    let dialog_text = recognize_dialog_region(ocr, &frame, dialog.bounds, 3)?;
    tray.log_diagnostic(|| {
        format!(
            "OCR диалога: text={:?}.",
            compact_diagnostic_text(&dialog_text, 240)
        )
    })?;
    let percentage = match parse_percentage(&dialog_text) {
        Ok(Some(percentage)) => percentage,
        Ok(None) => {
            tray.log_diagnostic(|| {
                "Пропуск: в OCR-тексте не найден поддерживаемый процент 0% или 100%.".to_owned()
            })?;
            return Ok(());
        }
        Err(error @ AppError::ConflictingPercentage { .. }) => {
            write_debug_warning(&format!("{error}"));
            tray.log_diagnostic(|| {
                format!("Пропуск: OCR одновременно обнаружил 0% и 100%: error={error}.")
            })?;
            return Ok(());
        }
        Err(error) => return Err(error),
    };
    tray.log_diagnostic(|| {
        format!(
            "Распознан процент: {}.",
            resurrection_percentage_label(percentage)
        )
    })?;
    let Some(numbered_nickname) = recognize_player_suffix(ocr, &frame, tray)? else {
        tray.log_diagnostic(|| {
            "Пропуск: панель персонажа или похожий на ник текст не распознаны.".to_owned()
        })?;
        return Ok(());
    };
    let action = choose_action(percentage, numbered_nickname);
    tray.log_diagnostic(|| {
        format!(
            "Решение: numbered_suffix={}, action={}.",
            numbered_nickname,
            action_label(action)
        )
    })?;

    let mut random = rand::rng();
    let delay = random.random_range(config.pre_click_min_delay_ms..=config.pre_click_max_delay_ms);
    tray.log_diagnostic(|| format!("Пауза перед повторной проверкой: delay_ms={delay}."))?;
    thread::sleep(Duration::from_millis(delay));
    let Some(verified_window) = matching_foreground_window(&config.window_title_fragments)? else {
        tray.log_diagnostic(|| {
            "Клик отменён: Lineage II перестала быть активным окном.".to_owned()
        })?;
        return Ok(());
    };
    if verified_window.handle != window.handle {
        tray.log_diagnostic(|| "Клик отменён: активным стало другое окно Lineage II.".to_owned())?;
        return Ok(());
    }
    let verified_frame = capture_window(&verified_window)?;
    let Some(verified_dialog) = find_resurrection_dialog(&verified_frame) else {
        tray.log_diagnostic(|| "Клик отменён: при повторной проверке диалог исчез.".to_owned())?;
        return Ok(());
    };
    if !same_dialog(dialog, verified_dialog) {
        tray.log_diagnostic(|| {
            format!(
                "Клик отменён: положение диалога изменилось, previous={}, current={}.",
                format_rect(dialog.bounds),
                format_rect(verified_dialog.bounds)
            )
        })?;
        return Ok(());
    }

    let button = match action {
        Action::Accept => verified_dialog.accept_button,
        Action::Reject => verified_dialog.reject_button,
    };
    let click = click_human_like(
        button,
        verified_frame.origin,
        config.click_min_duration_ms,
        config.click_max_duration_ms,
    )?;
    tray.log_diagnostic(|| {
        format!(
            "Клик выполнен: action={}, screen_x={}, screen_y={}, movement_ms={}.",
            action_label(action),
            click.target.x,
            click.target.y,
            click.duration_ms
        )
    })
}

fn recognize_player_suffix(
    ocr: &OcrConnector,
    frame: &Frame,
    tray: &TrayConnector,
) -> AppResult<Option<bool>> {
    let regions = find_player_panel_regions(frame);
    tray.log_diagnostic(|| format!("Кандидаты панели персонажа: count={}.", regions.len()))?;
    let mut recognized_panel = false;
    for region in regions {
        let text = recognize_original_region(ocr, frame, region, 3)?;
        tray.log_diagnostic(|| {
            format!(
                "OCR панели: region={}, text={:?}.",
                format_rect(region),
                compact_diagnostic_text(&text, 160)
            )
        })?;
        if has_numbered_nickname(&text) {
            tray.log_diagnostic(|| "В OCR-тексте панели найден суффикс _NN.".to_owned())?;
            return Ok(Some(true));
        }
        if contains_name_like_token(&text) {
            recognized_panel = true;
        }
    }
    Ok(recognized_panel.then_some(false))
}

fn recognize_dialog_region(
    ocr: &OcrConnector,
    frame: &Frame,
    region: crate::image::Rect,
    scale_factor: u32,
) -> AppResult<String> {
    let prepared = frame
        .crop(region)?
        .high_contrast_text()?
        .scale_nearest(scale_factor)?;
    ocr.recognize(&prepared)
}

fn recognize_original_region(
    ocr: &OcrConnector,
    frame: &Frame,
    region: crate::image::Rect,
    scale_factor: u32,
) -> AppResult<String> {
    let prepared = frame.crop(region)?.scale_nearest(scale_factor)?;
    ocr.recognize(&prepared)
}

fn contains_name_like_token(text: &str) -> bool {
    text.split(|character: char| !character.is_alphabetic())
        .any(|token| token.chars().count() >= 3)
}

fn compact_diagnostic_text(text: &str, maximum_characters: usize) -> String {
    let compact = text.split_whitespace().collect::<Vec<&str>>().join(" ");
    let characters = compact.chars().collect::<Vec<char>>();
    if characters.len() <= maximum_characters {
        return compact;
    }
    characters
        .into_iter()
        .take(maximum_characters)
        .chain(['…'])
        .collect()
}

fn format_rect(rect: Rect) -> String {
    format!(
        "x={},y={},width={},height={}",
        rect.x, rect.y, rect.width, rect.height
    )
}

fn resurrection_percentage_label(percentage: ResurrectionPercentage) -> &'static str {
    match percentage {
        ResurrectionPercentage::Zero => "0%",
        ResurrectionPercentage::Hundred => "100%",
    }
}

fn action_label(action: Action) -> &'static str {
    match action {
        Action::Accept => "accept",
        Action::Reject => "reject",
    }
}

fn same_dialog(left: DialogCandidate, right: DialogCandidate) -> bool {
    let left_center = left.bounds.center();
    let right_center = right.bounds.center();
    (left_center.x - right_center.x).unsigned_abs() <= 12
        && (left_center.y - right_center.y).unsigned_abs() <= 12
        && left.bounds.width.abs_diff(right.bounds.width) <= 20
        && left.bounds.height.abs_diff(right.bounds.height) <= 20
}

fn wait_for_next_cycle(seconds: u64, tray: &TrayConnector) {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while Instant::now() < deadline && !stop_shortcut_pressed() && !tray.exit_requested() {
        thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::fs;

    use crate::decision::ResurrectionPercentage;
    use crate::detection::{find_player_panel_regions, find_resurrection_dialog};
    use crate::image::{Frame, Point};
    use crate::platform::{OcrConnector, TrayConnector};

    use super::{
        compact_diagnostic_text, contains_name_like_token, parse_percentage,
        recognize_dialog_region, recognize_original_region, recognize_player_suffix,
    };

    #[test]
    fn player_panel_requires_name_like_text() {
        assert!(contains_name_like_token("113 lonedy"));
        assert!(contains_name_like_token("Персонаж_42"));
        assert!(!contains_name_like_token("113 HP MP"));
    }

    #[test]
    fn diagnostic_text_is_compact_and_bounded() {
        assert_eq!(
            compact_diagnostic_text("  first\r\n second   third  ", 12),
            "first second…"
        );
    }

    #[test]
    #[ignore = "set RES_BOT_RAW_FRAME, RES_BOT_FRAME_WIDTH and RES_BOT_FRAME_HEIGHT"]
    fn recognizes_external_zero_percent_reference_frame() {
        let frame = external_reference_frame();
        let dialog = find_resurrection_dialog(&frame).unwrap();
        let ocr = OcrConnector::new("ru-RU").unwrap();

        let dialog_text = recognize_dialog_region(&ocr, &frame, dialog.bounds, 3).unwrap();
        let percentage = parse_percentage(&dialog_text).unwrap();
        for panel in find_player_panel_regions(&frame) {
            let panel_text = recognize_original_region(&ocr, &frame, panel, 3).unwrap();
            println!("panel={panel:?}, panel_text={panel_text:?}");
        }
        let tray = TrayConnector::disabled_for_test();
        let suffix = recognize_player_suffix(&ocr, &frame, &tray).unwrap();
        println!("dialog_text={dialog_text:?}, percentage={percentage:?}, suffix={suffix:?}");

        assert_eq!(percentage, Some(ResurrectionPercentage::Zero));
        assert_eq!(suffix, Some(false));
    }

    #[test]
    #[ignore = "set RES_BOT_RAW_FRAME, RES_BOT_FRAME_WIDTH and RES_BOT_FRAME_HEIGHT"]
    fn recognizes_external_hundred_percent_reference_frame() {
        let frame = external_reference_frame();
        let dialog = find_resurrection_dialog(&frame).unwrap();
        let ocr = OcrConnector::new("ru-RU").unwrap();

        let dialog_text = recognize_dialog_region(&ocr, &frame, dialog.bounds, 3).unwrap();
        let percentage = parse_percentage(&dialog_text).unwrap();
        println!("dialog_text={dialog_text:?}, percentage={percentage:?}");

        assert_eq!(percentage, Some(ResurrectionPercentage::Hundred));
    }

    #[test]
    #[ignore = "set RES_BOT_RAW_FRAME, RES_BOT_FRAME_WIDTH and RES_BOT_FRAME_HEIGHT"]
    fn recognizes_external_numbered_player_panel() {
        let frame = external_reference_frame();
        let ocr = OcrConnector::new("ru-RU").unwrap();

        let tray = TrayConnector::disabled_for_test();
        let suffix = recognize_player_suffix(&ocr, &frame, &tray).unwrap();

        assert_eq!(suffix, Some(true));
    }

    fn external_reference_frame() -> Frame {
        let path = env::var("RES_BOT_RAW_FRAME").unwrap();
        let width = env::var("RES_BOT_FRAME_WIDTH")
            .unwrap()
            .parse::<u32>()
            .unwrap();
        let height = env::var("RES_BOT_FRAME_HEIGHT")
            .unwrap()
            .parse::<u32>()
            .unwrap();
        let pixels = fs::read(path).unwrap();
        Frame::new(Point { x: 0, y: 0 }, width, height, pixels).unwrap()
    }
}

