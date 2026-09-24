use anyhow::{ensure, Context, Result};
use base64::Engine as _;
use image::{DynamicImage, ImageEncoder as _};
use std::time::Duration;

use crate::media::DecodedRgbaFrame;

const APC_PREFIX: &str = "\u{1b}_ratty;i;";
const APC_END: &str = "\u{1b}\\";
const MAX_BASE64_CHUNK: usize = 4096;
const SUPPORT_QUERY: &[u8] = b"\x1b_ratty;i;s\x1b\\";
const DEVICE_STATUS_QUERY: &[u8] = b"\x1b[5n";
const SUPPORT_REPLY: &[u8] = b"\x1b_ratty;i;s;v=1;fmt=png;frame=rgba8;payload=1;chunk=1;placement=1;crop=1;fit=contain|cover|fill;filter=nearest|linear;opacity=1\x1b\\";
const SCREEN_BITMAP_ID: u32 = 0x5243_0001;
const SCREEN_PLACEMENT_ID: u32 = 0x5243_1001;
const REMOTE_VIDEO_BITMAP_ID: u32 = 0x5243_0002;
const REMOTE_VIDEO_PLACEMENT_ID: u32 = 0x5243_1002;
const VIEWER_BITMAP_ID: u32 = 0x5243_0003;
const VIEWER_PLACEMENT_ID: u32 = 0x5243_1003;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RattyBitmapSupport;

impl RattyBitmapSupport {
    pub fn parse(bytes: &[u8]) -> Option<Self> {
        bytes
            .windows(SUPPORT_REPLY.len())
            .any(|window| window == SUPPORT_REPLY)
            .then_some(Self)
    }
}

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
pub enum LiveSurface {
    Screen,
    RemoteVideo,
}

