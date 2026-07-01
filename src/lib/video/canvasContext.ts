export type CanvasLikeContext = {
  clearRect?: (x: number, y: number, width: number, height: number) => void;
};

export type CanvasLike<C extends CanvasLikeContext = CanvasLikeContext> = {
  width: number;
  height: number;
  getContext: (kind: "2d") => C | null;
};

export type CanvasContextCache<E extends CanvasLike<C>, C extends CanvasLikeContext> = {
  element: E | null;
  context: C | null;
};

export function createCanvasContextCache<
  E extends CanvasLike<C>,
  C extends CanvasLikeContext,
>(): CanvasContextCache<E, C> {
  return {
    element: null,
    context: null,
  };
}

export function getCachedCanvasContext<
  E extends CanvasLike<C>,
  C extends CanvasLikeContext,
>(cache: CanvasContextCache<E, C>, element: E | null): C | null {
  if (!element) {
    cache.element = null;
    cache.context = null;
    return null;
  }

  if (cache.element !== element || !cache.context) {
    cache.element = element;
    cache.context = element.getContext("2d");
  }

  return cache.context;
}

export function clearCanvasAndResetContext<
  E extends CanvasLike<C>,
  C extends CanvasLikeContext,
>(cache: CanvasContextCache<E, C>, element: E | null) {
  if (element) {
    const context =
      cache.element === element && cache.context
        ? cache.context
        : element.getContext("2d");
    context?.clearRect?.(0, 0, element.width, element.height);
  }
  cache.element = null;
  cache.context = null;
}

export function isCurrentRemoteDecodeCallback(
  callbackGeneration: number,
  currentGeneration: number,
  callbackSessionId: string | null,
  currentSessionId: string | null,
): boolean {
  return (
    callbackGeneration === currentGeneration &&
    Boolean(callbackSessionId) &&
    callbackSessionId === currentSessionId
  );
}
