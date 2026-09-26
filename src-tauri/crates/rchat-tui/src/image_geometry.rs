#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Destination {
    pub row: u16,
    pub col: u16,
    pub width: u32,
    pub height: u32,
}

impl Destination {
    pub const fn new(row: u16, col: u16, width: u32, height: u32) -> Self {
        Self {
            row,
            col,
            width,
            height,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl SourceRect {
    pub const fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewerView {
    pub zoom_percent: u16,
    pub pan_x: i32,
    pub pan_y: i32,
}

impl ViewerView {
    pub const fn new(zoom_percent: u16, pan_x: i32, pan_y: i32) -> Self {
        Self {
            zoom_percent,
            pan_x,
            pan_y,
        }
    }
}

pub(crate) fn viewer_geometry(
    width: u32,
    height: u32,
    view: ViewerView,
    viewport: Destination,
) -> (Destination, SourceRect) {
    let viewport_width = u64::from(viewport.width);
    let viewport_height = u64::from(viewport.height);
    let width_u64 = u64::from(width);
    let height_u64 = u64::from(height);
    let (fitted_width, fitted_height) =
        if width_u64.saturating_mul(viewport_height) >= height_u64.saturating_mul(viewport_width) {
            (
                viewport_width,
                viewport_width
                    .saturating_mul(height_u64)
                    .checked_div(width_u64)
                    .unwrap_or(1)
                    .max(1),
            )
        } else {
            (
                viewport_height
                    .saturating_mul(width_u64)
                    .checked_div(height_u64)
                    .unwrap_or(1)
                    .max(1),
                viewport_height,
            )
        };

    let zoom = u64::from(view.zoom_percent.max(25));
    let scaled_width = fitted_width
        .saturating_mul(zoom)
        .checked_div(100)
        .unwrap_or(1)
        .max(1);
    let scaled_height = fitted_height
        .saturating_mul(zoom)
        .checked_div(100)
        .unwrap_or(1)
        .max(1);
    let destination_width = scaled_width.min(viewport_width) as u32;
    let destination_height = scaled_height.min(viewport_height) as u32;
    let row_offset = viewport.height.saturating_sub(destination_height) / 2;
    let col_offset = viewport.width.saturating_sub(destination_width) / 2;
    let destination = Destination::new(
        viewport
            .row
            .saturating_add(u16::try_from(row_offset).unwrap_or(u16::MAX)),
        viewport
            .col
            .saturating_add(u16::try_from(col_offset).unwrap_or(u16::MAX)),
        destination_width,
        destination_height,
    );

    let (crop_width, crop_height) = if zoom <= 100 {
        (width, height)
    } else {
        let crop_width = width_u64
            .saturating_mul(u64::from(destination_width))
            .saturating_mul(100)
            .checked_div(fitted_width.saturating_mul(zoom))
            .unwrap_or(width_u64)
            .clamp(1, width_u64) as u32;
        let crop_height = height_u64
            .saturating_mul(u64::from(destination_height))
            .saturating_mul(100)
            .checked_div(fitted_height.saturating_mul(zoom))
            .unwrap_or(height_u64)
            .clamp(1, height_u64) as u32;
        (crop_width, crop_height)
    };
    let maximum_x = width.saturating_sub(crop_width);
    let maximum_y = height.saturating_sub(crop_height);
    let centered_x = maximum_x / 2;
    let centered_y = maximum_y / 2;
    let x = i64::from(centered_x)
        .saturating_add(i64::from(view.pan_x))
        .clamp(0, i64::from(maximum_x)) as u32;
    let y = i64::from(centered_y)
        .saturating_add(i64::from(view.pan_y))
        .clamp(0, i64::from(maximum_y)) as u32;
    (destination, SourceRect::new(x, y, crop_width, crop_height))
}
