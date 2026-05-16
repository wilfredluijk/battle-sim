// Wire types for the `/spectate` endpoint. Mirrors `docs/PROTOCOL.md` §2 and the structs
// in `server/src/protocol.rs` (SpectatorMsg::World et al.). Keep field names in lock-step
// with the wire format — these objects come straight out of JSON.parse.

export type SensorMode = 'active' | 'passive';

export interface ShipSnapshot {
  id: string;
  bot_name: string;
  pos: [number, number];
  heading_deg: number;
  speed: number;
  hp: number;
  ammo: number;
  throttle: number;
  rudder: number;
  alive: boolean;
  ready: boolean;
  commands_per_sec: number;
  sensor_mode: SensorMode;
}

export interface ShellSnapshot {
  id_index: number;
  pos: [number, number];
  vel: [number, number];
  ttl_ticks: number;
}

export type TickEvent =
  | { type: 'hit'; ship_id: string; amount: number }
  | { type: 'shell_splash'; pos: [number, number] }
  | { type: 'death'; ship_id: string };

export interface WorldFrame {
  type: 'world';
  tick: number;
  ships: ShipSnapshot[];
  shells: ShellSnapshot[];
  events: TickEvent[];
}

// ---------------------------------------------------------------------------
// Admin endpoint — matches server/src/admin.rs (AdminMsg / AdminServerMsg).
// ---------------------------------------------------------------------------

export type AdminRoomState = 'lobby' | 'running' | 'ended';

export interface AdminBotInfo {
  bot_id: string;
  name: string;
  ship_id: string;
  ready: boolean;
  alive: boolean;
}

export interface AdminStatePayload {
  room: string;
  state: AdminRoomState;
  tick: number;
  last_winner?: string | null;
  bots: AdminBotInfo[];
}

export type AdminServerMsg =
  | { type: 'state'; room: string; state: AdminRoomState; tick: number; last_winner?: string | null; bots: AdminBotInfo[] }
  | { type: 'ack'; command: string }
  | { type: 'error'; code: string; message: string };

export type AdminMsg =
  | { type: 'start' }
  | { type: 'abort' }
  | { type: 'reset' }
  | { type: 'kick'; bot_id: string };
