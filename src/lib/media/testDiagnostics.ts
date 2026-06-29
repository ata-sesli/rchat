export type MediaSupportSnapshot = {
  mediaDevices: boolean;
  getUserMedia: boolean;
  mediaRecorder: boolean;
};

export type MediaSupportRow = {
  label: string;
  ok: boolean;
};

export function currentMediaSupport(): MediaSupportSnapshot {
  return {
    mediaDevices: typeof navigator !== "undefined" && !!navigator.mediaDevices,
    getUserMedia:
      typeof navigator !== "undefined" &&
      typeof navigator.mediaDevices?.getUserMedia === "function",
    mediaRecorder: typeof MediaRecorder !== "undefined",
  };
}

export function mediaSupportRows(
  support: MediaSupportSnapshot,
): MediaSupportRow[] {
  return [
    { label: "navigator.mediaDevices", ok: support.mediaDevices },
    { label: "getUserMedia", ok: support.getUserMedia },
    { label: "MediaRecorder", ok: support.mediaRecorder },
  ];
}

export function formatAudioLevel(level: number): string {
  const clamped = Math.max(0, Math.min(1, Number.isFinite(level) ? level : 0));
  return `${Math.round(clamped * 100)}%`;
}

export function describeMediaError(error: unknown): string {
  if (typeof error === "object" && error && "name" in error) {
    const name = String((error as { name?: unknown }).name);
    if (name === "NotAllowedError" || name === "PermissionDeniedError") {
      return "Camera or microphone permission was denied.";
    }
    if (name === "NotFoundError" || name === "DevicesNotFoundError") {
      return "No matching camera or microphone device was found.";
    }
    if (name === "NotReadableError" || name === "TrackStartError") {
      return "The camera or microphone is already in use or cannot be opened.";
    }
    if (name === "OverconstrainedError" || name === "ConstraintNotSatisfiedError") {
      return "The selected camera or microphone does not support the requested settings.";
    }
  }

  if (error instanceof Error && error.message) {
    return error.message;
  }

  return String(error || "Unknown media device error");
}
