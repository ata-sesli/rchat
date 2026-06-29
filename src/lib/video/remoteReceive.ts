export type RemoteVideoChunkType = "key" | "delta";

export type RemoteVideoReceiveState = {
  hasKeyframe: boolean;
};

export type RemoteVideoFrameDecision =
  | { decode: true; waitingForKeyframe: false }
  | { decode: false; waitingForKeyframe: true };

export type RemoteVideoReceiveQueue = {
  tail: Promise<void>;
};

export type RemoteVideoDecoderConfig = {
  codec: string;
  codedWidth?: number;
  codedHeight?: number;
  optimizeForLatency: boolean;
  hardwareAcceleration?: "no-preference" | "prefer-hardware" | "prefer-software";
};

export function createRemoteVideoReceiveState(): RemoteVideoReceiveState {
  return { hasKeyframe: false };
}

export function createRemoteVideoDecoderConfigCandidates(
  codec: string,
  width: number,
  height: number,
): RemoteVideoDecoderConfig[] {
  const base = { codec, optimizeForLatency: true };
  if (!Number.isFinite(width) || !Number.isFinite(height) || width <= 0 || height <= 0) {
    return [base];
  }
  const coded = {
    ...base,
    codedWidth: Math.trunc(width),
    codedHeight: Math.trunc(height),
  };
  return [
    { ...coded, hardwareAcceleration: "prefer-software" },
    coded,
    base,
  ];
}

export function createRemoteVideoReceiveQueue(): RemoteVideoReceiveQueue {
  return { tail: Promise.resolve() };
}

export function enqueueRemoteVideoReceiveTask(
  queue: RemoteVideoReceiveQueue,
  task: () => Promise<void>,
  onError?: (error: unknown) => void,
): Promise<void> {
  const run = queue.tail.then(task);
  queue.tail = run.catch((error) => {
    onError?.(error);
  });
  return queue.tail;
}

export function hasRemoteVideoKeyframe(state: RemoteVideoReceiveState): boolean {
  return state.hasKeyframe;
}

export function shouldDecodeRemoteVideoFrame(
  state: RemoteVideoReceiveState,
  chunkType: RemoteVideoChunkType,
): RemoteVideoFrameDecision {
  if (chunkType === "key") {
    state.hasKeyframe = true;
    return { decode: true, waitingForKeyframe: false };
  }

  if (!state.hasKeyframe) {
    return { decode: false, waitingForKeyframe: true };
  }

  return { decode: true, waitingForKeyframe: false };
}

export function markRemoteVideoDecoderFailed(state: RemoteVideoReceiveState) {
  state.hasKeyframe = false;
}

export function markRemoteVideoSequenceGap(state: RemoteVideoReceiveState) {
  state.hasKeyframe = false;
}
