// Visual + simulation constants. The "MAX_*" gameplay values match `ShipSpecs::DEFAULT`
// in the server (`server/src/protocol.rs`). They're duplicated here so the spectator can
// scale meters without first seeing a `welcome` frame; a future spectator that wires up
// the bot welcome could read these from the server instead.

export const MAP_WIDTH = 1000;
export const MAP_HEIGHT = 1000;
export const SHIP_RADIUS = 12; // visual radius; sim hit_radius is 8
export const ACTIVE_RADAR_RANGE = 350;
export const SPLASH_DRAW_MS = 600;
export const MAX_EVENTS = 20;

export const MAX_HP = 100;
export const MAX_AMMO = 20;
export const MAX_FORWARD_SPEED = 6.0;
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
