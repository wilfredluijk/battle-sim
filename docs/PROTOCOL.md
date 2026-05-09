# Wire Protocol

Public contract between the naval-battle server and bot / spectator clients. This doc and `server/src/protocol.rs` are mirrors — when one changes, the other changes in the same commit.

- **Transport:** WebSocket over TCP.
- **Encoding:** UTF-8 JSON, one object per text frame, no batching, no binary frames.
- **Endpoints:**
  - `ws://<host>:<port>/bot` — bidirectional, untrusted.
  - `ws://<host>:<port>/spectate` — server-to-client only.
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
| `name` | string | Display name. Server may suffix to disambiguate duplicates. |
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
  "map": { "width": 1000, "height": 1000 },
  "tick_hz": 10,
  "ship_specs": {
    "max_forward_speed": 6.0,
    "max_reverse_speed": 2.0,
    "acceleration": 1.5,
    "turn_rate_deg_per_s": 15.0,
    "hull_hp": 100,
    "max_ammo": 20,
    "gun_cooldown_ticks": 15,
    "hit_radius": 8.0,
    "shell_speed": 50.0,
    "max_shell_range": 300.0,
    "splash_radius": 15.0,
    "max_splash_damage": 25
  }
}
```

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
Final message on the bot connection. `winner` is `null` for a draw.

```json
{
  "type": "game_over",
  "winner": "b_3",
  "final_tick": 1843,
  "replay_id": "match_20260508_171203"
}
```

#### `error`
Sent in response to a malformed or otherwise-rejected bot frame. See [§3 Error codes](#3-error-codes).

```json
{ "type": "error", "code": "late_command", "message": "command for tick 142 arrived after deadline" }
```

### 1.3 Late and missing commands

If a `command` arrives after the per-tick deadline, the server replies with `error` (`code: "late_command"`) and applies the previous tick's `throttle` / `rudder` / `sensor_mode`. No shot is fired that tick. The bot is **not** disconnected for missing or late commands — only for repeated protocol violations.

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
      "hp": 78,
      "alive": true
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
| `cooldown_active` | `fire` was issued while the gun was still cooling down. |

After 5 protocol violations on a single bot connection, the server sends `too_many_violations` and closes with WebSocket close code `Policy (1008)`.

---

## 4. Versioning

The server's release version is included in `welcome.version` (planned — currently absent in MVP). Additive changes (new optional fields, new event types) are backwards-compatible. Renamed or removed fields, type changes, and changed semantics are breaking and will bump the version sent in `welcome`.

---

## Changelog

<!-- Each entry: ## YYYY-MM-DD — version. List additions / changes / removals. -->
