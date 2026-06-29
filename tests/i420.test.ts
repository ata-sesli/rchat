// @ts-nocheck
import { describe, expect, test } from "bun:test";
import { expectedI420Length, rgbaToI420 } from "../src/lib/video/i420";

describe("i420 conversion", () => {
  test("computes planar 4:2:0 length for even dimensions", () => {
    expect(expectedI420Length(2, 2)).toBe(6);
    expect(expectedI420Length(1280, 720)).toBe(1_382_400);
  });

  test("rejects odd or empty dimensions", () => {
    expect(expectedI420Length(0, 2)).toBeNull();
    expect(expectedI420Length(2, 0)).toBeNull();
    expect(expectedI420Length(3, 2)).toBeNull();
    expect(expectedI420Length(2, 3)).toBeNull();
  });

  test("converts rgba pixels into y u v planes", () => {
    const rgba = new Uint8ClampedArray([
      0, 0, 0, 255,
      255, 255, 255, 255,
      255, 0, 0, 255,
      0, 255, 0, 255,
    ]);

    const i420 = rgbaToI420(rgba, 2, 2);

    expect(i420).toHaveLength(6);
    expect(Array.from(i420.slice(0, 4))).toEqual([16, 235, 81, 145]);
    expect(i420[4]).toBeGreaterThan(90);
    expect(i420[5]).toBeGreaterThan(130);
  });

  test("rejects rgba buffers with the wrong length", () => {
    expect(() => rgbaToI420(new Uint8ClampedArray(4), 2, 2)).toThrow(
      "RGBA buffer length does not match dimensions",
    );
  });
});
