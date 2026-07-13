# Wire Protocol

Public contract between the naval-battle server and bot / spectator clients. This doc and `server/src/protocol.rs` are mirrors — when one changes, the other changes in the same commit.

- **Transport:** WebSocket over TCP for the streaming surfaces; plain HTTP for the control plane.
- **Encoding:** UTF-8 JSON, one object per text frame, no batching, no binary frames.
- **Endpoints:**
  - `ws://<host>:<port>/bot` — bidirectional WebSocket, untrusted.
  - `ws://<host>:<port>/spectate` — server-to-client WebSocket only.
  - `http://<host>:<port>/api/*` — REST control plane for the operator UI (admin login + room lifecycle). See §2.5.
- **Discriminator:** every message has a `"type"` field (snake_case).
- **Coordinates:** `[x, y]` arrays of `f32`. Origin top-left, `+x` right, `+y` down. Bearings are in absolute compass degrees (0° = `+y`-axis is unspecified by the design; treat bearings consistently between `fire` and contacts — see PR-relative discussion in `system-design.md` §5.4).
- **Numbers:** all coordinates, speeds, headings, and ranges are `f32`. Tick numbers, HP, and ammo are unsigned integers.

---

## 1. Bot endpoint — `/bot`

### 1.1 Bot → Server

#### `hello`
First message after the WebSocket connects.

```json
{ "type": "hello", "name": "captain_kirk", "version": "1.0" }
```

| Field | Type | Notes |
|---|---|---|
| `name` | string | 1–32 bytes of `[A-Za-z0-9 _-]`. An invalid name is rejected with `invalid_name`; a duplicate of another live bot in the same room is rejected with `duplicate_name`. Both close the connection. |
| `version` | string | SDK version, free-form. |

#### `ready`
Sent after `welcome` is received and the bot is willing to start.

```json
{ "type": "ready" }
```

#### `select_powerups`
Optional. Declare the (exactly two distinct) powerups this bot will use for the match. May only be sent while the room is in `lobby`; the bot may send it before *or* after `ready`, but always before `game_start`. Sending it twice in lobby replaces the previous selection. Bots that never send it play with no powerups (vanilla).

```json
{ "type": "select_powerups", "powerups": ["rapid_fire", "heavy_shell"] }
```

| Field | Type | Notes |
|---|---|---|
| `powerups` | array of string | Exactly 2 distinct ids from `welcome.available_powerups`. |

Validation errors come back as typed `error` frames; the previous selection (if any) is preserved on failure. See §3 for the `powerup_*` codes. The full catalog and tuning lives in `docs/POWERUPS.md`.

#### `command`
Sent once per tick in response to a `tick` message. Echo the tick number from the `tick` frame.

```json
{
  "type": "command",
  "tick": 142,
  "throttle": 0.8,
  "rudder": -0.3,
  "fire": { "bearing_deg": 47.5, "range": 300.0 },
  "sensor_mode": "active",
  "activate_powerup": "overdrive"
}
```

| Field | Type | Notes |
|---|---|---|
| `tick` | u64 | Must match the tick number being responded to. |
| `throttle` | f32 in `[-1.0, 1.0]` | `1.0` = full ahead, `-1.0` = full reverse. |
| `rudder` | f32 in `[-1.0, 1.0]` | `-1.0` = hard port, `1.0` = hard starboard. |
| `fire` | object, optional | Omit if not firing this tick. |
| `fire.bearing_deg` | f32 | Absolute compass bearing for the shell. |
| `fire.range` | f32 | Requested travel distance, clamped server-side to `max_shell_range`. |
| `sensor_mode` | `"active"` \| `"passive"` | Required. |
| `activate_powerup` | string, optional | One-off activation of a powerup this bot picked (e.g. `"rapid_fire"`). Resolved alongside `fire` on the same tick. Each picked powerup can activate at most once per match. Unknown or already-used ids yield typed `powerup_*` errors. See `docs/POWERUPS.md` for the catalog. |

### 1.2 Server → Bot

#### `welcome`
Acknowledges the `hello` and assigns identifiers and gameplay constants.

```json
{
  "type": "welcome",
  "bot_id": "b_3",
  "ship_id": "s_3",
  "map": { "width": 700, "height": 700 },
  "tick_hz": 10,
  "ship_specs": {
    "max_forward_speed": 9.0,
    "max_reverse_speed": 2.0,
    "acceleration": 3.5,
    "turn_rate_deg_per_s": 20.0,
    "hull_hp": 100,
    "max_ammo": 250,
    "gun_cooldown_ticks": 15,
    "hit_radius": 8.0,
    "shell_speed": 70.0,
    "max_shell_range": 300.0,
    "splash_radius": 15.0,
    "max_splash_damage": 25
  },
  "available_powerups": [
    "overdrive", "reinforced_hull", "repair_drones", "smoke_screen",
    "rapid_fire", "heavy_shell", "long_range_salvo", "awacs_scan",
    "silent_running", "counter_battery_trace", "emp_burst", "decoy_flare"
  ]
}
```