impl LiveSurface {
    const fn ids(self) -> (u32, u32) {
        match self {
            Self::Screen => (SCREEN_BITMAP_ID, SCREEN_PLACEMENT_ID),
            Self::RemoteVideo => (REMOTE_VIDEO_BITMAP_ID, REMOTE_VIDEO_PLACEMENT_ID),
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

#[derive(Debug)]
struct LiveState {
    session_id: String,
    width: u32,
    height: u32,
    destination: Destination,
    placed: bool,
    next_sequence: u32,
}

#[derive(Debug)]
struct ViewerState {
    file_hash: String,
    width: u32,
    height: u32,
    destination: Destination,
    source: SourceRect,
}

#[derive(Debug, Default)]
pub struct RattyBitmapManager {
    screen: Option<LiveState>,
    remote_video: Option<LiveState>,
    viewer: Option<ViewerState>,
}

impl RattyBitmapManager {
    pub fn reconcile_pending_live_frame(
        &mut self,
        surface: LiveSurface,
        pending: &mut Option<DecodedRgbaFrame>,
        destination: Option<Destination>,
    ) -> Result<Vec<Vec<u8>>> {
        let Some(destination) = destination else {
            return Ok(Vec::new());
        };
        let Some(frame) = pending.take() else {
            return Ok(Vec::new());
        };
        self.update_live(surface, &frame, destination)
    }

    pub fn update_live(
        &mut self,
        surface: LiveSurface,
        frame: &DecodedRgbaFrame,
        destination: Destination,
    ) -> Result<Vec<Vec<u8>>> {
        validate_destination(destination)?;
        validate_rgba(frame.width, frame.height, &frame.rgba)?;
        let (bitmap_id, placement_id) = surface.ids();
        let slot = match surface {
            LiveSurface::Screen => &mut self.screen,
            LiveSurface::RemoteVideo => &mut self.remote_video,
        };
        let replace = slot.as_ref().is_some_and(|state| {
            state.session_id != frame.session_id
                || state.width != frame.width
                || state.height != frame.height
        });
        let mut commands = Vec::new();
        if replace {
            commands.extend(encode_delete_surface(bitmap_id, placement_id));
            *slot = None;
        }

        if slot.is_none() {
            let png = encode_rgba_png(frame.width, frame.height, &frame.rgba)?;
            commands.extend(encode_register_png(bitmap_id, &png)?);
            commands.push(encode_place(bitmap_id, placement_id, destination));
            *slot = Some(LiveState {
                session_id: frame.session_id.clone(),
                width: frame.width,
                height: frame.height,
                destination,
                placed: true,
                next_sequence: 1,
            });
            return Ok(commands);
        }

        let state = slot.as_mut().expect("live state was initialized");
        if !state.placed {
            commands.push(encode_place(bitmap_id, placement_id, destination));
            state.placed = true;
            state.destination = destination;
        } else if state.destination != destination {
            commands.push(encode_update_destination(placement_id, destination));
            state.destination = destination;
        }
        let sequence = state.next_sequence;
        state.next_sequence = sequence
            .checked_add(1)
            .context("Ratty bitmap frame sequence exhausted")?;
        commands.extend(encode_frame(
            bitmap_id,
            sequence,
            frame.width,
            frame.height,
            &frame.rgba,
        )?);
        Ok(commands)
    }

    pub fn reconcile_live_placement(
        &mut self,
        surface: LiveSurface,
        destination: Option<Destination>,
    ) -> Result<Vec<Vec<u8>>> {
        if let Some(destination) = destination {
            validate_destination(destination)?;
        }
        let (bitmap_id, placement_id) = surface.ids();
        let slot = match surface {
            LiveSurface::Screen => &mut self.screen,
            LiveSurface::RemoteVideo => &mut self.remote_video,
        };
        let Some(state) = slot.as_mut() else {
            return Ok(Vec::new());
        };
        let commands = match (state.placed, destination) {
            (true, None) => {
                state.placed = false;
                vec![encode_delete_placement(placement_id)]
            }
            (false, Some(destination)) => {
                state.placed = true;
                state.destination = destination;
                vec![encode_place(bitmap_id, placement_id, state.destination)]
            }
            (true, Some(destination)) if state.destination != destination => {
                state.destination = destination;
                vec![encode_update_destination(placement_id, destination)]
            }
            _ => Vec::new(),
        };
        Ok(commands)
    }

    pub fn clear_live(&mut self, surface: LiveSurface) -> Vec<Vec<u8>> {
        let slot = match surface {
            LiveSurface::Screen => &mut self.screen,
            LiveSurface::RemoteVideo => &mut self.remote_video,
        };
        if slot.take().is_some() {
            let (bitmap_id, placement_id) = surface.ids();
            encode_delete_surface(bitmap_id, placement_id).to_vec()
        } else {
            Vec::new()
        }
    }

    pub fn update_viewer(
        &mut self,
        file_hash: &str,
        image: &DynamicImage,
        view: ViewerView,
        destination: Destination,
    ) -> Result<Vec<Vec<u8>>> {
        validate_destination(destination)?;
        ensure!(
            image.width() > 0 && image.height() > 0,
            "viewer image is empty"
        );
        let (destination, source) =
            viewer_geometry(image.width(), image.height(), view, destination);
        let replace = self
            .viewer
            .as_ref()
            .is_some_and(|state| state.file_hash != file_hash);
        let mut commands = Vec::new();
        if replace {
            commands.extend(encode_delete_surface(VIEWER_BITMAP_ID, VIEWER_PLACEMENT_ID));
            self.viewer = None;
        }

        if self.viewer.is_none() {
            let png = encode_dynamic_png(image)?;
            commands.extend(encode_register_png(VIEWER_BITMAP_ID, &png)?);
            commands.push(encode_place(
                VIEWER_BITMAP_ID,
                VIEWER_PLACEMENT_ID,
                destination,
            ));
            if source != SourceRect::new(0, 0, image.width(), image.height()) {
                commands.push(encode_update_source(VIEWER_PLACEMENT_ID, source));
            }
            self.viewer = Some(ViewerState {
                file_hash: file_hash.to_string(),
                width: image.width(),
                height: image.height(),
                destination,
                source,
            });
            return Ok(commands);
        }

        let state = self.viewer.as_mut().expect("viewer state was initialized");
        if state.width != image.width() || state.height != image.height() {
            commands.extend(self.clear_viewer());
            commands.extend(self.update_viewer(file_hash, image, view, destination)?);
            return Ok(commands);
        }
        if state.destination != destination {
            commands.push(encode_update_destination(VIEWER_PLACEMENT_ID, destination));
            state.destination = destination;
        }
        if state.source != source {
            commands.push(encode_update_source(VIEWER_PLACEMENT_ID, source));
            state.source = source;
        }
        Ok(commands)
    }

    pub fn clear_viewer(&mut self) -> Vec<Vec<u8>> {
        if self.viewer.take().is_some() {
            encode_delete_surface(VIEWER_BITMAP_ID, VIEWER_PLACEMENT_ID).to_vec()
        } else {
            Vec::new()
        }
    }

    pub fn clear_all(&mut self) -> Vec<Vec<u8>> {
        let mut commands = self.clear_live(LiveSurface::Screen);
        commands.extend(self.clear_live(LiveSurface::RemoteVideo));
        commands.extend(self.clear_viewer());
        commands
    }
}

fn validate_destination(destination: Destination) -> Result<()> {
    ensure!(
        destination.width > 0 && destination.height > 0,
        "bitmap destination dimensions must be nonzero"
    );
    Ok(())
}

fn validate_rgba(width: u32, height: u32, rgba: &[u8]) -> Result<()> {
    let expected_len = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .and_then(|bytes| usize::try_from(bytes).ok())
        .context("frame dimensions overflow")?;
    ensure!(width > 0 && height > 0, "frame dimensions must be nonzero");
    ensure!(
        rgba.len() == expected_len,
        "RGBA8 frame length does not match its dimensions"
    );
    Ok(())
}

fn viewer_geometry(
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

fn encode_rgba_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>> {
    validate_rgba(width, height, rgba)?;
    let mut png = Vec::new();
    image::codecs::png::PngEncoder::new(&mut png)
        .write_image(rgba, width, height, image::ExtendedColorType::Rgba8)
        .context("failed to encode Ratty bitmap registration PNG")?;
    Ok(png)
}

fn encode_dynamic_png(image: &DynamicImage) -> Result<Vec<u8>> {
    let rgba = image.to_rgba8();
    encode_rgba_png(rgba.width(), rgba.height(), rgba.as_raw())
}

pub const fn support_query() -> &'static [u8] {
    SUPPORT_QUERY
}

#[cfg(unix)]
pub fn probe_support(timeout: Duration) -> Result<Option<RattyBitmapSupport>> {
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    probe_support_io(&mut stdin.lock(), &mut stdout.lock(), timeout)
}

#[cfg(unix)]
fn probe_support_io(
    reader: &mut (impl std::io::Read + std::os::fd::AsRawFd),
    writer: &mut impl std::io::Write,
    timeout: Duration,
) -> Result<Option<RattyBitmapSupport>> {
    use std::time::Instant;

    writer
        .write_all(support_query())
        .context("failed to write Ratty bitmap support query")?;
    writer
        .write_all(DEVICE_STATUS_QUERY)
        .context("failed to write terminal status query after Ratty bitmap probe")?;
    writer
        .flush()
        .context("failed to flush Ratty bitmap support query")?;

    let deadline = Instant::now() + timeout;
    let mut response = Vec::with_capacity(256);
    loop {
        let now = Instant::now();
        if now >= deadline {
            return Ok(None);
        }
        let remaining = deadline.saturating_duration_since(now);
        let timeout_ms = remaining.as_millis().clamp(1, i32::MAX as u128) as i32;
        let mut descriptor = libc::pollfd {
            fd: reader.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        let ready = unsafe { libc::poll(&mut descriptor, 1, timeout_ms) };
        if ready == 0 {
            return Ok(None);
        }
        if ready < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error).context("Ratty bitmap support probe failed");
        }
        if descriptor.revents & libc::POLLIN == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "terminal closed while waiting for Ratty bitmap capability reply",
            ))
            .context("Ratty bitmap support probe transport failed");
        }

