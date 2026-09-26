//! Experimental Kitty animation-frame output for the media smoke test.
use anyhow::{ensure, Result};
use base64::Engine as _;
use ratatui_image::FontSize;

use crate::{
    kitty_viewer::kitty_viewer_geometry,
    ratty_bitmap::{Destination, ViewerView},
};

const IMAGE_ID: u32 = 0x5243_0020;

#[derive(Default)]
pub struct KittyVideo {
    dimensions: Option<(u32, u32)>,
    destination: Option<Destination>,
}

impl KittyVideo {
    pub fn update(&mut self, rgba: &[u8], width: u32, height: u32) -> Result<Vec<Vec<u8>>> {
        ensure!(width > 0 && height > 0, "empty video frame");
        let expected = u64::from(width) * u64::from(height) * 4;
        ensure!(
            rgba.len() as u64 == expected,
            "invalid RGBA video frame length"
        );
        let mut commands = Vec::new();
        if self.dimensions.is_some_and(|size| size != (width, height)) {
            commands.extend(self.clear());
        }
        let action = if self.dimensions.is_some() {
            "a=f,r=1,X=1"
        } else {
            "a=t"
        };
        let payload = base64::engine::general_purpose::STANDARD.encode(rgba);
        let chunks = payload.as_bytes().chunks(4096);
        let count = chunks.len();
        for (index, chunk) in chunks.enumerate() {
            // Repeat action/metadata on continuation chunks as well, so older
            // Kitty releases cannot interpret a frame continuation as a new image.
            let more = u8::from(index + 1 < count);
            let mut command =
                format!("\x1b_G{action},i={IMAGE_ID},f=32,s={width},v={height},t=d,q=2,m={more};")
                    .into_bytes();
            command.extend_from_slice(chunk);
            command.extend_from_slice(b"\x1b\\");
            commands.push(command);
        }
        self.dimensions = Some((width, height));
        Ok(commands)
    }

    pub fn place(&mut self, viewport: Destination, font_size: FontSize) -> Vec<Vec<u8>> {
        let Some((width, height)) = self.dimensions else {
            return Vec::new();
        };
        let (destination, _) = kitty_viewer_geometry(
            width,
            height,
            ViewerView::new(100, 0, 0),
            viewport,
            font_size,
        );
        if self.destination == Some(destination) {
            return Vec::new();
        }
        self.destination = Some(destination);
        vec![format!(
            "\x1b7\x1b[{};{}H\x1b_Ga=p,i={IMAGE_ID},p=1,c={},r={},C=1,q=2\x1b\\\x1b8",
            u32::from(destination.row) + 1,
            u32::from(destination.col) + 1,
            destination.width,
            destination.height,
        )
        .into_bytes()]
    }

    pub fn clear(&mut self) -> Vec<Vec<u8>> {
        self.destination = None;
        if self.dimensions.take().is_some() {
            vec![format!("\x1b_Ga=d,d=I,i={IMAGE_ID},q=2\x1b\\").into_bytes()]
        } else {
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kitty_video_updates_root_frame_without_retransmit_or_delete() {
        let mut video = KittyVideo::default();
        let first = String::from_utf8(video.update(&[255; 16], 2, 2).unwrap().concat()).unwrap();
        assert!(first.contains("a=t,"));
        let next = String::from_utf8(video.update(&[0; 16], 2, 2).unwrap().concat()).unwrap();
        assert!(next.contains("a=f,r=1,X=1,"));
        assert!(!next.contains("a=t,"));
        assert!(!next.contains("a=d,"));
    }

    #[test]
    fn kitty_video_chunks_preserve_frame_action() {
        let mut video = KittyVideo::default();
        video.update(&vec![0; 64 * 64 * 4], 64, 64).unwrap();
        let commands = video.update(&vec![255; 64 * 64 * 4], 64, 64).unwrap();
        assert!(commands.len() > 1);
        for command in &commands {
            let command = std::str::from_utf8(command).unwrap();
            assert!(command.contains("a=f,"));
            let payload = command
                .split_once(';')
                .unwrap()
                .1
                .strip_suffix("\x1b\\")
                .unwrap();
            assert!(payload.len() <= 4096);
        }
        assert!(std::str::from_utf8(commands.last().unwrap())
            .unwrap()
            .contains("m=0;"));
    }
}
