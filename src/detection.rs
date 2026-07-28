use std::collections::VecDeque;

use crate::image::{Frame, Point, Rect, clamp_rect};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DialogCandidate {
    pub bounds: Rect,
    pub accept_button: Rect,
    pub reject_button: Rect,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Component {
    bounds: Rect,
    pixels: u32,
}

#[derive(Clone, Copy)]
enum ColorClass {
    Green,
    Red,
    Blue,
}

pub fn find_resurrection_dialog(frame: &Frame) -> Option<DialogCandidate> {
    let green = color_components(frame, ColorClass::Green)
        .into_iter()
        .filter(is_button_component)
        .collect::<Vec<Component>>();
    let red = color_components(frame, ColorClass::Red)
        .into_iter()
        .filter(is_button_component)
        .collect::<Vec<Component>>();

    green
        .iter()
        .flat_map(|accept| {
            red.iter()
                .filter_map(|reject| pair_dialog(frame, *accept, *reject))
        })
        .max_by_key(|candidate| candidate_score(frame, *candidate))
}

pub fn find_player_panel_regions(frame: &Frame) -> Vec<Rect> {
    let blue = color_components(frame, ColorClass::Blue)
        .into_iter()
        .filter(is_status_bar_component)
        .collect::<Vec<Component>>();
    let green = color_components(frame, ColorClass::Green)
        .into_iter()
        .filter(is_status_bar_component)
        .collect::<Vec<Component>>();

    let mut regions = blue
        .iter()
        .flat_map(|blue_bar| {
            green
                .iter()
                .filter_map(|green_bar| panel_region(frame, *blue_bar, *green_bar))
        })
        .collect::<Vec<Rect>>();
    regions.sort_by_key(|rect| (rect.y, rect.x));
    regions.dedup_by(|left, right| rectangles_overlap_significantly(*left, *right));
    regions
}

fn pair_dialog(frame: &Frame, accept: Component, reject: Component) -> Option<DialogCandidate> {
    let accept_bounds = accept.bounds;
    let reject_bounds = reject.bounds;
    let average_width = (accept_bounds.width + reject_bounds.width) / 2;
    let average_height = (accept_bounds.height + reject_bounds.height) / 2;
    let vertical_delta = (accept_bounds.y - reject_bounds.y).unsigned_abs();
    let gap = reject_bounds.x - accept_bounds.right();

    if accept_bounds.x >= reject_bounds.x
        || vertical_delta > average_height / 2
        || gap < 4
        || gap > average_width as i32
        || !similar_size(accept_bounds, reject_bounds)
    {
        return None;
    }

    let horizontal_margin = (average_width as f32 * 0.72).round() as i32;
    let top_margin = (average_height as f32 * 5.25).round() as i32;
    let bottom_margin = (average_height as f32 * 0.55).round() as i32;
    let left = accept_bounds.x - horizontal_margin;
    let top = accept_bounds.y.min(reject_bounds.y) - top_margin;
    let right = reject_bounds.right() + horizontal_margin;
    let bottom = accept_bounds.bottom().max(reject_bounds.bottom()) + bottom_margin;
    let bounds = clamp_rect(
        Rect {
            x: left,
            y: top,
            width: (right - left).max(1) as u32,
            height: (bottom - top).max(1) as u32,
        },
        frame.width,
        frame.height,
    )?;

    Some(DialogCandidate {
        bounds,
        accept_button: accept_bounds,
        reject_button: reject_bounds,
    })
}

fn panel_region(frame: &Frame, blue: Component, green: Component) -> Option<Rect> {
    let blue_bounds = blue.bounds;
    let green_bounds = green.bounds;
    let vertical_gap = green_bounds.y - blue_bounds.bottom();
    let left_delta = (green_bounds.x - blue_bounds.x).unsigned_abs();
    let width_ratio = blue_bounds.width.max(green_bounds.width) as f32
        / blue_bounds.width.min(green_bounds.width) as f32;

    if !(-3..=20).contains(&vertical_gap) || left_delta > 30 || width_ratio > 1.8 {
        return None;
    }

    let left = blue_bounds.x.min(green_bounds.x) - 22;
    let top = blue_bounds.y - 55;
    let right = blue_bounds.right().max(green_bounds.right()) + 24;
    let bottom = green_bounds.bottom() + 16;
    clamp_rect(
        Rect {
            x: left,
            y: top,
            width: (right - left).max(1) as u32,
            height: (bottom - top).max(1) as u32,
        },
        frame.width,
        frame.height,
    )
}

fn is_button_component(component: &Component) -> bool {
    let bounds = component.bounds;
    let area = bounds.width * bounds.height;
    (55..=360).contains(&bounds.width)
        && (12..=90).contains(&bounds.height)
        && component.pixels * 100 / area.max(1) >= 22
}

fn is_status_bar_component(component: &Component) -> bool {
    let bounds = component.bounds;
    let area = bounds.width * bounds.height;
    (45..=500).contains(&bounds.width)
        && (2..=18).contains(&bounds.height)
        && component.pixels * 100 / area.max(1) >= 35
}

fn similar_size(left: Rect, right: Rect) -> bool {
    let width_ratio = left.width.max(right.width) as f32 / left.width.min(right.width) as f32;
    let height_ratio = left.height.max(right.height) as f32 / left.height.min(right.height) as f32;
    width_ratio <= 1.45 && height_ratio <= 1.8
}

fn candidate_score(frame: &Frame, candidate: DialogCandidate) -> i64 {
    let center = candidate.bounds.center();
    let frame_center = Point {
        x: frame.width as i32 / 2,
        y: frame.height as i32 / 2,
    };
    let distance =
        (center.x - frame_center.x).unsigned_abs() + (center.y - frame_center.y).unsigned_abs();
    candidate.accept_button.width as i64 * candidate.accept_button.height as i64
        + candidate.reject_button.width as i64 * candidate.reject_button.height as i64
        - distance as i64
}

fn rectangles_overlap_significantly(left: Rect, right: Rect) -> bool {
    let overlap_width = left.right().min(right.right()) - left.x.max(right.x);
    let overlap_height = left.bottom().min(right.bottom()) - left.y.max(right.y);
    if overlap_width <= 0 || overlap_height <= 0 {
        return false;
    }
    let overlap = overlap_width as u32 * overlap_height as u32;
    let smaller = (left.width * left.height).min(right.width * right.height);
    overlap * 2 >= smaller
}

fn color_components(frame: &Frame, class: ColorClass) -> Vec<Component> {
    let width = frame.width as usize;
    let height = frame.height as usize;
    let mut mask = vec![false; width * height];
    for y in 0..height {
        for x in 0..width {
            mask[y * width + x] = color_matches(frame.pixel(x as u32, y as u32), class);
        }
    }

    let mut visited = vec![false; mask.len()];
    let mut components = Vec::<Component>::new();
    for y in 0..height {
        for x in 0..width {
            let index = y * width + x;
            if !mask[index] || visited[index] {
                continue;
            }
            components.push(collect_component(&mask, &mut visited, width, height, x, y));
        }
    }
    components
}

fn collect_component(
    mask: &[bool],
    visited: &mut [bool],
    width: usize,
    height: usize,
    start_x: usize,
    start_y: usize,
) -> Component {
    let mut queue = VecDeque::<(usize, usize)>::from([(start_x, start_y)]);
    let mut minimum_x = start_x;
    let mut maximum_x = start_x;
    let mut minimum_y = start_y;
    let mut maximum_y = start_y;
    let mut pixels = 0_u32;
    visited[start_y * width + start_x] = true;

    while let Some((x, y)) = queue.pop_front() {
        pixels += 1;
        minimum_x = minimum_x.min(x);
        maximum_x = maximum_x.max(x);
        minimum_y = minimum_y.min(y);
        maximum_y = maximum_y.max(y);

        for (next_x, next_y) in neighbors(x, y, width, height) {
            let index = next_y * width + next_x;
            if mask[index] && !visited[index] {
                visited[index] = true;
                queue.push_back((next_x, next_y));
            }
        }
    }

    Component {
        bounds: Rect {
            x: minimum_x as i32,
            y: minimum_y as i32,
            width: (maximum_x - minimum_x + 1) as u32,
            height: (maximum_y - minimum_y + 1) as u32,
        },
        pixels,
    }
}

fn neighbors(
    x: usize,
    y: usize,
    width: usize,
    height: usize,
) -> impl Iterator<Item = (usize, usize)> {
    let mut points = [(0_usize, 0_usize); 4];
    let mut count = 0_usize;
    if x > 0 {
        points[count] = (x - 1, y);
        count += 1;
    }
    if x + 1 < width {
        points[count] = (x + 1, y);
        count += 1;
    }
    if y > 0 {
        points[count] = (x, y - 1);
        count += 1;
    }
    if y + 1 < height {
        points[count] = (x, y + 1);
        count += 1;
    }
    points.into_iter().take(count)
}

fn color_matches(pixel: [u8; 4], class: ColorClass) -> bool {
    let blue = pixel[0] as i16;
    let green = pixel[1] as i16;
    let red = pixel[2] as i16;
    match class {
        ColorClass::Green => {
            green >= 42 && green > red + 8 && green > blue + 10 && red <= 125 && blue <= 100
        }
        ColorClass::Red => {
            red >= 52 && red > green + 14 && red > blue + 12 && green <= 105 && blue <= 105
        }
        ColorClass::Blue => {
            blue >= 40 && blue > red + 10 && blue > green + 5 && red <= 150 && green <= 180
        }
    }
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::fs;

    use crate::image::{Frame, Point, Rect};

    use super::{
        ColorClass, color_components, find_player_panel_regions, find_resurrection_dialog,
    };

    fn frame_with_rectangles(rectangles: &[(Rect, [u8; 4])]) -> Frame {
        let width = 800_u32;
        let height = 600_u32;
        let mut pixels = vec![15_u8; width as usize * height as usize * 4];
        for alpha in pixels.iter_mut().skip(3).step_by(4) {
            *alpha = 255;
        }
        for (rect, color) in rectangles {
            for y in rect.y as u32..rect.bottom() as u32 {
                for x in rect.x as u32..rect.right() as u32 {
                    let offset = (y as usize * width as usize + x as usize) * 4;
                    pixels[offset..offset + 4].copy_from_slice(color);
                }
            }
        }
        Frame::new(Point { x: 0, y: 0 }, width, height, pixels).unwrap()
    }

    #[test]
    fn finds_paired_dialog_buttons() {
        let frame = frame_with_rectangles(&[
            (
                Rect {
                    x: 310,
                    y: 360,
                    width: 110,
                    height: 28,
                },
                [24, 70, 44, 255],
            ),
            (
                Rect {
                    x: 432,
                    y: 360,
                    width: 110,
                    height: 28,
                },
                [35, 35, 86, 255],
            ),
        ]);

        let dialog = find_resurrection_dialog(&frame).unwrap();

        assert_eq!(dialog.accept_button.center(), Point { x: 365, y: 374 });
        assert_eq!(dialog.reject_button.center(), Point { x: 487, y: 374 });
        assert!(dialog.bounds.contains(dialog.accept_button.center()));
    }

    #[test]
    fn finds_status_panel_from_blue_and_green_bars() {
        let frame = frame_with_rectangles(&[
            (
                Rect {
                    x: 50,
                    y: 70,
                    width: 150,
                    height: 7,
                },
                [148, 110, 84, 255],
            ),
            (
                Rect {
                    x: 50,
                    y: 82,
                    width: 150,
                    height: 7,
                },
                [28, 134, 94, 255],
            ),
        ]);

        let regions = find_player_panel_regions(&frame);

        assert_eq!(regions.len(), 1);
        assert!(regions[0].contains(Point { x: 70, y: 30 }));
    }

    #[test]
    #[ignore = "set RES_BOT_RAW_FRAME, RES_BOT_FRAME_WIDTH and RES_BOT_FRAME_HEIGHT"]
    fn analyzes_external_reference_frame() {
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
        let frame = Frame::new(Point { x: 0, y: 0 }, width, height, pixels).unwrap();

        let dialog = find_resurrection_dialog(&frame);
        let panels = find_player_panel_regions(&frame);
        let blue_bars = color_components(&frame, ColorClass::Blue)
            .into_iter()
            .filter(|component| component.bounds.width >= 40)
            .collect::<Vec<_>>();
        let green_bars = color_components(&frame, ColorClass::Green)
            .into_iter()
            .filter(|component| component.bounds.width >= 40)
            .collect::<Vec<_>>();
        println!("dialog={dialog:?}, panels={panels:?}");
        println!("blue={blue_bars:?}");
        println!("green={green_bars:?}");

        assert!(dialog.is_some());
        assert!(!panels.is_empty());
    }
}