        let mut chunk = [0_u8; 512];
        let read = reader
            .read(&mut chunk)
            .context("failed to read Ratty bitmap support reply")?;
        if read == 0 {
            return Ok(None);
        }
        response.extend_from_slice(&chunk[..read]);
        if let Some(support) = RattyBitmapSupport::parse(&response) {
            return Ok(Some(support));
        }
        if response.len() > 4096 {
            return Ok(None);
        }
    }
}

#[cfg(not(unix))]
pub fn probe_support(_timeout: Duration) -> Result<Option<RattyBitmapSupport>> {
    Ok(None)
}

pub fn encode_register_png(bitmap_id: u32, png: &[u8]) -> Result<Vec<Vec<u8>>> {
    ensure!(!png.is_empty(), "bitmap registration PNG is empty");
    encode_payload_chunks(
        &base64::engine::general_purpose::STANDARD.encode(png),
        |more, first, payload| {
            if first {
                format!("r;id={bitmap_id};fmt=png;source=payload;more={more};{payload}")
            } else {
                format!("r;id={bitmap_id};more={more};{payload}")
            }
        },
    )
}

pub fn encode_place(bitmap_id: u32, placement_id: u32, destination: Destination) -> Vec<u8> {
    encode_command(format!(
        "p;id={bitmap_id};pid={placement_id};row={};col={};w={};h={};fit=contain;filter=linear;opacity=1",
        destination.row, destination.col, destination.width, destination.height
    ))
}

