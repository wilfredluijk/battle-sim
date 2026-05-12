import {
  ACTIVE_RADAR_RANGE,
  MAP_HEIGHT,
  MAP_WIDTH,
  SHIP_RADIUS,
  SPLASH_DRAW_MS,
} from './constants';
import { colorFor, withAlpha } from './palette';
import { worldToCanvasTransform } from './canvas';
import type { ShellSnapshot, ShipSnapshot, WorldFrame } from '../types/protocol';

export interface Splash {
  x: number;
  y: number;
  startedAt: number;
}

/**
 * Draw a single world frame onto `ctx`. Pure with respect to the caller's state: takes a
 * frame, a list of in-flight splash animations (mutated in place to expire old entries),
 * and the current `performance.now()` to interpolate splash radii.
 */
export function draw(
  ctx: CanvasRenderingContext2D,
  latest: WorldFrame | null,
  splashes: Splash[],
  now: number,
): void {
  const canvas = ctx.canvas;
  const w = canvas.width;
  const h = canvas.height;
  ctx.setTransform(1, 0, 0, 1, 0, 0);
  ctx.clearRect(0, 0, w, h);

  const { scale, offX, offY } = worldToCanvasTransform(w, h);
  ctx.setTransform(scale, 0, 0, scale, offX, offY);

  // Map bounds.
  ctx.lineWidth = 1 / scale;
  ctx.strokeStyle = 'rgba(255, 255, 255, 0.18)';
  ctx.strokeRect(0, 0, MAP_WIDTH, MAP_HEIGHT);

  if (!latest) return;

  // Active radar rings beneath everything else.
  for (const ship of latest.ships) {
    if (!ship.alive || ship.sensor_mode !== 'active') continue;
    ctx.beginPath();
    ctx.arc(ship.pos[0], ship.pos[1], ACTIVE_RADAR_RANGE, 0, Math.PI * 2);
    const col = colorFor(ship.bot_name);
    ctx.fillStyle = withAlpha(col, 0.06);
    ctx.fill();
    ctx.strokeStyle = withAlpha(col, 0.35);
    ctx.lineWidth = 1.5 / scale;
    ctx.stroke();
  }

  // Shells: small dot + a short trail opposite the velocity vector.
  for (const shell of latest.shells) {
    drawShell(ctx, shell, scale);
  }

  // Splash rings, animated. Mutate the splashes array to drop expired entries.
  for (let i = splashes.length - 1; i >= 0; i--) {
    const sp = splashes[i]!;
    const age = now - sp.startedAt;
    if (age > SPLASH_DRAW_MS) {
      splashes.splice(i, 1);
      continue;
    }
    const t = age / SPLASH_DRAW_MS;
    const r = 6 + t * 30;
    ctx.beginPath();
    ctx.arc(sp.x, sp.y, r, 0, Math.PI * 2);
    ctx.strokeStyle = `rgba(255, 240, 180, ${1 - t})`;
    ctx.lineWidth = 2 / scale;
    ctx.stroke();
  }

  for (const ship of latest.ships) {
    drawShip(ctx, ship, scale);
  }
}

function drawShell(ctx: CanvasRenderingContext2D, shell: ShellSnapshot, scale: number): void {
  ctx.beginPath();
  ctx.arc(shell.pos[0], shell.pos[1], 3, 0, Math.PI * 2);
  ctx.fillStyle = '#fff5b8';
  ctx.fill();
  const vx = shell.vel[0];
  const vy = shell.vel[1];
  const speed = Math.hypot(vx, vy) || 1;
  ctx.beginPath();
  ctx.moveTo(shell.pos[0], shell.pos[1]);
  ctx.lineTo(shell.pos[0] - (vx / speed) * 18, shell.pos[1] - (vy / speed) * 18);
  ctx.strokeStyle = 'rgba(255, 245, 184, 0.4)';
  ctx.lineWidth = 1.5 / scale;
  ctx.stroke();
}

function drawShip(ctx: CanvasRenderingContext2D, ship: ShipSnapshot, scale: number): void {
  const color = colorFor(ship.bot_name);
  const x = ship.pos[0];
  const y = ship.pos[1];

  // Compass heading: 0° = north (-y), 90° = east (+x).
  const rad = (ship.heading_deg * Math.PI) / 180;
  const dirX = Math.sin(rad);
  const dirY = -Math.cos(rad);
  const perpX = -dirY;
  const perpY = dirX;

  const nose: [number, number] = [
    x + dirX * SHIP_RADIUS * 1.4,
    y + dirY * SHIP_RADIUS * 1.4,
  ];
  const tailL: [number, number] = [
    x - dirX * SHIP_RADIUS * 0.8 + perpX * SHIP_RADIUS * 0.9,
    y - dirY * SHIP_RADIUS * 0.8 + perpY * SHIP_RADIUS * 0.9,
  ];
  const tailR: [number, number] = [
    x - dirX * SHIP_RADIUS * 0.8 - perpX * SHIP_RADIUS * 0.9,
    y - dirY * SHIP_RADIUS * 0.8 - perpY * SHIP_RADIUS * 0.9,
  ];

  ctx.beginPath();
  ctx.moveTo(nose[0], nose[1]);
  ctx.lineTo(tailL[0], tailL[1]);
  ctx.lineTo(tailR[0], tailR[1]);
  ctx.closePath();
  ctx.fillStyle = ship.alive ? color : withAlpha(color, 0.25);
  ctx.fill();
  ctx.strokeStyle = ship.alive ? 'rgba(0,0,0,0.45)' : 'rgba(0,0,0,0.2)';
  ctx.lineWidth = 1 / scale;
  ctx.stroke();

  // Name + HP bar in screen-pixel sizes (un-scale the world transform).
  const labelOffsetY = -SHIP_RADIUS * 1.8;
  ctx.save();
  ctx.translate(x, y + labelOffsetY);
  ctx.scale(1 / scale, 1 / scale);
  ctx.font = "11px -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif";
  ctx.textAlign = 'center';
  ctx.fillStyle = ship.alive ? 'rgba(255,255,255,0.95)' : 'rgba(255,255,255,0.4)';
  ctx.fillText(ship.bot_name, 0, 0);

  if (ship.alive) {
    const barW = 36;
    const barH = 4;
    ctx.fillStyle = 'rgba(255,255,255,0.15)';
    ctx.fillRect(-barW / 2, 4, barW, barH);
    const pct = Math.max(0, Math.min(1, ship.hp / 100));
    const hpFill = ship.hp > 60 ? '#58d68d' : ship.hp > 25 ? '#f4d35e' : '#ef6b6b';
    ctx.fillStyle = hpFill;
    ctx.fillRect(-barW / 2, 4, barW * pct, barH);
  }
  ctx.restore();
}
