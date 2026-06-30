export type LocalCameraToggleState = {
  starting: boolean;
};

export type LocalCameraToggleCommand = {
  enabled: boolean;
};

export type LocalCameraToggleDecision = {
  state: LocalCameraToggleState;
  command: LocalCameraToggleCommand | null;
};

export type LocalPreviewCanvasState = {
  cameraEnabled: boolean;
  hasPreviewError: boolean;
};

export function createLocalCameraToggleState(): LocalCameraToggleState {
  return { starting: false };
}

export function requestLocalCameraToggle(
  state: LocalCameraToggleState,
  currentlyEnabled: boolean,
): LocalCameraToggleDecision {
  if (state.starting) {
    return { state, command: null };
  }

  const enabled = !currentlyEnabled;
  return {
    state: { starting: enabled },
    command: { enabled },
  };
}

export function markLocalCameraToggleSettled(
  state: LocalCameraToggleState,
): LocalCameraToggleState {
  if (!state.starting) return state;
  return { starting: false };
}

export function shouldRenderLocalPreviewCanvas({
  cameraEnabled,
  hasPreviewError,
}: LocalPreviewCanvasState): boolean {
  return cameraEnabled && !hasPreviewError;
}
