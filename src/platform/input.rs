use std::thread;
use std::time::{Duration, Instant};

use rand::Rng;
use windows::Win32::Foundation::{GetLastError, POINT};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetAsyncKeyState, INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_LEFTDOWN,
    MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_VIRTUALDESK, MOUSEINPUT, SendInput,
    VK_CONTROL, VK_F12, VK_SHIFT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN,
    SM_YVIRTUALSCREEN,
};

use crate::error::{AppError, AppResult};
use crate::image::{Point, Rect};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClickOutcome {
    pub target: Point,
    pub duration_ms: u64,
    pub cursor_relocated: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClickAttempt {
    Clicked(ClickOutcome),
    Cancelled(ClickCancellation),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClickCancellation {
    MouseMovedDuringApproach { expected: Point, actual: Point },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MovementOutcome {
    Completed,
    Interrupted { expected: Point, actual: Point },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CompletedClick {
    original: Point,
    target: Point,
    duration_ms: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClickTiming {
    pub minimum_duration_ms: u64,
    pub maximum_duration_ms: u64,
}

pub fn stop_shortcut_pressed() -> bool {
    key_pressed(VK_CONTROL.0) && key_pressed(VK_SHIFT.0) && key_pressed(VK_F12.0)
}

pub fn click_human_like(
    button: Rect,
    frame_origin: Point,
    timing: ClickTiming,
) -> AppResult<ClickAttempt> {
    let completed = perform_click(button, frame_origin, timing)?;
    Ok(match completed {
        Ok(click) => ClickAttempt::Clicked(ClickOutcome {
            target: click.target,
            duration_ms: click.duration_ms,
            cursor_relocated: false,
        }),
        Err(cancellation) => ClickAttempt::Cancelled(cancellation),
    })
}

pub fn click_human_like_and_relocate(
    button: Rect,
    frame_origin: Point,
    timing: ClickTiming,
) -> AppResult<ClickAttempt> {
    let completed = perform_click(button, frame_origin, timing)?;
    let Ok(click) = completed else {
        return Ok(ClickAttempt::Cancelled(
            completed.expect_err("failed click contains a cancellation reason"),
        ));
    };
    thread::sleep(Duration::from_millis(80));
    let relocation_duration = (click.duration_ms * 2 / 3).max(120);
    let mut random = rand::rng();
    let relocation_target =
        random_nearby_point(click.original, virtual_desktop_bounds()?, &mut random);
    let cursor_relocated = matches!(
        move_cursor_bezier(
            click.target,
            relocation_target,
            relocation_duration,
            &mut random,
        )?,
        MovementOutcome::Completed
    );
    Ok(ClickAttempt::Clicked(ClickOutcome {
        target: click.target,
        duration_ms: click.duration_ms,
        cursor_relocated,
    }))
}

fn perform_click(
    button: Rect,
    frame_origin: Point,
    timing: ClickTiming,
) -> AppResult<Result<CompletedClick, ClickCancellation>> {
    let original = cursor_position()?;
    let mut random = rand::rng();
    let target = random_point_in_button(button, frame_origin, &mut random);
    let duration_ms = random.random_range(timing.minimum_duration_ms..=timing.maximum_duration_ms);
    match move_cursor_bezier(original, target, duration_ms, &mut random)? {
        MovementOutcome::Completed => {
            send_left_click_at(target)?;
            Ok(Ok(CompletedClick {
                original,
                target,
                duration_ms,
            }))
        }
        MovementOutcome::Interrupted { expected, actual } => {
            Ok(Err(ClickCancellation::MouseMovedDuringApproach {
                expected,
                actual,
            }))
        }
    }
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

pub fn wait_for_mouse_idle(required_ms: u64, timeout_ms: u64) -> AppResult<bool> {
    let timeout = Instant::now() + Duration::from_millis(timeout_ms);
    let mut previous = cursor_position()?;
    let mut stable_since = Instant::now();
    while Instant::now() < timeout {
        thread::sleep(Duration::from_millis(50));
        let current = cursor_position()?;
        if point_distance_exceeds(previous, current, 2) {
            stable_since = Instant::now();
        } else if stable_since.elapsed() >= Duration::from_millis(required_ms) {
            return Ok(true);
        }
        previous = current;
    }
    Ok(false)
}

fn move_cursor_bezier(
    start: Point,
    target: Point,
    duration_ms: u64,
    random: &mut impl Rng,
) -> AppResult<MovementOutcome> {
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

    let mut commanded_points = vec![start];
    for step in 1..=steps {
        if step > 1 {
            let actual = cursor_position()?;
            if !matches_commanded_path(actual, &commanded_points, 8) {
                return Ok(MovementOutcome::Interrupted {
                    expected: *commanded_points
                        .last()
                        .expect("the commanded path always contains its start"),
                    actual,
                });
            }
        }
        let t = step as f64 / steps as f64;
        let point = cubic_bezier(start, control_one, control_two, target, t);
        send_absolute_move(point)?;
        commanded_points.push(point);
        if step < steps {
            thread::sleep(Duration::from_millis(step_ms));
        }
    }
    Ok(MovementOutcome::Completed)
}

fn matches_commanded_path(actual: Point, commanded_points: &[Point], tolerance: i32) -> bool {
    commanded_points
        .iter()
        .any(|commanded| !point_distance_exceeds(*commanded, actual, tolerance))
}

fn point_distance_exceeds(left: Point, right: Point, tolerance: i32) -> bool {
    (left.x - right.x).unsigned_abs() > tolerance as u32
        || (left.y - right.y).unsigned_abs() > tolerance as u32
}

fn virtual_desktop_bounds() -> AppResult<Rect> {
    let x = unsafe { GetSystemMetrics(SM_XVIRTUALSCREEN) };
    let y = unsafe { GetSystemMetrics(SM_YVIRTUALSCREEN) };
    let width = unsafe { GetSystemMetrics(SM_CXVIRTUALSCREEN) };
    let height = unsafe { GetSystemMetrics(SM_CYVIRTUALSCREEN) };
    if width <= 0 || height <= 0 {
        return Err(AppError::InvalidVirtualDesktop {
            x,
            y,
            width,
            height,
        });
    }
    Ok(Rect {
        x,
        y,
        width: width as u32,
        height: height as u32,
    })
}

fn random_nearby_point(origin: Point, desktop: Rect, random: &mut impl Rng) -> Point {
    let radius = random.random_range(35.0_f64..=90.0_f64);
    let angle = random.random_range(0.0_f64..std::f64::consts::TAU);
    let x = origin.x + (angle.cos() * radius).round() as i32;
    let y = origin.y + (angle.sin() * radius).round() as i32;
    Point {
        x: x.clamp(desktop.x + 2, desktop.right() - 3),
        y: y.clamp(desktop.y + 2, desktop.bottom() - 3),
    }
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

fn send_left_click_at(point: Point) -> AppResult<()> {
    let inputs = [
        absolute_move_input(point)?,
        mouse_input(MOUSEEVENTF_LEFTDOWN),
        mouse_input(MOUSEEVENTF_LEFTUP),
    ];
    send_inputs("SendInput absolute movement and left click", &inputs)
}

fn send_absolute_move(point: Point) -> AppResult<()> {
    let input = absolute_move_input(point)?;
    send_inputs("SendInput absolute mouse movement", &[input])
}

fn absolute_move_input(point: Point) -> AppResult<INPUT> {
    let desktop = virtual_desktop_bounds()?;
    let normalized_x = normalize_absolute_coordinate(point.x, desktop.x, desktop.width);
    let normalized_y = normalize_absolute_coordinate(point.y, desktop.y, desktop.height);
    Ok(INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: normalized_x,
                dy: normalized_y,
                mouseData: 0,
                dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    })
}

fn normalize_absolute_coordinate(coordinate: i32, origin: i32, extent: u32) -> i32 {
    let maximum = extent.saturating_sub(1).max(1) as i64;
    let relative = i64::from(coordinate - origin).clamp(0, maximum);
    ((relative * 65_535) / maximum) as i32
}

fn send_inputs(operation: &'static str, inputs: &[INPUT]) -> AppResult<()> {
    let sent = unsafe { SendInput(inputs, size_of::<INPUT>() as i32) };
    if sent == inputs.len() as u32 {
        return Ok(());
    }
    let code = unsafe { GetLastError() }.0;
    if sent == 0 && code != 0 {
        return Err(AppError::Win32 { operation, code });
    }
    Err(AppError::PartialInput {
        requested: inputs.len() as u32,
        sent,
    })
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
    use rand::SeedableRng;
    use rand::rngs::StdRng;

    use crate::image::{Point, Rect};

    use super::{
        cubic_bezier, matches_commanded_path, normalize_absolute_coordinate,
        point_distance_exceeds, random_nearby_point,
    };

    #[test]
    fn bezier_curve_starts_and_ends_at_requested_points() {
        let start = Point { x: 10, y: 20 };
        let end = Point { x: 110, y: 220 };
        let first = Point { x: 30, y: 90 };
        let second = Point { x: 80, y: 160 };

        assert_eq!(cubic_bezier(start, first, second, end, 0.0), start);
        assert_eq!(cubic_bezier(start, first, second, end, 1.0), end);
    }

    #[test]
    fn cursor_tolerance_detects_user_movement() {
        let start = Point { x: 100, y: 200 };

        assert!(!point_distance_exceeds(start, Point { x: 102, y: 198 }, 2));
        assert!(point_distance_exceeds(start, Point { x: 103, y: 200 }, 2));
    }

    #[test]
    fn delayed_positions_from_own_commands_are_not_user_movement() {
        let commanded = [
            Point { x: 100, y: 200 },
            Point { x: 120, y: 210 },
            Point { x: 140, y: 220 },
        ];

        assert!(matches_commanded_path(
            Point { x: 102, y: 199 },
            &commanded,
            8
        ));
        assert!(matches_commanded_path(
            Point { x: 121, y: 212 },
            &commanded,
            8
        ));
        assert!(!matches_commanded_path(
            Point { x: 170, y: 260 },
            &commanded,
            8
        ));
    }

    #[test]
    fn relocation_target_stays_near_origin_and_inside_desktop() {
        let origin = Point { x: 500, y: 500 };
        let desktop = Rect {
            x: 0,
            y: 0,
            width: 1_000,
            height: 1_000,
        };
        let mut random = StdRng::seed_from_u64(42);

        let target = random_nearby_point(origin, desktop, &mut random);

        assert!(desktop.contains(target));
        assert!((target.x - origin.x).unsigned_abs() <= 90);
        assert!((target.y - origin.y).unsigned_abs() <= 90);
        assert_ne!(target, origin);
    }

    #[test]
    fn absolute_coordinates_cover_virtual_desktop_range() {
        assert_eq!(normalize_absolute_coordinate(-1_920, -1_920, 3_840), 0);
        assert_eq!(normalize_absolute_coordinate(1_919, -1_920, 3_840), 65_535);
        assert_eq!(normalize_absolute_coordinate(0, 0, 1_920), 0);
    }
}
