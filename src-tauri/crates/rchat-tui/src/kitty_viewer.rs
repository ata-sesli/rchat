//! Direct Kitty placement for a static viewer image. Unlike the ratatui-image
//! fallback, pan and zoom only replace the placement; pixels are uploaded once.
use anyhow::{ensure, Context, Result};
use base64::Engine as _;
use image::{DynamicImage, ImageEncoder as _};
use ratatui_image::FontSize;

use crate::ratty_bitmap::{viewer_geometry, Destination, SourceRect, ViewerView};

const VIEWER_IMAGE_ID: u32 = 0x5243_0003;
const VIEWER_PLACEMENT_ID: u32 = 0x5243_1003;
const MAX_BASE64_CHUNK: usize = 4096;

#[derive(Debug)]
struct ViewerState {
    file_hash: String,
    image_width: u32,
    image_height: u32,
    destination: Destination,
    source: SourceRect,
}

#[derive(Debug)]
pub struct KittyViewerManager {
    viewer: Option<ViewerState>,
    font_size: FontSize,
}

impl Default for KittyViewerManager {
    fn default() -> Self {
        Self::new(FontSize::new(1, 1))
    }
}

impl KittyViewerManager {
    pub fn new(font_size: FontSize) -> Self {
        Self {
            viewer: None,
            font_size,
        }
    }

    pub fn update_viewer(
        &mut self,
        file_hash: &str,
        image: &DynamicImage,
        view: ViewerView,
        viewport: Destination,
    ) -> Result<Vec<Vec<u8>>> {
        ensure!(
            viewport.width > 0 && viewport.height > 0,
            "viewer viewport is empty"
        );
        ensure!(
            image.width() > 0 && image.height() > 0,
            "viewer image is empty"
        );
        let (destination, source) = kitty_viewer_geometry(
            image.width(),
            image.height(),
            view,
            viewport,
            self.font_size,
        );
        let same_image = self.viewer.as_ref().is_some_and(|state| {
            state.file_hash == file_hash
                && state.image_width == image.width()
                && state.image_height == image.height()
        });
        if same_image {
            let state = self.viewer.as_mut().expect("checked above");
            if state.destination == destination && state.source == source {
                return Ok(Vec::new());
            }
            state.destination = destination;
            state.source = source;
            return Ok(vec![encode_place(destination, source)]);
        }

        let mut commands = self.clear_viewer();
        let rgba = image.to_rgba8();
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(
                rgba.as_raw(),
                rgba.width(),
                rgba.height(),
                image::ExtendedColorType::Rgba8,
            )
            .context("failed to encode viewer image for Kitty")?;
        commands.extend(encode_upload(&png)?);
        commands.push(encode_place(destination, source));
        self.viewer = Some(ViewerState {
            file_hash: file_hash.to_string(),
            image_width: image.width(),
            image_height: image.height(),
            destination,
            source,
        });
        Ok(commands)
    }

    pub fn repaint(&self) -> Vec<Vec<u8>> {
        self.viewer
            .as_ref()
            .map(|state| vec![encode_place(state.destination, state.source)])
            .unwrap_or_default()
    }

    pub fn clear_viewer(&mut self) -> Vec<Vec<u8>> {
        if self.viewer.take().is_some() {
            vec![format!("\x1b_Ga=d,d=I,i={VIEWER_IMAGE_ID},q=2\x1b\\").into_bytes()]
        } else {
            Vec::new()
        }
    }
}

pub(crate) fn kitty_viewer_geometry(
    image_width: u32,
    image_height: u32,
    view: ViewerView,
    viewport: Destination,
    font_size: FontSize,
) -> (Destination, SourceRect) {
    let cell_width = u32::from(font_size.width.max(1));
    let cell_height = u32::from(font_size.height.max(1));
    let pixel_viewport = Destination::new(
        0,
        0,
        viewport.width.saturating_mul(cell_width),
        viewport.height.saturating_mul(cell_height),
    );
    let (pixel_destination, source) =
        viewer_geometry(image_width, image_height, view, pixel_viewport);
    let width = pixel_destination
        .width
        .div_ceil(cell_width)
        .min(viewport.width);
    let height = pixel_destination
        .height
        .div_ceil(cell_height)
        .min(viewport.height);
    let row = viewport.row.saturating_add(
        u16::try_from(viewport.height.saturating_sub(height) / 2).unwrap_or(u16::MAX),
    );
    let col = viewport.col.saturating_add(
        u16::try_from(viewport.width.saturating_sub(width) / 2).unwrap_or(u16::MAX),
    );
    (Destination::new(row, col, width, height), source)
}

fn encode_upload(png: &[u8]) -> Result<Vec<Vec<u8>>> {
    ensure!(!png.is_empty(), "viewer PNG is empty");
    let encoded = base64::engine::general_purpose::STANDARD.encode(png);
    let chunks = encoded
        .as_bytes()
        .chunks(MAX_BASE64_CHUNK)
        .collect::<Vec<_>>();
    let mut commands = Vec::with_capacity(chunks.len());
    for (index, chunk) in chunks.iter().enumerate() {
        let more = u8::from(index + 1 < chunks.len());
        let prefix = if index == 0 {
            format!("\x1b_Ga=t,f=100,t=d,i={VIEWER_IMAGE_ID},q=2,m={more};")
        } else {
            format!("\x1b_Gm={more},q=2;")
        };
        let mut command = prefix.into_bytes();
        command.extend_from_slice(chunk);
        command.extend_from_slice(b"\x1b\\");
        commands.push(command);
    }
    Ok(commands)
}

