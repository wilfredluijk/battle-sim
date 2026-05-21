// Wire types for the spectator app.
//
// - The `/spectate` WebSocket types mirror `docs/PROTOCOL.md` §2 and `SpectatorMsg` in
//   `server/src/protocol.rs`.
// - The REST types mirror the `/api/*` control plane in `server/src/net.rs`.
//
// Keep field names in lock-step with the wire format — these objects come straight out of
// JSON.parse.

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
// REST control plane — matches the `/api/*` routes in `server/src/net.rs`.
// ---------------------------------------------------------------------------

export type RoomState = 'lobby' | 'running' | 'ended';

export interface AdminBotInfo {
  bot_id: string;
  name: string;
  ship_id: string;
  ready: boolean;
  alive: boolean;
}

/** Balance parameters — a flat map of `SimConfig` keys to numbers. */
export type SimConfig = Record<string, number>;

/** Response shape of `GET /api/room`: room lifecycle state plus the active parameters. */
export interface RoomInfo {
  room: string;
  state: RoomState;
  tick: number;
  last_winner?: string | null;
  bots: AdminBotInfo[];
  config: SimConfig;
}

/** One tunable's metadata from `GET /api/config/schema`. */
export interface ConfigField {
  key: string;
  label: string;
  group: string;
  default: number;
  min: number;
  max: number;
  integer: boolean;
}

/** One bot's row in a `MatchReport`. */
export interface BotReport {
  bot_id: string;
  name: string;
  shots_fired: number;
  hits_landed: number;
  accuracy: number;
  damage_dealt: number;
  damage_taken: number;
  kills: number;
  final_hp: number;
  survived: boolean;
}

/** Response shape of `GET /api/room/report`. */
export interface MatchReport {
  room: string;
  replay_id: string | null;
  outcome: 'winner' | 'draw' | 'aborted';
  winner: string | null;
  winner_name: string | null;
  duration_ticks: number;
  duration_seconds: number;
  bots: BotReport[];
}