pub fn encode_update_destination(placement_id: u32, destination: Destination) -> Vec<u8> {
    encode_command(format!(
        "u;pid={placement_id};row={};col={};w={};h={}",
        destination.row, destination.col, destination.width, destination.height
    ))
}

pub fn encode_update_source(placement_id: u32, source: SourceRect) -> Vec<u8> {
    encode_command(format!(
        "u;pid={placement_id};src_x={};src_y={};src_w={};src_h={}",
        source.x, source.y, source.width, source.height
    ))
}

pub fn encode_frame(
    bitmap_id: u32,
    sequence: u32,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> Result<Vec<Vec<u8>>> {
    ensure!(width > 0 && height > 0, "frame dimensions must be nonzero");
    let expected_len = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(4))
        .and_then(|bytes| usize::try_from(bytes).ok())
        .context("frame dimensions overflow")?;
    ensure!(
        rgba.len() == expected_len,
        "RGBA8 frame length does not match its dimensions"
    );
    encode_payload_chunks(
        &base64::engine::general_purpose::STANDARD.encode(rgba),
        |more, first, payload| {
            if first {
                format!(
                    "f;id={bitmap_id};seq={sequence};fmt=rgba8;w={width};h={height};more={more};{payload}"
                )
            } else {
                format!("f;id={bitmap_id};seq={sequence};more={more};{payload}")
            }
        },
    )
}

pub fn encode_delete_surface(bitmap_id: u32, placement_id: u32) -> [Vec<u8>; 2] {
    [
        encode_command(format!("d;pid={placement_id}")),
        encode_command(format!("d;id={bitmap_id}")),
    ]
}

pub fn encode_delete_placement(placement_id: u32) -> Vec<u8> {
    encode_command(format!("d;pid={placement_id}"))
}

pub fn cleanup_all_commands() -> Vec<Vec<u8>> {
    let mut commands = encode_delete_surface(SCREEN_BITMAP_ID, SCREEN_PLACEMENT_ID).to_vec();
    commands.extend(encode_delete_surface(
        REMOTE_VIDEO_BITMAP_ID,
        REMOTE_VIDEO_PLACEMENT_ID,
    ));
    commands.extend(encode_delete_surface(VIEWER_BITMAP_ID, VIEWER_PLACEMENT_ID));
    commands
}

fn encode_payload_chunks(
    encoded: &str,
    body: impl Fn(u8, bool, &str) -> String,
) -> Result<Vec<Vec<u8>>> {
    ensure!(!encoded.is_empty(), "bitmap payload is empty");
    debug_assert_eq!(MAX_BASE64_CHUNK % 4, 0);
    let chunks = encoded
        .as_bytes()
        .chunks(MAX_BASE64_CHUNK)
        .collect::<Vec<_>>();
    let last = chunks.len().saturating_sub(1);
    chunks
        .into_iter()
        .enumerate()
        .map(|(index, payload)| {
            let payload = std::str::from_utf8(payload).context("base64 payload is not ASCII")?;
            Ok(encode_command(body(
                u8::from(index != last),
                index == 0,
                payload,
            )))
        })
        .collect()
}

