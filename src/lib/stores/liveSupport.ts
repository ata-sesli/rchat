export function screenBroadcastCapabilitiesFromChecks({
  decodeSupported,
  decodeReason,
  captureSupported,
  captureReason,
}: {
  decodeSupported: boolean;
  decodeReason: string | null;
  captureSupported: boolean;
  captureReason: string | null;
}): {
  hostSupported: boolean;
  hostReason: string | null;
  viewerSupported: boolean;
  viewerReason: string | null;
} {
  return {
    hostSupported: decodeSupported && captureSupported,
    hostReason: !decodeSupported
      ? decodeReason
      : captureSupported
        ? null
        : captureReason,
    viewerSupported: decodeSupported,
    viewerReason: decodeSupported ? null : decodeReason,
  };
}
