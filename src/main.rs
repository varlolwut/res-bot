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
use crate::decision::{Action, choose_action, has_numbered_nickname, parse_percentage};
use crate::detection::{DialogCandidate, find_player_panel_regions, find_resurrection_dialog};
use crate::error::AppResult;
use crate::image::Frame;
use crate::platform::{
    OcrConnector, capture_window, click_human_like, initialize_dpi_awareness,
    matching_foreground_window, show_fatal_error, stop_shortcut_pressed,
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

    while !stop_shortcut_pressed() {
        run_cycle(&config, &ocr)?;
        wait_for_next_cycle(config.poll_interval_seconds);
    }
    Ok(())
}

fn run_cycle(config: &Config, ocr: &OcrConnector) -> AppResult<()> {
    let Some(window) = matching_foreground_window(&config.window_title_fragments)? else {
        return Ok(());
    };
    let frame = capture_window(&window)?;
    let Some(dialog) = find_resurrection_dialog(&frame) else {
        return Ok(());
    };

    let dialog_text = recognize_dialog_region(ocr, &frame, dialog.bounds, 3)?;
    let Some(percentage) = parse_percentage(&dialog_text)? else {
        return Ok(());
    };
    let Some(numbered_nickname) = recognize_player_suffix(ocr, &frame)? else {
        return Ok(());
    };
    let action = choose_action(percentage, numbered_nickname);

    let mut random = rand::rng();
    let delay = random.random_range(config.pre_click_min_delay_ms..=config.pre_click_max_delay_ms);
    thread::sleep(Duration::from_millis(delay));
    let Some(verified_window) = matching_foreground_window(&config.window_title_fragments)? else {
        return Ok(());
    };
    if verified_window.handle != window.handle {
        return Ok(());
    }
    let verified_frame = capture_window(&verified_window)?;
    let Some(verified_dialog) = find_resurrection_dialog(&verified_frame) else {
        return Ok(());
    };
    if !same_dialog(dialog, verified_dialog) {
        return Ok(());
    }

    let button = match action {
        Action::Accept => verified_dialog.accept_button,
        Action::Reject => verified_dialog.reject_button,
    };
    click_human_like(
        button,
        verified_frame.origin,
        config.click_min_duration_ms,
        config.click_max_duration_ms,
    )
}

fn recognize_player_suffix(ocr: &OcrConnector, frame: &Frame) -> AppResult<Option<bool>> {
    let regions = find_player_panel_regions(frame);
    let mut recognized_panel = false;
    for region in regions {
        let text = recognize_original_region(ocr, frame, region, 3)?;
        if has_numbered_nickname(&text) {
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

fn same_dialog(left: DialogCandidate, right: DialogCandidate) -> bool {
    let left_center = left.bounds.center();
    let right_center = right.bounds.center();
    (left_center.x - right_center.x).unsigned_abs() <= 12
        && (left_center.y - right_center.y).unsigned_abs() <= 12
        && left.bounds.width.abs_diff(right.bounds.width) <= 20
        && left.bounds.height.abs_diff(right.bounds.height) <= 20
}

fn wait_for_next_cycle(seconds: u64) {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while Instant::now() < deadline && !stop_shortcut_pressed() {
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
    use crate::platform::OcrConnector;

    use super::{
        contains_name_like_token, parse_percentage, recognize_dialog_region,
        recognize_original_region, recognize_player_suffix,
    };

    #[test]
    fn player_panel_requires_name_like_text() {
        assert!(contains_name_like_token("113 lonedy"));
        assert!(contains_name_like_token("Персонаж_42"));
        assert!(!contains_name_like_token("113 HP MP"));
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
        let suffix = recognize_player_suffix(&ocr, &frame).unwrap();
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

        let suffix = recognize_player_suffix(&ocr, &frame).unwrap();

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
