import {
  ACTIVE_RADAR_RANGE,
  MAP_HEIGHT,
  MAP_WIDTH,
  MAX_HP,
  SHIP_RADIUS,
  SPLASH_DRAW_MS,
} from './constants';
import { colorFor, withAlpha } from './palette';
import { worldToCanvasTransform } from './canvas';
import { chipsForShip } from './powerupHud';
import type {
  Contact,
  DecoySnapshot,
  ShellSnapshot,
  ShipSnapshot,
  SmokeCloudSnapshot,
  WorldFrame,
} from '../types/protocol';

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
  mapW: number = MAP_WIDTH,
  mapH: number = MAP_HEIGHT,
  maxHp: number = MAX_HP,
): void {
  const canvas = ctx.canvas;
  const w = canvas.width;
  const h = canvas.height;
  ctx.setTransform(1, 0, 0, 1, 0, 0);
  ctx.clearRect(0, 0, w, h);

  const { scale, offX, offY } = worldToCanvasTransform(w, h, mapW, mapH);
  ctx.setTransform(scale, 0, 0, scale, offX, offY);

  // Map bounds.
  ctx.lineWidth = 1 / scale;
  ctx.strokeStyle = 'rgba(255, 255, 255, 0.18)';
  ctx.strokeRect(0, 0, mapW, mapH);

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

  // Smoke clouds: translucent grey discs. Drawn under shells and ships so the
  // sailing target stays legible.
  for (const cloud of latest.smoke_clouds ?? []) {
    drawSmokeCloud(ctx, cloud, scale);
  }

  // Decoys: dashed ghost markers that look like ships but are clearly phantoms.
  for (const decoy of latest.decoys ?? []) {
    drawDecoy(ctx, decoy, scale);
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
    drawShip(ctx, ship, scale, maxHp);
  }
}

function drawSmokeCloud(
  ctx: CanvasRenderingContext2D,
  cloud: SmokeCloudSnapshot,
  scale: number,
): void {
  ctx.beginPath();
  ctx.arc(cloud.pos[0], cloud.pos[1], cloud.radius, 0, Math.PI * 2);
  ctx.fillStyle = 'rgba(180, 188, 198, 0.22)';
  ctx.fill();
  ctx.strokeStyle = 'rgba(220, 225, 235, 0.45)';
  ctx.setLineDash([5 / scale, 5 / scale]);
  ctx.lineWidth = 1.5 / scale;
  ctx.stroke();
  ctx.setLineDash([]);
}

function drawDecoy(ctx: CanvasRenderingContext2D, decoy: DecoySnapshot, scale: number): void {
  const color = colorFor(decoy.owner);
  const x = decoy.pos[0];
  const y = decoy.pos[1];
  const rad = (decoy.heading_deg * Math.PI) / 180;
  const dirX = Math.sin(rad);
  const dirY = -Math.cos(rad);
  const perpX = -dirY;
  const perpY = dirX;
  // Same hull silhouette as a real ship, drawn dashed and hollow.
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
  ctx.setLineDash([4 / scale, 4 / scale]);
  ctx.strokeStyle = withAlpha(color, 0.55);
  ctx.lineWidth = 1.5 / scale;
  ctx.stroke();
  ctx.setLineDash([]);
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

function drawShip(
  ctx: CanvasRenderingContext2D,
  ship: ShipSnapshot,
  scale: number,
  maxHp: number = MAX_HP,
): void {
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
    const pct = Math.max(0, Math.min(1, ship.hp / maxHp));
    const hpFill = pct > 0.6 ? '#58d68d' : pct > 0.25 ? '#f4d35e' : '#ef6b6b';
    ctx.fillStyle = hpFill;
    ctx.fillRect(-barW / 2, 4, barW * pct, barH);
  }

  // Powerup chip strip — pretty compact dots beneath the HP bar so a glance tells
  // you what each ship has up its sleeve.
  const chips = chipsForShip(ship);
  if (chips.length > 0) {
    const dotR = 3;
    const gap = 3;
    const stripW = chips.length * (dotR * 2) + (chips.length - 1) * gap;
    const startX = -stripW / 2 + dotR;
    chips.forEach((chip, i) => {
      const cx = startX + i * (dotR * 2 + gap);
      const cy = 14;
      ctx.beginPath();
      ctx.arc(cx, cy, dotR, 0, Math.PI * 2);
      if (chip.state === 'active') {
        ctx.fillStyle = '#f4d35e';
      } else if (chip.state === 'used') {
        ctx.fillStyle = 'rgba(180, 188, 198, 0.45)';
      } else {
        ctx.fillStyle = 'rgba(220, 225, 235, 0.9)';
      }
      ctx.fill();
      if (chip.state === 'active') {
        ctx.strokeStyle = 'rgba(244, 211, 94, 0.55)';
        ctx.lineWidth = 1.5;
        ctx.beginPath();
        ctx.arc(cx, cy, dotR + 2, 0, Math.PI * 2);
        ctx.stroke();
      }
    });
  }
  ctx.restore();
}