fn encode_place(destination: Destination, source: SourceRect) -> Vec<u8> {
    // Save and restore the TUI cursor because Kitty placements otherwise change it.
    format!(
        "\x1b7\x1b[{};{}H\x1b_Ga=p,i={VIEWER_IMAGE_ID},p={VIEWER_PLACEMENT_ID},x={},y={},w={},h={},c={},r={},C=1,q=2\x1b\\\x1b8",
        u32::from(destination.row) + 1,
        u32::from(destination.col) + 1,
        source.x,
        source.y,
        source.width,
        source.height,
        destination.width,
        destination.height,
    )
    .into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{DynamicImage, RgbaImage};

    fn image() -> DynamicImage {
        DynamicImage::ImageRgba8(RgbaImage::new(100, 100))
    }

    fn bytes(commands: &[Vec<u8>]) -> String {
        String::from_utf8(commands.concat()).unwrap()
    }

    #[test]
    fn uploads_once_then_updates_crop_without_pixel_payload() {
        let mut manager = KittyViewerManager::default();
        let destination = Destination::new(4, 2, 10, 10);
        let first = manager
            .update_viewer("first", &image(), ViewerView::new(100, 0, 0), destination)
            .unwrap();
        let first_wire = bytes(&first);
        assert!(first_wire.contains("a=t,f=100"));
        assert!(first_wire.contains("a=p,i="));
        assert!(first_wire.contains("c=10,r=10"));

        let second = manager
            .update_viewer("first", &image(), ViewerView::new(200, 10, 0), destination)
            .unwrap();
        let second_wire = bytes(&second);
        assert!(!second_wire.contains("a=t"));
        assert_eq!(second_wire.matches("a=p,i=").count(), 1);
        assert!(second_wire.contains("x=35,y=25,w=50,h=50"));
    }

    #[test]
    fn changing_image_releases_old_pixels_and_uploads_new_image() {
        let mut manager = KittyViewerManager::default();
        let destination = Destination::new(0, 0, 10, 10);
        manager
            .update_viewer("first", &image(), ViewerView::new(100, 0, 0), destination)
            .unwrap();
        let commands = manager
            .update_viewer("second", &image(), ViewerView::new(100, 0, 0), destination)
            .unwrap();
        let wire = bytes(&commands);
        assert!(wire.contains("a=d,d=I"));
        assert!(wire.contains("a=t,f=100"));
    }

    #[test]
    fn placement_preserves_cursor_and_uses_current_viewport() {
        let mut manager = KittyViewerManager::default();
        let commands = manager
            .update_viewer(
                "first",
                &image(),
                ViewerView::new(100, 0, 0),
                Destination::new(4, 2, 20, 10),
            )
            .unwrap();
        let wire = bytes(&commands);
        assert!(wire.contains("\x1b7\x1b[5;8H"));
        assert!(wire.contains("C=1,q=2"));
        assert!(wire.contains("\x1b8"));
    }

    #[test]
    fn unchanged_view_repaints_without_upload_and_clear_is_idempotent() {
        let mut manager = KittyViewerManager::default();
        let destination = Destination::new(0, 0, 10, 10);
        manager
            .update_viewer("first", &image(), ViewerView::new(100, 0, 0), destination)
            .unwrap();
        assert!(manager
            .update_viewer("first", &image(), ViewerView::new(100, 0, 0), destination)
            .unwrap()
            .is_empty());
        let repaint = bytes(&manager.repaint());
        assert!(repaint.contains("a=p,i="));
        assert!(!repaint.contains("a=t"));
        assert!(bytes(&manager.clear_viewer()).contains("a=d,d=I"));
        assert!(manager.clear_viewer().is_empty());
    }

    #[test]
    fn upload_chunks_do_not_exceed_kitty_limit() {
        let mut rgba = RgbaImage::new(128, 128);
        for (index, pixel) in rgba.pixels_mut().enumerate() {
            *pixel = image::Rgba([
                index as u8,
                (index >> 3) as u8,
                (index.wrapping_mul(73) >> 5) as u8,
                255,
            ]);
        }
        let mut manager = KittyViewerManager::default();
        let commands = manager
            .update_viewer(
                "large",
                &DynamicImage::ImageRgba8(rgba),
                ViewerView::new(100, 0, 0),
                Destination::new(0, 0, 20, 20),
            )
            .unwrap();
        let uploads = commands
            .iter()
            .filter(|command| command.starts_with(b"\x1b_Ga=t") || command.starts_with(b"\x1b_Gm="))
            .collect::<Vec<_>>();
        assert!(uploads.len() > 1);
        for command in uploads {
            let payload = command.split(|byte| *byte == b';').nth(1).unwrap();
            assert!(payload.len() - 2 <= MAX_BASE64_CHUNK);
        }
    }

    #[test]
    fn square_image_fits_physical_cells_not_square_character_grid() {
        let mut manager = KittyViewerManager::new(ratatui_image::FontSize::new(8, 16));
        let commands = manager
            .update_viewer(
                "square",
                &image(),
                ViewerView::new(100, 0, 0),
                Destination::new(0, 0, 80, 20),
            )
            .unwrap();
        let wire = bytes(&commands);
        assert!(wire.contains("\x1b[1;21H"));
        assert!(wire.contains("c=40,r=20"));

        let zoomed = manager
            .update_viewer(
                "square",
                &image(),
                ViewerView::new(200, 0, 0),
                Destination::new(0, 0, 80, 20),
            )
            .unwrap();
        let zoomed_wire = bytes(&zoomed);
        assert!(zoomed_wire.contains("x=0,y=25,w=100,h=50"));
        assert!(zoomed_wire.contains("c=80,r=20"));
    }
}
