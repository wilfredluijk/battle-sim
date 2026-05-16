# Wire Protocol

Public contract between the naval-battle server and bot / spectator clients. This doc and `server/src/protocol.rs` are mirrors — when one changes, the other changes in the same commit.

- **Transport:** WebSocket over TCP.
- **Encoding:** UTF-8 JSON, one object per text frame, no batching, no binary frames.
- **Endpoints:**
  - `ws://<host>:<port>/bot` — bidirectional, untrusted.
  - `ws://<host>:<port>/spectate` — server-to-client only.
  - `ws://<host>:<port>/admin?token=<TOKEN>` — bidirectional, gated by a rotating token (printed at INFO on server start, overridable with `--admin-token`).
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
| `name` | string | 1–32 bytes of `[A-Za-z0-9 _-]`. An invalid name is rejected with `invalid_name`; a duplicate of another live bot in the same room is rejected with `invalid_message`. Both close the connection. |
| `version` | string | SDK version, free-form. |

#### `ready`
Sent after `welcome` is received and the bot is willing to start.

```json
{ "type": "ready" }
```

#### `command`
Sent once per tick in response to a `tick` message. Echo the tick number from the `tick` frame.

```json
{
  "type": "command",
  "tick": 142,
  "throttle": 0.8,
  "rudder": -0.3,
  "fire": { "bearing_deg": 47.5, "range": 300.0 },
  "sensor_mode": "active"
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
  }
}
```

Field values shown above are the current defaults; the live `welcome` payload always reflects whatever the server is actually running. The runtime authority for these constants is `server/src/sim/constants.rs` — `ship_specs` is derived from there.

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
    "throttle": 0.8
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
    { "type": "shell_splash", "pos": [220.0, 505.0] }
  ]
}
```

`contacts[].id` is a per-tick contact ID, **not** the underlying `ship_id`. Trackers must do their own data association across ticks.

`contacts[].range` is omitted when not measured (passive mode).

`contacts[].kind` is one of `"ship"`, `"shell"`, `"unknown"`.

`events[]` only contains things this bot can perceive: own hits and splashes inside its sensor range.

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
hello → welcome → (ready) → game_start → tick* → game_over →
                  (lobby) → (ready) → game_start → tick* → game_over → …
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
      "sensor_mode": "active"
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
    { "type": "death", "ship_id": "s_2" }
  ]
}
```

`shells[].id_index` is a stable-per-shell index used by renderers to track trails across ticks.

`ships[].sensor_mode` is the bot's most recently commanded mode (`"active"` or `"passive"`); the renderer uses it to draw the active-radar ring.

`ships[].speed` is the ship's signed scalar velocity (positive = ahead, negative = astern); `throttle` and `rudder` are the last commanded control values, both in `[-1.0, 1.0]`.

`ships[].ready` is the lobby-readiness flag — `true` once the bot has sent `ready`. It stays `true` for the rest of the match.

`ships[].commands_per_sec` is the number of `command` frames the room accepted from this bot over the last second of sim time (i.e. the last `tick_hz` ticks). The value is zero in the lobby and ticks up once the match is running. Late, stale, or violation-rejected commands do not count.

---

## 2.5 Admin endpoint — `/admin`

Bidirectional control channel for an operator UI. Authentication: every connection MUST carry the current admin token as a query parameter — e.g. `ws://host:port/admin?token=AbCd...`. The server validates the token before completing the WebSocket upgrade; a missing or wrong token returns HTTP 401.

The token is generated at startup and printed to the log at `INFO` (look for the `admin token` line) unless `--admin-token <TOKEN>` overrides it. Tokens rotate on every server restart.

### 2.5.1 Server → Admin

#### `state`
Pushed immediately on subscribe and on every lifecycle transition (bot connect/disconnect/ready, match start, match end, lobby return).

```json
{
  "type": "state",
  "room": "main",
  "state": "lobby",
  "tick": 0,
  "last_winner": null,
  "bots": [
    { "bot_id": "b_1", "name": "alice", "ship_id": "s_1", "ready": true, "alive": true }
  ]
}
```

| Field | Type | Notes |
|---|---|---|
| `state` | `"lobby"` \| `"running"` \| `"ended"` | Room state machine. `ended` is the post-game pause; the room returns to `lobby` automatically after ~2s. |
| `tick` | u64 | Current `world.tick`. |
| `last_winner` | string \| null | `bot_id` of the most recent winner, or `null` for a draw / abort / fresh match. |
| `bots[].alive` | bool | Tracks the ship's `alive` flag for the running match; `true` in lobby. |

