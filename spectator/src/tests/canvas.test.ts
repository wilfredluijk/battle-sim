import { describe, expect, it } from 'vitest';
import { worldToCanvasTransform } from '../lib/canvas';

describe('complete battlefield fit', () => {
  it.each([
    [1506, 833, 700, 700], // Full HD with roster
    [1888, 833, 700, 700], // Full HD expanded
    [1466, 825, 700, 700], // Full HD projector
    [1466, 645, 700, 700], // Full HD monitor with browser chrome
    [912, 513, 700, 700],
    [1506, 833, 1400, 500], // Wide map
    [1506, 833, 500, 1400], // Tall map
    [1, 1, 700, 700], // Resizing must not create a negative scale
  ])('keeps every world corner inside %i × %i for a %i × %i map', (w, h, mapW, mapH) => {
    const { scale, offX, offY } = worldToCanvasTransform(w, h, mapW, mapH);
    expect(scale).toBeGreaterThan(0);
    for (const x of [0, mapW]) for (const y of [0, mapH]) {
      expect(offX + x * scale).toBeGreaterThan(0);
      expect(offX + x * scale).toBeLessThan(w);
      expect(offY + y * scale).toBeGreaterThan(0);
      expect(offY + y * scale).toBeLessThan(h);
    }
    expect((mapW * scale) / (mapH * scale)).toBeCloseTo(mapW / mapH);
    expect(offX + mapW * scale / 2).toBeCloseTo(w / 2);
    expect(offY + mapH * scale / 2).toBeCloseTo(h / 2);
  });

  it('keeps the same CSS geometry on a Retina/HiDPI display', () => {
    const normal = worldToCanvasTransform(1506, 833, 700, 700);
    const retina = worldToCanvasTransform(3012, 1666, 700, 700);
    expect(retina.scale / 2).toBe(normal.scale);
    expect(retina.offX / 2).toBe(normal.offX);
    expect(retina.offY / 2).toBe(normal.offY);
  });
});
