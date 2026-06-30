use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const SCREEN_CAPTURE_FPS: u32 = 15;
pub const PREVIEW_MAX_WIDTH: u32 = 320;
pub const PREVIEW_INTERVAL: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenCaptureProfile {
    P720,
}

impl ScreenCaptureProfile {
    pub fn dimensions(self) -> (u32, u32) {
        match self {
            Self::P720 => (1280, 720),
        }
    }

    pub fn fps(self) -> u32 {
        SCREEN_CAPTURE_FPS
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::P720 => "720p15",
        }
    }
}

impl Default for ScreenCaptureProfile {
    fn default() -> Self {
        Self::P720
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenCaptureCursorMode {
    Hidden,
    Embedded,
}

impl Default for ScreenCaptureCursorMode {
    fn default() -> Self {
        Self::Embedded
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenCaptureBackend {
    MacosScreenCaptureKit,
    LinuxPortalPipeWire,
    Unsupported,
}

impl ScreenCaptureBackend {
    pub fn label(self) -> &'static str {
        match self {
            Self::MacosScreenCaptureKit => "screencapturekit",
            Self::LinuxPortalPipeWire => "portal-pipewire",
            Self::Unsupported => "unsupported",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenCaptureConfig {
    pub profile: ScreenCaptureProfile,
    pub cursor_mode: ScreenCaptureCursorMode,
}

impl ScreenCaptureConfig {
    pub fn default_for_profile(profile: ScreenCaptureProfile) -> Self {
        Self {
            profile,
            cursor_mode: ScreenCaptureCursorMode::Embedded,
        }
    }
}

impl Default for ScreenCaptureConfig {
    fn default() -> Self {
        Self::default_for_profile(ScreenCaptureProfile::default())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenCaptureSupport {
    pub supported: bool,
    pub reason: Option<String>,
    pub backend: ScreenCaptureBackend,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenCaptureFormatInfo {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub format: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenCaptureSessionInfo {
    pub backend: ScreenCaptureBackend,
    pub source_label: String,
    pub requested_profile: String,
    pub format: ScreenCaptureFormatInfo,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ScreenCaptureSessionStats {
    pub captured_frames: u64,
    pub dropped_i420_frames: u64,
    pub dropped_preview_frames: u64,
    pub conversion_errors: u64,
    pub preview_frames: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct I420ScreenFrame {
    pub timestamp_us: i64,
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewFrame {
    pub timestamp_us: i64,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Debug, thiserror::Error)]
pub enum ScreenCaptureError {
    #[error("native screen capture is unavailable on this platform")]
    UnsupportedPlatform,
    #[error("screen capture permission denied or source unavailable: {0}")]
    PermissionOrSourceUnavailable(String),
    #[error("screen capture was cancelled")]
    Cancelled,
    #[error("screen capture format unsupported: {0}")]
    UnsupportedFormat(String),
    #[error("screen capture frame conversion failed: {0}")]
    Conversion(String),
    #[error("screen capture backend error: {0}")]
    Backend(String),
}

pub async fn screen_capture_support() -> ScreenCaptureSupport {
    platform::screen_capture_support().await
}

pub struct ScreenCaptureSession {
    inner: platform::PlatformScreenCaptureSession,
}

impl ScreenCaptureSession {
    pub async fn start(config: ScreenCaptureConfig) -> Result<Self, ScreenCaptureError> {
        platform::start_session(config)
            .await
            .map(|inner| Self { inner })
    }

    pub fn info(&self) -> &ScreenCaptureSessionInfo {
        self.inner.info()
    }

    pub fn try_recv_latest_i420(&mut self) -> Option<I420ScreenFrame> {
        self.inner.try_recv_latest_i420()
    }

    pub fn try_recv_latest_preview(&mut self) -> Option<PreviewFrame> {
        self.inner.try_recv_latest_preview()
    }

    pub fn stats(&self) -> ScreenCaptureSessionStats {
        self.inner.stats()
    }
}

#[derive(Clone)]
struct LatestSlot<T> {
    inner: Arc<Mutex<Option<T>>>,
    drops: Arc<AtomicU64>,
}

impl<T> Default for LatestSlot<T> {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(None)),
            drops: Arc::new(AtomicU64::new(0)),
        }
    }
}

impl<T> LatestSlot<T> {
    fn replace(&self, value: T) {
        if let Ok(mut slot) = self.inner.lock() {
            if slot.is_some() {
                self.drops.fetch_add(1, Ordering::Relaxed);
            }
            *slot = Some(value);
        }
    }

    fn take(&self) -> Option<T> {
        self.inner.lock().ok()?.take()
    }

    fn dropped(&self) -> u64 {
        self.drops.load(Ordering::Relaxed)
    }
}

#[derive(Default)]
struct CaptureStatsAtomic {
    captured_frames: AtomicU64,
    conversion_errors: AtomicU64,
    preview_frames: AtomicU64,
}

impl CaptureStatsAtomic {
    fn snapshot(&self, i420_drops: u64, preview_drops: u64) -> ScreenCaptureSessionStats {
        ScreenCaptureSessionStats {
            captured_frames: self.captured_frames.load(Ordering::Relaxed),
            dropped_i420_frames: i420_drops,
            dropped_preview_frames: preview_drops,
            conversion_errors: self.conversion_errors.load(Ordering::Relaxed),
            preview_frames: self.preview_frames.load(Ordering::Relaxed),
        }
    }
}

fn now_timestamp_us() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros()
        .min(i64::MAX as u128) as i64
}

fn validate_even_dimensions(width: u32, height: u32) -> Result<(), ScreenCaptureError> {
    if width == 0 || height == 0 || width % 2 != 0 || height % 2 != 0 {
        return Err(ScreenCaptureError::Conversion(format!(
            "invalid even frame dimensions: {}x{}",
            width, height
        )));
    }
    Ok(())
}

fn expected_i420_len(width: u32, height: u32) -> Result<usize, ScreenCaptureError> {
    validate_even_dimensions(width, height)?;
    let pixels = width
        .checked_mul(height)
        .ok_or_else(|| ScreenCaptureError::Conversion("frame dimensions overflow".to_string()))?
        as usize;
    pixels
        .checked_add(pixels / 2)
        .ok_or_else(|| ScreenCaptureError::Conversion("I420 frame length overflow".to_string()))
}

fn clamp_to_profile(width: u32, height: u32, profile: ScreenCaptureProfile) -> (u32, u32) {
    let (max_w, max_h) = profile.dimensions();
    if width <= max_w && height <= max_h {
        return (width & !1, height & !1);
    }
    let scale = (max_w as f64 / width as f64).min(max_h as f64 / height as f64);
    let out_w = ((width as f64 * scale).floor() as u32).max(2) & !1;
    let out_h = ((height as f64 * scale).floor() as u32).max(2) & !1;
    (out_w, out_h)
}

fn scale_i420_nearest(
    src: &[u8],
    src_width: u32,
    src_height: u32,
    dst_width: u32,
    dst_height: u32,
) -> Result<Vec<u8>, ScreenCaptureError> {
    let src_len = expected_i420_len(src_width, src_height)?;
    let dst_len = expected_i420_len(dst_width, dst_height)?;
    if src.len() != src_len {
        return Err(ScreenCaptureError::Conversion(
            "I420 scale input length mismatch".to_string(),
        ));
    }
    let sw = src_width as usize;
    let sh = src_height as usize;
    let dw = dst_width as usize;
    let dh = dst_height as usize;
    let src_y_len = sw * sh;
    let src_uv_w = sw / 2;
    let dst_y_len = dw * dh;
    let dst_uv_w = dw / 2;
    let mut out = vec![0_u8; dst_len];

    for y in 0..dh {
        let sy = y * sh / dh;
        for x in 0..dw {
            let sx = x * sw / dw;
            out[y * dw + x] = src[sy * sw + sx];
        }
    }
    for y in 0..dh / 2 {
        let sy = y * (sh / 2) / (dh / 2);
        for x in 0..dw / 2 {
            let sx = x * (sw / 2) / (dw / 2);
            out[dst_y_len + y * dst_uv_w + x] = src[src_y_len + sy * src_uv_w + sx];
            out[dst_y_len + dst_y_len / 4 + y * dst_uv_w + x] =
                src[src_y_len + src_y_len / 4 + sy * src_uv_w + sx];
        }
    }

    Ok(out)
}

fn bgra_to_i420(
    src: &[u8],
    width: u32,
    height: u32,
    stride: usize,
) -> Result<Vec<u8>, ScreenCaptureError> {
    rgba_like_to_i420(src, width, height, stride, RgbaLayout::Bgra)
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn rgba_to_i420(
    src: &[u8],
    width: u32,
    height: u32,
    stride: usize,
) -> Result<Vec<u8>, ScreenCaptureError> {
    rgba_like_to_i420(src, width, height, stride, RgbaLayout::Rgba)
}

#[derive(Debug, Clone, Copy)]
enum RgbaLayout {
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    Rgba,
    Bgra,
}

fn rgba_like_to_i420(
    src: &[u8],
    width: u32,
    height: u32,
    stride: usize,
    layout: RgbaLayout,
) -> Result<Vec<u8>, ScreenCaptureError> {
    validate_even_dimensions(width, height)?;
    let width_usize = width as usize;
    let height_usize = height as usize;
    if stride < width_usize * 4 || src.len() < stride * height_usize {
        return Err(ScreenCaptureError::Conversion(
            "RGBA/BGRA buffer too small".to_string(),
        ));
    }

    let mut out = vec![0_u8; expected_i420_len(width, height)?];
    let y_len = width_usize * height_usize;
    let uv_width = width_usize / 2;
    let u_offset = y_len;
    let v_offset = y_len + y_len / 4;

    for y in 0..height_usize {
        for x in 0..width_usize {
            let px = &src[y * stride + x * 4..][..4];
            let (r, g, b) = match layout {
                RgbaLayout::Rgba => (px[0], px[1], px[2]),
                RgbaLayout::Bgra => (px[2], px[1], px[0]),
            };
            out[y * width_usize + x] = rgb_to_y(r, g, b);
        }
    }

    for y in (0..height_usize).step_by(2) {
        for x in (0..width_usize).step_by(2) {
            let mut r_sum = 0_u16;
            let mut g_sum = 0_u16;
            let mut b_sum = 0_u16;
            for dy in 0..2 {
                for dx in 0..2 {
                    let px = &src[(y + dy) * stride + (x + dx) * 4..][..4];
                    let (r, g, b) = match layout {
                        RgbaLayout::Rgba => (px[0], px[1], px[2]),
                        RgbaLayout::Bgra => (px[2], px[1], px[0]),
                    };
                    r_sum += r as u16;
                    g_sum += g as u16;
                    b_sum += b as u16;
                }
            }
            let r = (r_sum / 4) as u8;
            let g = (g_sum / 4) as u8;
            let b = (b_sum / 4) as u8;
            let uv_index = (y / 2) * uv_width + (x / 2);
            out[u_offset + uv_index] = rgb_to_u(r, g, b);
            out[v_offset + uv_index] = rgb_to_v(r, g, b);
        }
    }

    Ok(out)
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn nv12_to_i420(
    y_plane: &[u8],
    y_stride: usize,
    uv_plane: &[u8],
    uv_stride: usize,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, ScreenCaptureError> {
    validate_even_dimensions(width, height)?;
    let width_usize = width as usize;
    let height_usize = height as usize;
    if y_stride < width_usize
        || uv_stride < width_usize
        || y_plane.len() < y_stride * height_usize
        || uv_plane.len() < uv_stride * (height_usize / 2)
    {
        return Err(ScreenCaptureError::Conversion(
            "NV12 buffer too small".to_string(),
        ));
    }

    let mut out = vec![0_u8; expected_i420_len(width, height)?];
    let y_len = width_usize * height_usize;
    let u_offset = y_len;
    let v_offset = y_len + y_len / 4;
    let uv_width = width_usize / 2;

    for row in 0..height_usize {
        let src_row = &y_plane[row * y_stride..row * y_stride + width_usize];
        let dst_row = &mut out[row * width_usize..(row + 1) * width_usize];
        dst_row.copy_from_slice(src_row);
    }

    for row in 0..height_usize / 2 {
        for col in 0..uv_width {
            let src = row * uv_stride + col * 2;
            let dst = row * uv_width + col;
            out[u_offset + dst] = uv_plane[src];
            out[v_offset + dst] = uv_plane[src + 1];
        }
    }

    Ok(out)
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn yuyv_to_i420(
    src: &[u8],
    width: u32,
    height: u32,
    stride: usize,
) -> Result<Vec<u8>, ScreenCaptureError> {
    validate_even_dimensions(width, height)?;
    let width_usize = width as usize;
    let height_usize = height as usize;
    if stride < width_usize * 2 || src.len() < stride * height_usize {
        return Err(ScreenCaptureError::Conversion(
            "YUYV buffer too small".to_string(),
        ));
    }

    let mut out = vec![0_u8; expected_i420_len(width, height)?];
    let y_len = width_usize * height_usize;
    let u_offset = y_len;
    let v_offset = y_len + y_len / 4;
    let uv_width = width_usize / 2;

    for row in 0..height_usize {
        for col in (0..width_usize).step_by(2) {
            let offset = row * stride + col * 2;
            out[row * width_usize + col] = src[offset];
            out[row * width_usize + col + 1] = src[offset + 2];
        }
    }

    for row in (0..height_usize).step_by(2) {
        for col in (0..width_usize).step_by(2) {
            let top = row * stride + col * 2;
            let bottom = (row + 1) * stride + col * 2;
            let u = ((src[top + 1] as u16 + src[bottom + 1] as u16) / 2) as u8;
            let v = ((src[top + 3] as u16 + src[bottom + 3] as u16) / 2) as u8;
            let uv_index = (row / 2) * uv_width + (col / 2);
            out[u_offset + uv_index] = u;
            out[v_offset + uv_index] = v;
        }
    }

    Ok(out)
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn i420_copy(
    y_plane: &[u8],
    y_stride: usize,
    u_plane: &[u8],
    u_stride: usize,
    v_plane: &[u8],
    v_stride: usize,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, ScreenCaptureError> {
    validate_even_dimensions(width, height)?;
    let width_usize = width as usize;
    let height_usize = height as usize;
    let uv_width = width_usize / 2;
    let uv_height = height_usize / 2;
    if y_stride < width_usize
        || u_stride < uv_width
        || v_stride < uv_width
        || y_plane.len() < y_stride * height_usize
        || u_plane.len() < u_stride * uv_height
        || v_plane.len() < v_stride * uv_height
    {
        return Err(ScreenCaptureError::Conversion(
            "I420 buffer too small".to_string(),
        ));
    }

    let mut out = vec![0_u8; expected_i420_len(width, height)?];
    let y_len = width_usize * height_usize;
    let u_len = y_len / 4;

    for row in 0..height_usize {
        out[row * width_usize..(row + 1) * width_usize]
            .copy_from_slice(&y_plane[row * y_stride..row * y_stride + width_usize]);
    }
    for row in 0..uv_height {
        out[y_len + row * uv_width..y_len + (row + 1) * uv_width]
            .copy_from_slice(&u_plane[row * u_stride..row * u_stride + uv_width]);
        out[y_len + u_len + row * uv_width..y_len + u_len + (row + 1) * uv_width]
            .copy_from_slice(&v_plane[row * v_stride..row * v_stride + uv_width]);
    }

    Ok(out)
}

fn i420_to_preview_rgba(
    data: &[u8],
    width: u32,
    height: u32,
) -> Result<PreviewFrame, ScreenCaptureError> {
    let (preview_width, preview_height) = preview_dimensions(width, height);
    let width_usize = width as usize;
    let height_usize = height as usize;
    if data.len() != expected_i420_len(width, height)? {
        return Err(ScreenCaptureError::Conversion(
            "I420 preview input length mismatch".to_string(),
        ));
    }

    let y_len = width_usize * height_usize;
    let uv_width = width_usize / 2;
    let u_offset = y_len;
    let v_offset = y_len + y_len / 4;
    let mut rgba = vec![0_u8; (preview_width as usize) * (preview_height as usize) * 4];
    for py in 0..preview_height as usize {
        let sy = py * height_usize / preview_height as usize;
        for px in 0..preview_width as usize {
            let sx = px * width_usize / preview_width as usize;
            let y_value = data[sy * width_usize + sx];
            let uv_index = (sy / 2) * uv_width + (sx / 2);
            let u = data[u_offset + uv_index];
            let v = data[v_offset + uv_index];
            let (r, g, b) = yuv_to_rgb(y_value, u, v);
            let dst = (py * preview_width as usize + px) * 4;
            rgba[dst] = r;
            rgba[dst + 1] = g;
            rgba[dst + 2] = b;
            rgba[dst + 3] = 255;
        }
    }

    Ok(PreviewFrame {
        timestamp_us: now_timestamp_us(),
        width: preview_width,
        height: preview_height,
        rgba,
    })
}

fn preview_dimensions(width: u32, height: u32) -> (u32, u32) {
    if width <= PREVIEW_MAX_WIDTH {
        return (width, height);
    }
    let preview_width = PREVIEW_MAX_WIDTH & !1;
    let preview_height =
        (((height as f64) * (preview_width as f64 / width as f64)).round() as u32).max(2) & !1;
    (preview_width, preview_height)
}

fn rgb_to_y(r: u8, g: u8, b: u8) -> u8 {
    clamp_u8(((66 * r as i32 + 129 * g as i32 + 25 * b as i32 + 128) >> 8) + 16)
}

fn rgb_to_u(r: u8, g: u8, b: u8) -> u8 {
    clamp_u8(((-38 * r as i32 - 74 * g as i32 + 112 * b as i32 + 128) >> 8) + 128)
}

fn rgb_to_v(r: u8, g: u8, b: u8) -> u8 {
    clamp_u8(((112 * r as i32 - 94 * g as i32 - 18 * b as i32 + 128) >> 8) + 128)
}

fn yuv_to_rgb(y: u8, u: u8, v: u8) -> (u8, u8, u8) {
    let c = y as i32 - 16;
    let d = u as i32 - 128;
    let e = v as i32 - 128;
    (
        clamp_u8((298 * c + 409 * e + 128) >> 8),
        clamp_u8((298 * c - 100 * d - 208 * e + 128) >> 8),
        clamp_u8((298 * c + 516 * d + 128) >> 8),
    )
}

fn clamp_u8(value: i32) -> u8 {
    value.clamp(0, 255) as u8
}

#[cfg(target_os = "macos")]
#[path = "platform/macos.rs"]
mod platform;
#[cfg(target_os = "linux")]
#[path = "platform/linux.rs"]
mod platform;
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
#[path = "platform/unsupported.rs"]
mod platform;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_is_720p15_picker_with_embedded_cursor() {
        let config = ScreenCaptureConfig::default();
        assert_eq!(config.profile, ScreenCaptureProfile::P720);
        assert_eq!(config.profile.dimensions(), (1280, 720));
        assert_eq!(config.profile.fps(), 15);
        assert_eq!(config.cursor_mode, ScreenCaptureCursorMode::Embedded);
    }

    #[test]
    fn rejects_odd_or_empty_dimensions() {
        assert!(expected_i420_len(0, 2).is_err());
        assert!(expected_i420_len(2, 0).is_err());
        assert!(expected_i420_len(3, 2).is_err());
        assert!(expected_i420_len(2, 3).is_err());
        assert_eq!(expected_i420_len(2, 2).unwrap(), 6);
    }

    #[test]
    fn nv12_to_i420_deinterleaves_chroma() {
        let y = [10, 11, 12, 13, 14, 15, 16, 17];
        let uv = [21, 31, 22, 32];
        let out = nv12_to_i420(&y, 4, &uv, 4, 4, 2).unwrap();
        assert_eq!(&out[..8], &y);
        assert_eq!(&out[8..10], &[21, 22]);
        assert_eq!(&out[10..12], &[31, 32]);
    }

    #[test]
    fn yuyv_to_i420_2x2_matches_expected_planes() {
        let row0 = [10, 20, 11, 30];
        let row1 = [12, 24, 13, 34];
        let src = [row0, row1].concat();
        let out = yuyv_to_i420(&src, 2, 2, 4).unwrap();
        assert_eq!(&out[..4], &[10, 11, 12, 13]);
        assert_eq!(out[4], 22);
        assert_eq!(out[5], 32);
    }

    #[test]
    fn rgba_or_bgra_to_i420_smoke_test() {
        let rgba = [
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
        ];
        let bgra = [
            0, 0, 255, 255, 0, 255, 0, 255, 255, 0, 0, 255, 255, 255, 255, 255,
        ];
        let rgba_out = rgba_to_i420(&rgba, 2, 2, 8).unwrap();
        let bgra_out = bgra_to_i420(&bgra, 2, 2, 8).unwrap();
        assert_eq!(rgba_out, bgra_out);
        assert_eq!(rgba_out.len(), 6);
    }

    #[test]
    fn latest_frame_queue_drops_stale_frames() {
        let slot = LatestSlot::default();
        slot.replace(1);
        slot.replace(2);
        slot.replace(3);
        assert_eq!(slot.dropped(), 2);
        assert_eq!(slot.take(), Some(3));
        assert_eq!(slot.take(), None);
    }

    #[test]
    fn i420_copy_respects_strides() {
        let y = [1, 2, 9, 3, 4, 9];
        let u = [5, 9];
        let v = [6, 9];
        let out = i420_copy(&y, 3, &u, 2, &v, 2, 2, 2).unwrap();
        assert_eq!(out, vec![1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn scale_i420_nearest_downscales_even_frame() {
        let y: Vec<u8> = (0..16).collect();
        let u = vec![100, 101, 102, 103];
        let v = vec![200, 201, 202, 203];
        let mut src = y;
        src.extend(u);
        src.extend(v);

        let out = scale_i420_nearest(&src, 4, 4, 2, 2).unwrap();

        assert_eq!(out, vec![0, 2, 8, 10, 100, 200]);
    }
}
