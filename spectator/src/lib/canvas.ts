import { MAP_HEIGHT, MAP_WIDTH } from './constants';

/**
 * Match the canvas's device-pixel resolution to its layout size so lines aren't fuzzy on
 * hi-DPI displays. Returns the actual pixel dimensions so callers can avoid re-reading
 * the canvas right after.
 */
export function fitCanvas(canvas: HTMLCanvasElement): { width: number; height: number } {
  const dpr = window.devicePixelRatio || 1;
  const rect = canvas.getBoundingClientRect();
  const width = Math.max(1, Math.round(rect.width * dpr));
  const height = Math.max(1, Math.round(rect.height * dpr));
  canvas.width = width;
  canvas.height = height;
  return { width, height };
}

/** World→canvas scale + offset, preserving aspect ratio with letterboxing. */
export interface CanvasTransform {
  scale: number;
  offX: number;
  offY: number;
}

export function worldToCanvasTransform(
  canvasW: number,
  canvasH: number,
  mapW: number = MAP_WIDTH,
  mapH: number = MAP_HEIGHT,
): CanvasTransform {
  const scale = Math.min(canvasW / mapW, canvasH / mapH);
  const offX = (canvasW - mapW * scale) / 2;
  const offY = (canvasH - mapH * scale) / 2;
  return { scale, offX, offY };
}