Field values shown above are the current defaults; the live `welcome` payload always reflects whatever the server is actually running. The runtime authority for these constants is `server/src/sim/constants.rs` — `ship_specs` is derived from there.

`available_powerups` is the catalog the server understands; pass any of these ids to `select_powerups`. See `docs/POWERUPS.md` for what each one does.

#### `game_start`
Sent when the operator transitions the room to `running`.

```json
{
  "type": "game_start",
  "tick": 0,
  "starting_position": [120.0, 340.0],
  "starting_heading_deg": 90.0
}
```

#### `tick`
Sent at the top of every simulation tick. Bot must reply with a `command` before `deadline_ms` elapses.

```json
{
  "type": "tick",
  "tick": 142,
  "deadline_ms": 80,
  "self": {
    "pos": [203.4, 511.7],
    "heading_deg": 92.3,
    "speed": 4.1,
    "hp": 78,
    "ammo": 14,
    "rudder": -0.3,
    "throttle": 0.8,
    "selected_powerups": ["overdrive", "rapid_fire"],
    "powerup_status": [
      { "id": "overdrive",  "used": false, "active_ticks_left": 0 },
      { "id": "rapid_fire", "used": true,  "active_ticks_left": 23 }
    ]
  },
  "contacts": [
    {
      "id": "c_a1",
      "kind": "ship",
      "pos": [450.0, 510.0],
      "bearing_deg": 88.0,
      "range": 247.0,
      "confidence": 0.85
    }
  ],
  "events": [
    { "type": "hit", "amount": 12 },
    { "type": "shell_splash", "pos": [220.0, 505.0] },
    { "type": "powerup_activated", "contact_id": "c_a1", "powerup": "smoke_screen" }
  ]
}
```

`contacts[].id` is a per-tick contact ID, **not** the underlying `ship_id`. Trackers must do their own data association across ticks.

`contacts[].range` is omitted when not measured (passive mode).

`contacts[].kind` is one of `"ship"`, `"shell"`, `"unknown"`.

`events[]` only contains things this bot can perceive: own hits and splashes inside its sensor range, plus `powerup_activated` events. A `powerup_activated` event carries a `contact_id`, **not** a `ship_id`: for the bot's own activation `contact_id` is omitted (`null`), and for another ship's activation it is the same per-tick anonymized `c_<n>` id that ship appears under in this frame's `contacts` — the event is emitted **only** when the activating ship actually shows up in this bot's sweep this tick (same visibility as a contact, soft counters included), and is re-anonymized every tick like contacts, so it cannot be used to track a specific opponent across ticks.

`self.selected_powerups` and `self.powerup_status` are omitted (or sent as empty arrays) when the bot picked no powerups. `powerup_status[i].active_ticks_left` counts down each tick; check `used && active_ticks_left == 0` to know a pick is spent.

A bot with `counter_battery_trace` armed will see a synthetic precise contact for the attacker for the next `counter_battery_reveal_ticks` tick frames (default 15) after the trace fires. These contacts use a `cbt_<n>` id and full confidence.

#### `game_over`
Sent when a match ends — either naturally (last ship standing / match timeout) or because the operator aborted. `winner` is `null` for a draw **or an aborted match**.

```json
{
  "type": "game_over",
  "winner": "b_3",
  "final_tick": 1843,
  "replay_id": "match_20260508_171203"
}
```

The connection stays open after `game_over`. A few seconds later (~`POST_GAME_LOBBY_TICKS / tick_hz`) the server emits a `lobby` frame and the bot can re-send `ready` to participate in the next match. Bots that want to exit can simply close the WebSocket from their end.

#### `lobby`
Sent when the room returns to the lobby after a match. SDKs should treat this as the signal to re-send `ready` if they want to play the next match. `tick` is always 0.

```json
{ "type": "lobby", "tick": 0 }
```

