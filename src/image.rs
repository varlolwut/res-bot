use crate::error::{AppError, AppResult};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Rect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    pub fn right(self) -> i32 {
        self.x + self.width as i32
    }

    pub fn bottom(self) -> i32 {
        self.y + self.height as i32
    }

    pub fn center(self) -> Point {
        Point {
            x: self.x + self.width as i32 / 2,
            y: self.y + self.height as i32 / 2,
        }
    }

    #[cfg(test)]
    pub fn contains(self, point: Point) -> bool {
        point.x >= self.x && point.x < self.right() && point.y >= self.y && point.y < self.bottom()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Frame {
    pub origin: Point,
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

impl Frame {
    pub fn new(origin: Point, width: u32, height: u32, bgra: Vec<u8>) -> AppResult<Self> {
        let expected = width as usize * height as usize * 4;
        if width == 0 || height == 0 || bgra.len() != expected {
            return Err(AppError::InvalidImage {
                width,
                height,
                bytes: bgra.len(),
            });
        }
        Ok(Self {
            origin,
            width,
            height,
            bgra,
        })
    }

    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        let offset = (y as usize * self.width as usize + x as usize) * 4;
        [
            self.bgra[offset],
            self.bgra[offset + 1],
            self.bgra[offset + 2],
            self.bgra[offset + 3],
        ]
    }

    pub fn crop(&self, rect: Rect) -> AppResult<Self> {
        if rect.x < 0
            || rect.y < 0
            || rect.right() > self.width as i32
            || rect.bottom() > self.height as i32
        {
            return Err(AppError::InvalidCrop {
                x: rect.x,
                y: rect.y,
                width: rect.width,
                height: rect.height,
            });
        }

        let row_bytes = rect.width as usize * 4;
        let mut bgra = Vec::<u8>::with_capacity(row_bytes * rect.height as usize);
        for y in rect.y as usize..rect.bottom() as usize {
            let start = (y * self.width as usize + rect.x as usize) * 4;
            bgra.extend_from_slice(&self.bgra[start..start + row_bytes]);
        }

        Self::new(
            Point {
                x: self.origin.x + rect.x,
                y: self.origin.y + rect.y,
            },
            rect.width,
            rect.height,
            bgra,
        )
    }

    pub fn scale_nearest(&self, factor: u32) -> AppResult<Self> {
        let width = self.width * factor;
        let height = self.height * factor;
        let mut bgra = vec![0_u8; width as usize * height as usize * 4];

        for output_y in 0..height {
            let source_y = output_y / factor;
            for output_x in 0..width {
                let source_x = output_x / factor;
                let source_offset =
                    (source_y as usize * self.width as usize + source_x as usize) * 4;
                let output_offset = (output_y as usize * width as usize + output_x as usize) * 4;
                bgra[output_offset..output_offset + 4]
                    .copy_from_slice(&self.bgra[source_offset..source_offset + 4]);
            }
        }

        Self::new(self.origin, width, height, bgra)
    }

    pub fn high_contrast_text(&self) -> AppResult<Self> {
        let mut bgra = Vec::<u8>::with_capacity(self.bgra.len());
        for pixel in self.bgra.chunks_exact(4) {
            let blue = pixel[0];
            let green = pixel[1];
            let red = pixel[2];
            let maximum = red.max(green).max(blue);
            let minimum = red.min(green).min(blue);
            let is_text = maximum >= 115 && maximum.saturating_sub(minimum) <= 90;
            let value = if is_text { 0_u8 } else { 255_u8 };
            bgra.extend_from_slice(&[value, value, value, 255]);
        }
        Self::new(self.origin, self.width, self.height, bgra)
    }
}

pub fn clamp_rect(rect: Rect, bounds_width: u32, bounds_height: u32) -> Option<Rect> {
    let left = rect.x.max(0);
    let top = rect.y.max(0);
    let right = rect.right().min(bounds_width as i32);
    let bottom = rect.bottom().min(bounds_height as i32);
    if right <= left || bottom <= top {
        return None;
    }
    Some(Rect {
        x: left,
        y: top,
        width: (right - left) as u32,
        height: (bottom - top) as u32,
    })
}

#[cfg(test)]
mod tests {
    use super::{Frame, Point, Rect};

    #[test]
    fn crop_preserves_pixels_and_moves_origin() {
        let pixels = vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16];
        let frame = Frame::new(Point { x: 10, y: 20 }, 2, 2, pixels).unwrap();

        let crop = frame
            .crop(Rect {
                x: 1,
                y: 0,
                width: 1,
                height: 2,
            })
            .unwrap();

        assert_eq!(crop.origin, Point { x: 11, y: 20 });
        assert_eq!(crop.bgra, vec![5, 6, 7, 8, 13, 14, 15, 16]);
    }
}
