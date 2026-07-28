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
    Action, ResurrectionPercentage, choose_action, has_numbered_nickname,
    has_numbered_nickname_header, parse_percentage,
};
use crate::detection::{DialogCandidate, find_player_panel_regions, find_resurrection_dialog};
use crate::error::{AppError, AppResult};
use crate::image::{Frame, Rect};
use crate::platform::{
    ClickAttempt, ClickCancellation, ClickTiming, OcrConnector, TrayConnector, capture_window,
    click_human_like, click_human_like_and_relocate, initialize_dpi_awareness,
    matching_foreground_window, show_fatal_error, stop_shortcut_pressed, wait_for_mouse_idle,
    write_debug_warning,
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
    let mut config = Config::load(&executable)?;
    let config_path = Config::path_for_executable(&executable);
    let ocr = OcrConnector::new("ru-RU")?;
    let tray = TrayConnector::start(config.clone(), config_path)?;
    let mut next_cycle = Instant::now();
    let mut self_check_deadline = None::<Instant>;

    while !stop_shortcut_pressed() && !tray.exit_requested() {
        if let Some(updated) = tray.take_config_update()? {
            config = updated;
            next_cycle = Instant::now();
            tray.log_diagnostic(|| "Настройки применены без перезапуска.".to_owned())?;
        }
        if tray.take_self_check_requested() {
            self_check_deadline = Some(Instant::now() + Duration::from_secs(3));
        }
        if self_check_deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            let report = run_self_check(&config, &ocr, &tray);
            tray.complete_self_check(report)?;
            self_check_deadline = None;
            next_cycle = Instant::now() + Duration::from_secs(config.poll_interval_seconds);
        }
        if self_check_deadline.is_none() && Instant::now() >= next_cycle {
            run_cycle(&config, &ocr, &tray)?;
            next_cycle = Instant::now() + Duration::from_secs(config.poll_interval_seconds);
        }
        thread::sleep(Duration::from_millis(100));
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
    let Some(initial) = analyze_frame(ocr, frame, tray, 1)? else {
        return Ok(());
    };

    let mut random = rand::rng();
    let delay = random.random_range(config.pre_click_min_delay_ms..=config.pre_click_max_delay_ms);
    tray.log_diagnostic(|| format!("Пауза перед подтверждением: delay_ms={delay}."))?;
    thread::sleep(Duration::from_millis(delay));
    let Some(confirmed) = confirm_analysis(config, ocr, tray, window.handle, initial)? else {
        return Ok(());
    };

    tray.log_diagnostic(|| {
        format!(
            "Ожидание свободной мыши: required_ms={}, timeout_ms={}.",
            config.mouse_idle_required_ms, config.mouse_idle_timeout_ms
        )
    })?;
    if !wait_for_mouse_idle(config.mouse_idle_required_ms, config.mouse_idle_timeout_ms)? {
        tray.log_diagnostic(|| {
            "Клик отменён: пользователь продолжает управлять мышью.".to_owned()
        })?;
        return Ok(());
    }

    let Some((verified_frame, verified_dialog)) =
        verify_click_target(config, tray, window.handle, confirmed.dialog)?
    else {
        return Ok(());
    };
    let button = match confirmed.action {
        Action::Accept => verified_dialog.accept_button,
        Action::Reject => verified_dialog.reject_button,
    };
    let timing = ClickTiming {
        minimum_duration_ms: config.click_min_duration_ms,
        maximum_duration_ms: config.click_max_duration_ms,
    };
    let click_result = if config.relocate_cursor_after_click {
        click_human_like_and_relocate(button, verified_frame.origin, timing)
    } else {
        click_human_like(button, verified_frame.origin, timing)
    };
    let attempt = match click_result {
        Ok(attempt) => attempt,
        Err(error) if is_mouse_input_error(&error) => {
            write_debug_warning(&format!("mouse action cancelled: error={error}"));
            tray.log_diagnostic(|| {
                format!("Клик отменён: Windows не приняла ввод мыши: error={error}.")
            })?;
            return Ok(());
        }
        Err(error) => return Err(error),
    };
    match attempt {
        ClickAttempt::Clicked(click) => {
            thread::sleep(Duration::from_millis(300));
            match verify_click_result(config, window.handle, verified_dialog)? {
                ClickResultVerification::Confirmed => tray.log_diagnostic(|| {
                    format!(
                        "Клик подтверждён: action={}, screen_x={}, screen_y={}, movement_ms={}, cursor_relocated={}.",
                        action_label(confirmed.action),
                        click.target.x,
                        click.target.y,
                        click.duration_ms,
                        click.cursor_relocated
                    )
                }),
                ClickResultVerification::DialogStillPresent => tray.log_diagnostic(|| {
                    format!(
                        "Клик не подтверждён: диалог остался на экране, action={}, screen_x={}, screen_y={}, movement_ms={}, cursor_relocated={}.",
                        action_label(confirmed.action),
                        click.target.x,
                        click.target.y,
                        click.duration_ms,
                        click.cursor_relocated
                    )
                }),
                ClickResultVerification::WindowChanged => tray.log_diagnostic(|| {
                    format!(
                        "Результат клика нельзя проверить: Lineage II перестала быть активным окном, action={}, screen_x={}, screen_y={}.",
                        action_label(confirmed.action),
                        click.target.x,
                        click.target.y
                    )
                }),
            }
        }
        ClickAttempt::Cancelled(ClickCancellation::MouseMovedDuringApproach {
            expected,
            actual,
        }) => tray.log_diagnostic(|| {
            format!(
                "Клик отменён: курсор покинул собственную траекторию: expected_x={}, expected_y={}, actual_x={}, actual_y={}.",
                expected.x, expected.y, actual.x, actual.y
            )
        }),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ClickResultVerification {
    Confirmed,
    DialogStillPresent,
    WindowChanged,
}

fn verify_click_result(
    config: &Config,
    expected_window: windows::Win32::Foundation::HWND,
    expected_dialog: DialogCandidate,
) -> AppResult<ClickResultVerification> {
    let Some(window) = matching_foreground_window(&config.window_title_fragments)? else {
        return Ok(ClickResultVerification::WindowChanged);
    };
    if window.handle != expected_window {
        return Ok(ClickResultVerification::WindowChanged);
    }
    let frame = capture_window(&window)?;
    let Some(dialog) = find_resurrection_dialog(&frame) else {
        return Ok(ClickResultVerification::Confirmed);
    };
    if same_dialog(expected_dialog, dialog) {
        return Ok(ClickResultVerification::DialogStillPresent);
    }
    Ok(ClickResultVerification::Confirmed)
}

struct FrameAnalysis {
    dialog: DialogCandidate,
    percentage: ResurrectionPercentage,
    numbered_nickname: bool,
    action: Action,
}

fn analyze_frame(
    ocr: &OcrConnector,
    frame: Frame,
    tray: &TrayConnector,
    frame_index: u32,
) -> AppResult<Option<FrameAnalysis>> {
    tray.log_diagnostic(|| {
        format!(
            "Получен кадр подтверждения: index={}, width={}, height={}.",
            frame_index, frame.width, frame.height
        )
    })?;
    let Some(dialog) = find_resurrection_dialog(&frame) else {
        tray.log_diagnostic(|| {
            format!(
                "Пропуск: на кадре {} пара зелёной и красной кнопок не найдена.",
                frame_index
            )
        })?;
        return Ok(None);
    };
    tray.log_diagnostic(|| {
        format!(
            "Найден диалог: index={}, bounds={}, accept={}, reject={}.",
            frame_index,
            format_rect(dialog.bounds),
            format_rect(dialog.accept_button),
            format_rect(dialog.reject_button)
        )
    })?;

    let dialog_text = recognize_dialog_region(ocr, &frame, dialog.bounds, 3)?;
    tray.log_diagnostic(|| {
        format!(
            "OCR диалога: index={}, text={:?}.",
            frame_index,
            compact_diagnostic_text(&dialog_text, 240)
        )
    })?;
    let percentage = match parse_percentage(&dialog_text) {
        Ok(Some(percentage)) => percentage,
        Ok(None) => {
            tray.log_diagnostic(|| {
                format!(
                    "Пропуск: на кадре {} не найден поддерживаемый процент 0% или 100%.",
                    frame_index
                )
            })?;
            return Ok(None);
        }
        Err(error @ AppError::ConflictingPercentage { .. }) => {
            write_debug_warning(&format!("{error}"));
            tray.log_diagnostic(|| {
                format!(
                    "Пропуск: на кадре {} OCR одновременно обнаружил 0% и 100%: error={error}.",
                    frame_index
                )
            })?;
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    let Some(numbered_nickname) = recognize_player_suffix(ocr, &frame, tray)? else {
        tray.log_diagnostic(|| {
            format!(
                "Пропуск: на кадре {} панель персонажа или похожий на ник текст не распознаны.",
                frame_index
            )
        })?;
        return Ok(None);
    };
    let action = choose_action(percentage, numbered_nickname);
    tray.log_diagnostic(|| {
        format!(
            "Решение: index={}, percentage={}, numbered_suffix={}, action={}.",
            frame_index,
            resurrection_percentage_label(percentage),
            numbered_nickname,
            action_label(action)
        )
    })?;
    Ok(Some(FrameAnalysis {
        dialog,
        percentage,
        numbered_nickname,
        action,
    }))
}

fn confirm_analysis(
    config: &Config,
    ocr: &OcrConnector,
    tray: &TrayConnector,
    expected_window: windows::Win32::Foundation::HWND,
    initial: FrameAnalysis,
) -> AppResult<Option<FrameAnalysis>> {
    let mut confirmed = initial;
    for frame_index in 2..=config.confirmation_frame_count {
        thread::sleep(Duration::from_millis(config.confirmation_interval_ms));
        let Some(window) = matching_foreground_window(&config.window_title_fragments)? else {
            tray.log_diagnostic(|| {
                "Подтверждение отменено: Lineage II перестала быть активным окном.".to_owned()
            })?;
            return Ok(None);
        };
        if window.handle != expected_window {
            tray.log_diagnostic(|| {
                "Подтверждение отменено: активным стало другое окно Lineage II.".to_owned()
            })?;
            return Ok(None);
        }
        let frame = capture_window(&window)?;
        let Some(candidate) = analyze_frame(ocr, frame, tray, frame_index)? else {
            return Ok(None);
        };
        if !same_dialog(confirmed.dialog, candidate.dialog)
            || !same_decision_evidence(&confirmed, &candidate)
        {
            tray.log_diagnostic(|| {
                format!(
                    "Подтверждение отменено: результаты кадров {} и {} различаются.",
                    frame_index - 1,
                    frame_index
                )
            })?;
            return Ok(None);
        }
        confirmed = candidate;
    }
    tray.log_diagnostic(|| {
        format!(
            "Решение подтверждено на {} последовательных кадрах.",
            config.confirmation_frame_count
        )
    })?;
    Ok(Some(confirmed))
}

fn verify_click_target(
    config: &Config,
    tray: &TrayConnector,
    expected_window: windows::Win32::Foundation::HWND,
    expected_dialog: DialogCandidate,
) -> AppResult<Option<(Frame, DialogCandidate)>> {
    let Some(verified_window) = matching_foreground_window(&config.window_title_fragments)? else {
        tray.log_diagnostic(|| {
            "Клик отменён: Lineage II перестала быть активным окном.".to_owned()
        })?;
        return Ok(None);
    };
    if verified_window.handle != expected_window {
        tray.log_diagnostic(|| "Клик отменён: активным стало другое окно Lineage II.".to_owned())?;
        return Ok(None);
    }
    let verified_frame = capture_window(&verified_window)?;
    let Some(verified_dialog) = find_resurrection_dialog(&verified_frame) else {
        tray.log_diagnostic(|| "Клик отменён: при повторной проверке диалог исчез.".to_owned())?;
        return Ok(None);
    };
    if !same_dialog(expected_dialog, verified_dialog) {
        tray.log_diagnostic(|| {
            format!(
                "Клик отменён: положение диалога изменилось, previous={}, current={}.",
                format_rect(expected_dialog.bounds),
                format_rect(verified_dialog.bounds)
            )
        })?;
        return Ok(None);
    }
    Ok(Some((verified_frame, verified_dialog)))
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
        let header = player_nickname_header(region);
        let header_text = recognize_original_region(ocr, frame, header, 5)?;
        tray.log_diagnostic(|| {
            format!(
                "OCR заголовка панели: region={}, text={:?}.",
                format_rect(header),
                compact_diagnostic_text(&header_text, 120)
            )
        })?;
        if has_numbered_nickname_header(&header_text) {
            tray.log_diagnostic(|| "В заголовке панели найден суффикс _NN.".to_owned())?;
            return Ok(Some(true));
        }
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

fn player_nickname_header(panel: Rect) -> Rect {
    Rect {
        height: panel.height.min(34),
        ..panel
    }
}

fn dialog_percentage_region(dialog: Rect) -> Rect {
    Rect {
        x: dialog.x + 20,
        y: dialog.y + 55,
        width: dialog.width.saturating_sub(40),
        height: dialog.height.min(58),
    }
}

fn recognize_dialog_region(
    ocr: &OcrConnector,
    frame: &Frame,
    region: crate::image::Rect,
    scale_factor: u32,
) -> AppResult<String> {
    let primary = recognize_high_contrast_region(ocr, frame, region, scale_factor)?;
    if parse_percentage(&primary)?.is_some() {
        return Ok(primary);
    }
    let focused_region = dialog_percentage_region(region);
    let focused = recognize_high_contrast_region(ocr, frame, focused_region, 8)?;
    Ok(format!("{primary}\n{focused}"))
}

fn recognize_high_contrast_region(
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

fn same_decision_evidence(left: &FrameAnalysis, right: &FrameAnalysis) -> bool {
    left.percentage == right.percentage
        && left.numbered_nickname == right.numbered_nickname
        && left.action == right.action
}

fn is_mouse_input_error(error: &AppError) -> bool {
    match error {
        AppError::PartialInput { .. } => true,
        AppError::Win32 { operation, .. } => operation.starts_with("SendInput"),
        _ => false,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SelfCheckStatus {
    Passed,
    Warning,
    Failed,
}

struct SelfCheckItem {
    status: SelfCheckStatus,
    name: &'static str,
    details: String,
}

fn run_self_check(config: &Config, ocr: &OcrConnector, tray: &TrayConnector) -> String {
    let mut items = vec![
        SelfCheckItem {
            status: SelfCheckStatus::Passed,
            name: "Русский OCR",
            details: "компонент доступен и OCR-движок создан".to_owned(),
        },
        match config.validate() {
            Ok(()) => SelfCheckItem {
                status: SelfCheckStatus::Passed,
                name: "Конфигурация",
                details: "все значения прошли проверку".to_owned(),
            },
            Err(error) => SelfCheckItem {
                status: SelfCheckStatus::Failed,
                name: "Конфигурация",
                details: error.to_string(),
            },
        },
    ];

    let window = match matching_foreground_window(&config.window_title_fragments) {
        Ok(Some(window)) => {
            items.push(SelfCheckItem {
                status: SelfCheckStatus::Passed,
                name: "Активное окно",
                details: format!(
                    "найдено {:?}, {}",
                    window.title,
                    format_rect(window.screen_bounds)
                ),
            });
            window
        }
        Ok(None) => {
            items.push(SelfCheckItem {
                status: SelfCheckStatus::Failed,
                name: "Активное окно",
                details: "Lineage II не активна или заголовок не совпал".to_owned(),
            });
            return format_self_check_report(&items);
        }
        Err(error) => {
            items.push(SelfCheckItem {
                status: SelfCheckStatus::Failed,
                name: "Активное окно",
                details: error.to_string(),
            });
            return format_self_check_report(&items);
        }
    };

    let frame = match capture_window(&window) {
        Ok(frame) => {
            items.push(SelfCheckItem {
                status: SelfCheckStatus::Passed,
                name: "Захват экрана",
                details: format!("получен кадр {}×{}", frame.width, frame.height),
            });
            frame
        }
        Err(error) => {
            items.push(SelfCheckItem {
                status: SelfCheckStatus::Failed,
                name: "Захват экрана",
                details: error.to_string(),
            });
            return format_self_check_report(&items);
        }
    };

    if let Some(dialog) = find_resurrection_dialog(&frame) {
        match recognize_dialog_region(ocr, &frame, dialog.bounds, 3)
            .and_then(|text| parse_percentage(&text).map(|percentage| (text, percentage)))
        {
            Ok((text, Some(percentage))) => items.push(SelfCheckItem {
                status: SelfCheckStatus::Passed,
                name: "Диалог воскрешения",
                details: format!(
                    "распознано {}, OCR={:?}",
                    resurrection_percentage_label(percentage),
                    compact_diagnostic_text(&text, 120)
                ),
            }),
            Ok((text, None)) => items.push(SelfCheckItem {
                status: SelfCheckStatus::Warning,
                name: "Диалог воскрешения",
                details: format!(
                    "диалог найден, но процент не распознан, OCR={:?}",
                    compact_diagnostic_text(&text, 120)
                ),
            }),
            Err(error) => items.push(SelfCheckItem {
                status: SelfCheckStatus::Failed,
                name: "Диалог воскрешения",
                details: error.to_string(),
            }),
        }
    } else {
        items.push(SelfCheckItem {
            status: SelfCheckStatus::Warning,
            name: "Диалог воскрешения",
            details: "не найден; для полной проверки откройте диалог".to_owned(),
        });
    }

    match recognize_player_suffix(ocr, &frame, tray) {
        Ok(Some(numbered)) => items.push(SelfCheckItem {
            status: SelfCheckStatus::Passed,
            name: "Панель персонажа",
            details: format!("панель распознана, суффикс _NN={numbered}"),
        }),
        Ok(None) => items.push(SelfCheckItem {
            status: SelfCheckStatus::Warning,
            name: "Панель персонажа",
            details: "панель или похожий на ник текст не распознаны".to_owned(),
        }),
        Err(error) => items.push(SelfCheckItem {
            status: SelfCheckStatus::Failed,
            name: "Панель персонажа",
            details: error.to_string(),
        }),
    }

    format_self_check_report(&items)
}

fn format_self_check_report(items: &[SelfCheckItem]) -> String {
    let failed = items
        .iter()
        .filter(|item| item.status == SelfCheckStatus::Failed)
        .count();
    let warnings = items
        .iter()
        .filter(|item| item.status == SelfCheckStatus::Warning)
        .count();
    let summary = if failed == 0 {
        "Самопроверка завершена: критических ошибок нет."
    } else {
        "Самопроверка завершена: обнаружены ошибки."
    };
    let details = items
        .iter()
        .map(|item| {
            format!(
                "{} {}: {}",
                self_check_status_label(item.status),
                item.name,
                item.details
            )
        })
        .collect::<Vec<String>>()
        .join("\r\n");
    format!("{summary}\r\nПредупреждений: {warnings}; ошибок: {failed}.\r\n\r\n{details}")
}

fn self_check_status_label(status: SelfCheckStatus) -> &'static str {
    match status {
        SelfCheckStatus::Passed => "[OK]",
        SelfCheckStatus::Warning => "[!]",
        SelfCheckStatus::Failed => "[X]",
    }
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::fs;

    use crate::decision::{Action, ResurrectionPercentage};
    use crate::detection::{DialogCandidate, find_player_panel_regions, find_resurrection_dialog};
    use crate::image::{Frame, Point, Rect};
    use crate::platform::{OcrConnector, TrayConnector};

    use super::{
        FrameAnalysis, SelfCheckItem, SelfCheckStatus, compact_diagnostic_text,
        contains_name_like_token, format_self_check_report, parse_percentage,
        recognize_dialog_region, recognize_original_region, recognize_player_suffix,
        same_decision_evidence,
    };

    #[test]
    fn player_panel_requires_name_like_text() {
        assert!(contains_name_like_token("113 player"));
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
    fn confirmation_requires_identical_decision_evidence() {
        let first = analysis(ResurrectionPercentage::Zero, true, Action::Accept);
        let same = analysis(ResurrectionPercentage::Zero, true, Action::Accept);
        let changed = analysis(ResurrectionPercentage::Hundred, false, Action::Accept);

        assert!(same_decision_evidence(&first, &same));
        assert!(!same_decision_evidence(&first, &changed));
    }

    #[test]
    fn self_check_report_summarizes_failures_and_warnings() {
        let report = format_self_check_report(&[
            SelfCheckItem {
                status: SelfCheckStatus::Passed,
                name: "OCR",
                details: "готов".to_owned(),
            },
            SelfCheckItem {
                status: SelfCheckStatus::Warning,
                name: "Диалог",
                details: "не найден".to_owned(),
            },
            SelfCheckItem {
                status: SelfCheckStatus::Failed,
                name: "Окно",
                details: "не активно".to_owned(),
            },
        ]);

        assert!(report.contains("Предупреждений: 1; ошибок: 1"));
        assert!(report.contains("[X] Окно: не активно"));
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

    fn analysis(
        percentage: ResurrectionPercentage,
        numbered_nickname: bool,
        action: Action,
    ) -> FrameAnalysis {
        let button = Rect {
            x: 10,
            y: 20,
            width: 100,
            height: 30,
        };
        FrameAnalysis {
            dialog: DialogCandidate {
                bounds: Rect {
                    x: 0,
                    y: 0,
                    width: 300,
                    height: 200,
                },
                accept_button: button,
                reject_button: Rect { x: 120, ..button },
            },
            percentage,
            numbered_nickname,
            action,
        }
    }
}
