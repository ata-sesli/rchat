import { describeMediaError } from "$lib/media/testDiagnostics";

export type MediaStreamLike = {
  getTracks(): Array<{ stop(): void }>;
};

export type GetUserMediaLike = (
  constraints: MediaStreamConstraints,
) => Promise<MediaStreamLike>;

export function videoCallCameraConstraints(): MediaStreamConstraints {
  return {
    audio: false,
    video: {
      width: { ideal: 1280 },
      height: { ideal: 720 },
      frameRate: { ideal: 30, max: 30 },
    },
  };
}

function defaultGetUserMedia(): GetUserMediaLike | null {
  if (
    typeof navigator === "undefined" ||
    typeof navigator.mediaDevices?.getUserMedia !== "function"
  ) {
    return null;
  }

  return (constraints) => navigator.mediaDevices.getUserMedia(constraints);
}

export async function ensureVideoCallCameraAccess(
  getUserMedia: GetUserMediaLike | null = defaultGetUserMedia(),
): Promise<void> {
  if (!getUserMedia) {
    throw new Error("Camera capture is unavailable on this device.");
  }

  let stream: MediaStreamLike | null = null;
  try {
    stream = await getUserMedia(videoCallCameraConstraints());
  } catch (e) {
    throw new Error(describeMediaError(e));
  } finally {
    stream?.getTracks().forEach((track) => track.stop());
  }
}
