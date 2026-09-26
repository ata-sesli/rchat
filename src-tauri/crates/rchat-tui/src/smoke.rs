use crate::media::DecodedRgbaFrame;

#[derive(Debug)]
pub struct SmokeFrameGenerator {
    width: u32,
    height: u32,
    seq: u32,
}

impl SmokeFrameGenerator {
    pub fn new(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            seq: 0,
        }
    }

    pub fn next_frame(&mut self) -> DecodedRgbaFrame {
        let seq = self.seq;
        self.seq = self.seq.wrapping_add(1);

        let mut rgba = vec![0_u8; (self.width as usize) * (self.height as usize) * 4];
        let marker_x = (seq.wrapping_mul(7) % self.width.max(1)) as i32;
        let marker_y = (seq.wrapping_mul(5) % self.height.max(1)) as i32;

        for y in 0..self.height as i32 {
            for x in 0..self.width as i32 {
                let index = ((y as u32 * self.width + x as u32) * 4) as usize;
                let dx = (x - marker_x).abs();
                let dy = (y - marker_y).abs();
                let active = dx < 24 && dy < 24;
                rgba[index] = if active {
                    255
                } else {
                    ((x * 255) / self.width as i32) as u8
                };
                rgba[index + 1] = if active {
                    80
                } else {
                    ((y * 255) / self.height as i32) as u8
                };
                rgba[index + 2] = if active { 40 } else { 110 };
                rgba[index + 3] = 255;
            }
        }

        DecodedRgbaFrame {
            session_id: "media-smoke".to_string(),
            seq,
            timestamp_us: i64::from(seq) * 100_000,
            width: self.width,
            height: self.height,
            rgba,
        }
    }

    /// Synthetic native-video input: a moving bright square over a luma gradient.
    pub fn next_i420_frame(&mut self, fps: u32) -> rchat_screen_capture::I420ScreenFrame {
        let seq = self.seq;
        self.seq = self.seq.wrapping_add(1);
        let y_len = self.width as usize * self.height as usize;
        let mut data =
            vec![128_u8; y_len + 2 * (self.width as usize / 2) * (self.height as usize / 2)];
        let marker_x = seq.wrapping_mul(7) % self.width.max(1);
        let marker_y = seq.wrapping_mul(5) % self.height.max(1);
        for y in 0..self.height {
            for x in 0..self.width {
                data[(y * self.width + x) as usize] =
                    if x.abs_diff(marker_x) < 24 && y.abs_diff(marker_y) < 24 {
                        235
                    } else {
                        (16 + x * 180 / self.width.max(1)) as u8
                    };
            }
        }
        rchat_screen_capture::I420ScreenFrame {
            timestamp_us: i64::from(seq) * 1_000_000 / i64::from(fps.max(1)),
            width: self.width,
            height: self.height,
            data,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_smoke_frame_generator_produces_valid_rgba_frames() {
        let mut generator = SmokeFrameGenerator::new(64, 32);

        let first = generator.next_frame();
        let second = generator.next_frame();

        assert_eq!(first.width, 64);
        assert_eq!(first.height, 32);
        assert_eq!(first.rgba.len(), 64 * 32 * 4);
        assert_eq!(first.seq, 0);
        assert_eq!(second.seq, 1);
        assert_ne!(first.rgba, second.rgba);
    }
}
