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

// ---------------------------------------------------------------------------
// Replay viewer — matches `/api/replays/*` in `server/src/replay.rs`.
// ---------------------------------------------------------------------------

export type ContactKind = 'ship' | 'shell' | 'unknown';

/** One sensor contact in a bot's filtered view (`tick.contacts` in `docs/PROTOCOL.md`). */
export interface Contact {
  id: string;
  kind: ContactKind;
  pos: [number, number];
  bearing_deg: number;
  /** Absent for passive bearing-only contacts. */
  range?: number | null;
  confidence: number;
}

/** A bot-facing combat event — the filtered form, distinct from the spectator `TickEvent`. */
export type BotTickEvent =
  | { type: 'hit'; amount: number }
  | { type: 'shell_splash'; pos: [number, number] };

/** One bot in a replay header. */
export interface ReplayBotInfo {
  bot_id: string;
  ship_id: string;
  name: string;
}

/** The replay log header, echoed by `GET /api/replays/{id}`. */
export interface ReplayHeaderInfo {
  version: number;
  replay_id: string;
  room: string;
  seed: number;
  tick_hz: number;
  tick_deadline_ms: number;
  map: { width: number; height: number };
  max_bots: number;
  sim_config: SimConfig;
  bots: ReplayBotInfo[];
}

/** One entry from `GET /api/replays`. */
export interface ReplaySummary {
  replay_id: string;
  room: string;
  seed: number;
  tick_hz: number;
  map: { width: number; height: number };
  sim_config: SimConfig;
  bots: string[];
  final_tick: number | null;
  winner_name: string | null;
}

/** Response shape of `GET /api/replays/{id}` — the ground-truth timeline. */
export interface CapturedReplay {
  header: ReplayHeaderInfo;
  /** `frames[t]` is the world at tick `t`. */
  frames: WorldFrame[];
  end: { tick: number; winner: string | null } | null;
}

/** One bot's sensor-filtered view at a single tick. */
export interface PerspectiveFrame {
  tick: number;
  contacts: Contact[];
  events: BotTickEvent[];
}

/** Response shape of `GET /api/replays/{id}/perspective/{bot_id}`. */
export interface CapturedPerspective {
  bot_id: string;
  /** Dense and aligned to the ground-truth timeline; `frames[t]` is tick `t`. */
  frames: PerspectiveFrame[];
}

// ---------------------------------------------------------------------------
// Monte Carlo batch runner — matches `/api/montecarlo/*` in `server/src/net.rs`.
// ---------------------------------------------------------------------------

export type VarianceMode = 'fixed' | 'rotated' | 'shuffled' | 'random';

/** Body of `POST /api/montecarlo/start`. */
export interface McStartRequest {
  n_matches: number;
  mc_seed: number;
  variance_mode: VarianceMode;
  /** Per-tick timeout for the lockstep loop, in milliseconds. Optional; defaults to 1000. */
  per_tick_timeout_ms?: number;
  /** Spectator broadcast cadence (every Nth tick); 0 disables spectator updates. */
  spectator_throttle?: number;
  /** Optional SimConfig override applied once at the start of the run. */
  sim_config?: SimConfig;
}

/** One row in `McStatus.results` — outcome of a single match in the batch. */
export interface McMatchResult {
  /** 1-based index of the match within the run. */
  match_index: number;
  seed: number;
  winner: string | null;
  winner_name: string | null;
  duration_ticks: number;
  replay_id: string | null;
}

/** Response shape of `GET /api/montecarlo/status`. */
export interface McStatus {
  running: boolean;
  run_id: string;
  completed: number;
  total: number;
  variance_mode: VarianceMode;
  mc_seed: number;
  started_at_unix: number;
  finished_at_unix: number | null;
  current_match_tick: number;
  wins: Record<string, number>;
  bot_names: Record<string, string>;
  draws: number;
  results: McMatchResult[];
  ended_reason: string | null;
}
