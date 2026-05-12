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
