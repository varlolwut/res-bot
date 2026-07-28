use std::thread;
use std::time::Duration;

use rand::Rng;
use windows::Win32::Foundation::{GetLastError, POINT};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSEINPUT, SendInput, VK_CONTROL, VK_F12, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{GetCursorPos, SetCursorPos};

use crate::error::{AppError, AppResult};
use crate::image::{Point, Rect};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClickOutcome {
    pub target: Point,
    pub duration_ms: u64,
}

pub fn stop_shortcut_pressed() -> bool {
    key_pressed(VK_CONTROL.0) && key_pressed(VK_SHIFT.0) && key_pressed(VK_F12.0)
}

pub fn click_human_like(
    button: Rect,
    frame_origin: Point,
    minimum_duration_ms: u64,
    maximum_duration_ms: u64,
) -> AppResult<ClickOutcome> {
    let mut random = rand::rng();
    let target = random_point_in_button(button, frame_origin, &mut random);
    let start = cursor_position()?;
    let duration_ms = random.random_range(minimum_duration_ms..=maximum_duration_ms);
    move_cursor_bezier(start, target, duration_ms, &mut random)?;
    send_left_click()?;
    Ok(ClickOutcome {
        target,
        duration_ms,
    })
}

fn key_pressed(virtual_key: u16) -> bool {
    (unsafe { GetAsyncKeyState(virtual_key as i32) }) < 0
}

fn random_point_in_button(button: Rect, frame_origin: Point, random: &mut impl Rng) -> Point {
    let horizontal_margin = (button.width / 7).max(2);
    let vertical_margin = (button.height / 5).max(2);
    Point {
        x: frame_origin.x
            + random.random_range(
                button.x + horizontal_margin as i32..button.right() - horizontal_margin as i32,
            ),
        y: frame_origin.y
            + random.random_range(
                button.y + vertical_margin as i32..button.bottom() - vertical_margin as i32,
            ),
    }
}

fn cursor_position() -> AppResult<Point> {
    let mut point = POINT::default();
    unsafe { GetCursorPos(&mut point) }.map_err(|source| AppError::Windows {
        operation: "GetCursorPos",
        source,
    })?;
    Ok(Point {
        x: point.x,
        y: point.y,
    })
}

fn move_cursor_bezier(
    start: Point,
    target: Point,
    duration_ms: u64,
    random: &mut impl Rng,
) -> AppResult<()> {
    let distance_x = target.x - start.x;
    let distance_y = target.y - start.y;
    let perpendicular_x = -distance_y;
    let perpendicular_y = distance_x;
    let curve = random.random_range(-0.12_f64..=0.12_f64);
    let control_one = Point {
        x: start.x + distance_x / 3 + (perpendicular_x as f64 * curve) as i32,
        y: start.y + distance_y / 3 + (perpendicular_y as f64 * curve) as i32,
    };
    let control_two = Point {
        x: start.x + distance_x * 2 / 3 - (perpendicular_x as f64 * curve) as i32,
        y: start.y + distance_y * 2 / 3 - (perpendicular_y as f64 * curve) as i32,
    };
    let step_ms = random.random_range(8_u64..=14_u64);
    let steps = (duration_ms / step_ms).max(2);

    for step in 1..=steps {
        let t = step as f64 / steps as f64;
        let point = cubic_bezier(start, control_one, control_two, target, t);
        unsafe { SetCursorPos(point.x, point.y) }.map_err(|source| AppError::Windows {
            operation: "SetCursorPos",
            source,
        })?;
        if step < steps {
            thread::sleep(Duration::from_millis(step_ms));
        }
    }
    Ok(())
}

fn cubic_bezier(start: Point, first: Point, second: Point, end: Point, t: f64) -> Point {
    let inverse = 1.0 - t;
    let x = inverse.powi(3) * start.x as f64
        + 3.0 * inverse.powi(2) * t * first.x as f64
        + 3.0 * inverse * t.powi(2) * second.x as f64
        + t.powi(3) * end.x as f64;
    let y = inverse.powi(3) * start.y as f64
        + 3.0 * inverse.powi(2) * t * first.y as f64
        + 3.0 * inverse * t.powi(2) * second.y as f64
        + t.powi(3) * end.y as f64;
    Point {
        x: x.round() as i32,
        y: y.round() as i32,
    }
}

fn send_left_click() -> AppResult<()> {
    let inputs = [
        mouse_input(MOUSEEVENTF_LEFTDOWN),
        mouse_input(MOUSEEVENTF_LEFTUP),
    ];
    let sent = unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) };
    if sent != inputs.len() as u32 {
        let code = unsafe { GetLastError() }.0;
        if sent == 0 && code != 0 {
            return Err(AppError::Win32 {
                operation: "SendInput",
                code,
            });
        }
        return Err(AppError::PartialInput {
            requested: inputs.len() as u32,
            sent,
        });
    }
    Ok(())
}

fn mouse_input(flags: windows::Win32::UI::Input::KeyboardAndMouse::MOUSE_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: 0,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use crate::image::Point;

    use super::cubic_bezier;

    #[test]
    fn bezier_curve_starts_and_ends_at_requested_points() {
        let start = Point { x: 10, y: 20 };
        let end = Point { x: 110, y: 220 };
        let first = Point { x: 30, y: 90 };
        let second = Point { x: 80, y: 160 };

        assert_eq!(cubic_bezier(start, first, second, end, 0.0), start);
        assert_eq!(cubic_bezier(start, first, second, end, 1.0), end);
    }
}