#### `ack`
Optional acknowledgement for a command. Use `state` pushes for authoritative truth.

```json
{ "type": "ack", "command": "start" }
```

#### `error`
Returned when the server rejects a command. Codes: `start_refused`, `not_running`, `not_ended`, `unknown_bot`, `malformed_json`, `invalid_message`.

```json
{ "type": "error", "code": "not_running", "message": "room is not running" }
```

### 2.5.2 Admin → Server

```json
{ "type": "start" }
{ "type": "abort" }
{ "type": "reset" }
{ "type": "kick", "bot_id": "b_3" }
```

| Command | Effect |
|---|---|
| `start` | Lobby → Running. Refused if not in Lobby, no bots, or not all ready. |
| `abort` | Force-end the running match (`game_over` with `winner: null`). |
| `reset` | Ended → Lobby immediately, skipping the post-game pause. Refused outside Ended. |
| `kick` | Disconnect a single bot by `bot_id`. |

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
| `stale_command` | `command.tick` was outside the accepted window (`world_tick ± 1`). |
| `non_finite_value` | A command contained `NaN` or `Inf` in `throttle`, `rudder`, or `fire.{bearing_deg,range}`. |
| `handshake_timeout` | The bot connected but did not send `hello` within the handshake timeout. The connection is closed. |

After 5 protocol violations on a single bot connection, the server sends `too_many_violations` and closes with WebSocket close code `Policy (1008)`. The violation-counted codes are `malformed_json`, `invalid_message`, `non_finite_value`, and `binary_frames_unsupported`. `handshake_timeout` and `invalid_name` close the connection on the first occurrence and bypass the counter. `late_command`, `stale_command`, `cooldown_active`, and `no_ammo` are gameplay rejections and do not count against the bot.

Two further server-side limits are enforced without a dedicated error code:

- **Duplicate `hello.name`** — when another live bot in the room already holds the name, registration fails and the server sends `invalid_message` (with the rejection reason in `message`), then closes the connection.
- **Per-IP connection cap** — when the peer IP is at `--max-connections-per-ip`, the TCP stream is dropped *before* the WebSocket handshake completes. The bot observes a connection close with no error frame.

WebSocket messages are capped at 16 KiB. The `/spectate` endpoint can be restricted to the loopback interface with `--tournament` so competing bots cannot use it to bypass the sensor filter.

---

## 4. Versioning

The server's release version is included in `welcome.version` (planned — currently absent in MVP). Additive changes (new optional fields, new event types) are backwards-compatible. Renamed or removed fields, type changes, and changed semantics are breaking and will bump the version sent in `welcome`.

---

## Changelog

<!-- Each entry: ## YYYY-MM-DD — version. List additions / changes / removals. -->

## 2026-05-16 — admin lifecycle plane + bots survive `game_over`

- Added `/admin` WebSocket endpoint gated by a rotating token (logged at startup, overridable with `--admin-token`). Client → server: `start`, `abort`, `reset`, `kick`. Server → client: `state` snapshots on every transition, plus `ack` / `error`.
- Added server → bot `lobby` message. After every `game_over` the room auto-returns to `Lobby` (~2 s post-game pause), reseeds the RNG from `--seed`, and emits `lobby` to every connected bot. Bots stay connected across matches; `bot_id` and `ship_id` are stable per connection.
- Python SDK: `Bot.on_game_over` now returns `Optional[bool]` (return `False` to disconnect, default `True`). New `Bot.on_lobby` hook. SDK auto-sends `ready` on `lobby`.
- Java SDK: `Bot.onGameOver` now returns `boolean`. New `Bot.onLobby` hook. SDK auto-sends `ready` on `lobby`.
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
- A per-IP cap on simultaneous TCP connections is enforced at accept time (`--max-connections-per-ip`, default 8; set to 0 to disable). Connections beyond the cap are dropped before the WebSocket handshake — no error frame is sent.
- `--tournament` restricts the `/spectate` endpoint to the loopback interface, preventing competing bots from subscribing to ground-truth world state.
- Duplicate `cooldown_active` / `no_ammo` errors are coalesced to one per tick to protect the bot's 32-slot outbound buffer from spam.