fn encode_command(body: String) -> Vec<u8> {
    format!("{APC_PREFIX}{body}{APC_END}").into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::DecodedRgbaFrame;
    use image::{DynamicImage, RgbaImage};

    fn frame(session_id: &str, seq: u32, width: u32, height: u32) -> DecodedRgbaFrame {
        DecodedRgbaFrame {
            session_id: session_id.to_string(),
            seq,
            timestamp_us: 0,
            width,
            height,
            rgba: vec![seq as u8; (width * height * 4) as usize],
        }
    }

    fn joined(commands: &[Vec<u8>]) -> String {
        String::from_utf8(commands.concat()).unwrap()
    }

    #[test]
    fn parses_only_complete_v1_support_reply() {
        assert!(RattyBitmapSupport::parse(SUPPORT_REPLY).is_some());
        assert!(RattyBitmapSupport::parse(b"\x1b_ratty;i;s;v=2\x1b\\").is_none());
        assert!(
            RattyBitmapSupport::parse(b"\x1b_ratty;i;s;v=1;fmt=png;frame=rgba8\x1b\\").is_none()
        );
    }

    #[test]
    fn extracts_support_reply_from_surrounding_terminal_bytes() {
        let mut response = b"noise-before".to_vec();
        response.extend_from_slice(SUPPORT_REPLY);
        response.extend_from_slice(b"noise-after");
        assert!(RattyBitmapSupport::parse(&response).is_some());
    }

    #[test]
    fn encodes_support_query_exactly() {
        assert_eq!(support_query(), b"\x1b_ratty;i;s\x1b\\");
    }

    #[cfg(unix)]
    #[test]
    fn support_probe_uses_the_supplied_stdio_transport() {
        use std::{
            io::{Read, Write},
            os::unix::net::UnixStream,
            thread,
        };

        let (client, mut terminal) = UnixStream::pair().unwrap();
        let mut reader = client.try_clone().unwrap();
        let mut writer = client;
        let responder = thread::spawn(move || {
            let mut query = vec![0; SUPPORT_QUERY.len() + DEVICE_STATUS_QUERY.len()];
            terminal.read_exact(&mut query).unwrap();
            assert_eq!(query, [SUPPORT_QUERY, DEVICE_STATUS_QUERY].concat());
            terminal.write_all(SUPPORT_REPLY).unwrap();
        });

        assert!(probe_support_io(&mut reader, &mut writer, Duration::from_secs(1))
            .unwrap()
            .is_some());
        responder.join().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn delayed_support_reply_is_rejected_by_legacy_deadline_and_accepted_by_session_deadline() {
        use std::{
            io::{Read, Write},
            os::unix::net::UnixStream,
            thread,
            time::Duration as StdDuration,
        };

        fn delayed_probe(timeout: Duration) -> Option<RattyBitmapSupport> {
            let (client, mut terminal) = UnixStream::pair().unwrap();
            let mut reader = client.try_clone().unwrap();
            let mut writer = client;
            let responder = thread::spawn(move || {
                let mut query = vec![0; SUPPORT_QUERY.len() + DEVICE_STATUS_QUERY.len()];
                terminal.read_exact(&mut query).unwrap();
                thread::sleep(StdDuration::from_millis(750));
                terminal.write_all(SUPPORT_REPLY).unwrap();
            });

            let result = probe_support_io(&mut reader, &mut writer, timeout).unwrap();
            responder.join().unwrap();
            result
        }

        assert!(delayed_probe(Duration::from_millis(500)).is_none());
        assert!(delayed_probe(Duration::from_secs(3)).is_some());
    }

    #[test]
    fn chunks_png_registration_on_base64_boundaries() {
        let png = vec![7_u8; 4_000];
        let commands = encode_register_png(42, &png).unwrap();
        assert!(commands.len() > 1);
        assert!(commands[0].starts_with(b"\x1b_ratty;i;r;id=42;fmt=png;source=payload;more=1;"));
        assert!(commands
            .last()
            .unwrap()
            .starts_with(b"\x1b_ratty;i;r;id=42;more=0;"));
        assert!(commands.iter().all(|command| command.ends_with(b"\x1b\\")));
    }

    #[test]
    fn validates_and_chunks_rgba_frame_updates() {
        let rgba = vec![255_u8; 64 * 32 * 4];
        let commands = encode_frame(9, 3, 64, 32, &rgba).unwrap();
        assert!(commands.len() > 1);
        assert!(commands[0].starts_with(b"\x1b_ratty;i;f;id=9;seq=3;fmt=rgba8;w=64;h=32;more=1;"));
        assert!(encode_frame(9, 4, 64, 32, &rgba[..rgba.len() - 1]).is_err());
    }

    #[test]
    fn encodes_placement_update_and_cleanup() {
        let destination = Destination::new(4, 2, 80, 30);
        assert_eq!(
            encode_place(42, 7, destination),
            b"\x1b_ratty;i;p;id=42;pid=7;row=4;col=2;w=80;h=30;fit=contain;filter=linear;opacity=1\x1b\\"
        );
        assert_eq!(
            encode_update_source(7, SourceRect::new(300, 120, 900, 600)),
            b"\x1b_ratty;i;u;pid=7;src_x=300;src_y=120;src_w=900;src_h=600\x1b\\"
        );
        assert_eq!(
            encode_delete_surface(42, 7),
            [
                b"\x1b_ratty;i;d;pid=7\x1b\\".to_vec(),
                b"\x1b_ratty;i;d;id=42\x1b\\".to_vec(),
            ]
        );
    }

    #[test]
    fn live_surface_registers_once_then_replaces_frames() {
        let mut manager = RattyBitmapManager::default();
        let destination = Destination::new(2, 40, 80, 24);

        let first = manager
            .update_live(
                LiveSurface::Screen,
                &frame("share-1", 99, 16, 8),
                destination,
            )
            .unwrap();
        let second = manager
            .update_live(
                LiveSurface::Screen,
                &frame("share-1", 1, 16, 8),
                destination,
            )
            .unwrap();

        let first = joined(&first);
        assert!(first.contains(";r;id="));
        assert!(first.contains(";p;id="));
        assert!(!first.contains(";f;id="));
        let second = joined(&second);
        assert!(!second.contains(";r;id="));
        assert!(second.contains(";f;id="));
        assert!(second.contains(";seq=1;"));
    }

    #[test]
    fn live_surface_recreates_on_session_or_dimension_change() {
        let mut manager = RattyBitmapManager::default();
        let destination = Destination::new(2, 40, 80, 24);
        manager
            .update_live(
                LiveSurface::RemoteVideo,
                &frame("call-1", 0, 16, 8),
                destination,
            )
            .unwrap();

        let changed = manager
            .update_live(
                LiveSurface::RemoteVideo,
                &frame("call-2", 0, 32, 18),
                destination,
            )
            .unwrap();
        let changed = joined(&changed);
        assert!(changed.contains(";d;pid="));
        assert!(changed.contains(";d;id="));
        assert!(changed.contains(";r;id="));
        assert!(changed.contains(";p;id="));
    }

    #[test]
    fn live_resize_updates_placement_without_replacing_registration() {
        let mut manager = RattyBitmapManager::default();
        manager
            .update_live(
                LiveSurface::Screen,
                &frame("share-1", 0, 16, 8),
                Destination::new(2, 40, 80, 24),
            )
            .unwrap();
        let resized = manager
            .update_live(
                LiveSurface::Screen,
                &frame("share-1", 1, 16, 8),
                Destination::new(3, 35, 70, 20),
            )
            .unwrap();
        let resized = joined(&resized);
        assert!(resized.contains(";u;pid="));
        assert!(resized.contains(";row=3;col=35;w=70;h=20"));
        assert!(!resized.contains(";r;id="));
    }

    #[test]
    fn live_placement_can_hide_restore_and_resize_without_a_frame() {
        let mut manager = RattyBitmapManager::default();
        manager
            .update_live(
                LiveSurface::Screen,
                &frame("share-1", 0, 16, 8),
                Destination::new(2, 40, 80, 24),
            )
            .unwrap();

        let hidden = manager
            .reconcile_live_placement(LiveSurface::Screen, None)
            .unwrap();
        let restored = manager
            .reconcile_live_placement(LiveSurface::Screen, Some(Destination::new(3, 35, 70, 20)))
            .unwrap();

        assert!(joined(&hidden).contains(";d;pid="));
        let restored = joined(&restored);
        assert!(restored.contains(";p;id="));
        assert!(restored.contains(";row=3;col=35;w=70;h=20"));
        assert!(!restored.contains(";r;id="));
    }

    #[test]
    fn decoded_live_frame_waits_until_current_draw_has_a_destination() {
        let mut manager = RattyBitmapManager::default();
        let mut pending = Some(frame("share-1", 0, 16, 8));

        let before_draw = manager
            .reconcile_pending_live_frame(LiveSurface::Screen, &mut pending, None)
            .unwrap();
        assert!(before_draw.is_empty());
        assert!(pending.is_some());

        let after_draw = manager
            .reconcile_pending_live_frame(
                LiveSurface::Screen,
                &mut pending,
                Some(Destination::new(2, 40, 80, 24)),
            )
            .unwrap();
        assert!(joined(&after_draw).contains(";r;id="));
        assert!(pending.is_none());
    }

    #[test]
    fn viewer_registers_once_and_pan_zoom_only_updates_source() {
        let image = DynamicImage::ImageRgba8(RgbaImage::new(800, 600));
        let mut manager = RattyBitmapManager::default();
        let destination = Destination::new(1, 1, 80, 30);

        let first = manager
            .update_viewer("hash-1", &image, ViewerView::new(100, 0, 0), destination)
            .unwrap();
        let changed = manager
            .update_viewer("hash-1", &image, ViewerView::new(200, 40, 20), destination)
            .unwrap();

        assert!(joined(&first).contains(";r;id="));
        let changed = joined(&changed);
        assert!(changed.contains(";u;pid="));
        assert!(changed.contains(";src_x="));
        assert!(!changed.contains(";r;id="));
    }

    #[test]
    fn viewer_zoom_uses_fitted_destination_and_viewport_shaped_crop() {
        let image = DynamicImage::ImageRgba8(RgbaImage::new(800, 600));
        let mut manager = RattyBitmapManager::default();
        let viewport = Destination::new(0, 0, 160, 90);

        let zoomed_out = manager
            .update_viewer("hash-1", &image, ViewerView::new(50, 0, 0), viewport)
            .unwrap();
        let fitted = manager
            .update_viewer("hash-1", &image, ViewerView::new(100, 0, 0), viewport)
            .unwrap();
        let zoomed_in = manager
            .update_viewer("hash-1", &image, ViewerView::new(125, 0, 0), viewport)
            .unwrap();

        assert!(joined(&zoomed_out).contains(";row=22;col=50;w=60;h=45;"));
        assert!(joined(&fitted).contains(";row=0;col=20;w=120;h=90"));
        let zoomed_in = joined(&zoomed_in);
        assert!(zoomed_in.contains(";row=0;col=5;w=150;h=90"));
        assert!(zoomed_in.contains(";src_x=0;src_y=60;src_w=800;src_h=480"));
    }

    #[test]
    fn viewer_replacement_and_cleanup_delete_old_surface() {
        let image = DynamicImage::ImageRgba8(RgbaImage::new(8, 8));
        let mut manager = RattyBitmapManager::default();
        let destination = Destination::new(1, 1, 20, 10);
        manager
            .update_viewer("hash-1", &image, ViewerView::new(100, 0, 0), destination)
            .unwrap();

        let replaced = manager
            .update_viewer("hash-2", &image, ViewerView::new(100, 0, 0), destination)
            .unwrap();
        let cleared = manager.clear_viewer();

        assert!(joined(&replaced).contains(";d;pid="));
        assert!(joined(&replaced).contains(";r;id="));
        assert!(joined(&cleared).contains(";d;id="));
        assert!(manager.clear_viewer().is_empty());
    }
}
