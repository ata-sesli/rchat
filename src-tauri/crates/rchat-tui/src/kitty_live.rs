//! Two independent persistent Kitty surfaces for screen sharing and remote video.
use crate::{image_geometry::Destination, kitty_video::KittyVideo, media::DecodedRgbaFrame};
use anyhow::Result;
use ratatui_image::FontSize;

#[derive(Clone, Copy)]
pub enum LiveSurface {
    Screen,
    RemoteVideo,
}

pub struct KittyLiveManager {
    screen: KittyVideo,
    remote_video: KittyVideo,
    screen_session: Option<String>,
    remote_session: Option<String>,
    font_size: FontSize,
}

impl KittyLiveManager {
    pub fn new(font_size: FontSize) -> Self {
        Self {
            screen: KittyVideo::new(0x5243_0001),
            remote_video: KittyVideo::new(0x5243_0002),
            screen_session: None,
            remote_session: None,
            font_size,
        }
    }

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
        let (video, session) = match surface {
            LiveSurface::Screen => (&mut self.screen, &mut self.screen_session),
            LiveSurface::RemoteVideo => (&mut self.remote_video, &mut self.remote_session),
        };
        let mut commands = Vec::new();
        if session.as_deref() != Some(frame.session_id.as_str()) {
            commands.extend(video.clear());
            *session = Some(frame.session_id.clone());
        }
        commands.extend(video.update(&frame.rgba, frame.width, frame.height)?);
        commands.extend(video.place(destination, self.font_size));
        Ok(commands)
    }

    pub fn reconcile_live_placement(
        &mut self,
        surface: LiveSurface,
        destination: Option<Destination>,
    ) -> Result<Vec<Vec<u8>>> {
        let video = match surface {
            LiveSurface::Screen => &mut self.screen,
            LiveSurface::RemoteVideo => &mut self.remote_video,
        };
        Ok(match destination {
            Some(destination) => video.place(destination, self.font_size),
            None => video.hide(),
        })
    }

    pub fn clear_live(&mut self, surface: LiveSurface) -> Vec<Vec<u8>> {
        match surface {
            LiveSurface::Screen => {
                self.screen_session = None;
                self.screen.clear()
            }
            LiveSurface::RemoteVideo => {
                self.remote_session = None;
                self.remote_video.clear()
            }
        }
    }

    // A global placement clear can happen when a modal opens. Preserve pixels,
    // but force active surfaces to place themselves again on the next draw.
    pub fn invalidate_placements(&mut self) {
        self.screen.hide();
        self.remote_video.hide();
    }
}

pub fn cleanup_all_commands() -> Vec<Vec<u8>> {
    [0x5243_0001u32, 0x5243_0002, 0x5243_0003, 0x5243_0020]
        .into_iter()
        .map(|id| format!("\x1b_Ga=d,d=I,i={id},q=2\x1b\\").into_bytes())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(session: &str) -> DecodedRgbaFrame {
        DecodedRgbaFrame {
            session_id: session.into(),
            seq: 1,
            timestamp_us: 0,
            width: 2,
            height: 2,
            rgba: vec![0; 16],
        }
    }

    #[test]
    fn hidden_surface_retains_latest_frame_and_restores_placement() {
        let mut manager = KittyLiveManager::new(FontSize::new(8, 16));
        let mut pending = Some(frame("screen"));
        let destination = Some(Destination::new(2, 3, 20, 10));
        assert!(manager
            .reconcile_pending_live_frame(LiveSurface::Screen, &mut pending, None)
            .unwrap()
            .is_empty());
        assert!(pending.is_some());
        let first = manager
            .reconcile_pending_live_frame(LiveSurface::Screen, &mut pending, destination)
            .unwrap();
        assert!(String::from_utf8(first.concat()).unwrap().contains("a=t,"));
        assert!(pending.is_none());
        manager.invalidate_placements();
        let restore = manager
            .reconcile_live_placement(LiveSurface::Screen, destination)
            .unwrap();
        assert!(String::from_utf8(restore.concat())
            .unwrap()
            .contains("a=p,"));
    }

    #[test]
    fn session_change_replaces_image_but_next_frame_updates_it() {
        let mut manager = KittyLiveManager::new(FontSize::new(8, 16));
        let destination = Some(Destination::new(0, 0, 20, 10));
        manager
            .reconcile_pending_live_frame(LiveSurface::Screen, &mut Some(frame("one")), destination)
            .unwrap();
        let next = manager
            .reconcile_pending_live_frame(LiveSurface::Screen, &mut Some(frame("one")), destination)
            .unwrap();
        assert!(String::from_utf8(next.concat())
            .unwrap()
            .contains("a=f,r=1,X=1,"));
        let changed = manager
            .reconcile_pending_live_frame(LiveSurface::Screen, &mut Some(frame("two")), destination)
            .unwrap();
        let changed = String::from_utf8(changed.concat()).unwrap();
        assert!(changed.contains("a=d,d=I,"));
        assert!(changed.contains("a=t,"));
        assert!(manager.clear_live(LiveSurface::RemoteVideo).is_empty());
    }
}
