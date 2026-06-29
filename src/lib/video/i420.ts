export function expectedI420Length(width: number, height: number): number | null {
  if (width <= 0 || height <= 0 || width % 2 !== 0 || height % 2 !== 0) {
    return null;
  }
  return width * height + (width * height) / 2;
}

function clampByte(value: number): number {
  return Math.max(0, Math.min(255, Math.round(value)));
}

function yFromRgb(r: number, g: number, b: number): number {
  return clampByte(16 + (65.738 * r + 129.057 * g + 25.064 * b) / 256);
}

function uFromRgb(r: number, g: number, b: number): number {
  return clampByte(128 + (-37.945 * r - 74.494 * g + 112.439 * b) / 256);
}

function vFromRgb(r: number, g: number, b: number): number {
  return clampByte(128 + (112.439 * r - 94.154 * g - 18.285 * b) / 256);
}

export function rgbaToI420(
  rgba: Uint8ClampedArray,
  width: number,
  height: number,
): Uint8Array {
  const expectedRgbaLength = width * height * 4;
  if (rgba.length !== expectedRgbaLength) {
    throw new Error("RGBA buffer length does not match dimensions");
  }

  const i420Length = expectedI420Length(width, height);
  if (i420Length === null) {
    throw new Error("I420 conversion requires positive even dimensions");
  }

  const output = new Uint8Array(i420Length);
  const yPlaneSize = width * height;
  const uOffset = yPlaneSize;
  const vOffset = yPlaneSize + yPlaneSize / 4;
  const uvWidth = width / 2;

  for (let y = 0; y < height; y += 1) {
    for (let x = 0; x < width; x += 1) {
      const rgbaIndex = (y * width + x) * 4;
      output[y * width + x] = yFromRgb(
        rgba[rgbaIndex],
        rgba[rgbaIndex + 1],
        rgba[rgbaIndex + 2],
      );
    }
  }

  for (let y = 0; y < height; y += 2) {
    for (let x = 0; x < width; x += 2) {
      let rSum = 0;
      let gSum = 0;
      let bSum = 0;
      for (let dy = 0; dy < 2; dy += 1) {
        for (let dx = 0; dx < 2; dx += 1) {
          const rgbaIndex = ((y + dy) * width + x + dx) * 4;
          rSum += rgba[rgbaIndex];
          gSum += rgba[rgbaIndex + 1];
          bSum += rgba[rgbaIndex + 2];
        }
      }

      const r = rSum / 4;
      const g = gSum / 4;
      const b = bSum / 4;
      const uvIndex = (y / 2) * uvWidth + x / 2;
      output[uOffset + uvIndex] = uFromRgb(r, g, b);
      output[vOffset + uvIndex] = vFromRgb(r, g, b);
    }
  }

  return output;
}