/**
 * Draw the battlefield as one bot's sensors see it: that bot's own ship in full detail,
 * and everything else only as the filtered `contacts` the bot actually received. Used by
 * the replay viewer's per-bot perspective mode.
 */
export function drawPerspective(
  ctx: CanvasRenderingContext2D,
  ownShip: ShipSnapshot | null,
  contacts: Contact[],
  mapW: number = MAP_WIDTH,
  mapH: number = MAP_HEIGHT,
  maxHp: number = MAX_HP,
): void {
  const canvas = ctx.canvas;
  const w = canvas.width;
  const h = canvas.height;
  ctx.setTransform(1, 0, 0, 1, 0, 0);
  ctx.clearRect(0, 0, w, h);

  const { scale, offX, offY } = worldToCanvasTransform(w, h, mapW, mapH);
  ctx.setTransform(scale, 0, 0, scale, offX, offY);

  ctx.lineWidth = 1 / scale;
  ctx.strokeStyle = 'rgba(255, 255, 255, 0.18)';
  ctx.strokeRect(0, 0, mapW, mapH);

  if (!ownShip) return;

  // The own ship's active-radar ring, if it is pinging.
  if (ownShip.alive && ownShip.sensor_mode === 'active') {
    const col = colorFor(ownShip.bot_name);
    ctx.beginPath();
    ctx.arc(ownShip.pos[0], ownShip.pos[1], ACTIVE_RADAR_RANGE, 0, Math.PI * 2);
    ctx.fillStyle = withAlpha(col, 0.06);
    ctx.fill();
    ctx.strokeStyle = withAlpha(col, 0.3);
    ctx.lineWidth = 1.5 / scale;
    ctx.stroke();
  }

  for (const c of contacts) {
    drawContact(ctx, ownShip, c, scale, mapW, mapH);
  }

  // Own ship last, fully detailed, on top of the contact overlay.
  drawShip(ctx, ownShip, scale, maxHp);
}

/**
 * Draw one sensor contact: a bearing line from the observer plus a blip at the contact's
 * estimated position. Opacity tracks `confidence`; a range-less (passive bearing-only)
 * contact is drawn dashed and hollow to signal positional uncertainty.
 */
function drawContact(
  ctx: CanvasRenderingContext2D,
  ownShip: ShipSnapshot,
  c: Contact,
  scale: number,
  mapW: number,
  mapH: number,
): void {
  const conf = Math.max(0.12, Math.min(1, c.confidence));
  const known = c.range != null;
  const ox = ownShip.pos[0];
  const oy = ownShip.pos[1];

  // Bearing line. Compass bearing: 0° = north (-y), 90° = east (+x).
  const rad = (c.bearing_deg * Math.PI) / 180;
  const dirX = Math.sin(rad);
  const dirY = -Math.cos(rad);
  const lineLen = known ? (c.range as number) : Math.hypot(mapW, mapH);
  ctx.beginPath();
  ctx.moveTo(ox, oy);
  ctx.lineTo(ox + dirX * lineLen, oy + dirY * lineLen);
  ctx.setLineDash([6 / scale, 6 / scale]);
  ctx.strokeStyle = `rgba(255, 255, 255, ${0.08 + 0.2 * conf})`;
  ctx.lineWidth = 1 / scale;
  ctx.stroke();
  ctx.setLineDash([]);

  const cx = c.pos[0];
  const cy = c.pos[1];

  if (c.kind === 'shell') {
    ctx.beginPath();
    ctx.arc(cx, cy, 3, 0, Math.PI * 2);
    ctx.fillStyle = `rgba(255, 245, 184, ${conf})`;
    ctx.fill();
    return;
  }

  // Ship or unknown contact: a ring at the estimated position.
  const color = c.kind === 'ship' ? '#ef6b6b' : '#9aa7b4';
  const radius = SHIP_RADIUS * (c.kind === 'ship' ? 1.0 : 0.8);
  ctx.beginPath();
  ctx.arc(cx, cy, radius, 0, Math.PI * 2);
  if (known) {
    ctx.fillStyle = withAlpha(color, 0.22 * conf);
    ctx.fill();
  } else {
    ctx.setLineDash([4 / scale, 4 / scale]);
  }
  ctx.strokeStyle = withAlpha(color, 0.35 + 0.5 * conf);
  ctx.lineWidth = (known ? 1.6 : 1) / scale;
  ctx.stroke();
  ctx.setLineDash([]);

  if (c.kind === 'unknown') {
    ctx.save();
    ctx.translate(cx, cy);
    ctx.scale(1 / scale, 1 / scale);
    ctx.font = "11px -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif";
    ctx.textAlign = 'center';
    ctx.textBaseline = 'middle';
    ctx.fillStyle = withAlpha(color, 0.4 + 0.6 * conf);
    ctx.fillText('?', 0, 0);
    ctx.restore();
  }
}
