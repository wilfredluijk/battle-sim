// Keep fallbacks synchronized with server/src/sim/constants.rs.
// Visual + simulation constants. The "MAX_*" gameplay values match the `SimConfig`
// defaults in the server (`server/src/sim/config.rs`). They are fallbacks only — once
// `GET /api/room` is available, the spectator uses the match's actual config.

export const MAP_WIDTH = 700;
export const MAP_HEIGHT = 700;
export const SHIP_RADIUS = 12; // visual radius; sim hit_radius is 8
export const ACTIVE_RADAR_RANGE = 350;
export const SPLASH_DRAW_MS = 600;
export const MAX_EVENTS = 20;

export const MAX_HP = 100;
export const MAX_AMMO = 250;
export const MAX_FORWARD_SPEED = 9.0;
export const MAX_REVERSE_SPEED = 2.0;

export const COLOR_PALETTE = [
  '#6cb1ff',
  '#ef9a4a',
  '#9f7df7',
  '#58d68d',
  '#f4d35e',
  '#ff6f9c',
  '#5fd1c5',
  '#c47bff',
] as const;
