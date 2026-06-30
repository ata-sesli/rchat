export type ScreenBroadcastResolution = "480p" | "720p";
export type ScreenBroadcastFps = 15 | 30;
export type ScreenBroadcastProfile = "480p15" | "480p30" | "720p15" | "720p30";

export const DEFAULT_SCREEN_BROADCAST_RESOLUTION: ScreenBroadcastResolution = "720p";
export const DEFAULT_SCREEN_BROADCAST_FPS: ScreenBroadcastFps = 15;
export const DEFAULT_SCREEN_BROADCAST_PROFILE: ScreenBroadcastProfile = "720p15";

export function buildScreenBroadcastProfile(
  resolution: ScreenBroadcastResolution,
  fps: ScreenBroadcastFps,
): ScreenBroadcastProfile {
  return `${resolution}${fps}` as ScreenBroadcastProfile;
}
