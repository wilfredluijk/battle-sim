// Wire types for the spectator app.
//
// - The `/spectate` WebSocket types mirror `docs/PROTOCOL.md` §2 and `SpectatorMsg` in
//   `server/src/protocol.rs`.
// - The REST types mirror the `/api/*` control plane in `server/src/net.rs`.
//
// Keep field names in lock-step with the wire format — these objects come straight out of
// JSON.parse.

export type SensorMode = 'active' | 'passive';

/** One picked powerup's live state, mirrored in every ship snapshot. */
export interface PowerupStatus {
  id: string;
  used: boolean;
  active_ticks_left: number;
}

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
  /** Loadout this bot picked for the match (pick order). Empty when none. */
  selected_powerups?: string[];
  /** Live status for each pick, same order as `selected_powerups`. */
  powerup_status?: PowerupStatus[];
}

export interface ShellSnapshot {
  id_index: number;
  pos: [number, number];
  vel: [number, number];
  ttl_ticks: number;
}

export interface SmokeCloudSnapshot {
  pos: [number, number];
  radius: number;
  expires_at: number;
}

export interface DecoySnapshot {
  fake_id: number;
  owner: string;
  pos: [number, number];
  heading_deg: number;
  expires_at: number;
}

export type TickEvent =
  | { type: 'hit'; ship_id: string; amount: number }
  | { type: 'shell_splash'; pos: [number, number] }
  | { type: 'death'; ship_id: string }
  | { type: 'powerup_activated'; ship_id: string; powerup: string };

export interface WorldFrame {
  type: 'world';
  tick: number;
  ships: ShipSnapshot[];
  shells: ShellSnapshot[];
  events: TickEvent[];
  smoke_clouds?: SmokeCloudSnapshot[];
  decoys?: DecoySnapshot[];
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
  connected?: boolean;
  forfeited?: boolean;
  disconnect_reason?: string | null;
  readiness_blocker?: string | null;
}

/** Match tuning, including the nested powerup configuration. */
export interface SimConfig {
  [key: string]: number | Record<string, number> | undefined;
  max_forward_speed?: number;
  max_reverse_speed?: number;
  acceleration?: number;
  turn_rate_deg_per_s?: number;
  hull_hp?: number;
  max_ammo?: number;
  gun_cooldown_ticks?: number;
  hit_radius?: number;
  shell_speed?: number;
  max_shell_range?: number;
  splash_radius?: number;
  max_splash_damage?: number;
  active_radar_range?: number;
  active_radar_noise?: number;
  passive_hear_active_range?: number;
  passive_hear_nearby_range?: number;
  passive_bearing_noise_deg?: number;
  wall_bump_damage?: number;
  powerups?: Record<string, number>;
}

/** Response shape of `GET /api/room`: room lifecycle state plus the active parameters. */
export interface RoomInfo {
  room: string;
  state: RoomState;
  tick: number;
  last_winner?: string | null;
  bots: AdminBotInfo[];
  config: SimConfig;
  map?: { width: number; height: number };
  tick_hz?: number;
  replay_mode?: boolean;
  capabilities?: { monte_carlo: boolean; manage_match: boolean; tournament: boolean };
  match_id?: string;
  config_hash?: string;
  match_timeout_ticks?: number;
  tick_deadline_ms?: number;
  expected_teams?: string[];
  roster_error?: string | null;
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
  forfeited?: boolean;
}

/** Response shape of `GET /api/room/report`. */
export interface MatchReport {
  room: string;
  replay_id: string | null;
  outcome: 'winner' | 'draw' | 'aborted';
  end_reason?: string;
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
  | { type: 'shell_splash'; pos: [number, number] }
  | { type: 'powerup_activated'; own: boolean; contact_id?: string; powerup: string };

/** One bot in a replay header. */
export interface ReplayBotInfo {
  bot_id: string;
  ship_id: string;
  name: string;
  selected_powerups?: string[];
  spawn_pos: [number, number];
  spawn_heading_deg: number;
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
  outcome?: string;
  end_reason?: string | null;
}

/** Response shape of `GET /api/replays/{id}` — the ground-truth timeline. */
export interface CapturedReplay {
  header: ReplayHeaderInfo;
  /** `frames[t]` is the world at tick `t`. */
  frames: WorldFrame[];
  end: { tick: number; winner: string | null; outcome?: string; end_reason?: string } | null;
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

/** Server-to-bot configuration metadata, protocol 2.0. */
export interface BotWelcome {
  type: 'welcome';
  protocol_version: string;
  simulation_dt: number;
  bot_id: string;
  ship_id: string;
  map: { width: number; height: number };
  tick_hz: number;
  ship_specs: Record<string, number>;
  available_powerups: string[];
}
export interface BotGameStart {
  type: 'game_start';
  tick: number;
  starting_position: [number, number];
  starting_heading_deg: number;
  ship_specs: Record<string, number>;
  simulation_dt: number;
}

/** Bot protocol v3. Full settings are acknowledged before readiness. */
export interface MatchConfiguration {
  protocol_version: '3.0';
  revision: number;
  simulation_dt: number;
  tick_hz: number;
  deadline_ms: number;
  map: { width: number; height: number };
  max_bots: number;
  match_timeout_ticks: number;
  sim_config: SimConfig;
  ship_specs: Record<string, number>;
  available_powerups: string[];
  command_policy: 'exact_match_and_tick_first_valid_wins';
  disconnect_policy: 'forfeit_hull_retained';
  timeout_ties: 'draw';
  action_phases: string[];
  starting_positions: 'constrained_random_hidden' | 'public_ring';
}
export interface ConfigurationMessage {
  type: 'configuration';
  config_hash: string;
  configuration: MatchConfiguration;
}
export type BotHandshake =
  | { type: 'hello'; name: string; version: string; token: string }
  | { type: 'ready'; config_hash: string };
