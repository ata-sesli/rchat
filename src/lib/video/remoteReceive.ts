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

export function createRemoteVideoReceiveState(): RemoteVideoReceiveState {
  return { hasKeyframe: false };
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