#### `error`
Sent in response to a malformed or otherwise-rejected bot frame. See [§3 Error codes](#3-error-codes).

```json
{ "type": "error", "code": "late_command", "message": "command for tick 142 arrived 95ms after frame (deadline 80ms)" }
```

The `message` field is human-readable and intended for logs / debugger output. It is *not* a stable contract — bots should branch on `code`, never on substring matches against `message`. Where useful, the server includes context (tick number, ms over deadline, ticks of cooldown remaining, expected schema); the exact wording may evolve.

### 1.3 Late and missing commands

If a `command` arrives after the per-tick deadline, the server replies with `error` (`code: "late_command"`) and applies the previous tick's `throttle` / `rudder` / `sensor_mode`. No shot is fired that tick. The bot is **not** disconnected for missing or late commands — only for repeated protocol violations.

### 1.4 Match lifecycle

Bots persist across matches on a single WebSocket connection:

```
hello → welcome → [select_powerups] → ready → game_start → tick* → game_over →
                  (lobby) → [select_powerups] → ready → game_start → tick* → … 
```

- `welcome` is sent exactly once per connection.
- `bot_id` and `ship_id` are stable for the lifetime of the connection.
- After every `game_over`, the server returns the room to the lobby and emits `lobby`. SDKs auto-send `ready` on receipt and wait for the next `game_start`. Bot authors who want to exit can close the connection from their side after `game_over`.
- An operator-issued `abort` is delivered to bots as a `game_over` with `winner: null`. An operator-issued `reset` cuts the post-game pause short; the bot still sees `game_over` (already delivered) followed by `lobby` immediately afterwards.

---

## 2. Spectator endpoint — `/spectate`

Read-only: the server pushes ground-truth state every tick, ignores anything the spectator sends.

#### `world`

```json
{
  "type": "world",
  "tick": 142,
  "ships": [
    {
      "id": "s_1",
      "bot_name": "captain_kirk",
      "pos": [203.4, 511.7],
      "heading_deg": 92.3,
      "speed": 4.1,
      "hp": 78,
      "ammo": 14,
      "throttle": 0.8,
      "rudder": -0.3,
      "alive": true,
      "ready": true,
      "commands_per_sec": 10.0,
      "sensor_mode": "active",
      "selected_powerups": ["overdrive", "rapid_fire"],
      "powerup_status": [
        { "id": "overdrive",  "used": false, "active_ticks_left": 0 },
        { "id": "rapid_fire", "used": true,  "active_ticks_left": 23 }
      ]
    }
  ],
  "shells": [
    {
      "id_index": 22,
      "pos": [310.0, 500.0],
      "vel": [40.0, 5.0],
      "ttl_ticks": 18
    }
  ],
  "events": [
    { "type": "hit", "ship_id": "s_1", "amount": 12 },
    { "type": "shell_splash", "pos": [220.0, 505.0] },
    { "type": "death", "ship_id": "s_2" },
    { "type": "powerup_activated", "ship_id": "s_1", "powerup": "rapid_fire" }
  ],
  "smoke_clouds": [
    { "pos": [320.0, 480.0], "radius": 60.0, "expires_at": 222 }
  ],
  "decoys": [
    {
      "fake_id": 0,
      "owner": "s_2",
      "pos": [500.0, 500.0],
      "heading_deg": 90.0,
      "expires_at": 200
    }
  ]
}
```

`shells[].id_index` is a stable-per-shell index used by renderers to track trails across ticks.

`ships[].sensor_mode` is the bot's most recently commanded mode (`"active"` or `"passive"`); the renderer uses it to draw the active-radar ring.

`ships[].speed` is the ship's signed scalar velocity (positive = ahead, negative = astern); `throttle` and `rudder` are the last commanded control values, both in `[-1.0, 1.0]`.

`ships[].ready` is the lobby-readiness flag — `true` once the bot has sent `ready`. It stays `true` for the rest of the match.

`ships[].commands_per_sec` is the number of `command` frames the room accepted from this bot over the last second of sim time (i.e. the last `tick_hz` ticks). The value is zero in the lobby and ticks up once the match is running. Late, stale, or violation-rejected commands do not count.

`ships[].selected_powerups` and `ships[].powerup_status` mirror the per-bot tick fields and are sent for spectator HUDs. Both are omitted (or empty arrays) when the bot picked nothing.

`smoke_clouds[]` lists live `smoke_screen` clouds; `decoys[]` lists live `decoy_flare` phantoms. Both arrays are omitted (or empty) when no such entity exists this tick. Spectators get full ground truth — bots only learn about smoke / decoys via the contact filter.

---

## 2.5 Admin control plane — `/api/*` (REST)

Operator control is a plain HTTP/JSON REST API, not a WebSocket. The spectator web UI uses it to inspect and drive the room; there is no stdin command interface.

**Authentication.** Mutating routes require a JSON Web Token. Obtain one with `POST /api/login`, then send it as `Authorization: Bearer <jwt>` on every mutating request. The admin password is set with `--admin-password` (or the `BATTLE_ADMIN_PASSWORD` env var); when neither is provided the server generates a random password and logs it once at `INFO` on startup. Tokens expire after `--token-ttl-hours` hours (default 12).

**Errors.** Any non-2xx response carries a JSON body `{ "code": "...", "message": "..." }`. Successful mutations return `204 No Content`.

### 2.5.1 Endpoints

| Method & path | Auth | Success | Purpose |
|---|---|---|---|
| `POST /api/login` | — | `200` | Exchange the admin password for a JWT. |
| `GET /api/room` | public | `200` | Current room state plus the active balance parameters. |
| `GET /api/room/report` | public | `200` / `404` | Most recent match report (`404` until a match has finished). |
| `GET /api/config/schema` | public | `200` | Metadata for the pre-match parameter form. |
| `PUT /api/room/config` | admin | `204` | Replace the match parameters. Only valid in the lobby. |
| `POST /api/room/start` | admin | `204` | Lobby → Running. Refused if not in lobby, no bots, or not all ready. |
| `POST /api/room/abort` | admin | `204` | Force-end the running match (`game_over` with `winner: null`). |
| `POST /api/room/reset` | admin | `204` | Ended → Lobby immediately, skipping the post-game pause. |
| `POST /api/room/kick` | admin | `204` | Disconnect a single bot by `bot_id`. |

### 2.5.2 `POST /api/login`

```json
// request                          // response (200)
{ "password": "hunter2" }           { "token": "eyJhbGc...", "expires_at": 1779400000 }
```

A wrong password returns `401` with code `invalid_credentials`.

### 2.5.3 `GET /api/room`

```json
{
  "room": "main",
  "state": "lobby",
  "tick": 0,
  "last_winner": null,
  "bots": [
    { "bot_id": "b_1", "name": "alice", "ship_id": "s_1", "ready": true, "alive": true }
  ],
  "config": { "hull_hp": 100, "shell_speed": 70.0, "...": "..." },
  "map": { "width": 700, "height": 700 }
}
```

| Field | Type | Notes |
|---|---|---|
| `state` | `"lobby"` \| `"running"` \| `"ended"` | Room state machine. `ended` is the post-game pause; the room returns to `lobby` automatically after ~2s. |
| `tick` | u64 | Current `world.tick`. |
| `last_winner` | string \| null | `bot_id` of the most recent winner, or `null` for a draw / abort / fresh match. |
| `config` | object | The active `SimConfig` — a flat map of every balance tunable to its current value. |
| `map` | object | Arena size in world units: `{ "width", "height" }`. Set via `--map WxH` (default 700×700), not part of `config`. The live spectator view reads it to size its bounds. |

### 2.5.4 `GET /api/config/schema`

Describes each tunable so a UI can render a form. `integer` fields must be sent as whole numbers in `PUT /api/room/config`.

```json
{
  "fields": [
    { "key": "hull_hp", "label": "Hull HP", "group": "ship",
      "default": 100, "min": 1, "max": 100000, "integer": true }
  ]
}
```

### 2.5.5 `PUT /api/room/config`

Body is a complete `SimConfig` object (every key from the schema). Parameters are frozen when the match starts and recorded in the replay header.

- `204` — applied.
- `400` `invalid_parameter` — a value failed validation (non-finite, out of bounds).
- `409` `not_in_lobby` — the room is running or ended; parameters cannot change mid-match.
- `401` `unauthorized` — missing or invalid bearer token.

### 2.5.6 `GET /api/room/report`

The post-match summary. `404` with code `no_report` until the first match has finished; the report then persists until the next match starts.

```json
{
  "room": "main",
  "replay_id": "match_main_1779400000",
  "outcome": "winner",
  "winner": "b_1",
  "winner_name": "alice",
  "duration_ticks": 412,
  "duration_seconds": 41.2,
  "bots": [
    { "bot_id": "b_1", "name": "alice", "shots_fired": 18, "hits_landed": 7,
      "accuracy": 0.388, "damage_dealt": 140, "damage_taken": 55,
      "kills": 1, "final_hp": 45, "survived": true }
  ]
}
```

| Field | Type | Notes |
|---|---|---|
| `outcome` | `"winner"` \| `"draw"` \| `"aborted"` | How the match ended. |
| `winner` / `winner_name` | string \| null | The winning `bot_id` and name; `null` for a draw or abort. |
| `duration_ticks` / `duration_seconds` | u64 / f32 | Match length. |
| `bots[].accuracy` | f32 | `hits_landed / shots_fired`, in `[0, 1]`; `0` when the bot never fired. |

### 2.5.7 `POST /api/room/kick`

```json
{ "bot_id": "b_3" }
```

Returns `404` with code `unknown_bot` when no bot holds that id.

## 2.6 Replay viewer — `/api/replays/*` (REST)

Read-only routes that back the spectator's replay viewer. They re-run a recorded match
server-side and return the reconstructed timeline as JSON. No JWT is required, but — like
`/spectate` — they are restricted to loopback peers when the server runs in tournament
mode (`403` `tournament_mode` otherwise), because replays expose ground-truth state.

| Method & path | Success | Purpose |
|---|---|---|
| `GET /api/replays` | `200` | List the replays on disk, newest first. |
| `GET /api/replays/{id}` | `200` | Re-run a replay; return the ground-truth timeline. |
| `GET /api/replays/{id}/perspective/{bot_id}` | `200` | Re-run a replay from one bot's sensors. |

`{id}` is a replay id (`match_<room>_<unix_secs>`); it is validated against
`[A-Za-z0-9_-]` and rejected with `400` `invalid_replay_id` otherwise. A missing file
returns `404` `replay_not_found`; a log older than the current replay format returns `422`
`unsupported_replay_version`.

### 2.6.1 `GET /api/replays`

```json
[
  {
    "replay_id": "match_main_1779367343",
    "room": "main", "seed": 42, "tick_hz": 60,
    "map": { "width": 700, "height": 700 },
    "sim_config": { "hull_hp": 100, "...": "..." },
    "bots": ["powerful", "tactician"],
    "final_tick": 1071,
    "winner_name": "powerful"
  }
]
```

`final_tick` and `winner_name` are `null` for a log with no `end` record (an incomplete
match) or a draw / aborted match.

### 2.6.2 `GET /api/replays/{id}`

Re-runs the simulation from the recorded inputs and captures the ground-truth world at
every tick.

```json
{
  "header": { "version": 5, "replay_id": "...", "seed": 42, "map": { "...": "..." },
              "sim_config": { "...": "..." },
              "bots": [ { "bot_id": "b_1", "ship_id": "s_1", "name": "powerful",
                          "spawn_pos": [300.0, 500.0], "spawn_heading_deg": 90.0 } ] },
  "frames": [ /* one `world` payload (§2 `world`) per tick; frames[t] is the world at tick t */ ],
  "end": { "tick": 1071, "winner": "b_1" }
}
```

`frames` has `final_tick + 1` entries: index `0` is the starting layout, index `t` is the
world after tick `t`. `end` is `null` for an incomplete log.

**On-disk log format v5.** The JSONL log driving these endpoints carries `header`, `tick`,
`disconnect`, and `end` records. A `disconnect` record —
`{ "type": "disconnect", "tick": T, "bot_id": "b_2" }` — is written whenever a bot
disconnects or is kicked mid-match (while the room is `running`); `T` is the last tick the
ship participated in. Re-simulation removes the ship at exactly that point so the shared RNG
stream and the recorded outcome stay bit-identical (the ship simply vanishes from later
`frames`). Logs written before v5 (`"version"` 2–4) carry no `disconnect` records and still
load — a match where nobody dropped is indistinguishable from an older log.

### 2.6.3 `GET /api/replays/{id}/perspective/{bot_id}`

Re-runs the match and captures one bot's sensor-filtered view — the same `contacts` the
bot received in its `tick` messages (§1.2 `tick`).

```json
{
  "bot_id": "b_1",
  "frames": [ { "tick": 0, "contacts": [], "events": [] },
              { "tick": 1, "contacts": [ /* §1.2 Contact */ ], "events": [ /* §1.2 TickEvent */ ] } ]
}
```

`frames` is dense and aligned to the ground-truth timeline (`frames[t]` is tick `t`).
Ticks where the bot received no `tick` message — tick 0 and the deciding tick — carry
empty `contacts` and `events`. An unknown `bot_id` returns `404` `unknown_bot`.

---

## 2.7 Monte Carlo batch runner — `/api/montecarlo/*` (REST)

Admin-only routes that drive a sequence of matches against the same connected bot roster,
varying the starting positions per match and reporting which bot wins most often. The
batch runs in **lockstep mode** — the server waits for every bot to send its command for
the current tick, then steps immediately, instead of pacing on wall-clock. With local
bots this typically completes ~10× faster than the equivalent number of wall-clocked
matches. Every match's replay is preserved in the replay directory.

### 2.7.1 Endpoints

| Method & path | Success | Purpose |
|---|---|---|
| `POST /api/montecarlo/start` | `200` | Start a batch. Body: `McStartRequest` (below). |
| `POST /api/montecarlo/stop` | `204` | Stop the active batch. Body: `{ "force_abort": bool }` (optional). |
| `GET /api/montecarlo/status` | `200` | Snapshot of the active or most-recent run. |

`start` and `stop` require `Authorization: Bearer <jwt>`. `status` is public so the
spectator UI can poll it without holding admin credentials.

Preconditions for `start`: the room must be in `lobby`, at least two bots must be
connected, and every connected bot must be `ready`. Otherwise `409 Conflict` with `code`
= `mc_refused`. Invalid `McStartRequest` fields return `400 Bad Request` with `code` =
`invalid_parameter`.

### 2.7.2 `McStartRequest`

```json
{
  "n_matches": 100,
  "mc_seed": 42,
  "variance_mode": "shuffled",
  "per_tick_timeout_ms": 1000,
  "spectator_throttle": 5,
  "sim_config": { "...": "optional SimConfig override" }
}
```

| Field | Meaning |
|---|---|
| `n_matches` | Number of matches to run. Capped at 10000. |
| `mc_seed` | Root seed; the per-match seed is `splitmix64(mc_seed ^ match_index)`. |
| `variance_mode` | One of `fixed`, `rotated`, `shuffled`, `random`. See below. |
| `per_tick_timeout_ms` | Optional. Lockstep deadline (default 1000). |
| `spectator_throttle` | Optional. Broadcast every Nth tick; `0` disables (default 5). |
| `sim_config` | Optional. Replaces the active balance parameters at run start. |

Variance modes:

- **`fixed`** — every match uses the standard ring layout (radius 400, evenly spaced).
- **`rotated`** — same ring, rotated by a per-match random angle.
- **`shuffled`** — rotate plus permute which bot lands on which slot.
- **`random`** — sample each ship's position uniformly inside a disk, rejection-sampled
  for a minimum separation. Initial heading is also randomized.

### 2.7.3 `GET /api/montecarlo/status`

```json
{
  "running": true,
  "run_id": "0000000000abcdef",
  "completed": 47,
  "total": 100,
  "variance_mode": "shuffled",
  "mc_seed": 42,
  "started_at_unix": 1779878000,
  "finished_at_unix": null,
  "current_match_tick": 312,
  "wins": { "b_1": 21, "b_2": 18, "b_3": 6 },
  "bot_names": { "b_1": "chaser", "b_2": "sniper", "b_3": "circler" },
  "draws": 2,
  "results": [
    { "match_index": 47, "seed": 12345, "winner": "b_1", "winner_name": "chaser",
      "duration_ticks": 312, "replay_id": "mc_..._match_0047_seed_..." }
  ],
  "ended_reason": null
}
```

`results` is a bounded tail of the most recent matches (last 20). Older replays remain
accessible via `GET /api/replays` and `GET /api/replays/{id}`. Once the run finishes,
`running` flips to `false` and `ended_reason` carries `"completed"`, `"stopped"`,
`"bot_disconnected"`, or `"error"`.

Replays produced during a run follow the naming scheme
`mc_<run_id>_match_<NNNN>_seed_<hex>` and live in the same directory as regular replays.

### 2.7.4 Bot-facing behaviour during an MC run

The wire protocol (`§1`) is unchanged. Bots see exactly the same sequence they'd see in
back-to-back single matches:

```
welcome → ready → game_start → tick × N → game_over → game_start → tick × N → game_over → …
```

Between matches the server skips the usual `POST_GAME_LOBBY_TICKS` pause, so `game_over`
is immediately followed by the next `game_start` (the `lobby` frame is **not** sent in
MC mode — bots stay implicitly "ready" for the whole run). A bot that wants to support
Monte Carlo runs should treat `game_start` as a one-shot reset signal and keep looping
on `tick` frames thereafter; a bot that exits on the first `game_over` will only play
one match per run.

If any bot disconnects mid-run the controller aborts, finalizes the status snapshot
with `ended_reason = "bot_disconnected"`, and force-ends the in-flight match with no
winner so survivors see a clean `game_over`.

---

## 3. Error codes

Codes are strings; the human-readable detail goes in `message`. Bot authors should switch on `code` for behaviour and surface `message` for diagnostics.

| Code | Meaning |
|---|---|
| `malformed_json` | The frame did not parse as JSON. |
| `invalid_message` | JSON parsed but did not match any known message schema. |
| `binary_frames_unsupported` | The `/bot` endpoint received a binary frame. |
| `too_many_violations` | Last warning before the connection is closed. |
| `late_command` | Command arrived after the per-tick deadline. |
| `cooldown_active` | `fire` was issued while the gun was still cooling down. The `message` field reports the current tick and the remaining cooldown ticks. Duplicate cooldown errors in the same tick are coalesced into a single frame. |
| `no_ammo` | `fire` was issued but the ship has no ammo left. Coalesced like `cooldown_active`. |
| `invalid_name` | `hello.name` was empty, longer than 32 bytes, or contained characters outside `[A-Za-z0-9 _-]`. The connection is closed. |
| `duplicate_name` | `hello.name` duplicates another live bot already registered in the room. The connection is closed. |
| `stale_command` | `command.tick` was outside the accepted window (`world_tick ± 1`). |
| `non_finite_value` | A command contained `NaN` or `Inf` in `throttle`, `rudder`, or `fire.{bearing_deg,range}`. |
| `handshake_timeout` | The bot connected but did not send `hello` within the handshake timeout. The connection is closed. |
| `powerup_unknown` | `select_powerups` or `command.activate_powerup` referenced an id not in `welcome.available_powerups`. |
| `powerup_duplicate` | `select_powerups.powerups` listed the same id twice. |
| `powerup_wrong_count` | `select_powerups.powerups` did not contain exactly two entries. |
| `powerup_lobby_only` | `select_powerups` was sent while the room was not in `lobby`. |
| `powerup_not_selected` | `command.activate_powerup` named a powerup the bot didn't pick for this match. |
| `powerup_already_used` | `command.activate_powerup` named a powerup the bot already activated this match. |

After 5 protocol violations on a single bot connection, the server sends `too_many_violations` and closes with WebSocket close code `Policy (1008)`. The violation-counted codes are `malformed_json`, `invalid_message`, `non_finite_value`, and `binary_frames_unsupported`. `handshake_timeout`, `invalid_name`, and `duplicate_name` close the connection on the first occurrence and bypass the counter. `late_command`, `stale_command`, `cooldown_active`, and `no_ammo` are gameplay rejections and do not count against the bot.

One further server-side limit is enforced without a dedicated error code:

- **Per-IP connection cap** — when the peer IP is at `--max-connections-per-ip`, the TCP stream is dropped *before* the WebSocket handshake completes. The bot observes a connection close with no error frame.

WebSocket messages are capped at 16 KiB. The `/spectate` endpoint can be restricted to the loopback interface with `--tournament` so competing bots cannot use it to bypass the sensor filter.

---

## 4. Versioning

The server's release version is included in `welcome.version` (planned — currently absent in MVP). Additive changes (new optional fields, new event types) are backwards-compatible. Renamed or removed fields, type changes, and changed semantics are breaking and will bump the version sent in `welcome`.

---

## Changelog

<!-- Each entry: ## YYYY-MM-DD — version. List additions / changes / removals. -->

## 2026-07-07 — turn-rate ratio clamped (physics behavior change)

- The heading turn rate is `turn_rate_max * rudder * min(|speed| / max_forward, 1.0)` — the
  speed ratio is now clamped to 1.0. Previously, when Overdrive expired while the ship was
  still above the un-boosted `max_forward`, the ratio exceeded 1.0 and the ship turned faster
  than `turn_rate_max` for a few ticks (an exploitable "super-turn on expiry"). No wire-shape
  change, but simulation output differs, so replay logs recorded before this fix will not
  re-simulate byte-identically.

## 2026-07-07 — `powerup_activated` no longer leaks a ground-truth ship id (breaking)

- **Breaking (bot-facing).** The `powerup_activated` tick event's `ship_id` field is
  replaced by `contact_id`. For the bot's own activation `contact_id` is omitted (`null`);
  for another ship's activation it is the per-tick anonymized `c_<n>` id that ship appears
  under in the same frame's `contacts`. The event is now emitted **only** when the
  activating ship actually shows up in that viewer's sweep this tick — gated by the real
  sensor result (soft counters like AWACS-vs-silent-running included), not a separate,
  stronger visibility rule. Previously the event carried the persistent ground-truth
  `ship_id`, letting a bot re-identify and track a specific opponent for the whole match,
  and fired on ticks where the viewer's sweep saw nothing. The **spectator** `world`
  event is unchanged and still carries the ground-truth `ship_id`. SDKs updated in step
  (`PowerupActivatedEvent.contact_id: Optional[str]`).

## 2026-07-07 — replay records mid-match disconnects

- Replay log format bumped to `v5`. The log gained a `disconnect` record
  (`{ "type": "disconnect", "tick": T, "bot_id": "..." }`) written whenever a bot
  disconnects or is kicked while the room is `running`. Re-simulation removes the ship at
  tick `T` so the shared RNG stream and the recorded outcome stay bit-identical; previously
  a dropped bot left a ghost ship in the replay and the re-simulated final state could
  contradict the recorded `end`. See §2.6. v2–v4 logs carry no `disconnect` records and
  still load. Bot and spectator wire protocols are unchanged.

## 2026-06-23 — more specific join-rejection codes

- A `hello.name` that duplicates another live bot in the room is now rejected with the
  dedicated `duplicate_name` error code instead of the generic `invalid_message`. A name
  failing `[A-Za-z0-9 _-]{1,32}` validation at registration likewise surfaces as
  `invalid_name`. Both still close the connection. Bots that switch on the exact `code`
  for the duplicate case should accept `duplicate_name`; the human-readable `message` is
  unchanged.

## 2026-06-02 — replay spawn state

- Replay format bumped to `v4`. The header's per-bot entries now record `spawn_pos`
  (`[x, y]`) and `spawn_heading_deg` — the ship's actual placed position at tick 0. This
  lets a replay reproduce the true starting layout even when the live run used a non-Fixed
  variance layout (Monte Carlo runs default to `shuffled`), which the old ring-based rebuild
  could not. Both fields are optional (`serde(default)`): older `v2`/`v3` logs deserialize
  them to `[0, 0]` / `0` and rebuild falls back to the default ring layout, so they still
  load. Readers now accept any header `version <= 4` and reject only newer-than-current
  logs.

## 2026-05-27 — powerups

- New `select_powerups` bot→server message (§1.1).
- New optional `activate_powerup` field on `command`.
- New `welcome.available_powerups` array advertising the server's catalog.
- `tick.self` gained optional `selected_powerups` and `powerup_status` arrays.
- New `powerup_activated` `events[]` entry on both the `tick` and `world` payloads.
- Spectator `world` payload gained optional `smoke_clouds[]` and `decoys[]` arrays, plus per-ship `selected_powerups` / `powerup_status`.
- New `powerup_*` error codes: `powerup_unknown`, `powerup_duplicate`, `powerup_wrong_count`, `powerup_lobby_only`, `powerup_not_selected`, `powerup_already_used`.
- Replay format bumped to `v3` (older logs are rejected with `replay_format_version`). The header now records each bot's `selected_powerups` and each `ReplayCommand` may carry `activate_powerup`.

All additions are forward-compatible for bots that don't use powerups: omit `select_powerups` and the new fields, and play as before.

## 2026-05-27 — Monte Carlo batch runner

- Added admin-gated `/api/montecarlo/start`, `/api/montecarlo/stop`, and public
  `/api/montecarlo/status` routes. See §2.7. Additive — the `/bot` wire protocol is
  unchanged. Bots that loop on `game_start` automatically support batch runs; bots that
  exit on the first `game_over` will play one match per run.
- Replays produced during a Monte Carlo run use the naming scheme
  `mc_<run_id>_match_<NNNN>_seed_<hex>` and remain accessible through the existing
  `/api/replays` routes.

## 2026-05-21 — replay viewer REST routes

- Added read-only `/api/replays/*` routes that re-run a recorded match server-side and
  return the reconstructed timeline: `GET /api/replays` (listing), `GET /api/replays/{id}`
  (ground-truth timeline), and `GET /api/replays/{id}/perspective/{bot_id}` (one bot's
  sensor view). See §2.6. Additive — no change to the bot or spectator wire protocol.

## 2026-05-21 — REST control plane + configurable match parameters

- Replaced the `/admin` WebSocket with a REST control plane under `/api/*`. Admin auth is now `POST /api/login` (password → JWT); mutating routes require an `Authorization: Bearer <jwt>` header. Lifecycle actions moved to `POST /api/room/{start,abort,reset,kick}`; room state is `GET /api/room` (public). See §2.5.
- Removed the server's stdin command interface — every operator action goes through `/api/*`.
- Added per-match balance parameters (`SimConfig`): ship / weapon / sensor tunables are editable in the lobby via `PUT /api/room/config` and described by `GET /api/config/schema`. `GET /api/room` now includes the active `config`.
- Added `GET /api/room/report` — a post-match summary (winner, duration, per-bot shots / hits / damage / kills). `404` until the first match finishes.
- Replay log format bumped to **v2**: the header now records the match's `sim_config` so a replay rebuilds with the exact parameters. v1 logs are no longer readable.
- The admin password is set with `--admin-password` / `BATTLE_ADMIN_PASSWORD` (random per start if unset, logged once at `INFO`); token lifetime via `--token-ttl-hours` (default 12). The `--admin-token` flag is gone.
- The `/bot` and `/spectate` WebSocket protocols are unchanged — existing bots need no update.

## 2026-05-16 — admin lifecycle plane + bots survive `game_over`

- Added `/admin` WebSocket endpoint gated by a rotating token (logged at startup, overridable with `--admin-token`). Client → server: `start`, `abort`, `reset`, `kick`. Server → client: `state` snapshots on every transition, plus `ack` / `error`.
- Added server → bot `lobby` message. After every `game_over` the room auto-returns to `Lobby` (~2 s post-game pause), reseeds the RNG from `--seed`, and emits `lobby` to every connected bot. Bots stay connected across matches; `bot_id` and `ship_id` are stable per connection.
- Python SDK: `Bot.on_game_over` now returns `Optional[bool]` (return `False` to disconnect, default `True`). New `Bot.on_lobby` hook. SDK auto-sends `ready` on `lobby`.
- Backwards compatible for bots that subclass with defaults — they automatically participate in subsequent matches.

## 2026-05-12 — richer spectator world frames

- `ships[]` in the `world` payload now carries `speed`, `ammo`, `throttle`, `rudder`, `ready`, and `commands_per_sec` in addition to the existing fields. Backwards compatible: existing spectator clients can ignore them.

## 2026-05-12 — security hardening

- Added error codes: `invalid_name`, `stale_command`, `non_finite_value`, `handshake_timeout`.
- `hello.name` is now validated against `[A-Za-z0-9 _-]{1,32}` (returns `invalid_name`). A name that duplicates another live bot in the same room is rejected at registration time and surfaces as `invalid_message`.
- `command.tick` must be within `world_tick ± 1`; otherwise the command is rejected as `stale_command` and the previous controls persist.
- Commands with `NaN` / `Inf` floats are rejected as `non_finite_value` and count toward the 5-violation budget.
- WebSocket message and frame size are capped at 16 KiB.
- The HTTP head and the post-upgrade `hello` each have a 5-second timeout (configurable via `--handshake-timeout-secs`), enforced by `handshake_timeout`.
- A per-IP cap on simultaneous TCP connections is enforced at accept time (`--max-connections-per-ip`, default 25; set to 0 to disable). Connections beyond the cap are dropped before the WebSocket handshake — no error frame is sent.
- `--tournament` restricts the `/spectate` endpoint to the loopback interface, preventing competing bots from subscribing to ground-truth world state.
- Duplicate `cooldown_active` / `no_ammo` errors are coalesced to one per tick to protect the bot's 32-slot outbound buffer from spam.
